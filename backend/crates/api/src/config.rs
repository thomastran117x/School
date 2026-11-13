//! Environment-backed application configuration.
//!
//! Values are read once during startup, validated together, and then passed to the
//! components that need them. A local `.env` file is loaded when present.

use std::{env, net::SocketAddr, str::FromStr, time::Duration};

use thiserror::Error;
use url::Url;

#[derive(Clone)]
/// Validated settings for the complete API process.
pub struct Settings {
    /// Interface or IP address on which the HTTP server listens.
    pub server_host: String,
    /// TCP port on which the HTTP server listens.
    pub server_port: u16,
    /// MySQL and SeaORM pool settings.
    pub database: DatabaseSettings,
    /// Redis client settings.
    pub redis: RedisSettings,
    /// OpenSearch client and authentication settings.
    pub opensearch: OpenSearchSettings,
    /// Maximum duration allowed for each dependency readiness probe.
    pub health_check_timeout: Duration,
    /// [`tracing_subscriber::EnvFilter`] expression used by application telemetry.
    pub log_filter: String,
}

#[derive(Clone)]
/// MySQL connection and pool configuration used by SeaORM.
pub struct DatabaseSettings {
    /// MySQL connection URL, including credentials and database name.
    pub url: String,
    /// Minimum number of connections retained by the pool.
    pub min_connections: u32,
    /// Maximum number of connections allowed in the pool.
    pub max_connections: u32,
    /// Maximum duration allowed when establishing a MySQL connection.
    pub connect_timeout: Duration,
}

#[derive(Clone)]
/// Redis connection configuration.
pub struct RedisSettings {
    /// Redis connection URL using the `redis` or `rediss` scheme.
    pub url: String,
}

#[derive(Clone)]
/// OpenSearch endpoint and optional basic-auth credentials.
pub struct OpenSearchSettings {
    /// Base HTTP(S) URL for the OpenSearch node.
    pub url: Url,
    /// Optional basic-auth username; must be paired with [`Self::password`].
    pub username: Option<String>,
    /// Optional basic-auth password; must be paired with [`Self::username`].
    pub password: Option<String>,
}

impl Settings {
    /// Loads and validates settings from the process environment.
    ///
    /// A local `.env` file is loaded before environment variables are read. Existing
    /// process variables retain precedence.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when a required value is missing or any supplied value
    /// fails validation.
    pub fn from_env() -> Result<Self, ConfigError> {
        dotenvy::dotenv().ok();
        Self::from_source(|key| env::var(key).ok())
    }

    /// Loads settings from an abstract key-value source.
    ///
    /// Keeping parsing independent from process-global environment mutation makes the
    /// configuration contract deterministic in unit tests.
    fn from_source(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let server_host = optional(&get, "APP_HOST", "0.0.0.0");
        let server_port = parse(&get, "APP_PORT", "3000")?;
        let database_url = required(&get, "DATABASE_URL")?;
        validate_url_scheme("DATABASE_URL", &database_url, &["mysql"])?;

        let min_connections = parse(&get, "DATABASE_MIN_CONNECTIONS", "0")?;
        let max_connections = parse(&get, "DATABASE_MAX_CONNECTIONS", "10")?;
        if max_connections == 0 || min_connections > max_connections {
            return Err(ConfigError::InvalidPoolLimits {
                min: min_connections,
                max: max_connections,
            });
        }

        let redis_url = required(&get, "REDIS_URL")?;
        validate_url_scheme("REDIS_URL", &redis_url, &["redis", "rediss"])?;

        let opensearch_url_raw = required(&get, "OPENSEARCH_URL")?;
        let opensearch_url =
            validate_url_scheme("OPENSEARCH_URL", &opensearch_url_raw, &["http", "https"])?;
        let username = non_empty(get("OPENSEARCH_USERNAME"));
        let password = non_empty(get("OPENSEARCH_PASSWORD"));
        if username.is_some() != password.is_some() {
            return Err(ConfigError::IncompleteOpenSearchCredentials);
        }

        let health_check_timeout_ms: u64 = parse(&get, "HEALTH_CHECK_TIMEOUT_MS", "2000")?;
        if health_check_timeout_ms == 0 {
            return Err(ConfigError::ZeroDuration("HEALTH_CHECK_TIMEOUT_MS"));
        }
        let database_connect_timeout_ms: u64 = parse(&get, "DATABASE_CONNECT_TIMEOUT_MS", "2000")?;
        if database_connect_timeout_ms == 0 {
            return Err(ConfigError::ZeroDuration("DATABASE_CONNECT_TIMEOUT_MS"));
        }

        let settings = Self {
            server_host,
            server_port,
            database: DatabaseSettings {
                url: database_url,
                min_connections,
                max_connections,
                connect_timeout: Duration::from_millis(database_connect_timeout_ms),
            },
            redis: RedisSettings { url: redis_url },
            opensearch: OpenSearchSettings {
                url: opensearch_url,
                username,
                password,
            },
            health_check_timeout: Duration::from_millis(health_check_timeout_ms),
            log_filter: optional(&get, "RUST_LOG", "backend_api=info,tower_http=info"),
        };

        settings.server_address()?;
        Ok(settings)
    }

    /// Combines the configured HTTP host and port into a socket address.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidSocketAddress`] when the two values do not form
    /// a valid IP socket address.
    pub fn server_address(&self) -> Result<SocketAddr, ConfigError> {
        SocketAddr::from_str(&format!("{}:{}", self.server_host, self.server_port)).map_err(|_| {
            ConfigError::InvalidSocketAddress {
                host: self.server_host.clone(),
                port: self.server_port,
            }
        })
    }
}

/// Reads a required, non-blank value from a configuration source.
fn required(
    get: &impl Fn(&str) -> Option<String>,
    key: &'static str,
) -> Result<String, ConfigError> {
    non_empty(get(key)).ok_or(ConfigError::Missing(key))
}

/// Reads a non-blank value or returns its configured default.
fn optional(get: &impl Fn(&str) -> Option<String>, key: &str, default: &str) -> String {
    non_empty(get(key)).unwrap_or_else(|| default.to_owned())
}

/// Converts blank strings into missing values.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

/// Parses a typed value from a configuration source or its default.
fn parse<T>(
    get: &impl Fn(&str) -> Option<String>,
    key: &'static str,
    default: &str,
) -> Result<T, ConfigError>
where
    T: FromStr,
{
    let value = optional(get, key, default);
    value
        .parse()
        .map_err(|_| ConfigError::InvalidValue { key, value })
}

/// Parses a URL and restricts it to an explicit set of schemes.
fn validate_url_scheme(
    key: &'static str,
    value: &str,
    allowed_schemes: &[&str],
) -> Result<Url, ConfigError> {
    let parsed = Url::parse(value).map_err(|_| ConfigError::InvalidUrl(key))?;
    if !allowed_schemes.contains(&parsed.scheme()) {
        return Err(ConfigError::InvalidUrlScheme {
            key,
            expected: allowed_schemes.join(" or "),
        });
    }
    Ok(parsed)
}

#[derive(Debug, Error, PartialEq, Eq)]
/// Validation failures produced while loading [`Settings`].
pub enum ConfigError {
    /// A required environment variable was not set or contained only whitespace.
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    /// An environment variable could not be parsed into its expected scalar type.
    #[error("{key} has invalid value {value:?}")]
    InvalidValue {
        /// Name of the invalid environment variable.
        key: &'static str,
        /// Invalid value supplied by the environment.
        value: String,
    },
    /// An endpoint setting was not a syntactically valid URL.
    #[error("{0} must be a valid URL")]
    InvalidUrl(&'static str),
    /// A URL used a scheme that is unsupported for its target service.
    #[error("{key} must use the {expected} URL scheme")]
    InvalidUrlScheme {
        /// Name of the environment variable containing the URL.
        key: &'static str,
        /// Human-readable list of accepted URL schemes.
        expected: String,
    },
    /// MySQL pool minimum and maximum values were inconsistent.
    #[error(
        "DATABASE_MIN_CONNECTIONS ({min}) cannot exceed DATABASE_MAX_CONNECTIONS ({max}), and the maximum must be greater than zero"
    )]
    InvalidPoolLimits {
        /// Configured minimum connection count.
        min: u32,
        /// Configured maximum connection count.
        max: u32,
    },
    /// Only one of the two required OpenSearch basic-auth values was supplied.
    #[error(
        "OPENSEARCH_USERNAME and OPENSEARCH_PASSWORD must either both be set or both be omitted"
    )]
    IncompleteOpenSearchCredentials,
    /// A duration setting was configured as zero milliseconds.
    #[error("{0} must be greater than zero")]
    ZeroDuration(&'static str),
    /// The configured HTTP host and port did not form a valid socket address.
    #[error("APP_HOST {host:?} and APP_PORT {port} do not form a valid socket address")]
    InvalidSocketAddress {
        /// Configured HTTP host.
        host: String,
        /// Configured HTTP port.
        port: u16,
    },
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn settings(values: &[(&str, &str)]) -> Result<Settings, ConfigError> {
        let values: HashMap<String, String> = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        Settings::from_source(|key| values.get(key).cloned())
    }

    fn required_values() -> Vec<(&'static str, &'static str)> {
        vec![
            ("DATABASE_URL", "mysql://app:secret@localhost/app"),
            ("REDIS_URL", "redis://localhost:6379"),
            ("OPENSEARCH_URL", "http://localhost:9200"),
        ]
    }

    #[test]
    fn loads_required_values_and_defaults() {
        let loaded = settings(&required_values()).unwrap();

        assert_eq!(loaded.server_address().unwrap().to_string(), "0.0.0.0:3000");
        assert_eq!(loaded.database.min_connections, 0);
        assert_eq!(loaded.database.max_connections, 10);
        assert_eq!(loaded.health_check_timeout, Duration::from_secs(2));
        assert!(loaded.opensearch.username.is_none());
    }

    #[test]
    fn rejects_a_missing_required_value() {
        let error = settings(&[
            ("REDIS_URL", "redis://localhost:6379"),
            ("OPENSEARCH_URL", "http://localhost:9200"),
        ])
        .err()
        .unwrap();

        assert_eq!(error, ConfigError::Missing("DATABASE_URL"));
    }

    #[test]
    fn rejects_wrong_url_schemes() {
        let mut values = required_values();
        values[0] = ("DATABASE_URL", "postgres://localhost/app");

        assert!(matches!(
            settings(&values),
            Err(ConfigError::InvalidUrlScheme {
                key: "DATABASE_URL",
                ..
            })
        ));
    }

    #[test]
    fn rejects_invalid_pool_limits() {
        let mut values = required_values();
        values.extend([
            ("DATABASE_MIN_CONNECTIONS", "11"),
            ("DATABASE_MAX_CONNECTIONS", "10"),
        ]);

        assert_eq!(
            settings(&values).err().unwrap(),
            ConfigError::InvalidPoolLimits { min: 11, max: 10 }
        );
    }

    #[test]
    fn requires_complete_opensearch_credentials() {
        let mut values = required_values();
        values.push(("OPENSEARCH_USERNAME", "reader"));

        assert_eq!(
            settings(&values).err().unwrap(),
            ConfigError::IncompleteOpenSearchCredentials
        );
    }
}
