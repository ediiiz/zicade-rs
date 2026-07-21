//! Windows FFI for WinHTTP PAC resolution (`cfg(windows)` only).

use core::ffi::c_void;

use windows::Win32::Foundation::{GlobalFree, HGLOBAL};
use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_AUTO_DETECT_TYPE_DHCP,
    WINHTTP_AUTO_DETECT_TYPE_DNS_A, WINHTTP_AUTOPROXY_AUTO_DETECT, WINHTTP_AUTOPROXY_CONFIG_URL,
    WINHTTP_AUTOPROXY_OPTIONS, WINHTTP_PROXY_INFO, WinHttpCloseHandle, WinHttpGetProxyForUrl,
    WinHttpOpen,
};
use windows::core::{PCWSTR, PWSTR, w};

use zicade_routing::{PacResult, RoutingError};

/// Encode a Rust string as a NUL-terminated UTF-16 buffer for Win32.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Owns a WinHTTP session handle (closed on `Drop`) plus an optional PAC config
/// URL kept alive for the lifetime of the session.
pub(super) struct Session {
    handle: *mut c_void,
    config_url: Option<Vec<u16>>,
}

impl Session {
    /// Open a WinHTTP session. `config_url` supplies an explicit PAC URL; when
    /// `None`, resolution relies purely on WPAD auto-detect.
    pub(super) fn open(config_url: Option<&str>) -> Result<Self, RoutingError> {
        // SAFETY: all string params are valid: the agent is a static wide
        // literal, and proxy name/bypass are null (`WINHTTP_NO_PROXY_NAME` /
        // `_BYPASS`) which is valid for `WINHTTP_ACCESS_TYPE_DEFAULT_PROXY`.
        // `dwflags` 0 selects synchronous mode (required by
        // `WinHttpGetProxyForUrl`). The returned handle is released in `Drop`.
        let handle = unsafe {
            WinHttpOpen(
                w!("Zicade/1.0"),
                WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        };
        if handle.is_null() {
            return Err(RoutingError::Backend(
                "WinHttpOpen returned a null session handle".to_owned(),
            ));
        }
        Ok(Self {
            handle,
            config_url: config_url.map(wide),
        })
    }

    /// Resolve the proxy for `url` via `WinHttpGetProxyForUrl`.
    pub(super) fn resolve(&self, url: &str) -> Result<PacResult, RoutingError> {
        let url_w = wide(url);

        let mut options = WINHTTP_AUTOPROXY_OPTIONS {
            dwFlags: WINHTTP_AUTOPROXY_AUTO_DETECT,
            dwAutoDetectFlags: WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A,
            // Let WinHTTP use the caller's credentials on a challenge so WPAD
            // fetches behind auth still succeed.
            fAutoLogonIfChallenged: true.into(),
            ..Default::default()
        };
        if let Some(cfg) = &self.config_url {
            // Enable the explicit PAC URL in addition to auto-detect.
            options.dwFlags |= WINHTTP_AUTOPROXY_CONFIG_URL;
            options.lpszAutoConfigUrl = PCWSTR(cfg.as_ptr());
        }

        let mut info = WINHTTP_PROXY_INFO::default();
        // SAFETY: `self.handle` is a live session handle from `WinHttpOpen`;
        // `url_w` is a valid NUL-terminated wide string that outlives the call;
        // `options` and `info` are valid, exclusively-borrowed out/in-pointers.
        // On success `info` owns WinHTTP-allocated strings we free below.
        let call = unsafe {
            WinHttpGetProxyForUrl(self.handle, PCWSTR(url_w.as_ptr()), &mut options, &mut info)
        };

        match call {
            Ok(()) => {
                let result = read_proxy_info(&info);
                free_proxy_info(&info);
                Ok(result)
            }
            Err(err) => Err(RoutingError::Backend(format!(
                "WinHttpGetProxyForUrl failed: 0x{:08x}",
                err.code().0
            ))),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: `handle` came from `WinHttpOpen` and is closed exactly
            // once here; the guard above prevents a double close.
            unsafe {
                let _ = WinHttpCloseHandle(self.handle);
            }
        }
    }
}

/// Read `WINHTTP_PROXY_INFO.lpszProxy` into a Rust string and parse it. A null
/// proxy list means DIRECT.
fn read_proxy_info(info: &WINHTTP_PROXY_INFO) -> PacResult {
    if info.lpszProxy.is_null() {
        return PacResult::Direct;
    }
    // SAFETY: `lpszProxy` is a non-null, NUL-terminated wide string owned by
    // `info` (allocated by WinHTTP); we only read it here.
    let raw = unsafe { info.lpszProxy.to_string() }.unwrap_or_default();
    PacResult::parse(&raw)
}

/// Free the WinHTTP-allocated members of `WINHTTP_PROXY_INFO`. WinHTTP
/// allocates these with `GlobalAlloc`, so `GlobalFree` is the correct release.
fn free_proxy_info(info: &WINHTTP_PROXY_INFO) {
    free_pwstr(info.lpszProxy);
    free_pwstr(info.lpszProxyBypass);
}

fn free_pwstr(s: PWSTR) {
    if s.is_null() {
        return;
    }
    // SAFETY: `s` is non-null and was allocated by WinHTTP via `GlobalAlloc`;
    // freeing it exactly once with `GlobalFree` is the documented contract.
    unsafe {
        let _ = GlobalFree(Some(HGLOBAL(s.as_ptr().cast::<c_void>())));
    }
}
