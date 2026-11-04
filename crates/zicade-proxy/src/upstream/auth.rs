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
    /// `scheme` is learned from the gateway's first `Proxy-Authenticate` and
    /// pinned for the rest of this connection's handshake.
    Negotiate {
        handshake: NegotiateHandshake<BoxAuth>,
        scheme: AuthScheme,
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
                // Overridden by the first 407 that names a scheme.
                scheme: AuthScheme::default(),
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
            AuthState::Negotiate { handshake, scheme } => {
                let offer = negotiate_offer(headers);
                // Pin the keyword the gateway asked for; it may advertise the
                // same SSPI handshake as `NTLM` instead of `Negotiate`.
                if let Some(advertised) = offer.scheme() {
                    *scheme = advertised;
                }
                let pinned = *scheme;
                let token = advance_handshake(handshake, offer.challenge())?;
                Ok(build_proxy_authorization(pinned, &token))
            }
        }
    }
}

/// The first `Proxy-Authenticate` entry that offers Negotiate or NTLM.
///
/// A single 407 can carry several headers (e.g. `NTLM` and `Basic`); only the
/// scheme we can answer is considered, and a later header must not mask an
/// earlier one.
fn negotiate_offer(headers: &[(String, String)]) -> NegotiateOffer {
    headers
        .iter()
        .filter(|(k, _)| k == "proxy-authenticate")
        .map(|(_, v)| parse_proxy_authenticate(v))
        .find(|offer| !matches!(offer, NegotiateOffer::NotOffered))
        .unwrap_or(NegotiateOffer::NotOffered)
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
