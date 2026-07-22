//! The started application: bind the proxy + web listeners, then serve both
//! under a single fan-out shutdown signal.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::Context as _;
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::watch;
use zicade_config::{Config, RoutingMode, ValidationCtx};
use zicade_observe::LogStore;
use zicade_proxy::ProxyServer;
use zicade_web::{AppState, MetricsSource, StatusSnapshot, load_or_create_token, router};

use crate::routing::build_routing;

/// Adapts the proxy's live [`zicade_proxy::ProxyMetrics`] to the web crate's
/// [`MetricsSource`] trait so `GET /api/status` reports real counters.
struct ProxyMetricsSource(zicade_proxy::ProxyMetrics);

impl MetricsSource for ProxyMetricsSource {
    fn total_requests(&self) -> u64 {
        self.0.total_requests()
    }

    fn active_connections(&self) -> usize {
        self.0.active_connections()
    }

    fn failed_requests(&self) -> u64 {
        self.0.failed_requests()
    }

    fn bytes_in(&self) -> u64 {
        self.0.bytes_in()
    }

    fn bytes_out(&self) -> u64 {
        self.0.bytes_out()
    }
}

/// A started application with both listeners bound and ready to serve.
pub struct App {
    proxy: ProxyServer,
    proxy_addr: SocketAddr,
    web_listener: TcpListener,
    web_addr: SocketAddr,
    router: Router,
}

impl App {
    /// Validate the config, build routing, and bind the proxy and web listeners
    /// on loopback. Fails fast (before serving) on an invalid config or a bind
    /// error, with context on the returned [`anyhow::Error`].
    pub async fn start(
        config: Config,
        config_path: PathBuf,
        logs: LogStore,
    ) -> anyhow::Result<Self> {
        config
            .validate(&ValidationCtx::host())
            .context("configuration is invalid")?;
        let routing = build_routing(&config)?;
        let routing_mode = routing_mode_label(config.routing.mode);

        let host: IpAddr =
            config.listen.host.parse().with_context(|| {
                format!("listen.host is not an IP address: {}", config.listen.host)
            })?;

        // The UI token is provisioned before binding so a failure here fails
        // fast without leaving listeners open.
        let token = load_or_create_token().context("failed to load or create the UI token")?;

        let proxy = ProxyServer::bind(SocketAddr::new(host, config.listen.port))
            .await
            .context("failed to bind the proxy listener")?
            .with_routing(routing);
        let proxy_addr = proxy.local_addr();

        // A live handle to the proxy's routing, captured before `proxy` is
        // moved into `Self`. The apply hook rebuilds routing from the new
        // config and swaps it here, so UI edits take effect without a restart.
        let routing_handle = proxy.routing_handle();
        let apply_handle = routing_handle.clone();
        let hook: zicade_web::ConfigApplyHook =
            std::sync::Arc::new(move |cfg: &zicade_config::Config| {
                match crate::routing::build_routing(cfg) {
                    Ok(routing) => apply_handle.set(routing),
                    Err(err) => tracing::error!(
                        error = %format!("{err:#}"),
                        "failed to rebuild routing on live config apply"
                    ),
                }
            });

        // The web server sits on the proxy port + 1 (both loopback).
        let web_port = proxy_addr
            .port()
            .checked_add(1)
            .context("proxy port + 1 overflows u16; choose a lower listen.port")?;
        let web_listener = TcpListener::bind(SocketAddr::new(host, web_port))
            .await
            .context("failed to bind the web listener")?;
        let web_addr = web_listener.local_addr()?;

        // Capture the live metrics handle before `proxy` is moved into `Self`.
        let metrics = proxy.metrics();

        let state = AppState::new(config, config_path, token, logs);
        let state = state.with_metrics_source(std::sync::Arc::new(ProxyMetricsSource(metrics)));
        let state = state.with_apply_hook(hook);
        state.set_status(StatusSnapshot {
            routing_mode: routing_mode.to_owned(),
            listen_addr: proxy_addr.to_string(),
            ..StatusSnapshot::default()
        });

        Ok(Self {
            proxy,
            proxy_addr,
            web_listener,
            web_addr,
            router: router(state),
        })
    }

    /// The bound proxy listen address.
    pub fn proxy_addr(&self) -> SocketAddr {
        self.proxy_addr
    }

    /// The bound web (UI/API) listen address.
    pub fn web_addr(&self) -> SocketAddr {
        self.web_addr
    }

    /// Serve the proxy and web servers until `shutdown` resolves, fanning the
    /// signal out to both. If either server exits on its own first, the other
    /// is signaled too so `run` never hangs; the first error is surfaced.
    pub async fn run(self, shutdown: impl Future<Output = ()> + Send) -> anyhow::Result<()> {
        let (tx, _rx) = watch::channel(false);
        let Self {
            proxy,
            web_listener,
            router,
            ..
        } = self;

        let proxy_tx = tx.clone();
        let proxy_fut = {
            let signal = wait_for_shutdown(tx.subscribe());
            async move {
                let res = proxy.serve(signal).await;
                let _ = proxy_tx.send(true);
                res.context("proxy server error")
            }
        };

        let web_tx = tx.clone();
        let web_fut = {
            let signal = wait_for_shutdown(tx.subscribe());
            async move {
                let res = axum::serve(web_listener, router)
                    .with_graceful_shutdown(signal)
                    .await;
                let _ = web_tx.send(true);
                res.context("web server error")
            }
        };

        let driver = async move {
            shutdown.await;
            let _ = tx.send(true);
        };

        let (proxy_res, web_res, ()) = tokio::join!(proxy_fut, web_fut, driver);
        proxy_res?;
        web_res?;
        Ok(())
    }
}

/// Resolves once the shutdown flag flips to `true` (returning immediately if it
/// is already set).
async fn wait_for_shutdown(mut rx: watch::Receiver<bool>) {
    let _ = rx.wait_for(|flag| *flag).await;
}

fn routing_mode_label(mode: RoutingMode) -> &'static str {
    match mode {
        RoutingMode::Direct => "direct",
        RoutingMode::Upstream => "upstream",
        RoutingMode::Pac => "pac",
    }
}
