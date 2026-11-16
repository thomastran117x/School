//! Course roles and the validated membership insert model.
//!
//! Enum string values must match the `course_memberships` migration exactly;
//! the unit tests below pin them.

use sea_orm::{ActiveValue::Set, entity::prelude::*};

use super::entities::course_membership;

/// A user's role within one course.
///
/// This is deliberately a separate type from the global account role
/// (`identity::Role`, `STUDENT`/`ADMIN`), with no conversion between them. A
/// course role is granted per course by a membership row: a globally `STUDENT`
/// account may be the `INSTRUCTOR` of a course it owns, and an `ADMIN` account
/// has no course role unless it is a member.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")]
pub enum CourseRole {
    /// Teaches and manages the course. Every course owner holds this role.
    #[sea_orm(string_value = "INSTRUCTOR")]
    Instructor,
    /// Takes the course.
    #[sea_orm(string_value = "STUDENT")]
    Student,
}

/// Insert model for a course membership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewCourseMembership {
    course_id: Uuid,
    user_id: Uuid,
    role: CourseRole,
}

impl NewCourseMembership {
    /// Describes a membership of `user_id` in `course_id` with `role`.
    pub fn new(course_id: Uuid, user_id: Uuid, role: CourseRole) -> Self {
        Self {
            course_id,
            user_id,
            role,
        }
    }

    /// Builds the insert, leaving `created_at` to MySQL.
    pub fn into_active_model(self) -> course_membership::ActiveModel {
        course_membership::ActiveModel {
            course_id: Set(self.course_id),
            user_id: Set(self.user_id),
            role: Set(self.role),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveEnum, Iterable};

    use super::*;

    #[test]
    fn role_values_match_the_migration_check_constraint() {
        let values: Vec<String> = CourseRole::iter().map(|role| role.to_value()).collect();
        assert_eq!(values, ["INSTRUCTOR", "STUDENT"]);
        assert_eq!(
            CourseRole::try_from_value(&"STUDENT".to_owned()),
            Ok(CourseRole::Student)
        );
        for rejected in ["student", "ADMIN", "TA", ""] {
            assert!(
                CourseRole::try_from_value(&rejected.to_owned()).is_err(),
                "{rejected:?}"
            );
        }
    }

    #[test]
    fn new_membership_leaves_the_creation_time_to_the_database() {
        let course_id = Uuid::now_v7();
        let user_id = Uuid::now_v7();
        let active = NewCourseMembership::new(course_id, user_id, CourseRole::Instructor)
            .into_active_model();

        assert_eq!(active.course_id, Set(course_id));
        assert_eq!(active.user_id, Set(user_id));
        assert_eq!(active.role, Set(CourseRole::Instructor));
        assert!(active.created_at.is_not_set());
    }
}
