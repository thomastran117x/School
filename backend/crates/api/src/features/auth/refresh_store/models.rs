//! Public data types of the refresh-session store.

use std::{fmt, time::Duration};

use thiserror::Error;

#[derive(Clone, PartialEq, Eq)]
/// A freshly issued refresh token and the session it belongs to.
pub struct IssuedSession {
    /// The opaque refresh token. It is shown to the caller once and never stored.
    pub token: String,
    /// Identifier shared by every token rotated from the same login.
    pub family_id: String,
    /// The user that owns the session.
    pub user_id: String,
    /// Lifetime of this token from the moment it was issued.
    pub expires_in: Duration,
}

impl fmt::Debug for IssuedSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IssuedSession")
            .field("token", &"<redacted>")
            .field("family_id", &self.family_id)
            .field("user_id", &self.user_id)
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Result of presenting a refresh token for rotation.
pub enum RotationOutcome {
    /// The token was valid and has been replaced by the contained successor.
    Rotated(IssuedSession),
    /// The token had already been rotated; its whole session family is now revoked.
    ReplayDetected,
    /// The token belongs to a family that was revoked or has reached its lifetime.
    Revoked,
    /// The token is unknown, malformed, or expired.
    Invalid,
}

#[derive(Debug, Error, PartialEq, Eq)]
/// Failures of refresh-session operations. None of them carry token material.
pub enum RefreshStoreError {
    /// Redis could not complete the operation; callers must deny the request.
    #[error("refresh session store is unavailable")]
    Unavailable,
    /// A user identifier was empty, too long, or used disallowed characters.
    #[error("identifier is not valid for the refresh session store")]
    InvalidIdentifier,
}
