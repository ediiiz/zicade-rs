//! Shared, cloneable application state for the axum handlers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use zicade_config::Config;
use zicade_observe::LogStore;

/// A read-only status snapshot surfaced by `GET /api/status`.
///
/// For M5 there is no live proxy wiring; callers update this via
/// [`AppState::set_status`]. Fields cover the routing mode, the bound listen
/// address, and simple request counters.
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
}

/// Cloneable handle (all fields are `Arc`-backed) passed to every handler.
#[derive(Clone)]
pub struct AppState {
    config: Arc<Mutex<Config>>,
    config_path: Arc<PathBuf>,
    token: Arc<str>,
    logs: LogStore,
    status: Arc<Mutex<StatusSnapshot>>,
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
        }
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

    /// A snapshot clone of the current status.
    pub(crate) fn status_snapshot(&self) -> StatusSnapshot {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Replace the in-memory config after a validated update.
    pub(crate) fn apply_config(&self, config: Config) {
        if let Ok(mut guard) = self.config.lock() {
            *guard = config;
        }
    }
}
