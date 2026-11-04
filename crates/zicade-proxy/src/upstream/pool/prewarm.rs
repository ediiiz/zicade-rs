//! The background task that keeps a pool topped up.

use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::Notify;

use super::{Refresh, UpstreamPool};

/// Bound on one prewarm dial, so a gateway that swallows SYNs cannot park the
/// task forever.
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest a full pool goes without checking its sockets, so one the gateway
/// closed is replaced before a `CONNECT` finds it.
const RECHECK: Duration = Duration::from_secs(5);

/// Keep the pool at its watermark until the pool is dropped.
///
/// The task holds only a [`Weak`] between steps: it must not be what keeps a
/// replaced routing snapshot's pool, and its sockets, alive.
pub(super) async fn run(pool: Weak<UpstreamPool>, wake: Arc<Notify>, addr: String) {
    tracing::debug!(upstream = %addr, "upstream socket prewarm started");
    let mut backoff = Backoff::default();
    // The upgraded `Arc` is consumed by `refresh`, so it is never held across
    // an await.
    while let Some(refresh) = pool.upgrade().map(|pool| pool.refresh()) {
        match refresh {
            Refresh::Full { next_expiry } => {
                backoff.reset();
                // Either a take woke us or a socket is about to expire: both
                // mean look again, so the timeout's result carries nothing.
                let _ = tokio::time::timeout(next_expiry.min(RECHECK), wake.notified()).await;
                continue;
            }
            Refresh::Short { closed_by_peer } if closed_by_peer > 0 => {
                wait(&mut backoff, &addr, "upstream closed parked sockets").await;
            }
            Refresh::Short { .. } => {}
        }
        match dial(&addr).await {
            Ok(stream) => {
                let Some(pool) = pool.upgrade() else { break };
                pool.put(stream);
            }
            Err(err) => wait(&mut backoff, &addr, &format!("dial failed: {err}")).await,
        }
    }
    tracing::debug!(upstream = %addr, "upstream socket prewarm stopped");
}

async fn wait(backoff: &mut Backoff, addr: &str, why: &str) {
    let delay = backoff.next_delay();
    tracing::debug!(
        upstream = %addr,
        retry_in_ms = delay.as_millis(),
        "upstream socket prewarm backing off: {why}"
    );
    tokio::time::sleep(delay).await;
}

async fn dial(addr: &str) -> std::io::Result<TcpStream> {
    match tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("dial timed out after {}s", DIAL_TIMEOUT.as_secs()),
        )),
    }
}

/// Exponential delay between failed dials, so an unreachable or hostile
/// gateway costs a dial every half minute rather than a tight loop. Reset only
/// once the pool is full again.
#[derive(Debug)]
pub(super) struct Backoff {
    next: Duration,
}

impl Backoff {
    pub(super) const FIRST: Duration = Duration::from_millis(500);
    pub(super) const MAX: Duration = Duration::from_secs(30);

    /// The delay before the next attempt; doubles each call up to [`Self::MAX`].
    pub(super) fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = (self.next * 2).min(Self::MAX);
        delay
    }

    /// Back to [`Self::FIRST`] once the pool has filled.
    pub(super) fn reset(&mut self) {
        self.next = Self::FIRST;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self { next: Self::FIRST }
    }
}
