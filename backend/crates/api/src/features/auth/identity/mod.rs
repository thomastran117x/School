//! Local and federated identity model backed by MySQL.
//!
//! The schema is owned by the `migration` crate (`users`, `local_credentials`,
//! `external_identities`). This module provides the matching SeaORM entities and
//! the value types that keep the security rules in the type system:
//!
//! - [`PasswordHash`] only accepts Argon2id PHC strings, prints as `[REDACTED]`,
//!   and is never serializable, so a credential cannot leak through `Debug`
//!   logging or an API response.
//! - [`NewStudentAccount`] has no role field: public signup always creates a
//!   `STUDENT` in the `UNVERIFIED` state and cannot request `ADMIN`.
//! - [`NewExternalIdentity`] only allows a tenant for Microsoft identities.
//!
//! Federated logins are matched only by provider and immutable subject, never
//! by email; linking a second sign-in method is an explicit, authenticated flow
//! (see `backend/README.md`).

pub(crate) mod entities;
mod models;

pub(crate) use models::{
    AccountStatus, IdentityModelError, IdentityProvider, NewExternalIdentity, NewLocalCredential,
    NewStudentAccount, PasswordAlgorithm, PasswordHash, Role,
};
