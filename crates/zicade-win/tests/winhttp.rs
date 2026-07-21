//! M4 acceptance tests for the real WinHTTP PAC backend (`zicade-win`).
//!
//! The non-gated Windows test proves `WinHttpOpen` links and that `resolve`
//! returns a `Result` without panicking (WPAD state varies per machine, so we
//! assert only that it does not panic). The live test is gated behind
//! `ZICADE_LIVE_PROXY=1` and skips explicitly otherwise. Off-Windows we assert
//! the stub reports the platform is unsupported.

use zicade_routing::{PacBackend, RoutingError};
use zicade_win::WinHttpPacBackend;

#[cfg(windows)]
#[test]
fn winhttp_backend_constructs_and_resolves_without_panic() {
    let backend = WinHttpPacBackend::new().expect("WinHttpOpen should succeed on Windows");
    // WPAD/PAC state varies per machine and may be absent offline; we only
    // require that resolution returns a Result and never panics.
    let result = backend.resolve("http://example.com/");
    match result {
        Ok(_) => {}
        Err(RoutingError::Backend(_)) => {}
        Err(other) => panic!("unexpected error variant: {other:?}"),
    }
}

#[cfg(windows)]
#[test]
fn winhttp_live_pac_resolution_gated() {
    // GATED: only runs with ZICADE_LIVE_PROXY=1 on the domain-joined box.
    if std::env::var("ZICADE_LIVE_PROXY").as_deref() != Ok("1") {
        eprintln!("SKIP live PAC: set ZICADE_LIVE_PROXY=1");
        return;
    }

    let backend = WinHttpPacBackend::new().expect("WinHttpOpen should succeed on Windows");

    let external = backend
        .resolve("https://example.com/")
        .expect("live external PAC resolution should succeed");
    eprintln!("live external -> {external:?}");

    let internal = backend
        .resolve("http://internal.corp.local/")
        .expect("live internal PAC resolution should succeed");
    eprintln!("live internal -> {internal:?}");
}

#[cfg(not(windows))]
#[test]
fn winhttp_backend_unsupported_off_windows() {
    assert!(matches!(
        WinHttpPacBackend::new(),
        Err(RoutingError::UnsupportedPlatform)
    ));
}
