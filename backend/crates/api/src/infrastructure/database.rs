//! Shared, bounded access to the SeaORM MySQL connection pool.
//!
//! The underlying SQLx pool is created once and cloned by handle. Callers submit
//! SeaORM operations through [`DatabaseClient::run`] so acquisition failures and
//! query deadlines have one sanitized error contract.

use std::{future::Future, pin::Pin, time::Duration};

use sea_orm::{ConnAcquireErr, ConnectOptions, Database, DatabaseConnection, DbErr};
use thiserror::Error;

use crate::config::DatabaseSettings;

type DatabaseFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, DbErr>> + Send + 'a>>;

#[derive(Clone)]
/// Cloneable access to the application's single SeaORM MySQL pool.
pub struct DatabaseClient {
    connection: DatabaseConnection,
    query_timeout: Duration,
}

impl DatabaseClient {
    /// Creates a lazy MySQL pool from validated settings.
    ///
    /// This validates SQLx connection options without requiring MySQL to be
    /// reachable. The first operation establishes physical connections.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseInitializationError`] when SeaORM rejects the pool or
    /// connection options. The error intentionally omits the URL and driver error.
    pub async fn initialize(
        settings: &DatabaseSettings,
    ) -> Result<Self, DatabaseInitializationError> {
        let mut options = ConnectOptions::new(settings.url.clone());
        let tls_mode = settings.tls_mode.as_sqlx();
        let tls_ca_path = settings.tls_ca_path.clone();
        options
            .min_connections(settings.min_connections)
            .max_connections(settings.max_connections)
            .connect_timeout(settings.connect_timeout)
            .acquire_timeout(settings.acquire_timeout)
            .connect_lazy(true)
            .sqlx_logging(false)
            .map_sqlx_mysql_opts(move |options| {
                let options = options.ssl_mode(tls_mode);
                match tls_ca_path.as_ref() {
                    Some(path) => options.ssl_ca(path),
                    None => options,
                }
            });

        let connection = Database::connect(options)
            .await
            .map_err(|_| DatabaseInitializationError)?;

        Ok(Self {
            connection,
            query_timeout: settings.query_timeout,
        })
    }

    /// Runs one SeaORM operation within the configured query deadline.
    ///
    /// The pool's shorter acquisition timeout remains responsible for identifying
    /// saturation before the overall query deadline expires.
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`DatabaseError`] for pool exhaustion, connection loss,
    /// query timeout, or another failed database operation.
    pub async fn run<T, F>(&self, operation: F) -> Result<T, DatabaseError>
    where
        F: for<'a> FnOnce(&'a DatabaseConnection) -> DatabaseFuture<'a, T>,
    {
        match tokio::time::timeout(self.query_timeout, operation(&self.connection)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(DatabaseError::from_db_error(error)),
            Err(_) => Err(DatabaseError::QueryTimedOut),
        }
    }

    /// Checks whether the pool can acquire a live MySQL connection.
    ///
    /// # Errors
    ///
    /// Returns the same sanitized errors as [`Self::run`].
    pub async fn ping(&self) -> Result<(), DatabaseError> {
        self.run(|connection| Box::pin(connection.ping())).await
    }

    /// Closes the pool, releasing idle connections and rejecting new acquisitions.
    ///
    /// Closing a pool that never connected succeeds immediately.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::Unavailable`] when the driver fails to close
    /// cleanly. The driver error is not exposed.
    pub async fn close(&self) -> Result<(), DatabaseError> {
        self.connection
            .clone()
            .close()
            .await
            .map_err(|_| DatabaseError::Unavailable)
    }
}

#[derive(Debug, Error)]
#[error("MySQL client configuration is invalid")]
/// Sanitized failure raised while constructing the lazy pool.
pub struct DatabaseInitializationError;

#[derive(Debug, Error)]
/// Sanitized failures returned by bounded database operations.
pub enum DatabaseError {
    /// No pooled connection became available before the acquisition deadline.
    #[error("database connection pool is exhausted")]
    PoolExhausted,
    /// The pool was closed or MySQL could not provide a usable connection.
    #[error("database is unavailable")]
    Unavailable,
    /// The complete operation exceeded its configured query deadline.
    #[error("database query timed out")]
    QueryTimedOut,
    /// MySQL rejected or could not complete an operation.
    #[error("database operation failed")]
    OperationFailed(DbErr),
}

impl DatabaseError {
    /// Classifies a SeaORM error without exposing its message through `Display`.
    fn from_db_error(error: DbErr) -> Self {
        match error {
            DbErr::ConnectionAcquire(ConnAcquireErr::Timeout) => Self::PoolExhausted,
            DbErr::ConnectionAcquire(ConnAcquireErr::ConnectionClosed) | DbErr::Conn(_) => {
                Self::Unavailable
            }
            other => Self::OperationFailed(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{env, time::Duration};

    use sea_orm::{ConnectionTrait, DbBackend, Statement};

    use super::*;
    use crate::config::DatabaseTlsMode;

    fn integration_settings(
        acquire_timeout: Duration,
        query_timeout: Duration,
    ) -> DatabaseSettings {
        DatabaseSettings {
            url: env::var("DATABASE_URL").expect("integration DATABASE_URL should be set"),
            min_connections: 0,
            max_connections: 1,
            connect_timeout: Duration::from_secs(2),
            acquire_timeout,
            query_timeout,
            tls_mode: DatabaseTlsMode::Disabled,
            tls_ca_path: None,
        }
    }

    #[test]
    fn classifies_pool_and_connection_errors() {
        assert!(matches!(
            DatabaseError::from_db_error(DbErr::ConnectionAcquire(ConnAcquireErr::Timeout)),
            DatabaseError::PoolExhausted
        ));
        assert!(matches!(
            DatabaseError::from_db_error(DbErr::ConnectionAcquire(
                ConnAcquireErr::ConnectionClosed
            )),
            DatabaseError::Unavailable
        ));
        assert!(matches!(
            DatabaseError::from_db_error(DbErr::Custom("safe fixture".to_owned())),
            DatabaseError::OperationFailed(_)
        ));
    }

    #[test]
    fn database_errors_have_sanitized_display_messages() {
        let error = DatabaseError::OperationFailed(DbErr::Custom(
            "mysql://user:secret@example.invalid/database".to_owned(),
        ));

        assert_eq!(error.to_string(), "database operation failed");
        assert!(!error.to_string().contains("secret"));
        assert_eq!(
            DatabaseInitializationError.to_string(),
            "MySQL client configuration is invalid"
        );
    }

    #[tokio::test]
    #[ignore = "requires the integration-test MySQL service"]
    async fn mysql_pool_enforces_tls_and_timeouts() {
        let mut connectivity_settings =
            integration_settings(Duration::from_secs(1), Duration::from_secs(1));
        connectivity_settings.tls_mode = DatabaseTlsMode::Required;
        let connectivity = DatabaseClient::initialize(&connectivity_settings)
            .await
            .expect("integration pool should initialize");
        let result = connectivity
            .run(|connection| {
                Box::pin(connection.query_one(Statement::from_string(
                    DbBackend::MySql,
                    "SELECT 1".to_owned(),
                )))
            })
            .await;
        assert!(
            result.is_ok(),
            "TLS connectivity query should succeed: {result:?}"
        );

        let query_timeout = DatabaseClient::initialize(&integration_settings(
            Duration::from_secs(1),
            Duration::from_millis(50),
        ))
        .await
        .expect("query-timeout pool should initialize");
        let error = query_timeout
            .run(|connection| Box::pin(connection.execute_unprepared("SELECT SLEEP(0.25)")))
            .await
            .expect_err("delayed query should time out");
        assert!(matches!(error, DatabaseError::QueryTimedOut));

        let exhausted = DatabaseClient::initialize(&integration_settings(
            Duration::from_millis(50),
            Duration::from_secs(1),
        ))
        .await
        .expect("pool-exhaustion pool should initialize");
        let held_connection = exhausted
            .connection
            .get_mysql_connection_pool()
            .acquire()
            .await
            .expect("first pooled connection should be available");
        let error = exhausted
            .ping()
            .await
            .expect_err("second acquisition should exhaust the pool");
        assert!(matches!(error, DatabaseError::PoolExhausted));
        drop(held_connection);
    }
}
