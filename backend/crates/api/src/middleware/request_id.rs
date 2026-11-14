//! Validation of client-supplied correlation IDs.
//!
//! `SetRequestIdLayer` trusts an incoming `x-request-id` header as-is and only
//! generates one when it is absent. [`sanitize`] runs before it and drops values that
//! are unsafe to echo into logs, response headers and error bodies, so a fresh UUID is
//! generated instead.

use axum::{extract::Request, middleware::Next, response::Response};

/// Header carrying the correlation ID.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Maximum accepted length of a client-supplied request ID.
pub const MAX_REQUEST_ID_LEN: usize = 64;

/// Returns whether `value` is an acceptable correlation ID.
///
/// Accepted IDs are 1 to [`MAX_REQUEST_ID_LEN`] characters drawn from ASCII letters,
/// digits, `-`, `_` and `.`.
pub fn is_valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REQUEST_ID_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Removes an invalid incoming `x-request-id` so a new one is generated downstream.
///
/// Valid IDs pass through unchanged.
pub async fn sanitize(mut request: Request, next: Next) -> Response {
    let valid = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .is_none_or(|value| value.to_str().is_ok_and(is_valid_request_id));
    if !valid {
        request.headers_mut().remove(REQUEST_ID_HEADER);
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_typical_ids() {
        assert!(is_valid_request_id("req-42"));
        assert!(is_valid_request_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_valid_request_id("a.b_c-D9"));
        assert!(is_valid_request_id(&"a".repeat(MAX_REQUEST_ID_LEN)));
    }

    #[test]
    fn rejects_unsafe_ids() {
        assert!(!is_valid_request_id(""));
        assert!(!is_valid_request_id(&"a".repeat(MAX_REQUEST_ID_LEN + 1)));
        assert!(!is_valid_request_id("has space"));
        assert!(!is_valid_request_id("<script>"));
        assert!(!is_valid_request_id("a\tb"));
        assert!(!is_valid_request_id("é"));
    }
}
