//! SeaORM migration registry for the backend's MySQL schema.
//!
//! Migrations are registered in dependency order and are never run implicitly by
//! the API process.
#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

mod m20261113_000001_create_users;
mod m20261113_000002_create_external_identities;
mod m20261113_000003_create_courses;
mod m20261113_000004_create_applications;
mod m20261113_000005_create_application_reviews;

use async_trait::async_trait;
pub use sea_orm_migration::prelude::*;

/// Registers every schema migration known to the backend.
///
pub struct Migrator;

#[async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20261113_000001_create_users::Migration),
            Box::new(m20261113_000002_create_external_identities::Migration),
            Box::new(m20261113_000003_create_courses::Migration),
            Box::new(m20261113_000004_create_applications::Migration),
            Box::new(m20261113_000005_create_application_reviews::Migration),
        ]
    }
}
