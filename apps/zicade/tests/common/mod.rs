//! Shared support for the gated live-proxy suite: env knobs, a production-wired
//! [`LiveProxy`] harness, raw HTTP/CONNECT helpers, a hand-built TLS ClientHello,
//! and sequential/concurrent soak drivers. Kept in `tests/common` so it compiles
//! as a helper module rather than its own test binary.
#![allow(dead_code)]

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

pub const GATE: &str = "ZICADE_LIVE_PROXY";

/// Whether the live suite is enabled (`ZICADE_LIVE_PROXY=1`).
pub fn live_enabled() -> bool {
    std::env::var(GATE).as_deref() == Ok("1")
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

pub fn upstream() -> String {
    env_or("ZICADE_LIVE_UPSTREAM", "wp8080:8080")
}

pub fn external_host() -> String {
    env_or("ZICADE_LIVE_EXTERNAL_HOST", "example.com:443")
}

pub fn http_url() -> String {
    env_or("ZICADE_LIVE_HTTP_URL", "http://example.com/")
}

/// Split a `host:port` string, defaulting the port when absent/unparseable.
pub fn split_host_port(hp: &str, default_port: u16) -> (String, u16) {
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
pub struct LiveProxy {
    pub addr: SocketAddr,
    pub metrics: ProxyMetrics,
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

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = self.join.await.expect("serve task must not panic");
    }
}

/// Build the live upstream+Negotiate proxy via the production routing mapper.
pub async fn spawn_live_proxy() -> LiveProxy {
    let routing =
        zicade::build_routing(&upstream_negotiate_config()).expect("build upstream routing");
    LiveProxy::spawn(routing).await
}

/// Read an HTTP response head (up to `\r\n\r\n`) and return the status code plus
/// the raw head text (for diagnostics).
pub async fn read_head(stream: &mut TcpStream) -> std::io::Result<(u16, String)> {
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
pub async fn connect_through(proxy: SocketAddr, target: &str) -> std::io::Result<(u16, TcpStream)> {
    let mut stream = TcpStream::connect(proxy).await?;
    let head = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await?;
    let (status, _text) = read_head(&mut stream).await?;
    Ok((status, stream))
}

/// Forward an absolute-form plain-HTTP request through the local proxy and
/// return its status.
pub async fn forward_get(proxy: SocketAddr, url: &str) -> std::io::Result<u16> {
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
pub async fn wait_for_active_zero(metrics: &ProxyMetrics, timeout: Duration) {
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
pub fn tls_client_hello(sni_host: &str) -> Vec<u8> {
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

/// Run `n` sequential CONNECT round-trips through the proxy, asserting each 200.
pub async fn soak_sequential(proxy: SocketAddr, target: &str, n: usize, label: &str) {
    for i in 0..n {
        let (status, _tunnel) = connect_through(proxy, target)
            .await
            .unwrap_or_else(|e| panic!("{label} CONNECT #{i} errored: {e}"));
        assert_eq!(status, 200, "{label} CONNECT #{i} returned {status}");
    }
}

/// Fire `n` CONNECT round-trips concurrently, asserting each 200. Running these
/// in parallel drives the `spawn_blocking` SSPI pool to its high-water mark.
pub async fn soak_concurrent(proxy: SocketAddr, target: &str, n: usize, label: &str) {
    let mut tasks = Vec::with_capacity(n);
    for _ in 0..n {
        let tgt = target.to_owned();
        tasks.push(tokio::spawn(async move {
            connect_through(proxy, &tgt).await.map(|(s, _)| s)
        }));
    }
    for (i, t) in tasks.into_iter().enumerate() {
        let status = t
            .await
            .expect("concurrent task must not panic")
            .unwrap_or_else(|e| panic!("{label} concurrent CONNECT #{i} errored: {e}"));
        assert_eq!(
            status, 200,
            "{label} concurrent CONNECT #{i} returned {status}"
        );
    }
}
