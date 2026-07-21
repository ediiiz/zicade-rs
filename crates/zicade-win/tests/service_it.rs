//! Gated SCM integration test: a real install -> uninstall round-trip against
//! the live Service Control Manager.
//!
//! Creating and deleting a service requires **Administrator** and a real SCM,
//! so this cannot run in the normal hermetic suite. It is gated behind
//! `ZICADE_SERVICE_IT=1` (mirroring the `ZICADE_LIVE_PROXY` pattern) and marked
//! `#[ignore]`, so `cargo test` stays admin-free. To run it:
//!
//! ```text
//! ZICADE_SERVICE_IT=1 cargo test -p zicade-win --test service_it -- --ignored
//! ```
//!
//! (from an elevated shell). It skips explicitly when the gate is unset rather
//! than silently passing. Off-Windows the stub-error path is asserted instead.

#[cfg(windows)]
#[test]
#[ignore = "requires Administrator + a real SCM; gate with ZICADE_SERVICE_IT=1"]
fn install_then_uninstall_round_trip() {
    use std::path::PathBuf;
    use zicade_win::service::{ServiceInstall, install, uninstall};

    if std::env::var("ZICADE_SERVICE_IT").as_deref() != Ok("1") {
        eprintln!(
            "SKIP install_then_uninstall_round_trip: set ZICADE_SERVICE_IT=1 and run elevated \
             (this creates and deletes a real service)"
        );
        return;
    }

    // A distinctive name so a stale entry from a crashed run is obvious.
    let name = "ZicadeServiceItTest";
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("zicade.exe"));

    let config = ServiceInstall {
        name: name.to_owned(),
        display_name: "Zicade Service IT Test".to_owned(),
        description: "Temporary service created by zicade-win's integration test.".to_owned(),
        exe_path: exe,
        args: vec!["service".to_owned(), "run".to_owned()],
    };

    install(&config).unwrap_or_else(|e| panic!("install '{name}': {e}"));
    // Deleting immediately proves OpenServiceW + DeleteService; we never start
    // it, so no dispatcher/runtime is involved.
    uninstall(name).unwrap_or_else(|e| panic!("uninstall '{name}': {e}"));

    eprintln!("install_then_uninstall_round_trip: '{name}' created and deleted");
}

#[cfg(not(windows))]
#[test]
fn service_unsupported_off_windows() {
    use zicade_win::WinError;
    use zicade_win::service::uninstall;
    assert_eq!(uninstall("Zicade"), Err(WinError::UnsupportedPlatform));
}
