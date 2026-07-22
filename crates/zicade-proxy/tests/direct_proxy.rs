//! M2 direct-mode data-path tests: HTTP forward, CONNECT tunnel, POST body
//! integrity (LESSON-4), and concurrency.

mod common;

use common::client::{ReqBody, proxy_connect, proxy_http};
use common::{TestProxy, origin};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn http_get_round_trip() {
    let origin = origin::spawn_http_echo().await;
    let proxy = TestProxy::spawn().await;
    let url = format!("http://{origin}/hello");

    let resp = proxy_http(proxy.addr, "GET", &url, &[], ReqBody::None).await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("x-origin-echo"), Some("1"));

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn post_body_integrity_small_large_chunked() {
    let origin = origin::spawn_http_echo().await;
    let proxy = TestProxy::spawn().await;
    let url = format!("http://{origin}/echo");

    let small = b"hello world".to_vec();
    let r = proxy_http(
        proxy.addr,
        "POST",
        &url,
        &[],
        ReqBody::ContentLength(small.clone()),
    )
    .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.body, small,
        "small content-length body must round-trip byte-identical"
    );

    let large: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
    let r = proxy_http(
        proxy.addr,
        "POST",
        &url,
        &[],
        ReqBody::ContentLength(large.clone()),
    )
    .await;
    assert_eq!(r.body.len(), large.len());
    assert_eq!(
        r.body, large,
        "large content-length body must round-trip byte-identical"
    );

    let chunked: Vec<u8> = (0..100 * 1024).map(|i| ((i * 7) % 253) as u8).collect();
    let r = proxy_http(
        proxy.addr,
        "POST",
        &url,
        &[],
        ReqBody::Chunked(chunked.clone()),
    )
    .await;
    assert_eq!(
        r.body, chunked,
        "chunked body must round-trip byte-identical"
    );

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn connect_tunnel_round_trip() {
    let echo = origin::spawn_tcp_echo().await;
    let proxy = TestProxy::spawn().await;
    let metrics = proxy.metrics.clone();

    let payload = b"ping-through-tunnel";
    let mut tunnel = proxy_connect(proxy.addr, &echo.to_string()).await;
    tunnel.write_all(payload).await.unwrap();
    let mut buf = vec![0u8; payload.len()];
    tunnel.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, payload);

    // Closing the client ends the tunnel; the (detached) splice task tallies the
    // bytes on close, so poll briefly until the counters settle.
    drop(tunnel);
    let n = payload.len() as u64;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while (metrics.bytes_out() < n || metrics.bytes_in() < n)
        && std::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        metrics.bytes_out(),
        n,
        "client→origin bytes must be counted"
    );
    assert_eq!(metrics.bytes_in(), n, "origin→client bytes must be counted");

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn many_concurrent_connections_all_succeed() {
    let origin = origin::spawn_http_echo().await;
    let proxy = TestProxy::spawn().await;
    let url = format!("http://{origin}/c");

    let mut handles = Vec::new();
    for i in 0..50u32 {
        let addr = proxy.addr;
        let url = url.clone();
        handles.push(tokio::spawn(async move {
            let body = format!("payload-{i}").into_bytes();
            let r = proxy_http(
                addr,
                "POST",
                &url,
                &[],
                ReqBody::ContentLength(body.clone()),
            )
            .await;
            assert_eq!(r.status, 200);
            assert_eq!(r.body, body, "concurrent request {i} body mismatch");
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    proxy.shutdown().await.unwrap();
}
