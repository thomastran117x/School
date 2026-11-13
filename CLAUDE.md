# Claude instructions

`AGENTS.md` is the canonical repository instruction file and applies to every task. Read and follow it before making changes. This file is a concise entry point and does not override `AGENTS.md`.

## Backend quick reference

- Work from `backend/` for Cargo, Docker Compose, migration, and integration commands.
- Read `backend/README.md` before changing configuration, runtime behavior, migrations, or local infrastructure.
- Keep the Axum API feature-based: add product behavior under `crates/api/src/features/<feature>/` and keep handlers, models, services, routes, and tests together.
- Keep shared datastore client construction in `infrastructure`; do not turn feature-specific logic into a global layer.
- Preserve lazy MySQL, Redis, and OpenSearch initialization so the server can start unready and recover.
- Use SeaORM migrations for schema changes. Never modify a shared schema manually as the implementation of a task.
- Maintain Rust documentation comments and the repository's missing-doc and broken-link lint standards.
- Never commit secrets or `.env`, expose internal failures in API responses, or delete persistent Compose volumes without explicit authorization.

## Before finishing

Run formatting, strict Clippy, compile checks, and tests as described in `AGENTS.md`. Run `bash scripts/integration-test.sh` for datastore, readiness, migration, startup, or Compose changes. Report exactly which checks passed and any checks that were not run.

