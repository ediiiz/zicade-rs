//! M4 acceptance tests for the real WinHTTP PAC backend (`zicade-win`).
//!
//! The non-gated Windows test proves `WinHttpOpen` links and that `resolve`
//! returns a `Result` without panicking (WPAD state varies per machine, so we
//! assert only that it does not panic). The live test is gated behind
//! `ZICADE_LIVE_PROXY=1` and exercises the `source = "auto"` discovery path
//! (per-user AutoConfigURL -> WPAD -> static proxy). Off-Windows we assert the
//! stubs report the platform is unsupported.

use zicade_routing::{PacBackend, PacResult, RoutingError};
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
fn winhttp_from_system_constructs_and_resolves_without_panic() {
    // The discovery constructor reads the per-user IE proxy config. Its outcome
    // is environment-dependent (AutoConfigURL / WPAD / static proxy / none), so
    // hermetically we only require it constructs and resolves without panicking.
    let backend =
        WinHttpPacBackend::from_system().expect("from_system should construct on Windows");
    match backend.resolve("http://example.com/") {
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

    // Exercise the real discovery path `source = "auto"` uses: read the per-user
    // IE proxy config, then AutoConfigURL -> WPAD -> static proxy -> DIRECT.
    let backend =
        WinHttpPacBackend::from_system().expect("from_system should construct on Windows");

    // An external host: on a box with a PAC/AutoConfigURL or a static proxy this
    // resolves to a Proxy; a box with nothing configured legitimately resolves
    // DIRECT (from_system maps "no config" to DIRECT). Either is valid; we log
    // the real decision and require the call to return cleanly (never panic).
    let external = backend.resolve("https://example.com/");
    match &external {
        Ok(PacResult::Proxy { host, port }) => {
            eprintln!("live PAC https://example.com/ -> PROXY {host}:{port}");
        }
        Ok(PacResult::Direct) => {
            eprintln!(
                "live PAC https://example.com/ -> DIRECT (no AutoConfigURL/WPAD/static proxy \
                 discovered for this user)"
            );
        }
        Err(RoutingError::Backend(msg)) => {
            // A discovered-but-unreachable PAC/WPAD script surfaces here; that is
            // a real environment state, not a test failure.
            eprintln!("live PAC https://example.com/: backend error ({msg})");
        }
        Err(other) => panic!("unexpected PAC error variant: {other:?}"),
    }

    // A likely-internal host, purely for logging (bypass lists / DIRECT rules).
    match backend.resolve("http://internal.corp.local/") {
        Ok(decision) => eprintln!("live PAC http://internal.corp.local/ -> {decision:?}"),
        Err(RoutingError::Backend(msg)) => {
            eprintln!("live PAC http://internal.corp.local/: backend error ({msg})");
        }
        Err(other) => panic!("unexpected PAC error variant: {other:?}"),
    }
}

#[cfg(not(windows))]
#[test]
fn winhttp_backend_unsupported_off_windows() {
    assert!(matches!(
        WinHttpPacBackend::new(),
        Err(RoutingError::UnsupportedPlatform)
    ));
    assert!(matches!(
        WinHttpPacBackend::with_config_url("http://wp/proxy.pac"),
        Err(RoutingError::UnsupportedPlatform)
    ));
    assert!(matches!(
        WinHttpPacBackend::from_system(),
        Err(RoutingError::UnsupportedPlatform)
    ));
}
