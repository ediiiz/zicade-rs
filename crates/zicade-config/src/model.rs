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

impl Config {
    /// A copy safe to show in the UI: stored passwords are removed.
    #[must_use]
    pub fn without_secrets(&self) -> Config {
        let mut cfg = self.clone();
        for auth in cfg.auth_slots_mut() {
            auth.password = None;
        }
        cfg
    }

    /// Fill in passwords an edit left out because the UI never saw them: a
    /// slot that is still Basic for the same username keeps `saved`'s password.
    /// A new password, a different username, or another auth mode wins.
    pub fn keep_secrets_from(&mut self, saved: &Config) {
        let saved_upstream = saved.routing.upstream.as_ref().map(|u| &u.auth);
        let saved_pac = saved.routing.pac.as_ref().map(|p| &p.auth);
        if let (Some(upstream), Some(old)) = (self.routing.upstream.as_mut(), saved_upstream) {
            upstream.auth.keep_password_from(old);
        }
        if let (Some(pac), Some(old)) = (self.routing.pac.as_mut(), saved_pac) {
            pac.auth.keep_password_from(old);
        }
    }

    fn auth_slots_mut(&mut self) -> impl Iterator<Item = &mut AuthConfig> {
        let upstream = self.routing.upstream.as_mut().map(|u| &mut u.auth);
        let pac = self.routing.pac.as_mut().map(|p| &mut p.auth);
        upstream.into_iter().chain(pac)
    }
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
    /// Idle socket pool for upstream dials. On by default: it removes one RTT
    /// (the TCP handshake to the gateway) from every request that follows a
    /// recent one. Set `"enabled": false` to restore the per-request dial.
    #[serde(default)]
    pub pool: PoolConfig,
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

/// Idle-socket pool for upstream connections.
///
/// The pool only ever holds sockets that are *not* tunnelled: a `CONNECT`ed
/// socket belongs to one destination for its whole life. What is reused is the
/// TCP handshake to the gateway — worth one RTT (49–78 ms on the access points
/// we measured) on every request that follows a recent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PoolConfig {
    /// Whether to prewarm idle upstream sockets at all.
    ///
    /// This only saves the TCP handshake to the upstream they are dialed to;
    /// it can never reuse an established `CONNECT` tunnel, so treat it as a
    /// latency trim, not a throughput multiplier.
    pub enabled: bool,
    /// Maximum idle sockets kept per upstream address.
    pub max_idle: usize,
    /// How long an idle socket may sit unused before it is dropped, in seconds.
    /// Keep this below the gateway's own idle reaping.
    pub idle_timeout_secs: u64,
    /// Watermark the prewarm task keeps the pool at. Kept below `max_idle` so
    /// an idle machine never holds the gateway's whole connection table open.
    pub min_idle: usize,
    /// Minimum spacing between prewarm dials, in milliseconds, so a dead
    /// gateway costs one dial per interval instead of a tight retry loop.
    pub prewarm_interval_ms: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        // 32 covers a browser's parallelism (6–8 per origin, a handful of
        // origins) with headroom while staying small. 20 s is below both the
        // gateway's idle reaping and zicade's default `socktimeout` of 20 s.
        // The watermark is 8: enough for a page opening several requests at
        // once, small enough to stay near-free when the machine is idle.
        Self {
            enabled: true,
            max_idle: 32,
            idle_timeout_secs: 20,
            min_idle: 8,
            prewarm_interval_ms: 50,
        }
    }
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

impl AuthConfig {
    fn keep_password_from(&mut self, saved: &AuthConfig) {
        if self.password.is_none()
            && self.mode == AuthMode::Basic
            && saved.mode == AuthMode::Basic
            && self.username == saved.username
        {
            self.password.clone_from(&saved.password);
        }
    }
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
