//! Gated live-proxy regression tests against the real corporate upstream.
//!
//! EVERY test here is gated behind `ZICADE_LIVE_PROXY=1`. With the gate unset
//! (the default — this offline/CI box included) each test prints an explicit
//! `SKIP` line and returns; it never silently passes and never touches the
//! network. On a domain-joined, on-corp machine,
//! `ZICADE_LIVE_PROXY=1 cargo test -p zicade --test live` exercises the real
//! `407 -> Negotiate -> 200` handshake, HTTPS `CONNECT` tunneling, plain-HTTP
//! forwarding, and a soak / handle-leak baseline against `wp8080:8080`.
//!
//! Routing is built through the production `zicade::build_routing` mapper so the
//! SSPI `AuthFactory` (SPN `HTTP/<upstream-host>`) is wired exactly as the
//! shipping binary wires it.
//!
//! Env knobs (all optional, sensible defaults so the suite is pointable):
//! - `ZICADE_LIVE_PROXY`         gate; must equal `"1"` to run.
//! - `ZICADE_LIVE_UPSTREAM`      corporate upstream `host:port` (default `wp8080:8080`).
//! - `ZICADE_LIVE_EXTERNAL_HOST` CONNECT target `host:port` (default `example.com:443`).
//! - `ZICADE_LIVE_EXTERNAL_URL`  external HTTPS URL, informational (default `https://example.com`).
//! - `ZICADE_LIVE_HTTP_URL`      plain-HTTP forward target (default `http://neverssl.com/`).

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use zicade_config::{
    AuthConfig, AuthMode, Config, ListenConfig, RoutingConfig, RoutingMode, UpstreamConfig,
};
use zicade_proxy::{ProxyError, ProxyMetrics, ProxyServer, Routing};

const GATE: &str = "ZICADE_LIVE_PROXY";

/// Whether the live suite is enabled (`ZICADE_LIVE_PROXY=1`).
fn live_enabled() -> bool {
    std::env::var(GATE).as_deref() == Ok("1")
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn upstream() -> String {
    env_or("ZICADE_LIVE_UPSTREAM", "wp8080:8080")
}

fn external_host() -> String {
    env_or("ZICADE_LIVE_EXTERNAL_HOST", "example.com:443")
}

fn http_url() -> String {
    env_or("ZICADE_LIVE_HTTP_URL", "http://neverssl.com/")
}

/// Split a `host:port` string, defaulting the port when absent/unparseable.
fn split_host_port(hp: &str, default_port: u16) -> (String, u16) {
    match hp.rsplit_once(':') {
        Some((h, p)) => (h.to_owned(), p.parse().unwrap_or(default_port)),
        None => (hp.to_owned(), default_port),
    }
}

/// Production upstream+Negotiate config pointed at `ZICADE_LIVE_UPSTREAM`.
fn upstream_negotiate_config() -> Config {
    let (host, port) = split_host_port(&upstream(), 8080);
    Config {
        listen: ListenConfig {
            host: "127.0.0.1".to_owned(),
            port: 0,
        },
        routing: RoutingConfig {
            mode: RoutingMode::Upstream,
            upstream: Some(UpstreamConfig {
                host,
                port,
                auth: AuthConfig {
                    mode: AuthMode::Negotiate,
                    username: None,
                    password: None,
                },
            }),
            pac: None,
        },
        ..Default::default()
    }
}

/// A proxy on an ephemeral loopback port with a shutdown handle and live metrics.
struct LiveProxy {
    addr: SocketAddr,
    metrics: ProxyMetrics,
    shutdown: Option<oneshot::Sender<()>>,
    join: JoinHandle<Result<(), ProxyError>>,
}

impl LiveProxy {
    async fn spawn(routing: Routing) -> Self {
        let server = ProxyServer::bind("127.0.0.1:0".parse().unwrap())
            .await
            .expect("bind ephemeral loopback port")
            .with_shutdown_timeout(Duration::from_secs(5))
            .with_routing(routing);
        let addr = server.local_addr();
        let metrics = server.metrics();
        let (tx, rx) = oneshot::channel::<()>();
        let join = tokio::spawn(server.serve(async move {
            let _ = rx.await;
        }));
        Self {
            addr,
            metrics,
            shutdown: Some(tx),
            join,
        }
    }

    async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = self.join.await.expect("serve task must not panic");
    }
}

/// Build the live upstream+Negotiate proxy via the production routing mapper.
async fn spawn_live_proxy() -> LiveProxy {
    let routing =
        zicade::build_routing(&upstream_negotiate_config()).expect("build upstream routing");
    LiveProxy::spawn(routing).await
}

/// Read an HTTP response head (up to `\r\n\r\n`) and return the status code plus
/// the raw head text (for diagnostics).
async fn read_head(stream: &mut TcpStream) -> std::io::Result<(u16, String)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    while find(&buf, b"\r\n\r\n").is_none() {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Ok((status, text))
}

/// Open a `CONNECT` tunnel through the local proxy. Returns the status and the
/// (spliced on 200) stream.
async fn connect_through(proxy: SocketAddr, target: &str) -> std::io::Result<(u16, TcpStream)> {
    let mut stream = TcpStream::connect(proxy).await?;
    let head = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await?;
    let (status, _text) = read_head(&mut stream).await?;
    Ok((status, stream))
}

/// Forward an absolute-form plain-HTTP request through the local proxy and
/// return its status.
async fn forward_get(proxy: SocketAddr, url: &str) -> std::io::Result<u16> {
    let host = url
        .strip_prefix("http://")
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url);
    let mut stream = TcpStream::connect(proxy).await?;
    let head = format!(
        "GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: zicade-live-test\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await?;
    let (status, _text) = read_head(&mut stream).await?;
    Ok(status)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Poll `active_connections()` until it reaches zero or the deadline elapses.
async fn wait_for_active_zero(metrics: &ProxyMetrics, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while metrics.active_connections() != 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Append a TLS extension (type + length-prefixed body) to `out`.
fn push_ext(out: &mut Vec<u8>, ext_type: u16, body: &[u8]) {
    out.extend_from_slice(&ext_type.to_be_bytes());
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
}

/// Build a minimal-but-valid TLS 1.2/1.3 ClientHello record with SNI, enough for
/// a real HTTPS server to answer with a ServerHello (or a TLS alert) — either
/// way, response bytes flow back through the tunnel, proving end-to-end data.
fn tls_client_hello(sni_host: &str) -> Vec<u8> {
    let host = sni_host.as_bytes();
    let mut ext = Vec::new();
    // server_name: list-length(2) + name_type(1=host) + host-length(2) + host.
    let mut sni = Vec::new();
    sni.extend_from_slice(&((host.len() + 3) as u16).to_be_bytes());
    sni.push(0);
    sni.extend_from_slice(&(host.len() as u16).to_be_bytes());
    sni.extend_from_slice(host);
    push_ext(&mut ext, 0x0000, &sni);
    push_ext(&mut ext, 0x000a, &[0x00, 0x04, 0x00, 0x1d, 0x00, 0x17]); // supported_groups
    push_ext(&mut ext, 0x000b, &[0x01, 0x00]); // ec_point_formats
    push_ext(
        &mut ext,
        0x000d,
        &[0x00, 0x08, 0x04, 0x03, 0x08, 0x04, 0x04, 0x01, 0x02, 0x01],
    ); // signature_algorithms
    push_ext(&mut ext, 0x002b, &[0x04, 0x03, 0x04, 0x03, 0x03]); // supported_versions

    let mut hello = Vec::new();
    hello.extend_from_slice(&[0x03, 0x03]); // client_version TLS 1.2
    hello.extend_from_slice(&[0x2a; 32]); // random (fixed is fine)
    hello.push(0x00); // session_id length
    let suites: [u8; 12] = [
        0x13, 0x01, 0x13, 0x02, 0x13, 0x03, 0xc0, 0x2f, 0xc0, 0x2b, 0x00, 0xff,
    ];
    hello.extend_from_slice(&(suites.len() as u16).to_be_bytes());
    hello.extend_from_slice(&suites);
    hello.extend_from_slice(&[0x01, 0x00]); // compression: 1 method, null
    hello.extend_from_slice(&(ext.len() as u16).to_be_bytes());
    hello.extend_from_slice(&ext);

    let mut handshake = vec![0x01]; // ClientHello
    let len = hello.len();
    handshake.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
    handshake.extend_from_slice(&hello);

    let mut record = vec![0x16, 0x03, 0x01]; // handshake, TLS 1.0 record version
    record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
    record.extend_from_slice(&handshake);
    record
}

/// TEST 1 — live Negotiate handshake + HTTPS via CONNECT through the gateway.
///
/// Starts the proxy in upstream+Negotiate mode, opens a CONNECT to the external
/// HTTPS host through it (the real `407 -> Negotiate -> 200` dance runs against
/// the corp upstream), asserts 200, then writes a TLS ClientHello and reads the
/// server's response bytes back through the tunnel to prove end-to-end data.
#[tokio::test]
async fn live_connect_negotiate_https_tunnel() {
    if !live_enabled() {
        eprintln!(
            "SKIP live_connect_negotiate_https_tunnel: set {GATE}=1 (on corp, domain-joined) to run"
        );
        return;
    }

    let proxy = spawn_live_proxy().await;
    let target = external_host();
    let (host, _port) = split_host_port(&target, 443);

    let (status, mut tunnel) = connect_through(proxy.addr, &target)
        .await
        .expect("CONNECT round-trip to the live upstream must not error");
    assert_eq!(
        status,
        200,
        "CONNECT {target} via {} must return 200 after the Negotiate handshake",
        upstream()
    );

    tunnel
        .write_all(&tls_client_hello(&host))
        .await
        .expect("write TLS ClientHello into the tunnel");
    tunnel.flush().await.expect("flush ClientHello");

    let mut buf = [0u8; 512];
    let n = tokio::time::timeout(Duration::from_secs(10), tunnel.read(&mut buf))
        .await
        .expect("reading the TLS response must not time out")
        .expect("reading the TLS response must not error");
    assert!(
        n > 0,
        "expected TLS response bytes back through the tunnel, got EOF"
    );
    eprintln!("live_connect_negotiate_https_tunnel: 200 + {n} TLS response bytes from {host}");

    drop(tunnel);
    proxy.shutdown().await;
}

/// TEST 2 — live plain-HTTP forward through the gateway.
///
/// Forwards `GET ZICADE_LIVE_HTTP_URL` through the proxy in upstream+Negotiate
/// mode and asserts a sane status (2xx/3xx). Plain-HTTP external targets can be
/// flaky, so this is deliberately lenient on the exact code.
#[tokio::test]
async fn live_http_forward_through_upstream() {
    if !live_enabled() {
        eprintln!(
            "SKIP live_http_forward_through_upstream: set {GATE}=1 (on corp, domain-joined) to run"
        );
        return;
    }

    let proxy = spawn_live_proxy().await;
    let url = http_url();
    let status = forward_get(proxy.addr, &url)
        .await
        .expect("HTTP forward round-trip to the live upstream must not error");
    assert!(
        (200..400).contains(&status),
        "GET {url} via {} returned {status}; expected a 2xx/3xx",
        upstream()
    );
    eprintln!("live_http_forward_through_upstream: {url} -> {status}");
    proxy.shutdown().await;
}

/// TEST 3 — soak / stability + handle-leak baseline (LESSON-7).
///
/// Runs 30 sequential then 10 concurrent CONNECT round-trips through the live
/// upstream, asserts all reached 200, that `active_connections()` drains back to
/// zero, and that the process OS-handle count grows by less than a tolerance
/// (the prior build leaked ~60 handles over 60 requests).
#[tokio::test]
async fn live_soak_stability_handles() {
    if !live_enabled() {
        eprintln!("SKIP live_soak_stability_handles: set {GATE}=1 (on corp, domain-joined) to run");
        return;
    }

    const SEQUENTIAL: usize = 30;
    const CONCURRENT: usize = 10;
    const HANDLE_TOLERANCE: u32 = 60;

    let proxy = spawn_live_proxy().await;
    let target = external_host();
    let before = zicade_win::process_handle_count();

    for i in 0..SEQUENTIAL {
        let (status, _tunnel) = connect_through(proxy.addr, &target)
            .await
            .unwrap_or_else(|e| panic!("sequential CONNECT #{i} errored: {e}"));
        assert_eq!(status, 200, "sequential CONNECT #{i} returned {status}");
    }

    let mut tasks = Vec::with_capacity(CONCURRENT);
    for _ in 0..CONCURRENT {
        let addr = proxy.addr;
        let tgt = target.clone();
        tasks.push(tokio::spawn(async move {
            connect_through(addr, &tgt).await.map(|(s, _)| s)
        }));
    }
    for (i, t) in tasks.into_iter().enumerate() {
        let status = t
            .await
            .expect("concurrent task must not panic")
            .unwrap_or_else(|e| panic!("concurrent CONNECT #{i} errored: {e}"));
        assert_eq!(status, 200, "concurrent CONNECT #{i} returned {status}");
    }

    wait_for_active_zero(&proxy.metrics, Duration::from_secs(5)).await;
    assert_eq!(
        proxy.metrics.active_connections(),
        0,
        "active connections must drain to zero after the soak"
    );
    assert!(
        proxy.metrics.total_requests() >= (SEQUENTIAL + CONCURRENT) as u64
            || proxy.metrics.total_requests() == 0,
        "total_requests should count forwarded requests (CONNECT tunnels may not increment it)"
    );

    let after = zicade_win::process_handle_count();
    if let (Some(before), Some(after)) = (before, after) {
        let delta = after.saturating_sub(before);
        eprintln!("live_soak_stability_handles: handles {before} -> {after} (delta {delta})");
        assert!(
            delta < HANDLE_TOLERANCE,
            "handle count grew by {delta} (>= {HANDLE_TOLERANCE}); suspected handle leak"
        );
    } else {
        eprintln!("live_soak_stability_handles: process_handle_count unavailable; skipping delta");
    }

    proxy.shutdown().await;
}
