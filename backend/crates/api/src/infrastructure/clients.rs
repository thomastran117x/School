//! Construction of shared external-service clients.
//!
//! Clients are configured lazily so the HTTP process can start while dependencies are
//! unavailable and expose that state through the health feature.

use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use thiserror::Error;

use super::{
    database::{DatabaseClient, DatabaseInitializationError},
    redis::{RedisClient, RedisInitializationError},
};
use crate::config::Settings;

#[derive(Clone)]
/// Cloneable handles for every datastore used by the backend.
pub struct Datastores {
    /// Lazy SeaORM connection pool for MySQL.
    pub mysql: DatabaseClient,
    /// Bounded, auto-reconnecting Redis client.
    pub redis: RedisClient,
    /// Asynchronous client for the configured OpenSearch node.
    pub opensearch: OpenSearch,
}

impl Datastores {
    /// Creates validated, lazy datastore clients from application settings.
    ///
    /// Client construction validates connection URLs and authentication configuration
    /// but does not require any datastore to be reachable.
    ///
    /// # Errors
    ///
    /// Returns [`InfrastructureError`] when one of the client configurations cannot be
    /// converted into its underlying library client.
    pub async fn initialize(settings: &Settings) -> Result<Self, InfrastructureError> {
        let mysql = DatabaseClient::initialize(&settings.database).await?;

        let redis = RedisClient::initialize(&settings.redis).await?;

        let pool = SingleNodeConnectionPool::new(settings.opensearch.url.clone());
        let mut transport = TransportBuilder::new(pool);
        if let (Some(username), Some(password)) = (
            settings.opensearch.username.clone(),
            settings.opensearch.password.clone(),
        ) {
            transport = transport.auth(Credentials::Basic(username, password));
        }
        let opensearch = OpenSearch::new(transport.build()?);

        Ok(Self {
            mysql,
            redis,
            opensearch,
        })
    }
}

#[derive(Debug, Error)]
/// Errors raised while constructing shared datastore clients.
pub enum InfrastructureError {
    /// SeaORM rejected the MySQL connection or pool configuration.
    #[error("MySQL client configuration is invalid")]
    MySql(#[from] DatabaseInitializationError),
    /// The Redis client or connection manager configuration was invalid.
    #[error("Redis client configuration is invalid")]
    Redis(#[from] RedisInitializationError),
    /// The OpenSearch HTTP transport could not be built.
    #[error("OpenSearch client configuration is invalid")]
    OpenSearch(#[from] opensearch::http::transport::BuildError),
}
