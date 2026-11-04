//! A parked socket is handed out only while it is open, silent and fresh; the
//! pool stays at its watermark; the registry is shared and bounded.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::prewarm::Backoff;
use super::{PoolConfig, Refresh, UpstreamPool, UpstreamPools};

fn config(min_idle: usize, idle_timeout: Duration) -> PoolConfig {
    PoolConfig {
        min_idle,
        idle_timeout,
    }
}

/// What the fake upstream does with each connection it accepts.
#[derive(Clone, Copy)]
enum Peer {
    Silent,
    Closes,
    Chatters,
}

async fn upstream(peer: Peer) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                match peer {
                    Peer::Closes => return,
                    Peer::Chatters => {
                        let _ = stream.write_all(b"HTTP/1.1 408 Timeout\r\n\r\n").await;
                    }
                    Peer::Silent => {}
                }
                let mut buf = [0u8; 64];
                while matches!(stream.read(&mut buf).await, Ok(n) if n > 0) {}
            });
        }
    });
    addr
}

/// A pool holding one socket to a `peer`, after giving the peer's close or
/// bytes time to arrive.
async fn parked(peer: Peer, config: PoolConfig) -> Arc<UpstreamPool> {
    let addr = upstream(peer).await;
    let pool = UpstreamPool::new(&addr, config);
    pool.put(TcpStream::connect(&addr).await.unwrap());
    tokio::time::sleep(Duration::from_millis(200)).await;
    pool
}

fn roomy() -> PoolConfig {
    config(4, Duration::from_secs(30))
}

#[tokio::test]
async fn an_open_silent_socket_is_handed_out() {
    let pool = parked(Peer::Silent, roomy()).await;
    assert!(pool.take_idle().is_some());
    assert_eq!(pool.idle_count(), 0);
}

#[tokio::test]
async fn a_socket_the_upstream_closed_is_not_handed_out() {
    let pool = parked(Peer::Closes, roomy()).await;
    assert!(pool.take_idle().is_none());
    assert_eq!(pool.idle_count(), 0, "the dead socket is closed, not kept");
}

#[tokio::test]
async fn a_socket_with_unread_bytes_is_closed_not_reused() {
    let pool = parked(Peer::Chatters, roomy()).await;
    assert!(pool.take_idle().is_none());
    assert_eq!(pool.idle_count(), 0);
}

#[tokio::test]
async fn an_expired_socket_is_not_handed_out() {
    let pool = parked(Peer::Silent, config(4, Duration::from_millis(50))).await;
    assert!(pool.take_idle().is_none());
}

#[tokio::test]
async fn the_pool_never_holds_more_than_its_watermark() {
    let addr = upstream(Peer::Silent).await;
    let pool = UpstreamPool::new(&addr, config(2, Duration::from_secs(30)));
    for _ in 0..3 {
        pool.put(TcpStream::connect(&addr).await.unwrap());
    }
    assert_eq!(pool.idle_count(), 2);
    assert!(matches!(pool.refresh(), Refresh::Full { .. }));
}

#[tokio::test]
async fn refresh_reports_sockets_the_upstream_closed() {
    let pool = parked(Peer::Closes, config(1, Duration::from_secs(30))).await;
    assert_eq!(pool.refresh(), Refresh::Short { closed_by_peer: 1 });
}

#[tokio::test]
async fn refresh_does_not_blame_the_upstream_for_expiry() {
    let pool = parked(Peer::Silent, config(1, Duration::from_millis(50))).await;
    assert_eq!(pool.refresh(), Refresh::Short { closed_by_peer: 0 });
}

#[test]
fn backoff_doubles_to_a_cap_and_resets() {
    let mut backoff = Backoff::default();
    assert_eq!(backoff.next_delay(), Backoff::FIRST);
    assert_eq!(backoff.next_delay(), Backoff::FIRST * 2);
    for _ in 0..20 {
        backoff.next_delay();
    }
    assert_eq!(backoff.next_delay(), Backoff::MAX);
    backoff.reset();
    assert_eq!(backoff.next_delay(), Backoff::FIRST);
}

#[test]
fn the_registry_shares_one_pool_per_upstream_up_to_a_cap() {
    let pools = UpstreamPools::new(roomy());
    let first = pools.get("proxy.example.com:8080").unwrap();
    assert!(Arc::ptr_eq(
        &first,
        &pools.get("proxy.example.com:8080").unwrap()
    ));
    for n in 1..UpstreamPools::MAX_UPSTREAMS {
        assert!(pools.get(&format!("proxy{n}.example.com:8080")).is_some());
    }
    assert!(
        pools.get("one-too-many.example.com:8080").is_none(),
        "past the cap an upstream is dialed per request"
    );
    assert!(pools.get("proxy.example.com:8080").is_some());
}
