//! Windows integration (SSPI + WinHTTP): the **only** crate permitted to use
//! `unsafe`/FFI. Every unsafe block carries a `// SAFETY:` comment. On
//! non-Windows targets everything compiles to stubs returning
//! [`WinError::UnsupportedPlatform`], so the rest of the workspace builds and
//! unit-tests on any OS.
//!
//! M0 scope: a real link-and-call SSPI smoke check ([`probe_sspi_negotiate`])
//! that proves the `windows` bindings link under the active toolchain (the
//! `x86_64-pc-windows-gnu` linker), rather than discovering a link failure at
//! M3. SSPI/WinHTTP behavior proper lands in M3/M4.

use std::fmt;

pub mod service;
mod sspi;
pub mod tray;
mod winhttp;

pub use service::ServiceStop;
pub use sspi::{LoopbackReport, SspiNegotiate, SspiPackage, run_loopback_handshake};
pub use winhttp::WinHttpPacBackend;

/// Error returned by platform integration points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WinError {
    /// The current platform is not Windows; the requested capability is unavailable.
    UnsupportedPlatform,
    /// An SSPI/Win32 call failed with the given `SECURITY_STATUS`/`HRESULT` code.
    Sspi(i32),
    /// A Service Control Manager operation failed; the message names the failing
    /// call and its Win32/HRESULT code (e.g. access-denied without Administrator).
    Service(String),
    /// A system-tray / console / shell-open operation failed; the message names
    /// the failing call and its code.
    Tray(String),
}

impl fmt::Display for WinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WinError::UnsupportedPlatform => write!(f, "unsupported platform (not Windows)"),
            WinError::Sspi(code) => write!(f, "SSPI call failed: 0x{code:08x}"),
            WinError::Service(msg) => write!(f, "service control manager error: {msg}"),
            WinError::Tray(msg) => write!(f, "system-tray error: {msg}"),
        }
    }
}

impl std::error::Error for WinError {}

/// Acquire — and immediately release — a Negotiate **outbound** credential
/// handle for the logged-in user (no stored password).
///
/// This is the M0 de-risk: it forces the linker to resolve real SSPI symbols
/// (`AcquireCredentialsHandleW` / `FreeCredentialsHandle`), proving the
/// `windows` crate links under this toolchain. Off-Windows it returns
/// [`WinError::UnsupportedPlatform`].
pub fn probe_sspi_negotiate() -> Result<(), WinError> {
    imp::probe_sspi_negotiate()
}

/// Number of open OS handles in the current process, or `None` off-Windows.
///
/// Used by leak/soak tests to assert handle usage returns to baseline
/// (LESSON-7: the prior build leaked handles, ~171→231 over 60 requests).
pub fn process_handle_count() -> Option<u32> {
    imp::process_handle_count()
}

#[cfg(not(windows))]
mod imp {
    use super::WinError;

    pub(super) fn probe_sspi_negotiate() -> Result<(), WinError> {
        Err(WinError::UnsupportedPlatform)
    }

    pub(super) fn process_handle_count() -> Option<u32> {
        None
    }
}

#[cfg(windows)]
mod imp {
    use super::WinError;
    use windows::Win32::Security::Authentication::Identity::{
        AcquireCredentialsHandleW, FreeCredentialsHandle, SECPKG_CRED_OUTBOUND,
    };
    use windows::Win32::Security::Credentials::SecHandle;
    use windows::core::{PCWSTR, w};

    pub(super) fn probe_sspi_negotiate() -> Result<(), WinError> {
        let mut cred = SecHandle::default();
        // SAFETY: `pszprincipal` is null (use the process's default identity),
        // `pszpackage` points at a valid static wide string ("Negotiate"), all
        // optional in-params are `None`, and `phcredential` is a valid,
        // exclusively-borrowed out-pointer. On success the handle is released
        // via `FreeCredentialsHandle` before this function returns.
        unsafe {
            AcquireCredentialsHandleW(
                PCWSTR::null(),
                w!("Negotiate"),
                SECPKG_CRED_OUTBOUND,
                None,
                None,
                None,
                None,
                &mut cred,
                None,
            )
            .map_err(|e| WinError::Sspi(e.code().0))?;

            // SAFETY: `cred` was just populated by a successful
            // `AcquireCredentialsHandleW`; freeing it exactly once is correct.
            let _ = FreeCredentialsHandle(&cred);
        }
        Ok(())
    }

    pub(super) fn process_handle_count() -> Option<u32> {
        use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};

        let mut count: u32 = 0;
        // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no
        // release; `count` is a valid, exclusively-borrowed out-pointer.
        unsafe {
            GetProcessHandleCount(GetCurrentProcess(), &mut count).ok()?;
        }
        Some(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn sspi_negotiate_links_and_acquires() {
        // Proves the `windows` SSPI bindings link under this toolchain AND that
        // a real Negotiate outbound credential handle is obtainable here.
        probe_sspi_negotiate().expect("acquire+free Negotiate credential handle");
    }

    #[cfg(not(windows))]
    #[test]
    fn sspi_probe_unsupported_off_windows() {
        assert_eq!(probe_sspi_negotiate(), Err(WinError::UnsupportedPlatform));
    }

    #[cfg(windows)]
    #[test]
    fn sspi_new_acquires_for_both_packages() {
        // Both selectable packages must yield a real outbound credential handle
        // on this box (NTLM is always present; Negotiate is too on a domain box
        // and standalone). Models `sspi_negotiate_links_and_acquires`.
        for pkg in [SspiPackage::Ntlm, SspiPackage::Negotiate] {
            SspiNegotiate::new(Some("HTTP/wp8080"), pkg)
                .unwrap_or_else(|e| panic!("acquire {pkg:?} credential handle: {e}"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn sspi_loopback_handshake_completes() {
        // LESSON-3: pair a real client (ISC) with a real server (ASC) on this
        // machine to produce GENUINE tokens and run the handshake to completion,
        // without a remote server and without hand-made tokens (which SSPI
        // rejected with SEC_E_INVALID_TOKEN in the prior build). `None` target
        // biases toward NTLM, which completes cleanly on loopback.
        let report = run_loopback_handshake(None).expect("loopback handshake should run");
        assert!(report.completed, "handshake did not complete: {report:?}");
        assert!(
            report.client_legs >= 1 && report.server_legs >= 1,
            "{report:?}"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn sspi_loopback_unsupported_off_windows() {
        assert_eq!(
            run_loopback_handshake(None),
            Err(WinError::UnsupportedPlatform)
        );
    }
}
