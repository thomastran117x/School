use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use super::models::{LivenessResponse, ReadinessResponse};
use crate::state::AppState;

pub(super) async fn live() -> Json<LivenessResponse> {
    Json(LivenessResponse { status: "ok" })
}

pub(super) async fn ready(State(state): State<AppState>) -> Response {
    let report = state.health.check().await;
    let status = if report.is_ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status, Json(ReadinessResponse::from(report))).into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::super::{
        models::{DependencyState, DependencyStatuses, ReadinessReport},
        service::HealthService,
    };
    use crate::{app, state::AppState};

    #[derive(Clone)]
    struct StubHealthService(ReadinessReport);

    #[async_trait]
    impl HealthService for StubHealthService {
        async fn check(&self) -> ReadinessReport {
            self.0.clone()
        }
    }

    fn state_with(statuses: DependencyStatuses) -> AppState {
        AppState::new(Arc::new(StubHealthService(ReadinessReport {
            dependencies: statuses,
        })))
    }

    async fn response_json(response: axum::response::Response) -> Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("response body should be readable")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("response should be valid JSON")
    }

    #[tokio::test]
    async fn liveness_is_independent_of_dependencies() {
        let response = app::router(state_with(DependencyStatuses::all(DependencyState::Down)))
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await, json!({"status": "ok"}));
    }

    #[tokio::test]
    async fn readiness_succeeds_when_every_dependency_is_up() {
        let response = app::router(state_with(DependencyStatuses::all(DependencyState::Up)))
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            json!({
                "status": "ready",
                "dependencies": {
                    "mysql": "up",
                    "redis": "up",
                    "opensearch": "up"
                }
            })
        );
    }

    #[tokio::test]
    async fn readiness_identifies_each_failed_dependency_without_error_details() {
        for (mysql, redis, opensearch, failed_name) in [
            (
                DependencyState::Down,
                DependencyState::Up,
                DependencyState::Up,
                "mysql",
            ),
            (
                DependencyState::Up,
                DependencyState::Down,
                DependencyState::Up,
                "redis",
            ),
            (
                DependencyState::Up,
                DependencyState::Up,
                DependencyState::Down,
                "opensearch",
            ),
        ] {
            let response = app::router(state_with(DependencyStatuses {
                mysql,
                redis,
                opensearch,
            }))
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            let body = response_json(response).await;
            assert_eq!(body["status"], "not_ready");
            assert_eq!(body["dependencies"][failed_name], "down");
            assert!(!body.to_string().contains("error"));
        }
    }
}
