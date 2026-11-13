//! Product features exposed by the backend.
//!
//! Each child module is a vertical slice that owns its transport handlers, models,
//! service logic, and feature-local tests.

/// Liveness and datastore-readiness endpoints.
pub(crate) mod health;
