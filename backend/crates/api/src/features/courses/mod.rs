//! Course catalogue feature.
//!
//! Currently contains the Redis cache-aside helpers used by catalogue reads; the course
//! service and routes that consume them are delivered separately.

// The cache is the contract for the catalogue read service and has no callers yet.
#[allow(dead_code, unused_imports)]
pub(crate) mod cache;
