//! Live proxy metrics with an RAII connection guard.
//!
//! The guard decrements the active count on drop — including on panic — so a
//! failing or hung connection cannot leak bookkeeping (LESSON-1/LESSON-7).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Cloneable handle to the proxy's live counters.
#[derive(Clone, Default, Debug)]
pub struct ProxyMetrics {
    active: Arc<AtomicUsize>,
    total_requests: Arc<AtomicU64>,
    failed_requests: Arc<AtomicU64>,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
}

impl ProxyMetrics {
    /// Number of connections currently being served.
    pub fn active_connections(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    /// Total HTTP requests forwarded since start.
    pub fn total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }

    /// Total forwarded requests that failed (produced a `502`/error) since start.
    pub fn failed_requests(&self) -> u64 {
        self.failed_requests.load(Ordering::Relaxed)
    }

    pub(crate) fn incr_request(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn incr_failed(&self) {
        self.failed_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Total bytes streamed FROM origins/upstreams back to clients (download).
    pub fn bytes_in(&self) -> u64 {
        self.bytes_in.load(Ordering::Relaxed)
    }

    /// Total bytes streamed FROM clients out to origins/upstreams (upload).
    pub fn bytes_out(&self) -> u64 {
        self.bytes_out.load(Ordering::Relaxed)
    }

    /// Add tunneled byte counts (see [`Self::bytes_in`]/[`Self::bytes_out`]).
    pub(crate) fn add_bytes(&self, inbound: u64, outbound: u64) {
        if inbound > 0 {
            self.bytes_in.fetch_add(inbound, Ordering::Relaxed);
        }
        if outbound > 0 {
            self.bytes_out.fetch_add(outbound, Ordering::Relaxed);
        }
    }

    /// Register a new active connection; the returned guard decrements the
    /// count when dropped.
    pub(crate) fn connection_guard(&self) -> ConnectionGuard {
        self.active.fetch_add(1, Ordering::Relaxed);
        ConnectionGuard {
            active: self.active.clone(),
        }
    }
}

pub(crate) struct ConnectionGuard {
    active: Arc<AtomicUsize>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::ProxyMetrics;

    #[test]
    fn incr_failed_bumps_failed_requests() {
        let metrics = ProxyMetrics::default();
        assert_eq!(metrics.failed_requests(), 0);
        metrics.incr_failed();
        metrics.incr_failed();
        assert_eq!(metrics.failed_requests(), 2);
        // Failures are counted independently of total requests.
        assert_eq!(metrics.total_requests(), 0);
    }

    #[test]
    fn add_bytes_accumulates_each_direction() {
        let metrics = ProxyMetrics::default();
        assert_eq!((metrics.bytes_in(), metrics.bytes_out()), (0, 0));
        metrics.add_bytes(100, 40);
        metrics.add_bytes(0, 60); // upload-only chunk; zero inbound is a no-op
        assert_eq!(metrics.bytes_in(), 100);
        assert_eq!(metrics.bytes_out(), 100);
    }
}
