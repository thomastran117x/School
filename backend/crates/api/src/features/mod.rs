//! Product features exposed by the backend.
//!
//! Each child module is a vertical slice that owns its transport handlers, models,
//! service logic, and feature-local tests.

/// Authentication building blocks, starting with the refresh-session store.
pub(crate) mod auth;
/// Course catalogue read caching, starting with cache-aside helpers.
pub(crate) mod courses;
/// Liveness and datastore-readiness endpoints.
pub(crate) mod health;
