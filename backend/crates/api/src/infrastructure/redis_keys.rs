//! Redis key namespace and TTL conventions.
//!
//! Every key written by the application is built through [`Key`] so it carries the
//! application prefix, a schema version, and a documented namespace. Every write goes
//! through a bounded TTL, so no key lives forever by accident.
//!
//! Format: `app:v1:<namespace>:<id>`, for example `app:v1:session:3f2a`.

// Helpers are the shared contract for upcoming features and have no callers yet.
#![allow(dead_code)]

use std::time::Duration;

use thiserror::Error;

/// Prefix shared by every application key; bump the version to invalidate old data.
const ROOT_PREFIX: &str = "app:v1";

/// Longest TTL any key may be given.
pub const MAX_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Documented key namespaces, each with its own default TTL.
pub enum KeyNamespace {
    /// Server-side session state. Default TTL: 24 hours.
    Session,
    /// Derived or cached data that can be rebuilt. Default TTL: 5 minutes.
    Cache,
    /// Rate-limit counters. Default TTL: 1 minute.
    RateLimit,
    /// Short-lived distributed locks. Default TTL: 30 seconds.
    Lock,
}

impl KeyNamespace {
    /// Returns the namespace segment used inside key names.
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Cache => "cache",
            Self::RateLimit => "ratelimit",
            Self::Lock => "lock",
        }
    }

    /// Returns the TTL applied when a caller has no more specific requirement.
    pub const fn default_ttl(self) -> Duration {
        match self {
            Self::Session => Duration::from_secs(24 * 60 * 60),
            Self::Cache => Duration::from_secs(5 * 60),
            Self::RateLimit => Duration::from_secs(60),
            Self::Lock => Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// A validated, fully-prefixed Redis key.
///
/// Keys must never embed secrets or token contents; use opaque identifiers.
pub struct Key(String);

impl Key {
    /// Builds a key in the given namespace.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError`] when `id` is empty, too long, or contains characters
    /// outside ASCII alphanumerics and `-`, `_`, `.`.
    pub fn new(namespace: KeyNamespace, id: &str) -> Result<Self, KeyError> {
        if id.is_empty() || id.len() > 128 {
            return Err(KeyError::InvalidId);
        }
        if !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(KeyError::InvalidId);
        }
        Ok(Self(format!("{ROOT_PREFIX}:{}:{id}", namespace.prefix())))
    }

    /// Returns the key as sent to Redis.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
/// Failures raised while constructing a [`Key`].
pub enum KeyError {
    /// The identifier was empty, longer than 128 bytes, or used disallowed characters.
    #[error("redis key identifier is invalid")]
    InvalidId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_prefixed_keys() {
        let key = Key::new(KeyNamespace::Session, "abc-123").unwrap();
        assert_eq!(key.as_str(), "app:v1:session:abc-123");
        assert_eq!(
            Key::new(KeyNamespace::RateLimit, "ip_1.2")
                .unwrap()
                .as_str(),
            "app:v1:ratelimit:ip_1.2"
        );
    }

    #[test]
    fn rejects_unsafe_identifiers() {
        for id in ["", "a:b", "a b", "a\n", &"x".repeat(129)] {
            assert_eq!(
                Key::new(KeyNamespace::Cache, id),
                Err(KeyError::InvalidId),
                "{id:?}"
            );
        }
    }

    #[test]
    fn default_ttls_are_bounded_and_nonzero() {
        for namespace in [
            KeyNamespace::Session,
            KeyNamespace::Cache,
            KeyNamespace::RateLimit,
            KeyNamespace::Lock,
        ] {
            assert!(namespace.default_ttl() > Duration::ZERO);
            assert!(namespace.default_ttl() <= MAX_TTL);
        }
    }
}
