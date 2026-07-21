//! Server-sent events stream of `LogEvent`s for the live-log panel.

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event, Sse};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use crate::state::AppState;

/// `GET /events/logs` — stream each broadcast [`zicade_observe::LogEvent`] as a
/// JSON `data:` SSE event. Lagged messages (a slow client) are skipped rather
/// than surfaced as errors, keeping the stream alive.
pub(crate) async fn sse_logs(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.logs().subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|item| match item {
        Ok(event) => serde_json::to_string(&event)
            .ok()
            .map(|json| Ok(Event::default().data(json))),
        Err(_lagged) => None,
    });
    Sse::new(stream)
}
