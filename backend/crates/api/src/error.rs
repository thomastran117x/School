//! Application-wide error type and its safe JSON contract.
//!
//! Handlers return [`AppError`]; [`IntoResponse`] converts it to a stable body:
//!
//! ```json
//! {"error": {"code": "not_found", "message": "resource not found", "request_id": "…"}}
//! ```
//!
//! Internal failures are logged server-side and never serialized. Only messages that
//! are safe to show to a client may be placed in [`AppError::Validation`] and
//! [`AppError::Conflict`].

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

use crate::middleware::request_context;

// Variants are consumed by feature handlers as they are added.
#[allow(dead_code)]
#[derive(Debug, Error)]
/// A failure that can be returned from an HTTP handler.
pub enum AppError {
    /// The request was malformed or failed validation. The message is client-safe.
    #[error("validation failed: {0}")]
    Validation(String),
    /// Authentication is missing or invalid.
    #[error("authentication required")]
    Unauthorized,
    /// The caller is authenticated but not permitted to perform the action.
    #[error("forbidden")]
    Forbidden,
    /// The requested resource does not exist.
    #[error("resource not found")]
    NotFound,
    /// The HTTP method is not supported for the route.
    #[error("method not allowed")]
    MethodNotAllowed,
    /// The request conflicts with current state. The message is client-safe.
    #[error("conflict: {0}")]
    Conflict(String),
    /// The request body exceeded the configured limit.
    #[error("payload too large")]
    PayloadTooLarge,
    /// An unexpected server-side failure. The source is logged but never returned.
    #[error("internal server error")]
    Internal(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[derive(Serialize)]
/// Top-level JSON envelope of every error response.
struct ErrorEnvelope<'a> {
    /// The error details.
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
/// Client-visible error details.
struct ErrorBody<'a> {
    /// Stable machine-readable error code.
    code: &'static str,
    /// Human-readable, client-safe message.
    message: &'a str,
    /// Identifier of the failed request, for correlation with server logs.
    request_id: Option<&'a str>,
}

impl AppError {
    /// Wraps an unexpected failure as an internal error.
    #[allow(dead_code)]
    pub fn internal(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self::Internal(source.into())
    }

    /// Returns the HTTP status code for this error.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Validation(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Returns the stable machine-readable code for this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Validation(_) => "validation_error",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::Conflict(_) => "conflict",
            Self::PayloadTooLarge => "payload_too_large",
            Self::Internal(_) => "internal_error",
        }
    }

    /// Returns the message that is safe to show to clients.
    fn public_message(&self) -> &str {
        match self {
            Self::Validation(message) | Self::Conflict(message) => message,
            Self::Unauthorized => "authentication required",
            Self::Forbidden => "forbidden",
            Self::NotFound => "resource not found",
            Self::MethodNotAllowed => "method not allowed",
            Self::PayloadTooLarge => "request body is too large",
            Self::Internal(_) => "internal server error",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if let Self::Internal(source) = &self {
            tracing::error!(error = %source, "request failed with an internal error");
        }
        let request_id = request_context::current();
        let body = ErrorEnvelope {
            error: ErrorBody {
                code: self.code(),
                message: self.public_message(),
                request_id: request_id.as_deref(),
            },
        };
        (self.status(), Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;
    use serde_json::{Value, json};

    use super::*;

    async fn render(error: AppError) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn maps_each_variant_to_its_status_and_code() {
        let cases = [
            (
                AppError::Validation("bad".into()),
                StatusCode::BAD_REQUEST,
                "validation_error",
            ),
            (
                AppError::Unauthorized,
                StatusCode::UNAUTHORIZED,
                "unauthorized",
            ),
            (AppError::Forbidden, StatusCode::FORBIDDEN, "forbidden"),
            (AppError::NotFound, StatusCode::NOT_FOUND, "not_found"),
            (
                AppError::MethodNotAllowed,
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
            ),
            (
                AppError::Conflict("dup".into()),
                StatusCode::CONFLICT,
                "conflict",
            ),
            (
                AppError::PayloadTooLarge,
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
            ),
            (
                AppError::internal("boom"),
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
            ),
        ];

        for (error, expected_status, expected_code) in cases {
            let (status, body) = render(error).await;
            assert_eq!(status, expected_status);
            assert_eq!(body["error"]["code"], expected_code);
        }
    }

    #[tokio::test]
    async fn internal_errors_never_leak_their_source() {
        let (status, body) =
            render(AppError::internal("mysql://root:hunter2@db/secret failed")).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!body.to_string().contains("hunter2"));
        assert_eq!(body["error"]["message"], "internal server error");
    }

    #[tokio::test]
    async fn includes_the_request_id_when_one_is_in_scope() {
        let (_, body) = request_context::with_request_id("req-123".to_owned(), async {
            render(AppError::NotFound).await
        })
        .await;

        assert_eq!(
            body,
            json!({"error": {
                "code": "not_found",
                "message": "resource not found",
                "request_id": "req-123"
            }})
        );
    }
}
