#![forbid(unsafe_code)]

//! Local web API + htmx-free UI (axum): config CRUD, status, and SSE log
//! streaming, with a local-token gate on mutations.
//!
//! Build the app with [`router`] around an [`AppState`]. The server is intended
//! to be bound to loopback (`127.0.0.1`) ONLY — never a routable interface —
//! since the token gate assumes only local processes can reach it. The
//! `X-Zicade-Token` header required on mutating routes doubles as CSRF
//! protection (a cross-site request cannot set a custom header).

mod assets;
mod error;
mod handlers;
mod router;
mod sse;
mod state;
mod token;

pub use router::{TOKEN_HEADER, router};
pub use state::{AppState, StatusSnapshot};
pub use token::load_or_create_token;
