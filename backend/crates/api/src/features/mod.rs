//! Product features exposed by the backend.
//!
//! Each child module is a vertical slice that owns its transport handlers, models,
//! service logic, and feature-local tests.

/// Authentication building blocks: the refresh-session store and the identity model.
pub(crate) mod auth;
/// Learning courses: the MySQL course model and catalogue read caching.
pub(crate) mod courses;
/// Liveness and datastore-readiness endpoints.
pub(crate) mod health;
