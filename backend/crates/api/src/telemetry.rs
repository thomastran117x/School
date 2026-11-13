use anyhow::Context;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init(log_filter: &str) -> anyhow::Result<()> {
    let filter = EnvFilter::try_new(log_filter).context("invalid RUST_LOG filter")?;

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .try_init()
        .context("tracing subscriber was already initialized")
}
