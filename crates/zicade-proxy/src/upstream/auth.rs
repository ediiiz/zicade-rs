//! Per-connection upstream auth driver: selects the `Proxy-Authorization`
//! scheme and produces each 407 leg's header value for the handshake loops in
//! the parent module.

use std::io;

use zicade_auth::header::{NegotiateOffer, build_proxy_authorization, parse_proxy_authenticate};
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
/// auth type — Negotiate builds `Negotiate <token>`, Basic sends its ready-made
/// `Basic <b64>` value.
pub(super) enum AuthState {
    None,
    Basic { credentials: String, sent: bool },
    Negotiate(NegotiateHandshake<BoxAuth>),
}

impl AuthState {
    pub(super) fn new(auth: &UpstreamAuth) -> Self {
        match auth {
            UpstreamAuth::None => AuthState::None,
            UpstreamAuth::Basic { credentials } => AuthState::Basic {
                credentials: credentials.clone(),
                sent: false,
            },
            UpstreamAuth::Negotiate(factory) => {
                AuthState::Negotiate(NegotiateHandshake::new(BoxAuth(factory())))
            }
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
            AuthState::None | AuthState::Negotiate(_) => None,
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
            AuthState::Negotiate(handshake) => {
                let token = advance_handshake(handshake, headers)?;
                Ok(build_proxy_authorization(&token))
            }
        }
    }
}

/// The decoded Negotiate challenge from a `Proxy-Authenticate` header, if any.
fn negotiate_challenge(headers: &[(String, String)]) -> Option<Vec<u8>> {
    let value = headers
        .iter()
        .find(|(k, _)| k == "proxy-authenticate")
        .map(|(_, v)| v.as_str())?;
    match parse_proxy_authenticate(value) {
        NegotiateOffer::Challenge(bytes) => Some(bytes),
        NegotiateOffer::Offered | NegotiateOffer::NotOffered => None,
    }
}

fn auth_io_err(err: AuthError) -> io::Error {
    io::Error::other(format!("upstream negotiate handshake failed: {err}"))
}

/// Drive one 407 leg: feed the challenge to the handshake and return the next
/// Negotiate token, or an error if the handshake has no more work.
fn advance_handshake(
    handshake: &mut NegotiateHandshake<BoxAuth>,
    headers: &[(String, String)],
) -> io::Result<Vec<u8>> {
    let challenge = negotiate_challenge(headers);
    match handshake
        .on_response(UpstreamResponse::ProxyAuthRequired { challenge })
        .map_err(auth_io_err)?
    {
        HandshakeStep::SendToken(token) => Ok(token),
        HandshakeStep::Complete => Err(io::Error::other(
            "handshake completed while upstream still demanded auth",
        )),
    }
}
