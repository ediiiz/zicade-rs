//! The options each drop-down offers: the name the config file uses, and the
//! label people read. Names map to the config enums through their serde names,
//! so the two can never drift apart.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// One drop-down option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    /// As written in the config file.
    pub name: &'static str,
    /// As shown in the window.
    pub label: &'static str,
}

const fn item(name: &'static str, label: &'static str) -> Item {
    Item { name, label }
}

pub const ROUTING_MODES: [Item; 3] = [
    item("direct", "Direct"),
    item("upstream", "Upstream proxy"),
    item("pac", "PAC script"),
];
pub const AUTH_MODES: [Item; 3] = [
    item("none", "None"),
    item("basic", "Username and password"),
    item("negotiate", "Windows sign-in"),
];
pub const SSPI_PACKAGES: [Item; 2] = [
    item("ntlm", "NTLM"),
    item("negotiate", "Negotiate (Kerberos)"),
];
pub const PAC_SOURCES: [Item; 3] = [
    item("auto", "Automatic"),
    item("file", "File"),
    item("url", "Web address"),
];
pub const FAIL_POLICIES: [Item; 2] = [
    item("error", "Fail the request"),
    item("direct", "Connect directly"),
];
pub const LOG_LEVELS: [Item; 5] = [
    item("trace", "Trace"),
    item("debug", "Debug"),
    item("info", "Info"),
    item("warn", "Warnings"),
    item("error", "Errors"),
];
pub const LOG_FORMATS: [Item; 3] = [
    item("json", "JSON"),
    item("text", "Text"),
    item("pretty", "Pretty"),
];

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

/// Where the option named `name` sits in `options`, if it is offered at all.
pub fn index_of(options: &[Item], name: &str) -> Option<usize> {
    options.iter().position(|option| option.name == name)
}

/// The labels to show, in order.
pub fn labels(options: &[Item]) -> Vec<&'static str> {
    options.iter().map(|option| option.label).collect()
}
