//! Validated query-string parameters.

use axum::{
    extract::{FromRequestParts, Query},
    http::request::Parts,
};
use serde::de::DeserializeOwned;
use validator::Validate;

use super::error_map;
use crate::error::AppError;

/// Extracts the query string into `T` and runs `T`'s validation rules.
///
/// An unparsable query string is a 400; rule failures are a 422 with field errors.
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidatedQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ValidatedQuery<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::Validation("invalid query string".to_owned()))?;
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

    use super::ValidatedQuery;

    #[derive(Deserialize, Validate)]
    struct Page {
        #[validate(range(min = 1, max = 100))]
        limit: u32,
    }

    async fn call(uri: &str) -> (StatusCode, Value) {
        let app = Router::new().route(
            "/",
            get(|ValidatedQuery(_): ValidatedQuery<Page>| async { "ok" }),
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
    async fn valid_query_passes() {
        assert_eq!(call("/?limit=10").await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn unparsable_query_is_400() {
        let (status, body) = call("/?limit=abc").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "validation_error");
    }

    #[tokio::test]
    async fn missing_required_param_is_400() {
        assert_eq!(call("/").await.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn out_of_range_is_422() {
        let (status, body) = call("/?limit=500").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["fields"][0]["field"], "limit");
        assert_eq!(body["error"]["fields"][0]["code"], "range");
    }
}
