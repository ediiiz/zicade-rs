//! Per-connection handling: HTTP forwarding (absolute-form → origin-form) and
//! `CONNECT` dispatch to the tunnel. Body framing is handled by hyper, so
//! request/response bodies stream through byte-for-byte (LESSON-4).

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::header::{HOST, HeaderValue};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::metrics::ProxyMetrics;
use crate::tunnel;
use crate::upstream::{self, PacRouter, RouteChoice, Routing, UpstreamTarget};

pub(crate) type BoxedBody = BoxBody<Bytes, hyper::Error>;
pub(crate) type ForwardError = Box<dyn std::error::Error + Send + Sync>;

/// Serve a single accepted connection. The connection guard is held for the
/// lifetime of the connection and released on drop (even on error/panic).
pub(crate) async fn handle_connection(stream: TcpStream, metrics: ProxyMetrics, routing: Routing) {
    let _guard = metrics.connection_guard();
    let io = TokioIo::new(stream);
    let service = service_fn(move |req| {
        let metrics = metrics.clone();
        let routing = routing.clone();
        async move { Ok::<_, hyper::Error>(proxy_service(req, metrics, routing).await) }
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
) -> Response<BoxedBody> {
    if req.method() == Method::CONNECT {
        return match &routing {
            Routing::Direct => handle_connect(req, metrics),
            Routing::Upstream(target) => handle_connect_upstream(req, target, metrics).await,
            Routing::Pac(router) => handle_connect_pac(req, router, metrics).await,
        };
    }
    metrics.incr_request();
    let result = match &routing {
        Routing::Direct => forward_http(req).await,
        Routing::Upstream(target) => {
            upstream::forward_via_upstream(&target.addr, req, &target.auth).await
        }
        Routing::Pac(router) => forward_pac(req, router).await,
    };
    match result {
        Ok(resp) => resp,
        Err(err) => {
            metrics.incr_failed();
            tracing::warn!(error = %err, "http forward failed");
            error_response(StatusCode::BAD_GATEWAY)
        }
    }
}

/// PAC-mode `CONNECT`: resolve the target through the injected router, then
/// dispatch to the SAME direct/upstream handlers. The PAC input URL is
/// synthesized from the CONNECT authority as `https://host:port`.
async fn handle_connect_pac(
    req: Request<Incoming>,
    router: &PacRouter,
    metrics: ProxyMetrics,
) -> Response<BoxedBody> {
    let Some(dst) = authority_target(req.uri()) else {
        return error_response(StatusCode::BAD_REQUEST);
    };
    match router(format!("https://{dst}")).await {
        Ok(RouteChoice::Direct) => handle_connect(req, metrics),
        Ok(RouteChoice::Upstream(target)) => handle_connect_upstream(req, &target, metrics).await,
        Err(err) => {
            tracing::warn!(error = %err, dst, "PAC resolution failed for CONNECT");
            error_response(StatusCode::BAD_GATEWAY)
        }
    }
}

/// PAC-mode HTTP forward: resolve the (absolute-form) request URI through the
/// injected router, then dispatch to the SAME direct/upstream handlers. A
/// resolver error becomes a `ForwardError` (surfaced as `502` by the caller).
async fn forward_pac(
    req: Request<Incoming>,
    router: &PacRouter,
) -> Result<Response<BoxedBody>, ForwardError> {
    let url = req.uri().to_string();
    match router(url).await? {
        RouteChoice::Direct => forward_http(req).await,
        RouteChoice::Upstream(target) => {
            upstream::forward_via_upstream(&target.addr, req, &target.auth).await
        }
    }
}

/// Upstream-mode `CONNECT`: establish an authenticated tunnel to the target
/// *through* the upstream proxy first, and only then reply 200 and splice.
async fn handle_connect_upstream(
    req: Request<Incoming>,
    target: &UpstreamTarget,
    metrics: ProxyMetrics,
) -> Response<BoxedBody> {
    let Some(dst) = authority_target(req.uri()) else {
        return error_response(StatusCode::BAD_REQUEST);
    };
    let peer = match upstream::connect_via_upstream(&target.addr, &dst, &target.auth).await {
        Ok(peer) => peer,
        Err(err) => {
            tracing::warn!(error = %err, dst, "upstream CONNECT failed");
            return error_response(StatusCode::BAD_GATEWAY);
        }
    };
    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                if let Err(err) = tunnel::splice(upgraded, peer, &metrics).await {
                    tracing::debug!(error = %err, dst, "upstream tunnel closed with error");
                }
            }
            Err(err) => tracing::warn!(error = %err, "CONNECT upgrade failed"),
        }
    });
    Response::new(empty_body())
}

/// On `CONNECT host:port`, reply 200 and splice the upgraded connection to the
/// origin (direct mode).
fn handle_connect(req: Request<Incoming>, metrics: ProxyMetrics) -> Response<BoxedBody> {
    let Some(target) = authority_target(req.uri()) else {
        return error_response(StatusCode::BAD_REQUEST);
    };
    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                if let Err(err) = tunnel::tunnel(upgraded, &target, &metrics).await {
                    tracing::debug!(error = %err, target, "tunnel closed with error");
                }
            }
            Err(err) => tracing::warn!(error = %err, "CONNECT upgrade failed"),
        }
    });
    Response::new(empty_body())
}

/// Forward a plain HTTP request to the origin and stream the response back.
async fn forward_http(req: Request<Incoming>) -> Result<Response<BoxedBody>, ForwardError> {
    let target = authority_target(req.uri()).ok_or("request missing host authority")?;
    let stream = TcpStream::connect(&target).await?;
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
fn to_origin_form(req: Request<Incoming>) -> Request<Incoming> {
    let (mut parts, body) = req.into_parts();

    if !parts.headers.contains_key(HOST) {
        if let Some(auth) = parts.uri.authority() {
            if let Ok(value) = HeaderValue::from_str(auth.as_str()) {
                parts.headers.insert(HOST, value);
            }
        }
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
