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
| `SHUTDOWN_TIMEOUT_MS` | No | `10000` | Maximum time in-flight requests may drain after SIGTERM/SIGINT before connections are dropped; see [Graceful shutdown](#graceful-shutdown) |
| `REFRESH_TOKEN_TTL_SECONDS` | No | `604800` | Lifetime of each refresh token (1 second to 30 days) |
| `REFRESH_FAMILY_MAX_LIFETIME_SECONDS` | No | `2592000` | Absolute lifetime of a refresh-session family; at least the token TTL, at most 30 days |
| `JWT_SIGNING_KEY_ID` | Yes | — | `kid` of the active signing key: 1-64 characters of `A-Z a-z 0-9 . _ -` |
| `JWT_SIGNING_KEY` | Yes | — | Base64 Ed25519 PKCS#8 DER private key; never logged. The published dev key is refused unless `APP_ENV=development`; see [Access tokens](#access-tokens) |
| `JWT_VERIFICATION_KEYS` | No | empty | Extra public keys accepted during rotation: comma-separated `kid:base64-SPKI-DER`, at most 4, unique kids |
| `JWT_ISSUER` | No | `school-api` | `iss` claim issued and required; no whitespace, at most 256 characters |
| `JWT_AUDIENCE` | No | `school-api` | `aud` claim issued and required; same rules as the issuer |
| `JWT_ACCESS_TOKEN_TTL_SECONDS` | No | `900` | Access-token lifetime, 60 to 3600 seconds |
| `JWT_CLOCK_SKEW_SECONDS` | No | `30` | Clock-skew allowance for `exp` and `iat` checks, 0 to 120 seconds |
| `COURSE_CACHE_DETAIL_TTL_SECONDS` | No | `300` | Lifetime of a cached course read (1 second to 30 days); also the staleness bound if an invalidation is missed |
| `COURSE_CACHE_LIST_TTL_SECONDS` | No | `60` | Lifetime of a cached catalogue page; must not exceed the detail TTL |
| `COURSE_CACHE_NEGATIVE_TTL_SECONDS` | No | `30` | Lifetime of a cached "course not found"; must not exceed the detail TTL |
| `RUST_LOG` | No | `backend_api=info,tower_http=info` | Tracing filter |
| `APP_ENV` | No | `production` | `development` (readable logs), `staging` or `production` (JSON logs) |
| `SERVICE_NAME` | No | `school-api` | Service name attached to every log event |
| `HTTP_HTTPS_ENABLED` | No | `false` | Set `true` when served over HTTPS (e.g. behind a TLS proxy) to send HSTS |
| `HTTP_HSTS_MAX_AGE_SECS` | No | `31536000` | `max-age` of the HSTS header |
| `HTTP_MAX_BODY_BYTES` | No | `1048576` | Maximum request body size; larger bodies get `413` |
| `CORS_ALLOWED_ORIGINS` | No | empty | Comma-separated exact frontend origins; empty disables CORS; `*` and paths are rejected |
| `CORS_ALLOWED_METHODS` | No | `GET,POST,PUT,PATCH,DELETE,OPTIONS` | Methods allowed cross-origin |
| `CORS_ALLOWED_HEADERS` | No | `authorization,content-type,x-request-id` | Request headers allowed cross-origin |
| `CORS_ALLOW_CREDENTIALS` | No | `false` | Allow credentialed CORS; never combined with a wildcard origin |
| `CORS_MAX_AGE_SECS` | No | `600` | Preflight cache lifetime |
| `RATE_LIMIT_ENABLED` | No | `true` | Master switch for rate limiting (a startup warning is logged when off) |
| `RATE_LIMIT_GENERAL_IP_CAPACITY` / `_REFILL_PER_MINUTE` | No | `120` / `120` | Per-IP burst size and refill rate for all traffic |
| `RATE_LIMIT_AUTH_IP_CAPACITY` / `_REFILL_PER_MINUTE` | No | `10` / `6` | Per-IP limit for authentication routes; may not exceed the general limit |
| `RATE_LIMIT_AUTH_ACCOUNT_CAPACITY` / `_REFILL_PER_MINUTE` | No | `5` / `1` | Per-account limit for authentication attempts |
| `RATE_LIMIT_FALLBACK_MAX_ENTRIES` | No | `10000` | Cap on in-process buckets used while Redis is down |
| `TRUSTED_PROXY_CIDRS` | No | empty | Proxy IPs/CIDRs whose `X-Forwarded-For` is trusted; empty trusts no header |
| `METRICS_ENABLED` | No | `true` | Start the Prometheus metrics listener |
| `METRICS_ADDR` | No | `127.0.0.1:9090` | Metrics listener address; must differ from the API address |
| `METRICS_BEARER_TOKEN` | When `METRICS_ADDR` is not loopback | unset | Bearer token scrapers must send; never logged |

### HTTP hardening, errors and logging

- **Security headers:** every response carries `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer` and `X-Frame-Options: DENY`; `Strict-Transport-Security` is added only when `HTTP_HTTPS_ENABLED=true`. No `Server`/`X-Powered-By` header is set. **Content-Security-Policy is the frontend's responsibility**: this API returns JSON, so the Next.js app that serves HTML must set its own CSP.
- **Errors:** failures use `AppError` and return `{"error": {"code", "message", "request_id"}}` with 400/401/403/404/405/409/413/422/429/500/503. 422 responses (`validation_failed`) add a `fields` array of `{field, code, message}`; malformed input (bad JSON, wrong content type, bad query/path) is 400 `validation_error`. Use the `ValidatedJson`/`ValidatedQuery`/`ValidatedPath` extractors in `crates/api/src/extract/` with `serde` + `validator` DTOs (shared length limits and rules in `extract/rules.rs`); the body size limit is `HTTP_MAX_BODY_BYTES`. Internal errors are logged server-side and never returned. Unknown routes, wrong methods and oversized bodies use the same body.
- **Request IDs:** every response carries `x-request-id`. A client-supplied value is kept only if it is 1-64 characters of `A-Z a-z 0-9 - _ .`; otherwise a fresh UUID is generated. The ID is also in the span and in error bodies.
- **Logging:** one `tracing` subscriber is installed at startup. Request spans record only method, route template (for example `/health/live`, or `unmatched` when no route matched; raw paths are never logged) and request ID, and the response event adds status and latency (no headers, bodies or query strings). Fields whose names contain `authorization`, `cookie`, `token`, `password`, `secret`, `email`, `student` and similar are logged as `[REDACTED]`; redaction is name-based, so never interpolate secrets into a log message.

The example environment explicitly disables MySQL TLS only for the localhost/isolated Compose development service. When the mode is omitted, the API and migration CLI require TLS with certificate and hostname verification. `verify_ca` verifies the certificate chain without checking the hostname; `required` encrypts transport without authenticating the server; `preferred` can fall back to plaintext. Configure TLS only through the dedicated variables; `DATABASE_URL` must not contain `ssl-mode` or `ssl-ca`. A supplied CA path must identify a readable file and is accepted only by a verifying mode. If the API runs in a container, mount that file and configure its container path.

### Rate limiting

Code: `crates/api/src/middleware/rate_limit/`. Each client IP (and, on auth routes, each account) owns a token bucket in Redis, updated by one atomic Lua script, so limits hold across every replica. A bucket allows a burst of `capacity` and refills continuously; idle buckets expire. Redis keys are `app:v1:ratelimit:<sha256>`, so raw IPs and account names never reach Redis, logs or metrics. Redis supplies the clock (`TIME`), so replica clock skew is irrelevant.

- **Responses:** `429` (`rate_limited`) with `Retry-After` in whole seconds (rounded up, at least 1). `Retry-After` is exposed to cross-origin callers. `/health/*` is exempt.
- **Policies:** `general_ip` applies to every request. `auth_ip` (per IP) and `auth_account` (per normalised account) are stricter and are applied by authentication routes: add `route_layer(from_fn_with_state(limiter.ip_limit(policies.auth_ip), limit_by_ip))` and call `limiter.check_account(&policies.auth_account, email)` after the body is validated (extractors consume the body, so a layer cannot see the account).
- **Failure policy:** auth policies **fail closed** (`503` `rate_limit_unavailable` with `Retry-After`) when Redis errors or times out; this is not configurable and is never silent. The general policy degrades to a bounded in-process limiter (`RATE_LIMIT_FALLBACK_MAX_ENTRIES`; each replica enforces independently, so the cluster-wide limit is looser during an outage). When the table is full, new subjects are refused rather than evicting existing buckets, so a key flood cannot reset limits. Outages log at most one warning per 10 seconds.
- **Trusted client IP:** the TCP peer is used by default. `X-Forwarded-For` is read only when the peer is in `TRUSTED_PROXY_CIDRS`, and then from the right: trusted hops are skipped and the first untrusted address is the client. Entries to its left are client-controlled and ignored. Malformed, oversized (over 2 KiB) or long (over 32 hops) headers fall back to the peer, which shares the proxy's bucket (strict). `X-Real-IP` and `Forwarded` are never read. IPv6 clients are bucketed per /64 and IPv4-mapped addresses are unwrapped. **Leave `TRUSTED_PROXY_CIDRS` empty unless every request reaches the API through proxies you control** and that overwrite or append to `X-Forwarded-For`; otherwise clients can spoof their address. If the proxy hides client addresses entirely, all traffic shares one bucket.
- **Metrics:** `rate_limit_decisions_total{policy,outcome}`, `rate_limiter_redis_errors_total{kind}` and `rate_limiter_redis_duration_seconds`. See below.

### Metrics and observability

Metrics use the `metrics` facade with a per-process Prometheus recorder (`crates/api/src/metrics.rs`).

**Endpoint.** `GET /metrics` is served only on a dedicated listener (`METRICS_ADDR`, default `127.0.0.1:9090`), never on the public API port (which returns 404 for `/metrics`). The listener is loopback by default; binding any other address requires `METRICS_BEARER_TOKEN`, otherwise startup fails. When a token is set, scrapers must send `Authorization: Bearer <token>` (compared in constant time). Responses: `200` Prometheus text format, `401` with `WWW-Authenticate: Bearer` for a missing or wrong token, `404` for any other path. Scrapes are not counted in request metrics. The Compose `api` service binds `0.0.0.0:9090` inside the container with a token (default `local-dev-metrics-token`, override `METRICS_BEARER_TOKEN`) and does not publish the port. Keep it on a private network in real deployments.

| Metric | Type | Labels |
| --- | --- | --- |
| `http_requests_total` | counter | `method`, `route`, `status` |
| `http_request_duration_seconds` | histogram | `method`, `route`, `status_class` |
| `http_request_errors_total` | counter (4xx/5xx) | `route`, `status_class` |
| `http_requests_in_flight` | gauge | none |
| `rate_limit_decisions_total` | counter | `policy`, `outcome` (`allowed`, `limited`, `fallback_allowed`, `fallback_limited`, `fail_closed`) |
| `rate_limiter_redis_errors_total` | counter | `kind` (`unavailable`, `timeout`, `failed`) |
| `rate_limiter_redis_duration_seconds` | histogram | none |
| `dependency_up` | gauge (1 up, 0 down) | `dependency` = `mysql`, `redis`, `opensearch` |
| `dependency_probe_duration_seconds` | histogram | `dependency` |
| `dependency_probe_failures_total` | counter | `dependency` |

**Low cardinality.** `route` is the matched route template (for example `/health/live`) or `unmatched`; raw paths, path parameters, query strings and user or student IDs are never labels. `method` is a standard HTTP method or `OTHER`. When adding metrics, only use labels drawn from a fixed, small set.

**Dependency health.** Each scrape runs the same readiness probes as `/health/ready` (bounded by `HEALTH_CHECK_TIMEOUT_MS`) and sets `dependency_up`, so a down datastore shows as `0` rather than failing the scrape. Probes also feed the duration and failure series.

**Prometheus.**

```yaml
scrape_configs:
  - job_name: school-api
    metrics_path: /metrics
    authorization:
      type: Bearer
      credentials_file: /etc/prometheus/school-api-token
    static_configs:
      - targets: ["api:9090"]
```

**OpenTelemetry.** The service exposes Prometheus metrics rather than exporting OTLP itself. Scrape it with an OpenTelemetry Collector and forward to any OTLP backend:

```yaml
receivers:
  prometheus:
    config:
      scrape_configs:
        - job_name: school-api
          authorization:
            credentials_file: /etc/otelcol/school-api-token
          static_configs:
            - targets: ["api:9090"]
exporters:
  otlphttp:
    endpoint: https://otlp.example.com
service:
  pipelines:
    metrics:
      receivers: [prometheus]
      exporters: [otlphttp]
```

### Application state

`AppState` (`crates/api/src/state.rs`) is the single cloneable container handed to Axum. It holds the validated `Settings` (`Arc`), the MySQL, Redis, and OpenSearch clients, and feature service contracts (`Arc<dyn Trait>`), all built once in `main.rs`; cloning it copies only handles, so requests never create connections or pools. Handlers take `State<AppState>`, or a narrower `State<DatabaseClient>`, `State<RedisClient>`, or `State<Arc<Settings>>` through `FromRef`. To add a dependency, add it to `AppState` and `AppStateParts`, construct it in `main.rs`, and use `state::test_support::lazy_state` in router tests with a stub service.

The API creates one lazy SeaORM pool and shares its cloneable handle through application state. Startup validates the connection URL, TLS policy, pool bounds, and timeout values without contacting MySQL or logging the URL. Database-backed features should run operations through the bounded database client so pool exhaustion and query deadlines become sanitized typed errors rather than raw SQLx or SeaORM failures.

The API creates one lazy, auto-reconnecting Redis connection manager at startup and shares its cloneable handle through `AppState`. `REDIS_TLS_MODE` defaults to `verify` (certificate and hostname checks, optional custom CA); `required` encrypts without authenticating the server; `disabled` is for the isolated local stack only. `rediss://` must be used exactly when TLS is enabled. Startup validates the URL and TLS settings without contacting Redis or logging the URL. Every command runs through `RedisClient`, so connection, command, and TTL failures become sanitized typed errors (`RedisError`) that never include the URL, password, keys, or values.

### Graceful shutdown

On SIGTERM (or SIGINT/Ctrl+C) the API:

1. Stops accepting new connections and starts draining in-flight requests.
2. Waits up to `SHUTDOWN_TIMEOUT_MS` (default 10 seconds, measured from the signal). Anything still running after that is abandoned and its connections are dropped, with a `shutdown timeout elapsed` warning.
3. Closes the MySQL pool (bounded by the same timeout). Redis and OpenSearch clients hold no state that needs flushing and are released on exit.
4. Logs `shutdown complete` and exits with status 0.

**Deployment requirement:** the orchestrator's termination grace period must be longer than `SHUTDOWN_TIMEOUT_MS` plus a few seconds for pool close, otherwise the process is SIGKILLed mid-drain. The Compose `api` service sets `stop_grace_period: 15s` for the default 10s; raise both together. On Kubernetes set `terminationGracePeriodSeconds` accordingly and let readiness fail or remove the pod from the Service endpoints before SIGTERM (for example with a short `preStop` sleep). The container runs the binary as PID 1 via exec-form `ENTRYPOINT`, so it receives the signal directly; do not wrap it in a shell script.

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

### Redis operations

Refresh sessions are auth-critical and share Redis with disposable caches. Full requirements are in [docs/redis-operations.md](docs/redis-operations.md): AOF persistence (`appendfsync everysec`) plus a replica in production, `maxmemory-policy noeviction` on any instance holding sessions (every key has a TTL, so `volatile-*` policies do not protect them), explicit `maxmemory` and alert thresholds, backup and restore rules, restart and session-loss behaviour, and the recommendation to isolate auth sessions from cache workloads on a separate instance. The Compose Redis service applies the development baseline, with the memory cap set by `REDIS_MAXMEMORY` (default `256mb`).

### Course catalogue cache

`features/courses/cache.rs` provides cache-aside reads for the course catalogue. MySQL is the source of truth; Redis only speeds up display reads and is **never** consulted for authorization or access decisions (for example "is this course published?"), which must read MySQL directly.

**Read path.** `CourseCache::detail` and `CourseCache::list` look up Redis, and on a miss call the caller's database loader, then fill the cache best-effort. Any Redis failure (unavailable, timeout, undecodable payload) is logged without keys or values and the read falls back to the loader, so a Redis outage degrades performance but not correctness or availability. Loader (database) errors propagate and are never cached. A "course not found" result is cached for the shorter negative TTL.

**Keys** (all in the `Course` namespace, with a payload schema version):

| Key id | TTL | Purpose |
| --- | --- | --- |
| `detail.v1.<course_id>` | `COURSE_CACHE_DETAIL_TTL_SECONDS` (`COURSE_CACHE_NEGATIVE_TTL_SECONDS` for not-found) | One course |
| `list.v1.<generation>.<query hash>` | `COURSE_CACHE_LIST_TTL_SECONDS` | One catalogue page |
| `ver.v1.<course_id>` | 24 hours | Per-course change counter that guards fills |
| `gen.v1` | 24 hours | Catalogue generation embedded in listing keys |

**Invalidation.** After the database change commits, writers call `CourseCache::invalidate_course(id)` for any create, update, or status change (publish, archive, restore). It atomically deletes the detail entry and bumps the course counter and the catalogue generation, which orphans every cached listing page without scanning (orphans expire by TTL). `invalidate_catalogue()` is available for bulk changes. Invalidation is retried once; if Redis is still down, the error is returned and logged, and staleness is bounded by the TTLs above.

**Stale-fill protection.** Readers record the counter before querying MySQL and fill the cache with a Lua script that writes only if the counter is unchanged. A read that overlaps a status change, such as archiving, therefore cannot re-cache the earlier row. Invalidation itself is an unconditional `MULTI`/`EXEC` group built with `Cache::atomic`. The scripts assume a single Redis node; a clustered deployment would need hash tags.

### Identity model

`features/auth/identity` holds the SeaORM entities and value types for users, password credentials, and linked OAuth identities. Its rules are described in [Identity schema](#identity-schema).

### Refresh sessions

The refresh-session store (`features/auth/refresh_store`) issues opaque 256-bit tokens and stores only their SHA-256 hash. Rotation, logout, and logout-all each run as one Lua script, so concurrent use of the same token yields at most one successor. Presenting an already-rotated token revokes its whole session family. The scripts derive family keys from stored ids, which assumes a single Redis node; a clustered deployment needs hash tags. Every Redis failure surfaces as `RefreshStoreError::Unavailable`, which callers must treat as a denial (fail closed).

### Access tokens

`features/auth/access_token` issues and verifies short-lived, stateless JWT access tokens. Verification uses only the configured public keys. `AccessTokenService` holds no datastore client, so a valid token is accepted without any Redis or MySQL lookup and keeps working during a datastore outage.

**Contract.** Tokens are compact JWS signed with **EdDSA (Ed25519)**, the only accepted algorithm.

- Header: `{"alg": "EdDSA", "typ": "at+jwt", "kid": "<JWT_SIGNING_KEY_ID>"}`.
- Claims:
  - `iss` (`JWT_ISSUER`) and `aud` (`JWT_AUDIENCE`, a single string);
  - `sub`, the user's UUID;
  - `iat` and `exp`, in Unix seconds;
  - `jti`, a random UUID;
  - `role`, `STUDENT` or `ADMIN`.

`issue(user_id, role)` must be given values read from the database, never from the request.

**Using it in handlers.** Add `principal: AuthenticatedPrincipal` as a handler argument. It reads exactly one `Authorization: Bearer <token>` header (the scheme is case-insensitive) and never reads the body, query or cookies, so a client cannot supply its own user ID or role. The principal has no public constructor. Router tests can use `AuthenticatedPrincipal::for_tests`. Every failure returns the same `401` `unauthorized` body with `WWW-Authenticate: Bearer`, so clients cannot tell which check failed. The server logs only a static reason at debug level (`access token rejected`, `reason=expired`), never the token or its claims.

**Rejected tokens.** A token is rejected when it has:

- any `alg` other than `EdDSA`, including `none`, HS256 signed with a public key, and RS256;
- a `typ` other than `at+jwt`;
- a missing or unknown `kid`, or any `crit` header;
- an invalid signature;
- a wrong `iss` or `aud`, or claims of the wrong JSON type (for example `aud` as an array);
- `exp` at or before `now - JWT_CLOCK_SKEW_SECONDS`, or `iat` after `now + JWT_CLOCK_SKEW_SECONDS`;
- a lifetime (`exp - iat`) that is not positive or exceeds 3600 seconds;
- a missing claim, a non-UUID or nil `sub`, or an unknown `role`;
- a length over 4 KiB, or anything that is not three base64url segments.

Header URLs and embedded keys (`jku`, `x5u`, `jwk`) are never used. Refresh tokens are opaque strings rather than JWTs, so one can never pass as an access token. The refresh store is never consulted for access-token validation.

**Keys.** Generate a key pair with OpenSSL:

```bash
openssl genpkey -algorithm ed25519 -outform DER -out jwt-key.der
base64 -w0 jwt-key.der                                              # JWT_SIGNING_KEY
openssl pkey -inform DER -in jwt-key.der -pubout -outform DER | base64 -w0   # public key for JWT_VERIFICATION_KEYS
```

Store the private key in your secret manager, never in the repository. `.env.example` and `compose.yaml` ship a published dev-only key (`kid` `dev-only`). Startup fails if that key is configured as the signing key or a verification key when `APP_ENV` is not `development`. Startup validation errors name the variable but never echo its value. The integration script generates a fresh key for each run.

**Key rotation.** Every replica signs with one key and accepts that key plus `JWT_VERIFICATION_KEYS`:

1. Generate a new key pair. Add `<new-kid>:<new-public>` to `JWT_VERIFICATION_KEYS` on every replica and roll out, so all replicas accept the new key before any replica signs with it.
2. Set `JWT_SIGNING_KEY_ID`/`JWT_SIGNING_KEY` to the new key, replace the new entry in `JWT_VERIFICATION_KEYS` with `<old-kid>:<old-public>`, and roll out. Tokens issued before the switch keep verifying.
3. After `JWT_ACCESS_TOKEN_TTL_SECONDS + JWT_CLOCK_SKEW_SECONDS` (at most about an hour), remove the old key.

If a key is compromised, skip the overlap: remove the key everywhere at once. Every token it signed fails immediately, and clients must use their refresh token to get a new access token.

**Limitation: no revocation before expiry.** Access tokens are deliberately stateless. A token stays valid until `exp`, even if the user logs out, is disabled or deleted, or changes role. The role in a principal is a snapshot taken when the token was issued. Exposure is bounded by `JWT_ACCESS_TOKEN_TTL_SECONDS + JWT_CLOCK_SKEW_SECONDS` (15.5 minutes by default). Sensitive operations must also check the account's current status and role in MySQL (AUTH-04). Logout and revocation act on refresh sessions, which stops any further access tokens from being issued.

The Compose OpenSearch service disables its security plugin for local development only. Do not expose it to an untrusted network or copy that setting into a production deployment.

## Migrations

The migration crate owns the authoritative MySQL schema for users, local credentials, external identities, courses, applications, and application reviews. Migrations never run implicitly during API startup; run them as an explicit release step before deploying application instances that depend on the new schema.

All primary and foreign keys use application-generated UUIDv7 values stored as `BINARY(16)`. Application timestamps are UTC `DATETIME(6)` values. The database enforces account roles and states, the course lifecycle, per-owner course code uniqueness, application and review states, unique emails, one application per student and course, one final review per application, and audit-preserving foreign keys. The identity tables are described in [Identity schema](#identity-schema), and courses in [Course schema](#course-schema).

> **Schema rewrites.** Migrations `000001_create_users`, `000002_create_external_identities`, and `000003_create_courses` were rewritten in place, and `000006_create_local_credentials` was added. A local database that applied the earlier versions must be rebuilt with `cargo run -p migration -- fresh`. Do this only against a disposable development database, because it drops all data.

### Identity schema

| Table | Purpose | Key constraints |
| --- | --- | --- |
| `users` | One row per account, including anonymized tombstones | `uq_users_normalized_email` (byte-exact; NULLs allowed only for `DELETED`), role `STUDENT`/`ADMIN` (default `STUDENT`), lifecycle checks below |
| `local_credentials` | Optional password credential; PK `user_id`, so at most one per user | Argon2id PHC strings only (`$argon2id$…`, `password_algorithm = 'ARGON2ID'`); FK `ON DELETE CASCADE` |
| `external_identities` | Google / Microsoft OpenID Connect identities | `UNIQUE (provider, provider_subject)` so an identity links to at most one user; `UNIQUE (user_id, provider)` so a user has at most one per provider; `provider IN ('GOOGLE','MICROSOFT')`; `tenant_id` only for `MICROSOFT`; FK `ON DELETE CASCADE` |

A user with no `local_credentials` row is OAuth-only. `users` holds no secret material. Rust entities and value types live in `crates/api/src/features/auth/identity/`:

- `PasswordHash` accepts only Argon2id PHC strings. Its `Debug` output prints `[REDACTED]`, and it implements neither `Display` nor `Serialize`.
- No identity entity derives `Serialize`, so password hashes cannot reach responses or logs by accident.
- `NewStudentAccount` has no role field. It always inserts `STUDENT` / `UNVERIFIED`.
- `NewExternalIdentity` rejects a tenant for any provider other than Microsoft.

**Account lifecycle.** `UNVERIFIED → ACTIVE ⇄ DISABLED`, and any live state may move to `PENDING_DELETION → DELETED`. The database enforces:

- `ACTIVE` requires `email_verified_at`, and `UNVERIFIED` forbids it.
- `deletion_requested_at` is set exactly for `PENDING_DELETION` and `DELETED`.
- `deleted_at` is set exactly for `DELETED`.
- `email` and `normalized_email` are both set or both `NULL`, and may be `NULL` only for `DELETED`.
- `idx_users_deletion_queue (account_status, deletion_requested_at)` serves the cleanup job.

**Deletion and retention (AUTH-12).**

- `applications` and `application_reviews` reference `users` with `RESTRICT`, so a user with history is never hard-deleted.
- Deletion runs in one transaction. It sets `DELETED` and `deleted_at`, clears both email columns (which frees the address for a new signup), replaces `display_name`, and deletes the user's `local_credentials` and `external_identities` rows.
- Applications and reviews are retained and still point to the tombstone.
- An account with no history, such as a never-verified signup, may be hard-deleted. The `CASCADE` foreign keys then remove its credential and identity rows. These cascades are limited to auth-only tables that have no dependents.

**Roles.** No API path creates or elevates `ADMIN`. Administrators are provisioned out-of-band. Signup must build its insert from `NewStudentAccount` and reject unknown request fields.

**Federated login and linking.**

- A login is matched only by `(provider, provider_subject)`, where the subject is the ID token's immutable `sub` claim. For Microsoft, also record `tid` in `tenant_id`. Never use email, UPN, or other mutable claims as the identifier.
- An unknown identity is never merged into an existing account by email. If its email matches an existing user, the login is refused, and the user is told to sign in with their existing method and link the provider explicitly.
- Linking is allowed only from an authenticated session after recent reauthentication.
- A user cannot unlink their last remaining sign-in method.
- `users.id` and `provider_subject` are immutable. `ON UPDATE RESTRICT` protects referenced IDs, and the Rust layer exposes no update path. Triggers are not used, because creating them on servers with binary logging requires elevated privileges.

### Course schema

A course is a learning course owned by a user, not an application-only offering. The legacy `applications` and `application_reviews` tables still reference `courses.id`, but new course features must not build on that workflow.

| Column | Type | Rules |
| --- | --- | --- |
| `id` | `BINARY(16)` | Immutable application-generated UUIDv7 |
| `owner_user_id` | `BINARY(16)` | FK `fk_courses_owner` → `users.id`, `ON UPDATE RESTRICT ON DELETE RESTRICT` |
| `title` | `VARCHAR(200)` | Required; not blank after trimming |
| `code` | `VARCHAR(32)`, case/accent-insensitive | Optional (for example `CSC101`); not blank when present |
| `term` | `VARCHAR(64)`, case/accent-insensitive | Optional (for example `Fall 2026`); not blank when present |
| `term_key` | generated, `COALESCE(term, '')` | Internal uniqueness key; never written and not mapped by the entity |
| `description` | `TEXT` | Required, may be empty; at most 10 000 characters |
| `status` | `VARCHAR(16)` | `DRAFT` (default), `PUBLISHED`, or `ARCHIVED` |
| `published_at`, `archived_at` | `DATETIME(6)` | Lifecycle timestamps, below |
| `created_at`, `updated_at` | `DATETIME(6)` | Maintained by MySQL |

Indexes: `idx_courses_owner_status (owner_user_id, status, created_at)` serves "my courses" listings and also covers the owner FK, and `idx_courses_status_created_at (status, created_at)` serves catalogue listings by status.

**Lifecycle.** `DRAFT → PUBLISHED ⇄ ARCHIVED`. A draft may also be archived directly. A published course never returns to `DRAFT`, so it cannot disappear from its members. The database enforces:

- `PUBLISHED` requires `published_at`, and `DRAFT` forbids it. `published_at` records the first publication and is kept through archive and restore.
- `archived_at` is set exactly while the course is `ARCHIVED`, and it is cleared on restore.

**Uniqueness (`uq_courses_owner_code_term`).**

- One owner cannot have two courses with the same `code` in the same `term`.
- Comparison is case- and accent-insensitive, so `CSC101`/`csc101` and `Fall 2026`/`fall 2026` collide.
- A missing term is its own value: an owner can have one termless `MAT200`. The generated `term_key` exists because MySQL `UNIQUE` ignores `NULL`.
- Courses without a code are never constrained.
- Different owners may share a code and term, for example parallel sections.
- The rule also covers archived courses, so an archived offering keeps its code and term.

**Retention.** Publishing, archiving, and restoring are status changes only and never delete rows. A course owner is tombstoned like any other user and cannot be hard-deleted while owning courses. Future coursework and membership tables must reference `courses` with `ON DELETE RESTRICT`, so archiving keeps their history intact.

**Rust model** (`crates/api/src/features/courses/`):

- `entities::course` maps the table. It does not derive `Serialize`, and it relates to the owning `user`.
- `NewCourse` trims and bounds title, code, and term (counted in characters, with control characters rejected) and bounds the description. It always inserts `DRAFT` with a fresh UUIDv7, with no ID or status input.
- `transition_status` applies `CourseStatus::can_transition_to` and sets only `status`, `published_at`, and `archived_at`, consistent with the checks above.

**Future extensions.** Application deadlines, seat capacity, and enrollment policy are intentionally absent from the initial LMS. If they are needed, add them through a forward migration, either as nullable columns or a separate enrollment-policy table, and do not revive the legacy application workflow.

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

