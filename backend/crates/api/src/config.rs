//! Environment-backed application configuration.
//!
//! Values are read once during startup, validated together, and then passed to the
//! components that need them. A local `.env` file is loaded when present.

use std::{env, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use sea_orm::sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};
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
    /// Refresh-session lifetime settings.
    pub auth: AuthSettings,
    /// Course catalogue cache lifetimes.
    // Consumed once the catalogue read service constructs its `CourseCache`.
    #[allow(dead_code)]
    pub course_cache: CourseCacheSettings,
    /// Maximum duration allowed for each dependency readiness probe.
    pub health_check_timeout: Duration,
    /// [`tracing_subscriber::EnvFilter`] expression used by application telemetry.
    pub log_filter: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Lifetime settings for the refresh-session store.
pub struct AuthSettings {
    /// Lifetime of each individual refresh token.
    pub refresh_token_ttl: Duration,
    /// Absolute lifetime of a session family, regardless of how often it rotates.
    pub refresh_family_max_lifetime: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Lifetimes for the course catalogue cache. They bound how stale a cached read can be
/// when an invalidation is missed (for example because Redis was unreachable).
pub struct CourseCacheSettings {
    /// Lifetime of a cached single-course read.
    pub detail_ttl: Duration,
    /// Lifetime of a cached catalogue listing page.
    pub list_ttl: Duration,
    /// Lifetime of a cached "course not found" result.
    pub negative_ttl: Duration,
}

impl Default for CourseCacheSettings {
    fn default() -> Self {
        Self {
            detail_ttl: Duration::from_secs(300),
            list_ttl: Duration::from_secs(60),
            negative_ttl: Duration::from_secs(30),
        }
    }
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
    /// Maximum duration allowed while waiting for a pooled connection.
    pub acquire_timeout: Duration,
    /// Maximum duration allowed for one database operation.
    pub query_timeout: Duration,
    /// TLS verification policy applied to MySQL connections.
    pub tls_mode: DatabaseTlsMode,
    /// Optional path to an additional PEM-encoded certificate authority bundle.
    pub tls_ca_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// TLS policy used by the MySQL client.
pub enum DatabaseTlsMode {
    /// Do not negotiate TLS. This is reserved for the isolated local stack.
    Disabled,
    /// Attempt TLS but permit a plaintext fallback.
    Preferred,
    /// Require encrypted transport without certificate verification.
    Required,
    /// Require TLS and verify the certificate authority.
    VerifyCa,
    /// Require TLS and verify both the certificate authority and server hostname.
    VerifyIdentity,
}

impl DatabaseTlsMode {
    /// Converts the validated setting into SQLx's MySQL TLS mode.
    pub fn as_sqlx(self) -> MySqlSslMode {
        match self {
            Self::Disabled => MySqlSslMode::Disabled,
            Self::Preferred => MySqlSslMode::Preferred,
            Self::Required => MySqlSslMode::Required,
            Self::VerifyCa => MySqlSslMode::VerifyCa,
            Self::VerifyIdentity => MySqlSslMode::VerifyIdentity,
        }
    }

    /// Returns whether the mode performs certificate validation.
    fn verifies_certificate(self) -> bool {
        matches!(self, Self::VerifyCa | Self::VerifyIdentity)
    }
}

impl FromStr for DatabaseTlsMode {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "disabled" => Ok(Self::Disabled),
            "preferred" => Ok(Self::Preferred),
            "required" => Ok(Self::Required),
            "verify_ca" => Ok(Self::VerifyCa),
            "verify_identity" => Ok(Self::VerifyIdentity),
            _ => Err(()),
        }
    }
}

#[derive(Clone)]
/// Redis connection configuration.
pub struct RedisSettings {
    /// Redis connection URL using the `redis` or `rediss` scheme. It may contain a
    /// password, so this struct deliberately does not implement `Debug`.
    pub url: String,
    /// Maximum duration allowed when establishing or re-establishing a connection.
    pub connect_timeout: Duration,
    /// Maximum duration allowed for one Redis command, including reconnection.
    pub command_timeout: Duration,
    /// TLS policy applied to Redis connections.
    pub tls_mode: RedisTlsMode,
    /// Optional path to an additional PEM-encoded certificate authority.
    pub tls_ca_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// TLS policy used by the Redis client.
pub enum RedisTlsMode {
    /// Do not use TLS. This is reserved for the isolated local stack.
    Disabled,
    /// Require encrypted transport without certificate or hostname verification.
    Required,
    /// Require TLS and verify the server certificate and hostname.
    Verify,
}

impl FromStr for RedisTlsMode {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "disabled" => Ok(Self::Disabled),
            "required" => Ok(Self::Required),
            "verify" => Ok(Self::Verify),
            _ => Err(()),
        }
    }
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
        let parsed_database_url = validate_url_scheme("DATABASE_URL", &database_url, &["mysql"])?;
        if parsed_database_url.query_pairs().any(|(key, _)| {
            matches!(
                key.to_ascii_lowercase().as_str(),
                "ssl-mode" | "sslmode" | "ssl-ca" | "sslca"
            )
        }) {
            return Err(ConfigError::DatabaseTlsInUrl);
        }
        database_url
            .parse::<MySqlConnectOptions>()
            .map_err(|_| ConfigError::InvalidUrl("DATABASE_URL"))?;

        let min_connections = parse(&get, "DATABASE_MIN_CONNECTIONS", "0")?;
        let max_connections = parse(&get, "DATABASE_MAX_CONNECTIONS", "10")?;
        if max_connections == 0 || min_connections > max_connections {
            return Err(ConfigError::InvalidPoolLimits {
                min: min_connections,
                max: max_connections,
            });
        }

        let redis_url = required(&get, "REDIS_URL")?;
        let parsed_redis_url = validate_url_scheme("REDIS_URL", &redis_url, &["redis", "rediss"])?;
        let redis_tls_mode: RedisTlsMode = parse(&get, "REDIS_TLS_MODE", "verify")?;
        let uses_tls_scheme = parsed_redis_url.scheme() == "rediss";
        if uses_tls_scheme != (redis_tls_mode != RedisTlsMode::Disabled) {
            return Err(ConfigError::RedisTlsSchemeMismatch);
        }
        let redis_connect_timeout = duration(&get, "REDIS_CONNECT_TIMEOUT_MS", "2000")?;
        let redis_command_timeout = duration(&get, "REDIS_COMMAND_TIMEOUT_MS", "1000")?;
        let redis_tls_ca_path = non_empty(get("REDIS_TLS_CA_PATH")).map(PathBuf::from);
        if let Some(path) = redis_tls_ca_path.as_ref() {
            if redis_tls_mode != RedisTlsMode::Verify {
                return Err(ConfigError::RedisTlsCaRequiresVerify);
            }
            if !path.is_file() || std::fs::File::open(path).is_err() {
                return Err(ConfigError::InvalidRedisTlsCaPath(path.clone()));
            }
        }

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
        let database_connect_timeout = duration(&get, "DATABASE_CONNECT_TIMEOUT_MS", "2000")?;
        let database_acquire_timeout = duration(&get, "DATABASE_ACQUIRE_TIMEOUT_MS", "2000")?;
        let database_query_timeout = duration(&get, "DATABASE_QUERY_TIMEOUT_MS", "5000")?;
        let database_tls_mode: DatabaseTlsMode =
            parse(&get, "DATABASE_TLS_MODE", "verify_identity")?;
        let database_tls_ca_path = non_empty(get("DATABASE_TLS_CA_PATH")).map(PathBuf::from);
        if let Some(path) = database_tls_ca_path.as_ref() {
            if !database_tls_mode.verifies_certificate() {
                return Err(ConfigError::TlsCaRequiresVerification);
            }
            if !path.is_file() || std::fs::File::open(path).is_err() {
                return Err(ConfigError::InvalidTlsCaPath(path.clone()));
            }
        }

        let refresh_token_ttl = seconds(&get, "REFRESH_TOKEN_TTL_SECONDS", "604800")?;
        let refresh_family_max_lifetime =
            seconds(&get, "REFRESH_FAMILY_MAX_LIFETIME_SECONDS", "2592000")?;
        if refresh_family_max_lifetime < refresh_token_ttl {
            return Err(ConfigError::InvalidAuthLifetimes);
        }

        let course_cache = CourseCacheSettings {
            detail_ttl: seconds(&get, "COURSE_CACHE_DETAIL_TTL_SECONDS", "300")?,
            list_ttl: seconds(&get, "COURSE_CACHE_LIST_TTL_SECONDS", "60")?,
            negative_ttl: seconds(&get, "COURSE_CACHE_NEGATIVE_TTL_SECONDS", "30")?,
        };
        if course_cache.list_ttl > course_cache.detail_ttl
            || course_cache.negative_ttl > course_cache.detail_ttl
        {
            return Err(ConfigError::InvalidCourseCacheTtls);
        }

        let settings = Self {
            server_host,
            server_port,
            database: DatabaseSettings {
                url: database_url,
                min_connections,
                max_connections,
                connect_timeout: database_connect_timeout,
                acquire_timeout: database_acquire_timeout,
                query_timeout: database_query_timeout,
                tls_mode: database_tls_mode,
                tls_ca_path: database_tls_ca_path,
            },
            redis: RedisSettings {
                url: redis_url,
                connect_timeout: redis_connect_timeout,
                command_timeout: redis_command_timeout,
                tls_mode: redis_tls_mode,
                tls_ca_path: redis_tls_ca_path,
            },
            opensearch: OpenSearchSettings {
                url: opensearch_url,
                username,
                password,
            },
            auth: AuthSettings {
                refresh_token_ttl,
                refresh_family_max_lifetime,
            },
            course_cache,
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

/// Parses a nonzero millisecond duration from a configuration source.
fn duration(
    get: &impl Fn(&str) -> Option<String>,
    key: &'static str,
    default: &str,
) -> Result<Duration, ConfigError> {
    let milliseconds: u64 = parse(get, key, default)?;
    if milliseconds == 0 {
        return Err(ConfigError::ZeroDuration(key));
    }
    Ok(Duration::from_millis(milliseconds))
}

/// Parses a nonzero second duration no longer than the maximum Redis TTL.
fn seconds(
    get: &impl Fn(&str) -> Option<String>,
    key: &'static str,
    default: &str,
) -> Result<Duration, ConfigError> {
    let seconds: u64 = parse(get, key, default)?;
    if seconds == 0 {
        return Err(ConfigError::ZeroDuration(key));
    }
    let value = Duration::from_secs(seconds);
    if value > crate::infrastructure::MAX_TTL {
        return Err(ConfigError::InvalidValue {
            key,
            value: seconds.to_string(),
        });
    }
    Ok(value)
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
    /// The database URL contained TLS settings managed by dedicated variables.
    #[error(
        "DATABASE_URL must not contain ssl-mode or ssl-ca; use DATABASE_TLS_MODE and DATABASE_TLS_CA_PATH"
    )]
    DatabaseTlsInUrl,
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
    /// A custom CA path was supplied for a TLS mode that does not verify certificates.
    #[error("DATABASE_TLS_CA_PATH requires DATABASE_TLS_MODE verify_ca or verify_identity")]
    TlsCaRequiresVerification,
    /// The configured custom certificate authority path was not a readable file.
    #[error("DATABASE_TLS_CA_PATH does not identify a readable file: {0:?}")]
    InvalidTlsCaPath(PathBuf),
    /// `REDIS_URL` scheme and `REDIS_TLS_MODE` disagree about whether TLS is used.
    #[error("REDIS_URL must use rediss:// exactly when REDIS_TLS_MODE is required or verify")]
    RedisTlsSchemeMismatch,
    /// A Redis CA path was supplied for a TLS mode that does not verify certificates.
    #[error("REDIS_TLS_CA_PATH requires REDIS_TLS_MODE verify")]
    RedisTlsCaRequiresVerify,
    /// The configured Redis certificate authority path was not a readable file.
    #[error("REDIS_TLS_CA_PATH does not identify a readable file: {0:?}")]
    InvalidRedisTlsCaPath(PathBuf),
    /// A duration setting was configured as zero milliseconds.
    #[error("{0} must be greater than zero")]
    ZeroDuration(&'static str),
    /// The refresh family lifetime was shorter than the refresh token lifetime.
    #[error("REFRESH_FAMILY_MAX_LIFETIME_SECONDS must be at least REFRESH_TOKEN_TTL_SECONDS")]
    InvalidAuthLifetimes,
    /// The list or negative cache TTL exceeded the detail cache TTL.
    #[error(
        "COURSE_CACHE_LIST_TTL_SECONDS and COURSE_CACHE_NEGATIVE_TTL_SECONDS must not exceed COURSE_CACHE_DETAIL_TTL_SECONDS"
    )]
    InvalidCourseCacheTtls,
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
            ("REDIS_URL", "rediss://localhost:6379"),
            ("OPENSEARCH_URL", "http://localhost:9200"),
        ]
    }

    #[test]
    fn loads_required_values_and_defaults() {
        let loaded = settings(&required_values()).unwrap();

        assert_eq!(loaded.server_address().unwrap().to_string(), "0.0.0.0:3000");
        assert_eq!(loaded.database.min_connections, 0);
        assert_eq!(loaded.database.max_connections, 10);
        assert_eq!(loaded.database.connect_timeout, Duration::from_secs(2));
        assert_eq!(loaded.database.acquire_timeout, Duration::from_secs(2));
        assert_eq!(loaded.database.query_timeout, Duration::from_secs(5));
        assert_eq!(loaded.database.tls_mode, DatabaseTlsMode::VerifyIdentity);
        assert_eq!(loaded.health_check_timeout, Duration::from_secs(2));
        assert!(loaded.opensearch.username.is_none());
        assert_eq!(
            loaded.auth.refresh_token_ttl,
            Duration::from_secs(7 * 24 * 60 * 60)
        );
        assert_eq!(
            loaded.auth.refresh_family_max_lifetime,
            Duration::from_secs(30 * 24 * 60 * 60)
        );
    }

    #[test]
    fn validates_refresh_lifetimes() {
        let with = |extra: &[(&'static str, &'static str)]| {
            let mut values = required_values();
            values.extend_from_slice(extra);
            settings(&values).err()
        };

        assert_eq!(
            with(&[("REFRESH_TOKEN_TTL_SECONDS", "0")]),
            Some(ConfigError::ZeroDuration("REFRESH_TOKEN_TTL_SECONDS"))
        );
        assert_eq!(
            with(&[("REFRESH_FAMILY_MAX_LIFETIME_SECONDS", "99999999")]),
            Some(ConfigError::InvalidValue {
                key: "REFRESH_FAMILY_MAX_LIFETIME_SECONDS",
                value: "99999999".to_owned()
            })
        );
        assert_eq!(
            with(&[
                ("REFRESH_TOKEN_TTL_SECONDS", "100"),
                ("REFRESH_FAMILY_MAX_LIFETIME_SECONDS", "50"),
            ]),
            Some(ConfigError::InvalidAuthLifetimes)
        );
    }

    #[test]
    fn loads_and_validates_course_cache_ttls() {
        let loaded = settings(&required_values()).unwrap();
        assert_eq!(loaded.course_cache, CourseCacheSettings::default());

        let with = |extra: &[(&'static str, &'static str)]| {
            let mut values = required_values();
            values.extend_from_slice(extra);
            settings(&values).err()
        };
        assert_eq!(
            with(&[("COURSE_CACHE_LIST_TTL_SECONDS", "0")]),
            Some(ConfigError::ZeroDuration("COURSE_CACHE_LIST_TTL_SECONDS"))
        );
        assert_eq!(
            with(&[("COURSE_CACHE_DETAIL_TTL_SECONDS", "99999999")]),
            Some(ConfigError::InvalidValue {
                key: "COURSE_CACHE_DETAIL_TTL_SECONDS",
                value: "99999999".to_owned()
            })
        );
        assert_eq!(
            with(&[("COURSE_CACHE_LIST_TTL_SECONDS", "301")]),
            Some(ConfigError::InvalidCourseCacheTtls)
        );
    }

    #[test]
    fn rejects_a_missing_required_value() {
        let error = settings(&[
            ("REDIS_URL", "rediss://localhost:6379"),
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
    fn rejects_database_tls_options_in_the_url() {
        let mut values = required_values();
        values[0] = (
            "DATABASE_URL",
            "mysql://app:secret@localhost/app?ssl-mode=disabled",
        );

        assert_eq!(
            settings(&values).err().unwrap(),
            ConfigError::DatabaseTlsInUrl
        );
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
    fn loads_explicit_database_tls_and_timeouts() {
        let mut values = required_values();
        values.extend([
            ("DATABASE_TLS_MODE", "required"),
            ("DATABASE_CONNECT_TIMEOUT_MS", "100"),
            ("DATABASE_ACQUIRE_TIMEOUT_MS", "200"),
            ("DATABASE_QUERY_TIMEOUT_MS", "300"),
        ]);

        let loaded = settings(&values).unwrap();

        assert_eq!(loaded.database.tls_mode, DatabaseTlsMode::Required);
        assert_eq!(loaded.database.connect_timeout, Duration::from_millis(100));
        assert_eq!(loaded.database.acquire_timeout, Duration::from_millis(200));
        assert_eq!(loaded.database.query_timeout, Duration::from_millis(300));
    }

    #[test]
    fn rejects_invalid_database_timeout_and_tls_settings() {
        for key in [
            "DATABASE_CONNECT_TIMEOUT_MS",
            "DATABASE_ACQUIRE_TIMEOUT_MS",
            "DATABASE_QUERY_TIMEOUT_MS",
        ] {
            let mut values = required_values();
            values.push((key, "0"));
            assert_eq!(
                settings(&values).err().unwrap(),
                ConfigError::ZeroDuration(key)
            );
        }

        let mut values = required_values();
        values.push(("DATABASE_TLS_MODE", "sometimes"));
        assert!(matches!(
            settings(&values),
            Err(ConfigError::InvalidValue {
                key: "DATABASE_TLS_MODE",
                ..
            })
        ));

        let mut values = required_values();
        values.extend([
            ("DATABASE_TLS_MODE", "disabled"),
            ("DATABASE_TLS_CA_PATH", "unused.pem"),
        ]);
        assert_eq!(
            settings(&values).err().unwrap(),
            ConfigError::TlsCaRequiresVerification
        );
    }

    #[test]
    fn loads_redis_defaults_and_explicit_values() {
        let loaded = settings(&required_values()).unwrap();
        assert_eq!(loaded.redis.tls_mode, RedisTlsMode::Verify);
        assert_eq!(loaded.redis.connect_timeout, Duration::from_secs(2));
        assert_eq!(loaded.redis.command_timeout, Duration::from_secs(1));

        let mut values = required_values();
        values[1] = ("REDIS_URL", "redis://localhost:6379");
        values.extend([
            ("REDIS_TLS_MODE", "disabled"),
            ("REDIS_CONNECT_TIMEOUT_MS", "100"),
            ("REDIS_COMMAND_TIMEOUT_MS", "200"),
        ]);
        let loaded = settings(&values).unwrap();
        assert_eq!(loaded.redis.tls_mode, RedisTlsMode::Disabled);
        assert_eq!(loaded.redis.connect_timeout, Duration::from_millis(100));
        assert_eq!(loaded.redis.command_timeout, Duration::from_millis(200));
    }

    #[test]
    fn rejects_invalid_redis_settings() {
        for key in ["REDIS_CONNECT_TIMEOUT_MS", "REDIS_COMMAND_TIMEOUT_MS"] {
            let mut values = required_values();
            values.push((key, "0"));
            assert_eq!(
                settings(&values).err().unwrap(),
                ConfigError::ZeroDuration(key)
            );
        }

        let mut values = required_values();
        values.push(("REDIS_TLS_MODE", "sometimes"));
        assert!(matches!(
            settings(&values),
            Err(ConfigError::InvalidValue {
                key: "REDIS_TLS_MODE",
                ..
            })
        ));

        // TLS mode defaults to verify, which needs a rediss:// URL.
        let mut values = required_values();
        values[1] = ("REDIS_URL", "redis://localhost:6379");
        assert_eq!(
            settings(&values).err().unwrap(),
            ConfigError::RedisTlsSchemeMismatch
        );

        // A rediss:// URL cannot be combined with disabled TLS.
        let mut values = required_values();
        values.push(("REDIS_TLS_MODE", "disabled"));
        assert_eq!(
            settings(&values).err().unwrap(),
            ConfigError::RedisTlsSchemeMismatch
        );

        let mut values = required_values();
        values.extend([
            ("REDIS_TLS_MODE", "required"),
            ("REDIS_TLS_CA_PATH", "unused.pem"),
        ]);
        assert_eq!(
            settings(&values).err().unwrap(),
            ConfigError::RedisTlsCaRequiresVerify
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
