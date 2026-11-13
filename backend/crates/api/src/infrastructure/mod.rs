//! Shared infrastructure adapters.
//!
//! This layer creates reusable external-service clients. Feature-specific behavior
//! belongs in the feature that consumes those clients.

mod clients;
mod database;

/// Lazily initialized MySQL, Redis, and OpenSearch clients.
pub use clients::Datastores;
/// Bounded access to the shared MySQL pool and its sanitized operation errors.
pub use database::{DatabaseClient, DatabaseError};
