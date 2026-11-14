# Repository instructions

These instructions apply to the entire repository. The project consists of a Rust backend under `backend/` and a Next.js frontend under `frontend/` (see `frontend/README.md` for its checks; read `frontend/AGENTS.md` first).

## Start here

Before changing backend code:

1. Read this file and `README.md`.
2. Read `backend/README.md` for runtime, configuration, migration, and testing details.
3. Run Rust and Compose commands from `backend/` unless a command explicitly targets the repository root.
4. Inspect the current implementation before introducing a new abstraction or dependency.

## Architecture

- The backend is a Rust 2024 workspace targeting Rust 1.92 or newer.
- `crates/api` is an Axum application organized into feature-based vertical slices.
- Put product behavior in `crates/api/src/features/<feature>/`. A feature owns its handlers, transport models, service logic, routes, and feature-local tests.
- Expose each feature router from its `mod.rs`, then mount it in `app.rs`.
- Keep cross-cutting runtime concerns in top-level modules such as `config`, `state`, `telemetry`, and `shutdown`.
- Put external client construction in `infrastructure`. Move code there only when it is genuinely shared across features.
- `migration` is the SeaORM migration crate. Schema changes must be represented by migrations and must not run implicitly during API startup.
- The API intentionally starts when datastores are unavailable. Preserve lazy client initialization and report dependency failures through readiness rather than terminating the process.

## Rust conventions

- Keep `cargo fmt` clean and treat every Clippy warning as an error.
- Preserve the workspace MSRV unless a deliberate toolchain upgrade is part of the task.
- Prefer explicit, typed errors with `thiserror` in reusable modules. Use `anyhow` for executable startup and top-level context.
- Use `tracing` for operational events; do not use `println!` in application code.
- Avoid `unwrap` and `expect` in production paths. They are acceptable in tests when failure indicates a broken test fixture.
- Keep async work on Tokio and avoid blocking calls in request handlers.
- Keep secrets and connection URLs out of logs, HTTP responses, source code, and committed `.env` files.
- Add `//!` module documentation and `///` documentation for types, fields, traits, and meaningful functions. The crates enable missing-doc and broken-link linting.
- Add dependencies at the workspace level when multiple members may share them. Commit `Cargo.lock` changes.

## Feature and API changes

- Keep handlers thin: extract input, call a feature service, and translate the result into an HTTP response.
- Keep response and request models inside the owning feature unless they are truly shared.
- Inject feature service contracts through `AppState` when handlers need external behavior. This keeps router tests independent from live datastores.
- Do not expose internal errors, credentials, or infrastructure details through API responses.
- When adding configuration, update validation, `.env.example`, Compose wiring when relevant, and `backend/README.md`.
- When adding an endpoint, document its route and behavior and cover success and failure status codes.

## Datastores and migrations

- Use SeaORM for MySQL access and the `migration` crate for schema evolution.
- Use the reconnecting Redis connection manager for Redis operations.
- Use the official OpenSearch client already configured by the project.
- Keep local-only insecure settings local. In particular, OpenSearch security and MySQL TLS are disabled only for the isolated development stack.
- Never delete or reset development volumes unless the user explicitly requests it. The integration harness may remove its own uniquely named ephemeral volumes.

## Required validation

Run the checks relevant to the change from `backend/`. Before handing off a normal backend change, run at least:

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
docker compose config --quiet
```

Also run the following when applicable:

```bash
# Documentation or public-contract changes
cargo doc --workspace --no-deps --document-private-items

# Datastore, startup, readiness, migration, or Compose changes
bash scripts/integration-test.sh

# Container packaging changes
docker compose --profile full build api
```

The integration script starts an isolated MySQL, Redis, and OpenSearch stack, verifies healthy and degraded readiness, and removes only its own resources.

## Tests

- Place focused unit tests next to the code they exercise.
- Test feature handlers through the Axum router and mock the feature service boundary.
- Cover configuration defaults and validation failures without mutating process-global environment variables.
- Use `scripts/integration-test.sh` for behavior that requires real datastores.
- Keep tests deterministic, parallel-safe, and free of dependencies on pre-existing local services.

## Documentation and handoff

- Keep root documentation concise and repository-oriented; keep detailed backend operations in `backend/README.md`.
- Update documentation in the same change when commands, environment variables, endpoints, layout, or behavior change.
- In the final handoff, summarize behavior changed, list important files, report validation performed, and disclose anything that was not verified.

## Generated and local files

- Do not commit `backend/target/`, `.env`, logs, editor settings, or local datastore data.
- Do commit Rust source, migrations, `Cargo.lock`, Compose and Docker configuration, CI workflows, scripts, and maintained documentation.

