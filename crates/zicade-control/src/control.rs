//! The shared, cloneable [`Control`] handle and the status it reports.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use zicade_config::{Config, RoutingMode, ValidationCtx};
use zicade_observe::LogStore;
use zicade_proxy::ConnectionSnapshot;

use crate::error::ControlError;

/// A source of live proxy metrics the status overlays onto its stored
/// snapshot. Implemented by an adapter over the running proxy handle.
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
    /// Open connections, then recently finished ones; newest first in each.
    fn connections(&self) -> Vec<ConnectionSnapshot> {
        Vec::new()
    }
}

/// A side-effecting hook run when a validated config is applied live.
///
/// The app wires this to rebuild the proxy's routing and swap it behind the
/// shared routing handle, so routing edits take effect without a restart.
pub type ConfigApplyHook = Arc<dyn Fn(&Config) + Send + Sync>;

/// A shared cell the corporate-network gate writes the current on-corp state
/// into; the status overlays it. `None` = the gate is disabled or the state is
/// not yet known.
pub type OnCorpCell = Arc<Mutex<Option<bool>>>;

/// What the status view shows.
///
/// `routing_mode` and `listen_addr` are set at startup via
/// [`Control::set_status`]; the counters are overlaid live from the attached
/// [`MetricsSource`] (see [`Control::status`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    /// Whether the machine is on the corporate network, while the corp-network
    /// gate is active; `None` when it is disabled or not yet known.
    pub on_corp: Option<bool>,
}

/// Cloneable handle (all fields are `Arc`-backed) the UI holds.
#[derive(Clone)]
pub struct Control {
    config: Arc<Mutex<Config>>,
    config_path: Arc<PathBuf>,
    logs: LogStore,
    status: Arc<Mutex<StatusSnapshot>>,
    metrics: Option<Arc<dyn MetricsSource>>,
    apply_hook: Option<ConfigApplyHook>,
    on_corp: Option<OnCorpCell>,
}

impl Control {
    /// Build a handle around a loaded config, its on-disk path, and the log
    /// store.
    pub fn new(config: Config, config_path: PathBuf, logs: LogStore) -> Self {
        Self {
            config: Arc::new(Mutex::new(config)),
            config_path: Arc::new(config_path),
            logs,
            status: Arc::new(Mutex::new(StatusSnapshot::default())),
            metrics: None,
            apply_hook: None,
            on_corp: None,
        }
    }

    /// Attach a live metrics source; its counters overlay the stored snapshot.
    #[must_use]
    pub fn with_metrics_source(mut self, src: Arc<dyn MetricsSource>) -> Self {
        self.metrics = Some(src);
        self
    }

    /// Attach the hook run with each validated config on [`Control::apply`].
    #[must_use]
    pub fn with_apply_hook(mut self, hook: ConfigApplyHook) -> Self {
        self.apply_hook = Some(hook);
        self
    }

    /// Attach the corp-network gate's on-corp cell.
    #[must_use]
    pub fn with_on_corp(mut self, cell: OnCorpCell) -> Self {
        self.on_corp = Some(cell);
        self
    }

    /// The path the config is persisted to.
    pub fn config_path(&self) -> &Path {
        self.config_path.as_path()
    }

    /// The log store feeding the log view.
    pub fn logs(&self) -> &LogStore {
        &self.logs
    }

    /// Replace the stored status snapshot.
    pub fn set_status(&self, status: StatusSnapshot) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = status;
    }

    /// The current config with every stored password removed — the only form
    /// the UI ever gets, so a password is never echoed back on screen.
    pub fn config_redacted(&self) -> Config {
        self.config
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .without_secrets()
    }

    /// Every tracked connection: open ones first, then the recent history.
    pub fn connections(&self) -> Vec<ConnectionSnapshot> {
        self.metrics
            .as_ref()
            .map(|metrics| metrics.connections())
            .unwrap_or_default()
    }

    /// The stored status with live counters and the on-corp state overlaid.
    pub fn status(&self) -> StatusSnapshot {
        let mut snapshot = self
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(metrics) = &self.metrics {
            snapshot.requests_total = metrics.total_requests();
            snapshot.active_connections = metrics.active_connections();
            snapshot.requests_failed = metrics.failed_requests();
            snapshot.bytes_in = metrics.bytes_in();
            snapshot.bytes_out = metrics.bytes_out();
        }
        if let Some(cell) = &self.on_corp {
            snapshot.on_corp = *cell.lock().unwrap_or_else(PoisonError::into_inner);
        }
        snapshot
    }

    /// Apply an edited config: keep passwords the UI never saw, validate,
    /// persist, run the apply hook, then make it the current config.
    ///
    /// Nothing changes unless every step before the hook succeeds. This may
    /// block (disk I/O, and the hook may rebuild PAC routing), so a UI should
    /// call it off its render thread.
    pub fn apply(&self, mut config: Config) -> Result<(), ControlError> {
        let current = self
            .config
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        config.keep_secrets_from(&current);
        config
            .validate(&ValidationCtx::host())
            .map_err(ControlError::Invalid)?;
        zicade_config::save_file(self.config_path(), &config).map_err(ControlError::Save)?;
        if let Some(hook) = &self.apply_hook {
            hook(&config);
        }
        // A listen change needs a restart, so `listen_addr` keeps reporting the
        // actually-bound address; only the routing label follows the edit.
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .routing_mode = routing_mode_label(config.routing.mode).to_owned();
        *self.config.lock().unwrap_or_else(PoisonError::into_inner) = config;
        Ok(())
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
