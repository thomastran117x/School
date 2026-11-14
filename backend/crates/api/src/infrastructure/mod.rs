//! Shared infrastructure adapters.
//!
//! This layer creates reusable external-service clients. Feature-specific behavior
//! belongs in the feature that consumes those clients.

mod cache;
mod clients;
mod database;
mod redis;
mod redis_keys;

/// General-purpose cache operations (raw, JSON, counters, atomic groups, scripts).
pub use cache::Cache;
/// Lazily initialized MySQL, Redis, and OpenSearch clients.
pub use clients::Datastores;
/// Bounded access to the shared MySQL pool and its sanitized operation errors.
pub use database::{DatabaseClient, DatabaseError};
/// Bounded access to the shared Redis connection and its sanitized errors.
pub use redis::{RedisClient, RedisError};
/// Validated Redis key construction, namespaces and the maximum TTL.
pub use redis_keys::{Key, KeyNamespace, MAX_TTL};
