//! M3c upstream-mode tests: route CONNECT tunnels and HTTP-forward requests
//! through a configured upstream proxy, completing a multi-leg Negotiate 407
//! handshake against an in-process fake upstream with a fake authenticator.

mod common;

use common::TestProxy;
use common::client::{ReqBody, proxy_connect, proxy_http};
use common::upstream_fake::{negotiate_factory, spawn_negotiate_upstream, spawn_noauth_upstream};
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
