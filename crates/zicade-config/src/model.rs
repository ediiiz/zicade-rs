//! Config schema (spec §5.3). These serde structs *are* the schema; unknown
//! enum variants are rejected by serde at parse time.

use serde::{Deserialize, Serialize};

/// Top-level Zicade configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub listen: ListenConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

/// Loopback listen address for the proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 3129,
        }
    }
}

/// How each request is routed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutingMode {
    /// Straight to the origin; no upstream.
    #[default]
    Direct,
    /// Always through the one configured upstream proxy.
    Upstream,
    /// Resolve per-request via a PAC file.
    Pac,
}

/// Routing configuration: mode plus the sections each mode needs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RoutingConfig {
    pub mode: RoutingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<UpstreamConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pac: Option<PacConfig>,
}

impl RoutingConfig {
    /// Build the effective upstream for a PAC-selected proxy `host:port`,
    /// inheriting `routing.pac.auth` so the handshake can run (LESSON-6).
    /// Returns `None` when there is no PAC config (e.g. an internal/DIRECT host
    /// carries no upstream).
    pub fn pac_upstream(&self, host: impl Into<String>, port: u16) -> Option<UpstreamConfig> {
        self.pac.as_ref().map(|pac| UpstreamConfig {
            host: host.into(),
            port,
            auth: pac.auth.clone(),
        })
    }
}

/// A single upstream proxy endpoint with its auth policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub auth: AuthConfig,
}

/// Where the PAC script comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PacSource {
    /// WinHTTP auto-detection (WPAD).
    #[default]
    Auto,
    /// A local file path.
    File,
    /// A remote URL.
    Url,
}

/// What to do when PAC resolution fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailPolicy {
    /// Fail the request with an error.
    #[default]
    Error,
    /// Fall back to a DIRECT connection.
    Direct,
}

/// PAC routing configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PacConfig {
    pub source: PacSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, rename = "failPolicy")]
    pub fail_policy: FailPolicy,
    /// Applied to PAC-selected upstreams (LESSON-6).
    #[serde(default)]
    pub auth: AuthConfig,
}

/// Upstream authentication scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// No upstream authentication.
    #[default]
    None,
    /// HTTP Basic (credentials in config).
    Basic,
    /// Windows SSPI Negotiate/NTLM (Windows-only).
    Negotiate,
}

/// The SSPI security package used for a Negotiate-mode upstream handshake.
///
/// Defaults to [`SspiPackage::Ntlm`], preserving the deliberate NTLM-only
/// behavior needed by the corporate gateway (which rejects SPNEGO and has no
/// Kerberos SPN). Environments that DO register a Kerberos SPN can opt into
/// `negotiate` for Kerberos SSO. Ignored for non-`negotiate` auth modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SspiPackage {
    /// The `NTLM` package (three-leg NTLM handshake). Default.
    #[default]
    Ntlm,
    /// The `Negotiate` package (SPNEGO; Kerberos when an SPN is available).
    Negotiate,
}

/// Authentication configuration for an upstream.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(default)]
    pub mode: AuthMode,
    /// SSPI package for `negotiate` mode; defaults to NTLM. Harmless for other
    /// modes.
    #[serde(default)]
    pub package: SspiPackage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// Logging configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
    pub format: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            format: "json".to_owned(),
        }
    }
}
