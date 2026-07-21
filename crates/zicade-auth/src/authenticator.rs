//! The `UpstreamAuthenticator` seam. The real SSPI implementation lives in
//! `zicade-win`; tests inject a fake. This keeps the handshake state machine
//! pure and testable without a domain-joined box.

use crate::error::AuthError;

/// Drives a security context one leg at a time.
///
/// Ownership models the credential/context handle lifecycle: the authenticator
/// is created once (acquiring the handle), stepped per leg, and dropped once
/// (releasing the handle) when the handshake ends.
pub trait UpstreamAuthenticator {
    /// Advance the context with the server's challenge token (`None` on the
    /// first leg) and return the next token to send to the upstream.
    fn step(&mut self, challenge: Option<&[u8]>) -> Result<Vec<u8>, AuthError>;
}
