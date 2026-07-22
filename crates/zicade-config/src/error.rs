//! Typed errors for config load/save/validation.

/// Errors produced while parsing, loading, saving, or validating config.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The JSON was malformed or violated the schema (e.g. an unknown enum
    /// variant such as an unrecognized routing mode or fail policy).
    #[error("failed to parse config JSON: {0}")]
    Parse(#[from] serde_json::Error),

    /// An I/O error occurred reading or writing the config file.
    #[error("config I/O error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// A port field was outside the valid range (must be 1-65535).
    #[error("invalid port in {field}: {port} (must be 1-65535)")]
    InvalidPort { field: &'static str, port: u16 },

    /// A routing mode was selected without the section it requires.
    #[error("routing mode '{mode}' requires a '{missing}' section")]
    MissingSection {
        mode: &'static str,
        missing: &'static str,
    },

    /// `auth.mode = negotiate` was set on a platform without SSPI support.
    #[error("auth mode 'negotiate' in {field} is only supported on Windows")]
    NegotiateUnsupported { field: &'static str },

    /// `auth.mode = basic` was set without a (non-empty) username. The password
    /// may be empty for some proxies, but a username is always required.
    #[error("auth mode 'basic' in {field} requires a non-empty username")]
    MissingCredentials { field: &'static str },

    /// `routing.corpNetwork` is enabled but misconfigured (e.g. no DNS suffixes,
    /// or a zero poll interval).
    #[error("routing.corpNetwork is invalid: {reason}")]
    CorpNetwork { reason: &'static str },
}
