//! Per-connection bookkeeping: every forwarded HTTP request and `CONNECT`
//! tunnel gets an entry while it runs, then a place in a bounded history.
//!
//! An entry is owned by a [`Tracked`] handle. Dropping the last handle closes
//! the entry — on success, error, or panic alike — so a dead connection can
//! never linger as "open".

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use crate::metrics::ProxyMetrics;

/// How many finished connections the history keeps.
pub(crate) const HISTORY_LEN: usize = 200;

/// What kind of traffic a connection carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionKind {
    /// A `CONNECT` tunnel (usually HTTPS); its contents are opaque.
    Tunnel,
    /// A plain-HTTP request; `status` is set once the response arrives.
    Http { method: String, status: Option<u16> },
}

/// Where a connection is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// Routing or dialing; no bytes relayed yet.
    Connecting,
    /// Relaying bytes.
    Open,
    /// Finished normally.
    Closed,
    /// Finished with an error, described for the user.
    Failed(String),
}

/// A point-in-time copy of one connection, for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionSnapshot {
    /// Unique for the life of the proxy; stable across snapshots.
    pub id: u64,
    pub kind: ConnectionKind,
    /// `host:port` the client asked for.
    pub target: String,
    /// `"direct"`, or the upstream proxy `host:port` the request went through.
    pub route: String,
    /// The client's address, when known.
    pub client: Option<SocketAddr>,
    pub opened_at: SystemTime,
    /// Time open so far, or total time once finished.
    pub duration: Duration,
    /// Bytes from the client towards the origin.
    pub bytes_up: u64,
    /// Bytes from the origin back to the client.
    pub bytes_down: u64,
    pub state: ConnectionState,
}

/// The tracker's book: open entries by id, and the finished ones.
#[derive(Clone, Default, Debug)]
pub(crate) struct Tracker {
    next_id: Arc<AtomicU64>,
    book: Arc<Mutex<Book>>,
}

#[derive(Default, Debug)]
struct Book {
    open: BTreeMap<u64, Arc<Entry>>,
    /// Newest last.
    closed: VecDeque<ConnectionSnapshot>,
}

#[derive(Debug)]
struct Entry {
    id: u64,
    target: String,
    client: Option<SocketAddr>,
    opened_at: SystemTime,
    started: Instant,
    up: AtomicU64,
    down: AtomicU64,
    failed: AtomicBool,
    fields: Mutex<Fields>,
}

#[derive(Debug)]
struct Fields {
    kind: ConnectionKind,
    route: String,
    state: ConnectionState,
}

impl Entry {
    fn snapshot(&self, duration: Duration) -> ConnectionSnapshot {
        let fields = self.fields.lock().unwrap_or_else(PoisonError::into_inner);
        ConnectionSnapshot {
            id: self.id,
            kind: fields.kind.clone(),
            target: self.target.clone(),
            route: fields.route.clone(),
            client: self.client,
            opened_at: self.opened_at,
            duration,
            bytes_up: self.up.load(Ordering::Relaxed),
            bytes_down: self.down.load(Ordering::Relaxed),
            state: fields.state.clone(),
        }
    }

    fn update(&self, change: impl FnOnce(&mut Fields)) {
        change(&mut self.fields.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

impl Tracker {
    fn open(&self, entry: Arc<Entry>) {
        self.book
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .open
            .insert(entry.id, entry);
    }

    fn close(&self, entry: &Entry) {
        entry.update(|fields| {
            if !matches!(fields.state, ConnectionState::Failed(_)) {
                fields.state = ConnectionState::Closed;
            }
        });
        let finished = entry.snapshot(entry.started.elapsed());
        let mut book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
        book.open.remove(&entry.id);
        if book.closed.len() == HISTORY_LEN {
            book.closed.pop_front();
        }
        book.closed.push_back(finished);
    }

    /// Open connections first, then finished ones; newest first in each.
    pub(crate) fn snapshot(&self) -> Vec<ConnectionSnapshot> {
        let book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
        let open = book
            .open
            .values()
            .rev()
            .map(|entry| entry.snapshot(entry.started.elapsed()));
        open.chain(book.closed.iter().rev().cloned()).collect()
    }
}

/// The owner of one tracked connection. Byte counts go to both the entry and
/// the proxy-wide totals, as they are relayed.
#[derive(Debug)]
pub(crate) struct Tracked {
    entry: Arc<Entry>,
    metrics: ProxyMetrics,
}

impl Tracked {
    /// Register a new connection and count it as a request.
    pub(crate) fn start(
        metrics: &ProxyMetrics,
        kind: ConnectionKind,
        target: String,
        route: String,
        client: Option<SocketAddr>,
    ) -> Self {
        let tracker = metrics.tracker();
        let entry = Arc::new(Entry {
            id: tracker.next_id.fetch_add(1, Ordering::Relaxed) + 1,
            target,
            client,
            opened_at: SystemTime::now(),
            started: Instant::now(),
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            fields: Mutex::new(Fields {
                kind,
                route,
                state: ConnectionState::Connecting,
            }),
        });
        tracker.open(Arc::clone(&entry));
        metrics.incr_request();
        Self {
            entry,
            metrics: metrics.clone(),
        }
    }

    /// Client -> origin bytes, just relayed.
    pub(crate) fn add_up(&self, n: u64) {
        self.entry.up.fetch_add(n, Ordering::Relaxed);
        self.metrics.add_bytes(0, n);
    }

    /// Origin -> client bytes, just relayed.
    pub(crate) fn add_down(&self, n: u64) {
        self.entry.down.fetch_add(n, Ordering::Relaxed);
        self.metrics.add_bytes(n, 0);
    }

    /// The route was decided (PAC resolves per request).
    pub(crate) fn set_route(&self, route: impl Into<String>) {
        let route = route.into();
        self.entry.update(|fields| fields.route = route);
    }

    /// Bytes are flowing.
    pub(crate) fn set_open(&self) {
        self.entry.update(|fields| {
            if fields.state == ConnectionState::Connecting {
                fields.state = ConnectionState::Open;
            }
        });
    }

    /// The origin answered an HTTP request with `status`.
    pub(crate) fn set_status(&self, status: u16) {
        self.entry.update(|fields| {
            if let ConnectionKind::Http { status: slot, .. } = &mut fields.kind {
                *slot = Some(status);
            }
        });
        self.set_open();
    }

    /// The connection failed; counted once however often it is reported.
    pub(crate) fn fail(&self, reason: impl ToString) {
        if self.entry.failed.swap(true, Ordering::Relaxed) {
            return;
        }
        self.metrics.incr_failed();
        let reason = reason.to_string();
        self.entry
            .update(|fields| fields.state = ConnectionState::Failed(reason));
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.metrics.tracker().close(&self.entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(metrics: &ProxyMetrics, target: &str) -> Tracked {
        Tracked::start(
            metrics,
            ConnectionKind::Tunnel,
            target.to_owned(),
            "direct".to_owned(),
            None,
        )
    }

    #[test]
    fn the_history_keeps_only_the_newest_finished_connections() {
        let metrics = ProxyMetrics::default();
        for i in 0..HISTORY_LEN + 5 {
            drop(start(&metrics, &format!("host-{i}:443")));
        }

        let all = metrics.connections();

        assert_eq!(all.len(), HISTORY_LEN);
        assert_eq!(all[0].target, format!("host-{}:443", HISTORY_LEN + 4));
        assert_eq!(metrics.total_requests(), (HISTORY_LEN + 5) as u64);
    }

    #[test]
    fn open_connections_come_first_and_a_failure_survives_closing() {
        let metrics = ProxyMetrics::default();
        let failed = start(&metrics, "bad:443");
        failed.fail("connection refused");
        failed.fail("reported twice");
        drop(failed);
        let open = start(&metrics, "good:443");
        open.set_open();

        let all = metrics.connections();

        assert_eq!(all[0].target, "good:443");
        assert_eq!(all[0].state, ConnectionState::Open);
        assert_eq!(
            all[1].state,
            ConnectionState::Failed("connection refused".to_owned())
        );
        assert_eq!(metrics.failed_requests(), 1);
    }

    #[test]
    fn bytes_reach_the_entry_and_the_totals_as_they_move() {
        let metrics = ProxyMetrics::default();
        let tracked = start(&metrics, "host:443");
        tracked.add_up(10);
        tracked.add_down(30);

        let entry = &metrics.connections()[0];

        assert_eq!((entry.bytes_up, entry.bytes_down), (10, 30));
        assert_eq!((metrics.bytes_out(), metrics.bytes_in()), (10, 30));
    }
}
