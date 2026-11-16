//! The `course_memberships` table: one row per user enrolled in or teaching a course.

use sea_orm::entity::prelude::*;

use crate::features::{auth::identity::entities::user, courses::membership::CourseRole};

/// One user's membership of one course.
///
/// The composite primary key `(course_id, user_id)` allows at most one role per
/// user and course.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "course_memberships")]
pub struct Model {
    /// The course the membership belongs to.
    #[sea_orm(primary_key, auto_increment = false)]
    pub course_id: Uuid,
    /// The member.
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    /// Role within this course; independent of the account's global role.
    pub role: CourseRole,
    /// When the membership was created (UTC).
    pub created_at: DateTime,
}

/// Relations from a membership to its course and user.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// The course.
    #[sea_orm(
        belongs_to = "super::course::Entity",
        from = "Column::CourseId",
        to = "super::course::Column::Id",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Course,
    /// The member.
    #[sea_orm(
        belongs_to = "user::Entity",
        from = "Column::UserId",
        to = "user::Column::Id",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    User,
}

impl Related<super::course::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Course.def()
    }
}

impl Related<user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
