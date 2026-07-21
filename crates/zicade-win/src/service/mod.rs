//! Windows Service (SCM) integration.
//!
//! The public surface here is cross-platform and safe; all FFI lives in
//! [`win`] behind `cfg(windows)` and degrades to
//! [`WinError::UnsupportedPlatform`] off-Windows.
//!
//! [`ServiceStop`] is the seam between the SCM control handler (which runs on
//! an OS thread, outside any async runtime) and the app's async shutdown: the
//! handler calls [`ServiceStop::trigger`], and the app awaits
//! [`ServiceStop::wait`] as its shutdown future.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

/// A safe, cloneable stop handle bridging the SCM control handler to the app's
/// async shutdown.
///
/// The control handler (a bare `extern "system"` callback with no captures)
/// signals a stop by calling [`trigger`](Self::trigger) from an arbitrary OS
/// thread; the app awaits [`wait`](Self::wait) inside its tokio runtime. Both
/// operations are safe and thread-safe.
#[derive(Clone)]
pub struct ServiceStop {
    inner: Arc<Inner>,
}

struct Inner {
    stopped: AtomicBool,
    notify: Notify,
}

impl ServiceStop {
    /// Create an un-triggered stop handle.
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                stopped: AtomicBool::new(false),
                notify: Notify::new(),
            }),
        }
    }

    /// Signal a stop. Safe to call from any thread, including outside a tokio
    /// runtime (as the SCM control handler is). Idempotent.
    pub fn trigger(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    /// Whether a stop has been signalled.
    pub fn is_triggered(&self) -> bool {
        self.inner.stopped.load(Ordering::SeqCst)
    }

    /// Resolve once a stop has been signalled (returning immediately if it
    /// already has). Await this as the app's shutdown future.
    pub async fn wait(&self) {
        // Stub (red): does not actually wait.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_pends_until_triggered() {
        let stop = ServiceStop::new();
        let waiter = stop.clone();
        let handle = tokio::spawn(async move { waiter.wait().await });

        // Give the task a chance to run; it must still be pending (no trigger).
        tokio::task::yield_now().await;
        assert!(!handle.is_finished(), "wait resolved before trigger");

        stop.trigger();
        tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("wait did not resolve after trigger")
            .expect("waiter task panicked");
    }

    #[tokio::test]
    async fn wait_returns_immediately_when_already_triggered() {
        let stop = ServiceStop::new();
        stop.trigger();
        assert!(stop.is_triggered());
        tokio::time::timeout(std::time::Duration::from_secs(5), stop.wait())
            .await
            .expect("wait hung despite an already-triggered stop");
    }
}
