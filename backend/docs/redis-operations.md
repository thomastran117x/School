# Redis operations: refresh sessions and caches

This document defines how Redis must be run when it holds refresh sessions alongside disposable caches. It complements the key and TTL reference in the [backend README](../README.md#redis-keys-and-ttls).

**Summary of the rules**

1. Refresh-session data is auth-critical. Run it with persistence (AOF, `everysec`), replication in production, and `maxmemory-policy noeviction`.
2. Never rely on an eviction policy to protect session keys. Every key the application writes has a TTL, so `volatile-*` policies treat session keys as evictable.
3. Run auth sessions on a dedicated Redis instance, separate from disposable cache workloads. The current code reads one `REDIS_URL`; the split is a recommended follow-up (see [Isolation](#7-isolation-recommendation)).

## 1. Data classes

| Namespace | Class | If lost | Rebuild |
| --- | --- | --- | --- |
| `RefreshToken`, `RefreshFamily`, `UserSessions` | **Auth-critical** | Users are signed out and must log in again. Lost family keys also remove reuse detection for tokens already rotated. | None. MySQL holds no session state. |
| `Session`, `Lock` | Auth-critical when used | Same as above for sessions. Lost locks only allow brief duplicate work. | None |
| `Course`, `Cache` | Disposable | Slower reads until refilled. No correctness impact, because Redis is never consulted for authorization. | MySQL (cache-aside) |
| `RateLimit` | Advisory | Buckets reset, so clients briefly get a fresh burst. | Self-healing |

## 2. Persistence and replication for refresh sessions

| Setting | Required value | Why |
| --- | --- | --- |
| `appendonly` | `yes` | Sessions must survive restarts. |
| `appendfsync` | `everysec` | Bounds crash loss to about 1 second of writes. `always` is safer but slower; `no` is not acceptable. |
| `aof-use-rdb-preamble` | `yes` (Redis default) | Faster restart from the AOF. |
| Replication (production) | At least one replica, in another failure domain, with Sentinel or a managed failover | A lost primary must not mean lost sessions. |
| `min-replicas-to-write` / `min-replicas-max-lag` | Optional: `1` / `10` | Refuses writes when no replica is caught up. This trades availability for durability, and refresh fails closed during the refusal. |

**Accepted loss window (RPO).** Replication is asynchronous, so a crash or failover can lose roughly the last second of writes. In that window:

- A refresh that rotated a token may be undone, so the old token works once more.
- A newly issued successor token may be unknown, so that user logs in again.

Neither case lets an unauthenticated caller in. If this window is not acceptable, use `appendfsync always` and synchronous waits (`WAIT`) in a follow-up change.

**Clustering.** Redis Cluster is not supported. The refresh and course scripts derive keys inside Lua and need hash tags first. Use primary/replica with Sentinel, or a managed non-cluster offering.

## 3. Eviction policy

| Instance | `maxmemory-policy` | Behaviour at the limit |
| --- | --- | --- |
| Dedicated auth instance | `noeviction` | Writes fail with `OOM`. The refresh store reports `Unavailable` and callers deny the request (fail closed). Sessions are never dropped silently. |
| Dedicated cache instance | `allkeys-lru` (or `allkeys-lfu`) | Old cache entries are dropped. Reads fall back to MySQL. |
| Shared instance (local dev, small deployments) | `noeviction` | A full cache can fill memory and cause `OOM` on auth writes (login and refresh return 503). This is the reason to isolate. |

Do **not** use `volatile-lru` or `volatile-ttl` to protect sessions. The key helpers require a TTL on every write, so all keys, including sessions, are eligible for eviction.

An eviction on the auth instance is an incident. Alert on `evicted_keys > 0`.

## 4. Memory limits and key TTLs

**Limits.** Always set `maxmemory`. An unbounded Redis is killed by the OS or container OOM killer, which loses unflushed writes and ignores policy.

| Instance | `maxmemory` | Headroom rule |
| --- | --- | --- |
| Auth | Sized from active sessions (below), minimum 256 MB | Keep `maxmemory` at most 70% of container or host memory. AOF rewrite and replication buffers need the rest. |
| Cache | Sized for the hot working set, for example 256 MB to 1 GB | Same 70% rule. |
| Local Compose (shared) | `REDIS_MAXMEMORY`, default `256mb` | Development baseline only. |

**Sizing the auth instance.** A live session uses about three keys (token, family, user index entry) and roughly 1 KB in total. At 100,000 concurrent sessions that is about 100 MB. Round up, and re-measure with `MEMORY USAGE` and `INFO memory` under real traffic.

**Alerts.** Warn at 70% of `maxmemory`, page at 85%, and page on any `evicted_keys` or `rejected_connections` increase on the auth instance.

**TTLs.** Every key has a TTL between 1 second and 30 days (`MAX_TTL` in `infrastructure/redis_keys.rs`).

| Keys | TTL | Source |
| --- | --- | --- |
| `refresh_token` | 7 days | `REFRESH_TOKEN_TTL_SECONDS` |
| `refresh_family`, `user_sessions` | 30 days (absolute family lifetime) | `REFRESH_FAMILY_MAX_LIFETIME_SECONDS` |
| `course` detail / negative | 300 s / 30 s | `COURSE_CACHE_DETAIL_TTL_SECONDS`, `COURSE_CACHE_NEGATIVE_TTL_SECONDS` |
| `course` list | 60 s | `COURSE_CACHE_LIST_TTL_SECONDS` |
| `course` `ver.*`, `gen.v1` | 24 hours | fixed |
| `ratelimit` | Idle buckets expire (about 1 minute default) | rate-limit script |

Shortening token or family TTLs lowers auth memory use and bounds how long a stale session can live. Lengthening them raises both.

## 5. Backups

- **What to back up:** the auth instance only. Do not back up the cache instance.
- **How:** RDB snapshots (`save 900 1`, `save 300 10`) or a managed daily snapshot, with the AOF directory included in volume-level backups. Keep 7 days.
- **Store backups encrypted and access-controlled.** They contain token hashes and user session indexes.
- **A backup is not a session-rollback tool.** Restoring an old snapshot brings back sessions that were revoked, rotated or logged out after it was taken, and can undo "log out everywhere".
- **Restore procedure:** restore, then decide. If the snapshot predates a security incident or a mass revocation, delete all `app:v1:refresh_token:*`, `app:v1:refresh_family:*` and `app:v1:user_sessions:*` keys before reopening traffic. Users then log in again, which is always safe.
- **Test restores** at least once per quarter.
- **MySQL is separate.** Redis backups do not replace the MySQL backup policy.

## 6. Restart and session-loss behaviour

| Event | Auth sessions | API behaviour |
| --- | --- | --- |
| Graceful Redis restart, AOF on | Preserved, minus up to 1 second of writes | Brief `Unavailable` while Redis reloads. Refresh fails closed. The API stays up, readiness reports Redis down, and it recovers automatically. |
| Redis crash or host failure, AOF on | Preserved up to the last fsync | Same as above after restart. |
| Failover to replica | Lost for the replication lag (about 1 second or more) | Clients reconnect through the connection manager. Refresh fails closed during the switch. |
| Volume lost, or restart with no persistence | **All sessions lost** | Every user must log in again. Reuse detection restarts from empty. No MySQL data is lost. |
| `OOM` at `maxmemory` with `noeviction` | Preserved | Writes fail, so refresh and login fail closed with 503. Reads still work. Alert and raise capacity. |
| Eviction on a dedicated cache instance | Not affected | Cache misses fall back to MySQL. |
| `FLUSHALL` or `FLUSHDB` on the auth instance | **All sessions lost** | Same as a lost volume. Restrict these commands with ACLs. |
| Redis unreachable | Unchanged | Refresh and auth rate limits fail closed (503). General rate limiting uses a bounded in-process fallback. Course reads go to MySQL. |
| API restart | Unaffected (state is in Redis) | The API starts even if Redis is down. Redis is connected lazily. |

Refresh tokens are server-side state. Losing Redis never exposes credentials: tokens are stored only as SHA-256 hashes. The worst outcome is a forced re-login.

## 7. Isolation recommendation

Run two separate Redis deployments:

| Deployment | Contents | Settings |
| --- | --- | --- |
| `redis-auth` | `RefreshToken`, `RefreshFamily`, `UserSessions` (and `Session`/`Lock` if used) | AOF `everysec`, replica and failover, `noeviction`, private network, TLS, ACL limited to those key patterns, backups |
| `redis-cache` | `Course`, `Cache`, `RateLimit` | No persistence, `allkeys-lru`, bounded `maxmemory`, TLS, no backups |

Why: a cache workload can then be evicted, flushed, resized or restarted without touching sessions, and auth memory is not consumed by cache growth. Logical databases (`SELECT n`) on one instance are **not** isolation, because memory, eviction policy and persistence are per instance.

**Current code state.** The API reads a single `REDIS_URL`, so one instance serves everything. Until the split is built, run that instance with the auth profile above (`noeviction`, AOF, replica) and a conservative cache TTL, and size it for both workloads.

**Recommended follow-up.** Add an optional `REDIS_CACHE_URL` (defaulting to `REDIS_URL`), build a second `RedisClient` in `infrastructure/clients.rs`, and route the course cache and rate limiter through it via `AppState`. Keep the refresh store on `REDIS_URL`.

## 8. Operator checklist

Run against the auth instance (add `--tls` and credentials as needed):

```bash
redis-cli CONFIG GET maxmemory            # non-zero
redis-cli CONFIG GET maxmemory-policy     # noeviction
redis-cli CONFIG GET appendonly           # yes
redis-cli CONFIG GET appendfsync          # everysec
redis-cli INFO persistence                # aof_enabled:1, no aof_last_write_status:err
redis-cli INFO stats | grep evicted_keys  # must stay 0
redis-cli INFO replication                # connected_slaves >= 1 in production
```

In production also confirm `REDIS_TLS_MODE=verify`, a password or ACL user in `REDIS_URL`, no public exposure of the port, and that `FLUSHALL`, `FLUSHDB` and `CONFIG` are restricted for the application user.
