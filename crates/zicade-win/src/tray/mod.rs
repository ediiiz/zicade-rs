//! System-tray (notification-area) integration and the small console helpers
//! the double-click launch heuristic needs.
//!
//! The public surface here is cross-platform and safe; all Win32 FFI lives in
//! [`win`] behind `cfg(windows)` and degrades to
//! [`WinError::UnsupportedPlatform`] off-Windows.
//!
//! The Win32 message pump is inherently single-threaded — window messages must
//! be pumped on the thread that created the window, and the window/icon handles
//! are `!Send`. So [`run_tray`] **owns its calling thread** until a menu handler
//! returns [`TrayControl::Quit`]; the binary gives it a thread of its own, since
//! the UI owns the main thread and the proxy runs on a tokio thread.

use crate::WinError;

#[cfg(windows)]
mod win;

/// Whether the tray message pump should keep running or exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayControl {
    /// Keep pumping messages (the icon stays in the tray).
    Continue,
    /// Tear down the icon and return from [`run_tray`].
    Quit,
}

/// One right-click menu entry: a stable command id plus its label.
///
/// The id is what [`run_tray`]'s `on_event` receives when the entry is clicked,
/// so callers keep their own id → action mapping (pure and testable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayMenuItem {
    /// Command id reported to `on_event` when this item is chosen. Must be
    /// non-zero (Win32 `TrackPopupMenu` uses 0 to mean "nothing selected").
    pub id: u32,
    /// Human-readable menu label.
    pub label: String,
}

impl TrayMenuItem {
    /// Create a menu item with the given command `id` and `label`.
    pub fn new(id: u32, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
        }
    }
}

/// Number of processes attached to this process's console.
///
/// This is `GetConsoleProcessList`'s result: `1` means the process owns a
/// freshly-created console (a double-click of the exe), any larger value means
/// the console is shared with a parent shell. Off-Windows returns `0`.
pub fn console_process_count() -> usize {
    #[cfg(windows)]
    {
        win::console_process_count()
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// Hide and detach this process's console window so nothing lingers on screen
/// once the app switches to tray mode.
///
/// Off-Windows returns [`WinError::UnsupportedPlatform`].
pub fn hide_console() -> Result<(), WinError> {
    #[cfg(windows)]
    {
        win::hide_console()
    }
    #[cfg(not(windows))]
    {
        Err(WinError::UnsupportedPlatform)
    }
}

/// Show a tray icon with the given `tooltip` and right-click `items`, then pump
/// window messages on the calling thread until a menu handler returns
/// [`TrayControl::Quit`].
///
/// `on_event` is invoked with the [`TrayMenuItem::id`] of the clicked entry; a
/// left-click on the icon reports the first item's id. It runs on this same
/// thread, so it must not block for long. Any thread may run the tray, as long
/// as it does nothing else. Off-Windows returns
/// [`WinError::UnsupportedPlatform`].
pub fn run_tray<F>(tooltip: &str, items: Vec<TrayMenuItem>, on_event: F) -> Result<(), WinError>
where
    F: FnMut(u32) -> TrayControl,
{
    #[cfg(windows)]
    {
        win::run_tray(tooltip, items, on_event)
    }
    #[cfg(not(windows))]
    {
        let _ = (tooltip, items, on_event);
        Err(WinError::UnsupportedPlatform)
    }
}
