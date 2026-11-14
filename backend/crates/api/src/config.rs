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
    /// Maximum time in-flight requests may drain after a shutdown signal.
    pub shutdown_timeout: Duration,
    /// [`tracing_subscriber::EnvFilter`] expression used by application telemetry.
    pub log_filter: String,
    /// Deployment environment, which selects log format and is attached to log events.
    pub environment: Environment,
    /// Service name attached to every log event.
    pub service_name: String,
    /// HTTP hardening settings: CORS, body limits and security headers.
    pub http: HttpSettings,
    /// Metrics listener settings.
    pub metrics: MetricsSettings,
}

#[derive(Clone, PartialEq, Eq)]
/// Settings for the dedicated Prometheus metrics listener.
///
/// Its `Debug` output redacts the scrape token.
pub struct MetricsSettings {
    /// Whether the metrics listener is started.
    pub enabled: bool,
    /// Address of the metrics listener, separate from the public API address.
    pub address: SocketAddr,
    /// Bearer token scrapers must present. Required for non-loopback addresses.
    pub bearer_token: Option<String>,
}

impl std::fmt::Debug for MetricsSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsSettings")
            .field("enabled", &self.enabled)
            .field("address", &self.address)
            .field(
                "bearer_token",
                &self.bearer_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl Default for MetricsSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            address: SocketAddr::from(([127, 0, 0, 1], 9090)),
            bearer_token: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Deployment environment of the running process.
pub enum Environment {
    /// Local development; logs are human-readable.
    Development,
    /// Pre-production deployment; logs are JSON.
    Staging,
    /// Production deployment; logs are JSON.
    Production,
}

impl Environment {
    /// Returns the lowercase name attached to log events.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }
}

impl FromStr for Environment {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "development" => Ok(Self::Development),
            "staging" => Ok(Self::Staging),
            "production" => Ok(Self::Production),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// HTTP-layer hardening settings.
pub struct HttpSettings {
    /// Whether the public deployment is served over HTTPS (for example behind a
    /// TLS-terminating proxy). `Strict-Transport-Security` is sent only when `true`.
    pub https_enabled: bool,
    /// `max-age` advertised in `Strict-Transport-Security`.
    pub hsts_max_age: Duration,
    /// Maximum accepted request body size in bytes.
    pub max_body_bytes: usize,
    /// Cross-origin resource sharing policy.
    pub cors: CorsSettings,
}

impl Default for HttpSettings {
    fn default() -> Self {
        Self {
            https_enabled: false,
            hsts_max_age: Duration::from_secs(31_536_000),
            max_body_bytes: 1024 * 1024,
            cors: CorsSettings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Allowlist-based CORS policy. An empty origin list disables cross-origin access.
pub struct CorsSettings {
    /// Exact frontend origins (`scheme://host[:port]`) allowed to call the API.
    pub allowed_origins: Vec<String>,
    /// HTTP methods permitted on cross-origin requests.
    pub allowed_methods: Vec<String>,
    /// Request headers permitted on cross-origin requests.
    pub allowed_headers: Vec<String>,
    /// Whether browsers may send credentials. Never combined with a wildcard.
    pub allow_credentials: bool,
    /// How long browsers may cache a preflight response.
    pub max_age: Duration,
}

impl Default for CorsSettings {
    fn default() -> Self {
        Self {
            allowed_origins: Vec::new(),
            allowed_methods: ["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]
                .map(str::to_owned)
                .to_vec(),
            allowed_headers: ["authorization", "content-type", "x-request-id"]
                .map(str::to_owned)
                .to_vec(),
            allow_credentials: false,
            max_age: Duration::from_secs(600),
        }
    }
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

    /// Builds valid settings from fixed local values, for tests in other modules.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self::from_source(|key| match key {
            "DATABASE_URL" => Some("mysql://app:secret@localhost/app".to_owned()),
            "REDIS_URL" => Some("rediss://localhost:6379".to_owned()),
            "OPENSEARCH_URL" => Some("http://localhost:9200".to_owned()),
            _ => None,
        })
        .expect("fixed test settings are valid")
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
        let shutdown_timeout = duration(&get, "SHUTDOWN_TIMEOUT_MS", "10000")?;
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
            shutdown_timeout,
            log_filter: optional(&get, "RUST_LOG", "backend_api=info,tower_http=info"),
            environment: parse(&get, "APP_ENV", "production")?,
            service_name: optional(&get, "SERVICE_NAME", "school-api"),
            http: http_settings(&get)?,
            metrics: metrics_settings(&get)?,
        };

        let server_address = settings.server_address()?;
        if settings.metrics.enabled && settings.metrics.address == server_address {
            return Err(ConfigError::MetricsAddressConflict);
        }
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

/// Loads and validates the metrics listener settings.
///
/// A non-loopback listener must be protected by `METRICS_BEARER_TOKEN`.
fn metrics_settings(get: &impl Fn(&str) -> Option<String>) -> Result<MetricsSettings, ConfigError> {
    let enabled = parse_bool(get, "METRICS_ENABLED", "true")?;
    let address: SocketAddr = parse(get, "METRICS_ADDR", "127.0.0.1:9090")?;
    let bearer_token = non_empty(get("METRICS_BEARER_TOKEN"));
    if enabled && !address.ip().is_loopback() && bearer_token.is_none() {
        return Err(ConfigError::MetricsTokenRequired);
    }
    Ok(MetricsSettings {
        enabled,
        address,
        bearer_token,
    })
}

/// Loads and validates HTTP hardening settings (CORS, body limit, HSTS).
fn http_settings(get: &impl Fn(&str) -> Option<String>) -> Result<HttpSettings, ConfigError> {
    let https_enabled = parse_bool(get, "HTTP_HTTPS_ENABLED", "false")?;
    let hsts_seconds: u64 = parse(get, "HTTP_HSTS_MAX_AGE_SECS", "31536000")?;
    let max_body_bytes: usize = parse(get, "HTTP_MAX_BODY_BYTES", "1048576")?;
    if max_body_bytes == 0 {
        return Err(ConfigError::ZeroDuration("HTTP_MAX_BODY_BYTES"));
    }

    let defaults = CorsSettings::default();
    let allowed_origins = csv(get("CORS_ALLOWED_ORIGINS"))
        .into_iter()
        .map(|origin| normalize_origin(&origin))
        .collect::<Result<Vec<_>, _>>()?;
    let allowed_methods = match non_empty(get("CORS_ALLOWED_METHODS")) {
        Some(value) => csv(Some(value))
            .into_iter()
            .map(|method| {
                let method = method.to_ascii_uppercase();
                axum::http::Method::from_bytes(method.as_bytes())
                    .ok()
                    .filter(|_| method != "*")
                    .map(|_| method.clone())
                    .ok_or(())
                    .map_err(|_| ConfigError::InvalidValue {
                        key: "CORS_ALLOWED_METHODS",
                        value: method,
                    })
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => defaults.allowed_methods,
    };
    let allowed_headers = match non_empty(get("CORS_ALLOWED_HEADERS")) {
        Some(value) => csv(Some(value))
            .into_iter()
            .map(|header| {
                axum::http::HeaderName::from_bytes(header.as_bytes())
                    .ok()
                    .filter(|_| header != "*")
                    .map(|name| name.as_str().to_owned())
                    .ok_or(())
                    .map_err(|_| ConfigError::InvalidValue {
                        key: "CORS_ALLOWED_HEADERS",
                        value: header,
                    })
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => defaults.allowed_headers,
    };
    let max_age_seconds: u64 = parse(get, "CORS_MAX_AGE_SECS", "600")?;

    Ok(HttpSettings {
        https_enabled,
        hsts_max_age: Duration::from_secs(hsts_seconds),
        max_body_bytes,
        cors: CorsSettings {
            allowed_origins,
            allowed_methods,
            allowed_headers,
            allow_credentials: parse_bool(get, "CORS_ALLOW_CREDENTIALS", "false")?,
            max_age: Duration::from_secs(max_age_seconds),
        },
    })
}

/// Parses a `true`/`false` flag from a configuration source.
fn parse_bool(
    get: &impl Fn(&str) -> Option<String>,
    key: &'static str,
    default: &str,
) -> Result<bool, ConfigError> {
    let value = optional(get, key, default);
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(ConfigError::InvalidValue { key, value }),
    }
}

/// Splits a comma-separated value into trimmed, non-empty items.
fn csv(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Validates a CORS origin and returns its canonical `scheme://host[:port]` form.
///
/// Wildcards, paths, queries, fragments and credentials are rejected so the allowlist
/// can only ever contain exact origins.
fn normalize_origin(value: &str) -> Result<String, ConfigError> {
    let invalid = || ConfigError::InvalidCorsOrigin(value.to_owned());
    let url = Url::parse(value).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid());
    }
    Ok(url.origin().ascii_serialization())
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
    /// A CORS origin was not an exact `http(s)://host[:port]` origin (wildcards are never allowed).
    #[error("CORS_ALLOWED_ORIGINS contains an invalid origin: {0:?}")]
    InvalidCorsOrigin(String),
    /// A metrics listener reachable beyond loopback was configured without a token.
    #[error("METRICS_BEARER_TOKEN is required when METRICS_ADDR is not a loopback address")]
    MetricsTokenRequired,
    /// The metrics listener was configured on the public API address.
    #[error("METRICS_ADDR must differ from the public API address")]
    MetricsAddressConflict,
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
        assert_eq!(loaded.shutdown_timeout, Duration::from_secs(10));
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
    fn validates_shutdown_timeout() {
        let with = |value: &'static str| {
            let mut values = required_values();
            values.push(("SHUTDOWN_TIMEOUT_MS", value));
            settings(&values)
        };

        assert_eq!(
            with("2500").unwrap().shutdown_timeout,
            Duration::from_millis(2500)
        );
        assert_eq!(
            with("0").err(),
            Some(ConfigError::ZeroDuration("SHUTDOWN_TIMEOUT_MS"))
        );
        assert!(with("soon").is_err());
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
    fn loads_secure_http_defaults() {
        let loaded = settings(&required_values()).unwrap();

        assert_eq!(loaded.environment, Environment::Production);
        assert_eq!(loaded.service_name, "school-api");
        assert_eq!(loaded.http, HttpSettings::default());
        assert!(loaded.http.cors.allowed_origins.is_empty());
        assert!(!loaded.http.cors.allow_credentials);
        assert!(!loaded.http.https_enabled);
    }

    #[test]
    fn loads_and_normalizes_cors_settings() {
        let mut values = required_values();
        values.extend([
            ("APP_ENV", "Development"),
            (
                "CORS_ALLOWED_ORIGINS",
                "http://localhost:3000/, https://app.example.com",
            ),
            ("CORS_ALLOWED_METHODS", "get,post"),
            ("CORS_ALLOWED_HEADERS", "Content-Type"),
            ("CORS_ALLOW_CREDENTIALS", "true"),
            ("HTTP_HTTPS_ENABLED", "true"),
            ("HTTP_MAX_BODY_BYTES", "2048"),
        ]);

        let loaded = settings(&values).unwrap();

        assert_eq!(loaded.environment, Environment::Development);
        assert_eq!(
            loaded.http.cors.allowed_origins,
            ["http://localhost:3000", "https://app.example.com"]
        );
        assert_eq!(loaded.http.cors.allowed_methods, ["GET", "POST"]);
        assert_eq!(loaded.http.cors.allowed_headers, ["content-type"]);
        assert!(loaded.http.cors.allow_credentials);
        assert!(loaded.http.https_enabled);
        assert_eq!(loaded.http.max_body_bytes, 2048);
    }

    #[test]
    fn rejects_invalid_http_settings() {
        let with = |extra: &[(&'static str, &'static str)]| {
            let mut values = required_values();
            values.extend_from_slice(extra);
            settings(&values).err()
        };

        for origin in ["*", "https://app.example.com/path", "ftp://x.test", "nope"] {
            assert_eq!(
                with(&[("CORS_ALLOWED_ORIGINS", origin)]),
                Some(ConfigError::InvalidCorsOrigin(origin.to_owned()))
            );
        }
        assert_eq!(
            with(&[("HTTP_MAX_BODY_BYTES", "0")]),
            Some(ConfigError::ZeroDuration("HTTP_MAX_BODY_BYTES"))
        );
        assert!(matches!(
            with(&[("APP_ENV", "qa")]),
            Some(ConfigError::InvalidValue { key: "APP_ENV", .. })
        ));
        assert!(matches!(
            with(&[("CORS_ALLOWED_HEADERS", "*")]),
            Some(ConfigError::InvalidValue {
                key: "CORS_ALLOWED_HEADERS",
                ..
            })
        ));
        assert!(matches!(
            with(&[("CORS_ALLOW_CREDENTIALS", "maybe")]),
            Some(ConfigError::InvalidValue {
                key: "CORS_ALLOW_CREDENTIALS",
                ..
            })
        ));
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

    #[test]
    fn loads_metrics_defaults_and_validates_exposure() {
        let loaded = settings(&required_values()).unwrap();
        assert_eq!(loaded.metrics, MetricsSettings::default());

        let with = |extra: &[(&'static str, &'static str)]| {
            let mut values = required_values();
            values.extend_from_slice(extra);
            settings(&values)
        };
        assert_eq!(
            with(&[("METRICS_ADDR", "0.0.0.0:9090")]).err(),
            Some(ConfigError::MetricsTokenRequired)
        );
        let exposed = with(&[
            ("METRICS_ADDR", "0.0.0.0:9090"),
            ("METRICS_BEARER_TOKEN", "token"),
        ])
        .unwrap();
        assert_eq!(exposed.metrics.bearer_token.as_deref(), Some("token"));
        assert!(
            with(&[
                ("METRICS_ADDR", "0.0.0.0:9090"),
                ("METRICS_ENABLED", "false")
            ])
            .is_ok()
        );
        assert_eq!(
            with(&[
                ("APP_HOST", "127.0.0.1"),
                ("METRICS_ADDR", "127.0.0.1:3000")
            ])
            .err(),
            Some(ConfigError::MetricsAddressConflict)
        );
        assert!(matches!(
            with(&[("METRICS_ADDR", "nope")]).err(),
            Some(ConfigError::InvalidValue {
                key: "METRICS_ADDR",
                ..
            })
        ));
    }
}
