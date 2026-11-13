# Rust backend

An Axum backend foundation with SeaORM/MySQL, Redis, and OpenSearch. The HTTP server starts even when a datastore is unavailable; `/health/ready` reports dependency availability and recovers automatically.

## Project layout

```text
backend/
├── crates/api/       Axum application organized as feature-based vertical slices
├── migration/        Versioned SeaORM migrations and migration CLI
├── compose.yaml      Local datastores and optional complete stack
└── Dockerfile        Multi-stage production-style API image
```

Inside the API crate, each product capability lives under `src/features/<feature>/` with its handlers, models, service logic, and feature-local tests. The current `health` feature is the reference slice. Cross-cutting runtime concerns stay outside feature folders:

```text
src/
├── features/
│   └── health/
│       ├── handlers.rs
│       ├── models.rs
│       ├── service.rs
│       └── mod.rs
├── infrastructure/   Shared datastore client construction
├── app.rs             Top-level router composition and middleware
├── config.rs          Environment configuration
├── state.rs           Dependencies shared with feature routers
├── telemetry.rs
├── shutdown.rs
└── main.rs
```

New domain features should expose their router from `features/<feature>/mod.rs`; feature-specific models and business logic should remain within that folder. Move code into shared infrastructure only when multiple features actually consume it.

## Run locally

Requirements: Rust 1.92 or newer, Docker Desktop, and Docker Compose.

```powershell
Copy-Item .env.example .env
docker compose up --detach --wait mysql redis opensearch
cargo run -p migration -- up
cargo run -p backend-api
```

The API listens on `http://127.0.0.1:3000` by default:

```powershell
Invoke-RestMethod http://127.0.0.1:3000/health/live
Invoke-RestMethod http://127.0.0.1:3000/health/ready
```

Liveness returns HTTP 200 whenever the process can serve requests. Readiness checks MySQL, Redis, and OpenSearch concurrently and returns HTTP 503 if any dependency is unavailable. Connection errors are logged internally but are not returned in the response.

## Run the complete stack in Docker

The `full` profile includes the API image. The API intentionally depends only on the datastore containers starting, not becoming healthy, so it can start in an unready state and recover.

```powershell
Copy-Item .env.example .env
docker compose --profile full up --detach --build
docker compose --profile full ps
```

Stop the stack without deleting data:

```powershell
docker compose --profile full down
```

To deliberately reset all local datastore data, run `docker compose --profile full down --volumes`. This permanently deletes the named development volumes.

## Configuration

Configuration comes from environment variables. For local Cargo runs, `dotenvy` loads `.env` when present. The file is ignored by Git.

| Variable | Required | Default | Purpose |
| --- | --- | --- | --- |
| `APP_HOST` | No | `0.0.0.0` | HTTP bind host |
| `APP_PORT` | No | `3000` | HTTP bind port |
| `DATABASE_URL` | Yes | — | MySQL connection URL |
| `DATABASE_MIN_CONNECTIONS` | No | `0` | Minimum lazy pool size |
| `DATABASE_MAX_CONNECTIONS` | No | `10` | Maximum pool size |
| `DATABASE_CONNECT_TIMEOUT_MS` | No | `2000` | MySQL connection timeout |
| `DATABASE_ACQUIRE_TIMEOUT_MS` | No | `2000` | Maximum wait for a pooled MySQL connection |
| `DATABASE_QUERY_TIMEOUT_MS` | No | `5000` | Maximum duration of one bounded database operation |
| `DATABASE_TLS_MODE` | No | `verify_identity` | MySQL TLS policy: `disabled`, `preferred`, `required`, `verify_ca`, or `verify_identity` |
| `DATABASE_TLS_CA_PATH` | No | — | Optional additional CA bundle for `verify_ca` or `verify_identity` |
| `REDIS_URL` | Yes | — | Redis URL (`redis://` or `rediss://`) |
| `OPENSEARCH_URL` | Yes | — | OpenSearch HTTP(S) endpoint |
| `OPENSEARCH_USERNAME` | No | — | Optional basic-auth username |
| `OPENSEARCH_PASSWORD` | No | — | Optional basic-auth password; must accompany username |
| `HEALTH_CHECK_TIMEOUT_MS` | No | `2000` | Per-dependency readiness timeout |
| `RUST_LOG` | No | `backend_api=info,tower_http=info` | Tracing filter |

The example environment explicitly disables MySQL TLS only for the localhost/isolated Compose development service. When the mode is omitted, the API and migration CLI require TLS with certificate and hostname verification. `verify_ca` verifies the certificate chain without checking the hostname; `required` encrypts transport without authenticating the server; `preferred` can fall back to plaintext. Configure TLS only through the dedicated variables; `DATABASE_URL` must not contain `ssl-mode` or `ssl-ca`. A supplied CA path must identify a readable file and is accepted only by a verifying mode. If the API runs in a container, mount that file and configure its container path.

The API creates one lazy SeaORM pool and shares its cloneable handle through application state. Startup validates the connection URL, TLS policy, pool bounds, and timeout values without contacting MySQL or logging the URL. Database-backed features should run operations through the bounded database client so pool exhaustion and query deadlines become sanitized typed errors rather than raw SQLx or SeaORM failures.

The Compose OpenSearch service disables its security plugin for local development only. Do not expose it to an untrusted network or copy that setting into a production deployment.

## Migrations

The migration crate owns the authoritative MySQL schema for users, external identities, courses, applications, and application reviews. Migrations never run implicitly during API startup; run them as an explicit release step before deploying application instances that depend on the new schema.

All primary and foreign keys use application-generated UUIDv7 values stored as `BINARY(16)`. Application timestamps are UTC `DATETIME(6)` values. The database enforces account roles and states, application and review states, positive course capacity, unique emails and course codes, one application per student and course, one final review per application, and audit-preserving foreign keys.

For local development, apply and inspect migrations with:

```powershell
cargo run -p migration -- generate create_example_table
cargo run -p migration -- up
cargo run -p migration -- status
cargo run -p migration -- down --num 1
```

The migration CLI reads `DATABASE_URL`, `DATABASE_TLS_MODE`, and optional `DATABASE_TLS_CA_PATH` from the environment. It applies the same secure TLS default as the API.

The production image includes the same migration executable as the workspace. After building the image, the Compose-backed equivalent of a deployment migration job is:

```powershell
docker compose up --detach --wait mysql
docker compose --profile full build api
docker compose --profile full run --rm --no-deps --entrypoint migration api up
```

For another deployment platform, run `/usr/local/bin/migration up` from the release image as a one-off job with the same database URL and TLS environment used by the API. Run only one migration job at a time and wait for it to succeed before rolling out the corresponding API release.

### Rollback and recovery

Prefer a forward corrective migration after a release has reached a shared environment. Before any rollback, take a verified database backup and roll back application instances that depend on the newer schema. `migration down --num 1` reverses only the latest migration; the initial domain migrations drop their tables and therefore destroy the data in those tables.

Use `migration reset`, `migration refresh`, and `migration fresh` only against disposable local or CI databases. MySQL commits data-definition statements implicitly. If a migration fails, inspect `migration status` and the actual schema before retrying. Do not expect API restarts to repair the schema. For an ambiguous or destructive partial failure, stop deployment and restore the verified backup or ship a reviewed corrective migration.

The migration contract test uses the isolated Compose database to exercise clean apply, idempotent reapply, constraints, full rollback, and reapplication:

```bash
bash scripts/test-migrations.sh
```

## Persistence and transactions

Feature-owned repositories use the shared `DatabaseClient` rather than creating pools or bypassing configured timeouts. SQL values are always passed through SeaORM statement parameters; repository code must not construct SQL by interpolating request or domain values.

`DatabaseClient::run_transaction` bounds an entire read-write transaction with the configured query timeout and rolls back when either the database or the feature operation returns an error. Application submission locks the course while checking active state and deadline, then relies on the unique student-course constraint as the authoritative duplicate guard.

An application decision locks the pending application before changing it. Acceptances also lock the course and perform a current locking read of accepted applications before comparing against capacity. This serializes last-seat decisions even under MySQL repeatable-read isolation. All future code that changes course capacity must update the same course row within its transaction so it participates in this locking convention. Application status and its final review are written in one transaction; failure of either write rolls both back.

Repositories expose domain outcomes such as duplicate application, invalid transition, and full capacity. SQL, connection URLs, driver messages, and credentials remain in internal error sources and are never used as client-facing messages.

## Quality checks

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps --document-private-items
docker compose config --quiet
```

## Continuous integration

The GitHub Actions workflow at `.github/workflows/backend-ci.yml` runs for backend changes on pull requests and pushes, and can also be started manually. It performs these gates in order:

1. Rust formatting and strict Clippy checks.
2. Locked compile checks and a complete workspace build.
3. Unit tests for library and binary targets.
4. Docker-backed migration and integration tests against clean MySQL, Redis, and OpenSearch services.
5. A production image build and smoke test of its packaged migration CLI.

The integration suite applies, rolls back, and reapplies migrations while checking schema constraints, verifies transactional application submission and concurrent review decisions, then checks required-TLS MySQL connectivity, bounded queries, pool exhaustion, liveness, healthy readiness, degraded readiness while Redis or MySQL is stopped, and automatic recovery after each service restarts. Run the same suite locally from a Bash-compatible shell:

```bash
bash scripts/integration-test.sh
```

The script uses a unique Compose project name and alternate host ports by default. It always removes its own containers, network, and ephemeral test volumes when complete. Ports and `COMPOSE_PROJECT_NAME` can be overridden through environment variables when necessary.

