//! M5 acceptance tests for `zicade-observe`.
//!
//! We avoid touching the global tracing subscriber (that makes tests order-
//! dependent and flaky); every test wraps its emission in
//! `tracing::subscriber::with_default(..)` so the layer is scoped to the test.

use tracing_subscriber::prelude::*;
use zicade_observe::{LogStore, channel_layer};

/// Build a scoped subscriber from a `ChannelLayer` and run `body` under it.
fn with_layer<F: FnOnce()>(cap: usize, body: F) -> LogStore {
    let (layer, store) = channel_layer(cap);
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, body);
    store
}

#[test]
fn recent_captures_events_in_order_with_levels() {
    let store = with_layer(64, || {
        tracing::info!("first message");
        tracing::warn!("second message");
    });

    let events = store.recent();
    assert_eq!(events.len(), 2, "both events should be buffered");
    assert_eq!(events[0].level, "INFO");
    assert!(events[0].message.contains("first message"));
    assert_eq!(events[1].level, "WARN");
    assert!(events[1].message.contains("second message"));
    assert!(
        events[0].target.starts_with("observe"),
        "target was {}",
        events[0].target
    );
}

#[test]
fn subscriber_receives_broadcast_events() {
    let (layer, store) = channel_layer(64);
    let mut rx = store.subscribe();
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("broadcast me");
    });

    let ev = rx.try_recv().expect("subscriber should receive the event");
    assert_eq!(ev.level, "INFO");
    assert!(ev.message.contains("broadcast me"));
}

#[test]
fn ring_is_bounded_and_keeps_newest() {
    let cap = 8usize;
    let store = with_layer(cap, || {
        for i in 0..(cap as u32 * 3) {
            tracing::info!(index = i, "evt");
        }
    });

    let events = store.recent();
    assert_eq!(events.len(), cap, "ring must cap at {cap}");
    // Oldest retained event is index (total - cap); newest is total-1.
    let total = cap as u32 * 3;
    let first_kept = total - cap as u32;
    assert!(
        events[0].fields.get("index") == Some(&first_kept.to_string()),
        "oldest retained should be index {first_kept}, fields={:?}",
        events[0].fields
    );
    assert!(
        events[cap - 1].fields.get("index") == Some(&(total - 1).to_string()),
        "newest retained should be index {}, fields={:?}",
        total - 1,
        events[cap - 1].fields
    );
}

#[test]
fn push_assigns_monotonic_seq() {
    let store = with_layer(8, || {
        tracing::info!("first");
        tracing::info!("second");
    });
    let events = store.recent();
    assert_eq!(events[0].seq, 1, "seq is 1-based");
    assert_eq!(events[1].seq, 2, "seq increments per push");
}

#[test]
fn ring_and_broadcast_carry_the_same_seq() {
    let (layer, store) = channel_layer(8);
    let mut rx = store.subscribe();
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("stamped");
    });

    let live = rx.try_recv().expect("broadcast event");
    let buffered = &store.recent()[0];
    assert_eq!(
        live.seq, buffered.seq,
        "the SSE dedup relies on ring and broadcast sharing one identity"
    );
    assert!(live.seq > 0, "stored events must carry an assigned seq");
}

#[test]
fn log_event_serializes_to_json() {
    let store = with_layer(4, || {
        tracing::info!(key = "value", "hello");
    });
    let ev = &store.recent()[0];
    let json = serde_json::to_string(ev).expect("LogEvent serializes");
    assert!(json.contains("\"level\""));
    assert!(json.contains("\"message\""));
    assert!(json.contains("hello"));
}
