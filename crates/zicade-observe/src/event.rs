//! The serializable log record fanned out to the ring buffer and SSE stream.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A single structured log event captured from the `tracing` pipeline.
///
/// `timestamp` is Unix time in milliseconds; `level` is the uppercase tracing
/// level name (`"INFO"`, `"WARN"`, ...). Extra key/value pairs recorded on the
/// event land in `fields` (stringified), while the primary `message` field is
/// lifted out for convenience.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEvent {
    /// Monotonic per-store sequence number, assigned by [`crate::LogStore`] on
    /// push (1-based; 0 = not yet stored). Lets the SSE endpoint replay the
    /// ring buffer and then dedup it against the live broadcast stream.
    #[serde(default)]
    pub seq: u64,
    /// Unix timestamp in milliseconds.
    pub timestamp: u64,
    /// Uppercase level name, e.g. `"INFO"`.
    pub level: String,
    /// The event's `target` (module path by default).
    pub target: String,
    /// The formatted human-readable message.
    pub message: String,
    /// Any additional structured fields, stringified.
    pub fields: BTreeMap<String, String>,
}

impl LogEvent {
    /// Build an event stamped with the current wall-clock time.
    pub(crate) fn now(
        level: String,
        target: String,
        message: String,
        fields: BTreeMap<String, String>,
    ) -> Self {
        Self {
            seq: 0, // assigned by the store on push
            timestamp: unix_millis(),
            level,
            target,
            message,
            fields,
        }
    }
}

/// Current Unix time in milliseconds, saturating to 0 before the epoch. Kept
/// infallible so the logging path never panics (no `unwrap`/`expect`).
fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
