//! Creates learning courses owned by a user.
//!
//! A course moves through `DRAFT → PUBLISHED ⇄ ARCHIVED` (a draft may also be
//! archived directly). Publishing and archiving are status changes only: rows
//! are never hard-deleted by the lifecycle, so coursework and memberships that
//! reference a course keep their foreign keys. `published_at` records the first
//! publication and `archived_at` is set exactly while the course is archived.
//!
//! Uniqueness: an owner cannot reuse the same `code` in the same `term`. The
//! comparison is case- and accent-insensitive, a missing term counts as its own
//! value (through the generated `term_key`, because MySQL `UNIQUE` ignores
//! `NULL`), courses without a code are never constrained, and the rule covers
//! archived courses too. Different owners may share a code and term, for
//! example parallel sections.
//!
//! There is deliberately no application deadline or seat capacity; see the
//! "Course schema" section of the backend README for future extensions.

use sea_orm_migration::prelude::*;

/// Creates the `courses` table.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE `courses` (
                    `id` BINARY(16) NOT NULL,
                    `owner_user_id` BINARY(16) NOT NULL,
                    `title` VARCHAR(200) NOT NULL,
                    `code` VARCHAR(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL,
                    `term` VARCHAR(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL,
                    `term_key` VARCHAR(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci GENERATED ALWAYS AS (COALESCE(`term`, '')) STORED NOT NULL,
                    `description` TEXT NOT NULL,
                    `status` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'DRAFT',
                    `published_at` DATETIME(6) NULL,
                    `archived_at` DATETIME(6) NULL,
                    `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                    `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
                    CONSTRAINT `pk_courses` PRIMARY KEY (`id`),
                    CONSTRAINT `uq_courses_owner_code_term` UNIQUE (`owner_user_id`, `code`, `term_key`),
                    INDEX `idx_courses_owner_status` (`owner_user_id`, `status`, `created_at`),
                    INDEX `idx_courses_status_created_at` (`status`, `created_at`),
                    CONSTRAINT `fk_courses_owner` FOREIGN KEY (`owner_user_id`) REFERENCES `users` (`id`) ON UPDATE RESTRICT ON DELETE RESTRICT,
                    CONSTRAINT `ck_courses_status` CHECK (`status` IN ('DRAFT', 'PUBLISHED', 'ARCHIVED')),
                    CONSTRAINT `ck_courses_title_not_blank` CHECK (CHAR_LENGTH(TRIM(`title`)) > 0),
                    CONSTRAINT `ck_courses_code_not_blank` CHECK (`code` IS NULL OR CHAR_LENGTH(TRIM(`code`)) > 0),
                    CONSTRAINT `ck_courses_term_not_blank` CHECK (`term` IS NULL OR CHAR_LENGTH(TRIM(`term`)) > 0),
                    CONSTRAINT `ck_courses_description_length` CHECK (CHAR_LENGTH(`description`) <= 10000),
                    CONSTRAINT `ck_courses_published_state` CHECK ((`status` <> 'PUBLISHED' OR `published_at` IS NOT NULL) AND (`status` <> 'DRAFT' OR `published_at` IS NULL)),
                    CONSTRAINT `ck_courses_archived_state` CHECK ((`status` = 'ARCHIVED') = (`archived_at` IS NOT NULL))
                ) ENGINE=InnoDB
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE `courses`")
            .await?;

        Ok(())
    }
}
