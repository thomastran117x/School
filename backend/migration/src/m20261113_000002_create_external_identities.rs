//! Creates external OAuth and OpenID Connect identity links.
//!
//! A provider identity (`provider`, `provider_subject`) links to at most one user,
//! and a user links at most one identity per provider. Link rows are immutable:
//! relinking deletes the old row and inserts a new one.

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
                    `provider` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                    `provider_subject` VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_bin NOT NULL,
                    `tenant_id` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NULL,
                    `linked_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                    CONSTRAINT `pk_external_identities` PRIMARY KEY (`id`),
                    CONSTRAINT `uq_external_identities_provider_subject` UNIQUE (`provider`, `provider_subject`),
                    CONSTRAINT `uq_external_identities_user_provider` UNIQUE (`user_id`, `provider`),
                    CONSTRAINT `fk_external_identities_user` FOREIGN KEY (`user_id`) REFERENCES `users` (`id`) ON UPDATE RESTRICT ON DELETE CASCADE,
                    CONSTRAINT `ck_external_identities_provider` CHECK (`provider` IN ('GOOGLE', 'MICROSOFT')),
                    CONSTRAINT `ck_external_identities_subject_not_empty` CHECK (CHAR_LENGTH(`provider_subject`) > 0),
                    CONSTRAINT `ck_external_identities_tenant` CHECK (`tenant_id` IS NULL OR (`provider` = 'MICROSOFT' AND CHAR_LENGTH(`tenant_id`) > 0))
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
