//! Axum extractor that authenticates a request from its bearer access token.
//!
//! Handlers take [`AuthenticatedPrincipal`] as an argument. The principal comes only
//! from the `Authorization` header; bodies, queries and cookies are never read, so a
//! client cannot choose its own user id or role.

use std::sync::Arc;

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{HeaderMap, header::AUTHORIZATION, request::Parts},
};

use super::{
    models::{AccessTokenError, AuthenticatedPrincipal},
    service::AccessTokenService,
};
use crate::error::AppError;

impl<S> FromRequestParts<S> for AuthenticatedPrincipal
where
    S: Send + Sync,
    Arc<AccessTokenService>: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let service = Arc::<AccessTokenService>::from_ref(state);
        bearer_token(&parts.headers)
            .and_then(|token| service.verify(token))
            .map_err(reject)
    }
}

/// Returns the token from exactly one `Authorization: Bearer <token>` header.
fn bearer_token(headers: &HeaderMap) -> Result<&str, AccessTokenError> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return Err(AccessTokenError::MissingCredentials);
    };
    let value = value
        .to_str()
        .map_err(|_| AccessTokenError::MissingCredentials)?;
    let (scheme, token) = value
        .split_once(' ')
        .ok_or(AccessTokenError::MissingCredentials)?;
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return Err(AccessTokenError::MissingCredentials);
    }
    Ok(token)
}

/// Logs the static rejection reason and returns the uniform `401`.
fn reject(error: AccessTokenError) -> AppError {
    tracing::debug!(reason = error.label(), "access token rejected");
    AppError::Unauthorized
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::{
        Json, Router,
        body::Body,
        http::{Method, Request, StatusCode, header},
        routing::{get, post},
    };
    use http_body_util::BodyExt;
    use sea_orm::prelude::Uuid;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use tracing::Level;

    use super::*;
    use crate::{config::JwtSettings, features::auth::identity::Role};

    fn service() -> Arc<AccessTokenService> {
        use crate::features::auth::access_token::test_keys::{PRIMARY_KID, PRIMARY_PRIVATE};
        let settings = JwtSettings::from_pairs(&[
            ("JWT_SIGNING_KEY_ID", PRIMARY_KID),
            ("JWT_SIGNING_KEY", PRIMARY_PRIVATE),
        ])
        .unwrap();
        Arc::new(AccessTokenService::new(&settings))
    }

    async fn whoami(principal: AuthenticatedPrincipal) -> Json<Value> {
        Json(json!({
            "user_id": principal.user_id().to_string(),
            "role": format!("{:?}", principal.role()),
        }))
    }

    /// State is only the token service: no datastore client exists in these tests,
    /// so a successful request proves verification needs no Redis lookup.
    fn app(service: Arc<AccessTokenService>) -> Router {
        Router::new()
            .route("/me", get(whoami))
            .route("/me", post(whoami))
            .with_state(service)
    }

    async fn send(
        service: Arc<AccessTokenService>,
        request: Request<Body>,
    ) -> (StatusCode, HeaderMap, Value) {
        let response = app(service).oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, headers, serde_json::from_slice(&bytes).unwrap())
    }

    fn request(authorization: &[&str]) -> Request<Body> {
        let mut builder = Request::builder().uri("/me");
        for value in authorization {
            builder = builder.header(header::AUTHORIZATION, *value);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn a_valid_bearer_token_authenticates_the_request() {
        let service = service();
        let user = Uuid::now_v7();
        let token = service.issue(user, Role::Admin).unwrap().token;

        for scheme in ["Bearer", "bearer"] {
            let (status, _, body) = send(
                Arc::clone(&service),
                request(&[&format!("{scheme} {token}")]),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["user_id"], user.to_string());
            assert_eq!(body["role"], "Admin");
        }
    }

    #[tokio::test]
    async fn missing_or_invalid_credentials_return_401() {
        let service = service();
        let token = service.issue(Uuid::now_v7(), Role::Student).unwrap().token;
        let mut tampered = token.clone();
        tampered.pop();
        tampered.push(if token.ends_with('A') { 'B' } else { 'A' });

        let cases: Vec<Vec<String>> = vec![
            vec![],
            vec![format!("Basic {token}")],
            vec!["Bearer".to_owned()],
            vec!["Bearer ".to_owned()],
            vec![token.clone()],
            vec![format!("Bearer {token}"), format!("Bearer {token}")],
            vec![format!("Bearer  {token}")],
            vec![format!("Bearer {tampered}")],
            vec!["Bearer not.a.jwt".to_owned()],
        ];
        for case in cases {
            let values: Vec<&str> = case.iter().map(String::as_str).collect();
            let (status, headers, body) = send(Arc::clone(&service), request(&values)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{values:?}");
            assert_eq!(headers[header::WWW_AUTHENTICATE], "Bearer");
            assert_eq!(body["error"]["code"], "unauthorized");
            assert_eq!(body["error"]["message"], "authentication required");
        }
    }

    #[tokio::test]
    async fn body_supplied_identity_is_ignored() {
        let service = service();
        let user = Uuid::now_v7();
        let token = service.issue(user, Role::Student).unwrap().token;
        let request = Request::builder()
            .method(Method::POST)
            .uri("/me")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"user_id": Uuid::now_v7().to_string(), "role": "ADMIN", "sub": "someone"})
                    .to_string(),
            ))
            .unwrap();

        let (status, _, body) = send(service, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["user_id"], user.to_string());
        assert_eq!(body["role"], "Student");
    }

    #[derive(Clone, Default)]
    struct LogBuffer(Arc<Mutex<Vec<u8>>>);

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

    /// Sends one valid and several invalid requests and returns all TRACE logs.
    async fn capture_logs(service: &Arc<AccessTokenService>, token: &str) -> String {
        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .with_max_level(Level::TRACE)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        tracing::callsite::rebuild_interest_cache();
        let mut expired_or_tampered = token.to_owned();
        expired_or_tampered.insert(5, 'x');
        for value in [
            format!("Bearer {token}"),
            format!("Bearer {expired_or_tampered}"),
            format!("Basic {token}"),
        ] {
            app(Arc::clone(service))
                .oneshot(request(&[&value]))
                .await
                .unwrap();
        }
        String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap()
    }

    #[tokio::test]
    async fn tokens_and_claims_are_never_logged() {
        let service = service();
        let user = Uuid::now_v7();
        let token = service.issue(user, Role::Student).unwrap().token;

        // Tracing caches callsite interest process-wide, so retry until the scoped
        // subscriber has observed the rejection event (see `app.rs` tests).
        let mut logs = String::new();
        for _ in 0..50 {
            logs = capture_logs(&service, &token).await;
            if logs.contains("access token rejected") {
                break;
            }
            tokio::task::yield_now().await;
        }

        assert!(logs.contains("access token rejected"), "{logs}");
        assert!(logs.contains("reason="), "{logs}");
        for segment in token.split('.') {
            assert!(!logs.contains(segment), "{logs}");
        }
        assert!(!logs.contains(&user.to_string()), "{logs}");
    }
}
