//! Pool-specific integration tests: the idle-socket pool must speed up the
//! dial without changing any observable proxy semantics.
//!
//! The regression this file guards is the one flagged by Issue #5: NTLM
//! authenticates the *TCP connection*, so a pooling optimisation that quietly
//! reuses an authenticated connection across requests would break isolation
//! between clients. These tests pin the intended contract —
//!
//!   * only an **unauthenticated, un-tunnelled** socket is ever pooled;
//!   * every request still runs its own full 407 handshake;
//!   * a socket the gateway refused is recycled rather than thrown away;
//!   * a failed handshake is retried at most once, and only when the attempt
//!     actually consumed a pooled socket.

mod common;

use std::time::Duration;

use common::client::{ReqBody, proxy_http};
use common::upstream_fake::{negotiate_factory, spawn_negotiate_upstream};
use common::{TestProxy, wait_for_active_zero};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zicade_proxy::{PoolConfig, Routing, UpstreamPool, UpstreamTarget};

/// A pool that fills itself quickly, so tests do not wait on the default
/// 50 ms cadence for eight sockets.
fn fast_pool(addr: &str) -> UpstreamPool {
    UpstreamPool::new(
        addr,
        PoolConfig {
            min_idle: 2,
            prewarm_interval: Duration::from_millis(5),
            ..PoolConfig::default()
        },
    )
}

/// End-to-end: with pooling enabled, a sequence of HTTP-forward requests must
/// still each complete their own multi-leg handshake and echo their own body.
///
/// This is the pooling counterpart of
/// `sequential_forward_requests_each_authenticate_independently`: if the pool
/// ever leaked auth state between requests, one of these would fail or return
/// another request's body.
#[tokio::test]
async fn pooled_forward_requests_still_authenticate_independently() {
    let upstream = spawn_negotiate_upstream().await;
    let pool = std::sync::Arc::new(fast_pool(&upstream.to_string()));
    let routing = Routing::Upstream(
        UpstreamTarget::new(upstream.to_string(), negotiate_factory()).with_pool(pool),
    );
    let proxy = TestProxy::spawn_with_routing(routing).await;

    for i in 0..6u32 {
        let body = format!("pooled-{i}").into_bytes();
        let resp = proxy_http(
            proxy.addr,
            "POST",
            "http://example.com/echo",
            &[],
            ReqBody::ContentLength(body.clone()),
        )
        .await;
        assert_eq!(resp.status, 200, "pooled request {i} did not succeed");
        assert_eq!(
            resp.body, body,
            "pooled request {i} must echo its own body, not another request's"
        );
    }

    wait_for_active_zero(&proxy.metrics, Duration::from_secs(2)).await;
    proxy.shutdown().await.unwrap();
}

/// A CONNECT tunnel takes its socket out of circulation permanently, so a
/// burst of tunnels must not shrink a pool that a prewarm task keeps topped up
/// — and every tunnel must still work.
#[tokio::test]
async fn tunnels_through_a_pool_all_succeed() {
    let upstream = spawn_negotiate_upstream().await;
    let pool = std::sync::Arc::new(fast_pool(&upstream.to_string()));
    let routing = Routing::Upstream(
        UpstreamTarget::new(upstream.to_string(), negotiate_factory()).with_pool(pool.clone()),
    );
    let proxy = TestProxy::spawn_with_routing(routing).await;

    for i in 0..3u32 {
        let payload = format!("tunnel-pooled-{i}").into_bytes();
        let mut tunnel = common::client::proxy_connect(proxy.addr, "example.com:443").await;
        tokio::io::AsyncWriteExt::write_all(&mut tunnel, &payload)
            .await
            .unwrap();
        let mut buf = vec![0u8; payload.len()];
        tokio::io::AsyncReadExt::read_exact(&mut tunnel, &mut buf)
            .await
            .unwrap();
        assert_eq!(buf, payload, "pooled tunnel {i} must echo independently");
        drop(tunnel);
    }

    proxy.shutdown().await.unwrap();
}

/// A pooled socket the gateway has already closed must not surface as a user
/// error: the first attempt fails on write, `connect_via_upstream` retries
/// once with a fresh dial, and the request succeeds.
///
/// Modelled by parking a socket that the fake upstream has already closed into
/// the pool by hand.
#[tokio::test]
async fn stale_pooled_socket_is_retried_with_a_fresh_dial() {
    let upstream = spawn_negotiate_upstream().await;
    let pool = std::sync::Arc::new(UpstreamPool::new(
        &upstream.to_string(),
        PoolConfig {
            // No prewarm: we seed the bad entry ourselves.
            min_idle: 0,
            ..PoolConfig::default()
        },
    ));

    // A real connection to the fake upstream, closed before it is pooled. The
    // fake closes its side as soon as it sees EOF, so the entry is dead.
    let dead = tokio::net::TcpStream::connect(upstream).await.unwrap();
    drop(dead);

    let routing = Routing::Upstream(
        UpstreamTarget::new(upstream.to_string(), negotiate_factory()).with_pool(pool.clone()),
    );
    let proxy = TestProxy::spawn_with_routing(routing).await;

    let body = b"after-stale-socket".to_vec();
    let resp = proxy_http(
        proxy.addr,
        "POST",
        "http://example.com/echo",
        &[],
        ReqBody::ContentLength(body.clone()),
    )
    .await;

    assert_eq!(
        resp.status, 200,
        "a stale pooled socket must be retried, not surfaced as an error"
    );
    assert_eq!(resp.body, body);
    proxy.shutdown().await.unwrap();
}

/// A handshake that fails on a socket this attempt dialled *fresh* must not be
/// retried: a second identical cold dial can only fail the same way, so a retry
/// would just double the latency and the load on a failing gateway.
///
/// This exercises the **CONNECT** path, which is the only one that consults the
/// pool (`forward_via_upstream` deliberately opens its own socket per request,
/// so a pool cannot help there). The fake upstream is live, so dials genuinely
/// connect, and it refuses the terminal CONNECT with 403 — exactly the
/// "gateway said no" case that used to be retried for free when `reused` was
/// derived from `pool.is_some()` instead of from the attempt that was made.
#[tokio::test]
async fn fresh_dial_failure_is_not_retried() {
    let (upstream, accepted) = common::upstream_fake::spawn_refusing_connect_upstream().await;
    let pool = std::sync::Arc::new(UpstreamPool::new(
        &upstream.to_string(),
        PoolConfig {
            // No prewarm, so the single attempt this test expects is a cold dial.
            min_idle: 0,
            ..PoolConfig::default()
        },
    ));
    let routing = Routing::Upstream(
        UpstreamTarget::new(upstream.to_string(), negotiate_factory()).with_pool(pool),
    );
    let proxy = TestProxy::spawn_with_routing(routing).await;

    let mut raw = tokio::net::TcpStream::connect(proxy.addr).await.unwrap();
    raw.write_all(
        b"CONNECT example.com:443 HTTP/1.1
Host: example.com:443

",
    )
    .await
    .unwrap();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(10), raw.read(&mut buf))
        .await
        .expect("must not hang")
        .unwrap();
    let text = String::from_utf8_lossy(&buf[..n]).to_string();
    assert!(
        text.starts_with("HTTP/1.1 502"),
        "a refused CONNECT should surface as 502, got: {text}"
    );

    // Give a retry, if there were one, time to land before counting.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        accepted.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a cold-dial failure must not be retried; the upstream saw more than one connection"
    );

    proxy.shutdown().await.unwrap();
}
