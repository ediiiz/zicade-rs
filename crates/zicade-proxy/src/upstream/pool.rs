//! Idle-connection prewarming for upstream proxy sockets.
//!
//! ## Why prewarm at all
//!
//! zicade deliberately opens a *fresh* upstream TCP connection per request
//! (NTLM authenticates the connection, so one authenticator lifecycle maps to
//! one connection — see `forward_via_upstream`). That is correct, but every
//! request then pays a fresh `connect()` to the gateway: one full RTT
//! (measured 49–78 ms on typical corporate netentsec access points) before the
//! `CONNECT` round trip even starts.
//!
//! What the pool reuses is therefore **only the TCP socket**, never an
//! established tunnel: a `CONNECT`ed socket is exclusive to one destination for
//! its whole life and is spliced away by `tunnel::splice`, so it can never
//! return to the pool. The pool holds **un-tunnelled, un-authenticated**
//! sockets and hands them out to whichever handshake runs next.
//!
//! Because a tunnelled socket never comes back, the pool cannot refill itself
//! from returned sockets alone — a request-heavy burst drains it and every
//! later request is back to a cold dial. That is why this pool is filled by a
//! **background prewarm task** ([`spawn_prewarm`]) that keeps dialing up to a
//! target watermark, rather than by callers returning sockets.
//!
//! ## Why handing out a raw socket is safe
//!
//! The key empirical fact (measured against the corporate gateway): its
//! authentication state is **not bound to the TCP connection**. Repeated
//! *fresh* connections to the same access point returned `200` without any
//! credentials, and a single `407` observed mid-run was followed immediately by
//! plain `200`s on new connections. Whatever session state the gateway keeps,
//! it survives socket teardown and re-creation.
//!
//! So reusing a socket cannot "leak" authentication from one request into
//! another in a way a fresh socket would not have had anyway — and the 407
//! loop in `connect_handshake` still runs on the reused socket whenever the
//! gateway does ask.
//!
//! ## Hygiene
//!
//! * Idle sockets are [`shrink`]d before entering the pool, discarding any
//!   unread bytes a caller may have left behind (e.g. the tail of a drained 407
//!   body). This is what makes a handoff safe without trusting the caller.
//! * Idle sockets are dropped after [`PoolConfig::idle_timeout`] so the gateway
//!   never sees a half-open socket it would be right to reclaim.
//! * A reused socket the gateway has closed fails its first write; the caller
//!   treats that as a plain connect failure and falls back to a fresh dial.
//! * The prewarm task never dials more often than [`PoolConfig::prewarm_interval`]
//!   and stops dialing once [`UpstreamPool::len`] reaches
//!   [`PoolConfig::min_idle`], so an idle machine hands the gateway back
//!   almost nothing.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;

/// Tunables for [`UpstreamPool`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolConfig {
    /// Maximum number of idle sockets kept per upstream address.
    pub max_idle: usize,
    /// How long an idle socket may sit unused before it is dropped.
    pub idle_timeout: Duration,
    /// Watermark the prewarm task keeps the pool at.
    ///
    /// Kept well below [`Self::max_idle`]: the goal is "a burst never hits a
    /// cold dial", not "hold the gateway's whole connection table open".
    pub min_idle: usize,
    /// Minimum spacing between prewarm dials, so a dead gateway is not hammered.
    pub prewarm_interval: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        // 32 idle sockets covers a browser's per-origin parallelism (typically
        // 6–8 per origin, across a handful of origins) with headroom, while
        // staying small enough that a quiet machine hands the gateway back
        // almost nothing. 20 s is comfortably below the gateway's own idle
        // reaping and below zicade's default `socktimeout` of 20 s.
        //
        // The watermark is 8: that already covers a browser opening a page
        // with several parallel requests, and a real burst refills on demand
        // via `dial()` instead of us holding 32 sockets open for nothing.
        Self {
            max_idle: 32,
            idle_timeout: Duration::from_secs(20),
            min_idle: 8,
            prewarm_interval: Duration::from_millis(50),
        }
    }
}

/// A socket returned to the pool, with the instant it went idle.
#[derive(Debug)]
struct Idle {
    stream: TcpStream,
    since: Instant,
}

/// Shared pool of idle upstream sockets, keyed by `host:port`.
#[derive(Debug)]
pub struct UpstreamPool {
    config: PoolConfig,
    addr: String,
    idle: Mutex<VecDeque<(String, Idle)>>,
    /// Number of sockets handed out that came from the pool (diagnostics).
    hits: AtomicUsize,
    /// Number of pooled sockets dropped because they were stale/dead.
    discards: AtomicUsize,
    /// Number of sockets created by the prewarm task.
    warmed: AtomicUsize,
}

impl UpstreamPool {
    /// Create a pool for `addr` with the given limits.
    #[must_use]
    pub fn new(addr: impl Into<String>, config: PoolConfig) -> Self {
        Self {
            config,
            addr: addr.into(),
            idle: Mutex::new(VecDeque::new()),
            hits: AtomicUsize::new(0),
            discards: AtomicUsize::new(0),
            warmed: AtomicUsize::new(0),
        }
    }

    /// The configuration this pool was built with.
    #[must_use]
    pub fn config(&self) -> PoolConfig {
        self.config
    }

    /// The upstream address this pool serves.
    #[must_use]
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Number of sockets currently idle.
    #[must_use]
    pub fn len(&self) -> usize {
        let queue = self
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.len()
    }

    /// Whether the pool holds no idle sockets.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Take an idle socket for `addr`, if one is still fresh.
    ///
    /// Stale entries are dropped on the way past, so a single call also
    /// performs the housekeeping that keeps the pool bounded.
    pub fn take(&self, addr: &str) -> Option<TcpStream> {
        let mut queue = self
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut i = 0;
        while i < queue.len() {
            if queue[i].0 != addr {
                i += 1;
                continue;
            }
            // Reached: the nth entry for this address. Remove it and hand it
            // out if it is still fresh; otherwise drop it and keep looking.
            let (_, idle) = queue.remove(i)?;
            if idle.since.elapsed() <= self.config.idle_timeout && idle.stream.peer_addr().is_ok() {
                self.hits.fetch_add(1, Ordering::Relaxed);
                tracing::trace!(upstream = addr, "upstream socket reused from pool");
                return Some(idle.stream);
            }
            self.discards.fetch_add(1, Ordering::Relaxed);
        }
        None
    }

    /// Return a socket to the pool.
    ///
    /// The socket is shrunk first so no unread bytes carry over into the next
    /// request. A socket the peer has closed is dropped rather than pooled.
    pub fn put(&self, addr: &str, mut stream: TcpStream) {
        if let Err(err) = super::wire::shrink(&mut stream) {
            self.discards.fetch_add(1, Ordering::Relaxed);
            tracing::trace!(upstream = addr, error = %err, "pooled socket not reusable");
            return;
        }
        let mut queue = self
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Evict stale entries for every address, and anything beyond the cap
        // for this one, before inserting.
        queue.retain(|(_, idle)| idle.since.elapsed() <= self.config.idle_timeout);
        let mut same_addr = queue.iter().filter(|(a, _)| a == addr).count();
        while same_addr >= self.config.max_idle {
            match queue.iter().position(|(a, _)| a == addr) {
                Some(pos) => {
                    queue.remove(pos);
                    self.discards.fetch_add(1, Ordering::Relaxed);
                    same_addr -= 1;
                }
                None => break,
            }
        }
        queue.push_back((
            addr.to_owned(),
            Idle {
                stream,
                since: Instant::now(),
            },
        ));
    }

    /// Drop every idle socket (used on shutdown/reconfigure).
    pub fn clear(&self) {
        let mut queue = self
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.clear();
    }

    /// `(reuse_hits, discards, warmed)` since start, for diagnostics.
    #[must_use]
    pub fn stats(&self) -> (usize, usize, usize) {
        (
            self.hits.load(Ordering::Relaxed),
            self.discards.load(Ordering::Relaxed),
            self.warmed.load(Ordering::Relaxed),
        )
    }
}

/// Keep `pool` filled up to [`PoolConfig::min_idle`] until the task is dropped.
///
/// Returns a [`JoinHandle`]; the task runs until aborted. The app aborts it on
/// reconfigure/shutdown alongside the rest of its background work.
///
/// # Errors
///
/// Individual dials may fail (gateway down, DNS hiccup); those are logged at
/// `debug` and retried after [`PoolConfig::prewarm_interval`]. The task itself
/// only ends when aborted.
pub fn spawn_prewarm(pool: std::sync::Arc<UpstreamPool>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let config = pool.config();
        let addr = pool.addr().to_owned();
        tracing::info!(
            upstream = %addr,
            min_idle = config.min_idle,
            "upstream socket prewarm started"
        );
        loop {
            // Refill one socket per tick so a dead gateway costs one dial per
            // interval instead of a tight retry loop.
            // Stop at the watermark: `len()` is read before the await, so a
            // burst can push it slightly over between checks; the extra
            // sockets are then simply consumed by that burst.
            if pool.len() < config.min_idle {
                match TcpStream::connect(&addr).await {
                    Ok(stream) => {
                        if let Err(err) = stream.set_nodelay(true) {
                            tracing::debug!(error = %err, "prewarm: set TCP_NODELAY failed");
                        }
                        pool.warmed.fetch_add(1, Ordering::Relaxed);
                        pool.put(&addr, stream);
                        tracing::debug!(upstream = %addr, size = pool.len(), "prewarm refilled");
                    }
                    Err(err) => {
                        tracing::debug!(upstream = %addr, error = %err, "prewarm dial failed");
                    }
                }
            }
            tokio::time::sleep(config.prewarm_interval).await;
        }
    })
}
