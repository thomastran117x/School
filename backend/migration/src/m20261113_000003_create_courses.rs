//! Creates authoritative course offerings.

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
                    `code` VARCHAR(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
                    `name` VARCHAR(255) NOT NULL,
                    `description` TEXT NOT NULL,
                    `capacity` INT UNSIGNED NOT NULL,
                    `application_deadline` DATETIME(6) NOT NULL,
                    `is_active` TINYINT UNSIGNED NOT NULL DEFAULT 1,
                    `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                    `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
                    CONSTRAINT `pk_courses` PRIMARY KEY (`id`),
                    CONSTRAINT `uq_courses_code` UNIQUE (`code`),
                    INDEX `idx_courses_availability` (`is_active`, `application_deadline`),
                    CONSTRAINT `ck_courses_capacity` CHECK (`capacity` > 0),
                    CONSTRAINT `ck_courses_is_active` CHECK (`is_active` IN (0, 1))
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
