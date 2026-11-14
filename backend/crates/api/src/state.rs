//! Dependencies shared with HTTP feature handlers.
//!
//! State contains service contracts rather than concrete adapters so feature routers
//! can be tested without external processes. Every dependency is constructed once at
//! startup; cloning the state only clones cheap handles (`Arc`s and pooled clients),
//! so no request ever builds its own connection or pool.
//!
//! # Adding a dependency
//!
//! 1. Add a field to [`AppState`] and to [`AppStateParts`].
//! 2. Construct it once in `main.rs` and pass it through [`AppState::from_parts`].
//! 3. Extract it in handlers with `State<AppState>`, or with `State<T>` when a
//!    [`FromRef`] implementation exists for `T`.

use std::sync::Arc;

use axum::extract::FromRef;
use opensearch::OpenSearch;

use crate::{
    config::Settings,
    features::{
        auth::refresh_store::RefreshSessionStore, courses::cache::CourseCache,
        health::HealthService,
    },
    infrastructure::{DatabaseClient, RedisClient},
};

#[derive(Clone)]
/// Cloneable application state injected into Axum handlers.
pub struct AppState {
    /// Validated application configuration, shared without copying.
    #[allow(dead_code)] // Consumed by upcoming features; initialized once here.
    pub config: Arc<Settings>,
    /// Shared, bounded access to the application's MySQL pool.
    pub database: DatabaseClient,
    /// Shared, bounded access to the application's Redis connection.
    #[allow(dead_code)] // Consumed by upcoming features; initialized once here.
    pub redis: RedisClient,
    /// Shared OpenSearch client (lazy; nothing connects until first use).
    #[allow(dead_code)] // Consumed by upcoming search features; initialized once here.
    pub opensearch: OpenSearch,
    /// Health service used by liveness and readiness handlers.
    pub(crate) health: Arc<dyn HealthService>,
    /// Refresh-session store consumed by the authentication endpoints.
    #[allow(dead_code)] // Consumed by AUTH-07.
    pub(crate) refresh_sessions: Arc<dyn RefreshSessionStore>,
    /// Fail-open Redis cache for course catalogue reads.
    #[allow(dead_code)] // Consumed by the course catalogue service.
    pub(crate) course_cache: Arc<CourseCache>,
}

/// Named inputs for [`AppState::from_parts`].
///
/// A struct keeps construction call sites stable when new dependencies are added.
pub(crate) struct AppStateParts {
    /// Validated application configuration.
    pub config: Arc<Settings>,
    /// Shared MySQL client.
    pub database: DatabaseClient,
    /// Shared Redis client.
    pub redis: RedisClient,
    /// Shared OpenSearch client.
    pub opensearch: OpenSearch,
    /// Health service implementation.
    pub(crate) health: Arc<dyn HealthService>,
    /// Refresh-session store implementation.
    pub(crate) refresh_sessions: Arc<dyn RefreshSessionStore>,
    /// Course catalogue cache.
    pub(crate) course_cache: Arc<CourseCache>,
}

impl AppState {
    /// Creates application state from already-constructed dependencies.
    pub(crate) fn from_parts(parts: AppStateParts) -> Self {
        Self {
            config: parts.config,
            database: parts.database,
            redis: parts.redis,
            opensearch: parts.opensearch,
            health: parts.health,
            refresh_sessions: parts.refresh_sessions,
            course_cache: parts.course_cache,
        }
    }
}

impl FromRef<AppState> for DatabaseClient {
    fn from_ref(state: &AppState) -> Self {
        state.database.clone()
    }
}

impl FromRef<AppState> for RedisClient {
    fn from_ref(state: &AppState) -> Self {
        state.redis.clone()
    }
}

impl FromRef<AppState> for Arc<Settings> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.config)
    }
}

/// Test helpers for building [`AppState`] without live datastores.
#[cfg(test)]
pub(crate) mod test_support {
    use std::{sync::Arc, time::Duration};

    use opensearch::{
        OpenSearch,
        http::transport::{SingleNodeConnectionPool, TransportBuilder},
    };

    use super::{AppState, AppStateParts};
    use crate::{
        config::{
            AuthSettings, CourseCacheSettings, DatabaseSettings, DatabaseTlsMode, RedisSettings,
            RedisTlsMode, Settings,
        },
        features::{
            auth::refresh_store::RedisRefreshSessionStore, courses::cache::CourseCache,
            health::HealthService,
        },
        infrastructure::{DatabaseClient, RedisClient},
    };

    /// Builds state backed by lazy clients that never connect, using `health` as the
    /// readiness service so tests control dependency status.
    pub(crate) async fn lazy_state(health: Arc<dyn HealthService>) -> AppState {
        let database = DatabaseClient::initialize(&DatabaseSettings {
            url: "mysql://test:test@localhost/test".to_owned(),
            min_connections: 0,
            max_connections: 1,
            connect_timeout: Duration::from_secs(1),
            acquire_timeout: Duration::from_secs(1),
            query_timeout: Duration::from_secs(1),
            tls_mode: DatabaseTlsMode::Disabled,
            tls_ca_path: None,
        })
        .await
        .expect("lazy test database should initialize");
        let redis = RedisClient::initialize(&RedisSettings {
            url: "redis://localhost:6379".to_owned(),
            connect_timeout: Duration::from_secs(1),
            command_timeout: Duration::from_secs(1),
            tls_mode: RedisTlsMode::Disabled,
            tls_ca_path: None,
        })
        .await
        .expect("lazy test redis client should initialize");
        let opensearch = OpenSearch::new(
            TransportBuilder::new(SingleNodeConnectionPool::new(
                "http://localhost:9200"
                    .parse()
                    .expect("fixed test URL is valid"),
            ))
            .build()
            .expect("lazy test opensearch client should build"),
        );
        let refresh_sessions = Arc::new(RedisRefreshSessionStore::new(
            redis.clone(),
            AuthSettings {
                refresh_token_ttl: Duration::from_secs(60),
                refresh_family_max_lifetime: Duration::from_secs(120),
            },
        ));
        let course_cache = Arc::new(CourseCache::new(
            redis.clone(),
            CourseCacheSettings::default(),
        ));
        AppState::from_parts(AppStateParts {
            config: Arc::new(Settings::for_tests()),
            database,
            redis,
            opensearch,
            health,
            refresh_sessions,
            course_cache,
        })
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{Request, StatusCode},
        routing::get,
    };
    use tower::ServiceExt;

    use super::*;
    use crate::features::health::{DependencyState, DependencyStatuses, ReadinessReport};

    struct StubHealth;

    #[async_trait]
    impl HealthService for StubHealth {
        async fn check(&self, _database: &DatabaseClient) -> ReadinessReport {
            ReadinessReport {
                dependencies: DependencyStatuses::all(DependencyState::Up),
            }
        }
    }

    fn assert_shareable<T: Clone + Send + Sync + 'static>() {}

    #[test]
    fn state_is_cloneable_and_thread_safe() {
        assert_shareable::<AppState>();
    }

    #[tokio::test]
    async fn sub_state_is_extracted_from_app_state() {
        async fn handler(State(settings): State<Arc<Settings>>) -> String {
            settings.service_name.clone()
        }

        let state = test_support::lazy_state(Arc::new(StubHealth)).await;
        let expected = state.config.service_name.clone();
        let response = Router::new()
            .route("/", get(handler))
            .with_state(state)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes, expected.as_bytes());
    }
}
