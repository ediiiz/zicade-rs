//! Network monitoring for the corporate-network gate.
//!
//! Two capabilities, both cross-platform in signature with the Win32 FFI behind
//! `cfg(windows)` in [`win`]:
//!
//! - [`active_dns_suffixes`] enumerates the connection-specific DNS suffixes of
//!   the currently-up network adapters (what the pure `on_corp` matcher in the
//!   app compares against the configured corporate suffixes).
//! - [`wait_for_network_change`] blocks until Windows reports an address change
//!   OR the given timeout elapses — giving the gate an event-driven wake with a
//!   polling fallback in a single call.
//!
//! Off-Windows both degrade gracefully: no suffixes, and a plain sleep.

use std::time::Duration;

#[cfg(windows)]
mod win;

/// The connection-specific DNS suffixes of the network adapters that are
/// currently up (e.g. `["dy.droot.org"]`). Empty off-Windows, or when no
/// adapter is up / enumeration fails (the gate then falls back to the configured
/// routing, per design).
pub fn active_dns_suffixes() -> Vec<String> {
    #[cfg(windows)]
    {
        win::active_dns_suffixes()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Block until the OS reports a network address change or `timeout` elapses,
/// whichever comes first. Returns in both cases; callers re-evaluate the network
/// afterwards. Off-Windows this is a plain [`std::thread::sleep`] for `timeout`
/// (pure polling).
pub fn wait_for_network_change(timeout: Duration) {
    #[cfg(windows)]
    {
        win::wait_for_network_change(timeout);
    }
    #[cfg(not(windows))]
    {
        std::thread::sleep(timeout);
    }
}
