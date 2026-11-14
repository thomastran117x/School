//! Allowlist-based CORS policy.
//!
//! Origins, methods and headers come exclusively from validated configuration. The
//! layer never uses a wildcard, so enabling credentials can never be combined with
//! `Access-Control-Allow-Origin: *`. With no configured origins, no cross-origin
//! response headers are emitted at all.

use axum::http::{HeaderName, HeaderValue, Method};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::config::CorsSettings;

/// Builds the [`CorsLayer`] for the configured policy.
pub fn layer(settings: &CorsSettings) -> CorsLayer {
    let origins: Vec<HeaderValue> = settings
        .allowed_origins
        .iter()
        .filter_map(|origin| HeaderValue::from_str(origin).ok())
        .collect();
    let methods: Vec<Method> = settings
        .allowed_methods
        .iter()
        .filter_map(|method| Method::from_bytes(method.as_bytes()).ok())
        .collect();
    let headers: Vec<HeaderName> = settings
        .allowed_headers
        .iter()
        .filter_map(|header| HeaderName::from_bytes(header.as_bytes()).ok())
        .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods(methods)
        .allow_headers(headers)
        .expose_headers([HeaderName::from_static("x-request-id")])
        .allow_credentials(settings.allow_credentials)
        .max_age(settings.max_age)
}
