use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use thiserror::Error;

use crate::config::Settings;

#[derive(Clone)]
pub struct Datastores {
    pub mysql: DatabaseConnection,
    pub redis: ConnectionManager,
    pub opensearch: OpenSearch,
}

impl Datastores {
    pub async fn initialize(settings: &Settings) -> Result<Self, InfrastructureError> {
        let mut mysql_options = ConnectOptions::new(settings.database.url.clone());
        mysql_options
            .min_connections(settings.database.min_connections)
            .max_connections(settings.database.max_connections)
            .connect_timeout(settings.database.connect_timeout)
            .connect_lazy(true)
            .sqlx_logging(false);
        let mysql = Database::connect(mysql_options).await?;

        let redis_client = redis::Client::open(settings.redis.url.as_str())?;
        let redis = ConnectionManager::new_lazy_with_config(
            redis_client,
            ConnectionManagerConfig::new()
                .set_connection_timeout(Some(settings.health_check_timeout))
                .set_response_timeout(Some(settings.health_check_timeout)),
        )?;

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
pub enum InfrastructureError {
    #[error("MySQL client configuration is invalid")]
    MySql(#[from] sea_orm::DbErr),
    #[error("Redis client configuration is invalid")]
    Redis(#[from] redis::RedisError),
    #[error("OpenSearch client configuration is invalid")]
    OpenSearch(#[from] opensearch::http::transport::BuildError),
}
