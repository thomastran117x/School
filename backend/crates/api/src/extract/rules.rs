//! Shared validation rules and length limits for request DTOs.

use validator::ValidationError;

/// Maximum length, in characters, of short names and titles.
#[allow(dead_code)]
pub const MAX_NAME_LEN: u64 = 120;
/// Maximum length, in characters, of free-form text such as descriptions.
#[allow(dead_code)]
pub const MAX_TEXT_LEN: u64 = 5_000;
/// Maximum length, in characters, of an email address (RFC 5321).
#[allow(dead_code)]
pub const MAX_EMAIL_LEN: u64 = 254;

/// Rejects strings that are empty or contain only whitespace.
///
/// Use with `#[validate(custom(function = "rules::not_blank"))]`.
#[allow(dead_code)]
pub fn not_blank(value: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        Err(ValidationError::new("blank"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_blank_rejects_empty_and_whitespace_only() {
        assert!(not_blank("").is_err());
        assert!(not_blank(" \t\n").is_err());
        assert!(not_blank(" a ").is_ok());
    }
}
