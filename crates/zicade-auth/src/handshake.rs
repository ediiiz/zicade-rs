//! The pure, sans-I/O Negotiate handshake state machine.
//!
//! The caller performs the actual request/response I/O and feeds each upstream
//! response in via [`NegotiateHandshake::on_response`]; the machine returns the
//! next `Proxy-Authorization` token to send, or [`HandshakeStep::Complete`].

use crate::authenticator::UpstreamAuthenticator;
use crate::error::AuthError;

/// Default cap on handshake legs, guarding against a hostile/looping upstream.
const DEFAULT_MAX_LEGS: u32 = 10;

/// A parsed upstream response relevant to the handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamResponse {
    /// 407, optionally carrying a decoded Negotiate challenge token.
    ProxyAuthRequired { challenge: Option<Vec<u8>> },
    /// A success status (handshake done).
    Success,
    /// Any other terminal status; the handshake ends and the caller inspects it.
    Other(u16),
}

/// What the caller should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeStep {
    /// Re-send the request carrying this token as `Proxy-Authorization`.
    SendToken(Vec<u8>),
    /// The handshake is finished.
    Complete,
}

/// Drives the Negotiate challenge/response legs against an upstream 407.
pub struct NegotiateHandshake<A> {
    auth: A,
    legs: u32,
    max_legs: u32,
    started: bool,
    done: bool,
}

impl<A: UpstreamAuthenticator> NegotiateHandshake<A> {
    /// Create a handshake over the given authenticator.
    pub fn new(auth: A) -> Self {
        Self {
            auth,
            legs: 0,
            max_legs: DEFAULT_MAX_LEGS,
            started: false,
            done: false,
        }
    }

    /// Override the leg cap.
    #[must_use]
    pub fn with_max_legs(mut self, max_legs: u32) -> Self {
        self.max_legs = max_legs;
        self
    }

    /// Number of tokens produced so far.
    pub fn legs(&self) -> u32 {
        self.legs
    }

    /// Borrow the underlying authenticator (used in tests/diagnostics).
    pub fn authenticator(&self) -> &A {
        &self.auth
    }

    /// Feed an upstream response and get the next step.
    pub fn on_response(&mut self, response: UpstreamResponse) -> Result<HandshakeStep, AuthError> {
        if self.done {
            return Ok(HandshakeStep::Complete);
        }
        match response {
            UpstreamResponse::Success | UpstreamResponse::Other(_) => {
                self.done = true;
                Ok(HandshakeStep::Complete)
            }
            UpstreamResponse::ProxyAuthRequired { challenge } => self.advance(challenge),
        }
    }

    fn advance(&mut self, challenge: Option<Vec<u8>>) -> Result<HandshakeStep, AuthError> {
        // A 407 with no continuation token after we already sent one = refusal.
        if self.started && challenge.is_none() {
            return Err(AuthError::Rejected);
        }
        if self.legs >= self.max_legs {
            return Err(AuthError::TooManyLegs { max: self.max_legs });
        }
        let token = self.auth.step(challenge.as_deref())?;
        self.legs += 1;
        self.started = true;
        Ok(HandshakeStep::SendToken(token))
    }
}
