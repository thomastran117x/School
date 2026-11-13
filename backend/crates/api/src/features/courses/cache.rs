//! Cache-aside helpers for course catalogue reads.
//!
//! MySQL is the source of truth; Redis only accelerates display reads. The cache is
//! therefore **fail-open**: any Redis error is logged and the read falls back to the
//! database loader supplied by the caller. Nothing here may be used to make
//! authorization or application-acceptance decisions: those paths must read the
//! database directly (see the "Course catalogue cache" section of the backend README).
//!
//! # Keys
//!
//! All keys live in [`KeyNamespace::Course`] (`app:v1:course:<id>`) and embed a payload
//! schema version (`v1`) so a payload format change can ship by bumping that segment.
//!
//! | Id                           | Purpose                                              |
//! | ---------------------------- | ---------------------------------------------------- |
//! | `detail.v1.<course_id>`      | One course, or a short-lived "not found" marker      |
//! | `list.v1.<generation>.<hash>`| One catalogue page; `hash` identifies the query      |
//! | `ver.v1.<course_id>`         | Per-course change counter (guards cache fills)       |
//! | `gen.v1`                     | Catalogue generation counter (bumped on any change)  |
//!
//! # Invalidation and stale-write protection
//!
//! A writer commits to MySQL first and then calls [`CourseCache::invalidate_course`],
//! which atomically bumps the course counter and the catalogue generation and deletes
//! the detail entry. Listing keys embed the generation, so every listing page is
//! invalidated in O(1) without scanning; the orphaned pages expire by TTL.
//!
//! A reader records the counter *before* reading the database and fills the cache with
//! a script that only writes if the counter is unchanged. A read that raced with a
//! deactivation therefore can never re-cache the pre-deactivation row.
//!
//! If invalidation itself fails (Redis down), staleness is bounded by the configured
//! TTLs (`COURSE_CACHE_*_TTL_SECONDS`).

use std::{future::Future, time::Duration};

use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    config::CourseCacheSettings,
    infrastructure::{Key, KeyNamespace, RedisClient, RedisError},
};

/// Counter keys outlive any in-flight read (database queries are bounded by seconds),
/// which is what makes the fill guard safe, yet remain bounded.
const COUNTER_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Value assumed for a counter that does not exist.
const ABSENT_COUNTER: &str = "0";

/// Writes `ARGV[2]` to `KEYS[2]` for `ARGV[3]` seconds only if the counter in `KEYS[1]`
/// still equals `ARGV[1]`. Returns 1 when written and 0 when the fill was stale.
const FILL_IF_UNCHANGED: &str = r"
local current = redis.call('GET', KEYS[1])
if not current then current = '0' end
if current ~= ARGV[1] then return 0 end
redis.call('SET', KEYS[2], ARGV[2], 'EX', ARGV[3])
return 1
";

/// Bumps the course counter (`KEYS[1]`) and catalogue generation (`KEYS[3]`), deletes the
/// detail entry (`KEYS[2]`), and refreshes counter expiry (`ARGV[1]` seconds).
const INVALIDATE_COURSE: &str = r"
redis.call('INCR', KEYS[1])
redis.call('EXPIRE', KEYS[1], ARGV[1])
redis.call('DEL', KEYS[2])
redis.call('INCR', KEYS[3])
redis.call('EXPIRE', KEYS[3], ARGV[1])
return 1
";

/// Bumps the catalogue generation (`KEYS[1]`), orphaning every cached listing page.
const INVALIDATE_CATALOGUE: &str = r"
redis.call('INCR', KEYS[1])
redis.call('EXPIRE', KEYS[1], ARGV[1])
return 1
";

#[derive(Debug, Error, PartialEq, Eq)]
/// Failures raised while invalidating cached course data.
pub(crate) enum CourseCacheError {
    /// The course identifier cannot be used in a Redis key.
    #[error("course identifier is not valid for caching")]
    InvalidIdentifier,
    /// Redis could not be reached, so cached entries may remain until their TTL expires.
    #[error("course cache invalidation failed")]
    InvalidationFailed,
}

#[derive(Clone)]
/// Fail-open, versioned Redis cache for course catalogue reads.
pub(crate) struct CourseCache {
    redis: RedisClient,
    settings: CourseCacheSettings,
}

impl CourseCache {
    /// Creates a cache over the shared Redis client with validated TTL settings.
    pub(crate) fn new(redis: RedisClient, settings: CourseCacheSettings) -> Self {
        Self { redis, settings }
    }

    /// Reads one course through the cache.
    ///
    /// `loader` queries the database and runs on a miss, on any Redis failure, or when
    /// `course_id` cannot be used as a key. A `None` result ("not found") is cached for
    /// the shorter negative TTL. Loader errors are returned and never cached.
    ///
    /// # Errors
    ///
    /// Returns only the loader's own error; Redis failures never surface here.
    pub(crate) async fn detail<T, E, F, Fut>(
        &self,
        course_id: &str,
        loader: F,
    ) -> Result<Option<T>, E>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<T>, E>>,
    {
        let (Some(token_key), Some(entry_key)) = (
            course_key(format!("ver.v1.{course_id}")),
            course_key(format!("detail.v1.{course_id}")),
        ) else {
            return loader().await;
        };
        let (found_ttl, missing_ttl) = (self.settings.detail_ttl, self.settings.negative_ttl);
        self.read_through(
            token_key,
            move |_| Some(entry_key),
            move |value: &Option<T>| {
                if value.is_some() {
                    found_ttl
                } else {
                    missing_ttl
                }
            },
            loader,
        )
        .await
    }

    /// Reads one catalogue listing page through the cache.
    ///
    /// `query` is any stable description of the filter and page (for example
    /// `"active=1&page=2&size=20"`); it is hashed, so it may contain any characters.
    ///
    /// # Errors
    ///
    /// Returns only the loader's own error; Redis failures never surface here.
    pub(crate) async fn list<T, E, F, Fut>(&self, query: &str, loader: F) -> Result<T, E>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let Some(generation_key) = course_key("gen.v1".to_owned()) else {
            return loader().await;
        };
        let digest = format!("{:x}", Sha256::digest(query.as_bytes()));
        let ttl = self.settings.list_ttl;
        self.read_through(
            generation_key,
            move |generation| course_key(format!("list.v1.{generation}.{}", &digest[..32])),
            move |_: &T| ttl,
            loader,
        )
        .await
    }

    /// Invalidates everything cached for one course and every catalogue listing.
    ///
    /// Call this **after** the database change commits (create, update, deactivate,
    /// reactivate). One retry is attempted before the failure is reported; callers
    /// should log the error and may rely on the TTLs as the staleness bound.
    ///
    /// # Errors
    ///
    /// Returns [`CourseCacheError::InvalidIdentifier`] for an unusable id, or
    /// [`CourseCacheError::InvalidationFailed`] when Redis stays unreachable.
    pub(crate) async fn invalidate_course(&self, course_id: &str) -> Result<(), CourseCacheError> {
        let (Some(token), Some(entry), Some(generation)) = (
            course_key(format!("ver.v1.{course_id}")),
            course_key(format!("detail.v1.{course_id}")),
            course_key("gen.v1".to_owned()),
        ) else {
            return Err(CourseCacheError::InvalidIdentifier);
        };
        let keys = vec![
            token.as_str().to_owned(),
            entry.as_str().to_owned(),
            generation.as_str().to_owned(),
        ];
        self.invalidate(INVALIDATE_COURSE, keys).await
    }

    /// Invalidates every cached catalogue listing, for example after a bulk change.
    ///
    /// # Errors
    ///
    /// Returns [`CourseCacheError::InvalidationFailed`] when Redis stays unreachable.
    pub(crate) async fn invalidate_catalogue(&self) -> Result<(), CourseCacheError> {
        let generation =
            course_key("gen.v1".to_owned()).ok_or(CourseCacheError::InvalidIdentifier)?;
        self.invalidate(INVALIDATE_CATALOGUE, vec![generation.as_str().to_owned()])
            .await
    }

    async fn invalidate(
        &self,
        script: &'static str,
        keys: Vec<String>,
    ) -> Result<(), CourseCacheError> {
        let args = vec![COUNTER_TTL.as_secs().to_string()];
        for attempt in 1..=2 {
            match self.invoke::<i64>(script, keys.clone(), args.clone()).await {
                Ok(_) => return Ok(()),
                Err(error) => {
                    tracing::warn!(%error, attempt, "course cache invalidation attempt failed");
                }
            }
        }
        tracing::error!(
            "course cache invalidation failed; stale entries persist until their TTL expires"
        );
        Err(CourseCacheError::InvalidationFailed)
    }

    /// Shared cache-aside flow: token, lookup, load, guarded fill.
    async fn read_through<V, E, F, Fut>(
        &self,
        token_key: Key,
        entry_key_for: impl FnOnce(&str) -> Option<Key>,
        ttl_of: impl FnOnce(&V) -> Duration,
        loader: F,
    ) -> Result<V, E>
    where
        V: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V, E>>,
    {
        // The counter must be read before the database so a concurrent invalidation
        // makes the later fill fail. Without it we cannot cache safely: just load.
        let Some(token) = self.read_counter(&token_key).await else {
            return loader().await;
        };
        let Some(entry_key) = entry_key_for(&token) else {
            return loader().await;
        };
        if let Some(hit) = self.lookup::<V>(&entry_key).await {
            return Ok(hit);
        }

        let value = loader().await?;
        self.fill(&token_key, &entry_key, &token, &value, ttl_of(&value))
            .await;
        Ok(value)
    }

    /// Reads a counter, treating a missing key as `0`; `None` when Redis fails.
    async fn read_counter(&self, key: &Key) -> Option<String> {
        match self.redis.get(key).await {
            Ok(value) => Some(value.unwrap_or_else(|| ABSENT_COUNTER.to_owned())),
            Err(error) => {
                tracing::warn!(%error, "course cache unavailable; reading from the database");
                None
            }
        }
    }

    /// Returns a decoded cache hit, or `None` for a miss, Redis error, or bad payload.
    async fn lookup<V: DeserializeOwned>(&self, key: &Key) -> Option<V> {
        match self.redis.get(key).await {
            Ok(Some(raw)) => match serde_json::from_str(&raw) {
                Ok(value) => Some(value),
                Err(_) => {
                    tracing::warn!("discarding undecodable course cache entry");
                    if let Err(error) = self.redis.delete(key).await {
                        tracing::warn!(%error, "could not delete undecodable course cache entry");
                    }
                    None
                }
            },
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(%error, "course cache lookup failed; reading from the database");
                None
            }
        }
    }

    /// Best-effort guarded write; failures only reduce the hit rate.
    async fn fill<V: Serialize>(
        &self,
        token_key: &Key,
        entry_key: &Key,
        token: &str,
        value: &V,
        ttl: Duration,
    ) {
        let Ok(payload) = serde_json::to_string(value) else {
            tracing::warn!("course value could not be serialized for caching");
            return;
        };
        let result = self
            .invoke::<i64>(
                FILL_IF_UNCHANGED,
                vec![token_key.as_str().to_owned(), entry_key.as_str().to_owned()],
                vec![token.to_owned(), payload, ttl.as_secs().max(1).to_string()],
            )
            .await;
        match result {
            Ok(1) => {}
            Ok(_) => tracing::debug!("skipped stale course cache fill after invalidation"),
            Err(error) => tracing::warn!(%error, "course cache fill failed"),
        }
    }

    async fn invoke<T>(
        &self,
        script: &'static str,
        keys: Vec<String>,
        args: Vec<String>,
    ) -> Result<T, RedisError>
    where
        T: redis::FromRedisValue + Send + 'static,
    {
        self.redis
            .run(move |mut connection| {
                Box::pin(async move {
                    let script = redis::Script::new(script);
                    let mut invocation = script.prepare_invoke();
                    for key in &keys {
                        invocation.key(key);
                    }
                    for arg in &args {
                        invocation.arg(arg);
                    }
                    invocation.invoke_async::<T>(&mut connection).await
                })
            })
            .await
    }
}

/// Builds a key in the course namespace, or `None` when the id is unusable.
fn course_key(id: String) -> Option<Key> {
    Key::new(KeyNamespace::Course, &id).ok()
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use serde::Deserialize;

    use super::*;
    use crate::config::{RedisSettings, RedisTlsMode};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Probe {
        name: String,
        is_active: bool,
    }

    fn probe(name: &str, is_active: bool) -> Probe {
        Probe {
            name: name.to_owned(),
            is_active,
        }
    }

    async fn cache(url: &str) -> CourseCache {
        let settings = RedisSettings {
            url: url.to_owned(),
            connect_timeout: Duration::from_millis(200),
            command_timeout: Duration::from_millis(500),
            tls_mode: RedisTlsMode::Disabled,
            tls_ca_path: None,
        };
        let redis = RedisClient::initialize(&settings)
            .await
            .expect("lazy client builds without a server");
        CourseCache::new(redis, CourseCacheSettings::default())
    }

    fn counting_loader(
        calls: &Arc<AtomicUsize>,
        value: Probe,
    ) -> impl FnOnce() -> std::future::Ready<Result<Option<Probe>, &'static str>> {
        let calls = Arc::clone(calls);
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(Some(value)))
        }
    }

    #[test]
    fn keys_are_namespaced_and_versioned() {
        assert_eq!(
            course_key("detail.v1.abc-123".to_owned()).unwrap().as_str(),
            "app:v1:course:detail.v1.abc-123"
        );
        assert_eq!(
            course_key("gen.v1".to_owned()).unwrap().as_str(),
            "app:v1:course:gen.v1"
        );
        assert!(course_key("detail.v1.a:b".to_owned()).is_none());
        assert!(course_key(format!("detail.v1.{}", "x".repeat(200))).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn redis_outage_falls_back_to_the_loader_without_error() {
        let cache = cache("redis://127.0.0.1:1").await;
        let calls = Arc::new(AtomicUsize::new(0));

        let detail = cache
            .detail("course-1", counting_loader(&calls, probe("Maths", true)))
            .await;
        assert_eq!(detail, Ok(Some(probe("Maths", true))));

        let list = cache
            .list("active=1", || async {
                Ok::<_, &'static str>(vec![probe("Maths", true)])
            })
            .await;
        assert_eq!(list, Ok(vec![probe("Maths", true)]));

        // Nothing could be cached, so every read reaches the database.
        let _ = cache
            .detail("course-1", counting_loader(&calls, probe("Maths", true)))
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn loader_errors_propagate_and_unusable_ids_bypass_the_cache() {
        let cache = cache("redis://127.0.0.1:1").await;

        let failed: Result<Option<Probe>, &str> =
            cache.detail("course-1", || async { Err("db down") }).await;
        assert_eq!(failed, Err("db down"));

        let calls = Arc::new(AtomicUsize::new(0));
        let bypassed = cache
            .detail("bad:id", counting_loader(&calls, probe("Maths", true)))
            .await;
        assert_eq!(bypassed, Ok(Some(probe("Maths", true))));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn invalidation_reports_failure_without_leaking_details() {
        let cache = cache("redis://127.0.0.1:1").await;

        let error = cache.invalidate_course("course-1").await.unwrap_err();
        assert_eq!(error, CourseCacheError::InvalidationFailed);
        assert!(!error.to_string().contains("redis://"));
        assert_eq!(
            cache.invalidate_course("bad:id").await,
            Err(CourseCacheError::InvalidIdentifier)
        );
        assert_eq!(
            cache.invalidate_catalogue().await,
            Err(CourseCacheError::InvalidationFailed)
        );
    }

    /// Tests that need a real Redis; run by `scripts/integration-test.sh`.
    mod integration {
        use super::*;

        async fn live_cache() -> CourseCache {
            let url = std::env::var("REDIS_URL").expect("REDIS_URL is set by the harness");
            cache(&url).await
        }

        fn unique_id() -> String {
            format!("it-{}", uuid::Uuid::new_v4().simple())
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test Redis service"]
        async fn miss_loads_then_hit_skips_the_database() {
            let cache = live_cache().await;
            let id = unique_id();
            let calls = Arc::new(AtomicUsize::new(0));

            for _ in 0..3 {
                let value = cache
                    .detail(&id, counting_loader(&calls, probe("Maths", true)))
                    .await;
                assert_eq!(value, Ok(Some(probe("Maths", true))));
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test Redis service"]
        async fn not_found_is_cached_briefly_and_never_masks_a_later_create() {
            let cache = live_cache().await;
            let id = unique_id();

            let missing: Result<Option<Probe>, &str> =
                cache.detail(&id, || async { Ok(None) }).await;
            assert_eq!(missing, Ok(None));

            cache.invalidate_course(&id).await.unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let created = cache
                .detail(&id, counting_loader(&calls, probe("New", true)))
                .await;
            assert_eq!(created, Ok(Some(probe("New", true))));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test Redis service"]
        async fn deactivation_invalidates_detail_and_listings() {
            let cache = live_cache().await;
            let id = unique_id();
            let query = format!("active=1&nonce={id}");

            let active = cache
                .detail(&id, || async { Ok::<_, &str>(Some(probe("Maths", true))) })
                .await;
            assert_eq!(active, Ok(Some(probe("Maths", true))));
            let listed = cache
                .list(&query, || async {
                    Ok::<_, &str>(vec![probe("Maths", true)])
                })
                .await;
            assert_eq!(listed, Ok(vec![probe("Maths", true)]));

            // The course is deactivated in the database, then the cache is invalidated.
            cache.invalidate_course(&id).await.unwrap();

            let detail = cache
                .detail(&id, || async { Ok::<_, &str>(Some(probe("Maths", false))) })
                .await;
            assert_eq!(detail, Ok(Some(probe("Maths", false))));
            let listed = cache
                .list(&query, || async { Ok::<_, &str>(Vec::<Probe>::new()) })
                .await;
            assert_eq!(listed, Ok(Vec::new()));
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test Redis service"]
        async fn a_read_that_raced_with_deactivation_cannot_cache_stale_data() {
            let cache = live_cache().await;
            let id = unique_id();

            // The slow reader loaded the active row, but the course is deactivated and
            // invalidated before the reader gets to fill the cache.
            let racing = cache.clone();
            let racing_id = id.clone();
            let stale = cache
                .detail(&id, move || async move {
                    racing.invalidate_course(&racing_id).await.unwrap();
                    Ok::<_, &str>(Some(probe("Maths", true)))
                })
                .await;
            assert_eq!(stale, Ok(Some(probe("Maths", true))));

            // The stale value was not stored: the next read goes back to the database.
            let calls = Arc::new(AtomicUsize::new(0));
            let fresh = cache
                .detail(&id, counting_loader(&calls, probe("Maths", false)))
                .await;
            assert_eq!(fresh, Ok(Some(probe("Maths", false))));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test Redis service"]
        async fn undecodable_entries_are_treated_as_a_miss() {
            let cache = live_cache().await;
            let id = unique_id();
            let key = course_key(format!("detail.v1.{id}")).unwrap();
            cache
                .redis
                .set_with_ttl(&key, "{not json", Duration::from_secs(30))
                .await
                .unwrap();

            let calls = Arc::new(AtomicUsize::new(0));
            let value = cache
                .detail(&id, counting_loader(&calls, probe("Maths", true)))
                .await;
            assert_eq!(value, Ok(Some(probe("Maths", true))));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test Redis service"]
        async fn entries_expire_after_their_ttl() {
            let mut cache = live_cache().await;
            cache.settings.detail_ttl = Duration::from_secs(1);
            let id = unique_id();
            let calls = Arc::new(AtomicUsize::new(0));

            let _ = cache
                .detail(&id, counting_loader(&calls, probe("Maths", true)))
                .await;
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let _ = cache
                .detail(&id, counting_loader(&calls, probe("Maths", true)))
                .await;
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
    }
}
