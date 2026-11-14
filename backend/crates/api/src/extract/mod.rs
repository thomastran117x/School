//! Reusable request extractors that parse, validate and report errors consistently.
//!
//! Declare a DTO with `serde` and `validator` and take it through an extractor:
//!
//! ```ignore
//! #[derive(Deserialize, Validate)]
//! #[serde(deny_unknown_fields)]
//! struct CreateCourse {
//!     #[validate(length(min = 1, max = rules::MAX_NAME_LEN), custom(function = "rules::not_blank"))]
//!     name: String,
//! }
//!
//! async fn create(ValidatedJson(dto): ValidatedJson<CreateCourse>) { /* … */ }
//! ```
//!
//! Outcomes:
//! - malformed input (bad JSON syntax, wrong content type, unparsable query/path) → 400
//!   `validation_error`;
//! - body over the configured limit → 413 `payload_too_large`;
//! - well-formed input with missing, mistyped or invalid fields → 422
//!   `validation_failed` with a `fields` array.
//!
//! Required fields are plain (non-`Option`) DTO fields, so omitting one yields a
//! `required` field error. Error messages never echo client input or serde internals.

// Extractors are consumed by feature handlers as they are added.
#![allow(dead_code)]

mod error_map;
mod json;
mod path;
mod query;
pub mod rules;

#[allow(unused_imports)]
pub use {json::ValidatedJson, path::ValidatedPath, query::ValidatedQuery};
