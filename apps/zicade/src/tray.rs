//! Tray-mode orchestration for the binary (double-click launch).
//!
//! The right-click menu has exactly two items — "Open WebUI" and "Close" — and
//! the id -> action mapping is pure logic ([`tray_action`]) so it can be tested
//! without a desktop session. The orchestration ([`run_tray_mode`]) hides the
//! console, runs the proxy + web server on a background tokio runtime, and pumps
//! the tray on the main thread, feeding the existing graceful-shutdown signal
//! when "Close" is chosen.

use zicade_win::tray::TrayMenuItem;

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
}
