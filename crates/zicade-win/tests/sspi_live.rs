//! Gated pure-SSPI live check: produce a real Negotiate initial token for the
//! corporate upstream's SPN on the domain-joined box.
//!
//! This isolates the SSPI leg from the proxy/network path in [`live`] — it
//! proves `AcquireCredentialsHandle` + `InitializeSecurityContext` yield a
//! genuine outbound token for `HTTP/<upstream-host>` without any socket. Gated
//! behind `ZICADE_LIVE_PROXY=1`; skips explicitly (never silently passes)
//! otherwise. Off-Windows the pure stub-error path is asserted instead.

#[cfg(windows)]
#[test]
fn sspi_negotiate_initial_token_gated() {
    use zicade_auth::UpstreamAuthenticator as _;
    use zicade_win::SspiNegotiate;

    // GATED: only runs with ZICADE_LIVE_PROXY=1 on the domain-joined box.
    if std::env::var("ZICADE_LIVE_PROXY").as_deref() != Ok("1") {
        eprintln!(
            "SKIP sspi_negotiate_initial_token_gated: set ZICADE_LIVE_PROXY=1 (on corp, \
             domain-joined) to run"
        );
        return;
    }

    // Derive the SPN host from the same knob the proxy suite uses.
    let upstream =
        std::env::var("ZICADE_LIVE_UPSTREAM").unwrap_or_else(|_| "wp8080:8080".to_owned());
    let host = upstream
        .rsplit_once(':')
        .map_or(upstream.as_str(), |(h, _)| h);
    let spn = format!("HTTP/{host}");

    let mut auth = SspiNegotiate::new(Some(&spn), zicade_win::SspiPackage::Ntlm)
        .expect("acquire Negotiate credential handle");
    let token = auth
        .step(None)
        .expect("SSPI must produce an initial Negotiate token");
    assert!(
        !token.is_empty(),
        "initial Negotiate token for {spn} must be non-empty"
    );
    eprintln!(
        "sspi_negotiate_initial_token_gated: {spn} -> {} token bytes",
        token.len()
    );
}

#[cfg(not(windows))]
#[test]
fn sspi_negotiate_unsupported_off_windows() {
    use zicade_win::{SspiNegotiate, SspiPackage, WinError};
    assert_eq!(
        SspiNegotiate::new(Some("HTTP/wp8080"), SspiPackage::Ntlm).err(),
        Some(WinError::UnsupportedPlatform)
    );
}
