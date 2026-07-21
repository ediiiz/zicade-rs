//! In-process fake upstream proxy that emulates the multi-leg Negotiate 407
//! dance on a single keep-alive TCP connection, plus a scripted fake
//! authenticator. No SSPI, no real network beyond loopback.

use std::net::SocketAddr;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use zicade_auth::{AuthError, UpstreamAuthenticator};
use zicade_proxy::{AuthFactory, UpstreamAuth};

/// The continuation token the fake upstream hands back on the first authed leg.
const SERVER_CHALLENGE: &[u8] = b"srv-challenge";

const R407_BARE: &str = "HTTP/1.1 407 Proxy Authentication Required\r\n\
Proxy-Authenticate: Negotiate\r\n\
Content-Length: 0\r\n\
Connection: keep-alive\r\n\r\n";

/// A scripted authenticator that returns two fixed tokens across its two legs.
struct FakeAuth {
    tokens: Vec<Vec<u8>>,
    next: usize,
}

impl FakeAuth {
    fn new() -> Self {
        Self {
            tokens: vec![b"tok1".to_vec(), b"tok2".to_vec()],
            next: 0,
        }
    }
}

impl UpstreamAuthenticator for FakeAuth {
    fn step(&mut self, _challenge: Option<&[u8]>) -> Result<Vec<u8>, AuthError> {
        let token = self
            .tokens
            .get(self.next)
            .cloned()
            .ok_or_else(|| AuthError::Authenticator("fake ran out of tokens".into()))?;
        self.next += 1;
        Ok(token)
    }
}

/// An [`AuthFactory`] that builds a fresh scripted fake per upstream connection.
pub fn negotiate_factory() -> UpstreamAuth {
    let factory: AuthFactory =
        Arc::new(|| Box::new(FakeAuth::new()) as Box<dyn UpstreamAuthenticator + Send>);
    UpstreamAuth::Negotiate(factory)
}

/// Spawn a fake upstream proxy that requires the 3-leg Negotiate handshake:
/// bare 407 -> challenge 407 -> success. On success a `CONNECT` becomes a raw
/// echo tunnel; a plain HTTP request echoes its body back with `Content-Length`.
pub async fn spawn_negotiate_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_negotiate(stream));
        }
    });
    addr
}

async fn serve_negotiate(mut stream: TcpStream) {
    let mut authed_legs = 0u32;
    loop {
        let Some((head, body)) = read_request(&mut stream).await else {
            return;
        };
        let has_auth = head.to_ascii_lowercase().contains("proxy-authorization");
        let is_connect = head.starts_with("CONNECT");

        if !has_auth {
            if stream.write_all(R407_BARE.as_bytes()).await.is_err() {
                return;
            }
            continue;
        }
        authed_legs += 1;
        if authed_legs == 1 {
            let challenge = STANDARD.encode(SERVER_CHALLENGE);
            let resp = format!(
                "HTTP/1.1 407 Proxy Authentication Required\r\n\
Proxy-Authenticate: Negotiate {challenge}\r\n\
Content-Length: 0\r\n\
Connection: keep-alive\r\n\r\n"
            );
            if stream.write_all(resp.as_bytes()).await.is_err() {
                return;
            }
            continue;
        }
        // Second authed leg: the handshake completed.
        if is_connect {
            let _ = stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await;
            echo_tunnel(stream).await;
            return;
        }
        let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
        let _ = stream.write_all(resp.as_bytes()).await;
        let _ = stream.write_all(&body).await;
        return;
    }
}

/// A scripted authenticator that never runs out of tokens, so the *proxy's*
/// leg cap (not the authenticator) is what bounds a looping handshake.
struct InfiniteAuth {
    next: usize,
}

impl UpstreamAuthenticator for InfiniteAuth {
    fn step(&mut self, _challenge: Option<&[u8]>) -> Result<Vec<u8>, AuthError> {
        let token = format!("tok{}", self.next).into_bytes();
        self.next += 1;
        Ok(token)
    }
}

/// An [`AuthFactory`] whose authenticator answers every challenge, used to prove
/// the proxy caps handshake legs on its own.
pub fn infinite_negotiate_factory() -> UpstreamAuth {
    let factory: AuthFactory =
        Arc::new(|| Box::new(InfiniteAuth { next: 0 }) as Box<dyn UpstreamAuthenticator + Send>);
    UpstreamAuth::Negotiate(factory)
}

/// Spawn a hostile fake upstream that answers EVERY leg with a fresh Negotiate
/// 407 challenge and never grants 200. The connection is kept alive so the
/// proxy keeps handshaking until its own leg cap trips; the fake then observes
/// the closed socket and exits (no hang, no leak on the fake side).
pub async fn spawn_looping_negotiate_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let challenge = STANDARD.encode(SERVER_CHALLENGE);
                let resp = format!(
                    "HTTP/1.1 407 Proxy Authentication Required\r\n\
Proxy-Authenticate: Negotiate {challenge}\r\n\
Content-Length: 0\r\n\
Connection: keep-alive\r\n\r\n"
                );
                while read_request(&mut stream).await.is_some() {
                    if stream.write_all(resp.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

const R407_BASIC: &str = "HTTP/1.1 407 Proxy Authentication Required\r\n\
Proxy-Authenticate: Basic realm=\"corp\"\r\n\
Content-Length: 0\r\n\
Connection: keep-alive\r\n\r\n";

/// Build an [`UpstreamAuth::Basic`] carrying a ready-to-send header value.
pub fn basic_auth(credentials: &str) -> UpstreamAuth {
    UpstreamAuth::Basic {
        credentials: credentials.to_owned(),
    }
}

/// Spawn a fake upstream that demands HTTP Basic: it answers `407` with a
/// `Proxy-Authenticate: Basic` challenge until it sees the exact
/// `Proxy-Authorization: <expected>` value, then `200` (CONNECT becomes an echo
/// tunnel; a plain request echoes its body). A wrong credential loops on `407`.
pub async fn spawn_basic_upstream(expected: &'static str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_basic(stream, expected));
        }
    });
    addr
}

async fn serve_basic(mut stream: TcpStream, expected: &str) {
    loop {
        let Some((head, body)) = read_request(&mut stream).await else {
            return;
        };
        if !head_has_authorization(&head, expected) {
            if stream.write_all(R407_BASIC.as_bytes()).await.is_err() {
                return;
            }
            continue;
        }
        if head.starts_with("CONNECT") {
            let _ = stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await;
            echo_tunnel(stream).await;
            return;
        }
        let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
        let _ = stream.write_all(resp.as_bytes()).await;
        let _ = stream.write_all(&body).await;
        return;
    }
}

/// Whether the request head carries `Proxy-Authorization: <expected>` (header
/// name case-insensitive, value exact).
fn head_has_authorization(head: &str, expected: &str) -> bool {
    head.split("\r\n").any(|line| {
        line.split_once(':')
            .map(|(k, v)| {
                k.trim().eq_ignore_ascii_case("proxy-authorization") && v.trim() == expected
            })
            .unwrap_or(false)
    })
}

/// Spawn a fake upstream that grants a `CONNECT` tunnel immediately with no auth.
pub async fn spawn_noauth_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                if read_request(&mut stream).await.is_none() {
                    return;
                }
                let _ = stream
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await;
                echo_tunnel(stream).await;
            });
        }
    });
    addr
}

async fn echo_tunnel(mut stream: TcpStream) {
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stream.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Read one request head (to `\r\n\r\n`) plus any `Content-Length` body.
async fn read_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let cl = content_length(&head);
    let mut body = buf[head_end..].to_vec();
    while body.len() < cl {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(cl);
    Some((head, body))
}

fn content_length(head: &str) -> usize {
    for line in head.split("\r\n") {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                return v.trim().parse().unwrap_or(0);
            }
        }
    }
    0
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
