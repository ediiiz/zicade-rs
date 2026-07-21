#![forbid(unsafe_code)]

//! Observability: a `tracing` layer that fans structured events into a bounded
//! in-memory ring buffer and a broadcast channel for the SSE log stream.
//!
//! Install [`channel_layer`]'s [`ChannelLayer`] on a `tracing_subscriber`
//! registry and keep the returned [`LogStore`] to read recent events
//! ([`LogStore::recent`]) or stream live ones ([`LogStore::subscribe`]).

mod event;
mod layer;
mod store;

pub use event::LogEvent;
pub use layer::{ChannelLayer, channel_layer};
pub use store::LogStore;
