#![forbid(unsafe_code)]

//! Zicade application wiring, in library form so the lifecycle is testable
//! without installing a global tracing subscriber (the binary installs that in
//! `main`; see [`init_tracing`]).
//!
//! [`App::start`] validates the config, maps it onto the proxy [`Routing`],
//! and binds the proxy + web listeners on loopback. [`App::run`] serves both
//! under one fan-out shutdown signal. The free [`run`] function is the
//! start-then-serve convenience used by `main`.

mod app;
pub mod cli;
mod netmon;
mod pac_router;
mod routing;

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt as _;
use zicade_config::{Config, LoggingConfig};
use zicade_observe::{ChannelLayer, LogStore};

pub use app::App;
pub use routing::build_routing;

/// Capacity (events) of the in-memory log ring buffer feeding the UI SSE stream.
pub const LOG_BUFFER_CAP: usize = 1024;

/// A type-erased handle for live-reloading the tracing max level. Invoked with a
/// config level string (`"debug"`, `"info"`, ...); it swaps the global level
/// filter so a UI log-level change takes effect on the live stream without a
/// restart. See [`init_tracing`] for the concrete handle and [`noop_log_reload`]
/// for the inert one used off the binary's path.
pub type LogReload = Arc<dyn Fn(&str) + Send + Sync>;

/// A no-op [`LogReload`] for tests and any context without a live subscriber.
#[must_use]
pub fn noop_log_reload() -> LogReload {
    Arc::new(|_| {})
}

/// Start the app and serve until `shutdown` resolves.
pub async fn run(
    config: Config,
    config_path: PathBuf,
    logs: LogStore,
    log_reload: LogReload,
    shutdown: impl Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    let app = App::start(config, config_path, logs, log_reload).await?;
    tracing::info!(
        proxy = %app.proxy_addr(),
        web = %app.web_addr(),
        "zicade started; open the web UI in a browser"
    );
    app.run(shutdown).await
}

/// Install the global tracing subscriber: the observe [`ChannelLayer`] (feeding
/// the UI) plus a fmt layer, gated by the configured level.
///
/// The level filter is installed behind a [`reload::Layer`], and the returned
/// [`LogReload`] swaps it live: it gates *both* the UI and fmt layers (a global
/// filter layer), so selecting `debug` in the UI immediately surfaces debug (and
/// every less-verbose level) on the live log stream. Reloading rebuilds the
/// callsite interest cache, so raising verbosity takes effect.
///
/// Call this exactly once, from `main`/tray — never from library/test code, to
/// avoid the set-once global-subscriber panic.
#[must_use]
pub fn init_tracing(logging: &LoggingConfig, observe_layer: ChannelLayer) -> LogReload {
    let (level, handle) = reload::Layer::new(level_filter(&logging.level));
    let fmt_layer = if logging.format.eq_ignore_ascii_case("json") {
        tracing_subscriber::fmt::layer().json().boxed()
    } else {
        tracing_subscriber::fmt::layer().compact().boxed()
    };
    tracing_subscriber::registry()
        .with(level)
        .with(observe_layer)
        .with(fmt_layer)
        .init();
    Arc::new(move |level: &str| {
        // A dropped subscriber (never, in practice — it is global and static for
        // the process lifetime) is the only failure; ignore it.
        let _ = handle.reload(level_filter(level));
    })
}

/// Parse a logging level string into a [`LevelFilter`], defaulting to `INFO`
/// for anything unrecognized.
pub fn level_filter(level: &str) -> LevelFilter {
    match level.trim().to_ascii_lowercase().as_str() {
        "trace" => LevelFilter::TRACE,
        "debug" => LevelFilter::DEBUG,
        "info" => LevelFilter::INFO,
        "warn" | "warning" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        "off" => LevelFilter::OFF,
        _ => LevelFilter::INFO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_filter_maps_known_levels() {
        assert_eq!(level_filter("trace"), LevelFilter::TRACE);
        assert_eq!(level_filter("DEBUG"), LevelFilter::DEBUG);
        assert_eq!(level_filter(" info "), LevelFilter::INFO);
        assert_eq!(level_filter("warn"), LevelFilter::WARN);
        assert_eq!(level_filter("error"), LevelFilter::ERROR);
        assert_eq!(level_filter("off"), LevelFilter::OFF);
    }

    #[test]
    fn level_filter_defaults_to_info() {
        assert_eq!(level_filter("nonsense"), LevelFilter::INFO);
        assert_eq!(level_filter(""), LevelFilter::INFO);
    }
}
