//! Backend API executable.
//!
//! This crate wires configuration, shared infrastructure, feature services, HTTP
//! routing, telemetry, and graceful shutdown into one Axum server process.
#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

mod app;
mod config;
mod error;
mod extract;
mod features;
mod infrastructure;
mod metrics;
mod middleware;
mod shutdown;
mod state;
mod telemetry;

use std::sync::Arc;

use anyhow::Context;
use features::{
    auth::refresh_store::RedisRefreshSessionStore, courses::cache::CourseCache,
    health::DatastoreHealthService,
};
use infrastructure::Datastores;
use state::{AppState, AppStateParts};

#[tokio::main]
/// Loads the application and serves HTTP requests until a shutdown signal arrives.
async fn main() -> anyhow::Result<()> {
    let settings = config::Settings::from_env().context("failed to load configuration")?;
    telemetry::init(&settings).context("failed to initialize tracing")?;

    let datastores = Datastores::initialize(&settings)
        .await
        .context("failed to initialize datastore clients")?;
    let database = datastores.mysql.clone();
    let database_for_shutdown = database.clone();
    let redis = datastores.redis.clone();
    let refresh_sessions = Arc::new(RedisRefreshSessionStore::new(
        redis.clone(),
        settings.auth.clone(),
    ));
    let course_cache = Arc::new(CourseCache::new(
        redis.clone(),
        settings.course_cache.clone(),
    ));
    let opensearch = datastores.opensearch.clone();
    let metrics = metrics::Metrics::new().context("failed to build metrics recorder")?;
    let health = Arc::new(
        DatastoreHealthService::new(datastores, settings.health_check_timeout)
            .with_metrics(metrics.clone()),
    );
    let http_settings = settings.http.clone();
    let state = AppState::from_parts(AppStateParts {
        config: Arc::new(settings.clone()),
        database,
        redis,
        opensearch,
        health,
        refresh_sessions,
        course_cache,
        metrics: metrics.clone(),
    });
    let metrics_settings = settings.metrics.clone();
    let metrics_state = state.clone();
    let app = app::router_with(state, &http_settings);

    let bind_address = settings
        .server_address()
        .context("configured HTTP address is invalid")?;
    let listener = tokio::net::TcpListener::bind(bind_address)
        .await
        .context("failed to bind HTTP listener")?;
    let address = listener
        .local_addr()
        .context("failed to read HTTP listener address")?;
    tracing::info!(%address, "HTTP server listening");

    let shutdown_timeout = settings.shutdown_timeout;
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let metrics_server = if metrics_settings.enabled {
        let listener = tokio::net::TcpListener::bind(metrics_settings.address)
            .await
            .context("failed to bind metrics listener")?;
        tracing::info!(
            address = %metrics_settings.address,
            protected = metrics_settings.bearer_token.is_some(),
            "metrics server listening"
        );
        let router = metrics::router(metrics_state, metrics_settings.bearer_token.as_deref());
        let mut stop = stop_rx.clone();
        let upkeep = metrics.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                tick.tick().await;
                upkeep.run_upkeep();
            }
        });
        Some(tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stop.wait_for(|stopped| *stopped).await;
                })
                .await
        }))
    } else {
        None
    };

    let signal = async move {
        shutdown::signal().await;
        let _ = stop_tx.send(true);
    };
    let outcome = shutdown::serve_with_deadline(signal, shutdown_timeout, |trigger| async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(trigger)
            .await
    })
    .await;

    match outcome {
        Some(result) => result.context("HTTP server stopped unexpectedly")?,
        None => tracing::warn!(
            timeout_ms = u64::try_from(shutdown_timeout.as_millis()).unwrap_or(u64::MAX),
            "shutdown timeout elapsed; dropping remaining connections"
        ),
    }

    if let Some(server) = metrics_server {
        match tokio::time::timeout(shutdown_timeout, server).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => tracing::warn!(%error, "metrics server stopped with an error"),
            Ok(Err(error)) => tracing::warn!(%error, "metrics server task failed"),
            Err(_) => tracing::warn!("timed out stopping the metrics server"),
        }
    }

    match tokio::time::timeout(shutdown_timeout, database_for_shutdown.close()).await {
        Ok(Ok(())) => tracing::info!("database pool closed"),
        Ok(Err(error)) => tracing::warn!(%error, "failed to close database pool"),
        Err(_) => tracing::warn!("timed out closing database pool"),
    }
    tracing::info!("shutdown complete");

    Ok(())
}
