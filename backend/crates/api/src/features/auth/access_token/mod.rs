//! Stateless JWT access tokens.
//!
//! [`AccessTokenService`] issues short-lived EdDSA (Ed25519) tokens and verifies them
//! using only the configured public keys, without any Redis or MySQL lookup. The
//! [`AuthenticatedPrincipal`] extractor gives handlers the verified caller.
//!
//! Token contract (see `backend/README.md`, "Access tokens"):
//!
//! - header: `alg: EdDSA`, `typ: at+jwt`, `kid`
//! - claims: `iss`, `aud`, `sub` (user UUID), `iat`, `exp`, `jti`, `role`
//!
//! Refresh tokens are opaque strings, not JWTs, so they can never pass as access
//! tokens. An issued token stays valid until `exp` even if the account is later
//! disabled. Sensitive operations must also check account status (AUTH-04).

mod extractor;
pub(crate) mod models;
pub(crate) mod service;
#[cfg(test)]
pub(crate) mod test_keys;

pub(crate) use models::{AccessTokenError, AuthenticatedPrincipal, IssueError, IssuedAccessToken};
pub(crate) use service::AccessTokenService;
