//! Creates student course applications.

use sea_orm_migration::prelude::*;

/// Creates the `applications` table.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE `applications` (
                    `id` BINARY(16) NOT NULL,
                    `student_id` BINARY(16) NOT NULL,
                    `course_id` BINARY(16) NOT NULL,
                    `status` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'PENDING',
                    `submitted_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                    `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
                    CONSTRAINT `pk_applications` PRIMARY KEY (`id`),
                    CONSTRAINT `uq_applications_student_course` UNIQUE (`student_id`, `course_id`),
                    INDEX `idx_applications_student_submitted_at` (`student_id`, `submitted_at`),
                    INDEX `idx_applications_course_status` (`course_id`, `status`),
                    INDEX `idx_applications_status_submitted_at` (`status`, `submitted_at`),
                    CONSTRAINT `fk_applications_student` FOREIGN KEY (`student_id`) REFERENCES `users` (`id`) ON UPDATE RESTRICT ON DELETE RESTRICT,
                    CONSTRAINT `fk_applications_course` FOREIGN KEY (`course_id`) REFERENCES `courses` (`id`) ON UPDATE RESTRICT ON DELETE RESTRICT,
                    CONSTRAINT `ck_applications_status` CHECK (`status` IN ('PENDING', 'ACCEPTED', 'REJECTED'))
                ) ENGINE=InnoDB
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE `applications`")
            .await?;

        Ok(())
    }
}
