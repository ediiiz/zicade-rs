//! The `PacBackend` seam and the `RouteResolver` that turns a raw PAC lookup
//! into a routing decision, applying the `failPolicy` and LESSON-6 auth
//! inheritance.

use zicade_config::{FailPolicy, PacConfig, UpstreamConfig};

use crate::error::RoutingError;
use crate::pac::PacResult;

/// The raw PAC lookup seam. The real implementation is WinHTTP (in
/// `zicade-win`); tests inject a fake.
pub trait PacBackend {
    /// Resolve the proxy decision for `url`.
    fn resolve(&self, url: &str) -> Result<PacResult, RoutingError>;
}

/// The routing decision for a single request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecision {
    /// Connect straight to the origin.
    Direct,
    /// Route through the given upstream (carrying its inherited auth).
    Upstream(UpstreamConfig),
}

/// Resolves a request URL to a [`RouteDecision`] using a [`PacBackend`],
/// applying `routing.pac` policy (auth inheritance + `failPolicy`).
pub struct RouteResolver<B: PacBackend> {
    pac: PacConfig,
    backend: B,
}

impl<B: PacBackend> RouteResolver<B> {
    /// Build a resolver from the PAC config and a backend.
    pub fn new(pac: PacConfig, backend: B) -> Self {
        Self { pac, backend }
    }

    /// Resolve `url` to a routing decision.
    ///
    /// On a `Proxy` result the selected upstream inherits `routing.pac.auth`
    /// (LESSON-6) so a downstream Negotiate handshake can run. On a backend
    /// error the `failPolicy` decides: `Error` propagates, `Direct` falls back.
    pub fn route(&self, url: &str) -> Result<RouteDecision, RoutingError> {
        match self.backend.resolve(url) {
            Ok(PacResult::Direct) => Ok(RouteDecision::Direct),
            Ok(PacResult::Proxy { host, port }) => Ok(RouteDecision::Upstream(UpstreamConfig {
                host,
                port,
                auth: self.pac.auth.clone(),
            })),
            Err(err) => match self.pac.fail_policy {
                FailPolicy::Error => Err(err),
                FailPolicy::Direct => Ok(RouteDecision::Direct),
            },
        }
    }
}
