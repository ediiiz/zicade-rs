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

/// Error returned by platform integration points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WinError {
    /// The current platform is not Windows; the requested capability is unavailable.
    UnsupportedPlatform,
    /// An SSPI/Win32 call failed with the given `SECURITY_STATUS`/`HRESULT` code.
    Sspi(i32),
}

impl fmt::Display for WinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WinError::UnsupportedPlatform => write!(f, "unsupported platform (not Windows)"),
            WinError::Sspi(code) => write!(f, "SSPI call failed: 0x{code:08x}"),
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

#[cfg(not(windows))]
mod imp {
    use super::WinError;

    pub(super) fn probe_sspi_negotiate() -> Result<(), WinError> {
        Err(WinError::UnsupportedPlatform)
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
}
