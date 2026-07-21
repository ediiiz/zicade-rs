//! M2 lifecycle tests encoding the §8 lessons: survive many connections with
//! clean bookkeeping (LESSON-1), no handle leak under load (LESSON-7), graceful
//! shutdown with no hang/panic (LESSON-2), and typed startup failure (LESSON-5).

mod common;

use std::time::{Duration, Instant};

use common::client::{ReqBody, proxy_connect, proxy_http};
use common::{TestProxy, origin, wait_for_active_zero};

#[tokio::test]
async fn survives_many_sequential_connections_and_cleans_up() {
    // LESSON-1: the prior build died after 1-2 connections and leaked tracking.
    let origin = origin::spawn_http_echo().await;
    let proxy = TestProxy::spawn().await;
    let url = format!("http://{origin}/seq");

    for i in 0..80u32 {
        let body = format!("seq-{i}").into_bytes();
        let r = proxy_http(
            proxy.addr,
            "POST",
            &url,
            &[],
            ReqBody::ContentLength(body.clone()),
        )
        .await;
        assert_eq!(r.status, 200, "request {i} failed");
        assert_eq!(r.body, body, "request {i} body mismatch");
    }

    wait_for_active_zero(&proxy.metrics, Duration::from_secs(2)).await;
    assert_eq!(
        proxy.metrics.active_connections(),
        0,
        "connections not cleaned up"
    );
    assert!(
        proxy.metrics.total_requests() >= 80,
        "request counter wrong"
    );

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn no_handle_leak_under_load() {
    // LESSON-7: prior build leaked OS handles (~171 -> 231 over 60 requests).
    let origin = origin::spawn_http_echo().await;
    let proxy = TestProxy::spawn().await;
    let url = format!("http://{origin}/leak");

    for _ in 0..5 {
        let _ = proxy_http(proxy.addr, "GET", &url, &[], ReqBody::None).await;
    }
    wait_for_active_zero(&proxy.metrics, Duration::from_secs(2)).await;
    let baseline = zicade_win::process_handle_count();

    for _ in 0..60 {
        let body = vec![b'x'; 1024];
        let r = proxy_http(proxy.addr, "POST", &url, &[], ReqBody::ContentLength(body)).await;
        assert_eq!(r.status, 200);
    }

    wait_for_active_zero(&proxy.metrics, Duration::from_secs(3)).await;
    assert_eq!(
        proxy.metrics.active_connections(),
        0,
        "connections must be reaped"
    );

    if let (Some(b), Some(a)) = (baseline, zicade_win::process_handle_count()) {
        let delta = i64::from(a) - i64::from(b);
        assert!(
            delta < 40,
            "handle count grew by {delta} (baseline {b} -> {a}); suspected leak"
        );
    }

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn graceful_shutdown_with_inflight_connection() {
    // LESSON-2: shutdown must not hang or panic even with a live tunnel.
    let echo = origin::spawn_tcp_echo().await;
    let proxy = TestProxy::spawn_with_timeout(Duration::from_millis(500)).await;

    let _tunnel = proxy_connect(proxy.addr, &echo.to_string()).await;

    let start = Instant::now();
    proxy.shutdown().await.unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "shutdown hung with in-flight conn: {elapsed:?}"
    );
}

#[tokio::test]
async fn shutdown_while_idle_is_prompt() {
    // LESSON-2: cancelling accept mid-loop must not panic and should be prompt.
    let proxy = TestProxy::spawn().await;
    let start = Instant::now();
    proxy.shutdown().await.unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "idle shutdown was slow"
    );
}

#[tokio::test]
async fn bind_failure_on_taken_port_is_typed_error() {
    // LESSON-5: binding an already-taken port fails fast with a typed error.
    use zicade_proxy::{ProxyError, ProxyServer};

    let first = ProxyServer::bind("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = first.local_addr();

    let err = ProxyServer::bind(addr)
        .await
        .expect_err("second bind must fail");
    assert!(matches!(err, ProxyError::Bind { .. }), "got {err:?}");
}
