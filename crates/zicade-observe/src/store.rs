//! The bounded in-memory ring buffer plus broadcast fan-out.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::event::LogEvent;

/// Default broadcast channel depth for SSE subscribers.
const BROADCAST_DEPTH: usize = 256;

/// A cloneable handle to a bounded log buffer and its live broadcast channel.
///
/// Cloning is cheap (an `Arc` bump); every clone observes the same ring and the
/// same broadcast sender. The ring drops the oldest event once it reaches `cap`.
#[derive(Clone)]
pub struct LogStore {
    inner: Arc<Inner>,
}

struct Inner {
    ring: Mutex<VecDeque<LogEvent>>,
    cap: usize,
    tx: broadcast::Sender<LogEvent>,
}

impl LogStore {
    /// Create a store whose ring retains at most `cap` events (min 1).
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        let (tx, _rx) = broadcast::channel(BROADCAST_DEPTH.max(cap));
        Self {
            inner: Arc::new(Inner {
                ring: Mutex::new(VecDeque::with_capacity(cap)),
                cap,
                tx,
            }),
        }
    }

    /// Append an event, evicting the oldest if the ring is full, then broadcast
    /// it to any live subscribers. A send error (no subscribers) is ignored.
    pub fn push(&self, event: LogEvent) {
        if let Ok(mut ring) = self.inner.ring.lock() {
            if ring.len() == self.inner.cap {
                ring.pop_front();
            }
            ring.push_back(event.clone());
        }
        let _ = self.inner.tx.send(event);
    }

    /// Snapshot the buffered events, oldest first.
    pub fn recent(&self) -> Vec<LogEvent> {
        self.inner
            .ring
            .lock()
            .map(|ring| ring.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Subscribe to the live event stream (used by the SSE endpoint).
    pub fn subscribe(&self) -> broadcast::Receiver<LogEvent> {
        self.inner.tx.subscribe()
    }

    /// Configured ring capacity.
    pub fn capacity(&self) -> usize {
        self.inner.cap
    }
}
