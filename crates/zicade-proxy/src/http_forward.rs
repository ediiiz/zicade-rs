//! Per-connection handling: HTTP forwarding (absolute-form → origin-form) and
//! `CONNECT` dispatch to the tunnel. Body framing is handled by hyper, so
//! request/response bodies stream through byte-for-byte (LESSON-4).
//!
//! Every request and tunnel is registered with the connection tracker as soon
//! as its target is known, and its bytes are counted as they are relayed.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::header::{HOST, HeaderValue};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::counting::{Flow, counting};
use crate::metrics::ProxyMetrics;
use crate::tracker::{ConnectionKind, Tracked};
use crate::tunnel::{self, TunnelInfo};
use crate::upstream::{self, PacRouter, RouteChoice, Routing, UpstreamTarget};

pub(crate) type BoxedBody = BoxBody<Bytes, hyper::Error>;
pub(crate) type ForwardError = Box<dyn std::error::Error + Send + Sync>;

/// The client side of an accepted connection, threaded into the CONNECT
/// handlers so tunnel logs can name both endpoints.
#[derive(Clone, Copy)]
struct ClientEndpoints {
    remote: Option<SocketAddr>,
    local: Option<SocketAddr>,
}

/// Serve a single accepted connection. The connection guard is held for the
/// lifetime of the connection and released on drop (even on error/panic).
pub(crate) async fn handle_connection(stream: TcpStream, metrics: ProxyMetrics, routing: Routing) {
    let _guard = metrics.connection_guard();
    // Capture the client endpoints up front, before the socket is consumed by
    // the upgrade, so tunnel logs can name both ends of the connection.
    let client = ClientEndpoints {
        remote: stream.peer_addr().ok(),
        local: stream.local_addr().ok(),
    };
    // Disable Nagle on the client side: zicade is a relay, and small writes
    // (TLS records, request/response headers) must not wait on delayed ACKs, or
    // the handshake/interactive phase stalls and clients time out and RST.
    if let Err(err) = stream.set_nodelay(true) {
        tracing::debug!(error = %err, "failed to set TCP_NODELAY on client socket");
    }
    let io = TokioIo::new(stream);
    let service = service_fn(move |req| {
        let metrics = metrics.clone();
        let routing = routing.clone();
        async move { Ok::<_, hyper::Error>(proxy_service(req, metrics, routing, client).await) }
    });

    // Per-connection isolation: a connection-level error is logged and dropped;
    // it cannot affect other connections.
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(io, service)
        .with_upgrades()
        .await;
}

async fn proxy_service(
    req: Request<Incoming>,
    metrics: ProxyMetrics,
    routing: Routing,
    client: ClientEndpoints,
) -> Response<BoxedBody> {
    let Some(target) = authority_target(req.uri()) else {
        return error_response(StatusCode::BAD_REQUEST);
    };
    if req.method() == Method::CONNECT {
        let tracked = Tracked::start(
            &metrics,
            ConnectionKind::Tunnel,
            target.clone(),
            route_label(&routing),
            client.remote,
        );
        return match &routing {
            Routing::Direct => handle_connect(req, target, tracked, client),
            Routing::Upstream(upstream) => {
                handle_connect_upstream(req, target, upstream, tracked, client).await
            }
            Routing::Pac(router) => handle_connect_pac(req, target, router, tracked, client).await,
        };
    }
    let kind = ConnectionKind::Http {
        method: req.method().to_string(),
        status: None,
    };
    let tracked = Arc::new(Tracked::start(
        &metrics,
        kind,
        target,
        route_label(&routing),
        client.remote,
    ));
    forward_tracked(req, &routing, tracked).await
}

/// Forward a plain-HTTP request, counting both bodies as they stream. The
/// entry stays open until the response body has been fully relayed.
async fn forward_tracked(
    req: Request<Incoming>,
    routing: &Routing,
    tracked: Arc<Tracked>,
) -> Response<BoxedBody> {
    let req = req.map(|body| counting(body.boxed(), Arc::clone(&tracked), Flow::Up));
    let result = match routing {
        Routing::Direct => forward_http(req).await,
        Routing::Upstream(target) => {
            upstream::forward_via_upstream(&target.addr, req, &target.auth).await
        }
        Routing::Pac(router) => forward_pac(req, router, &tracked).await,
    };
    match result {
        Ok(resp) => {
            tracked.set_status(resp.status().as_u16());
            resp.map(|body| counting(body, tracked, Flow::Down))
        }
        Err(err) => {
            tracing::warn!(error = %err, "http forward failed");
            tracked.fail(&err);
            error_response(StatusCode::BAD_GATEWAY)
        }
    }
}

/// How a request is routed before any per-request decision: `"direct"`, the
/// upstream's `host:port`, or `"pac"` until the PAC script has answered.
fn route_label(routing: &Routing) -> String {
    match routing {
        Routing::Direct => "direct".to_owned(),
        Routing::Upstream(target) => target.addr.clone(),
        Routing::Pac(_) => "pac".to_owned(),
    }
}

/// PAC-mode `CONNECT`: resolve the target through the injected router, then
/// dispatch to the SAME direct/upstream handlers. The PAC input URL is
/// synthesized from the CONNECT authority as `https://host:port`.
async fn handle_connect_pac(
    req: Request<Incoming>,
    dst: String,
    router: &PacRouter,
    tracked: Tracked,
    client: ClientEndpoints,
) -> Response<BoxedBody> {
    match router(format!("https://{dst}")).await {
        Ok(RouteChoice::Direct) => {
            tracing::debug!(dst, "PAC selected DIRECT for CONNECT");
            tracked.set_route("direct");
            handle_connect(req, dst, tracked, client)
        }
        Ok(RouteChoice::Upstream(target)) => {
            tracing::debug!(dst, upstream = %target.addr, "PAC selected upstream for CONNECT");
            tracked.set_route(target.addr.clone());
            handle_connect_upstream(req, dst, &target, tracked, client).await
        }
        Err(err) => {
            tracing::warn!(error = %err, dst, "PAC resolution failed for CONNECT");
            tracked.fail(format!("PAC resolution failed: {err}"));
            error_response(StatusCode::BAD_GATEWAY)
        }
    }
}

/// PAC-mode HTTP forward: resolve the (absolute-form) request URI through the
/// injected router, then dispatch to the SAME direct/upstream handlers. A
/// resolver error becomes a `ForwardError` (surfaced as `502` by the caller).
async fn forward_pac(
    req: Request<BoxedBody>,
    router: &PacRouter,
    tracked: &Tracked,
) -> Result<Response<BoxedBody>, ForwardError> {
    let url = req.uri().to_string();
    match router(url).await? {
        RouteChoice::Direct => {
            tracked.set_route("direct");
            forward_http(req).await
        }
        RouteChoice::Upstream(target) => {
            tracked.set_route(target.addr.clone());
            upstream::forward_via_upstream(&target.addr, req, &target.auth).await
        }
    }
}

/// Upstream-mode `CONNECT`: establish an authenticated tunnel to the target
/// *through* the upstream proxy first, and only then reply 200 and splice.
async fn handle_connect_upstream(
    req: Request<Incoming>,
    dst: String,
    target: &UpstreamTarget,
    tracked: Tracked,
    client: ClientEndpoints,
) -> Response<BoxedBody> {
    tracing::info!(
        client_remote = %fmt_addr(client.remote),
        target = %dst,
        upstream = %target.addr,
        "connect request received"
    );
    let dial_start = Instant::now();
    let peer = match upstream::connect_via_upstream(&target.addr, &dst, &target.auth).await {
        Ok(peer) => peer,
        Err(err) => {
            tracing::warn!(error = %err, dst, upstream = %target.addr, "upstream CONNECT failed");
            tracked.fail(format!("upstream CONNECT failed: {err}"));
            return error_response(StatusCode::BAD_GATEWAY);
        }
    };
    tracing::info!(
        client_remote = %fmt_addr(client.remote),
        target = %dst,
        upstream = %target.addr,
        dial_ms = dial_start.elapsed().as_secs_f64() * 1000.0,
        "CONNECT established (via upstream)"
    );
    let info = TunnelInfo {
        client_remote: client.remote,
        client_local: client.local,
        target: dst,
        upstream: Some(target.addr.clone()),
    };
    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                let _ = tunnel::splice(upgraded, peer, &tracked, info).await;
            }
            Err(err) => {
                tracing::warn!(error = %err, "CONNECT upgrade failed");
                tracked.fail(format!("CONNECT upgrade failed: {err}"));
            }
        }
    });
    Response::new(empty_body())
}

/// On `CONNECT host:port`, reply 200 and splice the upgraded connection to the
/// origin (direct mode).
fn handle_connect(
    req: Request<Incoming>,
    target: String,
    tracked: Tracked,
    client: ClientEndpoints,
) -> Response<BoxedBody> {
    tracing::info!(
        client_remote = %fmt_addr(client.remote),
        target = %target,
        upstream = "direct",
        "connect request received"
    );
    let info = TunnelInfo {
        client_remote: client.remote,
        client_local: client.local,
        target,
        upstream: None,
    };
    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                let _ = tunnel::tunnel(upgraded, &tracked, info).await;
            }
            Err(err) => {
                tracing::warn!(error = %err, "CONNECT upgrade failed");
                tracked.fail(format!("CONNECT upgrade failed: {err}"));
            }
        }
    });
    Response::new(empty_body())
}

/// Render an optional socket address for logging (`-` when unknown).
fn fmt_addr(a: Option<SocketAddr>) -> String {
    a.map(|a| a.to_string()).unwrap_or_else(|| "-".to_owned())
}

/// Forward a plain HTTP request to the origin and stream the response back.
async fn forward_http(req: Request<BoxedBody>) -> Result<Response<BoxedBody>, ForwardError> {
    let target = authority_target(req.uri()).ok_or("request missing host authority")?;
    let stream = TcpStream::connect(&target).await?;
    if let Err(err) = stream.set_nodelay(true) {
        tracing::debug!(error = %err, "failed to set TCP_NODELAY on origin socket");
    }
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let outbound = to_origin_form(req);
    let resp = sender.send_request(outbound).await?;
    let (parts, body) = resp.into_parts();
    Ok(Response::from_parts(parts, body.boxed()))
}

/// Rewrite an absolute-form proxy request into an origin-form request for the
/// upstream connection, preserving method/headers/body and setting `Host`.
fn to_origin_form<B>(req: Request<B>) -> Request<B> {
    let (mut parts, body) = req.into_parts();

    if !parts.headers.contains_key(HOST)
        && let Some(auth) = parts.uri.authority()
        && let Ok(value) = HeaderValue::from_str(auth.as_str())
    {
        parts.headers.insert(HOST, value);
    }

    let origin_form = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_owned())
        .unwrap_or_else(|| "/".to_owned());
    parts.uri = origin_form
        .parse()
        .unwrap_or_else(|_| Uri::from_static("/"));

    // Drop hop-by-hop proxy headers.
    parts.headers.remove("proxy-connection");

    Request::from_parts(parts, body)
}

/// Extract `host:port` from a URI authority (default port 80 for HTTP).
fn authority_target(uri: &Uri) -> Option<String> {
    let authority = uri.authority()?;
    let port = authority.port_u16().unwrap_or(80);
    Some(format!("{}:{}", authority.host(), port))
}

fn empty_body() -> BoxedBody {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

fn error_response(status: StatusCode) -> Response<BoxedBody> {
    let mut resp = Response::new(empty_body());
    *resp.status_mut() = status;
    resp
}
