//! Backend API executable.
//!
//! This crate wires configuration, shared infrastructure, feature services, HTTP
//! routing, telemetry, and graceful shutdown into one Axum server process.
#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

mod app;
mod config;
mod features;
mod infrastructure;
mod shutdown;
mod state;
mod telemetry;

use std::sync::Arc;

use anyhow::Context;
use features::health::DatastoreHealthService;
use infrastructure::Datastores;
use state::AppState;

#[tokio::main]
/// Loads the application and serves HTTP requests until a shutdown signal arrives.
async fn main() -> anyhow::Result<()> {
    let settings = config::Settings::from_env().context("failed to load configuration")?;
    telemetry::init(&settings.log_filter).context("failed to initialize tracing")?;

    let datastores = Datastores::initialize(&settings)
        .await
        .context("failed to initialize datastore clients")?;
    let health = Arc::new(DatastoreHealthService::new(
        datastores,
        settings.health_check_timeout,
    ));
    let app = app::router(AppState::new(health));

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

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown::signal())
        .await
        .context("HTTP server stopped unexpectedly")?;

    Ok(())
}
