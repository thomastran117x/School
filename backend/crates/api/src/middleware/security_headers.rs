//! Security-related response headers for an API.
//!
//! - `X-Content-Type-Options: nosniff` and `Referrer-Policy: no-referrer` are always
//!   sent, as is `X-Frame-Options: DENY`.
//! - `Strict-Transport-Security` is sent only when `HTTP_HTTPS_ENABLED=true`, because
//!   HSTS over plain HTTP is ignored by browsers and can mislead operators.
//! - No `Server` or `X-Powered-By` header is ever set, so the stack is not advertised.
//!
//! **Content-Security-Policy is the frontend's responsibility.** This service returns
//! JSON, not documents, so a CSP here would have no effect; the Next.js application
//! that renders HTML must set its own CSP.

use axum::{
    Router,
    http::{
        HeaderValue,
        header::{
            REFERRER_POLICY, STRICT_TRANSPORT_SECURITY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
        },
    },
};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::config::HttpSettings;

/// Adds the security headers described in the module documentation to `router`.
pub fn apply(router: Router, http: &HttpSettings) -> Router {
    let router = router
        .layer(SetResponseHeaderLayer::overriding(
            X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ));

    if !http.https_enabled {
        return router;
    }
    match HeaderValue::try_from(format!(
        "max-age={}; includeSubDomains",
        http.hsts_max_age.as_secs()
    )) {
        Ok(value) => router.layer(SetResponseHeaderLayer::overriding(
            STRICT_TRANSPORT_SECURITY,
            value,
        )),
        Err(_) => router,
    }
}
