//! What the log view holds: the newest events, each shown once.

use std::collections::VecDeque;

use zicade_observe::LogEvent;

/// A capped, de-duplicated tail of the log stream.
pub struct LogBuffer {
    capacity: usize,
    last_seq: u64,
    events: VecDeque<LogEvent>,
}

impl LogBuffer {
    /// Keep at most `capacity` events.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            last_seq: 0,
            events: VecDeque::with_capacity(capacity),
        }
    }

    /// Append `event` unless it was already seen. The view subscribes before
    /// replaying the backlog so nothing falls in between, which means the
    /// first live events may repeat backlog ones; the store's sequence numbers
    /// tell them apart.
    pub fn push(&mut self, event: LogEvent) {
        if event.seq != 0 && event.seq <= self.last_seq {
            return;
        }
        self.last_seq = self.last_seq.max(event.seq);
        if self.events.len() == self.capacity {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    /// Oldest-first.
    pub fn events(&self) -> impl ExactSizeIterator<Item = &LogEvent> {
        self.events.iter()
    }

    /// How many events are held.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether nothing has been logged yet.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// `[WARN] target: message key=value`, without the time (the view renders that
/// separately in local time).
pub fn describe(event: &LogEvent) -> String {
    let mut line = format!("[{}] {}: {}", event.level, event.target, event.message);
    for (key, value) in &event.fields {
        line.push_str(&format!(" {key}={value}"));
    }
    line
}
