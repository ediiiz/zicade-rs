#![forbid(unsafe_code)]

//! Zicade application wiring (library form so it is testable without a global
//! tracing subscriber). RED stub: the real implementation lands in the green
//! commit.

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;

use zicade_config::Config;
use zicade_observe::LogStore;
use zicade_proxy::Routing;

/// A started application: bound proxy + web listeners ready to serve.
pub struct App {
    _private: (),
}

impl App {
    /// Validate the config, build routing, and bind both listeners.
    pub async fn start(
        _config: Config,
        _config_path: PathBuf,
        _logs: LogStore,
    ) -> anyhow::Result<Self> {
        todo!()
    }

    /// The bound proxy listen address.
    pub fn proxy_addr(&self) -> SocketAddr {
        todo!()
    }

    /// The bound web (UI/API) listen address.
    pub fn web_addr(&self) -> SocketAddr {
        todo!()
    }

    /// Serve both servers until `shutdown` resolves, then drain.
    pub async fn run(self, _shutdown: impl Future<Output = ()> + Send) -> anyhow::Result<()> {
        todo!()
    }
}

/// Map a [`Config`] to the proxy's [`Routing`].
pub fn build_routing(_config: &Config) -> anyhow::Result<Routing> {
    todo!()
}

/// Start the app and serve until `shutdown` resolves.
pub async fn run(
    _config: Config,
    _config_path: PathBuf,
    _logs: LogStore,
    _shutdown: impl Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    todo!()
}
