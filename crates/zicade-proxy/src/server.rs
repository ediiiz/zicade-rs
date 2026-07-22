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

/// A cloneable, thread-safe handle to the proxy's live [`Routing`].
///
/// The proxy clones the *current* routing per accepted connection, so swapping
/// the value behind this handle takes effect on the next connection without a
/// restart. This is the seam the web layer uses to apply routing edits live.
#[derive(Clone, Debug)]
pub struct RoutingHandle(std::sync::Arc<std::sync::Mutex<Routing>>);

impl RoutingHandle {
    /// Wrap an initial routing in a fresh shared handle.
    pub fn new(routing: Routing) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(routing)))
    }

    /// A clone of the current routing (falls back to [`Routing::Direct`] if the
    /// lock is poisoned rather than panicking).
    pub fn current(&self) -> Routing {
        self.0.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Atomically replace the routing; observed by the next accepted connection.
    pub fn set(&self, routing: Routing) {
        if let Ok(mut g) = self.0.lock() {
            *g = routing;
        }
    }
}

/// A bound proxy listener, ready to serve in direct mode.
#[derive(Debug)]
pub struct ProxyServer {
    listener: TcpListener,
    local_addr: SocketAddr,
    metrics: ProxyMetrics,
    shutdown_timeout: Duration,
    routing: RoutingHandle,
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
            routing: RoutingHandle::new(Routing::Direct),
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
    pub fn with_routing(self, routing: Routing) -> Self {
        self.routing.set(routing);
        self
    }

    /// A cloneable handle to the live routing, for swapping it after `bind`
    /// (e.g. the web layer rebuilds routing on a config apply).
    pub fn routing_handle(&self) -> RoutingHandle {
        self.routing.clone()
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
                        let routing = self.routing.current();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upstream::{UpstreamAuth, UpstreamTarget};

    #[test]
    fn routing_handle_reflects_set() {
        let handle = RoutingHandle::new(Routing::Direct);
        assert_eq!(format!("{:?}", handle.current()), "Direct");

        handle.set(Routing::Upstream(UpstreamTarget {
            addr: "127.0.0.1:8080".to_owned(),
            auth: UpstreamAuth::None,
        }));
        assert_eq!(format!("{:?}", handle.current()), "Upstream");
    }

    #[tokio::test]
    async fn routing_handle_shares_state_with_server() {
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let server = ProxyServer::bind(addr).await.expect("bind");
        let handle = server.routing_handle();

        // The server starts in direct mode.
        assert_eq!(format!("{:?}", server.routing.current()), "Direct");

        // Setting through the shared handle is observed by the server's own
        // handle: they point at the same underlying routing.
        handle.set(Routing::Upstream(UpstreamTarget {
            addr: "127.0.0.1:8080".to_owned(),
            auth: UpstreamAuth::None,
        }));
        assert_eq!(format!("{:?}", server.routing.current()), "Upstream");
    }
}
