//! Operating-system shutdown signal handling and the bounded drain window.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use tokio::sync::Notify;

/// Boxed shutdown trigger handed to the server's graceful-shutdown hook.
pub type ShutdownTrigger = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Waits for Ctrl+C on every platform or SIGTERM on Unix.
///
/// The returned future completes once one supported signal is received, allowing
/// Axum to stop accepting new connections and finish in-flight requests. A
/// handler that cannot be installed is logged and never fires, so the other
/// signal still works.
pub async fn signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl+C handler");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    tracing::info!("shutdown signal received");
}

/// Runs a server until it finishes, bounding how long it may drain after `signal`.
///
/// `serve` receives the trigger to use as its graceful-shutdown future. Once
/// `signal` completes, the server stops accepting connections and may drain
/// in-flight requests for at most `timeout`. Returns `Some` with the server's
/// result when it finished in time, or `None` when the window elapsed and the
/// server future was dropped, which closes the remaining connections.
pub async fn serve_with_deadline<T, Fut>(
    signal: impl Future<Output = ()> + Send + 'static,
    timeout: Duration,
    serve: impl FnOnce(ShutdownTrigger) -> Fut,
) -> Option<T>
where
    Fut: Future<Output = T>,
{
    let notify = Arc::new(Notify::new());
    let trigger = Arc::clone(&notify);
    let graceful: ShutdownTrigger = Box::pin(async move {
        signal.await;
        trigger.notify_one();
    });

    let deadline = async {
        notify.notified().await;
        tokio::time::sleep(timeout).await;
    };

    tokio::select! {
        result = serve(graceful) => Some(result),
        () = deadline => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_result_when_server_drains_in_time() {
        let outcome = serve_with_deadline(
            std::future::ready(()),
            Duration::from_secs(5),
            |trigger| async move {
                trigger.await;
                42
            },
        )
        .await;

        assert_eq!(outcome, Some(42));
    }

    #[tokio::test]
    async fn gives_up_when_server_never_drains() {
        let outcome = serve_with_deadline(
            std::future::ready(()),
            Duration::from_millis(20),
            |trigger| async move {
                trigger.await;
                std::future::pending::<()>().await;
            },
        )
        .await;

        assert_eq!(outcome, None);
    }

    #[tokio::test]
    async fn deadline_does_not_start_before_signal() {
        let outcome = serve_with_deadline(
            std::future::pending::<()>(),
            Duration::from_millis(10),
            |_trigger| async {
                tokio::time::sleep(Duration::from_millis(60)).await;
                "done"
            },
        )
        .await;

        assert_eq!(outcome, Some("done"));
    }
}
