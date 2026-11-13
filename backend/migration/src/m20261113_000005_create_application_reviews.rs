//! Creates final application review records.

use sea_orm_migration::prelude::*;

/// Creates the `application_reviews` table.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE `application_reviews` (
                    `id` BINARY(16) NOT NULL,
                    `application_id` BINARY(16) NOT NULL,
                    `reviewer_id` BINARY(16) NOT NULL,
                    `decision` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                    `comments` TEXT NULL,
                    `reviewed_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                    CONSTRAINT `pk_application_reviews` PRIMARY KEY (`id`),
                    CONSTRAINT `uq_application_reviews_application_id` UNIQUE (`application_id`),
                    INDEX `idx_application_reviews_reviewer_reviewed_at` (`reviewer_id`, `reviewed_at`),
                    CONSTRAINT `fk_application_reviews_application` FOREIGN KEY (`application_id`) REFERENCES `applications` (`id`) ON UPDATE RESTRICT ON DELETE RESTRICT,
                    CONSTRAINT `fk_application_reviews_reviewer` FOREIGN KEY (`reviewer_id`) REFERENCES `users` (`id`) ON UPDATE RESTRICT ON DELETE RESTRICT,
                    CONSTRAINT `ck_application_reviews_decision` CHECK (`decision` IN ('ACCEPTED', 'REJECTED'))
                ) ENGINE=InnoDB
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE `application_reviews`")
            .await?;

        Ok(())
    }
}
