//! Tray-mode orchestration for the binary (double-click launch).
//!
//! The right-click menu has exactly two items — "Open WebUI" and "Close" — and
//! the id -> action mapping is pure logic ([`tray_action`]) so it can be tested
//! without a desktop session. The orchestration ([`run_tray_mode`]) hides the
//! console, runs the proxy + web server on a background tokio runtime, and pumps
//! the tray on the main thread, feeding the existing graceful-shutdown signal
//! when "Close" is chosen.

use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;

use tokio::sync::watch;
use zicade::{LOG_BUFFER_CAP, init_tracing};
use zicade_config::Config;
use zicade_observe::{LogStore, channel_layer};
use zicade_win::tray::{self, TrayControl, TrayMenuItem};

/// Command id for the "Open WebUI" menu item.
pub const OPEN_WEBUI_ID: u32 = 1;
/// Command id for the "Close" menu item.
pub const CLOSE_ID: u32 = 2;

/// What a clicked menu item should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    /// Open the web UI in the default browser.
    OpenWebUi,
    /// Trigger graceful shutdown and exit.
    Close,
}

/// The exactly-two right-click menu items, in display order.
pub fn tray_menu_items() -> Vec<TrayMenuItem> {
    vec![
        TrayMenuItem::new(OPEN_WEBUI_ID, "Open WebUI"),
        TrayMenuItem::new(CLOSE_ID, "Close"),
    ]
}

/// Map a clicked menu command id to its [`TrayAction`], or `None` if unknown.
pub fn tray_action(id: u32) -> Option<TrayAction> {
    match id {
        OPEN_WEBUI_ID => Some(TrayAction::OpenWebUi),
        CLOSE_ID => Some(TrayAction::Close),
        _ => None,
    }
}

/// Run the app in tray mode: hide the console, serve proxy + web on a background
/// tokio runtime, and pump the tray on the calling (main) thread until "Close".
///
/// The tray message pump must own a thread, so the tokio runtime runs on a
/// separate one; a [`watch`] channel carries the shutdown signal from the
/// "Close" handler (and, as a backstop, dropping the sender when the pump ends)
/// into [`zicade::run`]'s shutdown future.
pub fn run_tray_mode() -> ExitCode {
    // Resolve and load the config BEFORE hiding the console, so any load error
    // is still visible to a user who launched from a terminal.
    let config_path = match crate::default_config_path() {
        Ok(path) => path,
        Err(err) => return fail(&format!("{err:#}")),
    };
    let config = match crate::load_or_create_config(&config_path) {
        Ok(config) => config,
        Err(err) => return fail(&format!("{err:#}")),
    };
    let web_url = web_ui_url(&config);

    // Install the subscriber (logs flow to the web UI), then hide the console.
    let (observe_layer, logs) = channel_layer(LOG_BUFFER_CAP);
    init_tracing(&config.logging, observe_layer);
    if let Err(err) = tray::hide_console() {
        tracing::warn!(%err, "could not hide the console window");
    }
    tracing::info!(web = %web_url, "zicade started in tray mode");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server = thread::Builder::new()
        .name("zicade-runtime".to_owned())
        .spawn(move || serve_on_runtime(config, config_path, logs, shutdown_rx))
        .expect("spawn zicade runtime thread");

    let pump = tray::run_tray("Zicade proxy", tray_menu_items(), move |id| {
        on_tray_event(id, &web_url, &shutdown_tx)
    });

    // The pump has returned, so its `shutdown_tx` is dropped: the runtime's
    // shutdown future resolves (on the sent `true` or the sender drop). Join it.
    let _ = server.join();

    match pump {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => fail(&format!("tray error: {err}")),
    }
}

/// Handle one tray menu click. `Open WebUI` opens the browser and keeps
/// pumping; `Close` signals shutdown and ends the pump.
fn on_tray_event(id: u32, web_url: &str, shutdown_tx: &watch::Sender<bool>) -> TrayControl {
    match tray_action(id) {
        Some(TrayAction::OpenWebUi) => {
            if let Err(err) = tray::open_url(web_url) {
                tracing::warn!(%err, "failed to open the web UI in the default browser");
            }
            TrayControl::Continue
        }
        Some(TrayAction::Close) => {
            let _ = shutdown_tx.send(true);
            TrayControl::Quit
        }
        None => TrayControl::Continue,
    }
}

/// Build a tokio runtime and serve until the shutdown signal (a `true` value or
/// all senders dropped) resolves. Runs on the background runtime thread.
fn serve_on_runtime(
    config: Config,
    config_path: PathBuf,
    logs: LogStore,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!(%err, "failed to start the tray-mode async runtime");
            return;
        }
    };
    let shutdown = async move {
        let _ = shutdown_rx.wait_for(|flag| *flag).await;
    };
    if let Err(err) = runtime.block_on(zicade::run(config, config_path, logs, shutdown)) {
        tracing::error!(error = %format!("{err:#}"), "tray-mode server exited with an error");
    }
}

/// The web UI URL derived from the loaded config: proxy `listen.port` + 1.
fn web_ui_url(config: &Config) -> String {
    let web_port = config.listen.port.saturating_add(1);
    format!("http://{}:{}/", config.listen.host, web_port)
}

/// Print a fatal tray-startup error and return a failure exit code.
fn fail(message: &str) -> ExitCode {
    eprintln!("zicade: {message}");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_has_exactly_open_and_close() {
        let items = tray_menu_items();
        assert_eq!(items.len(), 2, "tray menu must have exactly two items");
        assert_eq!(items[0].id, OPEN_WEBUI_ID);
        assert_eq!(items[0].label, "Open WebUI");
        assert_eq!(items[1].id, CLOSE_ID);
        assert_eq!(items[1].label, "Close");
    }

    #[test]
    fn ids_map_to_their_actions() {
        assert_eq!(tray_action(OPEN_WEBUI_ID), Some(TrayAction::OpenWebUi));
        assert_eq!(tray_action(CLOSE_ID), Some(TrayAction::Close));
    }

    #[test]
    fn unknown_id_maps_to_no_action() {
        assert_eq!(tray_action(0), None);
        assert_eq!(tray_action(999), None);
    }

    #[test]
    fn web_ui_url_is_proxy_port_plus_one() {
        // Default config (127.0.0.1:3129) => web UI on 3130, per the spec.
        assert_eq!(web_ui_url(&Config::default()), "http://127.0.0.1:3130/");
    }
}
