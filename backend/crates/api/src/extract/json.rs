//! Validated JSON request bodies.

use axum::{
    body::Bytes,
    extract::{FromRequest, Request},
    http::{StatusCode, header},
};
use serde::de::DeserializeOwned;
use validator::Validate;

use super::error_map;
use crate::error::AppError;

/// Extracts a JSON body, deserializes it into `T` and runs `T`'s validation rules.
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidatedJson<T>(pub T);

impl<T, S> FromRequest<S> for ValidatedJson<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        if !has_json_content_type(&req) {
            return Err(AppError::Validation(
                "content-type must be application/json".to_owned(),
            ));
        }
        let bytes = Bytes::from_request(req, state).await.map_err(|rejection| {
            match rejection.status() {
                StatusCode::PAYLOAD_TOO_LARGE => AppError::PayloadTooLarge,
                _ => AppError::Validation("failed to read the request body".to_owned()),
            }
        })?;

        let deserializer = &mut serde_json::Deserializer::from_slice(&bytes);
        let value: T = serde_path_to_error::deserialize(deserializer).map_err(|error| {
            // Syntax and EOF errors mean the body is not JSON at all (400); anything
            // else is well-formed JSON with the wrong shape (422).
            if error.inner().is_syntax() || error.inner().is_eof() {
                AppError::Validation("request body is not valid JSON".to_owned())
            } else {
                error_map::from_deserialize(&error)
            }
        })?;
        value
            .validate()
            .map_err(|errors| error_map::from_validation(&errors))?;
        Ok(Self(value))
    }
}

/// Returns whether the request declares a JSON media type (`application/json` or `+json`).
fn has_json_content_type(req: &Request) -> bool {
    req.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<axum::http::HeaderValue>().ok())
        .and_then(|value| {
            let mime = value
                .to_str()
                .ok()?
                .split(';')
                .next()?
                .trim()
                .to_ascii_lowercase();
            Some(mime == "application/json" || mime.ends_with("+json"))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::post,
    };
    use http_body_util::BodyExt;
    use serde::Deserialize;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use tower_http::limit::RequestBodyLimitLayer;
    use validator::Validate;

    use super::ValidatedJson;
    use crate::extract::rules;

    #[derive(Deserialize, Validate)]
    #[serde(deny_unknown_fields)]
    struct Item {
        #[validate(length(min = 1, max = 5), custom(function = "rules::not_blank"))]
        name: String,
    }

    #[derive(Deserialize, Validate)]
    struct Create {
        #[validate(email)]
        email: String,
        #[validate(range(min = 1, max = 10))]
        count: u32,
        #[validate(nested)]
        items: Vec<Item>,
    }

    fn app() -> Router {
        Router::new()
            .route(
                "/",
                post(|ValidatedJson(_): ValidatedJson<Create>| async { "ok" }),
            )
            .layer(RequestBodyLimitLayer::new(256))
    }

    async fn send(content_type: &str, body: String) -> (StatusCode, Value) {
        let response = app()
            .oneshot(
                Request::post("/")
                    .header("content-type", content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    async fn json_body(body: Value) -> (StatusCode, Value) {
        send("application/json", body.to_string()).await
    }

    #[tokio::test]
    async fn accepts_valid_input() {
        let (status, _) =
            json_body(json!({"email": "a@b.co", "count": 2, "items": [{"name": "x"}]})).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn malformed_json_is_400() {
        let (status, body) = send("application/json", "{not json".to_owned()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "validation_error");
    }

    #[tokio::test]
    async fn wrong_content_type_is_400() {
        let (status, body) = send("text/plain", "{}".to_owned()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "validation_error");
    }

    #[tokio::test]
    async fn missing_required_field_is_422_with_field_name() {
        let (status, body) = json_body(json!({"count": 2, "items": []})).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["code"], "validation_failed");
        assert_eq!(body["error"]["fields"][0]["field"], "email");
        assert_eq!(body["error"]["fields"][0]["code"], "required");
    }

    #[tokio::test]
    async fn wrong_type_is_422_without_echoing_input() {
        let (status, body) =
            json_body(json!({"email": "a@b.co", "count": "SECRET", "items": []})).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["fields"][0]["field"], "count");
        assert_eq!(body["error"]["fields"][0]["code"], "invalid");
        assert!(!body.to_string().contains("SECRET"));
    }

    #[tokio::test]
    async fn rule_failures_report_every_field_with_nested_paths() {
        let (status, body) = json_body(json!({
            "email": "nope",
            "count": 99,
            "items": [{"name": "ok"}, {"name": "toolong"}, {"name": "   "}]
        }))
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let fields: Vec<(String, String)> = body["error"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["field"].as_str().unwrap().to_owned(),
                    f["code"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        for expected in [
            ("count", "range"),
            ("email", "email"),
            ("items[1].name", "length"),
            ("items[2].name", "blank"),
        ] {
            assert!(
                fields.contains(&(expected.0.to_owned(), expected.1.to_owned())),
                "missing {expected:?} in {fields:?}"
            );
        }
    }

    #[tokio::test]
    async fn unknown_field_is_rejected_when_denied() {
        let (status, body) = json_body(json!({
            "email": "a@b.co", "count": 1, "items": [{"name": "x", "extra": 1}]
        }))
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["fields"][0]["code"], "unknown_field");
    }

    #[tokio::test]
    async fn oversized_body_is_413() {
        let (status, _) = json_body(json!({"email": "a".repeat(1024)})).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }
}
