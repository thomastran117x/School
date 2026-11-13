use std::{fmt::Display, future::Future, time::Duration};

use async_trait::async_trait;

use super::models::{DependencyStatuses, ReadinessReport};
use crate::infrastructure::Datastores;

#[derive(Clone)]
pub(crate) struct DatastoreHealthService {
    datastores: Datastores,
    timeout: Duration,
}

impl DatastoreHealthService {
    pub fn new(datastores: Datastores, timeout: Duration) -> Self {
        Self {
            datastores,
            timeout,
        }
    }

    async fn mysql(&self) -> bool {
        bounded("mysql", self.timeout, self.datastores.mysql.ping()).await
    }

    async fn redis(&self) -> bool {
        let mut connection = self.datastores.redis.clone();
        let ping = async move {
            let response: String = redis::cmd("PING").query_async(&mut connection).await?;
            Ok::<_, redis::RedisError>(response)
        };
        bounded("redis", self.timeout, ping).await
    }

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
        bounded("opensearch", self.timeout, ping).await
    }
}

#[async_trait]
impl HealthService for DatastoreHealthService {
    async fn check(&self) -> ReadinessReport {
        let (mysql, redis, opensearch) =
            tokio::join!(self.mysql(), self.redis(), self.opensearch());
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
pub(crate) trait HealthService: Send + Sync {
    async fn check(&self) -> ReadinessReport;
}

async fn bounded<T, E>(
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

        assert!(!bounded("test", Duration::from_millis(1), check).await);
    }

    #[tokio::test]
    async fn bounded_check_accepts_success_and_rejects_errors() {
        assert!(
            bounded("test", Duration::from_secs(1), async {
                Ok::<_, &'static str>(())
            })
            .await
        );
        assert!(
            !bounded("test", Duration::from_secs(1), async {
                Err::<(), _>("failed")
            })
            .await
        );
    }
}
