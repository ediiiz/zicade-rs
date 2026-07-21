//! M4 acceptance tests for `zicade-routing` (spec §5.1 PAC mode + LESSON-6).
//! Pure logic, no network, runs on any OS: PAC resolution goes through an
//! injected [`FakeBackend`] rather than the real WinHTTP backend.

use zicade_config::{AuthMode, FailPolicy, PacConfig};
use zicade_routing::{FakeBackend, PacResult, RouteDecision, RouteResolver, RoutingError};

// --- PAC result parsing (pure) ------------------------------------------

#[test]
fn parse_direct() {
    assert_eq!(PacResult::parse("DIRECT"), PacResult::Direct);
}

#[test]
fn parse_proxy() {
    assert_eq!(
        PacResult::parse("PROXY wp8080:8080"),
        PacResult::Proxy {
            host: "wp8080".to_owned(),
            port: 8080
        }
    );
}

#[test]
fn parse_proxy_then_direct_takes_first_proxy() {
    assert_eq!(
        PacResult::parse("PROXY wp8080:8080; DIRECT"),
        PacResult::Proxy {
            host: "wp8080".to_owned(),
            port: 8080
        }
    );
}

#[test]
fn parse_empty_is_direct() {
    assert_eq!(PacResult::parse(""), PacResult::Direct);
}

#[test]
fn parse_whitespace_is_direct() {
    assert_eq!(PacResult::parse("   \t  "), PacResult::Direct);
}

#[test]
fn parse_socks_is_skipped_to_next_proxy() {
    // SOCKS entries are ignored; the first usable PROXY wins.
    assert_eq!(
        PacResult::parse("SOCKS sock5:1080; PROXY wp8080:8080"),
        PacResult::Proxy {
            host: "wp8080".to_owned(),
            port: 8080
        }
    );
}

#[test]
fn parse_bare_winhttp_proxy_list() {
    // WinHttpGetProxyForUrl returns a bare host:port list (no PROXY keyword).
    assert_eq!(
        PacResult::parse("wp8080:8080"),
        PacResult::Proxy {
            host: "wp8080".to_owned(),
            port: 8080
        }
    );
}

// --- Route resolution (LESSON-6) ----------------------------------------

fn pac_negotiate() -> PacConfig {
    let mut pac = PacConfig::default();
    pac.auth.mode = AuthMode::Negotiate;
    pac
}

#[test]
fn external_host_routes_to_upstream_with_pac_auth() {
    // LESSON-6: a PAC-selected upstream inherits routing.pac.auth so the
    // downstream Negotiate handshake can run.
    let backend = FakeBackend::classify("wp8080", 8080);
    let resolver = RouteResolver::new(pac_negotiate(), backend);

    let decision = resolver
        .route("https://external.example.com/")
        .expect("external route resolves");

    match decision {
        RouteDecision::Upstream(up) => {
            assert_eq!(up.host, "wp8080");
            assert_eq!(up.port, 8080);
            assert_eq!(
                up.auth.mode,
                AuthMode::Negotiate,
                "PAC auth must be applied"
            );
        }
        other => panic!("expected Upstream, got {other:?}"),
    }
}

#[test]
fn internal_host_routes_direct_no_auth() {
    let backend = FakeBackend::classify("wp8080", 8080);
    let resolver = RouteResolver::new(pac_negotiate(), backend);

    let decision = resolver
        .route("http://internal.corp.local/")
        .expect("internal route resolves");

    assert_eq!(decision, RouteDecision::Direct);
}

#[test]
fn fail_policy_error_propagates() {
    let mut pac = pac_negotiate();
    pac.fail_policy = FailPolicy::Error;
    let resolver = RouteResolver::new(pac, FakeBackend::failing());

    let err = resolver
        .route("https://external.example.com/")
        .expect_err("fail policy error must propagate");
    assert!(matches!(err, RoutingError::Backend(_)));
}

#[test]
fn fail_policy_direct_falls_back() {
    let mut pac = pac_negotiate();
    pac.fail_policy = FailPolicy::Direct;
    let resolver = RouteResolver::new(pac, FakeBackend::failing());

    let decision = resolver
        .route("https://external.example.com/")
        .expect("fail policy direct falls back to DIRECT");
    assert_eq!(decision, RouteDecision::Direct);
}
