//! Dependencies shared with HTTP feature handlers.
//!
//! State contains service contracts rather than concrete adapters so feature routers
//! can be tested without external processes.

use std::sync::Arc;

use crate::features::health::HealthService;

#[derive(Clone)]
/// Cloneable application state injected into Axum handlers.
pub struct AppState {
    /// Health service used by liveness and readiness handlers.
    pub health: Arc<dyn HealthService>,
}

impl AppState {
    /// Creates application state from the provided feature services.
    pub fn new(health: Arc<dyn HealthService>) -> Self {
        Self { health }
    }
}
