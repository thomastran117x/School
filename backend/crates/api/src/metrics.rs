//! Prometheus metrics: request totals, latency, errors and dependency health.
//!
//! # Cardinality rules
//!
//! Every label value comes from a small fixed set, so a client can never grow the
//! number of time series:
//!
//! - `route` is the matched route template (for example `/health/live`) or the constant
//!   `unmatched`. Raw paths, path parameters, query strings and user identifiers are
//!   never used as labels.
//! - `method` is one of the standard HTTP methods, or `OTHER`.
//! - `status` is the numeric response status code; `status_class` is `2xx`..`5xx`.
//! - `dependency` is one of `mysql`, `redis` or `opensearch`.
//!
//! # Exposure
//!
//! Metrics are never served on the public API port. [`router`] is mounted on a separate
//! listener (`METRICS_ADDR`, loopback by default) and optionally requires a bearer token.
//! The recorder is local to [`Metrics`] rather than process-global, so tests stay isolated.

use std::{sync::Arc, time::Instant};

use axum::{
    Router,
    extract::{MatchedPath, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::get,
};
use metrics::{Gauge, Key, Label, Level, Metadata, Recorder, SharedString, Unit};
use metrics_exporter_prometheus::{
    BuildError, PrometheusBuilder, PrometheusHandle, PrometheusRecorder,
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::{
    error::AppError,
    features::health::{DependencyState, DependencyStatuses},
    middleware::error_pages,
    state::AppState,
};

/// Route label used when no route template matched the request.
const UNMATCHED_ROUTE: &str = "unmatched";

/// Latency buckets, in seconds, shared by every duration histogram.
const LATENCY_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Metadata attached to every registered metric.
const METADATA: Metadata<'static> = Metadata::new(module_path!(), Level::INFO, None);

const REQUESTS_TOTAL: &str = "http_requests_total";
const REQUEST_DURATION: &str = "http_request_duration_seconds";
const REQUEST_ERRORS: &str = "http_request_errors_total";
const REQUESTS_IN_FLIGHT: &str = "http_requests_in_flight";
const DEPENDENCY_UP: &str = "dependency_up";
const PROBE_DURATION: &str = "dependency_probe_duration_seconds";
const PROBE_FAILURES: &str = "dependency_probe_failures_total";

/// Handle for recording and rendering application metrics.
///
/// Cloning is cheap; all clones share one recorder.
#[derive(Clone)]
pub struct Metrics {
    recorder: Arc<PrometheusRecorder>,
    handle: PrometheusHandle,
}

impl Metrics {
    /// Creates an isolated recorder with the application's metric descriptions.
    ///
    /// # Errors
    ///
    /// Returns [`BuildError`] if the histogram buckets are rejected.
    pub fn new() -> Result<Self, BuildError> {
        let recorder = PrometheusBuilder::new()
            .set_buckets(LATENCY_BUCKETS)?
            .build_recorder();
        let handle = recorder.handle();
        for (name, unit, help) in [
            (
                REQUESTS_TOTAL,
                None,
                "Total HTTP requests by method, route template and status.",
            ),
            (
                REQUEST_DURATION,
                Some(Unit::Seconds),
                "HTTP request latency by method, route template and status class.",
            ),
            (
                REQUEST_ERRORS,
                None,
                "HTTP responses with a 4xx or 5xx status by route template.",
            ),
            (
                REQUESTS_IN_FLIGHT,
                None,
                "HTTP requests currently being handled.",
            ),
            (
                DEPENDENCY_UP,
                None,
                "Whether a dependency passed its last probe (1) or not (0).",
            ),
            (
                PROBE_DURATION,
                Some(Unit::Seconds),
                "Duration of dependency health probes.",
            ),
            (
                PROBE_FAILURES,
                None,
                "Dependency health probes that failed or timed out.",
            ),
        ] {
            let help = SharedString::from(help);
            match name {
                REQUEST_DURATION | PROBE_DURATION => {
                    recorder.describe_histogram(name.into(), unit, help);
                }
                REQUESTS_IN_FLIGHT | DEPENDENCY_UP => {
                    recorder.describe_gauge(name.into(), unit, help);
                }
                _ => recorder.describe_counter(name.into(), unit, help),
            }
        }
        Ok(Self {
            recorder: Arc::new(recorder),
            handle,
        })
    }

    /// Renders all metrics in the Prometheus text exposition format.
    pub fn render(&self) -> String {
        self.handle.run_upkeep();
        self.handle.render()
    }

    /// Drains histogram buffers. Call periodically so memory stays bounded.
    pub fn run_upkeep(&self) {
        self.handle.run_upkeep();
    }

    /// Records a finished HTTP request.
    fn record_request(&self, method: &'static str, route: &str, status: StatusCode, seconds: f64) {
        let class = status_class(status);
        self.recorder
            .register_counter(
                &key(
                    REQUESTS_TOTAL,
                    [
                        ("method", method),
                        ("route", route),
                        ("status", status.as_str()),
                    ],
                ),
                &METADATA,
            )
            .increment(1);
        self.recorder
            .register_histogram(
                &key(
                    REQUEST_DURATION,
                    [
                        ("method", method),
                        ("route", route),
                        ("status_class", class),
                    ],
                ),
                &METADATA,
            )
            .record(seconds);
        if status.is_client_error() || status.is_server_error() {
            self.recorder
                .register_counter(
                    &key(REQUEST_ERRORS, [("route", route), ("status_class", class)]),
                    &METADATA,
                )
                .increment(1);
        }
    }

    /// Returns the in-flight gauge.
    fn in_flight(&self) -> Gauge {
        self.recorder
            .register_gauge(&key(REQUESTS_IN_FLIGHT, []), &METADATA)
    }

    /// Records one dependency probe. `dependency` must be a fixed, known name.
    pub fn record_probe(&self, dependency: &'static str, seconds: f64, success: bool) {
        self.recorder
            .register_histogram(
                &key(PROBE_DURATION, [("dependency", dependency)]),
                &METADATA,
            )
            .record(seconds);
        if !success {
            self.recorder
                .register_counter(
                    &key(PROBE_FAILURES, [("dependency", dependency)]),
                    &METADATA,
                )
                .increment(1);
        }
    }

    /// Sets the `dependency_up` gauge for every dependency from a readiness report.
    fn set_dependency_up(&self, statuses: &DependencyStatuses) {
        for (dependency, up) in [
            ("mysql", statuses.mysql == DependencyState::Up),
            ("redis", statuses.redis == DependencyState::Up),
            ("opensearch", statuses.opensearch == DependencyState::Up),
        ] {
            self.recorder
                .register_gauge(&key(DEPENDENCY_UP, [("dependency", dependency)]), &METADATA)
                .set(if up { 1.0 } else { 0.0 });
        }
    }
}

/// Builds a metric key from a name and labels.
fn key<const N: usize>(name: &'static str, labels: [(&'static str, &str); N]) -> Key {
    Key::from_parts(
        name,
        labels
            .into_iter()
            .map(|(name, value)| Label::new(name, value.to_owned()))
            .collect::<Vec<_>>(),
    )
}

/// Maps a status code to `1xx`..`5xx`.
fn status_class(status: StatusCode) -> &'static str {
    match status.as_u16() / 100 {
        1 => "1xx",
        2 => "2xx",
        3 => "3xx",
        4 => "4xx",
        _ => "5xx",
    }
}

/// Restricts the method label to the standard methods.
fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::HEAD => "HEAD",
        Method::OPTIONS => "OPTIONS",
        _ => "OTHER",
    }
}

/// Decrements the in-flight gauge when dropped, including when a request is cancelled.
struct InFlight(Gauge);

impl InFlight {
    /// Increments the gauge and returns a guard that undoes it.
    fn enter(metrics: &Metrics) -> Self {
        let gauge = metrics.in_flight();
        gauge.increment(1.0);
        Self(gauge)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.decrement(1.0);
    }
}

/// Middleware recording totals, latency, errors and in-flight requests.
///
/// It runs after routing, so the matched route template is available, and outside the
/// panic and error-page layers, so it sees the final status sent to the client.
pub async fn record_request(
    State(metrics): State<Metrics>,
    request: Request,
    next: Next,
) -> Response {
    let route = request.extensions().get::<MatchedPath>().map_or_else(
        || UNMATCHED_ROUTE.to_owned(),
        |path| path.as_str().to_owned(),
    );
    let method = method_label(request.method());
    let started = Instant::now();
    let guard = InFlight::enter(&metrics);

    let response = next.run(request).await;

    drop(guard);
    metrics.record_request(
        method,
        &route,
        response.status(),
        started.elapsed().as_secs_f64(),
    );
    response
}

/// Hashed bearer token required to scrape metrics, if one is configured.
#[derive(Clone)]
struct ScrapeAuth(Option<Arc<[u8; 32]>>);

impl ScrapeAuth {
    /// Hashes the configured token so comparisons are constant time and fixed length.
    fn new(token: Option<&str>) -> Self {
        Self(token.map(|token| Arc::new(Sha256::digest(token).into())))
    }

    /// Returns whether `authorization` carries the configured bearer token.
    fn allows(&self, authorization: Option<&HeaderValue>) -> bool {
        let Some(expected) = &self.0 else {
            return true;
        };
        let presented = authorization
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or("");
        let presented: [u8; 32] = Sha256::digest(presented).into();
        presented.ct_eq(expected.as_ref()).into()
    }
}

/// Rejects scrape requests that lack the configured bearer token.
async fn require_token(State(auth): State<ScrapeAuth>, request: Request, next: Next) -> Response {
    if auth.allows(request.headers().get(header::AUTHORIZATION)) {
        return next.run(request).await;
    }
    let mut response = AppError::Unauthorized.into_response();
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

/// Probes dependencies, refreshes `dependency_up` and renders all metrics.
async fn scrape(State(state): State<AppState>) -> Response {
    let report = state.health.check(&state.database).await;
    state.metrics.set_dependency_up(&report.dependencies);
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.render(),
    )
        .into_response()
}

/// Builds the router served on the dedicated metrics listener.
///
/// It exposes only `GET /metrics`, guarded by `token` when one is configured. It is
/// never merged into the public API router, and its own traffic is not counted.
pub fn router(state: AppState, token: Option<&str>) -> Router {
    Router::new()
        .route("/metrics", get(scrape))
        .fallback(error_pages::not_found)
        .with_state(state)
        .layer(from_fn_with_state(ScrapeAuth::new(token), require_token))
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::{
        app,
        features::health::{DependencyState, HealthService, ReadinessReport},
        infrastructure::DatabaseClient,
    };

    struct StubHealth(DependencyState);

    #[async_trait]
    impl HealthService for StubHealth {
        async fn check(&self, _database: &DatabaseClient) -> ReadinessReport {
            ReadinessReport {
                dependencies: DependencyStatuses::all(self.0),
            }
        }
    }

    async fn state(health: DependencyState) -> AppState {
        crate::state::test_support::lazy_state(Arc::new(StubHealth(health))).await
    }

    async fn get_uri(app: Router, uri: &str, auth: Option<&str>) -> (StatusCode, String) {
        let mut request = Request::builder().uri(uri);
        if let Some(auth) = auth {
            request = request.header(header::AUTHORIZATION, auth);
        }
        let response = app
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn requests_are_recorded_by_route_template_and_status() {
        let state = state(DependencyState::Up).await;
        let metrics = state.metrics.clone();
        let app = app::router(state);

        get_uri(app.clone(), "/health/live", None).await;
        get_uri(app.clone(), "/users/12345/profile?token=secret", None).await;

        let output = metrics.render();
        assert!(
            output.contains(
                r#"http_requests_total{method="GET",route="/health/live",status="200"} 1"#
            ),
            "{output}"
        );
        assert!(
            output
                .contains(r#"http_requests_total{method="GET",route="unmatched",status="404"} 1"#),
            "{output}"
        );
        assert!(
            output.contains(r#"http_request_errors_total{route="unmatched",status_class="4xx"} 1"#),
            "{output}"
        );
        assert!(
            output.contains("http_request_duration_seconds_bucket"),
            "{output}"
        );
        assert!(output.contains("http_requests_in_flight 0"), "{output}");
        for leaked in ["12345", "users", "secret", "token"] {
            assert!(!output.contains(leaked), "label leaked {leaked}: {output}");
        }
    }

    #[tokio::test]
    async fn unusual_methods_collapse_into_other() {
        let state = state(DependencyState::Up).await;
        let metrics = state.metrics.clone();
        let request = Request::builder()
            .method("BREW")
            .uri("/health/live")
            .body(Body::empty())
            .unwrap();

        app::router(state).oneshot(request).await.unwrap();

        assert!(metrics.render().contains(r#"method="OTHER""#));
    }

    #[tokio::test]
    async fn scrape_reports_dependency_health_and_is_not_self_counted() {
        let up = state(DependencyState::Up).await;
        let (status, body) = get_uri(router(up.clone(), None), "/metrics", None).await;
        assert_eq!(status, StatusCode::OK);
        for dependency in ["mysql", "redis", "opensearch"] {
            assert!(
                body.contains(&format!(r#"dependency_up{{dependency="{dependency}"}} 1"#)),
                "{body}"
            );
        }
        assert!(!body.contains("/metrics"), "{body}");

        let down = state(DependencyState::Down).await;
        let (_, body) = get_uri(router(down, None), "/metrics", None).await;
        assert!(
            body.contains(r#"dependency_up{dependency="redis"} 0"#),
            "{body}"
        );
    }

    #[tokio::test]
    async fn token_is_required_when_configured() {
        let state = state(DependencyState::Up).await;
        let app = || router(state.clone(), Some("s3cret"));

        let (status, body) = get_uri(app(), "/metrics", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!body.contains("s3cret"));
        let (status, _) = get_uri(app(), "/metrics", Some("Bearer wrong")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = get_uri(app(), "/metrics", Some("s3cret")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = get_uri(app(), "/metrics", Some("Bearer s3cret")).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_listener_serves_nothing_else() {
        let state = state(DependencyState::Up).await;
        let (status, _) = get_uri(router(state, None), "/health/live", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_public_router_does_not_expose_metrics() {
        let state = state(DependencyState::Up).await;
        let (status, _) = get_uri(app::router(state), "/metrics", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn probes_are_recorded_with_failures() {
        let metrics = Metrics::new().unwrap();
        metrics.record_probe("redis", 0.01, true);
        metrics.record_probe("redis", 0.02, false);

        let output = metrics.render();
        assert!(output.contains(r#"dependency_probe_failures_total{dependency="redis"} 1"#));
        assert!(
            output.contains(r#"dependency_probe_duration_seconds_count{dependency="redis"} 2"#)
        );
    }
}
