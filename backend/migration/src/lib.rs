//! SeaORM migration registry for the backend's MySQL schema.
//!
//! New migrations should be added as modules and returned from
//! [`MigratorTrait::migrations`] in execution order.
#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

use async_trait::async_trait;
pub use sea_orm_migration::prelude::*;

/// Registers every schema migration known to the backend.
///
/// The registry is intentionally empty until the first domain feature introduces a
/// persistent MySQL schema.
pub struct Migrator;

#[async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        Vec::new()
    }
}
