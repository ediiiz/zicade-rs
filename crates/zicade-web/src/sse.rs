//! Server-sent events: a `LogEvent` stream for the live-log panel, and a 1 Hz
//! status/metrics stream for the status panel + throughput chart.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, Sse};
use tokio::time::interval;
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};
use tokio_stream::{Stream, StreamExt};

use crate::state::AppState;

/// `GET /events/logs` — stream each broadcast [`zicade_observe::LogEvent`] as a
/// JSON `data:` SSE event. Lagged messages (a slow client) are skipped rather
/// than surfaced as errors, keeping the stream alive.
pub(crate) async fn sse_logs(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.logs().subscribe();
    let events = BroadcastStream::new(rx).filter_map(|item| match item {
        Ok(event) => serde_json::to_string(&event)
            .ok()
            .map(|json| Ok(Event::default().data(json))),
        Err(_lagged) => None,
    });
    // `take_until` is a `futures_util` combinator (not in tokio_stream's
    // `StreamExt`), so call it via the trait path to avoid clashing with the
    // tokio_stream `filter_map`/`map` used above. It ends the stream when the
    // app shuts down, so the SSE response closes and graceful shutdown finishes.
    let stream = futures_util::StreamExt::take_until(events, state.shutdown_signal());
    Sse::new(stream)
}

/// `GET /events/metrics` — push a full [`StatusSnapshot`] as a JSON `data:`
/// event once per second. The client derives throughput (bytes/sec) from the
/// deltas between consecutive `bytes_in`/`bytes_out` samples. The first tick
/// fires immediately, so the panel paints without waiting a second.
pub(crate) async fn sse_metrics(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let shutdown = state.shutdown_signal();
    let ticks = IntervalStream::new(interval(Duration::from_secs(1))).map(move |_| {
        let json = serde_json::to_string(&state.status_snapshot()).unwrap_or_default();
        Ok(Event::default().data(json))
    });
    // See `sse_logs`: `take_until` comes from `futures_util`, called via the
    // trait path to coexist with tokio_stream's `map` above.
    let stream = futures_util::StreamExt::take_until(ticks, shutdown);
    Sse::new(stream)
}
