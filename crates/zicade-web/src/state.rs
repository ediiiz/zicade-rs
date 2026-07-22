//! Shared, cloneable application state for the axum handlers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use zicade_config::{Config, RoutingMode};
use zicade_observe::LogStore;

/// A source of live proxy metrics the status endpoint can overlay onto its
/// stored snapshot. Implemented by an adapter over the running proxy handle.
pub trait MetricsSource: Send + Sync {
    /// Total requests forwarded since start.
    fn total_requests(&self) -> u64;
    /// Connections currently being served.
    fn active_connections(&self) -> usize;
    /// Requests that failed since start.
    fn failed_requests(&self) -> u64;
    /// Cumulative bytes streamed back to clients (download).
    fn bytes_in(&self) -> u64;
    /// Cumulative bytes streamed out to origins/upstreams (upload).
    fn bytes_out(&self) -> u64;
}

/// A side-effecting hook run when a validated config is applied live.
///
/// The app wires this to rebuild the proxy's routing and swap it behind the
/// shared [`zicade_proxy::RoutingHandle`], so UI routing edits take effect
/// without a restart. Invoked with a reference just before the new config is
/// moved into the in-memory store.
pub type ConfigApplyHook = std::sync::Arc<dyn Fn(&Config) + Send + Sync>;

/// A read-only status snapshot surfaced by `GET /api/status`.
///
/// `routing_mode` and `listen_addr` are set once at startup via
/// [`AppState::set_status`]; the request counters are overlaid live from the
/// attached [`MetricsSource`] (see [`AppState::status_snapshot`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    /// Active routing mode (`"direct"`, `"upstream"`, `"pac"`).
    pub routing_mode: String,
    /// The proxy listen address, e.g. `"127.0.0.1:3129"`.
    pub listen_addr: String,
    /// Total requests handled since start.
    pub requests_total: u64,
    /// Requests that failed.
    pub requests_failed: u64,
    /// Connections currently being served.
    pub active_connections: usize,
    /// Cumulative bytes streamed back to clients (download).
    pub bytes_in: u64,
    /// Cumulative bytes streamed out to origins/upstreams (upload).
    pub bytes_out: u64,
}

/// Cloneable handle (all fields are `Arc`-backed) passed to every handler.
#[derive(Clone)]
pub struct AppState {
    config: Arc<Mutex<Config>>,
    config_path: Arc<PathBuf>,
    token: Arc<str>,
    logs: LogStore,
    status: Arc<Mutex<StatusSnapshot>>,
    metrics: Option<Arc<dyn MetricsSource>>,
    apply_hook: Option<ConfigApplyHook>,
}

impl AppState {
    /// Build state around a loaded config, its on-disk path, the UI token, and
    /// the observability log store.
    pub fn new(config: Config, config_path: PathBuf, token: String, logs: LogStore) -> Self {
        Self {
            config: Arc::new(Mutex::new(config)),
            config_path: Arc::new(config_path),
            token: Arc::from(token),
            logs,
            status: Arc::new(Mutex::new(StatusSnapshot::default())),
            metrics: None,
            apply_hook: None,
        }
    }

    /// Attach a live metrics source; its counters overlay the stored snapshot in
    /// [`AppState::status_snapshot`].
    #[must_use]
    pub fn with_metrics_source(mut self, src: Arc<dyn MetricsSource>) -> Self {
        self.metrics = Some(src);
        self
    }

    /// Attach a hook invoked with each validated config on
    /// [`AppState::apply_config`] (used by the app to rebuild live routing).
    #[must_use]
    pub fn with_apply_hook(mut self, hook: ConfigApplyHook) -> Self {
        self.apply_hook = Some(hook);
        self
    }

    /// The path the config is persisted to.
    pub fn config_path(&self) -> &Path {
        self.config_path.as_path()
    }

    /// A clone of the shared config handle (used by callers/tests to observe
    /// in-memory updates applied by `PUT /api/config`).
    pub fn config_arc(&self) -> Arc<Mutex<Config>> {
        Arc::clone(&self.config)
    }

    /// Replace the status snapshot.
    pub fn set_status(&self, status: StatusSnapshot) {
        if let Ok(mut guard) = self.status.lock() {
            *guard = status;
        }
    }

    /// The configured UI token (mutation gate secret).
    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    /// Whether mutating routes require the `X-Zicade-Token` header, read live
    /// from the in-memory config (`web.authRequired`, default `false`).
    pub(crate) fn auth_required(&self) -> bool {
        self.config
            .lock()
            .map(|c| c.web.auth_required)
            .unwrap_or(false)
    }

    /// The log store feeding the SSE stream.
    pub(crate) fn logs(&self) -> &LogStore {
        &self.logs
    }

    /// A snapshot clone of the current in-memory config.
    pub(crate) fn config_snapshot(&self) -> Config {
        self.config.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// A snapshot clone of the current status. If a live metrics source is
    /// attached, its counters overlay the stored `requests_total`,
    /// `active_connections`, and `requests_failed` (preserving `routing_mode`
    /// and `listen_addr`).
    pub(crate) fn status_snapshot(&self) -> StatusSnapshot {
        let mut snapshot = self.status.lock().map(|s| s.clone()).unwrap_or_default();
        if let Some(metrics) = &self.metrics {
            snapshot.requests_total = metrics.total_requests();
            snapshot.active_connections = metrics.active_connections();
            snapshot.requests_failed = metrics.failed_requests();
            snapshot.bytes_in = metrics.bytes_in();
            snapshot.bytes_out = metrics.bytes_out();
        }
        snapshot
    }

    /// Replace the in-memory config after a validated update, first running the
    /// apply hook (if any) so live routing is rebuilt from the new config.
    ///
    /// Also refreshes the status snapshot's `routing_mode` so `GET /api/status`
    /// reflects the just-applied routing. `listen_addr` is left untouched: it is
    /// the actually-bound address, and a listen change requires a restart.
    pub(crate) fn apply_config(&self, config: Config) {
        if let Some(hook) = &self.apply_hook {
            hook(&config);
        }
        if let Ok(mut status) = self.status.lock() {
            status.routing_mode = routing_mode_label(config.routing.mode).to_owned();
        }
        if let Ok(mut guard) = self.config.lock() {
            *guard = config;
        }
    }
}

/// The stable string label for a routing mode, as surfaced in the status.
fn routing_mode_label(mode: RoutingMode) -> &'static str {
    match mode {
        RoutingMode::Direct => "direct",
        RoutingMode::Upstream => "upstream",
        RoutingMode::Pac => "pac",
    }
}
