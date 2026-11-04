//! Prewarmed upstream sockets for `CONNECT`: a pooled socket only ever saves
//! the dial. Each `CONNECT` still runs its own handshake, a rejection is never
//! asked twice, and a pool stops when its routing is replaced.

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Weak};
use std::time::Duration;

use common::client::{proxy_connect, proxy_connect_status};
use common::counting_upstream::{Counts, Script, spawn_counting_upstream};
use common::upstream_fake::{negotiate_factory, spawn_negotiate_upstream};
use common::{TestProxy, wait_for_active_zero};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zicade_auth::header::build_basic_authorization;
use zicade_proxy::{PoolConfig, Routing, UpstreamAuth, UpstreamPool, UpstreamTarget};

const TARGET: &str = "example.com:443";

fn pooled(
    upstream: SocketAddr,
    auth: UpstreamAuth,
    min_idle: usize,
) -> (Routing, Arc<UpstreamPool>) {
    let pool = UpstreamPool::new(
        upstream.to_string(),
        PoolConfig {
            min_idle,
            idle_timeout: Duration::from_secs(30),
        },
    );
    let target = UpstreamTarget {
        addr: upstream.to_string(),
        auth,
        pool: Some(Arc::clone(&pool)),
    };
    (Routing::Upstream(target), pool)
}

/// Poll `cond` until it holds, failing the test after a generous bound.
async fn wait_until(what: &str, cond: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Run one `CONNECT` (which starts prewarming) and wait for a full pool.
async fn warm(proxy: &TestProxy, pool: &UpstreamPool, min_idle: usize) {
    let _ = proxy_connect_status(proxy.addr, TARGET).await;
    wait_until("a full pool", || pool.idle_count() == min_idle).await;
}

async fn assert_echoes(tunnel: &mut tokio::net::TcpStream, payload: &[u8]) {
    tunnel.write_all(payload).await.unwrap();
    let mut buf = vec![0u8; payload.len()];
    tunnel.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, payload);
}

/// Let a retry, if the proxy made one, reach the fake before counting.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(300)).await;
}

#[tokio::test]
async fn every_pooled_connect_runs_its_own_full_handshake() {
    // The fake tracks handshake progress per connection, so a tunnel only
    // opens if the whole 407 exchange ran on the socket it was given.
    let upstream = spawn_negotiate_upstream().await;
    let (routing, pool) = pooled(upstream, negotiate_factory(), 1);
    let proxy = TestProxy::spawn_with_routing(routing).await;

    for i in 0..3u32 {
        wait_until("a ready socket", || i == 0 || pool.idle_count() == 1).await;
        let mut tunnel = proxy_connect(proxy.addr, TARGET).await;
        assert_echoes(&mut tunnel, format!("pooled-{i}").as_bytes()).await;
    }
    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_rejected_credential_is_sent_once_even_with_a_warm_pool() {
    let good = build_basic_authorization("svc", "right");
    let (upstream, counts) = spawn_counting_upstream(Script::Basic(good)).await;
    let wrong = UpstreamAuth::Basic {
        credentials: build_basic_authorization("svc", "wrong"),
    };
    let (routing, pool) = pooled(upstream, wrong, 1);
    let proxy = TestProxy::spawn_with_routing(routing).await;
    warm(&proxy, &pool, 1).await;

    let before = Counts::get(&counts.credentialed);
    let (status, _) = proxy_connect_status(proxy.addr, TARGET).await;
    settle().await;

    assert_eq!(status, 502);
    assert_eq!(
        Counts::get(&counts.credentialed) - before,
        1,
        "a rejected credential must not be retried: each retry counts toward account lockout"
    );
    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_refused_connect_is_asked_once_even_with_a_warm_pool() {
    let (upstream, counts) = spawn_counting_upstream(Script::Refuse).await;
    let (routing, pool) = pooled(upstream, UpstreamAuth::None, 1);
    let proxy = TestProxy::spawn_with_routing(routing).await;
    warm(&proxy, &pool, 1).await;

    let before = Counts::get(&counts.requests);
    let (status, _) = proxy_connect_status(proxy.addr, TARGET).await;
    settle().await;

    assert_eq!(status, 502);
    assert_eq!(Counts::get(&counts.requests) - before, 1);
    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_pooled_socket_that_dies_unanswered_is_retried_once_fresh() {
    // Request 1 is the warm-up; request 2 lands on the pooled socket and is
    // dropped unanswered; request 3 is the fresh retry.
    let (upstream, counts) = spawn_counting_upstream(Script::GrantButDrop(2)).await;
    let (routing, pool) = pooled(upstream, UpstreamAuth::None, 1);
    let proxy = TestProxy::spawn_with_routing(routing).await;
    warm(&proxy, &pool, 1).await;

    let mut tunnel = proxy_connect(proxy.addr, TARGET).await;
    assert_echoes(&mut tunnel, b"after-a-stale-socket").await;
    assert_eq!(Counts::get(&counts.requests), 3);
    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_fresh_socket_that_dies_unanswered_is_not_retried() {
    let (upstream, counts) = spawn_counting_upstream(Script::GrantButDrop(1)).await;
    let (routing, _pool) = pooled(upstream, UpstreamAuth::None, 1);
    let proxy = TestProxy::spawn_with_routing(routing).await;

    // The pool is still empty, so this CONNECT dials fresh.
    let (status, _) = proxy_connect_status(proxy.addr, TARGET).await;
    settle().await;

    assert_eq!(status, 502);
    assert_eq!(Counts::get(&counts.requests), 1);
    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn replacing_the_routing_stops_the_old_pool() {
    let (upstream, counts) = spawn_counting_upstream(Script::Grant).await;
    let (routing, pool) = pooled(upstream, UpstreamAuth::None, 2);
    let watched: Weak<UpstreamPool> = Arc::downgrade(&pool);
    drop(pool);
    let proxy = TestProxy::spawn_with_routing(routing).await;

    let tunnel = proxy_connect(proxy.addr, TARGET).await;
    drop(tunnel);
    let idle = || watched.upgrade().map_or(0, |pool| pool.idle_count());
    wait_until("a full pool", || idle() == 2).await;
    settle().await;
    assert_eq!(
        Counts::get(&counts.accepted),
        3,
        "one dial for the CONNECT, then prewarm stops at the watermark"
    );

    proxy.routing.set(Routing::Direct);
    wait_for_active_zero(&proxy.metrics, Duration::from_secs(2)).await;
    wait_until("the old pool to drop", || watched.upgrade().is_none()).await;
    wait_until("its sockets to close", || {
        Counts::get(&counts.closed) == Counts::get(&counts.accepted)
    })
    .await;
    settle().await;
    assert_eq!(
        Counts::get(&counts.accepted),
        3,
        "a replaced routing must not keep dialing the upstream"
    );
    proxy.shutdown().await.unwrap();
}
