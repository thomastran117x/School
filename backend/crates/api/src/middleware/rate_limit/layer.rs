//! Axum middleware applying a per-IP policy.

use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::{
    limiter::{Decision, RateLimiter},
    policy::Policy,
};
use crate::error::AppError;

/// Subject used when the peer address is unavailable. Sharing one strict bucket is the
/// safe choice; in production `ConnectInfo` is always present.
const UNKNOWN_PEER: &str = "unknown";

/// State for [`limit_by_ip`]: the limiter plus the policy to apply.
#[derive(Clone)]
pub struct IpLimit {
    limiter: RateLimiter,
    policy: Policy,
}

impl RateLimiter {
    /// Creates middleware state applying `policy` per client IP.
    ///
    /// Use with `axum::middleware::from_fn_with_state(limiter.ip_limit(policy), limit_by_ip)`.
    /// On a nested router prefer `route_layer`, so unmatched paths are not counted
    /// against the stricter policy.
    pub fn ip_limit(&self, policy: Policy) -> IpLimit {
        IpLimit {
            limiter: self.clone(),
            policy,
        }
    }
}

/// Rejects the request with 429 or 503 unless the client IP has capacity left.
///
/// Health probes (`/health…`) are exempt so orchestration is never throttled.
pub async fn limit_by_ip(State(limit): State<IpLimit>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if path == "/health" || path.starts_with("/health/") {
        return next.run(request).await;
    }
    let subject = match request.extensions().get::<ConnectInfo<SocketAddr>>() {
        Some(ConnectInfo(peer)) => {
            let ip: IpAddr = limit
                .limiter
                .resolver()
                .resolve(peer.ip(), request.headers());
            ip.to_string()
        }
        None => UNKNOWN_PEER.to_owned(),
    };
    match limit.limiter.check(&limit.policy, &subject).await {
        Decision::Allowed => next.run(request).await,
        Decision::Limited { retry_after } => {
            AppError::TooManyRequests { retry_after }.into_response()
        }
        Decision::Unavailable { retry_after } => {
            AppError::ServiceUnavailable { retry_after }.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        middleware::from_fn_with_state,
        routing::get,
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::{
        config::{BucketSettings, RateLimitSettings},
        middleware::rate_limit::{cidr::Cidr, limiter::tests::offline_limiter},
    };

    fn settings(general: u32, auth: u32, trusted: &[&str]) -> RateLimitSettings {
        RateLimitSettings {
            general_ip: BucketSettings {
                capacity: general,
                refill_per_minute: 1,
            },
            auth_ip: BucketSettings {
                capacity: auth,
                refill_per_minute: 1,
            },
            trusted_proxies: trusted.iter().map(|c| c.parse::<Cidr>().unwrap()).collect(),
            ..RateLimitSettings::default()
        }
    }

    /// Builds an app with the general policy everywhere and the auth policy on `/login`.
    async fn app(settings: &RateLimitSettings) -> Router {
        let (limiter, _) = offline_limiter(settings).await;
        let policies = *limiter.policies();
        let login = Router::new()
            .route("/login", get(|| async { "ok" }))
            .route_layer(from_fn_with_state(
                limiter.ip_limit(policies.auth_ip),
                limit_by_ip,
            ));
        Router::new()
            .route("/other", get(|| async { "ok" }))
            .route("/health/live", get(|| async { "ok" }))
            .merge(login)
            .layer(from_fn_with_state(
                limiter.ip_limit(policies.general_ip),
                limit_by_ip,
            ))
    }

    fn request(path: &str, peer: &str, forwarded: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(path);
        if let Some(forwarded) = forwarded {
            builder = builder.header("x-forwarded-for", forwarded);
        }
        let mut request = builder.body(Body::empty()).unwrap();
        let peer: IpAddr = peer.parse().unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(peer, 40_000)));
        request
    }

    async fn status(app: &Router, request: Request<Body>) -> StatusCode {
        app.clone().oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn a_burst_is_allowed_then_429_with_retry_after_and_a_json_body() {
        let app = app(&settings(3, 2, &[])).await;
        for _ in 0..3 {
            assert_eq!(
                status(&app, request("/other", "198.51.100.1", None)).await,
                StatusCode::OK
            );
        }
        let response = app
            .clone()
            .oneshot(request("/other", "198.51.100.1", None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry: u64 = response.headers()["retry-after"]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(retry >= 1);
        let body: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["error"]["code"], "rate_limited");
    }

    #[tokio::test]
    async fn clients_are_limited_independently() {
        let app = app(&settings(1, 1, &[])).await;
        assert_eq!(
            status(&app, request("/other", "198.51.100.1", None)).await,
            StatusCode::OK
        );
        assert_eq!(
            status(&app, request("/other", "198.51.100.1", None)).await,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            status(&app, request("/other", "198.51.100.2", None)).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn auth_routes_have_a_stricter_limit_and_fail_closed_without_redis() {
        // Redis is unreachable here: the general policy degrades to the local fallback
        // while the auth policy must refuse with 503 instead of allowing.
        let app = app(&settings(50, 2, &[])).await;
        assert_eq!(
            status(&app, request("/other", "198.51.100.1", None)).await,
            StatusCode::OK
        );
        let response = app
            .clone()
            .oneshot(request("/login", "198.51.100.1", None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().contains_key("retry-after"));
    }

    #[tokio::test]
    async fn spoofed_forwarded_headers_do_not_bypass_limits() {
        let app = app(&settings(1, 1, &["10.0.0.0/8"])).await;
        // Untrusted peer rotating X-Forwarded-For stays in a single bucket.
        assert_eq!(
            status(&app, request("/other", "198.51.100.1", Some("1.1.1.1"))).await,
            StatusCode::OK
        );
        for spoof in ["2.2.2.2", "3.3.3.3", "10.0.0.9"] {
            assert_eq!(
                status(&app, request("/other", "198.51.100.1", Some(spoof))).await,
                StatusCode::TOO_MANY_REQUESTS,
                "spoofed {spoof}"
            );
        }
    }

    #[tokio::test]
    async fn trusted_proxies_limit_the_forwarded_client_not_the_proxy() {
        let app = app(&settings(1, 1, &["10.0.0.0/8"])).await;
        let via_proxy = |client: &str| request("/other", "10.0.0.1", Some(client));
        assert_eq!(
            status(&app, via_proxy("198.51.100.1")).await,
            StatusCode::OK
        );
        assert_eq!(
            status(&app, via_proxy("198.51.100.1")).await,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            status(&app, via_proxy("198.51.100.2")).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn health_probes_are_never_throttled() {
        let app = app(&settings(1, 1, &[])).await;
        for _ in 0..5 {
            assert_eq!(
                status(&app, request("/health/live", "198.51.100.1", None)).await,
                StatusCode::OK
            );
        }
    }

    #[test]
    fn retry_after_is_at_least_one_second() {
        let response = AppError::TooManyRequests {
            retry_after: Duration::from_millis(10),
        }
        .into_response();
        assert_eq!(response.headers()["retry-after"], "1");
    }
}
