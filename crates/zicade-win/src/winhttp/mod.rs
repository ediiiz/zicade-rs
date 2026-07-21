//! Real WinHTTP PAC backend implementing [`zicade_routing::PacBackend`].
//!
//! The public surface is cross-platform; the FFI lives in [`win`] behind
//! `cfg(windows)` and degrades to [`RoutingError::UnsupportedPlatform`]
//! off-Windows.

use zicade_routing::{PacBackend, PacResult, RoutingError};

#[cfg(windows)]
mod discovery;
#[cfg(windows)]
mod win;

/// A PAC backend backed by WinHTTP auto-proxy resolution.
///
/// On Windows it owns a `WinHttpOpen` session handle (closed on `Drop`) and
/// resolves each URL via `WinHttpGetProxyForUrl` configured for WPAD
/// auto-detect (DHCP + DNS-A), optionally with an explicit PAC config URL.
pub struct WinHttpPacBackend {
    #[cfg(windows)]
    backend: win::Backend,
}

impl WinHttpPacBackend {
    /// Open a WinHTTP session that resolves proxies via WPAD auto-detect.
    /// Off-Windows returns [`RoutingError::UnsupportedPlatform`].
    pub fn new() -> Result<Self, RoutingError> {
        #[cfg(windows)]
        {
            Ok(Self {
                backend: win::Backend::Session(win::Session::open(None)?),
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
                backend: win::Backend::Session(win::Session::open(Some(config_url))?),
            })
        }
        #[cfg(not(windows))]
        {
            let _ = config_url;
            Err(RoutingError::UnsupportedPlatform)
        }
    }

    /// Discover the effective proxy configuration the way Windows and browsers
    /// do, honouring the per-user WinINET/IE settings with MSDN precedence:
    ///
    /// 1. the "Use setup script" address (`AutoConfigUrl`, a PAC URL);
    /// 2. WPAD network auto-detect (`fAutoDetect`);
    /// 3. a static manual proxy (honouring its bypass list);
    /// 4. otherwise DIRECT (nothing configured).
    ///
    /// This is what `pac.source = "auto"` uses. It fixes the prior behaviour
    /// where "auto" only did WPAD auto-detect and failed with
    /// `ERROR_WINHTTP_UNABLE_TO_DOWNLOAD_SCRIPT` on machines whose PAC lives in
    /// the per-user `AutoConfigURL`. Off-Windows returns
    /// [`RoutingError::UnsupportedPlatform`].
    pub fn from_system() -> Result<Self, RoutingError> {
        #[cfg(windows)]
        {
            Ok(Self {
                backend: win::Backend::from_system()?,
            })
        }
        #[cfg(not(windows))]
        {
            Err(RoutingError::UnsupportedPlatform)
        }
    }
}

impl PacBackend for WinHttpPacBackend {
    fn resolve(&self, url: &str) -> Result<PacResult, RoutingError> {
        #[cfg(windows)]
        {
            self.backend.resolve(url)
        }
        #[cfg(not(windows))]
        {
            let _ = url;
            Err(RoutingError::UnsupportedPlatform)
        }
    }
}
