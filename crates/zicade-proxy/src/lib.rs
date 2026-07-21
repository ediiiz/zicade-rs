#![forbid(unsafe_code)]

//! The proxy data path: accept loop, HTTP forwarding (absolute-form →
//! origin-form), and `CONNECT` tunneling, with per-connection isolation and
//! graceful shutdown. M2 implements direct mode against fake origins.

mod error;
mod http_forward;
mod metrics;
mod server;
mod tunnel;

pub use error::ProxyError;
pub use metrics::ProxyMetrics;
pub use server::ProxyServer;
