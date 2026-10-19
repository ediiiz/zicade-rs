//! Upstream-proxy mode: route CONNECT tunnels and HTTP-forward requests through
//! a configured upstream proxy, performing a multi-leg Negotiate (407) handshake
//! on a single upstream TCP connection.
//!
//! The conversation is raw HTTP/1.1 over one [`TcpStream`] (see [`wire`]) because
//! the handshake must span several request/response legs on the *same*
//! connection — a pooled hyper client cannot express that.

mod auth;
mod wire;

use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::HeaderMap;
use hyper::body::Incoming;
use hyper::client::conn::http1::SendRequest;
use hyper::header::{CONNECTION, PROXY_AUTHORIZATION};
use hyper::http::request::Parts;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use zicade_auth::UpstreamAuthenticator;

use crate::http_forward::{BoxedBody, ForwardError};

use auth::AuthState;

/// Bound on handshake legs, guarding against a looping/hostile upstream.
const MAX_LEGS: u32 = 12;

/// Upper bound on the upstream dial plus the whole CONNECT auth handshake.
/// Without it, a gateway that accepts TCP but stalls mid-handshake parks the
/// client's CONNECT forever (observed with corporate proxies under load).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Builds a fresh authenticator per upstream connection (one credential/context
/// handle lifecycle per connection).
pub type AuthFactory = Arc<dyn Fn() -> Box<dyn UpstreamAuthenticator + Send> + Send + Sync>;

/// How to authenticate to the upstream proxy.
#[derive(Clone)]
pub enum UpstreamAuth {
    /// No proxy authentication (the upstream grants requests directly).
    None,
    /// HTTP Basic. `credentials` is the ready-to-send `Proxy-Authorization`
    /// value (`Basic base64(user:pass)`); it is sent preemptively.
    Basic {
        /// The complete `Proxy-Authorization` header value.
        credentials: String,
    },
    /// Negotiate (SPNEGO/Kerberos-NTLM) via a per-connection authenticator.
    Negotiate(AuthFactory),
}

impl fmt::Debug for UpstreamAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            // Never print the credential.
            Self::Basic { .. } => f.write_str("Basic"),
            Self::Negotiate(_) => f.write_str("Negotiate"),
        }
    }
}

/// A configured upstream proxy target.
#[derive(Clone, Debug)]
pub struct UpstreamTarget {
    /// The upstream proxy address as `host:port`.
    pub addr: String,
    /// How to authenticate to it.
    pub auth: UpstreamAuth,
}

/// The per-request routing decision a [`PacRouter`] produces. Mirrors the fixed
/// [`Routing`] variants so PAC dispatch reuses the SAME direct/upstream code
/// paths.
#[derive(Clone, Debug)]
pub enum RouteChoice {
    /// Connect straight to the origin.
    Direct,
    /// Route through this upstream proxy (carrying its inherited auth).
    Upstream(UpstreamTarget),
}

/// Resolves a request URL to a [`RouteChoice`], once per request. The closure is
/// injected by the app (backed by WinHTTP PAC) so zicade-proxy stays decoupled
/// from zicade-routing / zicade-win — mirroring the [`AuthFactory`] seam. A
/// resolver `Err` is surfaced as `502`, never a panic.
pub type PacRouter = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = io::Result<RouteChoice>> + Send>> + Send + Sync,
>;

/// How the proxy routes outbound traffic.
#[derive(Clone, Default)]
pub enum Routing {
    /// Connect straight to origins (M2 behavior).
    #[default]
    Direct,
    /// Route everything through the configured upstream proxy.
    Upstream(UpstreamTarget),
    /// Resolve each request via an injected PAC router (per-request DIRECT vs
    /// PROXY selection).
    Pac(PacRouter),
}

impl fmt::Debug for Routing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Direct => f.write_str("Direct"),
            Self::Upstream(_) => f.write_str("Upstream"),
            Self::Pac(_) => f.write_str("Pac"),
        }
    }
}

/// Connect to the upstream proxy and establish an authenticated `CONNECT`
/// tunnel to `target`, returning the raw stream ready to splice. The dial and
/// the full handshake are bounded by [`HANDSHAKE_TIMEOUT`].
pub(crate) async fn connect_via_upstream(
    upstream_addr: &str,
    target: &str,
    auth: &UpstreamAuth,
) -> io::Result<TcpStream> {
    match tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        connect_handshake(upstream_addr, target, auth),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "upstream CONNECT handshake with {upstream_addr} timed out after {}s",
                HANDSHAKE_TIMEOUT.as_secs()
            ),
        )),
    }
}

#[allow(
    clippy::cognitive_complexity,
    reason = "the multi-leg 407 loop plus per-leg tracing reads as one handshake"
)]
async fn connect_handshake(
    upstream_addr: &str,
    target: &str,
    auth: &UpstreamAuth,
) -> io::Result<TcpStream> {
    let mut stream = TcpStream::connect(upstream_addr).await?;
    if let Err(err) = stream.set_nodelay(true) {
        tracing::debug!(error = %err, "failed to set TCP_NODELAY on upstream socket");
    }
    tracing::debug!(
        upstream = upstream_addr,
        target,
        "upstream CONNECT handshake begin"
    );
    let mut auth_state = AuthState::new(auth);
    let mut header = auth_state.initial_header();

    for leg in 0..MAX_LEGS {
        let head = connect_request(target, header.as_deref());
        wire::write_all(&mut stream, head.as_bytes()).await?;
        let (status, headers) = wire::read_head(&mut stream).await?;
        tracing::trace!(
            leg,
            status,
            target,
            authed = header.is_some(),
            "upstream CONNECT leg response"
        );

        match status {
            200 => {
                tracing::debug!(
                    upstream = upstream_addr,
                    target,
                    legs = leg + 1,
                    "upstream CONNECT tunnel authorized"
                );
                return Ok(stream);
            }
            407 => {
                // Drain any body so the connection is clean for the next leg.
                wire::drain_body(&mut stream, &headers).await?;
                header = Some(auth_state.on_challenge(&headers)?);
            }
            other => {
                wire::drain_body(&mut stream, &headers).await?;
                return Err(io::Error::other(format!(
                    "upstream refused CONNECT with status {other}"
                )));
            }
        }
    }
    Err(io::Error::other("upstream handshake exhausted legs"))
}

fn connect_request(target: &str, auth_header: Option<&str>) -> String {
    // Match what browsers and Winfoom send to the upstream proxy: an explicit
    // `Proxy-Connection: keep-alive` on the CONNECT. Some corporate gateways
    // otherwise apply a shorter/closing tunnel policy, which can drop long
    // uploads mid-stream.
    let mut head =
        format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n");
    if let Some(value) = auth_header {
        head.push_str(&format!("Proxy-Authorization: {value}\r\n"));
    }
    head.push_str("\r\n");
    head
}

/// Forward a plain HTTP request through the upstream proxy (absolute-form),
/// completing the auth handshake, and stream the final response back.
///
/// Each call opens a fresh upstream connection and runs the full handshake on
/// it: Negotiate/NTLM authenticates the *connection*, so one authenticator
/// lifecycle maps to one connection, and per-request teardown keeps the OS
/// handle count flat (see `forward_burst_drains_connections_and_holds_handle_count`).
/// Reusing an authed connection across a client's keep-alive requests would
/// save the handshake but needs per-client stream state and re-auth on upstream
/// close; it is deliberately not done.
///
/// The request body is buffered because every 407 leg resends it. The response
/// is not: hyper frames it (Content-Length, chunked, or read-until-close) and it
/// streams to the client while the connection task finishes in the background.
pub(crate) async fn forward_via_upstream(
    upstream_addr: &str,
    req: Request<Incoming>,
    auth: &UpstreamAuth,
) -> Result<Response<BoxedBody>, ForwardError> {
    let (parts, body) = req.into_parts();
    let body = body.collect().await?.to_bytes();
    let mut sender = open_upstream_conn(upstream_addr).await?;
    tracing::debug!(
        upstream = upstream_addr,
        method = %parts.method,
        uri = %parts.uri,
        body_len = body.len(),
        "upstream HTTP-forward begin"
    );
    let (resp, legs) = forward_handshake(&mut sender, &parts, &body, auth).await?;
    tracing::debug!(
        upstream = upstream_addr,
        status = resp.status().as_u16(),
        legs,
        "upstream HTTP-forward complete"
    );
    Ok(resp.map(BodyExt::boxed))
}

/// Resend the request, answering each 407 on the same connection, until the
/// upstream returns anything else. Returns that response and the legs it took.
async fn forward_handshake(
    sender: &mut SendRequest<Full<Bytes>>,
    parts: &Parts,
    body: &Bytes,
    auth: &UpstreamAuth,
) -> Result<(Response<Incoming>, u32), ForwardError> {
    let mut auth_state = AuthState::new(auth);
    let mut header = auth_state.initial_header();
    for leg in 0..MAX_LEGS {
        let resp = send_leg(sender, parts, body, header.as_deref()).await?;
        if resp.status() != StatusCode::PROXY_AUTHENTICATION_REQUIRED {
            return Ok((resp, leg + 1));
        }
        tracing::trace!(leg, authed = header.is_some(), "upstream HTTP-forward 407");
        let challenge_headers = drain_challenge(resp).await?;
        header = Some(auth_state.on_challenge(&challenge_headers)?);
    }
    Err("upstream handshake exhausted legs".into())
}

/// Dial the upstream and start an HTTP/1.1 client connection on it. The
/// connection task runs until the last response body is done or dropped.
async fn open_upstream_conn(upstream_addr: &str) -> Result<SendRequest<Full<Bytes>>, ForwardError> {
    let stream = TcpStream::connect(upstream_addr).await?;
    if let Err(err) = stream.set_nodelay(true) {
        tracing::debug!(error = %err, "failed to set TCP_NODELAY on upstream socket");
    }
    let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    tokio::spawn(async move {
        if let Err(err) = conn.await {
            tracing::debug!(error = %err, "upstream HTTP-forward connection ended with error");
        }
    });
    Ok(sender)
}

/// Send one handshake leg on the shared upstream connection.
async fn send_leg(
    sender: &mut SendRequest<Full<Bytes>>,
    parts: &Parts,
    body: &Bytes,
    auth_header: Option<&str>,
) -> Result<Response<Incoming>, ForwardError> {
    sender.ready().await?;
    let request = upstream_request(parts, body.clone(), auth_header)?;
    Ok(sender.send_request(request).await?)
}

/// Take a 407's headers and read its body to the end, so hyper keeps the
/// connection for the next leg. The cap stops a hostile upstream from making us
/// buffer much.
async fn drain_challenge(resp: Response<Incoming>) -> Result<Vec<(String, String)>, ForwardError> {
    let headers = header_pairs(resp.headers());
    Limited::new(resp.into_body(), wire::MAX_DRAIN_BYTES)
        .collect()
        .await?;
    Ok(headers)
}

/// The upstream leg of a forwarded request: absolute-form URI kept as-is, the
/// client's headers minus framing/hop-by-hop/auth (hyper re-derives framing
/// from the body), plus this leg's `Proxy-Authorization`.
fn upstream_request(
    parts: &Parts,
    body: Bytes,
    auth_header: Option<&str>,
) -> Result<Request<Full<Bytes>>, ForwardError> {
    let mut builder = Request::builder()
        .method(parts.method.clone())
        .uri(parts.uri.clone());
    for (name, value) in &parts.headers {
        if matches!(
            name.as_str(),
            "content-length"
                | "transfer-encoding"
                | "connection"
                | "proxy-connection"
                | "proxy-authorization"
        ) {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder = builder.header(CONNECTION, "keep-alive");
    if let Some(value) = auth_header {
        builder = builder.header(PROXY_AUTHORIZATION, value);
    }
    Ok(builder.body(Full::new(body))?)
}

/// Response headers as lowercased name/value pairs, the shape the auth driver
/// reads challenges from. Non-UTF-8 values cannot carry a challenge; skip them.
fn header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
        })
        .collect()
}
