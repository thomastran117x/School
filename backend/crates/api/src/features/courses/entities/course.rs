//! The `courses` table: one row per learning course.

use sea_orm::entity::prelude::*;

use crate::features::{auth::identity::entities::user, courses::models::CourseStatus};

/// One learning course.
///
/// The generated `term_key` column (`COALESCE(term, '')`, used only by the
/// per-owner uniqueness constraint) is intentionally absent: MySQL rejects
/// writes to generated columns, and it carries no information beyond `term`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "courses")]
pub struct Model {
    /// Immutable application-generated UUIDv7.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// User who owns and manages the course.
    pub owner_user_id: Uuid,
    /// Display title (1–200 characters).
    pub title: String,
    /// Optional catalogue code such as `CSC101` (1–32 characters).
    pub code: Option<String>,
    /// Optional academic term such as `Fall 2026` (1–64 characters).
    pub term: Option<String>,
    /// Course description (up to 10 000 characters, may be empty).
    pub description: String,
    /// Lifecycle state.
    pub status: CourseStatus,
    /// When the course was first published (UTC); kept through archive and restore.
    pub published_at: Option<DateTime>,
    /// When the course was archived (UTC); set exactly while archived.
    pub archived_at: Option<DateTime>,
    /// Creation time (UTC).
    pub created_at: DateTime,
    /// Last modification time (UTC).
    pub updated_at: DateTime,
}

/// Relations from a course to other tables.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// Instructors and students of the course, including the owner.
    #[sea_orm(has_many = "super::course_membership::Entity")]
    Memberships,
    /// The owning user.
    #[sea_orm(
        belongs_to = "user::Entity",
        from = "Column::OwnerUserId",
        to = "user::Column::Id",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Owner,
}

impl Related<user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Owner.def()
    }
}

impl Related<super::course_membership::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Memberships.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
