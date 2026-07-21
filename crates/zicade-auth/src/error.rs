//! Typed auth errors.

/// Errors from the Negotiate handshake or an authenticator.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The upstream kept challenging past the leg cap (guards against loops).
    #[error("negotiate handshake exceeded {max} legs")]
    TooManyLegs { max: u32 },

    /// The upstream returned 407 with no continuation after a token was sent —
    /// authentication was refused.
    #[error("upstream rejected the negotiate credentials")]
    Rejected,

    /// The underlying authenticator (e.g. SSPI) failed.
    #[error("authenticator failed: {0}")]
    Authenticator(String),
}
