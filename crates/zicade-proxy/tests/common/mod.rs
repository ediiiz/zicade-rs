//! Shared test harness for the direct-mode proxy integration tests:
//! in-process fake origins, a raw proxy client, and a spawned proxy handle.
#![allow(dead_code)]

pub mod client;
pub mod origin;
pub mod upstream_fake;

use std::net::SocketAddr;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use zicade_proxy::{ProxyError, ProxyMetrics, ProxyServer, Routing};

/// A proxy running on an ephemeral loopback port, with a handle to shut it down
/// and inspect its live metrics.
pub struct TestProxy {
    pub addr: SocketAddr,
    pub metrics: ProxyMetrics,
    shutdown: Option<oneshot::Sender<()>>,
    join: JoinHandle<Result<(), ProxyError>>,
}

impl TestProxy {
    /// Bind on 127.0.0.1:0 and start serving. Uses a short shutdown timeout so
    /// the graceful-shutdown test stays fast.
    pub async fn spawn() -> Self {
        Self::spawn_with_timeout(Duration::from_millis(750)).await
    }

    pub async fn spawn_with_timeout(shutdown_timeout: Duration) -> Self {
        Self::start(
            ProxyServer::bind("127.0.0.1:0".parse().unwrap())
                .await
                .expect("bind ephemeral port")
                .with_shutdown_timeout(shutdown_timeout),
        )
    }

    /// Bind on an ephemeral port and serve in the given routing mode (direct or
    /// through a configured upstream proxy).
    pub async fn spawn_with_routing(routing: Routing) -> Self {
        Self::start(
            ProxyServer::bind("127.0.0.1:0".parse().unwrap())
                .await
                .expect("bind ephemeral port")
                .with_shutdown_timeout(Duration::from_millis(750))
                .with_routing(routing),
        )
    }

    fn start(server: ProxyServer) -> Self {
        let addr = server.local_addr();
        let metrics = server.metrics();
        let (tx, rx) = oneshot::channel::<()>();
        let join = tokio::spawn(server.serve(async move {
            let _ = rx.await;
        }));
        Self {
            addr,
            metrics,
            shutdown: Some(tx),
            join,
        }
    }

    /// Trigger graceful shutdown and await the serve loop, returning its result.
    pub async fn shutdown(mut self) -> Result<(), ProxyError> {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.join.await.expect("serve task should not panic")
    }
}

/// Poll until active connections reach zero or the timeout elapses (connection
/// teardown is asynchronous, so a brief settle window is expected).
pub async fn wait_for_active_zero(metrics: &ProxyMetrics, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while metrics.active_connections() != 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
