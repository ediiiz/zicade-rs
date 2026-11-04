//! Unit tests for the upstream idle-socket pool.
//!
//! The pool is a pure data structure over `TcpStream`, so every test here runs
//! against a loopback `TcpListener`: real sockets, no SSPI, no network beyond
//! `127.0.0.1`.
//!
//! Coverage focus (what a reviewer of a pooling change wants proven):
//!
//! * `take` only ever hands back a socket that is still fresh, and hands back
//!   the *same* socket object it was given (no silent re-dial).
//! * Stale entries are discarded rather than returned, and the discard is
//!   counted.
//! * `take` for an address the pool does not hold returns `None` without
//!   consuming entries belonging to another address — the bug a naive
//!   `remove(i)?` scan would introduce.
//! * `max_idle` bounds the pool per address.
//! * `put` refuses a socket the peer already closed (`shrink` sees EOF), so a
//!   dead socket can never be handed to a later request.
//! * `spawn_prewarm` fills up to `min_idle` and then stops dialing.

use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use zicade_proxy::PoolConfig;
use zicade_proxy::UpstreamPool;

/// Bind a loopback listener that accepts forever, drains whatever arrives, and
/// keeps each socket open until the peer closes it.
///
/// Draining matters: a socket only becomes observably EOF to the *peer* once
/// the kernel has processed the FIN, which for a connected-but-never-read
/// socket can take a round trip. Reading on the accepting side makes the
/// `closed_socket_is_refused_by_put` case deterministic instead of racy.
async fn spawn_sink() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let handle = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                // Drain until the peer closes; the task then ends and drops the
                // socket. Holding it open for the lifetime of the connection is
                // what makes pooled sockets "still usable".
                let mut buf = [0u8; 1024];
                loop {
                    match tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                }
            });
        }
    });
    (addr, handle)
}

fn config() -> PoolConfig {
    PoolConfig::default()
}

#[tokio::test]
async fn put_then_take_returns_the_same_socket() {
    let (addr, _sink) = spawn_sink().await;
    let pool = UpstreamPool::new(&addr, config());

    let stream = TcpStream::connect(&addr).await.unwrap();
    let local = stream.local_addr().unwrap();
    pool.put(&addr, stream);
    assert_eq!(pool.len(), 1);

    let taken = pool.take(&addr).expect("fresh socket should be returned");
    assert_eq!(
        taken.local_addr().unwrap(),
        local,
        "take must return the pooled socket, not a fresh dial"
    );
    assert_eq!(pool.len(), 0);
    let (hits, discards, warmed) = pool.stats();
    assert_eq!(hits, 1);
    assert_eq!(discards, 0);
    assert_eq!(warmed, 0);
}

#[tokio::test]
async fn take_missing_address_does_not_consume_other_entries() {
    let (addr, _sink) = spawn_sink().await;
    let pool = UpstreamPool::new(&addr, config());

    for _ in 0..3 {
        pool.put(&addr, TcpStream::connect(&addr).await.unwrap());
    }
    assert_eq!(pool.len(), 3);

    // A different address must not steal or drain the entries held for `addr`.
    assert!(pool.take("127.0.0.1:1").is_none());
    assert_eq!(
        pool.len(),
        3,
        "a miss must not consume other addresses' sockets"
    );

    assert!(pool.take(&addr).is_some());
    assert_eq!(pool.len(), 2);
}

#[tokio::test]
async fn stale_entries_are_discarded_not_returned() {
    let (addr, _sink) = spawn_sink().await;
    let pool = UpstreamPool::new(
        &addr,
        PoolConfig {
            idle_timeout: Duration::from_millis(10),
            ..config()
        },
    );

    pool.put(&addr, TcpStream::connect(&addr).await.unwrap());
    tokio::time::sleep(Duration::from_millis(30)).await;

    assert!(
        pool.take(&addr).is_none(),
        "expired socket must not be reused"
    );
    assert_eq!(pool.len(), 0);
    let (hits, discards, _) = pool.stats();
    assert_eq!(hits, 0);
    assert_eq!(discards, 1);
}

#[tokio::test]
async fn max_idle_bounds_the_pool_per_address() {
    let (addr, _sink) = spawn_sink().await;
    let pool = UpstreamPool::new(
        &addr,
        PoolConfig {
            max_idle: 2,
            ..config()
        },
    );

    for _ in 0..5 {
        pool.put(&addr, TcpStream::connect(&addr).await.unwrap());
    }
    assert_eq!(pool.len(), 2, "pool must not exceed max_idle");
    let (_, discards, _) = pool.stats();
    assert_eq!(discards, 3, "evictions are counted as discards");
}

#[tokio::test]
async fn closed_socket_is_refused_by_put() {
    let (addr, _sink) = spawn_sink().await;
    let pool = UpstreamPool::new(&addr, config());

    // A socket the peer has already closed must never enter the pool: if it
    // did, the next request would pick it up and fail on first write.
    let mut dead = TcpStream::connect(&addr).await.unwrap();
    dead.shutdown().await.unwrap();
    // Poll until the local side actually observes EOF. A fixed sleep is racy
    // on Windows: the FIN has to round-trip before `try_read` returns 0.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut buf = [0u8; 1];
    while tokio::time::Instant::now() < deadline {
        if let Ok(0) = tokio::io::AsyncReadExt::read(&mut dead, &mut buf).await {
            break;
        }
    }

    pool.put(&addr, dead);
    assert!(
        pool.is_empty(),
        "a socket the peer closed must not be pooled"
    );
    let (_, discards, _) = pool.stats();
    assert_eq!(discards, 1);
}

#[tokio::test]
async fn clear_drops_everything() {
    let (addr, _sink) = spawn_sink().await;
    let pool = UpstreamPool::new(&addr, config());
    for _ in 0..3 {
        pool.put(&addr, TcpStream::connect(&addr).await.unwrap());
    }
    assert_eq!(pool.len(), 3);
    pool.clear();
    assert!(pool.is_empty());
    assert!(pool.take(&addr).is_none());
}

#[tokio::test]
async fn prewarm_fills_to_min_idle_and_then_stops() {
    let (addr, _sink) = spawn_sink().await;
    let min_idle = 3;
    let pool = std::sync::Arc::new(UpstreamPool::new(
        &addr,
        PoolConfig {
            min_idle,
            prewarm_interval: Duration::from_millis(10),
            ..config()
        },
    ));

    let task = zicade_proxy::spawn_prewarm(std::sync::Arc::clone(&pool));

    // Wait for the watermark, with a generous bound so a slow CI box still
    // passes but a never-filling pool fails instead of hanging.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while pool.len() < min_idle && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(pool.len(), min_idle, "prewarm must fill to min_idle");

    // Once at the watermark it must stop dialing: give it many intervals and
    // confirm the count does not creep past min_idle.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        pool.len(),
        min_idle,
        "prewarm must not overshoot the watermark while the pool is untouched"
    );

    let (_, _, warmed) = pool.stats();
    assert!(
        warmed >= min_idle as usize,
        "warmed counter must track dials"
    );

    task.abort();
}

#[tokio::test]
async fn prewarm_survives_a_dead_upstream() {
    // Point at a port with nothing listening: the prewarm loop must keep
    // ticking (logging at debug) rather than exit or spin.
    let dead_addr = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        drop(listener);
        addr
    };
    let pool = std::sync::Arc::new(UpstreamPool::new(
        &dead_addr,
        PoolConfig {
            min_idle: 1,
            prewarm_interval: Duration::from_millis(10),
            ..config()
        },
    ));

    let task = zicade_proxy::spawn_prewarm(std::sync::Arc::clone(&pool));
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert!(
        !task.is_finished(),
        "prewarm must keep running while the upstream is unreachable"
    );
    assert!(pool.is_empty());
    task.abort();
}
