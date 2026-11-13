//! Structured application logging and tracing setup.

use anyhow::Context;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// Installs the process-wide tracing subscriber using an `EnvFilter` expression.
///
/// # Errors
///
/// Returns an error when `log_filter` is invalid or another subscriber has already
/// been installed in this process.
pub fn init(log_filter: &str) -> anyhow::Result<()> {
    let filter = EnvFilter::try_new(log_filter).context("invalid RUST_LOG filter")?;

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .try_init()
        .context("tracing subscriber was already initialized")
}
