//! Windows FFI for network monitoring (`cfg(windows)` only): adapter DNS-suffix
//! enumeration and address-change notification.

use std::mem;
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_BUFFER_OVERFLOW, ERROR_IO_PENDING, ERROR_SUCCESS, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::NetworkManagement::IpHelper::{
    CancelIPChangeNotify, GET_ADAPTERS_ADDRESSES_FLAGS, GetAdaptersAddresses,
    IP_ADAPTER_ADDRESSES_LH, NotifyAddrChange,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::System::IO::OVERLAPPED;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::PCWSTR;

/// `AF_UNSPEC`: enumerate adapters regardless of IPv4/IPv6 (we only read the DNS
/// suffix, which is address-family independent). Inlined to avoid pulling in the
/// `Win32_Networking_WinSock` feature just for the constant `0`.
const AF_UNSPEC: u32 = 0;

/// Enumerate the connection-specific DNS suffixes of the adapters that are up.
///
/// Uses the standard two-call `GetAdaptersAddresses` pattern: probe the required
/// buffer size, allocate an aligned buffer, then walk the returned linked list.
/// Any unexpected status yields an empty list (the caller treats "can't detect"
/// as the configured mode, not a failure).
pub(super) fn active_dns_suffixes() -> Vec<String> {
    let flags = GET_ADAPTERS_ADDRESSES_FLAGS(0);
    let mut size: u32 = 0;

    // First call: probe the required buffer size (null buffer). The documented
    // success signal for the probe is ERROR_BUFFER_OVERFLOW with `size` set.
    // SAFETY: `adapteraddresses` is `None` and `size` is a valid out-pointer;
    // this form only writes `*size`.
    let ret = unsafe { GetAdaptersAddresses(AF_UNSPEC, flags, None, None, &mut size) };
    if ret != ERROR_BUFFER_OVERFLOW.0 || size == 0 {
        return Vec::new();
    }

    // Allocate a buffer aligned for the adapter struct (a `Vec<u8>` would not be
    // guaranteed to satisfy the struct's pointer alignment).
    let elem = mem::size_of::<IP_ADAPTER_ADDRESSES_LH>();
    let count = (size as usize).div_ceil(elem).max(1);
    let mut buf: Vec<IP_ADAPTER_ADDRESSES_LH> = Vec::with_capacity(count);
    let head = buf.as_mut_ptr();

    // Second call: fill the buffer. `size` is in/out and matches the allocation.
    // SAFETY: `head` addresses `count * elem >= size` bytes, correctly aligned;
    // `size` reflects that capacity.
    let ret = unsafe { GetAdaptersAddresses(AF_UNSPEC, flags, None, Some(head), &mut size) };
    if ret != ERROR_SUCCESS.0 {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut cur = head;
    while !cur.is_null() {
        // SAFETY: `cur` is the head Windows populated, or a `Next` link it wrote;
        // every node lives in `buf`, which outlives this loop.
        let node = unsafe { &*cur };
        if node.OperStatus == IfOperStatusUp && !node.DnsSuffix.is_null() {
            // SAFETY: `DnsSuffix` is a non-null, NUL-terminated wide string owned
            // by the node; read once here.
            if let Ok(s) = unsafe { node.DnsSuffix.to_string() } {
                if !s.is_empty() {
                    out.push(s);
                }
            }
        }
        cur = node.Next;
    }
    out
}

/// Block until an address change fires or `timeout` elapses.
///
/// Registers an overlapped [`NotifyAddrChange`] whose completion signals a
/// manual-reset event, then waits on that event with the timeout. On timeout the
/// still-pending notification is cancelled with [`CancelIPChangeNotify`] before
/// the `OVERLAPPED` is dropped, so Windows never writes to freed memory. If the
/// registration can't be set up, falls back to a plain sleep so the caller still
/// polls on schedule.
pub(super) fn wait_for_network_change(timeout: Duration) {
    let ms = timeout.as_millis().min(u32::MAX as u128) as u32;

    // Manual-reset, initially-unsignaled event to receive the completion.
    // SAFETY: default security attrs (`None`), literal bools, null name.
    let event = match unsafe { CreateEventW(None, true, false, PCWSTR::null()) } {
        Ok(h) if !h.is_invalid() => h,
        _ => {
            std::thread::sleep(timeout);
            return;
        }
    };

    let overlapped = OVERLAPPED {
        hEvent: event,
        ..Default::default()
    };
    let mut handle = HANDLE::default();

    // Register for address-change notifications (overlapped => ERROR_IO_PENDING).
    // SAFETY: `handle` is a valid out-pointer; `overlapped` (with its live event)
    // outlives the wait and any cancellation below.
    let reg = unsafe { NotifyAddrChange(&mut handle, &overlapped) };
    if reg != ERROR_IO_PENDING.0 {
        // SAFETY: closing the freshly-created, unshared event handle exactly once.
        unsafe {
            let _ = CloseHandle(event);
        }
        std::thread::sleep(timeout);
        return;
    }

    // SAFETY: `event` is a live handle; block up to `ms` milliseconds.
    let wait = unsafe { WaitForSingleObject(event, ms) };
    if wait != WAIT_OBJECT_0 {
        // Timed out (or wait failed): cancel the pending notification so it can
        // never complete into `overlapped` after we return.
        // SAFETY: `overlapped` is exactly the one handed to NotifyAddrChange and
        // is still alive here.
        unsafe {
            let _ = CancelIPChangeNotify(&overlapped);
        }
    }

    // SAFETY: the notification has either completed or been cancelled; the event
    // is unshared and closed exactly once here.
    unsafe {
        let _ = CloseHandle(event);
    }
}
