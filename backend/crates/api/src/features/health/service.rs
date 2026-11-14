//! Readiness service and external dependency probes.
//!
//! All probes run concurrently and are bounded independently so one slow datastore
//! cannot delay the response beyond the configured timeout.

use std::{
    fmt::Display,
    future::Future,
    time::{Duration, Instant},
};

use async_trait::async_trait;

use super::models::{DependencyStatuses, ReadinessReport};
use crate::{
    infrastructure::{DatabaseClient, DatabaseError, Datastores},
    metrics::Metrics,
};

#[derive(Clone)]
/// Health service that probes MySQL, Redis, and OpenSearch.
pub(crate) struct DatastoreHealthService {
    datastores: Datastores,
    timeout: Duration,
    metrics: Option<Metrics>,
}

impl DatastoreHealthService {
    /// Creates a health service using shared datastore clients and a per-probe timeout.
    pub fn new(datastores: Datastores, timeout: Duration) -> Self {
        Self {
            datastores,
            timeout,
            metrics: None,
        }
    }

    /// Records probe duration and failures into `metrics`.
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Pings the lazy SeaORM MySQL pool.
    async fn mysql(&self, database: &DatabaseClient) -> bool {
        let ping = async {
            let result: Result<(), DatabaseError> = database.ping().await;
            result
        };
        bounded("mysql", self.timeout, self.metrics.as_ref(), ping).await
    }

    /// Sends a Redis `PING` through the reconnecting connection manager.
    async fn redis(&self) -> bool {
        bounded(
            "redis",
            self.timeout,
            self.metrics.as_ref(),
            self.datastores.redis.ping(),
        )
        .await
    }

    /// Sends an OpenSearch ping request and accepts only successful HTTP responses.
    async fn opensearch(&self) -> bool {
        let ping = async {
            let response = self
                .datastores
                .opensearch
                .ping()
                .send()
                .await
                .map_err(|error| error.to_string())?;
            if response.status_code().is_success() {
                Ok(())
            } else {
                Err(format!("unexpected HTTP status {}", response.status_code()))
            }
        };
        bounded("opensearch", self.timeout, self.metrics.as_ref(), ping).await
    }
}

#[async_trait]
impl HealthService for DatastoreHealthService {
    async fn check(&self, database: &DatabaseClient) -> ReadinessReport {
        let (mysql, redis, opensearch) =
            tokio::join!(self.mysql(database), self.redis(), self.opensearch());
        ReadinessReport {
            dependencies: DependencyStatuses {
                mysql: mysql.into(),
                redis: redis.into(),
                opensearch: opensearch.into(),
            },
        }
    }
}

#[async_trait]
/// Contract used by health handlers to obtain current dependency readiness.
pub(crate) trait HealthService: Send + Sync {
    /// Checks every required dependency and returns a sanitized readiness report.
    async fn check(&self, database: &DatabaseClient) -> ReadinessReport;
}

/// Runs one dependency check under a timeout, logs operational failure details and
/// records the probe's duration and outcome when `metrics` is provided.
async fn bounded<T, E>(
    service: &'static str,
    timeout: Duration,
    metrics: Option<&Metrics>,
    check: impl Future<Output = Result<T, E>>,
) -> bool
where
    E: Display,
{
    let started = Instant::now();
    let healthy = probe(service, timeout, check).await;
    if let Some(metrics) = metrics {
        metrics.record_probe(service, started.elapsed().as_secs_f64(), healthy);
    }
    healthy
}

/// Runs the check under the timeout and logs failures.
async fn probe<T, E>(
    service: &'static str,
    timeout: Duration,
    check: impl Future<Output = Result<T, E>>,
) -> bool
where
    E: Display,
{
    match tokio::time::timeout(timeout, check).await {
        Ok(Ok(_)) => true,
        Ok(Err(error)) => {
            tracing::warn!(service, %error, "dependency health check failed");
            false
        }
        Err(_) => {
            tracing::warn!(
                service,
                timeout_ms = timeout.as_millis(),
                "dependency health check timed out"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounded_check_reports_timeout_as_down() {
        let check = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, &'static str>(())
        };

        assert!(!bounded("test", Duration::from_millis(1), None, check).await);
    }

    #[tokio::test]
    async fn bounded_check_accepts_success_and_rejects_errors() {
        assert!(
            bounded("test", Duration::from_secs(1), None, async {
                Ok::<_, &'static str>(())
            })
            .await
        );
        assert!(
            !bounded("test", Duration::from_secs(1), None, async {
                Err::<(), _>("failed")
            })
            .await
        );
    }
}
