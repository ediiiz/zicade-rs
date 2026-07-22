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
    #[serde(default)]
    pub web: WebConfig,
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
    /// Optional corporate-network gate. When present and enabled (for `pac`/
    /// `upstream` modes) the effective routing follows the configured mode only
    /// while an active adapter's DNS suffix matches [`CorpNetworkConfig`]; off
    /// the corporate network it falls back to `Direct`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "corpNetwork"
    )]
    pub corp_network: Option<CorpNetworkConfig>,
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

/// Corporate-network detection gate.
///
/// When `enabled`, a background monitor inspects the active network adapters'
/// DNS suffixes; the configured routing (`pac`/`upstream`) applies only while at
/// least one active suffix matches one of `dns_suffixes` (case-insensitive,
/// dot-boundary — e.g. `dy.droot.org` matches `droot.org`). Off the corporate
/// network the proxy routes `Direct`. Defaults to **disabled** so older configs
/// are unaffected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpNetworkConfig {
    /// Whether the gate is active. Default `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Corporate DNS suffixes that identify the corporate network, e.g.
    /// `["droot.org"]`. Required (non-empty) when `enabled`.
    #[serde(default, rename = "dnsSuffixes")]
    pub dns_suffixes: Vec<String>,
    /// How often (seconds) to re-check the network as a fallback to change
    /// events. Default 30.
    #[serde(default = "default_poll_seconds", rename = "pollSeconds")]
    pub poll_seconds: u64,
}

impl Default for CorpNetworkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            dns_suffixes: Vec::new(),
            poll_seconds: default_poll_seconds(),
        }
    }
}

/// Default re-check interval for [`CorpNetworkConfig::poll_seconds`].
fn default_poll_seconds() -> u64 {
    30
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

/// Web UI / local API configuration.
///
/// `auth_required` controls whether mutating endpoints (`PUT /api/config`)
/// require the local `X-Zicade-Token` header. It defaults to **false**: the web
/// server is loopback-only, so auth is normally unneeded. Set it to `true` to
/// require the token (which also serves as CSRF protection). Read-only endpoints
/// stay open on loopback regardless.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WebConfig {
    /// Require the `X-Zicade-Token` header on mutations. Default `false`.
    #[serde(default, rename = "authRequired")]
    pub auth_required: bool,
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
