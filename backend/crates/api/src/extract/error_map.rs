//! Conversion of `validator` and `serde` failures into client-safe [`AppError`]s.

use validator::{ValidationErrors, ValidationErrorsKind};

use crate::error::{AppError, FieldError};

/// Flattens nested validation errors into dotted-path [`FieldError`]s.
pub(super) fn from_validation(errors: &ValidationErrors) -> AppError {
    let mut out = Vec::new();
    collect(errors, "", &mut out);
    out.sort_by(|a, b| a.field.cmp(&b.field).then_with(|| a.code.cmp(&b.code)));
    AppError::UnprocessableEntity(out)
}

/// Recursively collects field errors under `prefix`.
fn collect(errors: &ValidationErrors, prefix: &str, out: &mut Vec<FieldError>) {
    for (name, kind) in errors.errors() {
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}.{name}")
        };
        match kind {
            ValidationErrorsKind::Field(list) => {
                for error in list {
                    let message = error.message.as_ref().map_or_else(
                        || format!("failed the `{}` rule", error.code),
                        ToString::to_string,
                    );
                    out.push(FieldError {
                        field: path.clone(),
                        code: error.code.to_string(),
                        message,
                    });
                }
            }
            ValidationErrorsKind::Struct(inner) => collect(inner, &path, out),
            ValidationErrorsKind::List(items) => {
                for (index, inner) in items {
                    collect(inner, &format!("{path}[{index}]"), out);
                }
            }
        }
    }
}

/// Maps a `serde_path_to_error` failure to a 422 with a single fixed-message field error.
///
/// Only the field path and a coarse code (`required` or `invalid`) are reported; the
/// serde message is inspected but never echoed, because it can quote client input.
pub(super) fn from_deserialize<E: std::fmt::Display>(
    error: &serde_path_to_error::Error<E>,
) -> AppError {
    let raw = error.inner().to_string();
    let path = error.path().to_string();
    let mut field = if path == "." { String::new() } else { path };
    if let Some(name) = raw
        .strip_prefix("missing field `")
        .and_then(|rest| rest.split('`').next())
    {
        // Required names come from the DTO definition, never from client input.
        if !field.is_empty() {
            field.push('.');
        }
        field.push_str(name);
    }
    if field.is_empty() {
        field = "body".to_owned();
    }
    let (code, message) = if raw.starts_with("missing field") {
        ("required", "this field is required")
    } else if raw.starts_with("unknown field") {
        ("unknown_field", "this field is not allowed")
    } else {
        ("invalid", "this field has an invalid value or type")
    };
    AppError::UnprocessableEntity(vec![FieldError {
        field,
        code: code.to_owned(),
        message: message.to_owned(),
    }])
}
