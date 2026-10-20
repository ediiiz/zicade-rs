//! Live per-connection rates, from the change in each connection's byte
//! counts between successive snapshots.

use std::collections::HashMap;
use std::time::Duration;

use zicade_control::ConnectionSnapshot;

/// The last-seen counters and current (up, down) rate per connection id.
#[derive(Default)]
pub struct ConnectionRates {
    last: HashMap<u64, (u64, u64, Duration)>,
    rates: HashMap<u64, (f64, f64)>,
}

impl ConnectionRates {
    /// Take a new snapshot, read at `at` (any monotonic clock offset).
    /// Connections missing from it are forgotten.
    pub fn update(&mut self, connections: &[ConnectionSnapshot], at: Duration) {
        let mut last = HashMap::with_capacity(connections.len());
        let mut rates = HashMap::with_capacity(connections.len());
        for c in connections {
            if let Some(&(up, down, then)) = self.last.get(&c.id) {
                let elapsed = at.saturating_sub(then).as_secs_f64();
                if elapsed > 0.0 {
                    let rate = |now: u64, before: u64| now.saturating_sub(before) as f64 / elapsed;
                    rates.insert(c.id, (rate(c.bytes_up, up), rate(c.bytes_down, down)));
                } else if let Some(&kept) = self.rates.get(&c.id) {
                    rates.insert(c.id, kept);
                }
            }
            last.insert(c.id, (c.bytes_up, c.bytes_down, at));
        }
        self.last = last;
        self.rates = rates;
    }

    /// The (up, down) bytes-per-second of connection `id`, once known.
    pub fn rate(&self, id: u64) -> Option<(f64, f64)> {
        self.rates.get(&id).copied()
    }

    /// How many connections are being followed.
    pub fn tracked(&self) -> usize {
        self.last.len()
    }
}
