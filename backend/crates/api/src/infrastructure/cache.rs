//! General-purpose cache operations over the shared Redis client.
//!
//! [`Cache`] provides the commands most features need (raw values, JSON values,
//! counters, `SET NX`, atomic command groups and cache-aside loading) so feature code
//! does not hand-roll Lua for them. Genuinely feature-specific atomic logic (for
//! example compare-and-set or single-winner rotation) stays in the feature as a Lua
//! script and runs through [`Cache::eval`].
//!
//! Every key is a validated [`Key`], every write has a bounded TTL, and every command
//! runs under the [`RedisClient`] deadline, so errors are sanitized [`RedisError`]s.

// The general helpers are the shared contract for upcoming features; not all have
// callers yet.
#![allow(dead_code)]

use std::{future::Future, time::Duration};

use serde::{Serialize, de::DeserializeOwned};

use super::{
    redis::{RedisClient, RedisError, validate_ttl},
    redis_keys::Key,
};

#[derive(Clone)]
/// Cloneable cache operations backed by the shared Redis connection.
pub struct Cache {
    redis: RedisClient,
}

impl Cache {
    /// Creates a cache over an existing (lazily connected) Redis client.
    pub fn new(redis: RedisClient) -> Self {
        Self { redis }
    }

    /// Reads a string value, returning `None` when the key does not exist.
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`RedisError`] when the command fails.
    pub async fn get(&self, key: &Key) -> Result<Option<String>, RedisError> {
        self.redis.get(key).await
    }

    /// Stores a string value that expires after `ttl`, replacing any existing value.
    ///
    /// # Errors
    ///
    /// Returns [`RedisError::InvalidTtl`] for a TTL outside 1 second to `MAX_TTL`,
    /// otherwise a sanitized command error.
    pub async fn set(&self, key: &Key, value: &str, ttl: Duration) -> Result<(), RedisError> {
        self.redis.set_with_ttl(key, value, ttl).await
    }

    /// Stores a value only if the key is absent (`SET NX EX`), for locks and
    /// de-duplication. Returns whether the value was written.
    ///
    /// # Errors
    ///
    /// Same as [`Self::set`].
    pub async fn set_nx(&self, key: &Key, value: &str, ttl: Duration) -> Result<bool, RedisError> {
        let seconds = validate_ttl(ttl)?;
        let (key, value) = (key.as_str().to_owned(), value.to_owned());
        self.redis
            .run(move |mut connection| {
                Box::pin(async move {
                    redis::cmd("SET")
                        .arg(key)
                        .arg(value)
                        .arg("NX")
                        .arg("EX")
                        .arg(seconds)
                        .query_async::<Option<String>>(&mut connection)
                        .await
                        .map(|reply| reply.is_some())
                })
            })
            .await
    }

    /// Reads and removes a value in one step (`GETDEL`), for single-use tokens.
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`RedisError`] when the command fails.
    pub async fn get_del(&self, key: &Key) -> Result<Option<String>, RedisError> {
        let key = key.as_str().to_owned();
        self.redis
            .run(move |mut connection| {
                Box::pin(async move {
                    redis::cmd("GETDEL")
                        .arg(key)
                        .query_async::<Option<String>>(&mut connection)
                        .await
                })
            })
            .await
    }

    /// Deletes a key, returning whether it existed.
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`RedisError`] when the command fails.
    pub async fn delete(&self, key: &Key) -> Result<bool, RedisError> {
        self.redis.delete(key).await
    }

    /// Deletes several keys in one command, returning how many existed.
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`RedisError`] when the command fails.
    pub async fn delete_many(&self, keys: &[&Key]) -> Result<u64, RedisError> {
        if keys.is_empty() {
            return Ok(0);
        }
        let keys: Vec<String> = keys.iter().map(|key| key.as_str().to_owned()).collect();
        self.redis
            .run(move |mut connection| {
                Box::pin(async move {
                    redis::cmd("DEL")
                        .arg(keys)
                        .query_async::<u64>(&mut connection)
                        .await
                })
            })
            .await
    }

    /// Returns whether the key exists.
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`RedisError`] when the command fails.
    pub async fn exists(&self, key: &Key) -> Result<bool, RedisError> {
        let key = key.as_str().to_owned();
        self.redis
            .run(move |mut connection| {
                Box::pin(async move {
                    redis::cmd("EXISTS")
                        .arg(key)
                        .query_async::<u64>(&mut connection)
                        .await
                        .map(|count| count > 0)
                })
            })
            .await
    }

    /// Returns the remaining lifetime of a key, or `None` when it is absent or has no
    /// expiry.
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`RedisError`] when the command fails.
    pub async fn ttl(&self, key: &Key) -> Result<Option<Duration>, RedisError> {
        let key = key.as_str().to_owned();
        self.redis
            .run(move |mut connection| {
                Box::pin(async move {
                    redis::cmd("PTTL")
                        .arg(key)
                        .query_async::<i64>(&mut connection)
                        .await
                        .map(|millis| u64::try_from(millis).ok().map(Duration::from_millis))
                })
            })
            .await
    }

    /// Sets a new expiry on an existing key, returning whether the key existed.
    ///
    /// # Errors
    ///
    /// Returns [`RedisError::InvalidTtl`] for a TTL outside 1 second to `MAX_TTL`,
    /// otherwise a sanitized command error.
    pub async fn expire(&self, key: &Key, ttl: Duration) -> Result<bool, RedisError> {
        let seconds = validate_ttl(ttl)?;
        let key = key.as_str().to_owned();
        self.redis
            .run(move |mut connection| {
                Box::pin(async move {
                    redis::cmd("EXPIRE")
                        .arg(key)
                        .arg(seconds)
                        .query_async::<u64>(&mut connection)
                        .await
                        .map(|applied| applied > 0)
                })
            })
            .await
    }

    /// Increments a counter and refreshes its expiry atomically, returning the new
    /// value. A missing counter starts at `1`.
    ///
    /// # Errors
    ///
    /// Returns [`RedisError::InvalidTtl`] for a TTL outside 1 second to `MAX_TTL`,
    /// otherwise a sanitized command error.
    pub async fn incr(&self, key: &Key, ttl: Duration) -> Result<i64, RedisError> {
        let seconds = validate_ttl(ttl)?;
        let key = key.as_str().to_owned();
        self.redis
            .run(move |mut connection| {
                Box::pin(async move {
                    let (value,): (i64,) = redis::pipe()
                        .atomic()
                        .cmd("INCR")
                        .arg(&key)
                        .cmd("EXPIRE")
                        .arg(&key)
                        .arg(seconds)
                        .ignore()
                        .query_async(&mut connection)
                        .await?;
                    Ok(value)
                })
            })
            .await
    }

    /// Reads a JSON value.
    ///
    /// An entry that no longer decodes (for example after a payload format change) is
    /// deleted on a best-effort basis and reported as a miss.
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`RedisError`] when the read fails.
    pub async fn get_json<T: DeserializeOwned>(&self, key: &Key) -> Result<Option<T>, RedisError> {
        let Some(raw) = self.get(key).await? else {
            return Ok(None);
        };
        if let Ok(value) = serde_json::from_str(&raw) {
            Ok(Some(value))
        } else {
            tracing::warn!("discarding undecodable cache entry");
            if let Err(error) = self.delete(key).await {
                tracing::warn!(%error, "could not delete undecodable cache entry");
            }
            Ok(None)
        }
    }

    /// Stores a value as JSON that expires after `ttl`.
    ///
    /// Returns `Ok(false)` without touching Redis when the value cannot be serialized.
    ///
    /// # Errors
    ///
    /// Same as [`Self::set`].
    pub async fn set_json<T: Serialize>(
        &self,
        key: &Key,
        value: &T,
        ttl: Duration,
    ) -> Result<bool, RedisError> {
        validate_ttl(ttl)?;
        let Ok(payload) = serde_json::to_string(value) else {
            tracing::warn!("value could not be serialized for caching");
            return Ok(false);
        };
        self.set(key, &payload, ttl).await?;
        Ok(true)
    }

    /// Fail-open cache-aside read.
    ///
    /// Returns a cached value when present; otherwise runs `loader`, stores the result
    /// for `ttl_of(&value)`, and returns it. Any Redis failure is logged and the loader
    /// result is returned uncached. Loader errors propagate and are never cached.
    ///
    /// Do not use this for data that must be invalidated against concurrent writers
    /// without a guard; use a feature-specific guarded fill (see [`Self::eval`]).
    ///
    /// # Errors
    ///
    /// Returns only the loader's own error.
    pub async fn get_or_load<T, E, F, Fut>(
        &self,
        key: &Key,
        ttl_of: impl FnOnce(&T) -> Duration,
        loader: F,
    ) -> Result<T, E>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        match self.get_json::<T>(key).await {
            Ok(Some(hit)) => return Ok(hit),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "cache lookup failed; loading from the source");
            }
        }
        let value = loader().await?;
        if let Err(error) = self.set_json(key, &value, ttl_of(&value)).await {
            tracing::warn!(%error, "cache fill failed");
        }
        Ok(value)
    }

    /// Runs a group of commands as one `MULTI`/`EXEC` transaction.
    ///
    /// `build` appends commands to the pipeline. Use this for unconditional write
    /// groups; logic that depends on values read inside the group needs a Lua script.
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`RedisError`] when the transaction fails.
    pub async fn atomic(&self, build: impl FnOnce(&mut redis::Pipeline)) -> Result<(), RedisError> {
        let mut pipeline = redis::pipe();
        pipeline.atomic();
        build(&mut pipeline);
        self.redis
            .run(move |mut connection| {
                Box::pin(async move { pipeline.query_async::<()>(&mut connection).await })
            })
            .await
    }

    /// Runs a feature-owned Lua script with the given keys and arguments.
    ///
    /// This is the single shared script invoker. Keep scripts for logic that must be
    /// atomic *and* conditional (compare-and-set, single-winner rotation).
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`RedisError`] when the script fails.
    pub async fn eval<T>(
        &self,
        script: &'static str,
        keys: &[&Key],
        args: &[String],
    ) -> Result<T, RedisError>
    where
        T: redis::FromRedisValue + Send + 'static,
    {
        let keys: Vec<String> = keys.iter().map(|key| key.as_str().to_owned()).collect();
        let args = args.to_vec();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{RedisSettings, RedisTlsMode},
        infrastructure::KeyNamespace,
    };

    async fn offline() -> Cache {
        let settings = RedisSettings {
            url: "redis://127.0.0.1:1".to_owned(),
            connect_timeout: Duration::from_millis(100),
            command_timeout: Duration::from_millis(200),
            tls_mode: RedisTlsMode::Disabled,
            tls_ca_path: None,
        };
        Cache::new(RedisClient::initialize(&settings).await.unwrap())
    }

    fn key() -> Key {
        Key::new(KeyNamespace::Cache, "unit").unwrap()
    }

    #[tokio::test]
    async fn ttl_is_validated_before_any_io() {
        let cache = offline().await;
        let key = key();
        for bad in [Duration::ZERO, Duration::from_millis(999), Duration::MAX] {
            assert_eq!(
                cache.set_nx(&key, "v", bad).await,
                Err(RedisError::InvalidTtl)
            );
            assert_eq!(cache.expire(&key, bad).await, Err(RedisError::InvalidTtl));
            assert_eq!(cache.incr(&key, bad).await, Err(RedisError::InvalidTtl));
            assert_eq!(
                cache.set_json(&key, &1_u8, bad).await,
                Err(RedisError::InvalidTtl)
            );
        }
    }

    #[tokio::test]
    async fn delete_many_of_nothing_skips_redis() {
        assert_eq!(offline().await.delete_many(&[]).await, Ok(0));
    }

    #[tokio::test]
    async fn get_or_load_falls_back_to_the_loader_when_redis_is_down() {
        let cache = offline().await;
        let value: Result<u8, ()> = cache
            .get_or_load(&key(), |_| Duration::from_secs(5), || async { Ok(7) })
            .await;
        assert_eq!(value, Ok(7));
        let failed: Result<u8, &str> = cache
            .get_or_load(&key(), |_| Duration::from_secs(5), || async { Err("boom") })
            .await;
        assert_eq!(failed, Err("boom"));
    }

    #[tokio::test]
    async fn outages_surface_as_sanitized_errors() {
        let cache = offline().await;
        let error = cache
            .incr(&key(), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("127.0.0.1"));
        assert!(
            cache
                .atomic(|pipe| {
                    pipe.cmd("PING");
                })
                .await
                .is_err()
        );
        assert!(cache.eval::<i64>("return 1", &[], &[]).await.is_err());
    }

    mod integration {
        use super::*;

        async fn live() -> Cache {
            let url = std::env::var("REDIS_URL").expect("REDIS_URL must be set");
            let settings = RedisSettings {
                url,
                connect_timeout: Duration::from_secs(2),
                command_timeout: Duration::from_secs(2),
                tls_mode: RedisTlsMode::Disabled,
                tls_ca_path: None,
            };
            Cache::new(RedisClient::initialize(&settings).await.unwrap())
        }

        fn unique(prefix: &str) -> Key {
            let id = format!("{prefix}.{}", uuid::Uuid::new_v4().simple());
            Key::new(KeyNamespace::Cache, &id).unwrap()
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn raw_nx_counter_and_ttl_operations() {
            let cache = live().await;
            let ttl = Duration::from_secs(30);

            let lock = unique("lock");
            assert!(cache.set_nx(&lock, "a", ttl).await.unwrap());
            assert!(!cache.set_nx(&lock, "b", ttl).await.unwrap());
            assert!(cache.exists(&lock).await.unwrap());
            assert!(cache.ttl(&lock).await.unwrap().is_some());
            assert_eq!(cache.get_del(&lock).await.unwrap().as_deref(), Some("a"));
            assert!(!cache.exists(&lock).await.unwrap());
            assert_eq!(cache.ttl(&lock).await.unwrap(), None);
            assert!(!cache.expire(&lock, ttl).await.unwrap());

            let counter = unique("counter");
            assert_eq!(cache.incr(&counter, ttl).await.unwrap(), 1);
            assert_eq!(cache.incr(&counter, ttl).await.unwrap(), 2);
            assert!(cache.ttl(&counter).await.unwrap().is_some());
            assert_eq!(cache.delete_many(&[&counter, &lock]).await.unwrap(), 1);
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn json_round_trip_and_undecodable_cleanup() {
            let cache = live().await;
            let key = unique("json");
            let ttl = Duration::from_secs(30);

            assert_eq!(cache.get_json::<Vec<u8>>(&key).await.unwrap(), None);
            assert!(cache.set_json(&key, &vec![1_u8, 2], ttl).await.unwrap());
            assert_eq!(
                cache.get_json::<Vec<u8>>(&key).await.unwrap(),
                Some(vec![1, 2])
            );

            cache.set(&key, "{not json", ttl).await.unwrap();
            assert_eq!(cache.get_json::<Vec<u8>>(&key).await.unwrap(), None);
            assert!(!cache.exists(&key).await.unwrap());
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn get_or_load_caches_and_atomic_applies_every_command() {
            let cache = live().await;
            let key = unique("aside");
            let calls = std::sync::atomic::AtomicUsize::new(0);
            for _ in 0..2 {
                let value: Result<u8, ()> = cache
                    .get_or_load(
                        &key,
                        |_| Duration::from_secs(30),
                        || async {
                            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            Ok(9)
                        },
                    )
                    .await;
                assert_eq!(value, Ok(9));
            }
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

            let (a, b) = (unique("atomic-a"), unique("atomic-b"));
            cache
                .atomic(|pipe| {
                    pipe.cmd("SET").arg(a.as_str()).arg("1").arg("EX").arg(30);
                    pipe.cmd("SET").arg(b.as_str()).arg("2").arg("EX").arg(30);
                })
                .await
                .unwrap();
            assert_eq!(cache.get(&a).await.unwrap().as_deref(), Some("1"));
            assert_eq!(cache.get(&b).await.unwrap().as_deref(), Some("2"));
            assert_eq!(
                cache
                    .eval::<i64>("return #KEYS", &[&a, &b], &[])
                    .await
                    .unwrap(),
                2
            );
            cache.delete_many(&[&key, &a, &b]).await.unwrap();
        }
    }
}
