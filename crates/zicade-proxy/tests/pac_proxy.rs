//! PAC-mode data-path tests: the proxy resolves each request through an
//! injected [`PacRouter`] closure and dispatches to the SAME direct/upstream
//! code paths as the fixed [`Routing`] variants. A fake in-test router stands in
//! for the real WinHTTP-backed resolver, so these tests are deterministic and
//! run on any OS.

mod common;

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

use common::TestProxy;
use common::client::{ReqBody, proxy_connect, proxy_http};
use common::origin::{spawn_http_echo, spawn_tcp_echo};
use common::upstream_fake::{negotiate_factory, spawn_negotiate_upstream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zicade_proxy::{PacRouter, RouteChoice, Routing, UpstreamTarget};

/// Build a [`PacRouter`] from a synchronous decision function.
fn router_from<F>(f: F) -> PacRouter
where
    F: Fn(String) -> io::Result<RouteChoice> + Send + Sync + 'static,
{
    Arc::new(move |url: String| {
        let decision = f(url);
        Box::pin(async move { decision }) as Pin<Box<dyn Future<Output = _> + Send>>
    })
}

#[tokio::test]
async fn pac_direct_connect_tunnels_straight_to_origin() {
    let origin = spawn_tcp_echo().await;
    // DIRECT: the proxy must connect straight to the CONNECT authority.
    let router = router_from(|_url| Ok(RouteChoice::Direct));
    let proxy = TestProxy::spawn_with_routing(Routing::Pac(router)).await;

    let mut tunnel = proxy_connect(proxy.addr, &origin.to_string()).await;
    tunnel.write_all(b"pac-direct-bytes").await.unwrap();
    let mut buf = vec![0u8; b"pac-direct-bytes".len()];
    tunnel.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"pac-direct-bytes");

    drop(tunnel);
    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn pac_direct_http_forward_reaches_origin() {
    let origin = spawn_http_echo().await;
    let router = router_from(|_url| Ok(RouteChoice::Direct));
    let proxy = TestProxy::spawn_with_routing(Routing::Pac(router)).await;

    let body = b"pac-direct-post".to_vec();
    let url = format!("http://{origin}/echo");
    let resp = proxy_http(
        proxy.addr,
        "POST",
        &url,
        &[],
        ReqBody::ContentLength(body.clone()),
    )
    .await;

    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("x-origin-echo"), Some("1"));
    assert_eq!(resp.body, body);

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn pac_upstream_connect_runs_negotiate_handshake() {
    let upstream = spawn_negotiate_upstream().await;
    let upstream_addr = upstream.to_string();
    // External: the resolver selects the upstream, carrying its auth.
    let router = router_from(move |_url| {
        Ok(RouteChoice::Upstream(UpstreamTarget {
            addr: upstream_addr.clone(),
            auth: negotiate_factory(),
        }))
    });
    let proxy = TestProxy::spawn_with_routing(Routing::Pac(router)).await;

    let mut tunnel = proxy_connect(proxy.addr, "example.com:443").await;
    tunnel.write_all(b"pac-upstream-bytes").await.unwrap();
    let mut buf = vec![0u8; b"pac-upstream-bytes".len()];
    tunnel.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"pac-upstream-bytes");

    drop(tunnel);
    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn pac_upstream_http_forward_runs_negotiate_handshake() {
    let upstream = spawn_negotiate_upstream().await;
    let upstream_addr = upstream.to_string();
    let router = router_from(move |_url| {
        Ok(RouteChoice::Upstream(UpstreamTarget {
            addr: upstream_addr.clone(),
            auth: negotiate_factory(),
        }))
    });
    let proxy = TestProxy::spawn_with_routing(Routing::Pac(router)).await;

    let body = b"pac-upstream-post".to_vec();
    let resp = proxy_http(
        proxy.addr,
        "POST",
        "http://example.com/echo",
        &[],
        ReqBody::ContentLength(body.clone()),
    )
    .await;

    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.body, body,
        "body must echo back byte-identical through the authed upstream"
    );

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn pac_resolver_error_on_http_forward_is_bad_gateway() {
    // FailPolicy::Error is expressed at the app layer as a resolver `Err`; the
    // proxy must surface it as 502, never panic.
    let router = router_from(|_url| Err(io::Error::other("pac resolution failed")));
    let proxy = TestProxy::spawn_with_routing(Routing::Pac(router)).await;

    let resp = proxy_http(
        proxy.addr,
        "POST",
        "http://example.com/echo",
        &[],
        ReqBody::ContentLength(b"nope".to_vec()),
    )
    .await;

    assert_eq!(resp.status, 502, "resolver error must surface as 502");

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn pac_resolver_error_on_connect_is_bad_gateway() {
    use tokio::net::TcpStream;

    let router = router_from(|_url| Err(io::Error::other("pac resolution failed")));
    let proxy = TestProxy::spawn_with_routing(Routing::Pac(router)).await;

    let mut stream = TcpStream::connect(proxy.addr).await.unwrap();
    stream
        .write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp).await.unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let status: u16 = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    assert_eq!(status, 502, "resolver error on CONNECT must yield 502");

    proxy.shutdown().await.unwrap();
}
