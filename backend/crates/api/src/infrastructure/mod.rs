//! Shared infrastructure adapters.
//!
//! This layer creates reusable external-service clients. Feature-specific behavior
//! belongs in the feature that consumes those clients.

mod clients;

/// Lazily initialized MySQL, Redis, and OpenSearch clients.
pub use clients::Datastores;
