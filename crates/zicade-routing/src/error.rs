//! Typed errors for PAC routing.

/// Errors produced while resolving a route via a PAC backend.
#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    /// The PAC backend (e.g. WinHTTP) failed to resolve a proxy for the URL.
    #[error("PAC backend resolution failed: {0}")]
    Backend(String),

    /// The current platform has no PAC backend (not Windows).
    #[error("unsupported platform (not Windows)")]
    UnsupportedPlatform,
}
