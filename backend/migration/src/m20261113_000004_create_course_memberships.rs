//! Creates course memberships: which users belong to which course, and how.
//!
//! A membership grants a course role, `INSTRUCTOR` or `STUDENT`. Course roles
//! are independent of the global account role in `users.role`: a globally
//! `STUDENT` account may teach a course, and an `ADMIN` has no implicit course
//! role.
//!
//! The composite primary key `(course_id, user_id)` is the uniqueness rule (a
//! user holds at most one role per course) and serves membership authorization
//! as a point lookup. The two secondary indexes serve "My Courses" listings and
//! course rosters; InnoDB appends the primary key to them, so both cover a join
//! to `courses` or `users`.
//!
//! Both foreign keys `RESTRICT`: archiving a course or tombstoning a user keeps
//! their memberships, and neither can be hard-deleted or re-keyed while
//! referenced. The invariant that a course owner is always an `INSTRUCTOR`
//! member cannot be expressed without circular foreign keys or triggers, so the
//! API's course store maintains it transactionally (see "Course membership
//! schema" in the backend README).

use sea_orm_migration::prelude::*;

/// Creates the `course_memberships` table.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE `course_memberships` (
                    `course_id` BINARY(16) NOT NULL,
                    `user_id` BINARY(16) NOT NULL,
                    `role` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                    `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                    CONSTRAINT `pk_course_memberships` PRIMARY KEY (`course_id`, `user_id`),
                    INDEX `idx_course_memberships_user_role_created` (`user_id`, `role`, `created_at`),
                    INDEX `idx_course_memberships_course_role_created` (`course_id`, `role`, `created_at`),
                    CONSTRAINT `fk_course_memberships_course` FOREIGN KEY (`course_id`) REFERENCES `courses` (`id`) ON UPDATE RESTRICT ON DELETE RESTRICT,
                    CONSTRAINT `fk_course_memberships_user` FOREIGN KEY (`user_id`) REFERENCES `users` (`id`) ON UPDATE RESTRICT ON DELETE RESTRICT,
                    CONSTRAINT `ck_course_memberships_role` CHECK (`role` IN ('INSTRUCTOR', 'STUDENT'))
                ) ENGINE=InnoDB
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE `course_memberships`")
            .await?;

        Ok(())
    }
}
