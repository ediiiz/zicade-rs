//! The options each drop-down offers, spelled as in the config file, and the
//! mapping between those names and the config enums (via their serde names,
//! so the two can never drift apart).

use serde::Serialize;
use serde::de::DeserializeOwned;

pub const ROUTING_MODES: [&str; 3] = ["direct", "upstream", "pac"];
pub const AUTH_MODES: [&str; 3] = ["none", "basic", "negotiate"];
pub const SSPI_PACKAGES: [&str; 2] = ["ntlm", "negotiate"];
pub const PAC_SOURCES: [&str; 3] = ["auto", "file", "url"];
pub const FAIL_POLICIES: [&str; 2] = ["error", "direct"];
pub const LOG_LEVELS: [&str; 5] = ["trace", "debug", "info", "warn", "error"];
pub const LOG_FORMATS: [&str; 3] = ["json", "text", "pretty"];

/// The enum value a config-file name stands for.
pub fn parse<T: DeserializeOwned>(name: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}

/// The config-file name of an enum value.
pub fn name_of<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => String::new(),
    }
}

/// Where `name` sits in `options`, if it is offered at all.
pub fn index_of(options: &[&str], name: &str) -> Option<usize> {
    options.iter().position(|option| *option == name)
}
