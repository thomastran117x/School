//! Consistent JSON bodies for errors produced by the framework itself.
//!
//! Routing and body-limit rejections are generated before any handler runs and carry
//! empty or plain-text bodies. This module rewrites them into the
//! [`AppError`] contract.

use axum::{
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::error::AppError;

/// Fallback for unmatched routes.
pub async fn not_found() -> AppError {
    AppError::NotFound
}

/// Rewrites non-JSON `405` and `413` responses into the standard error body.
pub async fn normalize(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if is_json {
        return response;
    }

    let replacement = match response.status() {
        StatusCode::PAYLOAD_TOO_LARGE => AppError::PayloadTooLarge,
        StatusCode::METHOD_NOT_ALLOWED => AppError::MethodNotAllowed,
        _ => return response,
    };
    let allow: Option<HeaderValue> = response.headers().get(header::ALLOW).cloned();
    let mut rewritten = replacement.into_response();
    if let Some(allow) = allow {
        rewritten.headers_mut().insert(header::ALLOW, allow);
    }
    rewritten
}
