//! Structured application logging and tracing setup.
//!
//! - `RUST_LOG` (via `Settings::log_filter`) selects the log level.
//! - Development logs are human-readable; staging and production logs are JSON.
//! - Every event carries the `service` and `environment` context.
//! - Values of sensitive fields (authorization, cookies, tokens, passwords, secrets
//!   and student personal data) are replaced with `[REDACTED]`, matched by field name
//!   at any nesting depth. Redaction is name-based: never interpolate secrets into a
//!   log *message*, and wrap them in a field with a sensitive name instead.
//! - Initialization fails instead of panicking when a subscriber is already installed.

use std::fmt;

use anyhow::Context;
use serde_json::Value;
use tracing::{Event, Subscriber};
use tracing_subscriber::{
    EnvFilter, Layer, Registry,
    field::MakeExt,
    fmt::{
        FmtContext, FormatEvent, FormatFields, MakeWriter,
        format::{Format, JsonFields, Writer, debug_fn},
    },
    layer::{Layered, SubscriberExt},
    registry::LookupSpan,
    util::SubscriberInitExt,
};

use crate::config::{Environment, Settings};

/// Replacement text for redacted values.
const REDACTED: &str = "[REDACTED]";

/// Lowercase substrings that mark a field name as sensitive.
const SENSITIVE_NAME_PARTS: &[&str] = &[
    "authorization",
    "cookie",
    "token",
    "password",
    "passwd",
    "secret",
    "api_key",
    "apikey",
    "credential",
    "email",
    "student",
    "ssn",
    "phone",
    "birth",
];

/// Returns whether a log field with this name must have its value redacted.
pub fn is_sensitive(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SENSITIVE_NAME_PARTS.iter().any(|part| name.contains(part))
}

/// Recursively replaces the values of sensitive keys in a JSON log record.
fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                if is_sensitive(key) {
                    *inner = Value::String(REDACTED.to_owned());
                } else {
                    redact_value(inner);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        _ => {}
    }
}

/// Event formatter that adds service context and redaction around an inner formatter.
struct ContextFormat<E> {
    /// Formatter producing the base record.
    inner: E,
    /// Service name attached to every record.
    service: String,
    /// Environment name attached to every record.
    environment: &'static str,
    /// Whether `inner` produces JSON that must be post-processed.
    json: bool,
}

impl<S, N, E> FormatEvent<S, N> for ContextFormat<E>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'w> FormatFields<'w> + 'static,
    E: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut buffer = String::new();
        self.inner
            .format_event(ctx, Writer::new(&mut buffer), event)?;

        if !self.json {
            return write!(writer, "[{}/{}] {buffer}", self.service, self.environment);
        }

        match serde_json::from_str::<Value>(buffer.trim_end()) {
            Ok(mut record) => {
                redact_value(&mut record);
                if let Value::Object(map) = &mut record {
                    map.insert("service".to_owned(), Value::String(self.service.clone()));
                    map.insert(
                        "environment".to_owned(),
                        Value::String(self.environment.to_owned()),
                    );
                }
                writeln!(writer, "{record}")
            }
            // Never fall back to emitting the unparsed (and therefore unredacted) record.
            Err(_) => writeln!(
                writer,
                r#"{{"level":"ERROR","message":"unserializable log record dropped","service":"{}","environment":"{}"}}"#,
                self.service.escape_default(),
                self.environment
            ),
        }
    }
}

/// Subscriber type the application layer is attached to.
type Base = Layered<EnvFilter, Registry>;

/// Builds the output layer for an environment, writing to `writer`.
fn build_layer<W>(
    environment: Environment,
    service: &str,
    writer: W,
) -> Box<dyn Layer<Base> + Send + Sync + 'static>
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let service = service.to_owned();
    let name = environment.as_str();
    match environment {
        Environment::Development => tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .event_format(ContextFormat {
                inner: Format::default(),
                service,
                environment: name,
                json: false,
            })
            .fmt_fields(
                debug_fn(|out, field, value| {
                    if is_sensitive(field.name()) {
                        write!(out, "{}={REDACTED}", field.name())
                    } else if field.name() == "message" {
                        write!(out, "{value:?}")
                    } else {
                        write!(out, "{}={value:?}", field.name())
                    }
                })
                .delimited(" "),
            )
            .boxed(),
        Environment::Staging | Environment::Production => tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .event_format(ContextFormat {
                inner: Format::default().json(),
                service,
                environment: name,
                json: true,
            })
            .fmt_fields(JsonFields::new())
            .boxed(),
    }
}

/// Installs the process-wide tracing subscriber.
///
/// # Errors
///
/// Returns an error when the log filter is invalid or another subscriber has already
/// been installed in this process.
pub fn init(settings: &Settings) -> anyhow::Result<()> {
    let filter = EnvFilter::try_new(&settings.log_filter).context("invalid RUST_LOG filter")?;
    let layer = build_layer(
        settings.environment,
        &settings.service_name,
        std::io::stdout,
    );

    tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init()
        .context("tracing subscriber was already initialized")
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    use super::*;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn capture(environment: Environment, emit: impl FnOnce()) -> String {
        let sink = Capture::default();
        let writer = sink.clone();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("trace"))
            .with(build_layer(environment, "svc", move || writer.clone()));
        tracing::subscriber::with_default(subscriber, emit);
        let bytes = sink.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn classifies_sensitive_field_names() {
        for name in [
            "authorization",
            "Cookie",
            "set-cookie",
            "access_token",
            "refresh_token",
            "password",
            "client_secret",
            "student_id",
            "student_email",
            "email",
        ] {
            assert!(is_sensitive(name), "{name} should be sensitive");
        }
        for name in ["message", "request_id", "method", "path", "status"] {
            assert!(!is_sensitive(name), "{name} should not be sensitive");
        }
    }

    #[test]
    fn json_output_redacts_events_and_spans_and_adds_context() {
        let output = capture(Environment::Production, || {
            let span = tracing::info_span!("http_request", request_id = "r1", cookie = "sid=abc");
            let _guard = span.enter();
            tracing::info!(
                authorization = "Bearer top-secret",
                password = "hunter2",
                student_email = "kid@school.test",
                user_id = 7,
                "login attempt"
            );
        });

        for secret in ["top-secret", "hunter2", "kid@school.test", "sid=abc"] {
            assert!(!output.contains(secret), "{secret} leaked: {output}");
        }
        let record: Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(record["service"], "svc");
        assert_eq!(record["environment"], "production");
        assert_eq!(record["fields"]["authorization"], REDACTED);
        assert_eq!(record["fields"]["user_id"], 7);
        assert_eq!(record["fields"]["message"], "login attempt");
    }

    #[test]
    fn development_output_is_plaintext_and_redacted() {
        let output = capture(Environment::Development, || {
            tracing::info!(token = "abc123", user_id = 7, "hello");
        });

        assert!(output.starts_with("[svc/development]"));
        assert!(output.contains("hello"));
        assert!(output.contains("user_id=7"));
        assert!(output.contains("token=[REDACTED]"));
        assert!(!output.contains("abc123"));
    }

    #[test]
    fn initializing_twice_returns_an_error_instead_of_panicking() {
        let settings = test_settings();

        // Another test may already have installed the global subscriber; either way a
        // second installation must fail cleanly.
        let _ = init(&settings);
        assert!(init(&settings).is_err());
    }

    #[test]
    fn rejects_an_invalid_filter() {
        let mut settings = test_settings();
        settings.log_filter = "backend_api=nonsense[".to_owned();

        assert!(init(&settings).is_err());
    }

    fn test_settings() -> Settings {
        Settings::for_tests()
    }
}
