#![forbid(unsafe_code)]

//! The proxy data path: accept loop, HTTP forwarding (absolute-form →
//! origin-form), and `CONNECT` tunneling, with per-connection isolation and
//! graceful shutdown. M2 implements direct mode against fake origins.

mod counting;
mod error;
mod http_forward;
mod metrics;
mod server;
mod tracker;
mod tunnel;
mod upstream;

pub use error::ProxyError;
pub use metrics::ProxyMetrics;
pub use server::{ProxyServer, RoutingHandle};
pub use tracker::{ConnectionKind, ConnectionSnapshot, ConnectionState};
pub use upstream::{AuthFactory, PacRouter, RouteChoice, Routing, UpstreamAuth, UpstreamTarget};
