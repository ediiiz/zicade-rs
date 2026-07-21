//! Negotiate proxy-auth header parsing and building.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// What a `Proxy-Authenticate` header offers for the Negotiate scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiateOffer {
    /// The header does not offer Negotiate.
    NotOffered,
    /// Negotiate is offered with no token (the initial challenge).
    Offered,
    /// Negotiate is offered with a decoded continuation token.
    Challenge(Vec<u8>),
}

/// Parse a single `Proxy-Authenticate` header value for a Negotiate offer.
pub fn parse_proxy_authenticate(value: &str) -> NegotiateOffer {
    let value = value.trim();
    let mut parts = value.splitn(2, char::is_whitespace);
    let scheme = parts.next().unwrap_or_default();
    if !scheme.eq_ignore_ascii_case("negotiate") {
        return NegotiateOffer::NotOffered;
    }
    match parts.next().map(str::trim).filter(|s| !s.is_empty()) {
        None => NegotiateOffer::Offered,
        Some(token) => match STANDARD.decode(token) {
            Ok(bytes) => NegotiateOffer::Challenge(bytes),
            // A malformed token is treated as a bare offer rather than fatal.
            Err(_) => NegotiateOffer::Offered,
        },
    }
}

/// Build a `Proxy-Authorization` header value carrying a Negotiate token.
pub fn build_proxy_authorization(token: &[u8]) -> String {
    format!("Negotiate {}", STANDARD.encode(token))
}

/// Build a `Proxy-Authorization` header value carrying HTTP Basic credentials
/// (`Basic base64(username:password)`, per RFC 7617). The password may be empty.
pub fn build_basic_authorization(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        STANDARD.encode(format!("{username}:{password}"))
    )
}
