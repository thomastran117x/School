//! Transport and service models owned by the health feature.

use serde::Serialize;

#[derive(Serialize)]
/// JSON body returned by the liveness endpoint.
pub(super) struct LivenessResponse {
    /// Stable process state, currently always `"ok"`.
    pub status: &'static str,
}

#[derive(Serialize)]
/// JSON body returned by the readiness endpoint.
pub(super) struct ReadinessResponse {
    /// Overall state: `"ready"` or `"not_ready"`.
    pub status: &'static str,
    /// Reachability of each required datastore.
    pub dependencies: DependencyStatuses,
}

impl From<ReadinessReport> for ReadinessResponse {
    fn from(report: ReadinessReport) -> Self {
        Self {
            status: if report.is_ready() {
                "ready"
            } else {
                "not_ready"
            },
            dependencies: report.dependencies,
        }
    }
}

#[derive(Clone, Debug)]
/// Internal result produced by a complete readiness check.
pub(crate) struct ReadinessReport {
    /// Reachability of each required datastore.
    pub dependencies: DependencyStatuses,
}

impl ReadinessReport {
    /// Returns `true` only when every required datastore is up.
    pub fn is_ready(&self) -> bool {
        self.dependencies.mysql == DependencyState::Up
            && self.dependencies.redis == DependencyState::Up
            && self.dependencies.opensearch == DependencyState::Up
    }
}

#[derive(Clone, Debug, Serialize)]
/// Readiness state for every datastore required by the API.
pub(crate) struct DependencyStatuses {
    /// MySQL readiness state.
    pub mysql: DependencyState,
    /// Redis readiness state.
    pub redis: DependencyState,
    /// OpenSearch readiness state.
    pub opensearch: DependencyState,
}

#[cfg(test)]
impl DependencyStatuses {
    /// Creates test statuses in which every dependency has the same state.
    pub fn all(state: DependencyState) -> Self {
        Self {
            mysql: state,
            redis: state,
            opensearch: state,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Publicly reportable state of one required dependency.
pub(crate) enum DependencyState {
    /// The dependency responded successfully within its timeout.
    Up,
    /// The dependency failed or did not respond before its timeout.
    Down,
}

impl From<bool> for DependencyState {
    fn from(up: bool) -> Self {
        if up { Self::Up } else { Self::Down }
    }
}
