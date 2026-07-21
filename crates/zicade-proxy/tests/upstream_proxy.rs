//! M3c upstream-mode tests: route CONNECT tunnels and HTTP-forward requests
//! through a configured upstream proxy, completing a multi-leg Negotiate 407
//! handshake against an in-process fake upstream with a fake authenticator.

mod common;

use std::time::Duration;

use common::client::{ReqBody, proxy_connect, proxy_http};
use common::upstream_fake::{
    basic_auth, infinite_negotiate_factory, negotiate_factory, spawn_basic_upstream,
    spawn_looping_negotiate_upstream, spawn_negotiate_upstream, spawn_noauth_upstream,
};
use common::{TestProxy, wait_for_active_zero};
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

// --- Issue #5: connection-reuse / keep-alive semantics regression tests ---
//
// NTLM/Negotiate authenticates the *TCP connection*, not the request. The
// forward path opens a fresh, independently authenticated upstream connection
// per HTTP request (see `upstream::forward_via_upstream`). These tests lock in
// that this is correct and leak-free, so any future reuse/pooling optimization
// cannot silently regress it.

/// Several sequential HTTP-forward requests through a Negotiate-407 upstream
/// must EACH complete an independent multi-leg handshake and echo their own
/// body — no auth-state bleed between requests, no spurious re-challenge
/// failure, no panic.
#[tokio::test]
async fn sequential_forward_requests_each_authenticate_independently() {
    let upstream = spawn_negotiate_upstream().await;
    let routing = Routing::Upstream(UpstreamTarget {
        addr: upstream.to_string(),
        auth: negotiate_factory(),
    });
    let proxy = TestProxy::spawn_with_routing(routing).await;

    for i in 0..8u32 {
        let body = format!("seq-forward-{i}").into_bytes();
        let resp = proxy_http(
            proxy.addr,
            "POST",
            "http://example.com/echo",
            &[],
            ReqBody::ContentLength(body.clone()),
        )
        .await;
        assert_eq!(resp.status, 200, "request {i} did not succeed");
        assert_eq!(resp.body, body, "request {i} body must echo independently");
    }

    proxy.shutdown().await.unwrap();
}

/// A misbehaving upstream that answers every authed leg with a fresh 407
/// challenge (never 200) must not hang the proxy: the leg cap (`MAX_LEGS`)
/// aborts the handshake and the client gets a prompt 502. An "infinite token"
/// authenticator is used so the cap under test is the proxy's, not the
/// authenticator running out of material.
#[tokio::test]
async fn forward_through_looping_upstream_hits_leg_cap_without_hanging() {
    let upstream = spawn_looping_negotiate_upstream().await;
    let routing = Routing::Upstream(UpstreamTarget {
        addr: upstream.to_string(),
        auth: infinite_negotiate_factory(),
    });
    let proxy = TestProxy::spawn_with_routing(routing).await;

    let resp = tokio::time::timeout(
        Duration::from_secs(10),
        proxy_http(
            proxy.addr,
            "POST",
            "http://example.com/loop",
            &[],
            ReqBody::ContentLength(b"looping".to_vec()),
        ),
    )
    .await
    .expect("proxy must not hang against a looping upstream");

    assert_eq!(
        resp.status, 502,
        "a looping upstream must surface as 502 once the leg cap trips"
    );

    proxy.shutdown().await.unwrap();
}

/// CONNECT tunnels are inherently per-connection: two sequential CONNECT
/// tunnels through the Negotiate upstream must each complete their own
/// handshake and splice a working echo tunnel.
#[tokio::test]
async fn sequential_connect_tunnels_each_authenticate_independently() {
    let upstream = spawn_negotiate_upstream().await;
    let routing = Routing::Upstream(UpstreamTarget {
        addr: upstream.to_string(),
        auth: negotiate_factory(),
    });
    let proxy = TestProxy::spawn_with_routing(routing).await;

    for i in 0..3u32 {
        let payload = format!("tunnel-round-{i}").into_bytes();
        let mut tunnel = proxy_connect(proxy.addr, "example.com:443").await;
        tunnel.write_all(&payload).await.unwrap();
        let mut buf = vec![0u8; payload.len()];
        tunnel.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, payload, "tunnel {i} must echo independently");
        drop(tunnel);
    }

    proxy.shutdown().await.unwrap();
}

/// LESSON-7 for the forward path: a burst of forward requests through the
/// authenticated upstream must drain `active_connections()` back to 0 and hold
/// a flat OS-handle count (per-request upstream connect/auth/teardown must not
/// leak). Mirrors the direct-mode soak: warm up first, then bound the delta.
#[tokio::test]
async fn forward_burst_drains_connections_and_holds_handle_count() {
    let upstream = spawn_negotiate_upstream().await;
    let routing = Routing::Upstream(UpstreamTarget {
        addr: upstream.to_string(),
        auth: negotiate_factory(),
    });
    let proxy = TestProxy::spawn_with_routing(routing).await;

    // Warm up so lazily-created OS handles are excluded from the baseline.
    for _ in 0..5 {
        let r = proxy_http(
            proxy.addr,
            "POST",
            "http://example.com/warmup",
            &[],
            ReqBody::ContentLength(b"warm".to_vec()),
        )
        .await;
        assert_eq!(r.status, 200);
    }
    wait_for_active_zero(&proxy.metrics, Duration::from_secs(2)).await;
    let baseline = zicade_win::process_handle_count();

    for i in 0..60u32 {
        let body = format!("burst-{i}").into_bytes();
        let r = proxy_http(
            proxy.addr,
            "POST",
            "http://example.com/burst",
            &[],
            ReqBody::ContentLength(body.clone()),
        )
        .await;
        assert_eq!(r.status, 200, "burst request {i} failed");
        assert_eq!(r.body, body, "burst request {i} body mismatch");
    }

    wait_for_active_zero(&proxy.metrics, Duration::from_secs(3)).await;
    assert_eq!(
        proxy.metrics.active_connections(),
        0,
        "forward-path connections must drain to zero"
    );

    if let (Some(b), Some(a)) = (baseline, zicade_win::process_handle_count()) {
        let delta = i64::from(a) - i64::from(b);
        assert!(
            delta < 40,
            "handle count grew by {delta} (baseline {b} -> {a}); suspected forward-path leak"
        );
    }

    proxy.shutdown().await.unwrap();
}
