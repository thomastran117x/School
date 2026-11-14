//! Application-wide error type and its safe JSON contract.
//!
//! Handlers return [`AppError`]; [`IntoResponse`] converts it to a stable body:
//!
//! ```json
//! {"error": {"code": "not_found", "message": "resource not found", "request_id": "…"}}
//! ```
//!
//! Validation failures of a well-formed request return 422 with a `fields` array of
//! per-field errors (`{"field", "code", "message"}`); malformed requests return 400.
//!
//! Internal failures are logged server-side and never serialized. Only messages that
//! are safe to show to a client may be placed in [`AppError::Validation`] and
//! [`AppError::Conflict`].

use std::time::Duration;

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
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
    /// The request was well-formed but one or more fields are invalid.
    #[error("request validation failed")]
    UnprocessableEntity(Vec<FieldError>),
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
    /// The caller exceeded a rate limit; `Retry-After` is sent.
    #[error("too many requests")]
    TooManyRequests {
        /// How long the caller should wait before retrying.
        retry_after: Duration,
    },
    /// A protected route could not be evaluated safely (for example the rate limiter's
    /// datastore is down) and is refusing traffic; `Retry-After` is sent.
    #[error("service unavailable")]
    ServiceUnavailable {
        /// How long the caller should wait before retrying.
        retry_after: Duration,
    },
    /// An unexpected server-side failure. Only a static diagnostic is carried, so no
    /// runtime data (URLs, credentials, student data) can reach the logs or the client.
    #[error("internal server error: {0}")]
    Internal(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
/// A single client-safe validation failure tied to one request field.
pub struct FieldError {
    /// Dotted path of the offending field, e.g. `items[0].name`.
    pub field: String,
    /// Stable machine-readable rule code, e.g. `length` or `required`.
    pub code: String,
    /// Human-readable, client-safe message.
    pub message: String,
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
    /// Per-field validation failures, present only for 422 responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    fields: Option<&'a [FieldError]>,
}

impl AppError {
    /// Creates an internal error from a static, secret-free diagnostic.
    ///
    /// The `&'static str` bound is deliberate: it rejects formatted strings and
    /// `Display` output of third-party errors, which may embed secrets. Log the
    /// detailed cause separately through a sanitized, typed error instead.
    #[allow(dead_code)]
    pub fn internal(reason: &'static str) -> Self {
        Self::Internal(reason)
    }

    /// Returns the HTTP status code for this error.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Validation(_) => StatusCode::BAD_REQUEST,
            Self::UnprocessableEntity(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::ServiceUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Returns the stable machine-readable code for this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Validation(_) => "validation_error",
            Self::UnprocessableEntity(_) => "validation_failed",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::Conflict(_) => "conflict",
            Self::PayloadTooLarge => "payload_too_large",
            Self::TooManyRequests { .. } => "rate_limited",
            Self::ServiceUnavailable { .. } => "rate_limit_unavailable",
            Self::Internal(_) => "internal_error",
        }
    }

    /// Returns the message that is safe to show to clients.
    fn public_message(&self) -> &str {
        match self {
            Self::Validation(message) | Self::Conflict(message) => message,
            Self::UnprocessableEntity(_) => "request validation failed",
            Self::Unauthorized => "authentication required",
            Self::Forbidden => "forbidden",
            Self::NotFound => "resource not found",
            Self::MethodNotAllowed => "method not allowed",
            Self::PayloadTooLarge => "request body is too large",
            Self::TooManyRequests { .. } => "too many requests; retry later",
            Self::ServiceUnavailable { .. } => "service temporarily unavailable; retry later",
            Self::Internal(_) => "internal server error",
        }
    }

    /// Returns the `Retry-After` delay in whole seconds (at least one), if any.
    fn retry_after_seconds(&self) -> Option<u64> {
        match self {
            Self::TooManyRequests { retry_after } | Self::ServiceUnavailable { retry_after } => {
                Some(
                    retry_after
                        .as_secs()
                        .saturating_add(u64::from(retry_after.subsec_nanos() > 0))
                        .max(1),
                )
            }
            _ => None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if let Self::Internal(reason) = &self {
            tracing::error!(reason = *reason, "request failed with an internal error");
        }
        let request_id = request_context::current();
        let body = ErrorEnvelope {
            error: ErrorBody {
                code: self.code(),
                message: self.public_message(),
                request_id: request_id.as_deref(),
                fields: match &self {
                    Self::UnprocessableEntity(fields) => Some(fields),
                    _ => None,
                },
            },
        };
        let retry_after = self.retry_after_seconds();
        let mut response = (self.status(), Json(body)).into_response();
        if let Some(seconds) = retry_after {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        }
        response
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
                AppError::UnprocessableEntity(vec![]),
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
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
                AppError::TooManyRequests {
                    retry_after: Duration::from_millis(1500),
                },
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
            ),
            (
                AppError::ServiceUnavailable {
                    retry_after: Duration::from_secs(5),
                },
                StatusCode::SERVICE_UNAVAILABLE,
                "rate_limit_unavailable",
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

    #[test]
    fn retry_after_rounds_up_and_is_only_set_for_throttling_errors() {
        let header = |error: AppError| {
            error
                .into_response()
                .headers()
                .get(header::RETRY_AFTER)
                .map(|value| value.to_str().unwrap().to_owned())
        };
        let ms = |ms| Duration::from_millis(ms);
        assert_eq!(
            header(AppError::TooManyRequests {
                retry_after: ms(1500)
            })
            .as_deref(),
            Some("2")
        );
        assert_eq!(
            header(AppError::TooManyRequests { retry_after: ms(1) }).as_deref(),
            Some("1")
        );
        assert_eq!(
            header(AppError::ServiceUnavailable {
                retry_after: ms(5000)
            })
            .as_deref(),
            Some("5")
        );
        assert_eq!(header(AppError::NotFound), None);
    }

    #[tokio::test]
    async fn internal_errors_never_leak_their_diagnostic_to_clients() {
        let (status, body) = render(AppError::internal("cache write failed")).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!body.to_string().contains("cache write failed"));
        assert_eq!(body["error"]["message"], "internal server error");
    }

    #[tokio::test]
    async fn unprocessable_entity_lists_field_errors() {
        let error = AppError::UnprocessableEntity(vec![FieldError {
            field: "name".into(),
            code: "length".into(),
            message: "too long".into(),
        }]);
        let (_, body) = render(error).await;

        assert_eq!(
            body["error"]["fields"],
            json!([{"field": "name", "code": "length", "message": "too long"}])
        );
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
