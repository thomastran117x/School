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
| `REDIS_URL` | Yes | — | Redis URL (`redis://`, or `rediss://` when TLS is enabled); may contain a password |
| `REDIS_CONNECT_TIMEOUT_MS` | No | `2000` | Redis connection timeout |
| `REDIS_COMMAND_TIMEOUT_MS` | No | `1000` | Maximum duration of one Redis command, including reconnection |
| `REDIS_TLS_MODE` | No | `verify` | Redis TLS policy: `disabled`, `required`, or `verify` |
| `REDIS_TLS_CA_PATH` | No | — | Optional additional CA for `verify` |
| `OPENSEARCH_URL` | Yes | — | OpenSearch HTTP(S) endpoint |
| `OPENSEARCH_USERNAME` | No | — | Optional basic-auth username |
| `OPENSEARCH_PASSWORD` | No | — | Optional basic-auth password; must accompany username |
| `HEALTH_CHECK_TIMEOUT_MS` | No | `2000` | Per-dependency readiness timeout |
| `REFRESH_TOKEN_TTL_SECONDS` | No | `604800` | Lifetime of each refresh token (1 second to 30 days) |
| `REFRESH_FAMILY_MAX_LIFETIME_SECONDS` | No | `2592000` | Absolute lifetime of a refresh-session family; at least the token TTL, at most 30 days |
| `COURSE_CACHE_DETAIL_TTL_SECONDS` | No | `300` | Lifetime of a cached course read (1 second to 30 days); also the staleness bound if an invalidation is missed |
| `COURSE_CACHE_LIST_TTL_SECONDS` | No | `60` | Lifetime of a cached catalogue page; must not exceed the detail TTL |
| `COURSE_CACHE_NEGATIVE_TTL_SECONDS` | No | `30` | Lifetime of a cached "course not found"; must not exceed the detail TTL |
| `RUST_LOG` | No | `backend_api=info,tower_http=info` | Tracing filter |

The example environment explicitly disables MySQL TLS only for the localhost/isolated Compose development service. When the mode is omitted, the API and migration CLI require TLS with certificate and hostname verification. `verify_ca` verifies the certificate chain without checking the hostname; `required` encrypts transport without authenticating the server; `preferred` can fall back to plaintext. Configure TLS only through the dedicated variables; `DATABASE_URL` must not contain `ssl-mode` or `ssl-ca`. A supplied CA path must identify a readable file and is accepted only by a verifying mode. If the API runs in a container, mount that file and configure its container path.

The API creates one lazy SeaORM pool and shares its cloneable handle through application state. Startup validates the connection URL, TLS policy, pool bounds, and timeout values without contacting MySQL or logging the URL. Database-backed features should run operations through the bounded database client so pool exhaustion and query deadlines become sanitized typed errors rather than raw SQLx or SeaORM failures.

The API creates one lazy, auto-reconnecting Redis connection manager at startup and shares its cloneable handle through `AppState`. `REDIS_TLS_MODE` defaults to `verify` (certificate and hostname checks, optional custom CA); `required` encrypts without authenticating the server; `disabled` is for the isolated local stack only. `rediss://` must be used exactly when TLS is enabled. Startup validates the URL and TLS settings without contacting Redis or logging the URL. Every command runs through `RedisClient`, so connection, command, and TTL failures become sanitized typed errors (`RedisError`) that never include the URL, password, keys, or values.

### Redis keys and TTLs

All keys are built with `Key::new(KeyNamespace, id)` as `app:v1:<namespace>:<id>`; identifiers are opaque (ASCII alphanumerics, `-`, `_`, `.`, up to 128 bytes) and must never contain secrets or token contents. Bump `v1` to invalidate all stored data after an incompatible format change. Features use the general-purpose `Cache` (`infrastructure/cache.rs`) rather than hand-written Lua for common needs: raw and JSON get/set, `set_nx`, `get_del`, `delete_many`, `exists`, `ttl`, `expire`, `incr` (atomic INCR + EXPIRE), fail-open `get_or_load`, and `atomic` (a `MULTI`/`EXEC` group). A TTL is mandatory on every write, at least one second, and at most 30 days. Lua is reserved for logic that is both atomic and conditional (refresh-token rotation and revocation, the course guarded fill) and always runs through the single shared `Cache::eval` invoker.

| Namespace | Prefix | Default TTL | Purpose |
| --- | --- | --- | --- |
| `Session` | `app:v1:session:` | 24 hours | Server-side session state |
| `Cache` | `app:v1:cache:` | 5 minutes | Rebuildable cached data |
| `RateLimit` | `app:v1:ratelimit:` | 1 minute | Rate-limit counters |
| `Lock` | `app:v1:lock:` | 30 seconds | Short-lived distributed locks |
| `RefreshToken` | `app:v1:refresh_token:` | `REFRESH_TOKEN_TTL_SECONDS` | One refresh token, keyed by its SHA-256 hash |
| `RefreshFamily` | `app:v1:refresh_family:` | `REFRESH_FAMILY_MAX_LIFETIME_SECONDS` | Live session family; deleting it revokes the session |
| `UserSessions` | `app:v1:user_sessions:` | `REFRESH_FAMILY_MAX_LIFETIME_SECONDS` | Set of a user's family ids, used by logout-all |
| `Course` | `app:v1:course:` | `COURSE_CACHE_*_TTL_SECONDS` | Course catalogue read cache and its invalidation counters |

### Course catalogue cache

`features/courses/cache.rs` provides cache-aside reads for the course catalogue. MySQL is the source of truth; Redis only speeds up display reads and is **never** consulted for authorization or application-acceptance decisions (for example "is this course active and open for applications?"), which must read MySQL directly.

**Read path.** `CourseCache::detail` and `CourseCache::list` look up Redis, and on a miss call the caller's database loader, then fill the cache best-effort. Any Redis failure (unavailable, timeout, undecodable payload) is logged without keys or values and the read falls back to the loader, so a Redis outage degrades performance but not correctness or availability. Loader (database) errors propagate and are never cached. A "course not found" result is cached for the shorter negative TTL.

**Keys** (all in the `Course` namespace, with a payload schema version):

| Key id | TTL | Purpose |
| --- | --- | --- |
| `detail.v1.<course_id>` | `COURSE_CACHE_DETAIL_TTL_SECONDS` (`COURSE_CACHE_NEGATIVE_TTL_SECONDS` for not-found) | One course |
| `list.v1.<generation>.<query hash>` | `COURSE_CACHE_LIST_TTL_SECONDS` | One catalogue page |
| `ver.v1.<course_id>` | 24 hours | Per-course change counter that guards fills |
| `gen.v1` | 24 hours | Catalogue generation embedded in listing keys |

**Invalidation.** After the database change commits, writers call `CourseCache::invalidate_course(id)` for any create, update, deactivation, or reactivation. It atomically deletes the detail entry and bumps the course counter and the catalogue generation, which orphans every cached listing page without scanning (orphans expire by TTL). `invalidate_catalogue()` is available for bulk changes. Invalidation is retried once; if Redis is still down, the error is returned and logged, and staleness is bounded by the TTLs above.

**Stale-fill protection.** Readers record the counter before querying MySQL and fill the cache with a Lua script that writes only if the counter is unchanged. A read that overlaps a deactivation therefore cannot re-cache the pre-deactivation row. Invalidation itself is an unconditional `MULTI`/`EXEC` group built with `Cache::atomic`. The scripts assume a single Redis node; a clustered deployment would need hash tags.

### Refresh sessions

The refresh-session store (`features/auth/refresh_store`) issues opaque 256-bit tokens and stores only their SHA-256 hash. Rotation, logout, and logout-all each run as one Lua script, so concurrent use of the same token yields at most one successor. Presenting an already-rotated token revokes its whole session family. The scripts derive family keys from stored ids, which assumes a single Redis node; a clustered deployment needs hash tags. Every Redis failure surfaces as `RefreshStoreError::Unavailable`, which callers must treat as a denial (fail closed).

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

The integration suite applies, rolls back, and reapplies migrations while checking schema constraints, then verifies required-TLS MySQL connectivity, bounded queries, pool exhaustion, liveness, healthy readiness, degraded readiness while Redis or MySQL is stopped, and automatic recovery after each service restarts. Run the same suite locally from a Bash-compatible shell:

```bash
bash scripts/integration-test.sh
```

The script uses a unique Compose project name and alternate host ports by default. It always removes its own containers, network, and ephemeral test volumes when complete. Ports and `COMPOSE_PROJECT_NAME` can be overridden through environment variables when necessary.

