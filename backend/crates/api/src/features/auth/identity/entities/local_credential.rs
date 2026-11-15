//! The `local_credentials` table: at most one password credential per user.

use sea_orm::entity::prelude::*;

use crate::features::auth::identity::models::{PasswordAlgorithm, PasswordHash};

/// A user's password credential. OAuth-only users have no row.
///
/// `password_hash` is a [`PasswordHash`], so the derived `Debug` of both this
/// model and its active model print `[REDACTED]` instead of the hash.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "local_credentials")]
pub struct Model {
    /// Owning user; also the primary key.
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    /// Argon2id PHC string.
    pub password_hash: PasswordHash,
    /// Hashing algorithm, used to plan rehashing.
    pub password_algorithm: PasswordAlgorithm,
    /// When the password was last set (UTC).
    pub password_changed_at: DateTime,
    /// Creation time (UTC).
    pub created_at: DateTime,
    /// Last modification time (UTC).
    pub updated_at: DateTime,
}

/// Relation from a credential to its user.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// The owning user.
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_update = "Restrict",
        on_delete = "Cascade"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
