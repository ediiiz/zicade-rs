//! Real SSPI Negotiate authentication and the loopback test harness (LESSON-3).
//!
//! The public surface is cross-platform; the FFI lives in [`win`] behind
//! `cfg(windows)` and everything degrades to `UnsupportedPlatform` off-Windows.

use zicade_auth::{AuthError, UpstreamAuthenticator};

use crate::WinError;

#[cfg(windows)]
mod win;

/// Outcome of the loopback SSPI handshake harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopbackReport {
    /// Number of client (`InitializeSecurityContext`) legs.
    pub client_legs: u32,
    /// Number of server (`AcceptSecurityContext`) legs.
    pub server_legs: u32,
    /// Whether both sides reached completion.
    pub completed: bool,
}

/// A real SSPI Negotiate authenticator (outbound credential) that implements
/// [`UpstreamAuthenticator`]. Off-Windows, construction fails.
pub struct SspiNegotiate {
    #[cfg(windows)]
    inner: win::ClientContext,
}

impl SspiNegotiate {
    /// Acquire a Negotiate outbound credential handle for the logged-in user.
    /// `target_spn` is the upstream's service principal name (e.g.
    /// `"HTTP/wp8080"`); `None` biases toward NTLM (used by the loopback test).
    pub fn new(target_spn: Option<&str>) -> Result<Self, WinError> {
        #[cfg(windows)]
        {
            Ok(Self {
                inner: win::ClientContext::new(target_spn)?,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = target_spn;
            Err(WinError::UnsupportedPlatform)
        }
    }
}

impl UpstreamAuthenticator for SspiNegotiate {
    fn step(&mut self, challenge: Option<&[u8]>) -> Result<Vec<u8>, AuthError> {
        #[cfg(windows)]
        {
            self.inner
                .step(challenge)
                .map_err(|e| AuthError::Authenticator(e.to_string()))
        }
        #[cfg(not(windows))]
        {
            let _ = challenge;
            Err(AuthError::Authenticator(
                "SSPI Negotiate unavailable (not Windows)".to_owned(),
            ))
        }
    }
}

/// Run a full Negotiate handshake between a real client (`InitializeSecurityContext`)
/// and a real server (`AcceptSecurityContext`) on this machine, producing genuine
/// tokens without any remote server (LESSON-3). Off-Windows returns
/// `UnsupportedPlatform`.
pub fn run_loopback_handshake(target_spn: Option<&str>) -> Result<LoopbackReport, WinError> {
    #[cfg(windows)]
    {
        win::run_loopback(target_spn)
    }
    #[cfg(not(windows))]
    {
        let _ = target_spn;
        Err(WinError::UnsupportedPlatform)
    }
}
