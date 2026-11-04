//! Prewarmed TCP sockets to an upstream proxy, so a `CONNECT` can skip the
//! dial.
//!
//! A pooled socket has never carried a request: the prewarm task dials it and
//! parks it here, and a `CONNECT` takes it and runs its own full
//! authentication handshake on it. Nothing is ever put back after a request,
//! so no authentication state and no unread response bytes can pass from one
//! `CONNECT` to the next.
//!
//! A pool belongs to one routing snapshot. Its prewarm task holds only a
//! [`Weak`] and is aborted when the pool drops, so replacing the routing (a
//! config apply, a network change, going direct) stops the dials and closes
//! the idle sockets instead of leaving a task behind per rebuild.

mod prewarm;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::task::AbortHandle;

use super::wire;

/// How many sockets to keep ready and for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolConfig {
    /// Idle sockets the prewarm task keeps ready.
    pub min_idle: usize,
    /// How long an idle socket is kept before it is closed and replaced, so a
    /// connection the gateway may have silently dropped is never handed out.
    pub idle_timeout: Duration,
}

/// Idle, never-used sockets to one upstream `host:port`.
pub struct UpstreamPool {
    addr: String,
    config: PoolConfig,
    idle: Mutex<VecDeque<Idle>>,
    /// Wakes the prewarm task when a take may have left the pool short.
    wake: Arc<Notify>,
    /// Set once the prewarm task is running; aborted when the pool drops.
    prewarm: OnceLock<AbortHandle>,
}

struct Idle {
    stream: TcpStream,
    since: Instant,
}

/// What the prewarm task should do next.
#[derive(Debug, PartialEq, Eq)]
enum Refresh {
    /// At the watermark; nothing to do before the oldest socket expires.
    Full { next_expiry: Duration },
    /// Below the watermark. `closed_by_peer` counts sockets the upstream
    /// closed or wrote to while parked: a gateway that does that to every new
    /// connection must be backed off from, not redialed in a loop.
    Short { closed_by_peer: usize },
}

impl UpstreamPool {
    /// A pool for `addr`. Prewarming starts with the first `CONNECT` that asks
    /// for a socket, so an upstream that is never used is never dialed.
    #[must_use]
    pub fn new(addr: impl Into<String>, config: PoolConfig) -> Arc<Self> {
        Arc::new(Self {
            addr: addr.into(),
            config,
            idle: Mutex::new(VecDeque::new()),
            wake: Arc::new(Notify::new()),
            prewarm: OnceLock::new(),
        })
    }

    /// The upstream `host:port` this pool dials.
    #[must_use]
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Sockets currently parked, ready or not yet found stale.
    #[must_use]
    pub fn idle_count(&self) -> usize {
        self.lock().len()
    }

    /// A ready socket, if one is parked and still usable; starts the prewarm
    /// task on first use and wakes it to replace what was taken.
    pub(crate) fn take(self: &Arc<Self>) -> Option<TcpStream> {
        self.start_prewarm();
        let stream = self.take_idle();
        self.wake.notify_one();
        stream
    }

    /// Pop the newest usable socket, closing expired or unusable ones met on
    /// the way.
    fn take_idle(&self) -> Option<TcpStream> {
        let mut idle = self.lock();
        while let Some(entry) = idle.pop_back() {
            if self.usable(&entry) {
                return Some(entry.stream);
            }
        }
        None
    }

    /// Close expired or unusable sockets and say whether to dial.
    fn refresh(&self) -> Refresh {
        let mut idle = self.lock();
        let before = idle.len();
        idle.retain(|entry| entry.since.elapsed() < self.config.idle_timeout);
        let expired = before - idle.len();
        idle.retain(|entry| wire::is_idle_and_open(&entry.stream));
        let closed_by_peer = before - expired - idle.len();
        if idle.len() < self.config.min_idle {
            return Refresh::Short { closed_by_peer };
        }
        let timeout = self.config.idle_timeout;
        let oldest = idle.iter().map(|entry| entry.since).min();
        Refresh::Full {
            next_expiry: oldest.map_or(timeout, |since| timeout.saturating_sub(since.elapsed())),
        }
    }

    /// Park a freshly dialed socket; one beyond the watermark is closed.
    fn put(&self, stream: TcpStream) {
        let mut idle = self.lock();
        if idle.len() < self.config.min_idle {
            idle.push_back(Idle {
                stream,
                since: Instant::now(),
            });
        }
    }

    fn usable(&self, entry: &Idle) -> bool {
        entry.since.elapsed() < self.config.idle_timeout && wire::is_idle_and_open(&entry.stream)
    }

    fn start_prewarm(self: &Arc<Self>) {
        if self.prewarm.get().is_some() {
            return;
        }
        // Routing can be rebuilt on a thread with no runtime; the request path
        // that calls this always has one.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        self.prewarm.get_or_init(|| {
            let task = prewarm::run(
                Arc::downgrade(self),
                Arc::clone(&self.wake),
                self.addr.clone(),
            );
            runtime.spawn(task).abort_handle()
        });
    }

    /// The idle list. A panic while it was held cannot leave it half-written,
    /// so a poisoned lock is recovered rather than giving up on the pool.
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Idle>> {
        self.idle.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for UpstreamPool {
    fn drop(&mut self) {
        if let Some(task) = self.prewarm.get() {
            task.abort();
        }
    }
}

impl fmt::Debug for UpstreamPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpstreamPool")
            .field("addr", &self.addr)
            .field("config", &self.config)
            .field("idle", &self.idle_count())
            .finish()
    }
}

/// One pool per upstream a routing snapshot sends `CONNECT`s to, created on
/// first use and dropped with the snapshot.
///
/// A PAC script can name any number of proxies, so the number of pools is
/// capped; an upstream past the cap is simply dialed per request.
#[derive(Debug)]
pub struct UpstreamPools {
    config: PoolConfig,
    pools: Mutex<HashMap<String, Arc<UpstreamPool>>>,
}

impl UpstreamPools {
    /// Most upstreams one snapshot keeps sockets warm for.
    pub const MAX_UPSTREAMS: usize = 8;

    /// An empty registry whose pools all use `config`.
    #[must_use]
    pub fn new(config: PoolConfig) -> Self {
        Self {
            config,
            pools: Mutex::new(HashMap::new()),
        }
    }

    /// The pool for `addr`, created on first use; `None` once
    /// [`Self::MAX_UPSTREAMS`] other upstreams already have one.
    #[must_use]
    pub fn get(&self, addr: &str) -> Option<Arc<UpstreamPool>> {
        let mut pools = self.pools.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(pool) = pools.get(addr) {
            return Some(Arc::clone(pool));
        }
        if pools.len() >= Self::MAX_UPSTREAMS {
            tracing::debug!(
                upstream = addr,
                "upstream socket pool limit reached; dialing per request"
            );
            return None;
        }
        let pool = UpstreamPool::new(addr, self.config);
        pools.insert(addr.to_owned(), Arc::clone(&pool));
        Some(pool)
    }
}
