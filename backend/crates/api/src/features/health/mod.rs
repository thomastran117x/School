//! Health feature.
//!
//! The feature exposes process liveness and dependency readiness without leaking
//! connection details or internal errors to callers.

mod handlers;
mod models;
mod service;

use axum::{Router, routing::get};

use crate::state::AppState;

/// Datastore-backed health implementation and the feature's service contract.
pub(crate) use service::{DatastoreHealthService, HealthService};

/// Readiness report types, exposed so router-level tests can stub the service.
#[cfg(test)]
pub(crate) use models::{DependencyState, DependencyStatuses, ReadinessReport};

/// Builds the health feature router mounted below `/health`.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/live", get(handlers::live))
        .route("/ready", get(handlers::ready))
}
