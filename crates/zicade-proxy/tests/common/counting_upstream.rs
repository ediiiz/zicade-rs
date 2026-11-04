//! A fake upstream proxy that counts what reaches it, so tests assert how many
//! times the proxy asked rather than how long it took.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use super::upstream_fake::{echo_tunnel, head_has_authorization, read_request};

/// What the fake does with each request.
#[derive(Clone)]
pub enum Script {
    /// Grant every `CONNECT` and echo the tunnel.
    Grant,
    /// Like [`Script::Grant`], but read the `n`th request (1-based, across all
    /// connections) and close without answering, as a gateway does with a
    /// connection it reaped while it sat idle.
    GrantButDrop(usize),
    /// Answer every request `403` and close.
    Refuse,
    /// `407` until the request carries exactly this `Proxy-Authorization`.
    Basic(String),
}

/// Running totals, read with [`Counts::get`].
#[derive(Debug, Default)]
pub struct Counts {
    /// TCP connections accepted.
    pub accepted: AtomicUsize,
    /// Connections that have ended.
    pub closed: AtomicUsize,
    /// Request heads read.
    pub requests: AtomicUsize,
    /// Requests that carried any `Proxy-Authorization`.
    pub credentialed: AtomicUsize,
}

impl Counts {
    pub fn get(counter: &AtomicUsize) -> usize {
        counter.load(Ordering::SeqCst)
    }
}

/// Spawn the fake on an ephemeral loopback port.
pub async fn spawn_counting_upstream(script: Script) -> (SocketAddr, Arc<Counts>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let counts = Arc::new(Counts::default());
    let shared = Arc::clone(&counts);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            shared.accepted.fetch_add(1, Ordering::SeqCst);
            let counts = Arc::clone(&shared);
            let script = script.clone();
            tokio::spawn(async move {
                serve(stream, &script, &counts).await;
                counts.closed.fetch_add(1, Ordering::SeqCst);
            });
        }
    });
    (addr, counts)
}

async fn serve(mut stream: TcpStream, script: &Script, counts: &Counts) {
    while let Some((head, _body)) = read_request(&mut stream).await {
        let number = counts.requests.fetch_add(1, Ordering::SeqCst) + 1;
        if head.to_ascii_lowercase().contains("proxy-authorization") {
            counts.credentialed.fetch_add(1, Ordering::SeqCst);
        }
        let grant = match script {
            Script::Grant => true,
            Script::GrantButDrop(n) => {
                if number == *n {
                    return;
                }
                true
            }
            Script::Refuse => {
                let refusal =
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(refusal).await;
                return;
            }
            Script::Basic(expected) => head_has_authorization(&head, expected),
        };
        if !grant {
            let challenge = "HTTP/1.1 407 Proxy Authentication Required\r\n\
Proxy-Authenticate: Basic realm=\"test\"\r\nContent-Length: 0\r\n\r\n";
            if stream.write_all(challenge.as_bytes()).await.is_err() {
                return;
            }
            continue;
        }
        if stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .is_ok()
        {
            echo_tunnel(stream).await;
        }
        return;
    }
}
