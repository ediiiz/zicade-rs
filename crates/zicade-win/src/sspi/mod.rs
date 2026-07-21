//! Real SSPI Negotiate authentication and the loopback test harness (LESSON-3).
//!
//! The public surface is cross-platform; the FFI lives in [`win`] behind
//! `cfg(windows)` and everything degrades to `UnsupportedPlatform` off-Windows.

use zicade_auth::{AuthError, UpstreamAuthenticator};

use crate::WinError;

#[cfg(windows)]
mod win;

/// Which SSPI security package to acquire for the outbound handshake.
///
/// Defaults to [`SspiPackage::Ntlm`]: the corporate gateway rejects SPNEGO and
/// has no Kerberos SPN, so only NTLM completes there. Environments with a
/// registered Kerberos SPN can select [`SspiPackage::Negotiate`] for SSO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SspiPackage {
    /// The `NTLM` package. Default.
    #[default]
    Ntlm,
    /// The `Negotiate` package (SPNEGO / Kerberos).
    Negotiate,
}

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
    /// Acquire an outbound credential handle for the logged-in user using the
    /// selected `package`. `target_spn` is the upstream's service principal
    /// name (e.g. `"HTTP/wp8080"`).
    pub fn new(target_spn: Option<&str>, package: SspiPackage) -> Result<Self, WinError> {
        #[cfg(windows)]
        {
            Ok(Self {
                inner: win::ClientContext::new(target_spn, package)?,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (target_spn, package);
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
