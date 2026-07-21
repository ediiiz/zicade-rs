//! Upstream-proxy mode: route CONNECT tunnels and HTTP-forward requests through
//! a configured upstream proxy, performing a multi-leg Negotiate (407) handshake
//! on a single upstream TCP connection.
//!
//! The conversation is raw HTTP/1.1 over one [`TcpStream`] (see [`wire`]) because
//! the handshake must span several request/response legs on the *same*
//! connection — a pooled hyper client cannot express that.

mod wire;

use std::fmt;
use std::io;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::http::request::Parts;
use hyper::{Request, Response};
use tokio::net::TcpStream;
use zicade_auth::header::{NegotiateOffer, build_proxy_authorization, parse_proxy_authenticate};
use zicade_auth::{
    AuthError, HandshakeStep, NegotiateHandshake, UpstreamAuthenticator, UpstreamResponse,
};

use crate::http_forward::{BoxedBody, ForwardError};

/// Bound on handshake legs, guarding against a looping/hostile upstream.
const MAX_LEGS: u32 = 12;

/// Builds a fresh authenticator per upstream connection (one credential/context
/// handle lifecycle per connection).
pub type AuthFactory = Arc<dyn Fn() -> Box<dyn UpstreamAuthenticator + Send> + Send + Sync>;

/// How to authenticate to the upstream proxy.
#[derive(Clone)]
pub enum UpstreamAuth {
    /// No proxy authentication (the upstream grants requests directly).
    None,
    /// Negotiate (SPNEGO/Kerberos-NTLM) via a per-connection authenticator.
    Negotiate(AuthFactory),
}

impl fmt::Debug for UpstreamAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("None"),
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

/// How the proxy routes outbound traffic.
#[derive(Clone, Default)]
pub enum Routing {
    /// Connect straight to origins (M2 behavior).
    #[default]
    Direct,
    /// Route everything through the configured upstream proxy.
    Upstream(UpstreamTarget),
}

impl fmt::Debug for Routing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Direct => f.write_str("Direct"),
            Self::Upstream(_) => f.write_str("Upstream"),
        }
    }
}

/// Adapts a boxed trait object into a concrete [`UpstreamAuthenticator`] so it
/// can drive the generic [`NegotiateHandshake`].
struct BoxAuth(Box<dyn UpstreamAuthenticator + Send>);

impl UpstreamAuthenticator for BoxAuth {
    fn step(&mut self, challenge: Option<&[u8]>) -> Result<Vec<u8>, AuthError> {
        self.0.step(challenge)
    }
}

fn new_handshake(auth: &UpstreamAuth) -> Option<NegotiateHandshake<BoxAuth>> {
    match auth {
        UpstreamAuth::None => None,
        UpstreamAuth::Negotiate(factory) => Some(NegotiateHandshake::new(BoxAuth(factory()))),
    }
}

/// The decoded Negotiate challenge from a `Proxy-Authenticate` header, if any.
fn negotiate_challenge(headers: &[(String, String)]) -> Option<Vec<u8>> {
    let value = headers
        .iter()
        .find(|(k, _)| k == "proxy-authenticate")
        .map(|(_, v)| v.as_str())?;
    match parse_proxy_authenticate(value) {
        NegotiateOffer::Challenge(bytes) => Some(bytes),
        NegotiateOffer::Offered | NegotiateOffer::NotOffered => None,
    }
}

fn auth_io_err(err: AuthError) -> io::Error {
    io::Error::other(format!("upstream negotiate handshake failed: {err}"))
}

/// Drive one 407 leg: feed the challenge to the handshake and return the next
/// `Proxy-Authorization` token, or an error if the handshake has no more work.
fn advance_handshake(
    handshake: Option<&mut NegotiateHandshake<BoxAuth>>,
    headers: &[(String, String)],
) -> io::Result<Vec<u8>> {
    let handshake = handshake
        .ok_or_else(|| io::Error::other("upstream demanded proxy auth but none is configured"))?;
    let challenge = negotiate_challenge(headers);
    match handshake
        .on_response(UpstreamResponse::ProxyAuthRequired { challenge })
        .map_err(auth_io_err)?
    {
        HandshakeStep::SendToken(token) => Ok(token),
        HandshakeStep::Complete => Err(io::Error::other(
            "handshake completed while upstream still demanded auth",
        )),
    }
}

/// Connect to the upstream proxy and establish an authenticated `CONNECT`
/// tunnel to `target`, returning the raw stream ready to splice.
pub(crate) async fn connect_via_upstream(
    upstream_addr: &str,
    target: &str,
    auth: &UpstreamAuth,
) -> io::Result<TcpStream> {
    let mut stream = TcpStream::connect(upstream_addr).await?;
    let mut handshake = new_handshake(auth);
    let mut token: Option<Vec<u8>> = None;

    for _ in 0..MAX_LEGS {
        let head = connect_request(target, token.as_deref());
        wire::write_all(&mut stream, head.as_bytes()).await?;
        let (status, headers) = wire::read_head(&mut stream).await?;

        match status {
            200 => return Ok(stream),
            407 => {
                // Drain any body so the connection is clean for the next leg.
                wire::read_body(&mut stream, &headers).await?;
                token = Some(advance_handshake(handshake.as_mut(), &headers)?);
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

fn connect_request(target: &str, token: Option<&[u8]>) -> String {
    let mut head = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some(token) = token {
        head.push_str(&format!(
            "Proxy-Authorization: {}\r\n",
            build_proxy_authorization(token)
        ));
    }
    head.push_str("\r\n");
    head
}

/// Forward a plain HTTP request through the upstream proxy (absolute-form),
/// completing the Negotiate handshake, and return the final response.
pub(crate) async fn forward_via_upstream(
    upstream_addr: &str,
    req: Request<Incoming>,
    auth: &UpstreamAuth,
) -> Result<Response<BoxedBody>, ForwardError> {
    let (parts, body) = req.into_parts();
    let body = body.collect().await?.to_bytes();

    let mut stream = TcpStream::connect(upstream_addr).await?;
    let mut handshake = new_handshake(auth);
    let mut token: Option<Vec<u8>> = None;

    for _ in 0..MAX_LEGS {
        let head = forward_request_head(&parts, body.len(), token.as_deref());
        wire::write_all(&mut stream, head.as_bytes()).await?;
        wire::write_all(&mut stream, &body).await?;

        let (status, headers) = wire::read_head(&mut stream).await?;
        let resp_body = wire::read_body(&mut stream, &headers).await?;

        if status == 407 {
            token = Some(advance_handshake(handshake.as_mut(), &headers)?);
            continue;
        }
        return build_response(status, &headers, resp_body);
    }
    Err("upstream handshake exhausted legs".into())
}

fn forward_request_head(parts: &Parts, body_len: usize, token: Option<&[u8]>) -> String {
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
    if let Some(token) = token {
        head.push_str(&format!(
            "Proxy-Authorization: {}\r\n",
            build_proxy_authorization(token)
        ));
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
