//! Top-level HTTP application composition.
//!
//! Feature routers are mounted here alongside middleware that applies uniformly to
//! the entire API.

use axum::{
    Router, extract::MatchedPath, http::HeaderName, middleware::from_fn, response::IntoResponse,
};
use tower_http::{
    LatencyUnit,
    catch_panic::CatchPanicLayer,
    limit::RequestBodyLimitLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::{DefaultOnFailure, DefaultOnResponse, TraceLayer},
};
use tracing::{Level, Span};

use crate::{
    config::HttpSettings,
    error::AppError,
    features,
    middleware::{cors, error_pages, request_context, request_id, security_headers},
    state::AppState,
};

/// Builds the router with default (restrictive) HTTP hardening settings.
///
/// Equivalent to [`router_with`] using [`HttpSettings::default`]: no CORS origins, a
/// 1 MiB body limit and no HSTS.
#[cfg(test)]
pub fn router(state: AppState) -> Router {
    router_with(state, &HttpSettings::default())
}

/// Builds the complete Axum router with application state and shared middleware.
///
/// Layers, outermost first: incoming request ID validation, request ID assignment,
/// HTTP tracing, request ID
/// propagation, request ID scoping for error bodies, security headers, CORS, panic
/// catching, error-page normalization, then the request body limit.
pub fn router_with(state: AppState, http: &HttpSettings) -> Router {
    let request_id_header = HeaderName::from_static("x-request-id");

    let router = Router::new()
        .nest("/health", features::health::router())
        .fallback(error_pages::not_found)
        .with_state(state)
        .layer(RequestBodyLimitLayer::new(http.max_body_bytes))
        .layer(from_fn(error_pages::normalize))
        .layer(CatchPanicLayer::custom(|_| {
            AppError::internal("request handler panicked").into_response()
        }))
        .layer(cors::layer(&http.cors));

    security_headers::apply(router, http)
        .layer(from_fn(request_context::scope))
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(make_span)
                .on_response(
                    DefaultOnResponse::new()
                        .level(Level::INFO)
                        .latency_unit(LatencyUnit::Millis),
                )
                .on_failure(DefaultOnFailure::new().latency_unit(LatencyUnit::Millis)),
        )
        .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
        .layer(from_fn(request_id::sanitize))
}

/// Creates the per-request span.
///
/// Only the method, route template and request ID are recorded. The route is the
/// matched template (for example `/health/live`), or `unmatched` when no route
/// matched, so raw paths with identifiers are never logged. Headers, bodies and the
/// query string are deliberately excluded because they can carry credentials or
/// personal data.
fn make_span<B>(request: &axum::http::Request<B>) -> Span {
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown");
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str);
    tracing::info_span!(
        "http_request",
        method = %request.method(),
        route = %route,
        request_id = %request_id,
    )
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use async_trait::async_trait;
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode, header},
    };
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::{
        config::{
            AuthSettings, CorsSettings, DatabaseSettings, DatabaseTlsMode, RedisSettings,
            RedisTlsMode,
        },
        features::{
            auth::refresh_store::RedisRefreshSessionStore,
            health::{DependencyState, DependencyStatuses, HealthService, ReadinessReport},
        },
        infrastructure::{DatabaseClient, RedisClient},
    };

    struct StubHealth;

    #[async_trait]
    impl HealthService for StubHealth {
        async fn check(&self, _database: &DatabaseClient) -> ReadinessReport {
            ReadinessReport {
                dependencies: DependencyStatuses::all(DependencyState::Up),
            }
        }
    }

    async fn state() -> AppState {
        let database = DatabaseClient::initialize(&DatabaseSettings {
            url: "mysql://test:test@localhost/test".to_owned(),
            min_connections: 0,
            max_connections: 1,
            connect_timeout: Duration::from_secs(1),
            acquire_timeout: Duration::from_secs(1),
            query_timeout: Duration::from_secs(1),
            tls_mode: DatabaseTlsMode::Disabled,
            tls_ca_path: None,
        })
        .await
        .unwrap();
        let redis = RedisClient::initialize(&RedisSettings {
            url: "redis://localhost:6379".to_owned(),
            connect_timeout: Duration::from_secs(1),
            command_timeout: Duration::from_secs(1),
            tls_mode: RedisTlsMode::Disabled,
            tls_ca_path: None,
        })
        .await
        .unwrap();
        let sessions = Arc::new(RedisRefreshSessionStore::new(
            redis.clone(),
            AuthSettings {
                refresh_token_ttl: Duration::from_secs(60),
                refresh_family_max_lifetime: Duration::from_secs(120),
            },
        ));
        AppState::new(database, redis, Arc::new(StubHealth), sessions)
    }

    fn http(origins: &[&str], credentials: bool) -> HttpSettings {
        HttpSettings {
            max_body_bytes: 16,
            cors: CorsSettings {
                allowed_origins: origins.iter().map(|o| (*o).to_owned()).collect(),
                allow_credentials: credentials,
                ..CorsSettings::default()
            },
            ..HttpSettings::default()
        }
    }

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn preflight(origin: &str) -> Request<Body> {
        Request::builder()
            .method(Method::OPTIONS)
            .uri("/health/live")
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn preflight_from_an_allowed_origin_is_approved_without_wildcard() {
        let app = router_with(state().await, &http(&["http://localhost:3000"], true));

        let response = app
            .oneshot(preflight("http://localhost:3000"))
            .await
            .unwrap();

        assert!(response.status().is_success());
        let headers = response.headers();
        assert_eq!(
            headers[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "http://localhost:3000"
        );
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_CREDENTIALS], "true");
        assert!(
            headers[header::ACCESS_CONTROL_ALLOW_METHODS]
                .to_str()
                .unwrap()
                .contains("GET")
        );
    }

    #[tokio::test]
    async fn preflight_from_an_unknown_origin_gets_no_allow_origin() {
        let app = router_with(state().await, &http(&["http://localhost:3000"], true));

        let response = app
            .oneshot(preflight("https://evil.example"))
            .await
            .unwrap();

        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[tokio::test]
    async fn no_configured_origins_means_no_cors_headers() {
        let app = router_with(state().await, &http(&[], false));

        let response = app
            .oneshot(preflight("http://localhost:3000"))
            .await
            .unwrap();

        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[tokio::test]
    async fn oversized_bodies_are_rejected_with_413_json() {
        let app = router_with(state().await, &http(&[], false));
        let request = Request::builder()
            .method(Method::POST)
            .uri("/health/live")
            .header(header::CONTENT_LENGTH, "1024")
            .body(Body::from(vec![b'x'; 1024]))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "payload_too_large");
    }

    #[tokio::test]
    async fn unknown_routes_return_json_404_with_the_request_id() {
        let app = router_with(state().await, &http(&[], false));
        let request = Request::builder()
            .uri("/nope")
            .header("x-request-id", "req-42")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["x-request-id"], "req-42");
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "not_found");
        assert_eq!(body["error"]["request_id"], "req-42");
    }

    async fn request_id_header_for(id: Option<&str>) -> String {
        let mut builder = Request::builder().uri("/health/live");
        if let Some(id) = id {
            builder = builder.header("x-request-id", id);
        }
        let response = router_with(state().await, &http(&[], false))
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        response.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn missing_request_id_is_generated() {
        let id = request_id_header_for(None).await;
        assert!(request_id::is_valid_request_id(&id));
        assert_eq!(id.len(), 36);
    }

    #[tokio::test]
    async fn valid_request_id_is_echoed() {
        assert_eq!(
            request_id_header_for(Some("req-42.a_b")).await,
            "req-42.a_b"
        );
    }

    #[tokio::test]
    async fn invalid_request_ids_are_replaced() {
        for bad in ["<script>", "has space", &"a".repeat(65)] {
            let id = request_id_header_for(Some(bad)).await;
            assert_ne!(id, bad);
            assert_eq!(id.len(), 36, "expected a generated UUID for {bad:?}");
        }
    }

    #[derive(Clone, Default)]
    struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
        type Writer = LogBuffer;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Sends `uri` through the app and returns everything that was logged.
    async fn capture_logs(uri: &str) -> String {
        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .with_max_level(Level::INFO)
            .finish();
        let app = router_with(state().await, &http(&[], false));
        let _guard = tracing::subscriber::set_default(subscriber);
        // Parallel tests without a subscriber may have cached "never" for these callsites.
        tracing::callsite::rebuild_interest_cache();
        app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = buffer.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    /// Retries because tracing caches callsite interest process-wide: while other tests
    /// hit the same callsites with no subscriber, a scoped subscriber can miss the span
    /// (but not the response event) on an attempt.
    async fn logs_for(uri: &str) -> String {
        let mut logs = String::new();
        for _ in 0..50 {
            logs = capture_logs(uri).await;
            if logs.contains("method=") {
                break;
            }
            tokio::task::yield_now().await;
        }
        logs
    }

    /// One test, because scoped subscribers in parallel tests race on tracing's shared
    /// callsite interest cache and can swallow events.
    #[tokio::test]
    async fn logs_are_safe_and_carry_route_status_and_latency() {
        let logs = logs_for("/health/live?token=secret").await;
        assert!(logs.contains("method=GET"), "{logs}");
        assert!(logs.contains("route=/health/live"), "{logs}");
        assert!(logs.contains("status=200"), "{logs}");
        assert!(logs.contains("latency"), "{logs}");
        assert!(!logs.contains("secret"), "{logs}");
        assert!(!logs.contains("token"), "{logs}");

        let logs = logs_for("/nope/123?x=1").await;
        assert!(logs.contains("route=unmatched"), "{logs}");
        assert!(logs.contains("status=404"), "{logs}");
        assert!(!logs.contains("/nope/123"), "{logs}");
        assert!(!logs.contains("x=1"), "{logs}");
    }

    #[tokio::test]
    async fn wrong_method_returns_json_405_and_keeps_allow() {
        let app = router_with(state().await, &http(&[], false));
        let request = Request::builder()
            .method(Method::DELETE)
            .uri("/health/live")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert!(response.headers().contains_key(header::ALLOW));
        assert_eq!(
            body_json(response).await["error"]["code"],
            "method_not_allowed"
        );
    }

    #[tokio::test]
    async fn security_headers_are_present_and_hsts_requires_https() {
        let plain = router_with(state().await, &http(&[], false))
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(plain.headers()["x-content-type-options"], "nosniff");
        assert_eq!(plain.headers()["referrer-policy"], "no-referrer");
        assert!(!plain.headers().contains_key("strict-transport-security"));
        assert!(!plain.headers().contains_key("server"));

        let mut secure = http(&[], false);
        secure.https_enabled = true;
        let secure = router_with(state().await, &secure)
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            secure.headers()["strict-transport-security"],
            "max-age=31536000; includeSubDomains"
        );
    }
}
