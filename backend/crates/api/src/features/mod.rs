//! Product features exposed by the backend.
//!
//! Each child module is a vertical slice that owns its transport handlers, models,
//! service logic, and feature-local tests.

/// Course application persistence and decision transactions.
pub(crate) mod applications;
/// Liveness and datastore-readiness endpoints.
pub(crate) mod health;
