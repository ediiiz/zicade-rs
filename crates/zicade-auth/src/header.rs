//! Proxy-auth header parsing and building for the Negotiate/NTLM schemes.
//!
//! Some gateways advertise connection auth as `Proxy-Authenticate: NTLM` rather
//! than `Negotiate`, and reject a reply under any keyword but the one they
//! advertised. The keyword is therefore carried with the handshake instead of
//! being hard-coded.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// Which proxy-auth scheme the upstream gateway is speaking.
///
/// It decides the keyword written into `Proxy-Authorization`. A raw NTLM token
/// is accepted under either keyword; a SPNEGO token only under `Negotiate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthScheme {
    /// `Proxy-Authenticate: Negotiate` (RFC 4559 spelling).
    #[default]
    Negotiate,
    /// `Proxy-Authenticate: NTLM`.
    Ntlm,
}

impl AuthScheme {
    /// The keyword used in the `Proxy-Authorization` header.
    #[must_use]
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Negotiate => "Negotiate",
            Self::Ntlm => "NTLM",
        }
    }
}

/// What a `Proxy-Authenticate` header offers for the Negotiate/NTLM schemes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiateOffer {
    /// The header does not offer either scheme.
    NotOffered,
    /// A scheme is offered with no token (the initial challenge).
    Offered(AuthScheme),
    /// A scheme is offered with a decoded continuation token.
    Challenge(AuthScheme, Vec<u8>),
}

impl NegotiateOffer {
    /// The scheme this offer belongs to, if any.
    #[must_use]
    pub fn scheme(&self) -> Option<AuthScheme> {
        match self {
            Self::NotOffered => None,
            Self::Offered(s) | Self::Challenge(s, _) => Some(*s),
        }
    }

    /// The continuation token, when the offer carries one.
    #[must_use]
    pub fn challenge(&self) -> Option<&[u8]> {
        match self {
            Self::Challenge(_, bytes) => Some(bytes),
            Self::Offered(_) | Self::NotOffered => None,
        }
    }
}

/// Parse a single `Proxy-Authenticate` header value for a Negotiate/NTLM offer.
pub fn parse_proxy_authenticate(value: &str) -> NegotiateOffer {
    let value = value.trim();
    let mut parts = value.splitn(2, char::is_whitespace);
    let scheme = parts.next().unwrap_or_default();
    let scheme = if scheme.eq_ignore_ascii_case("negotiate") {
        AuthScheme::Negotiate
    } else if scheme.eq_ignore_ascii_case("ntlm") {
        AuthScheme::Ntlm
    } else {
        return NegotiateOffer::NotOffered;
    };
    match parts.next().map(str::trim).filter(|s| !s.is_empty()) {
        None => NegotiateOffer::Offered(scheme),
        Some(token) => match STANDARD.decode(token) {
            Ok(bytes) => NegotiateOffer::Challenge(scheme, bytes),
            // A malformed token is treated as a bare offer rather than fatal.
            Err(_) => NegotiateOffer::Offered(scheme),
        },
    }
}

/// Build a `Proxy-Authorization` header value carrying a token for `scheme`.
pub fn build_proxy_authorization(scheme: AuthScheme, token: &[u8]) -> String {
    format!("{} {}", scheme.keyword(), STANDARD.encode(token))
}

/// Build a `Proxy-Authorization` header value carrying HTTP Basic credentials
/// (`Basic base64(username:password)`, per RFC 7617). The password may be empty.
pub fn build_basic_authorization(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        STANDARD.encode(format!("{username}:{password}"))
    )
}
