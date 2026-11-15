//! Public data types of the access-token service.

use std::{fmt, time::Duration};

use sea_orm::prelude::Uuid;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::features::auth::identity::Role;

/// Value of the `typ` header of every access token (RFC 9068).
pub(super) const ACCESS_TOKEN_TYPE: &str = "at+jwt";

/// Wire format of the access-token claims.
///
/// Every field is strictly typed, so a token that encodes a claim as another JSON
/// type (for example `aud` as an array) is rejected rather than reinterpreted.
/// Deliberately not `Debug`: the claims are token contents and must never be logged.
#[derive(Serialize, Deserialize)]
pub(super) struct AccessTokenClaims {
    /// Issuer; must equal the configured `JWT_ISSUER`.
    pub iss: String,
    /// Audience; a single string that must equal the configured `JWT_AUDIENCE`.
    pub aud: String,
    /// Subject: the user's UUID.
    pub sub: String,
    /// Issued-at time, in seconds since the Unix epoch.
    pub iat: i64,
    /// Expiry time, in seconds since the Unix epoch.
    pub exp: i64,
    /// Unique token id (a random UUID).
    pub jti: String,
    /// Authorization role, using the database strings (`STUDENT`, `ADMIN`).
    pub role: String,
}

/// The caller of a request, established only by verifying a signed access token.
///
/// Fields are private and the type has no public constructor, so handlers cannot
/// build a principal from request bodies or other untrusted input. It is not
/// `Serialize`, and nothing logs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedPrincipal {
    /// The authenticated user (`sub`).
    user_id: Uuid,
    /// The role carried by the token. It may be stale by up to the token lifetime.
    role: Role,
    /// The token's unique id (`jti`).
    token_id: Uuid,
    /// When the token expires, in seconds since the Unix epoch.
    expires_at: i64,
}

impl AuthenticatedPrincipal {
    /// Builds a principal from claims the service has already verified.
    pub(super) fn verified(user_id: Uuid, role: Role, token_id: Uuid, expires_at: i64) -> Self {
        Self {
            user_id,
            role,
            token_id,
            expires_at,
        }
    }

    /// Builds a principal directly, for router tests of protected endpoints.
    #[cfg(test)]
    pub(crate) fn for_tests(user_id: Uuid, role: Role) -> Self {
        Self::verified(user_id, role, Uuid::new_v4(), i64::MAX)
    }

    /// Returns the authenticated user's id.
    pub fn user_id(&self) -> Uuid {
        self.user_id
    }

    /// Returns the role the token was issued with.
    ///
    /// The role is a snapshot from issuance. Sensitive operations must also check
    /// current account status in MySQL (AUTH-04).
    pub fn role(&self) -> Role {
        self.role
    }

    /// Returns the token's unique id.
    pub fn token_id(&self) -> Uuid {
        self.token_id
    }

    /// Returns the expiry time in seconds since the Unix epoch.
    pub fn expires_at(&self) -> i64 {
        self.expires_at
    }
}

/// A freshly signed access token.
#[derive(Clone, PartialEq, Eq)]
pub struct IssuedAccessToken {
    /// The compact JWT. It is shown to the caller and never stored or logged.
    pub token: String,
    /// Lifetime of the token from issuance.
    pub expires_in: Duration,
}

impl fmt::Debug for IssuedAccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IssuedAccessToken")
            .field("token", &"<redacted>")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// Why an access token was rejected. No variant carries token material.
///
/// Every variant becomes the same `401 unauthorized` response, so clients cannot
/// probe which check failed. The [`label`](Self::label) is only for server logs.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum AccessTokenError {
    /// No single `Authorization: Bearer <token>` header was presented.
    #[error("no bearer token was presented")]
    MissingCredentials,
    /// The value is not a compact JWS with a valid header.
    #[error("access token is malformed")]
    Malformed,
    /// The header names an algorithm other than EdDSA.
    #[error("access token uses an unsupported algorithm")]
    UnsupportedAlgorithm,
    /// The `typ` header is missing or is not `at+jwt`.
    #[error("token is not an access token")]
    WrongType,
    /// The `kid` header is missing or names no configured key.
    #[error("access token key is unknown")]
    UnknownKey,
    /// The signature does not verify.
    #[error("access token signature is invalid")]
    InvalidSignature,
    /// The token has expired, allowing for clock skew.
    #[error("access token has expired")]
    Expired,
    /// The token was issued in the future, allowing for clock skew.
    #[error("access token is not yet valid")]
    NotYetValid,
    /// The `iss` claim is not the configured issuer.
    #[error("access token issuer is not accepted")]
    WrongIssuer,
    /// The `aud` claim is not the configured audience.
    #[error("access token audience is not accepted")]
    WrongAudience,
    /// A claim is missing, has the wrong type, or has an invalid value.
    #[error("access token claims are invalid")]
    InvalidClaims,
}

impl AccessTokenError {
    /// Returns a stable, low-cardinality label for logs and metrics.
    pub fn label(self) -> &'static str {
        match self {
            Self::MissingCredentials => "missing_credentials",
            Self::Malformed => "malformed",
            Self::UnsupportedAlgorithm => "unsupported_algorithm",
            Self::WrongType => "wrong_type",
            Self::UnknownKey => "unknown_key",
            Self::InvalidSignature => "invalid_signature",
            Self::Expired => "expired",
            Self::NotYetValid => "not_yet_valid",
            Self::WrongIssuer => "wrong_issuer",
            Self::WrongAudience => "wrong_audience",
            Self::InvalidClaims => "invalid_claims",
        }
    }
}

/// Failure to sign a new access token.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum IssueError {
    /// The signer rejected the key or claims. Treat it as an internal error.
    #[error("access token could not be signed")]
    Signing,
}
