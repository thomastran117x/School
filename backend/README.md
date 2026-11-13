# Rust backend

An Axum backend foundation with SeaORM/MySQL, Redis, and OpenSearch. The HTTP server starts even when a datastore is unavailable; `/health/ready` reports dependency availability and recovers automatically.

## Project layout

```text
backend/
├── crates/api/       Axum application organized as feature-based vertical slices
├── migration/        SeaORM migration CLI (no domain migrations yet)
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
| `REDIS_URL` | Yes | — | Redis URL (`redis://` or `rediss://`) |
| `OPENSEARCH_URL` | Yes | — | OpenSearch HTTP(S) endpoint |
| `OPENSEARCH_USERNAME` | No | — | Optional basic-auth username |
| `OPENSEARCH_PASSWORD` | No | — | Optional basic-auth password; must accompany username |
| `HEALTH_CHECK_TIMEOUT_MS` | No | `2000` | Per-dependency readiness timeout |
| `RUST_LOG` | No | `backend_api=info,tower_http=info` | Tracing filter |

The example MySQL URL disables TLS only for the localhost/isolated Compose development service. Use a TLS-enforcing URL for remotely managed or production MySQL.

The Compose OpenSearch service disables its security plugin for local development only. Do not expose it to an untrusted network or copy that setting into a production deployment.

## Migrations

The migration crate is ready for schema migrations but intentionally starts empty. It never runs implicitly during API startup.

```powershell
cargo run -p migration -- generate create_example_table
cargo run -p migration -- up
cargo run -p migration -- status
cargo run -p migration -- down
```

SeaORM's migration CLI reads `DATABASE_URL` from the environment.

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
4. Docker-backed integration tests against MySQL, Redis, and OpenSearch.

The integration suite verifies migration connectivity, liveness, healthy readiness, degraded readiness while Redis is stopped, and automatic recovery after Redis restarts. Run the same suite locally from a Bash-compatible shell:

```bash
bash scripts/integration-test.sh
```

The script uses a unique Compose project name and alternate host ports by default. It always removes its own containers, network, and ephemeral test volumes when complete. Ports and `COMPOSE_PROJECT_NAME` can be overridden through environment variables when necessary.

