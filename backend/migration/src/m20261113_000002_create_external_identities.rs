//! Creates external OAuth and OpenID Connect identity links.

use sea_orm_migration::prelude::*;

/// Creates the `external_identities` table.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE `external_identities` (
                    `id` BINARY(16) NOT NULL,
                    `user_id` BINARY(16) NOT NULL,
                    `provider` VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                    `provider_subject` VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_bin NOT NULL,
                    `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                    `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
                    CONSTRAINT `pk_external_identities` PRIMARY KEY (`id`),
                    CONSTRAINT `uq_external_identities_provider_subject` UNIQUE (`provider`, `provider_subject`),
                    INDEX `idx_external_identities_user_id` (`user_id`),
                    CONSTRAINT `fk_external_identities_user` FOREIGN KEY (`user_id`) REFERENCES `users` (`id`) ON UPDATE RESTRICT ON DELETE RESTRICT
                ) ENGINE=InnoDB
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE `external_identities`")
            .await?;

        Ok(())
    }
}
