#![forbid(unsafe_code)]

//! The UI's handle on the running app: read the config (with passwords
//! redacted) and live status, follow the log stream, and apply an edited
//! config — validated, persisted, and swapped into live routing in one step.
//!
//! It knows nothing about any particular UI toolkit, so the rules about what
//! the UI may see and what an apply must do are tested here, headless.

mod control;
mod error;

pub use control::{ConfigApplyHook, Control, MetricsSource, OnCorpCell, StatusSnapshot};
pub use error::ControlError;
pub use zicade_proxy::{ConnectionKind, ConnectionSnapshot, ConnectionState};
