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
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::http::request::Parts;
use hyper::{Request, Response};
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

// The complexity is the multi-leg 407 auth loop plus its per-leg tracing; the
// legs form one cohesive handshake and read more clearly kept together.
#[allow(clippy::cognitive_complexity)]
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
                wire::read_body(&mut stream, &headers).await?;
                header = Some(auth_state.on_challenge(&headers)?);
            }
            other => {
                wire::read_body(&mut stream, &headers).await?;
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
/// completing the Negotiate handshake, and return the final response.
///
/// Each call opens a **fresh** upstream `TcpStream` and runs the full multi-leg
/// auth handshake, then drops the stream when the response is buffered. This is
/// an intentional, correct, leak-free choice — not an oversight (Issue #5):
///
/// - Negotiate/NTLM authenticates the *TCP connection*, not the request, so one
///   authenticator/credential-handle lifecycle maps cleanly to one connection.
/// - The per-request connect/auth/teardown holds a flat OS-handle steady state
///   (LESSON-7); the RAII drop here is what guarantees that. See the
///   `forward_burst_drains_connections_and_holds_handle_count` regression test.
///
/// Reusing an authenticated upstream stream across requests on the same client
/// keep-alive connection would save the handshake cost, but it is **deliberately
/// deferred**: it needs per-client-connection stream state with interior
/// mutability, mid-reuse reconnect/re-auth on upstream close, and careful keep-
/// alive framing — meaningful complexity with real risk to the leak-free
/// guarantee, for a latency win that does not outweigh it here.
// The complexity is the multi-leg 407 auth loop plus its per-leg tracing; the
// legs form one cohesive handshake and read more clearly kept together.
#[allow(clippy::cognitive_complexity)]
pub(crate) async fn forward_via_upstream(
    upstream_addr: &str,
    req: Request<Incoming>,
    auth: &UpstreamAuth,
) -> Result<Response<BoxedBody>, ForwardError> {
    let (parts, body) = req.into_parts();
    let body = body.collect().await?.to_bytes();

    let mut stream = TcpStream::connect(upstream_addr).await?;
    if let Err(err) = stream.set_nodelay(true) {
        tracing::debug!(error = %err, "failed to set TCP_NODELAY on upstream socket");
    }
    tracing::debug!(
        upstream = upstream_addr,
        method = %parts.method,
        uri = %parts.uri,
        body_len = body.len(),
        "upstream HTTP-forward begin"
    );
    let mut auth_state = AuthState::new(auth);
    let mut header = auth_state.initial_header();

    for leg in 0..MAX_LEGS {
        let head = forward_request_head(&parts, body.len(), header.as_deref());
        wire::write_all(&mut stream, head.as_bytes()).await?;
        wire::write_all(&mut stream, &body).await?;

        let (status, headers) = wire::read_head(&mut stream).await?;
        let resp_body = wire::read_body(&mut stream, &headers).await?;
        tracing::trace!(
            leg,
            status,
            authed = header.is_some(),
            resp_len = resp_body.len(),
            "upstream HTTP-forward leg response"
        );

        if status == 407 {
            header = Some(auth_state.on_challenge(&headers)?);
            continue;
        }
        tracing::debug!(
            upstream = upstream_addr,
            status,
            legs = leg + 1,
            "upstream HTTP-forward complete"
        );
        return build_response(status, &headers, resp_body);
    }
    Err("upstream handshake exhausted legs".into())
}

fn forward_request_head(parts: &Parts, body_len: usize, auth_header: Option<&str>) -> String {
    let mut head = format!("{} {} HTTP/1.1\r\n", parts.method, parts.uri);
    for (name, value) in &parts.headers {
        let key = name.as_str().to_ascii_lowercase();
        // Re-derive framing/auth/hop-by-hop headers ourselves.
        if matches!(
            key.as_str(),
            "content-length"
                | "transfer-encoding"
                | "connection"
                | "proxy-connection"
                | "proxy-authorization"
        ) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    head.push_str(&format!("Content-Length: {body_len}\r\n"));
    head.push_str("Connection: keep-alive\r\n");
    if let Some(value) = auth_header {
        head.push_str(&format!("Proxy-Authorization: {value}\r\n"));
    }
    head.push_str("\r\n");
    head
}

fn build_response(
    status: u16,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Result<Response<BoxedBody>, ForwardError> {
    let mut builder = Response::builder().status(status);
    for (key, value) in headers {
        // Framing is re-derived from the buffered body (Full sets it).
        if matches!(
            key.as_str(),
            "content-length" | "transfer-encoding" | "connection"
        ) {
            continue;
        }
        builder = builder.header(key.as_str(), value.as_str());
    }
    let body = Full::new(Bytes::from(body))
        .map_err(|never| match never {})
        .boxed();
    Ok(builder.body(body)?)
}
