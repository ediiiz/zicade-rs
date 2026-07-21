//! M3c upstream-mode tests: route CONNECT tunnels and HTTP-forward requests
//! through a configured upstream proxy, completing a multi-leg Negotiate 407
//! handshake against an in-process fake upstream with a fake authenticator.

mod common;

use common::TestProxy;
use common::client::{ReqBody, proxy_connect, proxy_http};
use common::upstream_fake::{
    basic_auth, negotiate_factory, spawn_basic_upstream, spawn_negotiate_upstream,
    spawn_noauth_upstream,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zicade_proxy::{Routing, UpstreamAuth, UpstreamTarget};

#[tokio::test]
async fn connect_through_upstream_negotiate_tunnels() {
    let upstream = spawn_negotiate_upstream().await;
    let routing = Routing::Upstream(UpstreamTarget {
        addr: upstream.to_string(),
        auth: negotiate_factory(),
    });
    let proxy = TestProxy::spawn_with_routing(routing).await;

    // The CONNECT target is opaque to the fake; the 3-leg 407 dance must
    // complete before the tunnel is spliced.
    let mut tunnel = proxy_connect(proxy.addr, "example.com:443").await;
    tunnel.write_all(b"through-the-upstream").await.unwrap();
    let mut buf = vec![0u8; b"through-the-upstream".len()];
    tunnel.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"through-the-upstream");

    drop(tunnel);
    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_forward_through_upstream_negotiate_echoes_body() {
    let upstream = spawn_negotiate_upstream().await;
    let routing = Routing::Upstream(UpstreamTarget {
        addr: upstream.to_string(),
        auth: negotiate_factory(),
    });
    let proxy = TestProxy::spawn_with_routing(routing).await;

    let body = b"post-body-through-upstream".to_vec();
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

/// The credential the fake Basic upstream accepts (Aladdin:open sesame).
const GOOD_BASIC: &str = "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==";

#[tokio::test]
async fn connect_through_upstream_basic_tunnels() {
    let upstream = spawn_basic_upstream(GOOD_BASIC).await;
    let routing = Routing::Upstream(UpstreamTarget {
        addr: upstream.to_string(),
        auth: basic_auth(GOOD_BASIC),
    });
    let proxy = TestProxy::spawn_with_routing(routing).await;

    let mut tunnel = proxy_connect(proxy.addr, "example.com:443").await;
    tunnel.write_all(b"basic-tunnel-bytes").await.unwrap();
    let mut buf = vec![0u8; b"basic-tunnel-bytes".len()];
    tunnel.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"basic-tunnel-bytes");

    drop(tunnel);
    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_forward_through_upstream_basic_echoes_body() {
    let upstream = spawn_basic_upstream(GOOD_BASIC).await;
    let routing = Routing::Upstream(UpstreamTarget {
        addr: upstream.to_string(),
        auth: basic_auth(GOOD_BASIC),
    });
    let proxy = TestProxy::spawn_with_routing(routing).await;

    let body = b"post-body-basic-upstream".to_vec();
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
        "body must echo back byte-identical through the Basic-authed upstream"
    );

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_forward_through_upstream_basic_wrong_credentials_fails_cleanly() {
    // The upstream only accepts GOOD_BASIC; the proxy is configured with a
    // different credential, so the handshake must fail cleanly (502), not loop.
    let upstream = spawn_basic_upstream(GOOD_BASIC).await;
    let routing = Routing::Upstream(UpstreamTarget {
        addr: upstream.to_string(),
        auth: basic_auth("Basic d3Jvbmc6Y3JlZHM="),
    });
    let proxy = TestProxy::spawn_with_routing(routing).await;

    let resp = proxy_http(
        proxy.addr,
        "POST",
        "http://example.com/echo",
        &[],
        ReqBody::ContentLength(b"nope".to_vec()),
    )
    .await;

    assert_eq!(
        resp.status, 502,
        "wrong Basic credential must surface as 502 without looping"
    );

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn connect_through_upstream_basic_wrong_credentials_fails_cleanly() {
    // Raw CONNECT (proxy_connect asserts 200, so drive the socket by hand) must
    // get a non-200 (502) reply rather than hang when the credential is wrong.
    use tokio::net::TcpStream;

    let upstream = spawn_basic_upstream(GOOD_BASIC).await;
    let routing = Routing::Upstream(UpstreamTarget {
        addr: upstream.to_string(),
        auth: basic_auth("Basic d3Jvbmc6Y3JlZHM="),
    });
    let proxy = TestProxy::spawn_with_routing(routing).await;

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
    assert_eq!(
        status, 502,
        "wrong Basic credential must yield 502, got:\n{text}"
    );

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn connect_through_upstream_no_auth() {
    let upstream = spawn_noauth_upstream().await;
    let routing = Routing::Upstream(UpstreamTarget {
        addr: upstream.to_string(),
        auth: UpstreamAuth::None,
    });
    let proxy = TestProxy::spawn_with_routing(routing).await;

    let mut tunnel = proxy_connect(proxy.addr, "example.com:443").await;
    tunnel.write_all(b"no-auth-tunnel").await.unwrap();
    let mut buf = vec![0u8; b"no-auth-tunnel".len()];
    tunnel.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"no-auth-tunnel");

    drop(tunnel);
    proxy.shutdown().await.unwrap();
}
