//! Creates user accounts and their lifecycle fields.
//!
//! Credentials live in `local_credentials` and federated logins in
//! `external_identities`, so a user row never holds secret material. A deleted
//! account is kept as an anonymized tombstone (both email columns `NULL`) so
//! historical applications and reviews keep their foreign keys.

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
                    `email` VARCHAR(320) NULL,
                    `normalized_email` VARCHAR(320) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_bin NULL,
                    `display_name` VARCHAR(255) NOT NULL,
                    `role` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'STUDENT',
                    `account_status` VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'UNVERIFIED',
                    `email_verified_at` DATETIME(6) NULL,
                    `deletion_requested_at` DATETIME(6) NULL,
                    `deleted_at` DATETIME(6) NULL,
                    `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                    `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
                    CONSTRAINT `pk_users` PRIMARY KEY (`id`),
                    CONSTRAINT `uq_users_normalized_email` UNIQUE (`normalized_email`),
                    INDEX `idx_users_deletion_queue` (`account_status`, `deletion_requested_at`),
                    CONSTRAINT `ck_users_role` CHECK (`role` IN ('STUDENT', 'ADMIN')),
                    CONSTRAINT `ck_users_account_status` CHECK (`account_status` IN ('UNVERIFIED', 'ACTIVE', 'DISABLED', 'PENDING_DELETION', 'DELETED')),
                    CONSTRAINT `ck_users_deleted_state` CHECK ((`account_status` = 'DELETED') = (`deleted_at` IS NOT NULL)),
                    CONSTRAINT `ck_users_deletion_requested` CHECK ((`account_status` IN ('PENDING_DELETION', 'DELETED')) = (`deletion_requested_at` IS NOT NULL)),
                    CONSTRAINT `ck_users_email_presence` CHECK (((`email` IS NULL) = (`normalized_email` IS NULL)) AND (`email` IS NOT NULL OR `account_status` = 'DELETED')),
                    CONSTRAINT `ck_users_verification_state` CHECK ((`account_status` <> 'ACTIVE' OR `email_verified_at` IS NOT NULL) AND (`account_status` <> 'UNVERIFIED' OR `email_verified_at` IS NULL))
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
