use serde::Serialize;

#[derive(Serialize)]
pub(super) struct LivenessResponse {
    pub status: &'static str,
}

#[derive(Serialize)]
pub(super) struct ReadinessResponse {
    pub status: &'static str,
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
pub(crate) struct ReadinessReport {
    pub dependencies: DependencyStatuses,
}

impl ReadinessReport {
    pub fn is_ready(&self) -> bool {
        self.dependencies.mysql == DependencyState::Up
            && self.dependencies.redis == DependencyState::Up
            && self.dependencies.opensearch == DependencyState::Up
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DependencyStatuses {
    pub mysql: DependencyState,
    pub redis: DependencyState,
    pub opensearch: DependencyState,
}

#[cfg(test)]
impl DependencyStatuses {
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
pub(crate) enum DependencyState {
    Up,
    Down,
}

impl From<bool> for DependencyState {
    fn from(up: bool) -> Self {
        if up { Self::Up } else { Self::Down }
    }
}
