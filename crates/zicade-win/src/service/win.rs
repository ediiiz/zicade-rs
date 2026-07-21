//! Windows FFI for Service Control Manager integration (`cfg(windows)` only).
//!
//! Every `unsafe` block carries a `// SAFETY:` note. The SCM entrypoint
//! (`service_main`) and control handler (`control_handler`) are bare
//! `extern "system"` callbacks that cannot capture, so `run_dispatcher` stashes
//! the boxed service body and the [`ServiceStop`] handle in module statics
//! before handing control to `StartServiceCtrlDispatcherW`; the callbacks read
//! them back out. This enforces a **single service per process**.

use core::ffi::c_void;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::System::Services::{
    ChangeServiceConfig2W, CloseServiceHandle, CreateServiceW, DeleteService, OpenSCManagerW,
    OpenServiceW, RegisterServiceCtrlHandlerExW, SC_HANDLE, SC_MANAGER_CONNECT,
    SC_MANAGER_CREATE_SERVICE, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_ALL_ACCESS,
    SERVICE_AUTO_START, SERVICE_CONFIG_DESCRIPTION, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP,
    SERVICE_DESCRIPTIONW, SERVICE_ERROR_NORMAL, SERVICE_RUNNING, SERVICE_START_PENDING,
    SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE, SERVICE_STOP_PENDING,
    SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS, SetServiceStatus,
    StartServiceCtrlDispatcherW,
};
use windows::core::{PCWSTR, PWSTR};

use super::{ServiceInstall, ServiceStop};
use crate::WinError;

/// The boxed service body plus the stop handle and fallback name, handed off
/// from `run_dispatcher` to the non-capturing `service_main`.
struct Handoff {
    body: Box<dyn FnOnce(ServiceStop) + Send>,
    stop: ServiceStop,
    name: String,
}

/// Single-service-per-process handoff slot.
static HANDOFF: Mutex<Option<Handoff>> = Mutex::new(None);
/// Stop handle the control handler signals on STOP/SHUTDOWN.
static STOP: Mutex<Option<ServiceStop>> = Mutex::new(None);
/// Raw `SERVICE_STATUS_HANDLE` value so the control handler can report status.
/// `0` means "not yet registered".
static STATUS_HANDLE: AtomicIsize = AtomicIsize::new(0);

/// Encode a Rust string as a NUL-terminated UTF-16 buffer for Win32.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Map a `windows` error into a [`WinError::Service`] naming the failing call.
fn svc_err(op: &str, e: &windows::core::Error) -> WinError {
    WinError::Service(format!("{op} failed: 0x{:08x}", e.code().0))
}

/// RAII guard closing an SC handle exactly once on drop.
struct ScHandle(SC_HANDLE);

impl Drop for ScHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from Open/CreateService and is closed exactly
        // once here (the guard owns it).
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

/// Build the registered binary path: the quoted exe followed by `args`.
fn build_bin_path(exe: &Path, args: &[String]) -> String {
    let mut path = format!("\"{}\"", exe.display());
    for arg in args {
        path.push(' ');
        path.push_str(arg);
    }
    path
}

pub(super) fn install(config: &ServiceInstall) -> Result<(), WinError> {
    let name_w = wide(&config.name);
    let display_w = wide(&config.display_name);
    let bin_w = wide(&build_bin_path(&config.exe_path, &config.args));

    // SAFETY: null machine/database targets the local SCM; the access mask
    // requests create-service rights. The returned handle is owned by ScHandle.
    let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CREATE_SERVICE) }
        .map_err(|e| svc_err("OpenSCManagerW", &e))?;
    let scm = ScHandle(scm);

    // SAFETY: `scm.0` is a live SCManager handle; all wide strings are valid,
    // NUL-terminated, and outlive the call; the optional out-params are null.
    // The returned service handle is owned by ScHandle.
    let service = unsafe {
        CreateServiceW(
            scm.0,
            PCWSTR(name_w.as_ptr()),
            PCWSTR(display_w.as_ptr()),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR(bin_w.as_ptr()),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )
    }
    .map_err(|e| svc_err("CreateServiceW", &e))?;
    let service = ScHandle(service);

    if !config.description.is_empty() {
        set_description(&service, &config.description)?;
    }
    Ok(())
}

/// Attach a description to a freshly created service.
fn set_description(service: &ScHandle, description: &str) -> Result<(), WinError> {
    let mut desc_w = wide(description);
    let info = SERVICE_DESCRIPTIONW {
        lpDescription: PWSTR(desc_w.as_mut_ptr()),
    };
    // SAFETY: `service.0` is a live service handle; SERVICE_CONFIG_DESCRIPTION
    // expects a pointer to a SERVICE_DESCRIPTIONW, which `info` (and its backing
    // `desc_w`) provide and keep valid for the duration of the call.
    unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_DESCRIPTION,
            Some(core::ptr::from_ref(&info).cast::<c_void>()),
        )
    }
    .map_err(|e| svc_err("ChangeServiceConfig2W", &e))
}

pub(super) fn uninstall(name: &str) -> Result<(), WinError> {
    let name_w = wide(name);

    // SAFETY: null machine/database targets the local SCM; connect access is
    // sufficient to open a service for deletion. Owned by ScHandle.
    let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
        .map_err(|e| svc_err("OpenSCManagerW", &e))?;
    let scm = ScHandle(scm);

    // SAFETY: `scm.0` is live; `name_w` is a valid NUL-terminated wide string;
    // SERVICE_ALL_ACCESS includes DELETE. Owned by ScHandle.
    let service = unsafe { OpenServiceW(scm.0, PCWSTR(name_w.as_ptr()), SERVICE_ALL_ACCESS) }
        .map_err(|e| svc_err("OpenServiceW", &e))?;
    let service = ScHandle(service);

    // SAFETY: `service.0` is a live handle opened with delete rights.
    unsafe { DeleteService(service.0) }.map_err(|e| svc_err("DeleteService", &e))
}

pub(super) fn run_dispatcher<F>(name: &str, body: F) -> Result<(), WinError>
where
    F: FnOnce(ServiceStop) + Send + 'static,
{
    let stop = ServiceStop::new();
    {
        let mut slot = HANDOFF
            .lock()
            .map_err(|_| WinError::Service("handoff mutex poisoned".to_owned()))?;
        if slot.is_some() {
            return Err(WinError::Service(
                "a service dispatcher is already running in this process".to_owned(),
            ));
        }
        *slot = Some(Handoff {
            body: Box::new(body),
            stop: stop.clone(),
            name: name.to_owned(),
        });
    }

    // The table entry name is ignored for own-process services, but the array
    // must be NUL-terminated with a zeroed final entry.
    let mut name_buf = wide(name);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(name_buf.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];

    // SAFETY: `table` is a valid, NUL-terminated SERVICE_TABLE_ENTRYW array that
    // (with its backing `name_buf`) outlives this blocking call.
    // StartServiceCtrlDispatcherW returns only after the service stops.
    let result = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };

    // The dispatcher has returned; clear the per-process handoff state.
    if let Ok(mut slot) = HANDOFF.lock() {
        *slot = None;
    }
    if let Ok(mut slot) = STOP.lock() {
        *slot = None;
    }
    STATUS_HANDLE.store(0, Ordering::SeqCst);

    result.map_err(|e| svc_err("StartServiceCtrlDispatcherW", &e))
}

/// SCM service entrypoint (runs on an SCM-spawned thread). Reports
/// `START_PENDING` → `RUNNING`, runs the stashed body, then `STOPPED`.
extern "system" fn service_main(argc: u32, argv: *mut PWSTR) {
    let Some(handoff) = HANDOFF.lock().ok().and_then(|mut slot| slot.take()) else {
        return;
    };
    let Handoff { body, stop, name } = handoff;

    // Prefer the name SCM started us as (argv[0]); fall back to the stashed one.
    let argv_name = service_name_from_argv(argc, argv);
    let effective = if argv_name.is_empty() {
        name
    } else {
        argv_name
    };
    let name_w = wide(&effective);

    // SAFETY: `name_w` is a valid NUL-terminated wide string; `control_handler`
    // is a valid extern "system" callback; a null context is fine because the
    // handler reads shared state from module statics (single service/process).
    let handle = match unsafe {
        RegisterServiceCtrlHandlerExW(PCWSTR(name_w.as_ptr()), Some(control_handler), None)
    } {
        Ok(handle) => handle,
        // Without a status handle we cannot report state; nothing to do but
        // return (the dispatcher will observe the service never started).
        Err(_) => return,
    };

    STATUS_HANDLE.store(handle.0 as isize, Ordering::SeqCst);
    if let Ok(mut slot) = STOP.lock() {
        *slot = Some(stop.clone());
    }

    report(handle, SERVICE_START_PENDING, 0, 1, 3000);
    report(
        handle,
        SERVICE_RUNNING,
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN,
        0,
        0,
    );

    // Run the app until a STOP/SHUTDOWN triggers `stop` and the body returns.
    body(stop);

    report(handle, SERVICE_STOPPED, 0, 0, 0);
}

/// SCM control handler (runs on an SCM thread). Maps STOP/SHUTDOWN to a
/// `STOP_PENDING` report plus a stop trigger; other controls are acknowledged.
extern "system" fn control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    if control == SERVICE_CONTROL_STOP || control == SERVICE_CONTROL_SHUTDOWN {
        let raw = STATUS_HANDLE.load(Ordering::SeqCst);
        if raw != 0 {
            report(
                SERVICE_STATUS_HANDLE(raw as *mut c_void),
                SERVICE_STOP_PENDING,
                0,
                1,
                15000,
            );
        }
        if let Ok(slot) = STOP.lock() {
            if let Some(stop) = slot.as_ref() {
                stop.trigger();
            }
        }
    }
    0 // NO_ERROR
}

/// Report a service status to the SCM.
fn report(
    handle: SERVICE_STATUS_HANDLE,
    state: SERVICE_STATUS_CURRENT_STATE,
    controls_accepted: u32,
    checkpoint: u32,
    wait_hint: u32,
) {
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: controls_accepted,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: checkpoint,
        dwWaitHint: wait_hint,
    };
    // SAFETY: `handle` is a valid status handle from
    // RegisterServiceCtrlHandlerExW; `status` is a fully-initialized
    // SERVICE_STATUS passed by shared reference for the duration of the call.
    unsafe {
        let _ = SetServiceStatus(handle, &status);
    }
}

/// Extract the service name from SCM's `argv[0]`, or empty if unavailable.
fn service_name_from_argv(argc: u32, argv: *mut PWSTR) -> String {
    if argc == 0 || argv.is_null() {
        return String::new();
    }
    // SAFETY: the SCM guarantees `argv` points to `argc` PWSTR entries and that
    // `argv[0]` is the service name as a valid NUL-terminated wide string.
    unsafe {
        let first = *argv;
        if first.is_null() {
            String::new()
        } else {
            first.to_string().unwrap_or_default()
        }
    }
}
