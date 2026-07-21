//! Real WinHTTP PAC backend implementing [`zicade_routing::PacBackend`].
//!
//! The public surface is cross-platform; the FFI lives in [`win`] behind
//! `cfg(windows)` and degrades to [`RoutingError::UnsupportedPlatform`]
//! off-Windows.

use zicade_routing::{PacBackend, PacResult, RoutingError};

#[cfg(windows)]
mod win;

/// A PAC backend backed by WinHTTP auto-proxy resolution.
///
/// On Windows it owns a `WinHttpOpen` session handle (closed on `Drop`) and
/// resolves each URL via `WinHttpGetProxyForUrl` configured for WPAD
/// auto-detect (DHCP + DNS-A), optionally with an explicit PAC config URL.
pub struct WinHttpPacBackend {
    #[cfg(windows)]
    session: win::Session,
}

impl WinHttpPacBackend {
    /// Open a WinHTTP session that resolves proxies via WPAD auto-detect.
    /// Off-Windows returns [`RoutingError::UnsupportedPlatform`].
    pub fn new() -> Result<Self, RoutingError> {
        #[cfg(windows)]
        {
            Ok(Self {
                session: win::Session::open(None)?,
            })
        }
        #[cfg(not(windows))]
        {
            Err(RoutingError::UnsupportedPlatform)
        }
    }

    /// Open a WinHTTP session that resolves proxies via an explicit PAC config
    /// URL (auto-detect is also enabled as a fallback). Off-Windows returns
    /// [`RoutingError::UnsupportedPlatform`].
    pub fn with_config_url(config_url: &str) -> Result<Self, RoutingError> {
        #[cfg(windows)]
        {
            Ok(Self {
                session: win::Session::open(Some(config_url))?,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = config_url;
            Err(RoutingError::UnsupportedPlatform)
        }
    }
}

impl PacBackend for WinHttpPacBackend {
    fn resolve(&self, url: &str) -> Result<PacResult, RoutingError> {
        #[cfg(windows)]
        {
            self.session.resolve(url)
        }
        #[cfg(not(windows))]
        {
            let _ = url;
            Err(RoutingError::UnsupportedPlatform)
        }
    }
}
