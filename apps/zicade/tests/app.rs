//! M6 integration tests: binary wiring + lifecycle.
//!
//! Driven through the testable [`App`] entrypoint with an injected shutdown and
//! ephemeral loopback ports. No global tracing subscriber is installed here (the
//! binary installs it in `main`), and no real upstream network is touched.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use zicade::{App, build_routing};
use zicade_config::{
    AuthConfig, AuthMode, Config, ListenConfig, RoutingConfig, RoutingMode, UpstreamConfig,
};
use zicade_observe::channel_layer;
use zicade_proxy::Routing;

/// A unique temp config path per call (no external tempfile crate).
fn temp_config_path() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zicade-app-test-{nanos}-{n}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("config.json")
}

fn direct_config(port: u16) -> Config {
    Config {
        listen: ListenConfig {
            host: "127.0.0.1".to_owned(),
            port,
        },
        ..Default::default()
    }
}

/// Find a free loopback port whose successor (`port + 1`, used by the web
/// server) is also currently free, minimizing collisions in the wiring test.
async fn free_port_pair() -> u16 {
    loop {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind :0");
        let p = l.local_addr().expect("local_addr").port();
        drop(l);
        if p < u16::MAX && TcpListener::bind(("127.0.0.1", p + 1)).await.is_ok() {
            return p;
        }
    }
}

async fn wait_connectable(addr: SocketAddr) {
    for _ in 0..100 {
        if TcpStream::connect(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server never became connectable at {addr}");
}

#[tokio::test]
async fn starts_and_shuts_down_gracefully() {
    let (_layer, logs) = channel_layer(64);
    let port = free_port_pair().await;
    let app = App::start(direct_config(port), temp_config_path(), logs)
        .await
        .expect("app should start in direct mode");

    let proxy_addr = app.proxy_addr();
    let web_addr = app.web_addr();
    assert_eq!(proxy_addr.port(), port, "proxy bound to requested port");
    assert_eq!(web_addr.port(), port + 1, "web bound to proxy port + 1");

    let (tx, rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(app.run(async move {
        let _ = rx.await;
    }));

    // Confirm the proxy is actually serving before triggering shutdown.
    wait_connectable(proxy_addr).await;

    tx.send(()).expect("send shutdown");
    let joined = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("run must return within timeout (no hang)")
        .expect("run task must not panic");
    assert!(joined.is_ok(), "run returned an error: {joined:?}");
}

#[tokio::test]
async fn shuts_down_even_with_an_open_sse_stream() {
    // Regression: a browser tab holding an `/events/metrics` SSE connection open
    // used to stall axum's graceful shutdown forever, so the tray "Close" left a
    // zombie process. The shutdown signal now ends the SSE stream, so `run`
    // returns even while a client is still connected.
    let (_layer, logs) = channel_layer(64);
    let port = free_port_pair().await;
    let app = App::start(direct_config(port), temp_config_path(), logs)
        .await
        .expect("app should start in direct mode");
    let web_addr = app.web_addr();

    let (tx, rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(app.run(async move {
        let _ = rx.await;
    }));

    // Open a long-lived SSE connection (as a browser EventSource would) and read
    // the first streamed bytes, so it is registered as an in-flight connection.
    let mut sse = TcpStream::connect(web_addr).await.expect("connect web");
    sse.write_all(
        b"GET /events/metrics HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n",
    )
    .await
    .expect("write SSE request");
    let mut buf = [0u8; 256];
    let n = tokio::time::timeout(Duration::from_secs(3), sse.read(&mut buf))
        .await
        .expect("first SSE bytes within 3s")
        .expect("read the SSE response");
    assert!(n > 0, "SSE stream should send its initial response bytes");

    // Trigger shutdown while the SSE connection is still open.
    tx.send(()).expect("send shutdown");
    let joined = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("run must return even with an open SSE stream (no zombie)")
        .expect("run task must not panic");
    assert!(joined.is_ok(), "run returned an error: {joined:?}");

    drop(sse);
}

#[tokio::test]
async fn invalid_config_fails_fast() {
    let (_layer, logs) = channel_layer(64);
    // Upstream mode with no `[routing.upstream]` section fails validation.
    let cfg = Config {
        routing: RoutingConfig {
            mode: RoutingMode::Upstream,
            upstream: None,
            pac: None,
        },
        ..Default::default()
    };
    let res = tokio::time::timeout(
        Duration::from_secs(2),
        App::start(cfg, temp_config_path(), logs),
    )
    .await
    .expect("start must return promptly, not hang");
    assert!(res.is_err(), "invalid config must fail fast with an error");
}

#[tokio::test]
async fn port_in_use_fails_fast() {
    let (_layer, logs) = channel_layer(64);
    // Occupy a port, then point the proxy at it.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind :0");
    let taken = listener.local_addr().expect("local_addr").port();

    let res = tokio::time::timeout(
        Duration::from_secs(2),
        App::start(direct_config(taken), temp_config_path(), logs),
    )
    .await
    .expect("start must return promptly, not hang");
    assert!(
        res.is_err(),
        "binding an already-taken proxy port must fail fast"
    );
    drop(listener);
}

#[test]
fn build_routing_maps_direct_and_upstream() {
    // Direct mode -> Routing::Direct.
    let direct = build_routing(&direct_config(3129)).expect("direct routing builds");
    assert!(matches!(direct, Routing::Direct), "got {direct:?}");

    // Upstream mode with a plain (no-auth) upstream -> Routing::Upstream.
    let cfg = Config {
        routing: RoutingConfig {
            mode: RoutingMode::Upstream,
            upstream: Some(UpstreamConfig {
                host: "wp8080".to_owned(),
                port: 8080,
                auth: AuthConfig {
                    mode: AuthMode::None,
                    ..Default::default()
                },
            }),
            pac: None,
        },
        ..Default::default()
    };
    let upstream = build_routing(&cfg).expect("upstream routing builds");
    assert!(matches!(upstream, Routing::Upstream(_)), "got {upstream:?}");
}
