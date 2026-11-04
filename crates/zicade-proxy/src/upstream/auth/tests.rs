use std::sync::Arc;

use zicade_auth::header::AuthScheme;
use zicade_auth::{AuthError, UpstreamAuthenticator};

use super::{AuthState, negotiate_offer, spnego_under_ntlm};
use crate::upstream::{AuthFactory, UpstreamAuth};

/// Hands out `tok1`, `tok2`, ... whatever the challenge.
struct CountingAuth(u32);

impl UpstreamAuthenticator for CountingAuth {
    fn step(&mut self, _challenge: Option<&[u8]>) -> Result<Vec<u8>, AuthError> {
        self.0 += 1;
        Ok(format!("tok{}", self.0).into_bytes())
    }
}

fn negotiate_state() -> AuthState {
    let factory: AuthFactory =
        Arc::new(|| Box::new(CountingAuth(0)) as Box<dyn UpstreamAuthenticator + Send>);
    AuthState::new(&UpstreamAuth::Negotiate(factory))
}

fn offers(values: &[&str]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|v| ("proxy-authenticate".to_owned(), (*v).to_owned()))
        .collect()
}

fn keyword(header: &str) -> &str {
    header.split_whitespace().next().unwrap_or_default()
}

#[test]
fn offer_is_found_after_a_scheme_we_cannot_answer() {
    let offer = negotiate_offer(&offers(&["Basic realm=\"x\"", "NTLM"]));
    assert_eq!(offer.scheme(), Some(AuthScheme::Ntlm));
}

#[test]
fn offer_is_found_before_a_scheme_we_cannot_answer() {
    let offer = negotiate_offer(&offers(&["NTLM", "Basic realm=\"x\""]));
    assert_eq!(offer.scheme(), Some(AuthScheme::Ntlm));
}

#[test]
fn offer_is_found_inside_a_comma_separated_value() {
    // `bG9s` is base64 for `lol`.
    let offer = negotiate_offer(&offers(&[
        "Basic realm=\"x\", charset=\"UTF-8\", NTLM bG9s",
    ]));
    assert_eq!(offer.scheme(), Some(AuthScheme::Ntlm));
    assert_eq!(offer.challenge(), Some(&b"lol"[..]));
}

#[test]
fn other_headers_are_not_offers() {
    let headers = vec![("www-authenticate".to_owned(), "Negotiate".to_owned())];
    assert_eq!(negotiate_offer(&headers).scheme(), None);
}

#[test]
fn first_leg_answers_with_the_advertised_keyword() {
    let mut state = negotiate_state();
    let header = state
        .on_challenge(&offers(&["Basic realm=\"x\"", "NTLM"]))
        .unwrap();
    assert_eq!(keyword(&header), "NTLM");
}

#[test]
fn later_legs_keep_the_first_legs_keyword() {
    let mut state = negotiate_state();
    let first = state.on_challenge(&offers(&["NTLM"])).unwrap();
    let second = state.on_challenge(&offers(&["Negotiate bG9s"])).unwrap();
    assert_eq!((keyword(&first), keyword(&second)), ("NTLM", "NTLM"));
}

#[test]
fn first_leg_without_negotiate_or_ntlm_fails_naming_the_offers() {
    let mut state = negotiate_state();
    let err = state
        .on_challenge(&offers(&[
            "Basic realm=\"a, b\"",
            "Digest realm=\"x\", qop=\"auth\"",
        ]))
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "upstream offers no Negotiate/NTLM proxy auth (offered: Basic, Digest)"
    );
}

#[test]
fn first_leg_naming_no_scheme_answers_negotiate() {
    let mut state = negotiate_state();
    let header = state.on_challenge(&[]).unwrap();
    assert_eq!(keyword(&header), "Negotiate");
}

#[test]
fn spnego_token_under_ntlm_is_flagged() {
    // A SPNEGO NegTokenInit opens with the ASN.1 application tag 0x60.
    let spnego = [0x60, 0x48, 0x06, 0x06];
    let raw_ntlm = b"NTLMSSP\0\x01\x00\x00\x00";
    assert!(spnego_under_ntlm(AuthScheme::Ntlm, &spnego));
    assert!(!spnego_under_ntlm(AuthScheme::Ntlm, raw_ntlm));
    assert!(!spnego_under_ntlm(AuthScheme::Negotiate, &spnego));
}
