//! The shared limiter: Redis token buckets with explicit failure behaviour.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

use super::{
    client_ip::ClientIpResolver,
    fallback::LocalFallback,
    policy::{FailureMode, Policies, Policy},
    script::TOKEN_BUCKET,
};
use crate::{
    config::RateLimitSettings,
    infrastructure::{Cache, Key, KeyNamespace, RedisClient, RedisError},
    metrics::Metrics,
};

/// Retry guidance returned when a fail-closed policy cannot reach Redis.
const UNAVAILABLE_RETRY_AFTER: Duration = Duration::from_secs(5);
/// Minimum spacing of Redis-failure warnings, so an outage cannot flood the logs.
const WARN_INTERVAL_SECS: u64 = 10;

/// Outcome of one limiter check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The request may proceed.
    Allowed,
    /// The subject exceeded its limit; respond with 429.
    Limited {
        /// Time until a token is available.
        retry_after: Duration,
    },
    /// A fail-closed policy could not be evaluated; respond with 503.
    Unavailable {
        /// Suggested delay before retrying.
        retry_after: Duration,
    },
}

/// Cloneable handle to the distributed rate limiter.
#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Inner>,
}

struct Inner {
    cache: Cache,
    fallback: LocalFallback,
    metrics: Metrics,
    resolver: ClientIpResolver,
    policies: Policies,
    enabled: bool,
    started: Instant,
    last_warning_secs: AtomicU64,
}

impl RateLimiter {
    /// Builds the limiter. Nothing connects to Redis until the first check.
    pub fn new(redis: RedisClient, metrics: Metrics, settings: &RateLimitSettings) -> Self {
        Self {
            inner: Arc::new(Inner {
                cache: Cache::new(redis),
                fallback: LocalFallback::new(settings.fallback_max_entries),
                metrics,
                resolver: ClientIpResolver::new(settings.trusted_proxies.clone()),
                policies: Policies::from_settings(settings),
                enabled: settings.enabled,
                started: Instant::now(),
                last_warning_secs: AtomicU64::new(0),
            }),
        }
    }

    /// The configured policies.
    pub fn policies(&self) -> &Policies {
        &self.inner.policies
    }

    /// The trusted client-IP resolver.
    pub fn resolver(&self) -> &ClientIpResolver {
        &self.inner.resolver
    }

    /// Whether limiting is enabled.
    pub fn enabled(&self) -> bool {
        self.inner.enabled
    }

    /// Checks a per-account policy. The account (for example an email address) is
    /// trimmed and lower-cased so case variants share one bucket.
    pub async fn check_account(&self, policy: &Policy, account: &str) -> Decision {
        self.check(policy, &account.trim().to_lowercase()).await
    }

    /// Consumes one token for `subject` under `policy`.
    ///
    /// The subject is hashed with the policy name, so raw addresses and account names
    /// never reach Redis keys, logs or metrics.
    pub async fn check(&self, policy: &Policy, subject: &str) -> Decision {
        let inner = &self.inner;
        if !inner.enabled {
            return Decision::Allowed;
        }
        let id = bucket_id(policy, subject);
        let Ok(key) = Key::new(KeyNamespace::RateLimit, &id) else {
            return self.on_redis_failure(policy, &id, "failed");
        };
        let args = [
            policy.capacity.to_string(),
            policy.refill_per_minute.to_string(),
        ];

        let started = Instant::now();
        let result = inner
            .cache
            .eval::<(i64, i64, i64)>(TOKEN_BUCKET, &[&key], &args)
            .await;
        let elapsed = started.elapsed().as_secs_f64();

        match result {
            Ok((allowed, _remaining, retry_ms)) => {
                inner.metrics.record_limiter_redis(elapsed, None);
                if allowed == 1 {
                    inner.metrics.record_rate_limit(policy.name, "allowed");
                    Decision::Allowed
                } else {
                    inner.metrics.record_rate_limit(policy.name, "limited");
                    Decision::Limited {
                        retry_after: Duration::from_millis(
                            u64::try_from(retry_ms).unwrap_or(1).max(1),
                        ),
                    }
                }
            }
            Err(error) => {
                inner
                    .metrics
                    .record_limiter_redis(elapsed, Some(error_kind(&error)));
                self.on_redis_failure(policy, &id, error_kind(&error))
            }
        }
    }

    /// Applies the policy's failure mode after Redis could not answer.
    fn on_redis_failure(&self, policy: &Policy, id: &str, kind: &'static str) -> Decision {
        let inner = &self.inner;
        self.warn_rate_limited(policy, kind);
        match policy.failure {
            FailureMode::Closed => {
                inner.metrics.record_rate_limit(policy.name, "fail_closed");
                Decision::Unavailable {
                    retry_after: UNAVAILABLE_RETRY_AFTER,
                }
            }
            FailureMode::Open => match inner.fallback.check(id, policy) {
                Ok(()) => {
                    inner
                        .metrics
                        .record_rate_limit(policy.name, "fallback_allowed");
                    Decision::Allowed
                }
                Err(retry_after) => {
                    inner
                        .metrics
                        .record_rate_limit(policy.name, "fallback_limited");
                    Decision::Limited { retry_after }
                }
            },
        }
    }

    /// Logs at most one Redis-failure warning per [`WARN_INTERVAL_SECS`].
    fn warn_rate_limited(&self, policy: &Policy, kind: &'static str) {
        let now = self.inner.started.elapsed().as_secs() + WARN_INTERVAL_SECS;
        let last = self.inner.last_warning_secs.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= WARN_INTERVAL_SECS
            && self
                .inner
                .last_warning_secs
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            tracing::warn!(
                policy = policy.name,
                error = kind,
                fail_closed = policy.failure == FailureMode::Closed,
                "rate limiter could not reach Redis"
            );
        }
    }
}

/// Maps a Redis error to a fixed metric label.
fn error_kind(error: &RedisError) -> &'static str {
    match error {
        RedisError::Unavailable => "unavailable",
        RedisError::TimedOut => "timeout",
        RedisError::InvalidTtl | RedisError::OperationFailed => "failed",
    }
}

/// Derives the opaque, key-safe bucket id for a policy and subject.
fn bucket_id(policy: &Policy, subject: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(policy.name.as_bytes());
    hasher.update([0]);
    hasher.update(subject.as_bytes());
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        config::{RedisSettings, RedisTlsMode},
        middleware::rate_limit::policy::FailureMode,
    };

    /// A limiter whose Redis is guaranteed unreachable.
    pub(crate) async fn offline_limiter(settings: &RateLimitSettings) -> (RateLimiter, Metrics) {
        let redis = RedisClient::initialize(&RedisSettings {
            url: "redis://127.0.0.1:1".to_owned(),
            connect_timeout: Duration::from_millis(100),
            command_timeout: Duration::from_millis(200),
            tls_mode: RedisTlsMode::Disabled,
            tls_ca_path: None,
        })
        .await
        .unwrap();
        let metrics = Metrics::new().unwrap();
        (RateLimiter::new(redis, metrics.clone(), settings), metrics)
    }

    fn policy(name: &'static str, capacity: u32, failure: FailureMode) -> Policy {
        Policy {
            name,
            capacity,
            refill_per_minute: 1,
            failure,
        }
    }

    #[test]
    fn bucket_ids_are_key_safe_and_separate_policies_and_subjects() {
        let a = policy("a", 1, FailureMode::Open);
        let b = policy("b", 1, FailureMode::Open);
        let id = bucket_id(&a, "2001:db8::1");
        assert_eq!(id.len(), 64);
        assert!(Key::new(KeyNamespace::RateLimit, &id).is_ok());
        assert!(!id.contains("db8"));
        assert_ne!(id, bucket_id(&b, "2001:db8::1"));
        assert_ne!(id, bucket_id(&a, "2001:db8::2"));
    }

    #[tokio::test]
    async fn fail_closed_policies_never_allow_when_redis_is_down() {
        let (limiter, metrics) = offline_limiter(&RateLimitSettings::default()).await;
        let auth = policy("auth_ip", 1000, FailureMode::Closed);
        for _ in 0..5 {
            assert!(matches!(
                limiter.check(&auth, "203.0.113.1").await,
                Decision::Unavailable { retry_after } if retry_after >= Duration::from_secs(1)
            ));
        }
        let rendered = metrics.render();
        assert!(
            rendered.contains(
                r#"rate_limit_decisions_total{policy="auth_ip",outcome="fail_closed"} 5"#
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("rate_limiter_redis_errors_total"),
            "{rendered}"
        );
        assert!(
            rendered.contains("rate_limiter_redis_duration_seconds"),
            "{rendered}"
        );
    }

    #[tokio::test]
    async fn fail_open_policies_fall_back_to_a_bounded_local_limit() {
        let (limiter, metrics) = offline_limiter(&RateLimitSettings::default()).await;
        let general = policy("general_ip", 2, FailureMode::Open);
        assert_eq!(limiter.check(&general, "x").await, Decision::Allowed);
        assert_eq!(limiter.check(&general, "x").await, Decision::Allowed);
        assert!(matches!(
            limiter.check(&general, "x").await,
            Decision::Limited { .. }
        ));
        assert_eq!(limiter.check(&general, "y").await, Decision::Allowed);
        let rendered = metrics.render();
        assert!(
            rendered.contains(r#"outcome="fallback_allowed""#),
            "{rendered}"
        );
        assert!(
            rendered.contains(r#"outcome="fallback_limited""#),
            "{rendered}"
        );
    }

    #[tokio::test]
    async fn a_disabled_limiter_allows_everything_without_touching_redis() {
        let settings = RateLimitSettings {
            enabled: false,
            ..RateLimitSettings::default()
        };
        let (limiter, _) = offline_limiter(&settings).await;
        let auth = policy("auth_ip", 1, FailureMode::Closed);
        assert_eq!(limiter.check(&auth, "x").await, Decision::Allowed);
    }

    /// Tests against a real Redis, run by `scripts/integration-test.sh`.
    mod integration {
        use super::*;

        async fn live_limiter() -> RateLimiter {
            let url = std::env::var("REDIS_URL").expect("REDIS_URL is set by the harness");
            let redis = RedisClient::initialize(&RedisSettings {
                tls_mode: RedisTlsMode::Disabled,
                url,
                connect_timeout: Duration::from_secs(2),
                command_timeout: Duration::from_secs(2),
                tls_ca_path: None,
            })
            .await
            .unwrap();
            RateLimiter::new(
                redis,
                Metrics::new().unwrap(),
                &RateLimitSettings::default(),
            )
        }

        fn subject() -> String {
            uuid::Uuid::new_v4().simple().to_string()
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn bursts_are_allowed_up_to_capacity_then_limited() {
            let limiter = live_limiter().await;
            let policy = policy("it_burst", 5, FailureMode::Closed);
            let subject = subject();
            for _ in 0..5 {
                assert_eq!(limiter.check(&policy, &subject).await, Decision::Allowed);
            }
            let Decision::Limited { retry_after } = limiter.check(&policy, &subject).await else {
                panic!("sixth request must be limited");
            };
            // One token per minute: guidance is about a minute.
            assert!(
                retry_after > Duration::from_secs(50) && retry_after <= Duration::from_secs(60)
            );
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn tokens_refill_over_time() {
            let limiter = live_limiter().await;
            let policy = Policy {
                refill_per_minute: 120, // one token every 500 ms
                ..policy("it_refill", 1, FailureMode::Closed)
            };
            let subject = subject();
            assert_eq!(limiter.check(&policy, &subject).await, Decision::Allowed);
            assert!(matches!(
                limiter.check(&policy, &subject).await,
                Decision::Limited { .. }
            ));
            tokio::time::sleep(Duration::from_millis(700)).await;
            assert_eq!(limiter.check(&policy, &subject).await, Decision::Allowed);
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn idle_buckets_expire_from_redis() {
            let limiter = live_limiter().await;
            let policy = Policy {
                refill_per_minute: 6000, // refills fully in well under two seconds
                ..policy("it_expiry", 2, FailureMode::Closed)
            };
            let subject = subject();
            assert_eq!(limiter.check(&policy, &subject).await, Decision::Allowed);
            let key = Key::new(KeyNamespace::RateLimit, &bucket_id(&policy, &subject)).unwrap();
            let cache = &limiter.inner.cache;
            assert!(cache.exists(&key).await.unwrap());
            tokio::time::sleep(Duration::from_millis(2500)).await;
            assert!(!cache.exists(&key).await.unwrap());
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn concurrent_callers_never_exceed_capacity() {
            let limiter = live_limiter().await;
            let policy = policy("it_atomic", 10, FailureMode::Closed);
            let subject = subject();
            let mut tasks = Vec::new();
            for _ in 0..64 {
                let limiter = limiter.clone();
                let subject = subject.clone();
                tasks.push(tokio::spawn(async move {
                    limiter.check(&policy, &subject).await
                }));
            }
            let mut allowed = 0;
            for task in tasks {
                if task.await.unwrap() == Decision::Allowed {
                    allowed += 1;
                }
            }
            assert_eq!(allowed, 10);
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn separate_replicas_share_one_bucket() {
            let (replica_a, replica_b) = (live_limiter().await, live_limiter().await);
            let policy = policy("it_replicas", 4, FailureMode::Closed);
            let subject = subject();
            for _ in 0..2 {
                assert_eq!(replica_a.check(&policy, &subject).await, Decision::Allowed);
                assert_eq!(replica_b.check(&policy, &subject).await, Decision::Allowed);
            }
            assert!(matches!(
                replica_a.check(&policy, &subject).await,
                Decision::Limited { .. }
            ));
            assert!(matches!(
                replica_b.check(&policy, &subject).await,
                Decision::Limited { .. }
            ));
        }
    }
}
