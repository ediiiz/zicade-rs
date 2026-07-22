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
    /// Fans the shutdown signal out to the proxy, the web server, and the web
    /// crate's SSE streams (via the receiver handed to `AppState`). Firing it
    /// ends the long-lived SSE responses so axum's graceful shutdown completes
    /// instead of waiting forever on an open browser tab.
    shutdown_tx: watch::Sender<bool>,
    /// The corporate-network gate, when enabled. Its monitor thread is spawned in
    /// [`App::run`] (so it observes the same shutdown signal) and gates live
    /// routing between the configured mode (on-corp) and `Direct` (off-corp).
    gate: Option<crate::netmon::NetworkGate>,
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
        let routing_mode = routing_mode_label(config.routing.mode);
        let gate_enabled = crate::netmon::gate_enabled(&config);

        let host: IpAddr =
            config.listen.host.parse().with_context(|| {
                format!("listen.host is not an IP address: {}", config.listen.host)
            })?;

        // The UI token is provisioned before binding so a failure here fails
        // fast without leaving listeners open.
        let token = load_or_create_token().context("failed to load or create the UI token")?;

        // Bind the proxy. When the corp-network gate is enabled it owns the live
        // routing (set by the gate below); otherwise the configured routing is
        // fixed now, failing fast on a build error.
        let mut proxy = ProxyServer::bind(SocketAddr::new(host, config.listen.port))
            .await
            .context("failed to bind the proxy listener")?;
        if !gate_enabled {
            proxy = proxy.with_routing(build_routing(&config)?);
        }
        let proxy_addr = proxy.local_addr();

        // A live handle to the proxy's routing, captured before `proxy` is moved
        // into `Self`. Either the apply hook (ungated) or the gate (gated) swaps
        // routing here, so changes take effect without a restart.
        let routing_handle = proxy.routing_handle();
        let (hook, gate) = build_apply_hook_and_gate(&config, gate_enabled, &routing_handle);

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

        // One shutdown channel drives everything: `run`'s driver flips it, the
        // proxy and web servers watch it, and the web layer's SSE streams end on
        // it (the receiver handed to `AppState`).
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let state = AppState::new(config, config_path, token, logs);
        let state = state.with_metrics_source(std::sync::Arc::new(ProxyMetricsSource(metrics)));
        let state = state.with_apply_hook(hook);
        let state = state.with_shutdown(shutdown_rx);
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
            shutdown_tx,
            gate,
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
        let Self {
            proxy,
            web_listener,
            router,
            shutdown_tx: tx,
            gate,
            ..
        } = self;

        // Start the corp-network gate's monitor thread now, sharing the same
        // shutdown signal so it winds down with everything else.
        if let Some(gate) = gate {
            gate.spawn(tx.subscribe());
        }

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

/// Build the config-apply hook and, when the corp-network gate is enabled, the
/// gate itself.
///
/// - **Gated:** the gate owns live routing; its initial routing is applied
///   synchronously (before serving) and the apply hook reconfigures the gate.
/// - **Ungated:** the apply hook rebuilds routing and swaps it directly (the
///   pre-existing live-apply behavior).
fn build_apply_hook_and_gate(
    config: &Config,
    gate_enabled: bool,
    routing_handle: &zicade_proxy::RoutingHandle,
) -> (
    zicade_web::ConfigApplyHook,
    Option<crate::netmon::NetworkGate>,
) {
    if gate_enabled {
        let gate = crate::netmon::NetworkGate::new(
            routing_handle.clone(),
            config.clone(),
            Box::new(|cfg: &Config| match build_routing(cfg) {
                Ok(routing) => routing,
                Err(err) => {
                    tracing::error!(
                        error = %format!("{err:#}"),
                        "corp-network gate: routing rebuild failed; using Direct"
                    );
                    zicade_proxy::Routing::Direct
                }
            }),
            Box::new(zicade_win::active_dns_suffixes),
            Box::new(|_corp| {}),
        );
        // Set the correct initial routing before the proxy begins serving.
        gate.evaluate_now();
        let hook_gate = gate.clone();
        let hook: zicade_web::ConfigApplyHook =
            std::sync::Arc::new(move |cfg: &Config| hook_gate.reconfigure(cfg.clone()));
        (hook, Some(gate))
    } else {
        let apply_handle = routing_handle.clone();
        let hook: zicade_web::ConfigApplyHook =
            std::sync::Arc::new(move |cfg: &Config| match build_routing(cfg) {
                Ok(routing) => apply_handle.set(routing),
                Err(err) => tracing::error!(
                    error = %format!("{err:#}"),
                    "failed to rebuild routing on live config apply"
                ),
            });
        (hook, None)
    }
}
