//! Config validation. Platform-dependent rules (negotiate-is-Windows-only) take
//! an injected [`ValidationCtx`] so they are testable on any OS.

use crate::error::ConfigError;
use crate::model::{AuthConfig, AuthMode, Config, RoutingMode};

/// Platform capabilities the validator needs. Injected so tests can exercise
/// both the Windows and non-Windows branches deterministically.
#[derive(Debug, Clone, Copy)]
pub struct ValidationCtx {
    /// Whether SSPI Negotiate is available on this platform.
    pub negotiate_supported: bool,
}

impl ValidationCtx {
    /// Context for the current host (`negotiate_supported` = `cfg!(windows)`).
    pub fn host() -> Self {
        Self {
            negotiate_supported: cfg!(windows),
        }
    }

    /// A context that supports Negotiate (as a real Windows box would).
    pub fn windows() -> Self {
        Self {
            negotiate_supported: true,
        }
    }
}

impl Config {
    /// Validate the config against the given platform context. Returns the
    /// first violation found.
    pub fn validate(&self, ctx: &ValidationCtx) -> Result<(), ConfigError> {
        check_port("listen.port", self.listen.port)?;

        match self.routing.mode {
            RoutingMode::Upstream if self.routing.upstream.is_none() => {
                return Err(ConfigError::MissingSection {
                    mode: "upstream",
                    missing: "upstream",
                });
            }
            RoutingMode::Pac if self.routing.pac.is_none() => {
                return Err(ConfigError::MissingSection {
                    mode: "pac",
                    missing: "pac",
                });
            }
            _ => {}
        }

        if let Some(up) = &self.routing.upstream {
            check_port("routing.upstream.port", up.port)?;
            check_auth(&up.auth, "routing.upstream.auth", ctx)?;
        }
        if let Some(pac) = &self.routing.pac {
            check_auth(&pac.auth, "routing.pac.auth", ctx)?;
        }
        Ok(())
    }
}

fn check_port(field: &'static str, port: u16) -> Result<(), ConfigError> {
    if port == 0 {
        Err(ConfigError::InvalidPort { field, port })
    } else {
        Ok(())
    }
}

fn check_auth(
    auth: &AuthConfig,
    field: &'static str,
    ctx: &ValidationCtx,
) -> Result<(), ConfigError> {
    if auth.mode == AuthMode::Negotiate && !ctx.negotiate_supported {
        return Err(ConfigError::NegotiateUnsupported { field });
    }
    Ok(())
}
