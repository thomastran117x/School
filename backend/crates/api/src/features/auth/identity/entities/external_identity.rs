//! The `external_identities` table: OAuth / OpenID Connect identities linked to users.

use sea_orm::entity::prelude::*;

use crate::features::auth::identity::models::IdentityProvider;

/// One provider identity linked to one user.
///
/// (`provider`, `provider_subject`) is unique, and each user has at most one
/// identity per provider. Rows are never updated; relinking replaces the row.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "external_identities")]
pub struct Model {
    /// Application-generated UUIDv7.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Linked user.
    pub user_id: Uuid,
    /// Identity provider.
    pub provider: IdentityProvider,
    /// The provider's immutable `sub` claim (case-sensitive).
    pub provider_subject: String,
    /// Microsoft `tid` claim; always `None` for other providers.
    pub tenant_id: Option<String>,
    /// When the identity was linked (UTC).
    pub linked_at: DateTime,
}

/// Relation from an identity to its user.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// The linked user.
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
