//! Creates password credentials for local sign-in.
//!
//! A user has at most one credential row; OAuth-only users have none. Only
//! Argon2id PHC strings are accepted, which structurally rules out storing a
//! plaintext password.

use sea_orm_migration::prelude::*;

/// Creates the `local_credentials` table.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE `local_credentials` (
                    `user_id` BINARY(16) NOT NULL,
                    `password_hash` VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                    `password_algorithm` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'ARGON2ID',
                    `password_changed_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                    `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                    `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
                    CONSTRAINT `pk_local_credentials` PRIMARY KEY (`user_id`),
                    CONSTRAINT `fk_local_credentials_user` FOREIGN KEY (`user_id`) REFERENCES `users` (`id`) ON UPDATE RESTRICT ON DELETE CASCADE,
                    CONSTRAINT `ck_local_credentials_algorithm` CHECK (`password_algorithm` IN ('ARGON2ID')),
                    CONSTRAINT `ck_local_credentials_hash_format` CHECK (`password_algorithm` <> 'ARGON2ID' OR `password_hash` LIKE '$argon2id$%')
                ) ENGINE=InnoDB
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE `local_credentials`")
            .await?;

        Ok(())
    }
}
