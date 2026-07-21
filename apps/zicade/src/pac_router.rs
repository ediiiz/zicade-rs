//! PAC per-request routing for the app layer.
//!
//! `mode = "pac"` injects a [`PacRouter`] closure into the proxy. For each
//! request the proxy hands the closure the target URL; it resolves the proxy
//! via WinHTTP and returns a [`RouteChoice`] (DIRECT, or PROXY carrying
//! `routing.pac.auth` — LESSON-6).
//!
//! The WinHTTP session handle is not `Send`/`Sync`, so the backend is built
//! once on a dedicated thread and never leaves it; resolve requests reach it
//! over a channel. This keeps blocking WinHTTP calls off the async runtime
//! without sharing the raw handle across threads.

use std::io;
use std::sync::Arc;
use std::thread;

use anyhow::Context as _;
use tokio::sync::{mpsc, oneshot};
use zicade_config::{Config, FailPolicy, PacSource, RoutingConfig};
use zicade_proxy::{PacRouter, RouteChoice, Routing, UpstreamTarget};
use zicade_routing::{PacBackend, PacResult, RoutingError};
use zicade_win::WinHttpPacBackend;

use crate::routing::build_auth;

/// A resolve request: the target URL plus a one-shot channel for the result.
type ResolveJob = (String, oneshot::Sender<Result<PacResult, RoutingError>>);
type ResolverTx = mpsc::UnboundedSender<ResolveJob>;

/// Build the PAC [`Routing`]: start the resolver thread and hand the proxy a
/// closure that resolves each request through it.
pub(crate) fn build_pac(config: &Config) -> anyhow::Result<Routing> {
    let pac = config
        .routing
        .pac
        .as_ref()
        .context("routing.mode = pac requires a [routing.pac] section")?;
    tracing::info!(
        source = ?pac.source,
        fail_policy = ?pac.fail_policy,
        auth = ?pac.auth.mode,
        "PAC per-request routing enabled"
    );

    let tx = spawn_resolver(pac.source, pac.url.clone(), pac.path.clone());
    let routing = Arc::new(config.routing.clone());
    let router: PacRouter = Arc::new(move |url: String| {
        let tx = tx.clone();
        let routing = Arc::clone(&routing);
        Box::pin(async move {
            let resolved = resolve_via_thread(&tx, url).await;
            to_route_choice(resolved, &routing)
        })
    });
    Ok(Routing::Pac(router))
}

/// Send one resolve request to the backend thread and await its reply. A dead
/// or dropped channel is reported as a backend error so `failPolicy` decides.
async fn resolve_via_thread(tx: &ResolverTx, url: String) -> Result<PacResult, RoutingError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    if tx.send((url, reply_tx)).is_err() {
        return Err(RoutingError::Backend(
            "PAC resolver thread stopped".to_owned(),
        ));
    }
    reply_rx.await.unwrap_or_else(|_| {
        Err(RoutingError::Backend(
            "PAC resolver dropped the request".to_owned(),
        ))
    })
}

/// Spawn the dedicated thread that owns the WinHTTP backend and serves resolve
/// requests one at a time (the raw session handle stays on this thread).
fn spawn_resolver(source: PacSource, url: Option<String>, path: Option<String>) -> ResolverTx {
    let (tx, mut rx) = mpsc::unbounded_channel::<ResolveJob>();
    thread::Builder::new()
        .name("zicade-pac".to_owned())
        .spawn(move || {
            let backend = build_backend(source, url, path);
            if let Err(msg) = &backend {
                tracing::warn!(error = %msg, "PAC backend unavailable; failPolicy governs each request");
            }
            while let Some((request_url, reply)) = rx.blocking_recv() {
                let result = match &backend {
                    Ok(b) => b.resolve(&request_url),
                    Err(msg) => Err(RoutingError::Backend(msg.clone())),
                };
                let _ = reply.send(result);
            }
        })
        .expect("spawn PAC resolver thread");
    tx
}

/// Build the WinHTTP backend for the configured [`PacSource`]. The error is a
/// `String` (not [`RoutingError`], which is not `Clone`) so it can be replayed
/// on every request the backend cannot serve.
fn build_backend(
    source: PacSource,
    url: Option<String>,
    path: Option<String>,
) -> Result<WinHttpPacBackend, String> {
    let backend = match source {
        PacSource::Auto => WinHttpPacBackend::from_system(),
        PacSource::Url => {
            let url =
                url.ok_or_else(|| "routing.pac.url is required for source = \"url\"".to_owned())?;
            WinHttpPacBackend::with_config_url(&url)
        }
        PacSource::File => {
            let path = path
                .ok_or_else(|| "routing.pac.path is required for source = \"file\"".to_owned())?;
            WinHttpPacBackend::with_config_url(&file_url(&path))
        }
    };
    backend.map_err(|err| err.to_string())
}

/// Turn a local filesystem path into a `file://` URL WinHTTP can fetch as a PAC
/// config URL. Canonicalizes when possible and strips the Windows verbatim
/// prefix (`\\?\`).
fn file_url(path: &str) -> String {
    let p = std::path::Path::new(path);
    let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let slashed = abs.to_string_lossy().replace('\\', "/");
    let trimmed = slashed.strip_prefix("//?/").unwrap_or(&slashed);
    format!("file:///{}", trimmed.trim_start_matches('/'))
}

/// Map a raw PAC lookup onto a [`RouteChoice`], applying LESSON-6 auth
/// inheritance (the selected upstream carries `routing.pac.auth`, with the
/// Negotiate SPN keyed on the RESOLVED proxy host) and `failPolicy` on error.
fn to_route_choice(
    resolved: Result<PacResult, RoutingError>,
    routing: &RoutingConfig,
) -> io::Result<RouteChoice> {
    match resolved {
        Ok(PacResult::Direct) => Ok(RouteChoice::Direct),
        Ok(PacResult::Proxy { host, port }) => {
            let upstream = routing.pac_upstream(host.clone(), port).ok_or_else(|| {
                io::Error::other("PAC selected a proxy but no [routing.pac] config is present")
            })?;
            let auth = build_auth(&upstream.auth, &host);
            Ok(RouteChoice::Upstream(UpstreamTarget {
                addr: format!("{host}:{port}"),
                auth,
            }))
        }
        Err(err) => match routing
            .pac
            .as_ref()
            .map(|p| p.fail_policy)
            .unwrap_or_default()
        {
            FailPolicy::Direct => {
                tracing::warn!(%err, "PAC resolution failed; failPolicy = direct, falling back to DIRECT");
                Ok(RouteChoice::Direct)
            }
            FailPolicy::Error => Err(io::Error::other(format!("PAC resolution failed: {err}"))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::to_route_choice;
    use zicade_config::{AuthMode, FailPolicy, PacConfig, RoutingConfig, RoutingMode};
    use zicade_proxy::RouteChoice;
    use zicade_routing::{PacResult, RoutingError};

    fn routing_pac(fail: FailPolicy) -> RoutingConfig {
        let mut pac = PacConfig::default();
        pac.auth.mode = AuthMode::Negotiate;
        pac.fail_policy = fail;
        RoutingConfig {
            mode: RoutingMode::Pac,
            upstream: None,
            pac: Some(pac),
        }
    }

    #[test]
    fn proxy_result_builds_upstream_inheriting_pac_auth() {
        // LESSON-6: the PAC-selected upstream inherits routing.pac.auth and its
        // address is the RESOLVED proxy host:port.
        let routing = routing_pac(FailPolicy::Error);
        let choice = to_route_choice(
            Ok(PacResult::Proxy {
                host: "wp8080".to_owned(),
                port: 8080,
            }),
            &routing,
        )
        .expect("proxy result maps to a route choice");
        match choice {
            RouteChoice::Upstream(target) => {
                assert_eq!(target.addr, "wp8080:8080");
                assert_eq!(format!("{:?}", target.auth), "Negotiate");
            }
            RouteChoice::Direct => panic!("expected Upstream, got Direct"),
        }
    }

    #[test]
    fn direct_result_maps_to_direct() {
        let routing = routing_pac(FailPolicy::Error);
        let choice = to_route_choice(Ok(PacResult::Direct), &routing).unwrap();
        assert!(matches!(choice, RouteChoice::Direct));
    }

    #[test]
    fn error_with_fail_policy_direct_falls_back_to_direct() {
        let routing = routing_pac(FailPolicy::Direct);
        let choice =
            to_route_choice(Err(RoutingError::Backend("no PAC".to_owned())), &routing).unwrap();
        assert!(matches!(choice, RouteChoice::Direct));
    }

    #[test]
    fn error_with_fail_policy_error_propagates() {
        let routing = routing_pac(FailPolicy::Error);
        let result = to_route_choice(Err(RoutingError::Backend("no PAC".to_owned())), &routing);
        assert!(result.is_err(), "FailPolicy::Error must surface an error");
    }
}
