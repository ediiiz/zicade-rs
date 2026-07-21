//! M3 tests for Negotiate proxy-auth header parsing/building.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use zicade_auth::header::{
    NegotiateOffer, build_basic_authorization, build_proxy_authorization, parse_proxy_authenticate,
};

#[test]
fn parses_bare_offer() {
    assert_eq!(
        parse_proxy_authenticate("Negotiate"),
        NegotiateOffer::Offered
    );
}

#[test]
fn parses_challenge_token() {
    let tok = b"\x01\x02\x03challenge-bytes";
    let header = format!("Negotiate {}", STANDARD.encode(tok));
    match parse_proxy_authenticate(&header) {
        NegotiateOffer::Challenge(bytes) => assert_eq!(bytes, tok),
        other => panic!("expected Challenge, got {other:?}"),
    }
}

#[test]
fn scheme_match_is_case_insensitive() {
    assert_eq!(
        parse_proxy_authenticate("negotiate"),
        NegotiateOffer::Offered
    );
}

#[test]
fn non_negotiate_scheme_is_not_offered() {
    assert_eq!(
        parse_proxy_authenticate("Basic realm=\"x\""),
        NegotiateOffer::NotOffered
    );
}

#[test]
fn builds_basic_authorization_known_vector() {
    // RFC 7617 canonical example.
    assert_eq!(
        build_basic_authorization("Aladdin", "open sesame"),
        "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
    );
}

#[test]
fn builds_basic_authorization_allows_empty_password() {
    let header = build_basic_authorization("user", "");
    assert_eq!(header, format!("Basic {}", STANDARD.encode("user:")));
}

#[test]
fn build_then_parse_round_trips() {
    let tok = b"hello-token-bytes";
    let header = build_proxy_authorization(tok);
    assert_eq!(header, format!("Negotiate {}", STANDARD.encode(tok)));
    match parse_proxy_authenticate(&header) {
        NegotiateOffer::Challenge(bytes) => assert_eq!(bytes, tok),
        other => panic!("expected Challenge, got {other:?}"),
    }
}
