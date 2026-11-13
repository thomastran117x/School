mod handlers;
mod models;
mod service;

use axum::{Router, routing::get};

use crate::state::AppState;

pub(crate) use service::{DatastoreHealthService, HealthService};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/live", get(handlers::live))
        .route("/ready", get(handlers::ready))
}
