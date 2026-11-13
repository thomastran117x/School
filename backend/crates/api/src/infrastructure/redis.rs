//! Shared, bounded access to the Redis connection manager.
//!
//! One multiplexed, auto-reconnecting [`ConnectionManager`] is created at startup and
//! cloned by handle. Every command runs under [`RedisClient::run`], which enforces the
//! command deadline and converts driver errors into sanitized [`RedisError`] values so
//! URLs, passwords, and stored values never reach logs or responses.

// Helpers are the shared contract for upcoming features and have no callers yet.
#![allow(dead_code)]

use std::{fmt, future::Future, pin::Pin, time::Duration};

use redis::{
    Client, ConnectionAddr, IntoConnectionInfo, TlsCertificates,
    aio::{ConnectionManager, ConnectionManagerConfig},
};
use thiserror::Error;

use super::redis_keys::{Key, MAX_TTL};
use crate::config::{RedisSettings, RedisTlsMode};

type RedisFuture<T> = Pin<Box<dyn Future<Output = redis::RedisResult<T>> + Send>>;

#[derive(Clone)]
/// Cloneable access to the application's single Redis connection manager.
pub struct RedisClient {
    manager: ConnectionManager,
    command_timeout: Duration,
}

impl RedisClient {
    /// Creates a lazy Redis connection manager from validated settings.
    ///
    /// URL and TLS configuration are validated without requiring Redis to be
    /// reachable; the first command establishes the connection.
    ///
    /// # Errors
    ///
    /// Returns [`RedisInitializationError`] when the URL, certificate authority, or
    /// TLS configuration is rejected. The error omits the URL and driver error.
    pub async fn initialize(settings: &RedisSettings) -> Result<Self, RedisInitializationError> {
        let mut info = settings
            .url
            .as_str()
            .into_connection_info()
            .map_err(|_| RedisInitializationError)?;

        let client = match settings.tls_mode {
            RedisTlsMode::Disabled => Client::open(info).map_err(|_| RedisInitializationError)?,
            RedisTlsMode::Required => {
                let ConnectionAddr::TcpTls { host, port, .. } = info.addr().clone() else {
                    return Err(RedisInitializationError);
                };
                info = info.set_addr(ConnectionAddr::TcpTls {
                    host,
                    port,
                    insecure: true,
                    tls_params: None,
                });
                Client::open(info).map_err(|_| RedisInitializationError)?
            }
            RedisTlsMode::Verify => {
                let root_cert = match settings.tls_ca_path.as_ref() {
                    Some(path) => Some(
                        tokio::fs::read(path)
                            .await
                            .map_err(|_| RedisInitializationError)?,
                    ),
                    None => None,
                };
                Client::build_with_tls(
                    info,
                    TlsCertificates {
                        client_tls: None,
                        root_cert,
                    },
                )
                .map_err(|_| RedisInitializationError)?
            }
        };

        let manager = ConnectionManager::new_lazy_with_config(
            client,
            ConnectionManagerConfig::new()
                .set_connection_timeout(Some(settings.connect_timeout))
                .set_response_timeout(Some(settings.command_timeout)),
        )
        .map_err(|_| RedisInitializationError)?;

        Ok(Self {
            manager,
            command_timeout: settings.command_timeout,
        })
    }

    /// Runs one Redis operation within the configured command deadline.
    ///
    /// The closure receives a clone of the shared connection manager.
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`RedisError`] for connection loss, timeout, or any other
    /// failed command.
    pub async fn run<T, F>(&self, operation: F) -> Result<T, RedisError>
    where
        F: FnOnce(ConnectionManager) -> RedisFuture<T>,
    {
        match tokio::time::timeout(self.command_timeout, operation(self.manager.clone())).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(RedisError::from_redis_error(&error)),
            Err(_) => Err(RedisError::TimedOut),
        }
    }

    /// Sends `PING` to verify that Redis is reachable.
    ///
    /// # Errors
    ///
    /// Returns the same sanitized errors as [`Self::run`].
    pub async fn ping(&self) -> Result<(), RedisError> {
        self.run(|mut connection| {
            Box::pin(async move {
                redis::cmd("PING")
                    .query_async::<String>(&mut connection)
                    .await
                    .map(|_| ())
            })
        })
        .await
    }

    /// Reads a string value, returning `None` when the key does not exist.
    ///
    /// # Errors
    ///
    /// Returns the same sanitized errors as [`Self::run`].
    pub async fn get(&self, key: &Key) -> Result<Option<String>, RedisError> {
        let key = key.as_str().to_owned();
        self.run(|mut connection| {
            Box::pin(async move {
                redis::cmd("GET")
                    .arg(key)
                    .query_async::<Option<String>>(&mut connection)
                    .await
            })
        })
        .await
    }

    /// Stores a string value that expires after `ttl`.
    ///
    /// A TTL is mandatory so no application key can be written without an expiry.
    ///
    /// # Errors
    ///
    /// Returns [`RedisError::InvalidTtl`] when `ttl` is shorter than one second or
    /// longer than [`MAX_TTL`], otherwise the same errors as [`Self::run`].
    pub async fn set_with_ttl(
        &self,
        key: &Key,
        value: &str,
        ttl: Duration,
    ) -> Result<(), RedisError> {
        if ttl < Duration::from_secs(1) || ttl > MAX_TTL {
            return Err(RedisError::InvalidTtl);
        }
        let key = key.as_str().to_owned();
        let value = value.to_owned();
        let seconds = ttl.as_secs();
        self.run(|mut connection| {
            Box::pin(async move {
                redis::cmd("SET")
                    .arg(key)
                    .arg(value)
                    .arg("EX")
                    .arg(seconds)
                    .query_async::<()>(&mut connection)
                    .await
            })
        })
        .await
    }

    /// Deletes a key, returning whether it existed.
    ///
    /// # Errors
    ///
    /// Returns the same sanitized errors as [`Self::run`].
    pub async fn delete(&self, key: &Key) -> Result<bool, RedisError> {
        let key = key.as_str().to_owned();
        self.run(|mut connection| {
            Box::pin(async move {
                redis::cmd("DEL")
                    .arg(key)
                    .query_async::<u64>(&mut connection)
                    .await
                    .map(|removed| removed > 0)
            })
        })
        .await
    }
}

#[derive(Debug, Error)]
#[error("Redis client configuration is invalid")]
/// Sanitized failure raised while constructing the lazy connection manager.
pub struct RedisInitializationError;

#[derive(Error, PartialEq, Eq)]
/// Sanitized failures returned by bounded Redis operations.
pub enum RedisError {
    /// Redis refused, dropped, or could not establish a connection.
    #[error("redis is unavailable")]
    Unavailable,
    /// The operation exceeded its configured command deadline.
    #[error("redis command timed out")]
    TimedOut,
    /// The requested TTL was outside the permitted range.
    #[error("redis ttl is outside the permitted range")]
    InvalidTtl,
    /// Redis rejected or could not complete the command.
    #[error("redis operation failed")]
    OperationFailed,
}

impl RedisError {
    /// Classifies a driver error without retaining its message.
    fn from_redis_error(error: &redis::RedisError) -> Self {
        if error.is_timeout() {
            Self::TimedOut
        } else if error.is_connection_refusal()
            || error.is_connection_dropped()
            || error.is_io_error()
        {
            Self::Unavailable
        } else {
            Self::OperationFailed
        }
    }
}

impl fmt::Debug for RedisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Debug intentionally mirrors Display so it cannot leak driver details.
        write!(formatter, "RedisError({self})")
    }
}

#[cfg(test)]
mod tests {
    use std::{env, time::Instant};

    use super::*;
    use crate::infrastructure::redis_keys::KeyNamespace;

    fn settings(url: &str, tls_mode: RedisTlsMode) -> RedisSettings {
        RedisSettings {
            url: url.to_owned(),
            connect_timeout: Duration::from_millis(200),
            command_timeout: Duration::from_millis(500),
            tls_mode,
            tls_ca_path: None,
        }
    }

    #[test]
    fn errors_have_sanitized_display_and_debug() {
        for error in [
            RedisError::Unavailable,
            RedisError::TimedOut,
            RedisError::InvalidTtl,
            RedisError::OperationFailed,
        ] {
            let text = format!("{error} {error:?}");
            assert!(!text.contains("secret"));
            assert!(!text.contains("redis://"));
        }
        assert_eq!(
            RedisInitializationError.to_string(),
            "Redis client configuration is invalid"
        );
    }

    #[test]
    fn classifies_driver_errors_without_retaining_messages() {
        let io = redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "redis://:secret@host",
        ));
        assert_eq!(RedisError::from_redis_error(&io), RedisError::Unavailable);

        let other = redis::RedisError::from((
            redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
            "secret",
        ));
        assert_eq!(
            RedisError::from_redis_error(&other),
            RedisError::OperationFailed
        );
    }

    #[tokio::test]
    async fn initializes_lazily_for_every_tls_mode() {
        RedisClient::initialize(&settings(
            "redis://:secret@127.0.0.1:1",
            RedisTlsMode::Disabled,
        ))
        .await
        .expect("plaintext client should initialize without a server");
        RedisClient::initialize(&settings(
            "rediss://:secret@127.0.0.1:1",
            RedisTlsMode::Required,
        ))
        .await
        .expect("required-TLS client should initialize without a server");
        RedisClient::initialize(&settings(
            "rediss://:secret@127.0.0.1:1",
            RedisTlsMode::Verify,
        ))
        .await
        .expect("verified-TLS client should initialize without a server");
    }

    #[tokio::test]
    async fn rejects_unreadable_ca_without_leaking_the_url() {
        let mut bad = settings("rediss://:secret@127.0.0.1:1", RedisTlsMode::Verify);
        bad.tls_ca_path = Some("does-not-exist.pem".into());
        let error = RedisClient::initialize(&bad)
            .await
            .err()
            .expect("missing CA should fail");
        assert!(!error.to_string().contains("secret"));
    }

    #[tokio::test]
    async fn unreachable_server_fails_fast_with_a_typed_error() {
        let client =
            RedisClient::initialize(&settings("redis://127.0.0.1:1", RedisTlsMode::Disabled))
                .await
                .unwrap();
        let started = Instant::now();
        let error = client.ping().await.expect_err("nothing listens on port 1");
        assert!(matches!(
            error,
            RedisError::Unavailable | RedisError::TimedOut
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn ttl_is_validated_before_any_io() {
        let client =
            RedisClient::initialize(&settings("redis://127.0.0.1:1", RedisTlsMode::Disabled))
                .await
                .unwrap();
        let key = Key::new(KeyNamespace::Cache, "k").unwrap();
        for ttl in [
            Duration::ZERO,
            Duration::from_millis(10),
            MAX_TTL + Duration::from_secs(1),
        ] {
            assert_eq!(
                client.set_with_ttl(&key, "v", ttl).await,
                Err(RedisError::InvalidTtl)
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires the integration-test Redis service"]
    async fn redis_round_trips_values_with_ttl() {
        let url = env::var("REDIS_URL").expect("integration REDIS_URL should be set");
        let client = RedisClient::initialize(&settings(&url, RedisTlsMode::Disabled))
            .await
            .expect("integration client should initialize");
        client.ping().await.expect("PING should succeed");

        let key = Key::new(KeyNamespace::Cache, "integration-test").unwrap();
        client
            .set_with_ttl(&key, "value", KeyNamespace::Cache.default_ttl())
            .await
            .unwrap();
        assert_eq!(client.get(&key).await.unwrap().as_deref(), Some("value"));
        assert!(client.delete(&key).await.unwrap());
        assert_eq!(client.get(&key).await.unwrap(), None);

        let slow = client
            .run(|mut connection| {
                Box::pin(async move {
                    redis::cmd("BLPOP")
                        .arg("app:v1:cache:integration-empty-list")
                        .arg(2)
                        .query_async::<Option<(String, String)>>(&mut connection)
                        .await
                })
            })
            .await;
        assert!(slow.is_err(), "slow command must be bounded: {slow:?}");
    }
}
