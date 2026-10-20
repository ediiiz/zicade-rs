//! Plain-text form fields <-> [`Config`].
//!
//! The form holds exactly what the user typed; [`ConfigForm::to_config`] is the
//! one place that decides how text becomes config. Only the section for the
//! selected routing mode is emitted, so a half-filled section the user can no
//! longer see never blocks a save.

use zicade_config::{
    AuthConfig, AuthMode, Config, CorpNetworkConfig, FailPolicy, ListenConfig, LoggingConfig,
    PacConfig, PacSource, RoutingConfig, RoutingMode, SspiPackage, UpstreamConfig,
};

/// Re-check interval used when the field is blank or zero.
const DEFAULT_POLL_SECONDS: u64 = 30;

/// One auth slot as the form edits it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthForm {
    pub mode: AuthMode,
    pub package: SspiPackage,
    pub username: String,
    /// Always blank when loaded: the UI only sees redacted configs.
    pub password: String,
}

impl AuthForm {
    fn from_auth(auth: &AuthConfig) -> Self {
        Self {
            mode: auth.mode,
            package: auth.package,
            username: auth.username.clone().unwrap_or_default(),
            password: auth.password.clone().unwrap_or_default(),
        }
    }

    /// A blank password becomes `None`, which tells `Control::apply` to keep
    /// the saved one.
    fn to_auth(&self) -> AuthConfig {
        AuthConfig {
            mode: self.mode,
            package: self.package,
            username: non_blank(&self.username),
            password: non_blank(&self.password),
        }
    }
}

/// Every editable field, as text where the user types it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigForm {
    pub listen_host: String,
    pub listen_port: String,
    pub mode: RoutingMode,
    pub upstream_host: String,
    pub upstream_port: String,
    pub upstream_auth: AuthForm,
    pub pac_source: PacSource,
    pub pac_path: String,
    pub pac_url: String,
    pub pac_fail_policy: FailPolicy,
    pub pac_auth: AuthForm,
    pub corp_enabled: bool,
    /// Comma- or newline-separated.
    pub corp_suffixes: String,
    pub corp_poll_seconds: String,
    pub log_level: String,
    pub log_format: String,
}

/// A field the user must fix before the config can be applied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {reason}")]
pub struct FormError {
    pub field: &'static str,
    pub reason: &'static str,
}

impl ConfigForm {
    /// Fill the form from a (redacted) config.
    pub fn from_config(config: &Config) -> Self {
        let routing = &config.routing;
        let upstream = routing.upstream.clone().unwrap_or_else(blank_upstream);
        let pac = routing.pac.clone().unwrap_or_default();
        let corp = routing.corp_network.clone().unwrap_or_default();
        Self {
            listen_host: config.listen.host.clone(),
            listen_port: config.listen.port.to_string(),
            mode: routing.mode,
            upstream_host: upstream.host,
            upstream_port: port_text(upstream.port),
            upstream_auth: AuthForm::from_auth(&upstream.auth),
            pac_source: pac.source,
            pac_path: pac.path.unwrap_or_default(),
            pac_url: pac.url.unwrap_or_default(),
            pac_fail_policy: pac.fail_policy,
            pac_auth: AuthForm::from_auth(&pac.auth),
            corp_enabled: corp.enabled,
            corp_suffixes: corp.dns_suffixes.join(", "),
            corp_poll_seconds: corp.poll_seconds.to_string(),
            log_level: config.logging.level.clone(),
            log_format: config.logging.format.clone(),
        }
    }

    /// Build the config to apply, or name the first field that cannot parse.
    /// Semantic checks (missing host, unsupported auth) are left to
    /// `Config::validate`, which `Control::apply` runs.
    pub fn to_config(&self) -> Result<Config, FormError> {
        let gated = matches!(self.mode, RoutingMode::Upstream | RoutingMode::Pac);
        Ok(Config {
            listen: ListenConfig {
                host: self.listen_host.trim().to_owned(),
                port: parse_port("Listen port", &self.listen_port)?,
            },
            routing: RoutingConfig {
                mode: self.mode,
                upstream: match self.mode {
                    RoutingMode::Upstream => Some(self.upstream()?),
                    _ => None,
                },
                pac: (self.mode == RoutingMode::Pac).then(|| self.pac()),
                corp_network: if gated { self.corp()? } else { None },
            },
            logging: LoggingConfig {
                level: self.log_level.trim().to_owned(),
                format: self.log_format.trim().to_owned(),
            },
        })
    }

    fn upstream(&self) -> Result<UpstreamConfig, FormError> {
        Ok(UpstreamConfig {
            host: self.upstream_host.trim().to_owned(),
            port: parse_port("Upstream port", &self.upstream_port)?,
            auth: self.upstream_auth.to_auth(),
        })
    }

    fn pac(&self) -> PacConfig {
        PacConfig {
            source: self.pac_source,
            path: non_blank(&self.pac_path),
            url: non_blank(&self.pac_url),
            fail_policy: self.pac_fail_policy,
            auth: self.pac_auth.to_auth(),
        }
    }

    /// `None` for an untouched gate (off, no suffixes), so saving a config that
    /// never had a corp-network section does not add one.
    fn corp(&self) -> Result<Option<CorpNetworkConfig>, FormError> {
        let dns_suffixes: Vec<String> = self
            .corp_suffixes
            .split([',', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if !self.corp_enabled && dns_suffixes.is_empty() {
            return Ok(None);
        }
        let poll = self.corp_poll_seconds.trim();
        let poll_seconds = if poll.is_empty() {
            DEFAULT_POLL_SECONDS
        } else {
            poll.parse().map_err(|_| FormError {
                field: "Re-check interval",
                reason: "must be a whole number of seconds",
            })?
        };
        Ok(Some(CorpNetworkConfig {
            enabled: self.corp_enabled,
            dns_suffixes,
            poll_seconds,
        }))
    }
}

/// The upstream fields shown before one has ever been configured.
fn blank_upstream() -> UpstreamConfig {
    UpstreamConfig {
        host: String::new(),
        port: 0,
        auth: AuthConfig::default(),
    }
}

/// Port 0 means "not set yet", which reads better as an empty field.
fn port_text(port: u16) -> String {
    if port == 0 {
        String::new()
    } else {
        port.to_string()
    }
}

fn parse_port(field: &'static str, text: &str) -> Result<u16, FormError> {
    text.trim().parse().map_err(|_| FormError {
        field,
        reason: "must be a number from 1 to 65535",
    })
}

fn non_blank(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
