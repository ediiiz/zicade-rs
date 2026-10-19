//! Response framing on the plain-HTTP forward path through an upstream proxy:
//! the final response must stream to the client as it arrives, whatever its
//! framing, without trusting the upstream's `Content-Length` for allocation.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::TestProxy;
use common::client::{ReqBody, proxy_http};
use common::upstream_fake::spawn_scripted_upstream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use zicade_proxy::{Routing, UpstreamAuth, UpstreamTarget};

async fn proxy_via(upstream: std::net::SocketAddr) -> TestProxy {
    TestProxy::spawn_with_routing(Routing::Upstream(UpstreamTarget {
        addr: upstream.to_string(),
        auth: UpstreamAuth::None,
    }))
    .await
}

fn released() -> Arc<Notify> {
    let release = Arc::new(Notify::new());
    release.notify_one();
    release
}

/// A response with neither `Content-Length` nor chunked framing is delimited by
/// the upstream closing the connection; its body must not be dropped.
#[tokio::test]
async fn unframed_response_body_is_read_until_close() {
    let (upstream, head) = spawn_scripted_upstream(
        b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nbody-until-close".to_vec(),
        Vec::new(),
        released(),
    )
    .await;
    let proxy = proxy_via(upstream).await;

    let resp = proxy_http(proxy.addr, "GET", "http://example.com/", &[], ReqBody::None).await;

    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"body-until-close");
    let head = head.await.unwrap();
    assert!(
        head.starts_with("GET http://example.com/ HTTP/1.1\r\n"),
        "upstream must receive an absolute-form request line, got {head:?}"
    );
    proxy.shutdown().await.unwrap();
}

/// The proxy must not size an allocation from the upstream's `Content-Length`:
/// a bogus multi-petabyte length used to abort the whole process.
#[tokio::test]
async fn huge_declared_content_length_does_not_abort() {
    let (upstream, _) = spawn_scripted_upstream(
        b"HTTP/1.1 200 OK\r\nContent-Length: 1125899906842624\r\n\r\npartial".to_vec(),
        Vec::new(),
        released(),
    )
    .await;
    let proxy = proxy_via(upstream).await;

    let mut client = TcpStream::connect(proxy.addr).await.unwrap();
    client
        .write_all(b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await
        .unwrap();
    let mut seen = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut seen)).await;

    // Whatever the client saw, the proxy is still alive and serving.
    drop(client);
    proxy.shutdown().await.unwrap();
}

/// The first bytes of a response reach the client before the upstream has sent
/// the rest: the final leg streams instead of buffering the whole body.
#[tokio::test]
async fn final_response_streams_before_upstream_finishes() {
    let release = Arc::new(Notify::new());
    let (upstream, _) = spawn_scripted_upstream(
        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nfirst".to_vec(),
        b"-rest".to_vec(),
        release.clone(),
    )
    .await;
    let proxy = proxy_via(upstream).await;

    let mut client = TcpStream::connect(proxy.addr).await.unwrap();
    client
        .write_all(
            b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();

    let mut seen = Vec::new();
    let mut buf = [0u8; 256];
    let streamed = tokio::time::timeout(Duration::from_secs(5), async {
        while !seen.windows(5).any(|w| w == b"first") {
            let n = client.read(&mut buf).await.unwrap();
            assert_ne!(n, 0, "proxy closed before streaming the first bytes");
            seen.extend_from_slice(&buf[..n]);
        }
    })
    .await;
    assert!(
        streamed.is_ok(),
        "first body bytes were held back until the upstream finished"
    );

    release.notify_one();
    client.read_to_end(&mut seen).await.unwrap();
    assert!(seen.ends_with(b"first-rest"));
    proxy.shutdown().await.unwrap();
}
