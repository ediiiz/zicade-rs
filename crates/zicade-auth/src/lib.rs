#![forbid(unsafe_code)]

//! Upstream authentication: the [`UpstreamAuthenticator`] seam and the pure,
//! testable [`NegotiateHandshake`] state machine that drives the
//! challenge/response legs against an upstream 407. The real SSPI implementation
//! lives in `zicade-win`; tests inject a fake.

mod authenticator;
mod error;
mod handshake;
pub mod header;

pub use authenticator::UpstreamAuthenticator;
pub use error::AuthError;
pub use handshake::{HandshakeStep, NegotiateHandshake, UpstreamResponse};
