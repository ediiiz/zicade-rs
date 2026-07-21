//! Windows FFI for WinHTTP PAC resolution (`cfg(windows)` only).

use core::ffi::c_void;

use windows::Win32::Foundation::{GlobalFree, HGLOBAL};
use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_AUTO_DETECT_TYPE_DHCP,
    WINHTTP_AUTO_DETECT_TYPE_DNS_A, WINHTTP_AUTOPROXY_AUTO_DETECT, WINHTTP_AUTOPROXY_CONFIG_URL,
    WINHTTP_AUTOPROXY_OPTIONS, WINHTTP_CURRENT_USER_IE_PROXY_CONFIG, WINHTTP_PROXY_INFO,
    WinHttpCloseHandle, WinHttpGetIEProxyConfigForCurrentUser, WinHttpGetProxyForUrl, WinHttpOpen,
};
use windows::core::{HRESULT, PCWSTR, PWSTR, w};

use zicade_routing::{PacResult, RoutingError};

use super::discovery::{IeProxyConfig, StaticResolver, Strategy, select_strategy};

/// HRESULT for `ERROR_FILE_NOT_FOUND` — `WinHttpGetIEProxyConfigForCurrentUser`
/// reports "no IE proxy config for this user" this way; we treat it as an empty
/// configuration (=> DIRECT) rather than a hard error.
const HRESULT_FILE_NOT_FOUND: HRESULT = HRESULT(0x8007_0002u32 as i32);

/// The concrete resolver `source = "auto"` discovery selects: a live WinHTTP
/// session (PAC config URL or WPAD auto-detect), a pure static-proxy resolver,
/// or an unconditional DIRECT.
pub(super) enum Backend {
    Session(Session),
    Static(StaticResolver),
    Direct,
}

impl Backend {
    /// Discover the effective proxy configuration like Windows/browsers do:
    /// read the per-user IE settings, then apply AutoConfigUrl > WPAD > static
    /// proxy > DIRECT (see [`select_strategy`]).
    pub(super) fn from_system() -> Result<Self, RoutingError> {
        let cfg = read_ie_proxy_config()?;
        match select_strategy(&cfg) {
            Strategy::ConfigUrl(url) => Ok(Backend::Session(Session::open(Some(&url))?)),
            Strategy::Wpad => Ok(Backend::Session(Session::open(None)?)),
            Strategy::StaticProxy { proxy, bypass } => Ok(Backend::Static(StaticResolver::new(
                &proxy,
                bypass.as_deref(),
            ))),
            Strategy::Direct => Ok(Backend::Direct),
        }
    }

    /// Resolve the proxy decision for `url`.
    pub(super) fn resolve(&self, url: &str) -> Result<PacResult, RoutingError> {
        match self {
            Backend::Session(session) => session.resolve(url),
            Backend::Static(resolver) => Ok(resolver.resolve(url)),
            Backend::Direct => Ok(PacResult::Direct),
        }
    }
}

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

/// Read the per-user WinINET/IE proxy configuration
/// (`WinHttpGetIEProxyConfigForCurrentUser`) into owned Rust values.
///
/// The call allocates the three `PWSTR` members with `GlobalAlloc`; the caller
/// owns them and MUST free each with `GlobalFree`. [`take_pwstr`] copies each
/// non-null string out and frees it exactly once, so no pointer leaks and none
/// is freed twice. On the error path nothing was allocated, so nothing is freed
/// (`ERROR_FILE_NOT_FOUND` is mapped to an empty config, i.e. DIRECT).
fn read_ie_proxy_config() -> Result<IeProxyConfig, RoutingError> {
    let mut raw = WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default();
    // SAFETY: `raw` is a valid, zeroed out-parameter of the exact expected type.
    // On success WinHTTP fills its three PWSTR fields with GlobalAlloc'd strings
    // that we copy out and free below via `take_pwstr`.
    let call = unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut raw) };
    if let Err(err) = call {
        if err.code() == HRESULT_FILE_NOT_FOUND {
            return Ok(IeProxyConfig::empty());
        }
        return Err(RoutingError::Backend(format!(
            "WinHttpGetIEProxyConfigForCurrentUser failed: 0x{:08x}",
            err.code().0
        )));
    }
    // Copy each string out and free it before returning; `fAutoDetect` is a
    // plain BOOL (no allocation).
    let auto_config_url = take_pwstr(raw.lpszAutoConfigUrl);
    let proxy = take_pwstr(raw.lpszProxy);
    let bypass = take_pwstr(raw.lpszProxyBypass);
    Ok(IeProxyConfig {
        auto_config_url,
        auto_detect: raw.fAutoDetect.as_bool(),
        proxy,
        bypass,
    })
}

/// Copy a WinHTTP-allocated `PWSTR` into an owned `String`, then free it exactly
/// once with `GlobalFree`. Returns `None` for a null pointer or empty string.
fn take_pwstr(s: PWSTR) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: `s` is a non-null, NUL-terminated wide string WinHTTP allocated
    // with `GlobalAlloc`; we read it exactly once here.
    let value = unsafe { s.to_string() }.ok();
    // SAFETY: the pointer is freed exactly once immediately after the single
    // read above, and `s` is not used again (no double-free, no leak).
    unsafe {
        let _ = GlobalFree(Some(HGLOBAL(s.as_ptr().cast::<c_void>())));
    }
    value.filter(|v| !v.is_empty())
}
