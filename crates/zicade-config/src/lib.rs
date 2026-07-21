#![forbid(unsafe_code)]

//! Configuration schema, load/save, and validation for Zicade (spec §5.3).
//!
//! The serde structs in [`model`] are the schema. [`Config::validate`] enforces
//! the cross-field and platform rules (negotiate-is-Windows-only, required
//! sections per routing mode, PAC-auth inheritance for selected upstreams).

mod error;
mod io;
mod model;
mod validate;

pub use error::ConfigError;
pub use io::{from_json_str, load_file, save_file, to_json_string};
pub use model::{
    AuthConfig, AuthMode, Config, FailPolicy, ListenConfig, LoggingConfig, PacConfig, PacSource,
    RoutingConfig, RoutingMode, SspiPackage, UpstreamConfig,
};
pub use validate::ValidationCtx;
