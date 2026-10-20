//! Per-connection tracking and live byte accounting: what the UI's
//! Connections page and throughput chart are built on.

mod common;

use std::time::{Duration, Instant};

use common::client::{ReqBody, proxy_connect, proxy_http};
use common::{TestProxy, origin};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use zicade_proxy::{ConnectionKind, ConnectionSnapshot, ConnectionState, ProxyMetrics};

/// Poll `metrics` until `done` holds, failing with the last snapshot after 3s.
async fn wait_until(
    metrics: &ProxyMetrics,
    what: &str,
    done: impl Fn(&ProxyMetrics, &[ConnectionSnapshot]) -> bool,
) -> Vec<ConnectionSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let connections = metrics.connections();
        if done(metrics, &connections) {
            return connections;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; connections: {connections:#?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn tunnel_bytes_are_counted_while_the_tunnel_is_still_open() {
    // A long download must move the throughput chart as it happens, not as one
    // spike when the tunnel finally closes.
    let echo = origin::spawn_tcp_echo().await;
    let proxy = TestProxy::spawn().await;
    let metrics = proxy.metrics.clone();

    let mut tunnel = proxy_connect(proxy.addr, &echo.to_string()).await;
    let payload = vec![7u8; 1000];
    tunnel.write_all(&payload).await.unwrap();
    let mut back = vec![0u8; payload.len()];
    tunnel.read_exact(&mut back).await.unwrap();

    let connections = wait_until(&metrics, "live tunnel bytes", |m, _| {
        m.bytes_out() >= 1000 && m.bytes_in() >= 1000
    })
    .await;
    let open: Vec<_> = connections
        .iter()
        .filter(|c| c.state == ConnectionState::Open)
        .collect();
    assert_eq!(open.len(), 1, "{connections:#?}");
    let tunnel_entry = open[0];
    assert_eq!(tunnel_entry.kind, ConnectionKind::Tunnel);
    assert_eq!(tunnel_entry.target, echo.to_string());
    assert_eq!(tunnel_entry.route, "direct");
    assert!(tunnel_entry.client.is_some());
    assert_eq!(
        (tunnel_entry.bytes_up, tunnel_entry.bytes_down),
        (1000, 1000)
    );

    drop(tunnel);
    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_finished_tunnel_stays_in_the_history_as_closed() {
    let echo = origin::spawn_tcp_echo().await;
    let proxy = TestProxy::spawn().await;
    let metrics = proxy.metrics.clone();

    let mut tunnel = proxy_connect(proxy.addr, &echo.to_string()).await;
    tunnel.write_all(b"hello").await.unwrap();
    let mut back = [0u8; 5];
    tunnel.read_exact(&mut back).await.unwrap();
    drop(tunnel);

    let connections = wait_until(&metrics, "the tunnel to close", |_, c| {
        c.iter().any(|c| c.state == ConnectionState::Closed)
    })
    .await;
    let closed = &connections[0];
    assert_eq!((closed.bytes_up, closed.bytes_down), (5, 5));
    assert_eq!(metrics.total_requests(), 1, "a CONNECT is a request too");
    assert_eq!(metrics.failed_requests(), 0);

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_tunnel_whose_origin_refuses_is_recorded_as_failed() {
    let refused = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };
    let proxy = TestProxy::spawn().await;
    let metrics = proxy.metrics.clone();

    let _tunnel = proxy_connect(proxy.addr, &refused.to_string()).await;

    let connections = wait_until(&metrics, "the failed tunnel", |_, c| {
        c.iter()
            .any(|c| matches!(c.state, ConnectionState::Failed(_)))
    })
    .await;
    let ConnectionState::Failed(reason) = &connections[0].state else {
        unreachable!()
    };
    assert!(!reason.is_empty());
    assert_eq!(connections[0].target, refused.to_string());
    assert_eq!(metrics.failed_requests(), 1);

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_http_request_is_tracked_with_its_method_status_and_body_bytes() {
    let origin = origin::spawn_http_echo().await;
    let proxy = TestProxy::spawn().await;
    let metrics = proxy.metrics.clone();
    let body = vec![b'x'; 5000];

    let resp = proxy_http(
        proxy.addr,
        "POST",
        &format!("http://{origin}/echo"),
        &[],
        ReqBody::ContentLength(body.clone()),
    )
    .await;
    assert_eq!(resp.body, body);

    let connections = wait_until(&metrics, "the request to finish", |_, c| {
        c.iter().any(|c| c.state == ConnectionState::Closed)
    })
    .await;
    let request = &connections[0];
    assert_eq!(
        request.kind,
        ConnectionKind::Http {
            method: "POST".to_owned(),
            status: Some(200),
        }
    );
    assert_eq!(request.target, origin.to_string());
    assert_eq!((request.bytes_up, request.bytes_down), (5000, 5000));
    assert_eq!((metrics.bytes_out(), metrics.bytes_in()), (5000, 5000));
    assert_eq!(metrics.total_requests(), 1);

    proxy.shutdown().await.unwrap();
}
