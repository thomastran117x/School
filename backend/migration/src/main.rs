//! Command-line entry point for applying and inspecting SeaORM migrations.
#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

#[tokio::main]
/// Runs the standard SeaORM migration CLI against `DATABASE_URL`.
async fn main() {
    sea_orm_migration::cli::run_cli(migration::Migrator).await;
}
