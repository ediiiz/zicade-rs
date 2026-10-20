//! Tray mode: the proxy on a tokio thread, the tray icon on a thread of its
//! own, and the native window on the main thread (the UI toolkit needs it).
//!
//! The right-click menu has exactly two items — "Open Zicade" and "Close" —
//! and a left-click runs the first. The id -> action mapping is pure logic
//! ([`tray_action`]) so it can be tested without a desktop session.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use tokio::sync::{mpsc as async_mpsc, watch};
use zicade::{App, LOG_BUFFER_CAP, init_tracing};
use zicade_config::Config;
use zicade_control::Control;
use zicade_observe::{LogStore, channel_layer};
use zicade_ui::{Launch, UiCommand};
use zicade_win::tray::{self, TrayControl, TrayMenuItem};

/// Command id for the "Open Zicade" menu item (also the left-click action).
pub const OPEN_ID: u32 = 1;
/// Command id for the "Close" menu item.
pub const CLOSE_ID: u32 = 2;

/// What a clicked menu item should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    /// Show (or bring forward) the window.
    Open,
    /// Stop the proxy and exit.
    Close,
}

/// The exactly-two right-click menu items, in display order. The first is
/// what a left-click on the icon runs.
pub fn tray_menu_items() -> Vec<TrayMenuItem> {
    vec![
        TrayMenuItem::new(OPEN_ID, "Open Zicade"),
        TrayMenuItem::new(CLOSE_ID, "Close"),
    ]
}

/// Map a clicked menu command id to its [`TrayAction`], or `None` if unknown.
pub fn tray_action(id: u32) -> Option<TrayAction> {
    match id {
        OPEN_ID => Some(TrayAction::Open),
        CLOSE_ID => Some(TrayAction::Close),
        _ => None,
    }
}

/// Run in tray mode until "Close". `show_window` opens the window at start;
/// start-on-login passes `false` so signing in does not pop it up.
pub fn run_tray_mode(show_window: bool) -> ExitCode {
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

    let (observe_layer, logs) = channel_layer(LOG_BUFFER_CAP);
    let log_reload = init_tracing(&config.logging, observe_layer);
    if let Err(err) = tray::hide_console() {
        tracing::warn!(%err, "could not hide the console window");
    }

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (started_tx, started_rx) = mpsc::channel();
    let server = match thread::Builder::new()
        .name("zicade-runtime".to_owned())
        .spawn(move || {
            serve_on_runtime(
                config,
                config_path,
                logs,
                log_reload,
                shutdown_rx,
                started_tx,
            )
        }) {
        Ok(handle) => handle,
        Err(err) => return fail(&format!("could not start the runtime thread: {err}")),
    };
    // A start failure still opens the window, so the user sees why instead of
    // a tray icon that does nothing.
    let control = started_rx
        .recv()
        .unwrap_or_else(|_| Err("the proxy runtime stopped before starting".to_owned()));
    let show_window = show_window || control.is_err();
    tracing::info!("zicade started in tray mode");

    let (ui_tx, ui_rx) = async_mpsc::unbounded_channel();
    let tray_shutdown = shutdown_tx.clone();
    if let Err(err) = thread::Builder::new()
        .name("zicade-tray".to_owned())
        .spawn(move || pump_tray(&ui_tx, &tray_shutdown, server))
    {
        let _ = shutdown_tx.send(true);
        return fail(&format!("could not start the tray thread: {err}"));
    }

    zicade_ui::run(Launch {
        control,
        show_window,
        commands: ui_rx,
        reduce_motion: zicade_win::animations_enabled() == Some(false),
    });
    // Only reached if the UI loop returns on its own; stop the proxy with it.
    let _ = shutdown_tx.send(true);
    ExitCode::SUCCESS
}

/// Show the tray icon and pump it until "Close". Close stops the proxy and
/// waits for it to finish before telling the UI to quit, so in-flight
/// connections drain even if the UI ends the process right away.
fn pump_tray(
    ui_tx: &async_mpsc::UnboundedSender<UiCommand>,
    shutdown_tx: &watch::Sender<bool>,
    server: JoinHandle<()>,
) {
    let mut server = Some(server);
    let pumped = tray::run_tray("Zicade proxy", tray_menu_items(), |id| {
        match tray_action(id) {
            Some(TrayAction::Open) => {
                let _ = ui_tx.send(UiCommand::Show);
                TrayControl::Continue
            }
            Some(TrayAction::Close) => {
                let _ = shutdown_tx.send(true);
                if let Some(server) = server.take() {
                    let _ = server.join();
                }
                TrayControl::Quit
            }
            None => TrayControl::Continue,
        }
    });
    if let Err(err) = pumped {
        tracing::error!(%err, "tray icon failed; quitting");
        let _ = shutdown_tx.send(true);
    }
    // The receiver is gone only if the UI already quit; nothing left to tell.
    let _ = ui_tx.send(UiCommand::Quit);
}

/// Build a tokio runtime, start the app, report its [`Control`] (or why it
/// failed) on `started`, then serve until the shutdown signal.
fn serve_on_runtime(
    config: Config,
    config_path: PathBuf,
    logs: LogStore,
    log_reload: zicade::LogReload,
    mut shutdown_rx: watch::Receiver<bool>,
    started: mpsc::Sender<Result<Control, String>>,
) {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = started.send(Err(format!("failed to start the async runtime: {err}")));
            return;
        }
    };
    runtime.block_on(async move {
        let app = match App::start(config, config_path, logs, log_reload).await {
            Ok(app) => app,
            Err(err) => {
                tracing::error!(error = %format!("{err:#}"), "zicade failed to start");
                let _ = started.send(Err(format!("{err:#}")));
                return;
            }
        };
        tracing::info!(proxy = %app.proxy_addr(), "proxy listening");
        let _ = started.send(Ok(app.control()));
        let shutdown = async move {
            let _ = shutdown_rx.wait_for(|flag| *flag).await;
        };
        if let Err(err) = app.run(shutdown).await {
            tracing::error!(error = %format!("{err:#}"), "tray-mode server exited with an error");
        }
    });
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
    fn menu_has_exactly_open_and_close_with_open_first() {
        // Left-click runs the first item, so "Open Zicade" must lead.
        let items = tray_menu_items();
        assert_eq!(items.len(), 2, "tray menu must have exactly two items");
        assert_eq!(items[0].id, OPEN_ID);
        assert_eq!(items[0].label, "Open Zicade");
        assert_eq!(items[1].id, CLOSE_ID);
        assert_eq!(items[1].label, "Close");
    }

    #[test]
    fn ids_map_to_their_actions() {
        assert_eq!(tray_action(OPEN_ID), Some(TrayAction::Open));
        assert_eq!(tray_action(CLOSE_ID), Some(TrayAction::Close));
    }

    #[test]
    fn unknown_id_maps_to_no_action() {
        assert_eq!(tray_action(0), None);
        assert_eq!(tray_action(999), None);
    }
}
