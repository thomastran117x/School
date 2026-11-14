//! Validated path parameters.

use axum::{
    extract::{FromRequestParts, Path},
    http::request::Parts,
};
use serde::de::DeserializeOwned;
use validator::Validate;

use super::error_map;
use crate::error::AppError;

/// Extracts path parameters into `T` and runs `T`'s validation rules.
///
/// Unparsable parameters (e.g. a non-UUID id) are a 400; rule failures are a 422.
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidatedPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ValidatedPath<T>
where
    T: DeserializeOwned + Validate + Send,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(value) = Path::<T>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::Validation("invalid path parameter".to_owned()))?;
        value
            .validate()
            .map_err(|errors| error_map::from_validation(&errors))?;
        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::get,
    };
    use http_body_util::BodyExt;
    use serde::Deserialize;
    use serde_json::Value;
    use tower::ServiceExt;
    use validator::Validate;

    use super::ValidatedPath;

    #[derive(Deserialize, Validate)]
    struct Id {
        #[validate(range(min = 1))]
        id: u32,
    }

    async fn call(uri: &str) -> (StatusCode, Value) {
        let app = Router::new().route(
            "/{id}",
            get(|ValidatedPath(_): ValidatedPath<Id>| async { "ok" }),
        );
        let response = app
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn valid_path_passes() {
        assert_eq!(call("/7").await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn unparsable_path_is_400() {
        let (status, body) = call("/abc").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "validation_error");
    }

    #[tokio::test]
    async fn rule_failure_is_422() {
        let (status, body) = call("/0").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["fields"][0]["field"], "id");
    }
}
