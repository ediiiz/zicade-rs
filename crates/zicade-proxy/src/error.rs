//! Typed proxy errors.

use std::net::SocketAddr;

/// Errors from binding or running the proxy.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    /// Failed to bind the listen socket (e.g. the port is already in use).
    #[error("failed to bind proxy listener on {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },

    /// A general I/O error while serving.
    #[error("proxy I/O error: {0}")]
    Io(#[from] std::io::Error),
}
