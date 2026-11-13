//! Redis-backed refresh-session store.
//!
//! Refresh tokens are opaque random values. Only their SHA-256 hash is stored, every
//! rotation is a single atomic Redis script, and presenting an already-rotated token
//! revokes its whole session family. Any Redis failure fails closed.

mod models;
mod scripts;
mod service;

pub(crate) use models::{IssuedSession, RefreshStoreError, RotationOutcome};
pub(crate) use service::{RedisRefreshSessionStore, RefreshSessionStore};
