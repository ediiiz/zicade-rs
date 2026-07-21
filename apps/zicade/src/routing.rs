//! Map a validated [`Config`] onto the proxy's [`Routing`] enum.
//!
//! Direct is a straight pass-through. Upstream builds an [`UpstreamTarget`],
//! mapping the auth mode onto the proxy's [`UpstreamAuth`] (Negotiate wires a
//! per-connection SSPI authenticator factory). PAC is a documented known
//! limitation for M6: the [`RouteResolver`] is constructed and validated, but
//! per-request PAC routing is not yet wired into the data path, so the proxy
//! runs Direct.

use std::sync::Arc;

use anyhow::Context as _;
use zicade_auth::{AuthError, UpstreamAuthenticator};
use zicade_config::{AuthConfig, AuthMode, Config, RoutingMode};
use zicade_proxy::{AuthFactory, Routing, UpstreamAuth, UpstreamTarget};
use zicade_routing::RouteResolver;
use zicade_win::{SspiNegotiate, WinHttpPacBackend};

/// Build the proxy [`Routing`] for the given config.
pub fn build_routing(config: &Config) -> anyhow::Result<Routing> {
    match config.routing.mode {
        RoutingMode::Direct => Ok(Routing::Direct),
        RoutingMode::Upstream => build_upstream(config),
        RoutingMode::Pac => build_pac(config),
    }
}

fn build_upstream(config: &Config) -> anyhow::Result<Routing> {
    let up = config
        .routing
        .upstream
        .as_ref()
        .context("routing.mode = upstream requires a [routing.upstream] section")?;
    let addr = format!("{}:{}", up.host, up.port);
    let auth = build_auth(&up.auth, &up.host);
    tracing::info!(upstream = %addr, auth = ?up.auth.mode, "routing via upstream proxy");
    Ok(Routing::Upstream(UpstreamTarget { addr, auth }))
}

/// PAC mode: construct + validate the resolver, then fall back to Direct in the
/// data path (documented M6 limitation).
fn build_pac(config: &Config) -> anyhow::Result<Routing> {
    let pac = config
        .routing
        .pac
        .as_ref()
        .context("routing.mode = pac requires a [routing.pac] section")?;
    match WinHttpPacBackend::new() {
        Ok(backend) => {
            let _resolver = RouteResolver::new(pac.clone(), backend);
            tracing::warn!(
                "PAC routing is configured and the WinHTTP resolver was built, but \
                 per-request PAC routing is not yet wired into the proxy data path; \
                 running Direct (known limitation)"
            );
        }
        Err(err) => {
            tracing::warn!(%err, "could not open the WinHTTP PAC backend; running Direct");
        }
    }
    Ok(Routing::Direct)
}

/// Map an [`AuthConfig`] onto the proxy's [`UpstreamAuth`].
fn build_auth(auth: &AuthConfig, host: &str) -> UpstreamAuth {
    match auth.mode {
        AuthMode::None => UpstreamAuth::None,
        AuthMode::Basic => {
            tracing::warn!(
                "Basic upstream auth is not yet wired; connecting without proxy auth \
                 (known limitation)"
            );
            UpstreamAuth::None
        }
        AuthMode::Negotiate => UpstreamAuth::Negotiate(sspi_factory(host)),
    }
}

/// A per-connection factory building a fresh SSPI Negotiate authenticator.
///
/// Construction failures (e.g. off-Windows) are deferred to `step` time via
/// [`DeferredSspi`], matching the proxy's `AuthFactory` contract (the closure is
/// infallible and returns a boxed authenticator).
fn sspi_factory(host: &str) -> AuthFactory {
    let spn = format!("HTTP/{host}");
    Arc::new(move || {
        Box::new(DeferredSspi(SspiNegotiate::new(Some(&spn))))
            as Box<dyn UpstreamAuthenticator + Send>
    })
}

/// Wraps the fallible SSPI construction so any error surfaces on the first
/// handshake `step` rather than at factory-call time.
struct DeferredSspi(Result<SspiNegotiate, zicade_win::WinError>);

impl UpstreamAuthenticator for DeferredSspi {
    fn step(&mut self, challenge: Option<&[u8]>) -> Result<Vec<u8>, AuthError> {
        match &mut self.0 {
            Ok(inner) => inner.step(challenge),
            Err(err) => Err(AuthError::Authenticator(format!(
                "SSPI Negotiate initialization failed: {err}"
            ))),
        }
    }
}
