//! M3 tests for Negotiate/NTLM proxy-auth header parsing/building.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use zicade_auth::header::{
    AuthScheme, NegotiateOffer, build_basic_authorization, build_proxy_authorization,
    parse_proxy_authenticate,
};

#[test]
fn parses_bare_offer() {
    assert_eq!(
        parse_proxy_authenticate("Negotiate"),
        NegotiateOffer::Offered(AuthScheme::Negotiate)
    );
}

#[test]
fn parses_bare_ntlm_offer() {
    // Gateways that expose the SSPI handshake under the NTLM keyword send this.
    assert_eq!(
        parse_proxy_authenticate("NTLM"),
        NegotiateOffer::Offered(AuthScheme::Ntlm)
    );
}

#[test]
fn parses_ntlm_challenge_token() {
    let tok = b"\x01\x02\x03ntlm-challenge";
    let header = format!("NTLM {}", STANDARD.encode(tok));
    match parse_proxy_authenticate(&header) {
        NegotiateOffer::Challenge(scheme, bytes) => {
            assert_eq!(scheme, AuthScheme::Ntlm);
            assert_eq!(bytes, tok);
        }
        other => panic!("expected Challenge, got {other:?}"),
    }
}

#[test]
fn offer_exposes_scheme_and_challenge() {
    let tok = b"bytes";
    let header = format!("Negotiate {}", STANDARD.encode(tok));
    let offer = parse_proxy_authenticate(&header);
    assert_eq!(offer.scheme(), Some(AuthScheme::Negotiate));
    assert_eq!(offer.challenge(), Some(&tok[..]));

    let bare = parse_proxy_authenticate("NTLM");
    assert_eq!(bare.scheme(), Some(AuthScheme::Ntlm));
    assert_eq!(bare.challenge(), None);

    assert_eq!(NegotiateOffer::NotOffered.scheme(), None);
}

#[test]
fn parses_challenge_token() {
    let tok = b"\x01\x02\x03challenge-bytes";
    let header = format!("Negotiate {}", STANDARD.encode(tok));
    match parse_proxy_authenticate(&header) {
        NegotiateOffer::Challenge(bytes_scheme, bytes) => {
            assert_eq!(bytes_scheme, AuthScheme::Negotiate);
            assert_eq!(bytes, tok);
        }
        other => panic!("expected Challenge, got {other:?}"),
    }
}

#[test]
fn scheme_match_is_case_insensitive() {
    assert_eq!(
        parse_proxy_authenticate("negotiate"),
        NegotiateOffer::Offered(AuthScheme::Negotiate)
    );
    assert_eq!(
        parse_proxy_authenticate("ntlm"),
        NegotiateOffer::Offered(AuthScheme::Ntlm)
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
    let header = build_proxy_authorization(AuthScheme::Negotiate, tok);
    assert_eq!(header, format!("Negotiate {}", STANDARD.encode(tok)));
    match parse_proxy_authenticate(&header) {
        NegotiateOffer::Challenge(AuthScheme::Negotiate, bytes) => assert_eq!(bytes, tok),
        other => panic!("expected Challenge, got {other:?}"),
    }
}

#[test]
fn ntlm_build_then_parse_round_trips() {
    // The keyword must survive the round trip: gateways that advertise NTLM
    // reject a `Negotiate`-prefixed reply outright.
    let tok = b"ntlm-token-bytes";
    let header = build_proxy_authorization(AuthScheme::Ntlm, tok);
    assert_eq!(header, format!("NTLM {}", STANDARD.encode(tok)));
    match parse_proxy_authenticate(&header) {
        NegotiateOffer::Challenge(AuthScheme::Ntlm, bytes) => assert_eq!(bytes, tok),
        other => panic!("expected Challenge, got {other:?}"),
    }
}
