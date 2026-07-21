//! The proxy accept loop, connection tracking, and graceful shutdown.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::task::JoinSet;

use crate::error::ProxyError;
use crate::http_forward::handle_connection;
use crate::metrics::ProxyMetrics;
use crate::upstream::Routing;

/// Default bound on how long graceful shutdown waits for in-flight connections
/// to drain before aborting stragglers.
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// A bound proxy listener, ready to serve in direct mode.
#[derive(Debug)]
pub struct ProxyServer {
    listener: TcpListener,
    local_addr: SocketAddr,
    metrics: ProxyMetrics,
    shutdown_timeout: Duration,
    routing: Routing,
}

impl ProxyServer {
    /// Bind the proxy listener. Fails fast with [`ProxyError::Bind`] if the
    /// address/port is unavailable (LESSON-5) rather than panicking.
    pub async fn bind(addr: SocketAddr) -> Result<Self, ProxyError> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|source| ProxyError::Bind { addr, source })?;
        let local_addr = listener.local_addr()?;
        Ok(Self {
            listener,
            local_addr,
            metrics: ProxyMetrics::default(),
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            routing: Routing::Direct,
        })
    }

    /// Override the graceful-shutdown drain timeout.
    #[must_use]
    pub fn with_shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    /// Select the routing mode (direct, or through a configured upstream proxy).
    /// Defaults to [`Routing::Direct`], preserving M2 behavior.
    #[must_use]
    pub fn with_routing(mut self, routing: Routing) -> Self {
        self.routing = routing;
        self
    }

    /// The actual bound address (useful when binding to port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// A cloneable handle to the live metrics.
    pub fn metrics(&self) -> ProxyMetrics {
        self.metrics.clone()
    }

    /// Run the accept loop until `shutdown` resolves, then drain in-flight
    /// connections within the configured timeout and abort any stragglers.
    ///
    /// The accept future is cancel-safe, so losing the `select!` race on
    /// shutdown never panics (LESSON-2).
    pub async fn serve(self, shutdown: impl Future<Output = ()> + Send) -> Result<(), ProxyError> {
        tokio::pin!(shutdown);
        let mut conns = JoinSet::new();

        loop {
            tokio::select! {
                biased;
                () = &mut shutdown => break,
                accepted = self.listener.accept() => {
                    if let Ok((stream, _peer)) = accepted {
                        let metrics = self.metrics.clone();
                        let routing = self.routing.clone();
                        conns.spawn(handle_connection(stream, metrics, routing));
                    }
                    // Transient accept errors are ignored; the loop continues.
                }
                // Reap finished connections so the set does not grow unbounded.
                Some(_) = conns.join_next(), if !conns.is_empty() => {}
            }
        }

        drain(&mut conns, self.shutdown_timeout).await;
        Ok(())
    }
}

async fn drain(conns: &mut JoinSet<()>, timeout: Duration) {
    let wait_all = async { while conns.join_next().await.is_some() {} };
    if tokio::time::timeout(timeout, wait_all).await.is_err() {
        conns.abort_all();
        while conns.join_next().await.is_some() {}
    }
}
