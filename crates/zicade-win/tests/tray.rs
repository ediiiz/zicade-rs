//! Tests for the system-tray (notification-area) FFI wrapper (`zicade-win`).
//!
//! The Win32 message pump + `Shell_NotifyIconW` cannot run headless, so the
//! test that pops a *real* tray icon is gated behind `ZICADE_TRAY_IT=1` and
//! marked `#[ignore]` (mirroring `service_it.rs`'s `ZICADE_SERVICE_IT` gate).
//! To run it on a real desktop session:
//!
//! ```text
//! ZICADE_TRAY_IT=1 cargo test -p zicade-win --test tray -- --ignored
//! ```
//!
//! The non-gated Windows tests only prove the wrappers construct/link and that
//! `console_process_count` (the double-click heuristic input) does not panic.
//! Off-Windows we assert the stubs report the platform is unsupported.

use zicade_win::tray::{TrayControl, TrayMenuItem, console_process_count, run_tray};

#[cfg(windows)]
#[test]
fn console_process_count_does_not_panic() {
    // GetConsoleProcessList's value is environment-dependent (>=1 with a
    // console, and the double-click case is exactly 1); we only require the
    // FFI wrapper links and returns without panicking.
    let _ = console_process_count();
}

#[test]
fn menu_item_and_control_model() {
    let item = TrayMenuItem::new(7, "Open WebUI");
    assert_eq!(item.id, 7);
    assert_eq!(item.label, "Open WebUI");
    assert_ne!(TrayControl::Continue, TrayControl::Quit);
}

#[cfg(windows)]
#[test]
#[ignore = "pops a real tray icon; needs a desktop session; gate with ZICADE_TRAY_IT=1"]
fn tray_pops_and_closes_on_close() {
    if std::env::var("ZICADE_TRAY_IT").as_deref() != Ok("1") {
        eprintln!("SKIP tray_pops_and_closes_on_close: set ZICADE_TRAY_IT=1 (desktop session)");
        return;
    }
    // Manual IT: a tray icon appears with exactly "Open WebUI" and "Close".
    // Clicking "Close" (id 2) ends the pump; "Open WebUI" (id 1) keeps it up.
    let items = vec![
        TrayMenuItem::new(1, "Open WebUI"),
        TrayMenuItem::new(2, "Close"),
    ];
    run_tray("Zicade (integration test)", items, |id| {
        eprintln!("tray menu item clicked: {id}");
        if id == 2 {
            TrayControl::Quit
        } else {
            TrayControl::Continue
        }
    })
    .expect("run_tray should pump and return cleanly after Close");
    eprintln!("tray_pops_and_closes_on_close: pump exited");
}

#[cfg(not(windows))]
#[test]
fn tray_unsupported_off_windows() {
    use zicade_win::WinError;
    use zicade_win::tray::{hide_console, open_url};

    assert_eq!(console_process_count(), 0);
    assert_eq!(hide_console(), Err(WinError::UnsupportedPlatform));
    assert_eq!(
        open_url("http://127.0.0.1:3130/"),
        Err(WinError::UnsupportedPlatform)
    );
    let items = vec![TrayMenuItem::new(1, "Open WebUI")];
    assert_eq!(
        run_tray("t", items, |_| TrayControl::Quit),
        Err(WinError::UnsupportedPlatform)
    );
}
