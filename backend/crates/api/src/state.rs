//! Dependencies shared with HTTP feature handlers.
//!
//! State contains service contracts rather than concrete adapters so feature routers
//! can be tested without external processes.

use std::sync::Arc;

use crate::{features::health::HealthService, infrastructure::DatabaseClient};

#[derive(Clone)]
/// Cloneable application state injected into Axum handlers.
pub struct AppState {
    /// Shared, bounded access to the application's MySQL pool.
    pub database: DatabaseClient,
    /// Health service used by liveness and readiness handlers.
    pub health: Arc<dyn HealthService>,
}

impl AppState {
    /// Creates application state from the provided feature services.
    pub fn new(database: DatabaseClient, health: Arc<dyn HealthService>) -> Self {
        Self { database, health }
    }
}
