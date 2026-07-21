//! A `tracing_subscriber::Layer` that captures events into a [`LogStore`].

use std::collections::BTreeMap;
use std::fmt::Debug;

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::event::LogEvent;
use crate::store::LogStore;

/// A tracing layer that turns each event into a [`LogEvent`] and pushes it to a
/// shared [`LogStore`] (ring buffer + broadcast).
pub struct ChannelLayer {
    store: LogStore,
}

/// Build a [`ChannelLayer`] and the [`LogStore`] it feeds. Install the layer on
/// a subscriber and keep the store to read `recent()` / `subscribe()`.
pub fn channel_layer(cap: usize) -> (ChannelLayer, LogStore) {
    let store = LogStore::new(cap);
    (
        ChannelLayer {
            store: store.clone(),
        },
        store,
    )
}

impl<S: Subscriber> Layer<S> for ChannelLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let log = LogEvent::now(
            meta.level().to_string(),
            meta.target().to_owned(),
            visitor.message,
            visitor.fields,
        );
        self.store.push(log);
    }
}

/// Collects the `message` field and any other structured fields into strings.
#[derive(Default)]
struct FieldVisitor {
    message: String,
    fields: BTreeMap<String, String>,
}

impl FieldVisitor {
    fn record(&mut self, field: &Field, value: String) {
        if field.name() == "message" {
            self.message = value;
        } else {
            self.fields.insert(field.name().to_owned(), value);
        }
    }
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record(field, value.to_owned());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record(field, value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record(field, value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.record(field, format!("{value:?}"));
    }
}
