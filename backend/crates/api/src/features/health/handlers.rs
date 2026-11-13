//! HTTP handlers for the health feature.

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use super::models::{LivenessResponse, ReadinessResponse};
use crate::state::AppState;

/// Reports whether the HTTP process is alive and able to serve requests.
pub(super) async fn live() -> Json<LivenessResponse> {
    Json(LivenessResponse { status: "ok" })
}

/// Reports whether every required external dependency is currently reachable.
///
/// The handler returns `200 OK` when all checks pass and `503 Service Unavailable`
/// with per-dependency state when any check fails or times out.
pub(super) async fn ready(State(state): State<AppState>) -> Response {
    let report = state.health.check(&state.database).await;
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
    use crate::{
        app,
        config::{DatabaseSettings, DatabaseTlsMode},
        infrastructure::DatabaseClient,
        state::AppState,
    };

    #[derive(Clone)]
    struct StubHealthService(ReadinessReport);

    #[async_trait]
    impl HealthService for StubHealthService {
        async fn check(&self, _database: &DatabaseClient) -> ReadinessReport {
            self.0.clone()
        }
    }

    async fn state_with(statuses: DependencyStatuses) -> AppState {
        let database = DatabaseClient::initialize(&DatabaseSettings {
            url: "mysql://test:test@localhost/test".to_owned(),
            min_connections: 0,
            max_connections: 1,
            connect_timeout: std::time::Duration::from_secs(1),
            acquire_timeout: std::time::Duration::from_secs(1),
            query_timeout: std::time::Duration::from_secs(1),
            tls_mode: DatabaseTlsMode::Disabled,
            tls_ca_path: None,
        })
        .await
        .expect("lazy test database should initialize");
        AppState::new(
            database,
            Arc::new(StubHealthService(ReadinessReport {
                dependencies: statuses,
            })),
        )
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
        let response =
            app::router(state_with(DependencyStatuses::all(DependencyState::Down)).await)
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
        let response = app::router(state_with(DependencyStatuses::all(DependencyState::Up)).await)
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
            let response = app::router(
                state_with(DependencyStatuses {
                    mysql,
                    redis,
                    opensearch,
                })
                .await,
            )
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
