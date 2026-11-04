//! Per-connection upstream auth driver: selects the `Proxy-Authorization`
//! scheme and produces each 407 leg's header value for the handshake loops in
//! the parent module.

use std::io;

use zicade_auth::header::{
    AuthScheme, NegotiateOffer, build_proxy_authorization, parse_proxy_authenticate,
};
use zicade_auth::{
    AuthError, HandshakeStep, NegotiateHandshake, UpstreamAuthenticator, UpstreamResponse,
};

use super::UpstreamAuth;

/// Adapts a boxed trait object into a concrete [`UpstreamAuthenticator`] so it
/// can drive the generic [`NegotiateHandshake`]. Same visibility as
/// [`AuthState`], whose `Negotiate` variant embeds it.
pub(super) struct BoxAuth(Box<dyn UpstreamAuthenticator + Send>);

impl UpstreamAuthenticator for BoxAuth {
    fn step(&mut self, challenge: Option<&[u8]>) -> Result<Vec<u8>, AuthError> {
        self.0.step(challenge)
    }
}

/// Per-connection auth driver: chooses the `Proxy-Authorization` scheme and
/// produces each leg's header value. This is where the scheme is selected by
/// auth type — Negotiate/NTLM builds `<scheme> <token>`, Basic sends its
/// ready-made `Basic <b64>` value.
pub(super) enum AuthState {
    None,
    Basic {
        credentials: String,
        sent: bool,
    },
    /// `scheme` is `None` until the connection's first 407, then fixed: the
    /// keyword a gateway advertises there is the one it expects on every leg.
    Negotiate {
        handshake: NegotiateHandshake<BoxAuth>,
        scheme: Option<AuthScheme>,
    },
}

impl AuthState {
    pub(super) fn new(auth: &UpstreamAuth) -> Self {
        match auth {
            UpstreamAuth::None => AuthState::None,
            UpstreamAuth::Basic { credentials } => AuthState::Basic {
                credentials: credentials.clone(),
                sent: false,
            },
            UpstreamAuth::Negotiate(factory) => AuthState::Negotiate {
                handshake: NegotiateHandshake::new(BoxAuth(factory())),
                scheme: None,
            },
        }
    }

    /// The `Proxy-Authorization` value to send preemptively on the first leg.
    /// Only Basic sends preemptively; Negotiate opens with no auth.
    pub(super) fn initial_header(&mut self) -> Option<String> {
        match self {
            AuthState::Basic { credentials, sent } => {
                *sent = true;
                Some(credentials.clone())
            }
            AuthState::None | AuthState::Negotiate { .. } => None,
        }
    }

    /// Produce the next `Proxy-Authorization` value in response to a `407`, or an
    /// error if this auth mode cannot (or should not) answer another challenge.
    /// For Basic, a `407` after the credential was already sent means the
    /// credential is wrong: fail cleanly rather than resend and loop.
    pub(super) fn on_challenge(&mut self, headers: &[(String, String)]) -> io::Result<String> {
        match self {
            AuthState::None => Err(io::Error::other(
                "upstream demanded proxy auth but none is configured",
            )),
            AuthState::Basic { credentials, sent } => {
                if *sent {
                    Err(io::Error::other(
                        "upstream rejected Basic proxy credentials",
                    ))
                } else {
                    *sent = true;
                    Ok(credentials.clone())
                }
            }
            AuthState::Negotiate { handshake, scheme } => negotiate_leg(handshake, scheme, headers),
        }
    }
}

/// Answer one 407 of a Negotiate/NTLM handshake. The first leg learns the
/// keyword; later legs keep it even if the gateway names the other one, since
/// switching keywords mid-handshake makes the gateway restart or reject it.
fn negotiate_leg(
    handshake: &mut NegotiateHandshake<BoxAuth>,
    scheme: &mut Option<AuthScheme>,
    headers: &[(String, String)],
) -> io::Result<String> {
    let offer = negotiate_offer(headers);
    let first_leg = scheme.is_none();
    let keyword = match *scheme {
        Some(learned) => learned,
        None => first_leg_scheme(headers, &offer)?,
    };
    *scheme = Some(keyword);
    let token = advance_handshake(handshake, offer.challenge())?;
    if first_leg && spnego_under_ntlm(keyword, &token) {
        tracing::warn!(
            "upstream advertises NTLM proxy auth but the SSPI package produced a \
             SPNEGO token, which NTLM-only gateways reject; set the upstream auth \
             package to \"ntlm\""
        );
    }
    Ok(build_proxy_authorization(keyword, &token))
}

/// The keyword to answer a connection's first 407 with.
///
/// A 407 that offers schemes, none of them Negotiate or NTLM, cannot be
/// answered: sending a token anyway lets a gateway that ignores it stall the
/// handshake until the timeout, so fail now and name what it did offer. A 407
/// with no `Proxy-Authenticate` at all rules nothing out, so it gets the
/// RFC 4559 `Negotiate` keyword the configured auth mode implies.
fn first_leg_scheme(
    headers: &[(String, String)],
    offer: &NegotiateOffer,
) -> io::Result<AuthScheme> {
    if let Some(advertised) = offer.scheme() {
        return Ok(advertised);
    }
    let offered: Vec<&str> = challenges(headers).map(scheme_name).collect();
    if offered.is_empty() {
        return Ok(AuthScheme::Negotiate);
    }
    Err(io::Error::other(format!(
        "upstream offers no Negotiate/NTLM proxy auth (offered: {})",
        offered.join(", ")
    )))
}

/// The first challenge that offers Negotiate or NTLM, across every
/// `Proxy-Authenticate` header: a 407 may list `Basic` before `NTLM`, in
/// separate headers or one comma-separated value, and the scheme we can answer
/// must not be masked by one we cannot.
fn negotiate_offer(headers: &[(String, String)]) -> NegotiateOffer {
    challenges(headers)
        .map(parse_proxy_authenticate)
        .find(|offer| !matches!(offer, NegotiateOffer::NotOffered))
        .unwrap_or(NegotiateOffer::NotOffered)
}

/// Each challenge (`<scheme> [token68 | auth-params]`) in the
/// `Proxy-Authenticate` headers. A value may hold several challenges separated
/// by commas, which also separate a challenge's auth-params; a comma-separated
/// piece starts a new challenge only when its first word is a bare token
/// rather than a `name=value` param.
fn challenges(headers: &[(String, String)]) -> impl Iterator<Item = &str> {
    headers
        .iter()
        .filter(|(k, _)| k == "proxy-authenticate")
        .flat_map(|(_, v)| v.split(','))
        .map(str::trim)
        .filter(|piece| {
            let name = scheme_name(piece);
            !name.is_empty() && name.bytes().all(is_tchar)
        })
}

fn scheme_name(challenge: &str) -> &str {
    challenge.split_whitespace().next().unwrap_or_default()
}

/// RFC 9110 `tchar`: the characters an auth-scheme name may contain.
fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// Every raw NTLM message starts with this; SPNEGO tokens are ASN.1 instead.
const NTLMSSP_SIGNATURE: &[u8] = b"NTLMSSP\0";

/// Whether we are about to send a SPNEGO token under the `NTLM` keyword. A
/// gateway advertising `NTLM` expects raw NTLM messages, but the `Negotiate`
/// SSPI package wraps them in SPNEGO. The package is chosen in the app's
/// config, so the token itself is inspected rather than threading the package
/// through every layer.
fn spnego_under_ntlm(scheme: AuthScheme, token: &[u8]) -> bool {
    scheme == AuthScheme::Ntlm && !token.starts_with(NTLMSSP_SIGNATURE)
}

fn auth_io_err(err: AuthError) -> io::Error {
    io::Error::other(format!("upstream negotiate handshake failed: {err}"))
}

/// Drive one 407 leg: feed the challenge to the handshake and return the next
/// SSPI token, or an error if the handshake has no more work.
fn advance_handshake(
    handshake: &mut NegotiateHandshake<BoxAuth>,
    challenge: Option<&[u8]>,
) -> io::Result<Vec<u8>> {
    match handshake
        .on_response(UpstreamResponse::ProxyAuthRequired {
            challenge: challenge.map(<[u8]>::to_vec),
        })
        .map_err(auth_io_err)?
    {
        HandshakeStep::SendToken(token) => Ok(token),
        HandshakeStep::Complete => Err(io::Error::other(
            "handshake completed while upstream still demanded auth",
        )),
    }
}

#[cfg(test)]
mod tests;
