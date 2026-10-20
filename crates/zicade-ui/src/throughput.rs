//! Cumulative byte counters, sampled about once a second, turned into the
//! rates and history the status view draws.

use std::collections::VecDeque;
use std::time::Duration;

/// Rolling (in, out) bytes-per-second history.
pub struct Throughput {
    capacity: usize,
    previous: Option<(u64, u64, Duration)>,
    history: VecDeque<(f64, f64)>,
}

impl Throughput {
    /// Keep at most `capacity` rate points.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            previous: None,
            history: VecDeque::with_capacity(capacity),
        }
    }

    /// Record the counters as read at `at` (any monotonic clock offset).
    pub fn sample(&mut self, bytes_in: u64, bytes_out: u64, at: Duration) {
        if let Some((prev_in, prev_out, prev_at)) = self.previous {
            let elapsed = at.saturating_sub(prev_at).as_secs_f64();
            if elapsed <= 0.0 {
                return;
            }
            // A restarted proxy resets its counters; show zero, not a negative.
            let rate = |now: u64, before: u64| now.saturating_sub(before) as f64 / elapsed;
            if self.history.len() == self.capacity {
                self.history.pop_front();
            }
            self.history
                .push_back((rate(bytes_in, prev_in), rate(bytes_out, prev_out)));
        }
        self.previous = Some((bytes_in, bytes_out, at));
    }

    /// The most recent (in, out) rate, once two samples exist.
    pub fn latest(&self) -> Option<(f64, f64)> {
        self.history.back().copied()
    }

    /// Oldest-first rate history.
    pub fn history(&self) -> Vec<(f64, f64)> {
        self.history.iter().copied().collect()
    }
}

/// `1536.0` -> `"1.5 KB/s"`: binary units, one decimal below ten.
pub fn format_bytes_per_sec(bps: f64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut value = bps.max(0.0);
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit > 0 && value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// `1234567` -> `"1,234,567"`.
pub fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `1536` -> `"1.5 KB"`: binary units, one decimal below ten.
pub fn format_bytes(bytes: u64) -> String {
    format_bytes_per_sec(bytes as f64)
        .trim_end_matches("/s")
        .to_owned()
}

/// `125s` -> `"2m 05s"`, `3725s` -> `"1h 02m"`.
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    match (secs / 3600, secs / 60 % 60, secs % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s:02}s"),
        (h, m, _) => format!("{h}h {m:02}m"),
    }
}
