//! Tray-mode orchestration for the binary (double-click launch).
//!
//! The right-click menu has exactly two items — "Open WebUI" and "Close" — and
//! the id -> action mapping is pure logic ([`tray_action`]) so it can be tested
//! without a desktop session. The orchestration ([`run_tray_mode`]) hides the
//! console, runs the proxy + web server on a background tokio runtime, and pumps
//! the tray on the main thread, feeding the existing graceful-shutdown signal
//! when "Close" is chosen.

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
