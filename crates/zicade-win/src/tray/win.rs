//! Windows FFI for the system tray, console hiding, and browser-open
//! (`cfg(windows)` only). Every `unsafe` block carries a `// SAFETY:` note.
//!
//! The message pump is single-threaded: [`run_tray`] creates a hidden window on
//! the calling thread and pumps its messages there. The bare `extern "system"`
//! window procedure cannot capture, so the menu items and the pending menu
//! selection live in a **thread-local** slot (single pump per thread); the
//! caller's `on_event` closure stays on `run_tray`'s stack and is invoked from
//! the message loop, never from inside the window procedure — so it can borrow
//! freely without racing the procedure's own borrows.

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList, GetConsoleWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
    ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, HICON, IDI_APPLICATION,
    IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED, LoadIconW, LoadImageW, MF_STRING, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, SM_CXSMICON, SM_CYSMICON, SW_HIDE, SW_SHOWNORMAL,
    SetForegroundWindow, ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
    TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_NULL,
    WM_RBUTTONUP, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

use super::{TrayControl, TrayMenuItem};
use crate::WinError;

/// Tray icon callback message (application-private range).
const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
/// The tray icon's uID (single icon per pump).
const ICON_UID: u32 = 1;
/// Window class name for the hidden tray window.
const CLASS_NAME: PCWSTR = w!("ZicadeTrayWindow");

/// Per-thread pump state read by the (non-capturing) window procedure.
struct PumpState {
    items: Vec<TrayMenuItem>,
    selected: Option<u32>,
}

// The initializer is already a `const {}` block; this clippy version still
// flags the macro (a known false positive that no initializer form satisfies),
// so silence it locally.
#[allow(clippy::missing_const_for_thread_local)]
mod tls {
    use super::{PumpState, RefCell};

    thread_local! {
        pub(super) static PUMP: RefCell<Option<PumpState>> = const { RefCell::new(None) };
    }
}
use tls::PUMP;

/// Encode a Rust string as a NUL-terminated UTF-16 buffer for Win32.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

pub(super) fn console_process_count() -> usize {
    let mut buf = [0u32; 16];
    // SAFETY: `buf` is a valid, writable slice; GetConsoleProcessList fills up
    // to its length and returns the total count (0 if no console is attached).
    let n = unsafe { GetConsoleProcessList(&mut buf) };
    n as usize
}

pub(super) fn hide_console() -> Result<(), WinError> {
    // SAFETY: GetConsoleWindow returns this process's console window or a null
    // HWND; ShowWindow/FreeConsole tolerate the null/absent case, and we ignore
    // their booleans because "no console to hide" is a success for our purpose.
    unsafe {
        let hwnd = GetConsoleWindow();
        if !hwnd.is_invalid() {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        let _ = FreeConsole();
    }
    Ok(())
}

pub(super) fn open_url(url: &str) -> Result<(), WinError> {
    let url_w = wide(url);
    // SAFETY: `url_w` and the static verb are valid NUL-terminated wide strings
    // living for the duration of the call; all other pointers are null/empty.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(url_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns an HINSTANCE-typed error/success code: > 32 is OK.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(WinError::Tray(format!(
            "ShellExecuteW failed: {}",
            result.0 as isize
        )))
    }
}

pub(super) fn run_tray<F>(
    tooltip: &str,
    items: Vec<TrayMenuItem>,
    mut on_event: F,
) -> Result<(), WinError>
where
    F: FnMut(u32) -> TrayControl,
{
    let hwnd = create_window()?;
    PUMP.with(|slot| {
        *slot.borrow_mut() = Some(PumpState {
            items,
            selected: None,
        });
    });

    let result = add_icon(hwnd, tooltip).and_then(|()| pump(hwnd, &mut on_event));

    let _ = delete_icon(hwnd);
    // SAFETY: `hwnd` came from CreateWindowExW and is destroyed exactly once.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    PUMP.with(|slot| *slot.borrow_mut() = None);
    result
}

/// Register the class (idempotently) and create the hidden tray window.
fn create_window() -> Result<HWND, WinError> {
    // SAFETY: a null module name asks for the current process's module handle.
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
        .map_err(|e| WinError::Tray(format!("GetModuleHandleW failed: 0x{:08x}", e.code().0)))?;
    let hinstance = module.into();

    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hinstance,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `wc` is fully initialized and its class-name pointer is a static
    // wide literal. A zero return means "already registered" on a repeat call,
    // which CreateWindowExW below tolerates; a genuine failure surfaces there.
    unsafe {
        let _ = RegisterClassW(&wc);
    }

    // SAFETY: the class is registered; all pointers are valid/null; the window
    // is never shown, so its default geometry is irrelevant.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("Zicade"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance),
            None,
        )
    }
    .map_err(|e| WinError::Tray(format!("CreateWindowExW failed: 0x{:08x}", e.code().0)))?;
    Ok(hwnd)
}

/// Resource id of the app icon embedded into the executable by `apps/zicade`'s
/// build script (`1 ICON "assets/zicade.ico"`).
const APP_ICON_RESOURCE_ID: u16 = 1;

/// Load the Zicade app icon at the system small-icon size for the tray.
///
/// The icon is loaded from the running module (the exe), where the build script
/// embeds it as [`APP_ICON_RESOURCE_ID`]. `LR_SHARED` means the system owns the
/// cached handle (no `DestroyIcon` needed), matching the old `LoadIconW` path.
/// If the resource is absent — e.g. a test harness that doesn't embed it — we
/// fall back to the shared system application icon.
fn load_tray_icon() -> HICON {
    // SAFETY: a null module name asks for the current process's module handle; it
    // fails only in pathological cases, where we drop to the fallback below.
    if let Ok(module) = unsafe { GetModuleHandleW(PCWSTR::null()) } {
        // SAFETY: GetSystemMetrics just reads a system-wide constant.
        let cx = unsafe { GetSystemMetrics(SM_CXSMICON) };
        let cy = unsafe { GetSystemMetrics(SM_CYSMICON) };
        // SAFETY: loads icon resource `APP_ICON_RESOURCE_ID` from our module at
        // the small-icon size. Casting the numeric id to a pointer is the
        // documented MAKEINTRESOURCE convention for naming a resource by id.
        let loaded = unsafe {
            LoadImageW(
                Some(module.into()),
                PCWSTR(APP_ICON_RESOURCE_ID as usize as *const u16),
                IMAGE_ICON,
                cx,
                cy,
                LR_DEFAULTCOLOR | LR_SHARED,
            )
        };
        if let Ok(handle) = loaded {
            if !handle.is_invalid() {
                return HICON(handle.0);
            }
        }
    }
    // SAFETY: a null instance with IDI_APPLICATION loads the shared system icon.
    unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default()
}

/// Build the notify-icon data for `hwnd` with the given tooltip.
fn icon_data(hwnd: HWND, tooltip: &str) -> NOTIFYICONDATAW {
    let hicon = load_tray_icon();

    let mut sz_tip = [0u16; 128];
    for (dst, src) in sz_tip.iter_mut().zip(tooltip.encode_utf16()).take(127) {
        *dst = src;
    }

    NOTIFYICONDATAW {
        cbSize: core::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_UID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY_CALLBACK,
        hIcon: hicon,
        szTip: sz_tip,
        ..Default::default()
    }
}

fn add_icon(hwnd: HWND, tooltip: &str) -> Result<(), WinError> {
    let data = icon_data(hwnd, tooltip);
    // SAFETY: `data` is a fully-initialized NOTIFYICONDATAW borrowed for the call.
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &data) };
    if ok.as_bool() {
        Ok(())
    } else {
        Err(WinError::Tray(
            "Shell_NotifyIconW(NIM_ADD) failed".to_owned(),
        ))
    }
}

fn delete_icon(hwnd: HWND) -> Result<(), WinError> {
    let data = NOTIFYICONDATAW {
        cbSize: core::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_UID,
        ..Default::default()
    };
    // SAFETY: `data` identifies the icon added above; borrowed for the call.
    let ok = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
    if ok.as_bool() {
        Ok(())
    } else {
        Err(WinError::Tray(
            "Shell_NotifyIconW(NIM_DELETE) failed".to_owned(),
        ))
    }
}

/// Pump messages until a menu selection makes `on_event` return `Quit`, or the
/// window is destroyed (WM_QUIT).
fn pump<F>(hwnd: HWND, on_event: &mut F) -> Result<(), WinError>
where
    F: FnMut(u32) -> TrayControl,
{
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a valid out-pointer; a null window filter pumps every
        // message posted to this thread.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        match got.0 {
            -1 => return Err(WinError::Tray("GetMessageW failed".to_owned())),
            0 => return Ok(()), // WM_QUIT
            _ => {}
        }
        // SAFETY: `msg` was populated by the successful GetMessageW above.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if let Some(cmd) = take_selected() {
            if on_event(cmd) == TrayControl::Quit {
                let _ = hwnd; // window torn down by the caller after we return
                return Ok(());
            }
        }
    }
}

/// Remove and return any menu command recorded by the window procedure.
fn take_selected() -> Option<u32> {
    PUMP.with(|slot| slot.borrow_mut().as_mut().and_then(|s| s.selected.take()))
}

/// The hidden window's procedure: turn a right-click on the tray icon into a
/// popup menu, and post-quit on destroy.
extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY_CALLBACK => {
            let event = (lparam.0 as u32) & 0xffff;
            if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                show_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: no preconditions; posts WM_QUIT to this thread's queue.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: forwarding unhandled messages with the original parameters is
        // the documented default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Build and track the right-click popup menu, recording the chosen command id.
fn show_menu(hwnd: HWND) {
    // SAFETY: CreatePopupMenu yields an owned menu handle; on failure we bail.
    let menu = match unsafe { CreatePopupMenu() } {
        Ok(menu) => menu,
        Err(_) => return,
    };

    PUMP.with(|slot| {
        if let Some(state) = slot.borrow().as_ref() {
            for item in &state.items {
                let label = wide(&item.label);
                // SAFETY: `menu` is live; `label` is a valid NUL-terminated wide
                // string whose contents AppendMenuW copies during the call.
                unsafe {
                    let _ = AppendMenuW(menu, MF_STRING, item.id as usize, PCWSTR(label.as_ptr()));
                }
            }
        }
    });

    let mut pt = POINT::default();
    // SAFETY: `pt` is a valid out-pointer; `hwnd` is this thread's live window.
    // TPM_RETURNCMD makes TrackPopupMenu return the chosen id (0 if dismissed).
    let cmd = unsafe {
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd
    };

    let selected = cmd.0 as u32;
    if selected != 0 {
        PUMP.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.selected = Some(selected);
            }
        });
    }
}
