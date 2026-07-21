//! Windows SSPI FFI: outbound client (`InitializeSecurityContext`), inbound
//! server (`AcceptSecurityContext`), and the loopback driver. Every `unsafe`
//! block carries a `// SAFETY:` note.
//!
//! `SecBufferDesc` points at a `SecBuffer`; both are kept as named locals in the
//! same scope as the FFI call so the self-referential pointer never dangles.

use windows::Win32::Foundation::SEC_E_OK;
use windows::Win32::Security::Authentication::Identity::{
    ASC_REQ_ALLOCATE_MEMORY, ASC_REQ_CONNECTION, AcceptSecurityContext, AcquireCredentialsHandleW,
    DeleteSecurityContext, FreeContextBuffer, FreeCredentialsHandle, ISC_REQ_ALLOCATE_MEMORY,
    ISC_REQ_CONNECTION, InitializeSecurityContextW, SECBUFFER_TOKEN, SECBUFFER_VERSION,
    SECPKG_CRED, SECPKG_CRED_INBOUND, SECPKG_CRED_OUTBOUND, SECURITY_NATIVE_DREP, SecBuffer,
    SecBufferDesc,
};
use windows::Win32::Security::Credentials::SecHandle;
use windows::core::{PCWSTR, w};

use super::LoopbackReport;
use crate::WinError;

fn sspi_err(e: windows::core::Error) -> WinError {
    WinError::Sspi(e.code().0)
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn acquire_cred(usage: SECPKG_CRED) -> Result<SecHandle, WinError> {
    let mut cred = SecHandle::default();
    // SAFETY: null principal (default identity), valid static package name, all
    // optional inputs None, and a valid out-pointer for the handle.
    unsafe {
        AcquireCredentialsHandleW(
            PCWSTR::null(),
            w!("Negotiate"),
            usage,
            None,
            None,
            None,
            None,
            &mut cred,
            None,
        )
    }
    .map_err(sspi_err)?;
    Ok(cred)
}

fn token_input(data: &mut [u8]) -> SecBuffer {
    SecBuffer {
        cbBuffer: u32::try_from(data.len()).unwrap_or(u32::MAX),
        BufferType: SECBUFFER_TOKEN,
        pvBuffer: data.as_mut_ptr().cast(),
    }
}

fn empty_token() -> SecBuffer {
    SecBuffer {
        cbBuffer: 0,
        BufferType: SECBUFFER_TOKEN,
        pvBuffer: core::ptr::null_mut(),
    }
}

fn desc_of(buf: &mut SecBuffer) -> SecBufferDesc {
    SecBufferDesc {
        ulVersion: SECBUFFER_VERSION,
        cBuffers: 1,
        pBuffers: buf,
    }
}

/// Copy an SSPI-allocated output token into a `Vec` and free it.
fn take_token(buf: &SecBuffer) -> Vec<u8> {
    if buf.pvBuffer.is_null() {
        return Vec::new();
    }
    let out = if buf.cbBuffer == 0 {
        Vec::new()
    } else {
        // SAFETY: SSPI allocated `cbBuffer` bytes at `pvBuffer` (ALLOCATE_MEMORY).
        let slice =
            unsafe { std::slice::from_raw_parts(buf.pvBuffer.cast::<u8>(), buf.cbBuffer as usize) };
        slice.to_vec()
    };
    // SAFETY: `pvBuffer` was allocated by SSPI and is freed exactly once here.
    unsafe {
        let _ = FreeContextBuffer(buf.pvBuffer);
    }
    out
}

/// Outbound (client) Negotiate context.
pub(super) struct ClientContext {
    cred: SecHandle,
    ctx: Option<SecHandle>,
    target: Option<Vec<u16>>,
}

impl ClientContext {
    pub(super) fn new(target_spn: Option<&str>) -> Result<Self, WinError> {
        Ok(Self {
            cred: acquire_cred(SECPKG_CRED_OUTBOUND)?,
            ctx: None,
            target: target_spn.map(to_wide),
        })
    }

    pub(super) fn step(&mut self, challenge: Option<&[u8]>) -> Result<Vec<u8>, WinError> {
        let mut in_data = challenge.map(<[u8]>::to_vec);
        let mut in_buf = in_data
            .as_mut()
            .map_or_else(empty_token, |d| token_input(d));
        let in_desc = desc_of(&mut in_buf);
        let pinput = challenge.map(|_| std::ptr::from_ref(&in_desc));

        let mut out_buf = empty_token();
        let mut out_desc = desc_of(&mut out_buf);
        let mut attrs = 0u32;
        let ctx_in = self.ctx.as_ref().map(std::ptr::from_ref);
        let mut new_ctx = SecHandle::default();
        let target_ptr = self.target.as_ref().map(|v| v.as_ptr());
        let req = ISC_REQ_ALLOCATE_MEMORY | ISC_REQ_CONNECTION;

        // SAFETY: cred/context handles are valid; the input (when present) and
        // output descriptors point at the live `in_buf`/`out_buf` locals for the
        // whole call; the output token is freed via FreeContextBuffer.
        let hr = unsafe {
            InitializeSecurityContextW(
                Some(&self.cred),
                ctx_in,
                target_ptr,
                req,
                0,
                SECURITY_NATIVE_DREP,
                pinput,
                0,
                Some(&mut new_ctx),
                Some(&mut out_desc),
                &mut attrs,
                None,
            )
        };
        self.ctx = Some(new_ctx);
        hr.ok().map_err(sspi_err)?;
        Ok(take_token(&out_buf))
    }
}

impl Drop for ClientContext {
    fn drop(&mut self) {
        if let Some(ctx) = self.ctx.as_ref() {
            // SAFETY: ctx was produced by a successful ISC; deleted once.
            unsafe {
                let _ = DeleteSecurityContext(ctx);
            }
        }
        // SAFETY: cred was acquired in `new`; freed once.
        unsafe {
            let _ = FreeCredentialsHandle(&self.cred);
        }
    }
}

/// Inbound (server) Negotiate context — loopback/test support.
pub(super) struct ServerContext {
    cred: SecHandle,
    ctx: Option<SecHandle>,
}

impl ServerContext {
    pub(super) fn new() -> Result<Self, WinError> {
        Ok(Self {
            cred: acquire_cred(SECPKG_CRED_INBOUND)?,
            ctx: None,
        })
    }

    /// Accept a client token; returns `(output_token, completed)`.
    pub(super) fn accept(&mut self, input: &[u8]) -> Result<(Vec<u8>, bool), WinError> {
        let mut in_data = input.to_vec();
        let mut in_buf = token_input(&mut in_data);
        let in_desc = desc_of(&mut in_buf);

        let mut out_buf = empty_token();
        let mut out_desc = desc_of(&mut out_buf);
        let mut attrs = 0u32;
        let ctx_in = self.ctx.as_ref().map(std::ptr::from_ref);
        let mut new_ctx = SecHandle::default();
        let req = ASC_REQ_ALLOCATE_MEMORY | ASC_REQ_CONNECTION;

        // SAFETY: cred/context valid; input/output descriptors reference the live
        // `in_buf`/`out_buf` locals for the call; output token freed in take_token.
        let hr = unsafe {
            AcceptSecurityContext(
                Some(&self.cred),
                ctx_in,
                Some(&in_desc),
                req,
                SECURITY_NATIVE_DREP,
                Some(&mut new_ctx),
                Some(&mut out_desc),
                &mut attrs,
                None,
            )
        };
        self.ctx = Some(new_ctx);
        hr.ok().map_err(sspi_err)?;
        let completed = hr == SEC_E_OK;
        Ok((take_token(&out_buf), completed))
    }
}

impl Drop for ServerContext {
    fn drop(&mut self) {
        if let Some(ctx) = self.ctx.as_ref() {
            // SAFETY: ctx produced by a successful ASC; deleted once.
            unsafe {
                let _ = DeleteSecurityContext(ctx);
            }
        }
        // SAFETY: cred acquired in `new`; freed once.
        unsafe {
            let _ = FreeCredentialsHandle(&self.cred);
        }
    }
}

pub(super) fn run_loopback(target_spn: Option<&str>) -> Result<LoopbackReport, WinError> {
    let mut client = ClientContext::new(target_spn)?;
    let mut server = ServerContext::new()?;

    let mut client_token = client.step(None)?;
    let mut client_legs = 1u32;
    let mut server_legs = 0u32;

    for _ in 0..10 {
        let (server_token, completed) = server.accept(&client_token)?;
        server_legs += 1;
        if completed {
            if !server_token.is_empty() {
                let _ = client.step(Some(&server_token))?;
                client_legs += 1;
            }
            return Ok(LoopbackReport {
                client_legs,
                server_legs,
                completed: true,
            });
        }
        client_token = client.step(Some(&server_token))?;
        client_legs += 1;
    }

    Ok(LoopbackReport {
        client_legs,
        server_legs,
        completed: false,
    })
}
