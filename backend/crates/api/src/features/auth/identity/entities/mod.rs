//! SeaORM entities for the identity tables.
//!
//! None of these entities derive `Serialize`: features must map them to their
//! own response models and decide explicitly which fields are public.

pub(crate) mod external_identity;
pub(crate) mod local_credential;
pub(crate) mod user;
