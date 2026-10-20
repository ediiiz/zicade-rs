#![forbid(unsafe_code)]

//! The native desktop window (gpui-kit): configuration, live status, and logs,
//! all read and written through a [`zicade_control::Control`] handle.
//!
//! [`run`] takes over the calling thread — it must be the main thread — and
//! returns only if the UI loop ends. The window is opened and brought forward
//! on [`UiCommand::Show`]; closing it leaves the app (and the proxy) running.
//! Started without the window, the UI toolkit itself waits for the first
//! [`UiCommand::Show`], so a tray app nobody has opened stays small.

mod assets;
pub mod chart_model;
pub mod choices;
pub mod connection_rates;
pub mod form_model;
pub mod log_buffer;
mod shell;
pub mod throughput;
mod views;

pub use shell::{MainWindow, follow_tray, wait_for_show};
pub use views::{MainView, Page, bind_keys};

use tokio::sync::mpsc::UnboundedReceiver;
use zicade_control::Control;

/// What the tray asks of the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiCommand {
    /// Open the window, or bring it forward if it is already open.
    Show,
    /// End the UI loop; the proxy has already been stopped.
    Quit,
}

/// Everything the UI needs to start.
pub struct Launch {
    /// The running app, or why it failed to start (shown in the window).
    pub control: Result<Control, String>,
    /// Open the window right away rather than waiting for [`UiCommand::Show`]
    /// (which also defers starting the UI toolkit until then).
    pub show_window: bool,
    /// Commands from the tray.
    pub commands: UnboundedReceiver<UiCommand>,
    /// Hold still: the system asked for less motion (Windows' "Show
    /// animations" is off), so animated values jump to their targets.
    pub reduce_motion: bool,
}

/// Run the UI loop on this (the main) thread.
pub fn run(launch: Launch) {
    shell::run(launch);
}
