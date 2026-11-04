//! Map a validated [`Config`] onto the proxy's [`Routing`] enum.
//!
//! Direct is a straight pass-through. Upstream builds an [`UpstreamTarget`],
//! mapping the auth mode onto the proxy's [`UpstreamAuth`] (Negotiate wires a
//! per-connection SSPI authenticator factory). PAC per-request routing lives in
//! [`crate::pac_router`], which injects a `PacRouter` closure backed by WinHTTP.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use zicade_auth::header::build_basic_authorization;
use zicade_auth::{AuthError, UpstreamAuthenticator};
use zicade_config::{AuthConfig, AuthMode, Config, RoutingMode};
use zicade_proxy::{AuthFactory, PoolConfig, Routing, UpstreamAuth, UpstreamPool, UpstreamTarget};
use zicade_win::SspiNegotiate;

/// Build the proxy [`Routing`] for the given config.
pub fn build_routing(config: &Config) -> anyhow::Result<Routing> {
    match config.routing.mode {
        RoutingMode::Direct => Ok(Routing::Direct),
        RoutingMode::Upstream => build_upstream(config),
        RoutingMode::Pac => crate::pac_router::build_pac(config),
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

    // The pool lives for the lifetime of this routing snapshot and is shared by
    // every connection that clones it. Without one, each request pays a fresh
    // TCP dial to the gateway (one RTT) before its CONNECT even starts.
    //
    // A tunnelled socket is spliced away and never returns, so the pool cannot
    // refill itself: it is filled by a background prewarm task instead.
    let mut target = UpstreamTarget::new(addr, auth);
    if config.routing.pool.enabled {
        let pool = Arc::new(UpstreamPool::new(
            target.addr.clone(),
            PoolConfig {
                max_idle: config.routing.pool.max_idle,
                idle_timeout: Duration::from_secs(config.routing.pool.idle_timeout_secs),
                min_idle: config.routing.pool.min_idle,
                prewarm_interval: Duration::from_millis(config.routing.pool.prewarm_interval_ms),
            },
        ));
        tracing::info!(
            max_idle = config.routing.pool.max_idle,
            min_idle = config.routing.pool.min_idle,
            idle_timeout_secs = config.routing.pool.idle_timeout_secs,
            "upstream socket prewarm enabled (saves the TCP dial, not the tunnel)"
        );
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(zicade_proxy::spawn_prewarm(Arc::clone(&pool)));
        } else {
            // Built outside a runtime (unit tests calling this synchronously):
            // skip prewarm, the pool simply stays cold.
            tracing::debug!("no tokio runtime; upstream socket prewarm not started");
        }
        target = target.with_pool(pool);
    } else {
        tracing::info!("upstream socket prewarm disabled");
    }
    Ok(Routing::Upstream(target))
}

/// Map an [`AuthConfig`] onto the proxy's [`UpstreamAuth`]. `host` is the proxy
/// host used to derive the Negotiate SPN (`HTTP/<host>`); for PAC this is the
/// RESOLVED proxy host (LESSON-6).
pub(crate) fn build_auth(auth: &AuthConfig, host: &str) -> UpstreamAuth {
    match auth.mode {
        AuthMode::None => UpstreamAuth::None,
        AuthMode::Basic => {
            // Config validation guarantees a non-empty username for basic mode;
            // password may be empty for some proxies.
            let username = auth.username.as_deref().unwrap_or_default();
            let password = auth.password.as_deref().unwrap_or_default();
            let credentials = build_basic_authorization(username, password);
            tracing::info!("upstream auth: HTTP Basic");
            UpstreamAuth::Basic { credentials }
        }
        AuthMode::Negotiate => {
            let package = map_package(auth.package);
            tracing::info!(?package, "upstream auth: SSPI Negotiate");
            UpstreamAuth::Negotiate(sspi_factory(host, package))
        }
    }
}

/// Map the config-level SSPI package onto the zicade-win runtime package.
fn map_package(package: zicade_config::SspiPackage) -> zicade_win::SspiPackage {
    match package {
        zicade_config::SspiPackage::Ntlm => zicade_win::SspiPackage::Ntlm,
        zicade_config::SspiPackage::Negotiate => zicade_win::SspiPackage::Negotiate,
    }
}

/// A per-connection factory building a fresh SSPI authenticator for `package`.
///
/// Construction failures (e.g. off-Windows) are deferred to `step` time via
/// [`DeferredSspi`], matching the proxy's `AuthFactory` contract (the closure is
/// infallible and returns a boxed authenticator).
fn sspi_factory(host: &str, package: zicade_win::SspiPackage) -> AuthFactory {
    let spn = format!("HTTP/{host}");
    Arc::new(move || {
        Box::new(DeferredSspi(SspiNegotiate::new(Some(&spn), package)))
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

#[cfg(test)]
mod tests {
    use super::{build_auth, map_package};
    use zicade_config::{AuthConfig, AuthMode, SspiPackage as CfgPkg};
    use zicade_win::SspiPackage as WinPkg;

    #[test]
    fn maps_config_package_onto_win_package() {
        assert_eq!(map_package(CfgPkg::Ntlm), WinPkg::Ntlm);
        assert_eq!(map_package(CfgPkg::Negotiate), WinPkg::Negotiate);
    }

    #[test]
    fn basic_mode_builds_basic_upstream_auth() {
        let auth = AuthConfig {
            mode: AuthMode::Basic,
            package: CfgPkg::default(),
            username: Some("svc".to_owned()),
            password: Some("secret".to_owned()),
        };
        // Debug never leaks the credential; it just names the scheme.
        assert_eq!(format!("{:?}", build_auth(&auth, "wp8080")), "Basic");
    }

    #[test]
    fn none_mode_builds_none_upstream_auth() {
        let auth = AuthConfig::default();
        assert_eq!(format!("{:?}", build_auth(&auth, "wp8080")), "None");
    }
}
