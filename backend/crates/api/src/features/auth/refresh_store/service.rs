//! Refresh-session store contract and its Redis implementation.

use std::time::Duration;

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};

use super::{
    models::{IssuedSession, RefreshStoreError, RotationOutcome},
    scripts,
};
use crate::{
    config::AuthSettings,
    infrastructure::{Key, KeyNamespace, RedisClient, RedisError},
};

/// Number of random bytes in a refresh token (256 bits).
const TOKEN_BYTES: usize = 32;
/// Length of a base64url-encoded refresh token without padding.
const TOKEN_LENGTH: usize = 43;

#[async_trait]
/// Storage contract for refresh sessions.
///
/// Every method fails closed: an error means the caller must deny the request.
pub trait RefreshSessionStore: Send + Sync {
    /// Starts a new session family for `user_id` and returns its first token.
    ///
    /// # Errors
    ///
    /// Returns [`RefreshStoreError::InvalidIdentifier`] for an unusable user id, or
    /// [`RefreshStoreError::Unavailable`] when Redis fails.
    async fn issue(&self, user_id: &str) -> Result<IssuedSession, RefreshStoreError>;

    /// Spends `presented_token` and returns its successor.
    ///
    /// Of any number of concurrent calls with the same token, at most one returns
    /// [`RotationOutcome::Rotated`]; the others observe a replay and revoke the family.
    ///
    /// # Errors
    ///
    /// Returns [`RefreshStoreError::Unavailable`] when Redis fails.
    async fn rotate(&self, presented_token: &str) -> Result<RotationOutcome, RefreshStoreError>;

    /// Revokes the session family that `presented_token` belongs to (logout).
    ///
    /// Unknown or expired tokens succeed silently so logout stays idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`RefreshStoreError::Unavailable`] when Redis fails.
    async fn revoke_session(&self, presented_token: &str) -> Result<(), RefreshStoreError>;

    /// Revokes every session family of `user_id` (logout everywhere).
    ///
    /// # Errors
    ///
    /// Returns [`RefreshStoreError::InvalidIdentifier`] for an unusable user id, or
    /// [`RefreshStoreError::Unavailable`] when Redis fails.
    async fn revoke_all(&self, user_id: &str) -> Result<(), RefreshStoreError>;
}

#[derive(Clone)]
/// [`RefreshSessionStore`] backed by the shared Redis client.
pub struct RedisRefreshSessionStore {
    redis: RedisClient,
    settings: AuthSettings,
}

impl RedisRefreshSessionStore {
    /// Creates a store that uses `redis` and the configured lifetimes.
    pub fn new(redis: RedisClient, settings: AuthSettings) -> Self {
        Self { redis, settings }
    }

    /// Runs a script and maps any Redis failure to a closed-fail error.
    async fn invoke<T>(
        &self,
        script: &'static str,
        keys: Vec<String>,
        args: Vec<String>,
    ) -> Result<T, RefreshStoreError>
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
            .map_err(unavailable)
    }
}

#[async_trait]
impl RefreshSessionStore for RedisRefreshSessionStore {
    async fn issue(&self, user_id: &str) -> Result<IssuedSession, RefreshStoreError> {
        let user_key = Key::new(KeyNamespace::UserSessions, user_id)
            .map_err(|_| RefreshStoreError::InvalidIdentifier)?;
        let family_id = uuid::Uuid::new_v4().simple().to_string();
        let token = generate_token();
        let token_key = token_key(&token)?;
        let family_key = Key::new(KeyNamespace::RefreshFamily, &family_id)
            .map_err(|_| RefreshStoreError::InvalidIdentifier)?;
        let token_ttl = self
            .settings
            .refresh_token_ttl
            .min(self.settings.refresh_family_max_lifetime);

        self.invoke::<i64>(
            scripts::CREATE,
            vec![
                token_key.as_str().to_owned(),
                family_key.as_str().to_owned(),
                user_key.as_str().to_owned(),
            ],
            vec![
                user_id.to_owned(),
                family_id.clone(),
                millis(token_ttl),
                millis(self.settings.refresh_family_max_lifetime),
            ],
        )
        .await?;

        Ok(IssuedSession {
            token,
            family_id,
            user_id: user_id.to_owned(),
            expires_in: token_ttl,
        })
    }

    async fn rotate(&self, presented_token: &str) -> Result<RotationOutcome, RefreshStoreError> {
        if !is_well_formed(presented_token) {
            return Ok(RotationOutcome::Invalid);
        }
        let old_key = token_key(presented_token)?;
        let successor = generate_token();
        let new_key = token_key(&successor)?;

        let reply: Vec<String> = self
            .invoke(
                scripts::ROTATE,
                vec![old_key.as_str().to_owned(), new_key.as_str().to_owned()],
                vec![family_prefix(), millis(self.settings.refresh_token_ttl)],
            )
            .await?;

        match reply.iter().map(String::as_str).collect::<Vec<_>>()[..] {
            ["invalid"] => Ok(RotationOutcome::Invalid),
            ["revoked"] => Ok(RotationOutcome::Revoked),
            ["replay"] => {
                tracing::warn!("refresh token replay detected; session family revoked");
                Ok(RotationOutcome::ReplayDetected)
            }
            ["rotated", user_id, family_id, ttl_ms] => {
                let ttl_ms = ttl_ms.parse::<u64>().map_err(|_| {
                    tracing::error!("refresh rotation script returned an invalid TTL");
                    RefreshStoreError::Unavailable
                })?;
                Ok(RotationOutcome::Rotated(IssuedSession {
                    token: successor,
                    family_id: family_id.to_owned(),
                    user_id: user_id.to_owned(),
                    expires_in: Duration::from_millis(ttl_ms),
                }))
            }
            _ => {
                tracing::error!("refresh rotation script returned an unexpected reply");
                Err(RefreshStoreError::Unavailable)
            }
        }
    }

    async fn revoke_session(&self, presented_token: &str) -> Result<(), RefreshStoreError> {
        if !is_well_formed(presented_token) {
            return Ok(());
        }
        let key = token_key(presented_token)?;
        self.invoke::<i64>(
            scripts::REVOKE_SESSION,
            vec![key.as_str().to_owned()],
            vec![family_prefix()],
        )
        .await
        .map(|_| ())
    }

    async fn revoke_all(&self, user_id: &str) -> Result<(), RefreshStoreError> {
        let user_key = Key::new(KeyNamespace::UserSessions, user_id)
            .map_err(|_| RefreshStoreError::InvalidIdentifier)?;
        self.invoke::<i64>(
            scripts::REVOKE_USER,
            vec![user_key.as_str().to_owned()],
            vec![family_prefix()],
        )
        .await
        .map(|_| ())
    }
}

/// Logs a sanitized Redis failure and converts it into the fail-closed error.
fn unavailable(error: RedisError) -> RefreshStoreError {
    tracing::error!(%error, "refresh session store operation failed");
    RefreshStoreError::Unavailable
}

/// Generates a 256-bit opaque token from the operating system's random source.
fn generate_token() -> String {
    let mut bytes = [0_u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Returns whether `token` has the shape produced by [`generate_token`].
fn is_well_formed(token: &str) -> bool {
    token.len() == TOKEN_LENGTH
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Returns the lowercase hex SHA-256 digest used as the stored token identifier.
fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Builds the Redis key for a token without ever embedding the token itself.
fn token_key(token: &str) -> Result<Key, RefreshStoreError> {
    Key::new(KeyNamespace::RefreshToken, &hash_token(token))
        .map_err(|_| RefreshStoreError::InvalidIdentifier)
}

/// Prefix scripts use to derive family keys from stored family ids.
fn family_prefix() -> String {
    Key::namespace_prefix(KeyNamespace::RefreshFamily)
}

/// Formats a duration as whole milliseconds for script arguments.
fn millis(duration: Duration) -> String {
    u64::try_from(duration.as_millis())
        .unwrap_or(u64::MAX)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RedisSettings, RedisTlsMode};

    fn auth_settings(token_ttl: Duration) -> AuthSettings {
        AuthSettings {
            refresh_token_ttl: token_ttl,
            refresh_family_max_lifetime: Duration::from_secs(60),
        }
    }

    fn redis_settings(url: &str) -> RedisSettings {
        RedisSettings {
            url: url.to_owned(),
            connect_timeout: Duration::from_millis(200),
            command_timeout: Duration::from_millis(500),
            tls_mode: RedisTlsMode::Disabled,
            tls_ca_path: None,
        }
    }

    async fn store(url: &str, token_ttl: Duration) -> RedisRefreshSessionStore {
        let redis = RedisClient::initialize(&redis_settings(url))
            .await
            .expect("redis client initializes lazily");
        RedisRefreshSessionStore::new(redis, auth_settings(token_ttl))
    }

    #[test]
    fn generated_tokens_are_unique_and_well_formed() {
        let first = generate_token();
        let second = generate_token();
        assert_ne!(first, second);
        assert!(is_well_formed(&first));
        assert!(!is_well_formed("short"));
        assert!(!is_well_formed(&"a:".repeat(21)));
    }

    #[test]
    fn token_keys_contain_the_hash_and_never_the_token() {
        let token = generate_token();
        let key = token_key(&token).unwrap();
        assert!(!key.as_str().contains(&token));
        assert!(key.as_str().ends_with(&hash_token(&token)));
        assert_eq!(hash_token(&token), hash_token(&token));
        assert_eq!(hash_token(&token).len(), 64);
    }

    #[test]
    fn issued_session_debug_redacts_the_token() {
        let session = IssuedSession {
            token: "super-secret".to_owned(),
            family_id: "f".to_owned(),
            user_id: "u".to_owned(),
            expires_in: Duration::from_secs(1),
        };
        assert!(!format!("{session:?}").contains("super-secret"));
    }

    #[tokio::test]
    async fn fails_closed_when_redis_is_unreachable() {
        let store = store("redis://127.0.0.1:1", Duration::from_secs(30)).await;
        let token = generate_token();

        assert_eq!(
            store.issue("user-1").await.unwrap_err(),
            RefreshStoreError::Unavailable
        );
        assert_eq!(
            store.rotate(&token).await.unwrap_err(),
            RefreshStoreError::Unavailable
        );
        assert_eq!(
            store.revoke_session(&token).await.unwrap_err(),
            RefreshStoreError::Unavailable
        );
        assert_eq!(
            store.revoke_all("user-1").await.unwrap_err(),
            RefreshStoreError::Unavailable
        );
    }

    #[tokio::test]
    async fn rejects_invalid_user_identifiers_before_touching_redis() {
        let store = store("redis://127.0.0.1:1", Duration::from_secs(30)).await;
        assert_eq!(
            store.issue("a:b").await.unwrap_err(),
            RefreshStoreError::InvalidIdentifier
        );
        assert_eq!(
            store.revoke_all("").await.unwrap_err(),
            RefreshStoreError::InvalidIdentifier
        );
    }

    #[tokio::test]
    async fn malformed_tokens_are_invalid_without_redis() {
        let store = store("redis://127.0.0.1:1", Duration::from_secs(30)).await;
        assert_eq!(
            store.rotate("not-a-token").await.unwrap(),
            RotationOutcome::Invalid
        );
        assert!(store.revoke_session("not-a-token").await.is_ok());
    }

    /// Tests that need the Redis service started by `scripts/integration-test.sh`.
    mod integration {
        use std::{env, sync::Arc};

        use super::*;

        async fn live_store(token_ttl: Duration) -> Arc<RedisRefreshSessionStore> {
            let url = env::var("REDIS_URL").expect("REDIS_URL is set by the integration script");
            Arc::new(store(&url, token_ttl).await)
        }

        fn unique_user() -> String {
            format!("user-{}", uuid::Uuid::new_v4().simple())
        }

        fn rotated(outcome: RotationOutcome) -> IssuedSession {
            match outcome {
                RotationOutcome::Rotated(session) => session,
                other => panic!("expected rotation, got {other:?}"),
            }
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn refresh_rotation_chain_and_replay_revokes_family() {
            let store = live_store(Duration::from_secs(30)).await;
            let first = store.issue(&unique_user()).await.unwrap();
            let second = rotated(store.rotate(&first.token).await.unwrap());
            let third = rotated(store.rotate(&second.token).await.unwrap());
            assert_eq!(second.family_id, first.family_id);
            assert_eq!(third.user_id, first.user_id);

            // Replaying an old token revokes the family, including the newest token.
            assert_eq!(
                store.rotate(&first.token).await.unwrap(),
                RotationOutcome::ReplayDetected
            );
            assert_eq!(
                store.rotate(&third.token).await.unwrap(),
                RotationOutcome::Revoked
            );
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn refresh_concurrent_rotation_allows_one_spend() {
            let store = live_store(Duration::from_secs(30)).await;
            let session = store.issue(&unique_user()).await.unwrap();

            let handles: Vec<_> = (0..32)
                .map(|_| {
                    let store = Arc::clone(&store);
                    let token = session.token.clone();
                    tokio::spawn(async move { store.rotate(&token).await.unwrap() })
                })
                .collect();
            let mut winners = Vec::new();
            for handle in handles {
                let outcome = handle.await.unwrap();
                match outcome {
                    RotationOutcome::Rotated(next) => winners.push(next),
                    RotationOutcome::ReplayDetected | RotationOutcome::Revoked => {}
                    other => panic!("unexpected outcome {other:?}"),
                }
            }

            assert_eq!(winners.len(), 1, "exactly one concurrent spend may win");
            // The losers' replay revoked the family, so the winner's token is dead too.
            assert_eq!(
                store.rotate(&winners[0].token).await.unwrap(),
                RotationOutcome::Revoked
            );
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn refresh_logout_revokes_only_that_session() {
            let store = live_store(Duration::from_secs(30)).await;
            let user = unique_user();
            let phone = store.issue(&user).await.unwrap();
            let laptop = store.issue(&user).await.unwrap();

            store.revoke_session(&phone.token).await.unwrap();
            // Logging out twice is harmless.
            store.revoke_session(&phone.token).await.unwrap();

            assert_eq!(
                store.rotate(&phone.token).await.unwrap(),
                RotationOutcome::Revoked
            );
            rotated(store.rotate(&laptop.token).await.unwrap());
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn refresh_logout_all_revokes_every_session_of_the_user() {
            let store = live_store(Duration::from_secs(30)).await;
            let user = unique_user();
            let other_user = unique_user();
            let phone = store.issue(&user).await.unwrap();
            let laptop = store.issue(&user).await.unwrap();
            let bystander = store.issue(&other_user).await.unwrap();

            store.revoke_all(&user).await.unwrap();

            for token in [&phone.token, &laptop.token] {
                assert_eq!(store.rotate(token).await.unwrap(), RotationOutcome::Revoked);
            }
            rotated(store.rotate(&bystander.token).await.unwrap());
        }

        #[tokio::test]
        #[ignore = "requires the integration-test Redis service"]
        async fn refresh_tokens_expire() {
            let store = live_store(Duration::from_secs(1)).await;
            let session = store.issue(&unique_user()).await.unwrap();

            tokio::time::sleep(Duration::from_millis(2200)).await;

            assert_eq!(
                store.rotate(&session.token).await.unwrap(),
                RotationOutcome::Invalid
            );
        }
    }
}
