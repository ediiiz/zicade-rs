//! The started application: bind the proxy listener, build the UI's
//! [`Control`] handle, then serve until the shutdown signal.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::Context as _;
use tokio::sync::watch;
use zicade_config::{Config, RoutingMode, ValidationCtx};
use zicade_control::{Control, MetricsSource, StatusSnapshot};
use zicade_observe::LogStore;
use zicade_proxy::ProxyServer;

use crate::routing::build_routing;

/// Adapts the proxy's live [`zicade_proxy::ProxyMetrics`] to the control
/// crate's [`MetricsSource`] trait so the status view reports real counters.
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

/// A started application with its listener bound and ready to serve.
pub struct App {
    proxy: ProxyServer,
    proxy_addr: SocketAddr,
    control: Control,
    /// Fans the shutdown signal out to the proxy and the corp-network monitor.
    shutdown_tx: watch::Sender<bool>,
    /// The corporate-network gate. Always present so the corp-network toggle can
    /// be switched on (or off) live via the UI without a restart; while disabled
    /// it is dormant (its `evaluate` is a no-op). Its monitor thread is spawned in
    /// [`App::run`] (so it observes the same shutdown signal) and gates live
    /// routing between the configured mode (on-corp) and `Direct` (off-corp).
    gate: crate::netmon::NetworkGate,
}

impl App {
    /// Validate the config, build routing, and bind the proxy listener on
    /// loopback. Fails fast (before serving) on an invalid config or a bind
    /// error, with context on the returned [`anyhow::Error`].
    pub async fn start(
        config: Config,
        config_path: PathBuf,
        logs: LogStore,
        log_reload: crate::LogReload,
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
        // into `Self`. The gate swaps routing through this handle (on network or
        // config changes), so edits take effect without a restart.
        let routing_handle = proxy.routing_handle();
        // Shared on-corp state (gate → status view). Always allocated: the gate is
        // always present, and it writes `None` here while disabled so the status
        // omits `onCorp` until the gate is actually gating.
        let on_corp_cell: zicade_control::OnCorpCell =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let (hook, gate) =
            build_apply_hook_and_gate(&config, &routing_handle, on_corp_cell.clone(), log_reload);

        // Capture the live metrics handle before `proxy` is moved into `Self`.
        let metrics = proxy.metrics();

        // One shutdown channel drives everything: `run`'s driver flips it and
        // the proxy and the corp-network monitor watch it.
        let (shutdown_tx, _) = watch::channel(false);

        let control = Control::new(config, config_path, logs)
            .with_metrics_source(std::sync::Arc::new(ProxyMetricsSource(metrics)))
            .with_apply_hook(hook)
            .with_on_corp(on_corp_cell);
        control.set_status(StatusSnapshot {
            routing_mode: routing_mode.to_owned(),
            listen_addr: proxy_addr.to_string(),
            ..StatusSnapshot::default()
        });

        Ok(Self {
            proxy,
            proxy_addr,
            control,
            shutdown_tx,
            gate,
        })
    }

    /// The bound proxy listen address.
    pub fn proxy_addr(&self) -> SocketAddr {
        self.proxy_addr
    }

    /// The UI's handle on this app: config, status, logs, and live apply.
    pub fn control(&self) -> Control {
        self.control.clone()
    }

    /// Serve the proxy until `shutdown` resolves, then stop the corp-network
    /// monitor with it.
    pub async fn run(self, shutdown: impl Future<Output = ()> + Send) -> anyhow::Result<()> {
        let Self {
            proxy,
            shutdown_tx: tx,
            gate,
            ..
        } = self;

        // Start the corp-network gate's monitor thread now, sharing the same
        // shutdown signal so it winds down with everything else. While the gate
        // is disabled the thread just polls and does nothing, so it is always
        // spawned — enabling the gate live via the UI then takes effect.
        gate.spawn(tx.subscribe())
            .context("start the corp-network monitor thread")?;

        let driver = {
            let tx = tx.clone();
            async move {
                shutdown.await;
                let _ = tx.send(true);
            }
        };
        let proxy_fut = proxy.serve(wait_for_shutdown(tx.subscribe()));
        let (proxy_res, ()) = tokio::join!(proxy_fut, driver);
        // Whatever ended the proxy, the monitor thread must stop with it.
        let _ = tx.send(true);
        proxy_res.context("proxy server error")
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

/// Build the (always-present) corp-network gate and the config-apply hook that
/// drives it.
///
/// The gate owns the proxy's live routing whether or not corp-network detection
/// is currently enabled:
/// - **Enabled:** the gate swaps between the configured routing (on-corp) and
///   `Direct` (off-corp); its initial decision is applied synchronously here
///   (before serving) via [`NetworkGate::evaluate_now`].
/// - **Disabled:** the gate is dormant (`evaluate` is a no-op). Its
///   [`NetworkGate::reconfigure`] applies the plain configured routing, which is
///   how a live routing edit takes effect — matching the old ungated behavior.
///
/// Because the gate is always built, toggling corp-network detection on/off in
/// the UI takes effect live: the apply hook always calls `reconfigure`, which
/// enables or disables gating as the new config dictates. The hook also
/// live-applies the log level (via `log_reload`) before reconfiguring.
fn build_apply_hook_and_gate(
    config: &Config,
    routing_handle: &zicade_proxy::RoutingHandle,
    on_corp_cell: zicade_control::OnCorpCell,
    log_reload: crate::LogReload,
) -> (zicade_control::ConfigApplyHook, crate::netmon::NetworkGate) {
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
        Box::new(move |state: Option<bool>| {
            if let Ok(mut c) = on_corp_cell.lock() {
                *c = state;
            }
        }),
    );
    // Set the correct initial routing before the proxy begins serving. A no-op
    // while the gate is disabled; the caller applies the plain configured routing
    // (failing fast on a build error) in that case.
    gate.evaluate_now();
    let hook_gate = gate.clone();
    let hook: zicade_control::ConfigApplyHook = std::sync::Arc::new(move |cfg: &Config| {
        // Apply the log level first so the reconfigure it triggers is captured at
        // the newly-selected verbosity.
        (log_reload)(&cfg.logging.level);
        hook_gate.reconfigure(cfg.clone());
    });
    (hook, gate)
}
