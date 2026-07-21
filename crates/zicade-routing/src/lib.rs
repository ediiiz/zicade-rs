#![forbid(unsafe_code)]

//! Routing: the [`PacBackend`] seam, the [`RouteResolver`] that applies PAC
//! policy, [`RouteDecision`], and PAC-result parsing ([`PacResult`]).
//!
//! Traits live here so tests inject fakes ([`FakeBackend`]) and the real
//! WinHTTP backend (in `zicade-win`) is one impl among several. The parser and
//! resolver are pure and OS-independent; only the WinHTTP backend touches FFI.

mod error;
mod fake;
mod pac;
mod resolver;

pub use error::RoutingError;
pub use fake::FakeBackend;
pub use pac::PacResult;
pub use resolver::{PacBackend, RouteDecision, RouteResolver};
