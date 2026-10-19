//! M6 integration tests: binary wiring + lifecycle.
//!
//! Driven through the testable [`App`] entrypoint with an injected shutdown and
//! ephemeral loopback ports. No global tracing subscriber is installed here (the
//! binary installs it in `main`), and no real upstream network is touched.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use zicade::{App, build_routing, noop_log_reload};
use zicade_config::{
    AuthConfig, AuthMode, Config, ListenConfig, RoutingConfig, RoutingMode, UpstreamConfig,
};
use zicade_observe::{LogStore, channel_layer};
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

/// A loopback port that was free a moment ago.
async fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind :0");
    l.local_addr().expect("local_addr").port()
}

/// Start a direct-mode [`App`] on a free loopback port, returning the app and
/// its chosen proxy port.
///
/// [`free_port`] can only *probe* a free port then release it; between that
/// release and [`App::start`] rebinding it, a parallel test can grab the port
/// (ephemeral `:0` allocation favors just-freed ports), surfacing as a bind
/// error. That race is benign, so we simply retry on a fresh port rather than
/// letting the suite flake.
async fn start_direct_app(logs: LogStore) -> (App, u16) {
    for _ in 0..50 {
        let port = free_port().await;
        match App::start(
            direct_config(port),
            temp_config_path(),
            logs.clone(),
            noop_log_reload(),
        )
        .await
        {
            Ok(app) => return (app, port),
            // Port taken between probe and bind: try another.
            Err(_) => continue,
        }
    }
    panic!("could not start the app on a free loopback port after 50 attempts");
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
    let (app, port) = start_direct_app(logs).await;

    let proxy_addr = app.proxy_addr();
    assert_eq!(proxy_addr.port(), port, "proxy bound to requested port");

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
async fn control_reports_the_running_proxy() {
    // The UI reads everything through this handle; it must describe the proxy
    // that is actually bound, not the defaults.
    let (_layer, logs) = channel_layer(64);
    let (app, port) = start_direct_app(logs).await;

    let status = app.control().status();

    assert_eq!(status.listen_addr, format!("127.0.0.1:{port}"));
    assert_eq!(status.routing_mode, "direct");
    assert_eq!(status.requests_total, 0);
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
            corp_network: None,
        },
        ..Default::default()
    };
    let res = tokio::time::timeout(
        Duration::from_secs(2),
        App::start(cfg, temp_config_path(), logs, noop_log_reload()),
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
        App::start(
            direct_config(taken),
            temp_config_path(),
            logs,
            noop_log_reload(),
        ),
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
            corp_network: None,
        },
        ..Default::default()
    };
    let upstream = build_routing(&cfg).expect("upstream routing builds");
    assert!(matches!(upstream, Routing::Upstream(_)), "got {upstream:?}");
}
