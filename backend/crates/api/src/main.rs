//! Backend API executable.
//!
//! This crate wires configuration, shared infrastructure, feature services, HTTP
//! routing, telemetry, and graceful shutdown into one Axum server process.
#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

mod app;
mod config;
mod error;
mod features;
mod infrastructure;
mod middleware;
mod shutdown;
mod state;
mod telemetry;

use std::sync::Arc;

use anyhow::Context;
use features::{auth::refresh_store::RedisRefreshSessionStore, health::DatastoreHealthService};
use infrastructure::Datastores;
use state::AppState;

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
    let health = Arc::new(DatastoreHealthService::new(
        datastores,
        settings.health_check_timeout,
    ));
    let app = app::router_with(
        AppState::new(database, redis, health, refresh_sessions),
        &settings.http,
    );

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
    let outcome =
        shutdown::serve_with_deadline(shutdown::signal(), shutdown_timeout, |trigger| async move {
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

    match tokio::time::timeout(shutdown_timeout, database_for_shutdown.close()).await {
        Ok(Ok(())) => tracing::info!("database pool closed"),
        Ok(Err(error)) => tracing::warn!(%error, "failed to close database pool"),
        Err(_) => tracing::warn!("timed out closing database pool"),
    }
    tracing::info!("shutdown complete");

    Ok(())
}
