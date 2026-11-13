//! Creates user accounts and their authentication lifecycle fields.

use sea_orm_migration::prelude::*;

/// Creates the `users` table.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE `users` (
                    `id` BINARY(16) NOT NULL,
                    `email` VARCHAR(320) NOT NULL,
                    `normalized_email` VARCHAR(320) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_bin NOT NULL,
                    `display_name` VARCHAR(255) NOT NULL,
                    `password_hash` VARCHAR(255) NULL,
                    `role` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'STUDENT',
                    `account_status` VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'UNVERIFIED',
                    `email_verified_at` DATETIME(6) NULL,
                    `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                    `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
                    `deleted_at` DATETIME(6) NULL,
                    CONSTRAINT `pk_users` PRIMARY KEY (`id`),
                    CONSTRAINT `uq_users_normalized_email` UNIQUE (`normalized_email`),
                    CONSTRAINT `ck_users_role` CHECK (`role` IN ('STUDENT', 'ADMIN')),
                    CONSTRAINT `ck_users_account_status` CHECK (`account_status` IN ('UNVERIFIED', 'ACTIVE', 'DISABLED', 'DELETED')),
                    CONSTRAINT `ck_users_deleted_state` CHECK ((`account_status` = 'DELETED') = (`deleted_at` IS NOT NULL))
                ) ENGINE=InnoDB
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE `users`")
            .await?;

        Ok(())
    }
}
