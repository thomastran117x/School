//! Task-local access to the current request ID.
//!
//! `SetRequestIdLayer` stamps every request with an `x-request-id` header. [`scope`]
//! exposes that value to code that has no access to the request, such as the
//! [`IntoResponse`](axum::response::IntoResponse) implementation of
//! [`AppError`](crate::error::AppError).

use axum::{extract::Request, middleware::Next, response::Response};

tokio::task_local! {
    /// Request ID of the request being handled by the current task.
    static REQUEST_ID: String;
}

/// Runs `future` with `request_id` visible through [`current`].
pub async fn with_request_id<F: Future>(request_id: String, future: F) -> F::Output {
    REQUEST_ID.scope(request_id, future).await
}

/// Returns the request ID of the request being handled, if one is in scope.
pub fn current() -> Option<String> {
    REQUEST_ID.try_with(Clone::clone).ok()
}

/// Middleware that scopes the request's `x-request-id` header around the inner service.
pub async fn scope(request: Request, next: Next) -> Response {
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown")
        .to_owned();
    with_request_id(request_id, next.run(request)).await
}
