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

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

use crate::WinError;

#[cfg(windows)]
mod win;

/// Parameters for installing the Zicade Windows service.
///
/// The service is registered as own-process, auto-start, launching `exe_path`
/// with `args` appended (typically `["service", "run"]` so the installed
/// service re-enters the binary in SCM mode).
#[derive(Debug, Clone)]
pub struct ServiceInstall {
    /// The service key name (what `sc`/SCM identify it by).
    pub name: String,
    /// The human-readable display name shown in `services.msc`.
    pub display_name: String,
    /// The service description.
    pub description: String,
    /// The absolute path to the service executable (usually the current exe).
    pub exe_path: PathBuf,
    /// Arguments appended after the exe path in the registered binary path.
    pub args: Vec<String>,
}

/// Install the service described by `config` (own-process, auto-start).
///
/// Requires Administrator; without it the underlying `OpenSCManagerW` fails
/// with access-denied, surfaced as [`WinError::Service`] (never a panic).
/// Off-Windows returns [`WinError::UnsupportedPlatform`].
pub fn install(config: &ServiceInstall) -> Result<(), WinError> {
    #[cfg(windows)]
    {
        win::install(config)
    }
    #[cfg(not(windows))]
    {
        let _ = config;
        Err(WinError::UnsupportedPlatform)
    }
}

/// Uninstall (delete) the service named `name`.
///
/// Requires Administrator. Off-Windows returns
/// [`WinError::UnsupportedPlatform`].
pub fn uninstall(name: &str) -> Result<(), WinError> {
    #[cfg(windows)]
    {
        win::uninstall(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Err(WinError::UnsupportedPlatform)
    }
}

/// Hand control to the SCM: connect to the dispatcher and run `service_body`
/// as the service, passing it a [`ServiceStop`] to await for graceful
/// shutdown.
///
/// This blocks until the service stops. Report progression is handled inside:
/// `START_PENDING` → `RUNNING`, then on STOP/SHUTDOWN `STOP_PENDING` →
/// (body returns) → `STOPPED`.
///
/// **Single service per process:** the non-capturing SCM entrypoint reads the
/// body and stop handle from module statics, so only one dispatcher may run in
/// a process at a time. A second concurrent call returns [`WinError::Service`].
/// Off-Windows returns [`WinError::UnsupportedPlatform`].
pub fn run_dispatcher<F>(name: &str, service_body: F) -> Result<(), WinError>
where
    F: FnOnce(ServiceStop) + Send + 'static,
{
    #[cfg(windows)]
    {
        win::run_dispatcher(name, service_body)
    }
    #[cfg(not(windows))]
    {
        let _ = (name, service_body);
        Err(WinError::UnsupportedPlatform)
    }
}

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
        loop {
            // Register with the notifier BEFORE re-checking the flag, so a
            // `trigger()` racing between the check and the await cannot be
            // missed (`notify_waiters` only wakes already-registered waiters).
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            if self.inner.stopped.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
            if self.inner.stopped.load(Ordering::SeqCst) {
                return;
            }
        }
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

    #[cfg(not(windows))]
    #[test]
    fn install_unsupported_off_windows() {
        let cfg = ServiceInstall {
            name: "Zicade".to_owned(),
            display_name: "Zicade".to_owned(),
            description: String::new(),
            exe_path: PathBuf::from("/nonexistent/zicade"),
            args: vec!["service".to_owned(), "run".to_owned()],
        };
        assert_eq!(install(&cfg), Err(WinError::UnsupportedPlatform));
    }

    #[cfg(not(windows))]
    #[test]
    fn uninstall_unsupported_off_windows() {
        assert_eq!(uninstall("Zicade"), Err(WinError::UnsupportedPlatform));
    }

    #[cfg(not(windows))]
    #[test]
    fn run_dispatcher_unsupported_off_windows() {
        let res = run_dispatcher("Zicade", |_stop| {});
        assert_eq!(res, Err(WinError::UnsupportedPlatform));
    }
}
