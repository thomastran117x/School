//! The `users` table: one row per account, including anonymized tombstones.

use sea_orm::entity::prelude::*;

use crate::features::auth::identity::models::{AccountStatus, Role};

/// One user account.
///
/// The row holds no credential material. `email` and `normalized_email` are
/// `None` only for a [`AccountStatus::Deleted`] tombstone.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "users")]
pub struct Model {
    /// Immutable application-generated UUIDv7.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Email as entered by the user; `None` once the account is deleted.
    pub email: Option<String>,
    /// Canonical email used for uniqueness and lookup.
    pub normalized_email: Option<String>,
    /// Name shown to other users.
    pub display_name: String,
    /// Authorization role.
    pub role: Role,
    /// Lifecycle state.
    pub account_status: AccountStatus,
    /// When the email address was verified (UTC).
    pub email_verified_at: Option<DateTime>,
    /// When deletion was requested (UTC); set for pending and deleted accounts.
    pub deletion_requested_at: Option<DateTime>,
    /// When the account was anonymized (UTC); set only for deleted accounts.
    pub deleted_at: Option<DateTime>,
    /// Creation time (UTC).
    pub created_at: DateTime,
    /// Last modification time (UTC).
    pub updated_at: DateTime,
}

/// Relations from a user to its sign-in methods.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// The optional password credential.
    #[sea_orm(has_one = "super::local_credential::Entity")]
    LocalCredential,
    /// Linked OAuth / OpenID Connect identities.
    #[sea_orm(has_many = "super::external_identity::Entity")]
    ExternalIdentities,
}

impl Related<super::local_credential::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::LocalCredential.def()
    }
}

impl Related<super::external_identity::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ExternalIdentities.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
