//! Gated live-proxy regression tests against the real corporate upstream.
//!
//! EVERY test here is gated behind `ZICADE_LIVE_PROXY=1`. With the gate unset
//! (the default — this offline/CI box included) each test prints an explicit
//! `SKIP` line and returns; it never silently passes and never touches the
//! network. On a domain-joined, on-corp machine,
//! `ZICADE_LIVE_PROXY=1 cargo test -p zicade --test live` exercises the real
//! `407 -> Negotiate/NTLM -> 200` handshake, HTTPS `CONNECT` tunneling,
//! plain-HTTP forwarding, and a soak / handle-leak regression against
//! `wp8080:8080`.
//!
//! Routing is built through the production `zicade::build_routing` mapper so the
//! SSPI `AuthFactory` is wired exactly as the shipping binary wires it. Shared
//! harness/helpers live in [`common`].
//!
//! Env knobs (all optional, sensible defaults so the suite is pointable):
//! - `ZICADE_LIVE_PROXY`         gate; must equal `"1"` to run.
//! - `ZICADE_LIVE_UPSTREAM`      corporate upstream `host:port` (default `wp8080:8080`).
//! - `ZICADE_LIVE_EXTERNAL_HOST` CONNECT target `host:port` (default `example.com:443`).
//! - `ZICADE_LIVE_HTTP_URL`      plain-HTTP forward target (default `http://example.com/`).

mod common;

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use common::{
    GATE, connect_through, external_host, forward_get, http_url, live_enabled, soak_concurrent,
    soak_sequential, spawn_live_proxy, split_host_port, tls_client_hello, upstream,
    wait_for_active_zero,
};

/// TEST 1 — live Negotiate/NTLM handshake + HTTPS via CONNECT through the gateway.
///
/// Starts the proxy in upstream mode, opens a CONNECT to the external HTTPS host
/// through it (the real `407 -> handshake -> 200` dance runs against the corp
/// upstream), asserts 200, then writes a TLS ClientHello and reads the server's
/// response bytes back through the tunnel to prove end-to-end data.
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
        "CONNECT {target} via {} must return 200 after the handshake",
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
/// Forwards `GET ZICADE_LIVE_HTTP_URL` through the proxy in upstream mode and
/// asserts a sane status (2xx/3xx). Plain-HTTP external targets can be flaky, so
/// this is deliberately lenient on the exact code.
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

/// TEST 3 — soak / stability + handle-leak regression (LESSON-7).
///
/// The prior Zig build leaked OS handles roughly linearly (~60 over 60 requests).
/// This proves ours reaches a flat steady state: we first *warm up* the tokio
/// runtime (worker threads, IOCP, `spawn_blocking` pool, DNS resolver — a
/// legitimate one-time cost) across BOTH sequential and concurrent paths,
/// snapshot the handle count as the baseline, then run a heavier sequential +
/// concurrent load and assert the count barely moves. A real leak keeps climbing
/// here; steady state stays put. We also assert active connections drain to zero,
/// so no tunnel task is left dangling.
#[tokio::test]
async fn live_soak_stability_handles() {
    if !live_enabled() {
        eprintln!("SKIP live_soak_stability_handles: set {GATE}=1 (on corp, domain-joined) to run");
        return;
    }

    const WARMUP_SEQ: usize = 20;
    const SEQUENTIAL: usize = 40;
    const CONCURRENT: usize = 10;
    // Post-warmup steady state is flat for the sequential path; the concurrent
    // `spawn_blocking` SSPI pool adds bounded run-to-run jitter (observed 5–16
    // handles). A *linear* leak would add ~1 handle/connection → ~50 over the 50
    // measured connections (the prior Zig build's behavior). 32 sits well above
    // the jitter and well below a leak, so it flags regressions without flaking.
    const HANDLE_TOLERANCE: u32 = 32;

    let proxy = spawn_live_proxy().await;
    let target = external_host();

    // Warm up BOTH paths — sequential and a concurrent burst — so the tokio
    // worker/`spawn_blocking` pools reach their high-water mark before we
    // snapshot. Otherwise the first concurrent burst inflates the pool and looks
    // like a leak. Then drain and snapshot the baseline.
    soak_sequential(proxy.addr, &target, WARMUP_SEQ, "warmup").await;
    soak_concurrent(proxy.addr, &target, CONCURRENT, "warmup").await;
    wait_for_active_zero(&proxy.metrics, Duration::from_secs(5)).await;
    let before = zicade_win::process_handle_count();

    // Measured phase: sequential then concurrent load, same concurrency as warmup.
    soak_sequential(proxy.addr, &target, SEQUENTIAL, "sequential").await;
    soak_concurrent(proxy.addr, &target, CONCURRENT, "measured").await;

    wait_for_active_zero(&proxy.metrics, Duration::from_secs(5)).await;
    assert_eq!(
        proxy.metrics.active_connections(),
        0,
        "active connections must drain to zero after the soak"
    );

    let after = zicade_win::process_handle_count();
    if let (Some(before), Some(after)) = (before, after) {
        let delta = after.saturating_sub(before);
        eprintln!(
            "live_soak_stability_handles: steady-state handles {before} -> {after} \
             (delta {delta} over {} measured connections)",
            SEQUENTIAL + CONCURRENT
        );
        assert!(
            delta < HANDLE_TOLERANCE,
            "handle count grew by {delta} (>= {HANDLE_TOLERANCE}) after warmup; suspected leak"
        );
    } else {
        eprintln!("live_soak_stability_handles: process_handle_count unavailable; skipping delta");
    }

    proxy.shutdown().await;
}
