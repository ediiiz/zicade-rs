//! Acceptance tests for network monitoring (`zicade-win::netmon`).
//!
//! Adapter/network state is environment-dependent, so the Windows tests only
//! require the calls return without panicking and that `wait_for_network_change`
//! honors its timeout. Off-Windows we assert the graceful stubs (no suffixes; a
//! sleep for the timeout).

use std::time::{Duration, Instant};

use zicade_win::{active_dns_suffixes, wait_for_network_change};

#[cfg(windows)]
#[test]
fn active_dns_suffixes_enumerates_without_panic() {
    // The set of suffixes depends on the machine's adapters; we only require the
    // call returns a Vec without panicking and is repeatable.
    let first = active_dns_suffixes();
    let second = active_dns_suffixes();
    assert_eq!(
        first.len(),
        second.len(),
        "two back-to-back enumerations should agree on this idle box"
    );
    for s in &first {
        assert!(!s.is_empty(), "no empty suffix should be reported");
    }
}

#[cfg(windows)]
#[test]
fn wait_for_network_change_returns_by_timeout() {
    // With no network change forced, the call must return around the timeout and
    // never hang. Allow generous slack for the overlapped setup/cancel.
    let start = Instant::now();
    wait_for_network_change(Duration::from_millis(200));
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "wait must return promptly by its timeout, took {elapsed:?}"
    );
}

#[cfg(not(windows))]
#[test]
fn active_dns_suffixes_empty_off_windows() {
    assert!(active_dns_suffixes().is_empty());
}

#[cfg(not(windows))]
#[test]
fn wait_for_network_change_sleeps_off_windows() {
    let start = Instant::now();
    wait_for_network_change(Duration::from_millis(50));
    assert!(start.elapsed() >= Duration::from_millis(40));
}
