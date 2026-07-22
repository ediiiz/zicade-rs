//! Status endpoint + live-metrics tests for `zicade-web`.
//!
//! Covers `GET /api/status` (stored snapshot and live-metrics overlay) and the
//! `GET /events/metrics` SSE stream. Driven through the in-memory `Router` via
//! `tower::ServiceExt::oneshot`, mirroring `web.rs`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use zicade_config::Config;
use zicade_observe::channel_layer;
use zicade_web::{AppState, MetricsSource, StatusSnapshot, router};

const TOKEN: &str = "test-token-0123456789abcdef";

fn temp_config_path() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zicade-web-status-test-{nanos}-{n}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("config.json")
}

fn state_with_config(cfg: Config) -> AppState {
    let path = temp_config_path();
    zicade_config::save_file(&path, &cfg).expect("seed config file");
    let (_layer, logs) = channel_layer(64);
    AppState::new(cfg, path, TOKEN.to_owned(), logs)
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("utf8 body")
}

/// A metrics source with fixed counters, to prove the overlay in the snapshot.
struct FakeMetrics;
impl MetricsSource for FakeMetrics {
    fn total_requests(&self) -> u64 {
        42
    }
    fn active_connections(&self) -> usize {
        3
    }
    fn failed_requests(&self) -> u64 {
        5
    }
    fn bytes_in(&self) -> u64 {
        1000
    }
    fn bytes_out(&self) -> u64 {
        250
    }
}

#[tokio::test]
async fn status_reflects_stored_snapshot() {
    let state = state_with_config(Config::default());
    state.set_status(StatusSnapshot {
        routing_mode: "pac".to_owned(),
        listen_addr: "127.0.0.1:3129".to_owned(),
        requests_total: 7,
        requests_failed: 1,
        ..StatusSnapshot::default()
    });
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let got: StatusSnapshot = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(got.routing_mode, "pac");
    assert_eq!(got.requests_total, 7);
    assert_eq!(got.requests_failed, 1);
    assert_eq!(got.listen_addr, "127.0.0.1:3129");
}

#[tokio::test]
async fn status_overlays_live_metrics() {
    let state = state_with_config(Config::default());
    state.set_status(StatusSnapshot {
        routing_mode: "direct".to_owned(),
        listen_addr: "127.0.0.1:3129".to_owned(),
        ..StatusSnapshot::default()
    });
    let state = state.with_metrics_source(std::sync::Arc::new(FakeMetrics));
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let got: StatusSnapshot = serde_json::from_str(&body_string(resp).await).unwrap();
    // Live metrics (incl. bytes) overlay the counters; stored fields preserved.
    assert_eq!(got.requests_total, 42);
    assert_eq!(got.active_connections, 3);
    assert_eq!(got.requests_failed, 5);
    assert_eq!(got.bytes_in, 1000);
    assert_eq!(got.bytes_out, 250);
    assert_eq!(got.routing_mode, "direct");
}

#[tokio::test]
async fn status_overlays_on_corp() {
    // The corp-network gate writes on-corp state into a shared cell that the
    // status snapshot overlays live; a disabled gate (no cell) omits the field.
    let cell: zicade_web::OnCorpCell = std::sync::Arc::new(std::sync::Mutex::new(Some(false)));
    let state = state_with_config(Config::default()).with_on_corp(cell.clone());
    let app = router(state.clone());

    let get_status = |app: axum::Router| async move {
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let got: StatusSnapshot = serde_json::from_str(&body_string(resp).await).unwrap();
        got
    };

    assert_eq!(
        get_status(app).await.on_corp,
        Some(false),
        "off-corp overlaid"
    );

    *cell.lock().unwrap() = Some(true);
    let app = router(state);
    assert_eq!(
        get_status(app).await.on_corp,
        Some(true),
        "on-corp overlaid live"
    );
}

#[tokio::test]
async fn metrics_sse_streams_a_snapshot() {
    let state = state_with_config(Config::default());
    let state = state.with_metrics_source(std::sync::Arc::new(FakeMetrics));
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/events/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(ct.starts_with("text/event-stream"), "content-type was {ct}");

    // The first tick fires immediately, so a snapshot frame arrives promptly.
    let mut body = resp.into_body();
    let frame = tokio::time::timeout(Duration::from_secs(3), body.frame())
        .await
        .expect("a frame within 3s")
        .expect("stream should yield a frame")
        .expect("frame should be Ok");
    let data = frame.into_data().unwrap_or_default();
    let text = String::from_utf8_lossy(&data);
    assert!(
        text.contains("routing_mode") && text.contains("bytes_in"),
        "first SSE frame should be a StatusSnapshot JSON, got: {text}"
    );
}

#[tokio::test]
async fn metrics_sse_ends_when_shutdown_fires() {
    // A long-lived SSE stream must end when the app signals shutdown; otherwise
    // it keeps the connection open and stalls axum's graceful shutdown (the tray
    // "Close" then leaves a zombie process behind).
    let (tx, rx) = tokio::sync::watch::channel(false);
    let state = state_with_config(Config::default())
        .with_metrics_source(std::sync::Arc::new(FakeMetrics))
        .with_shutdown(rx);
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/events/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let mut body = resp.into_body();
    // The immediate first tick proves the stream is live before we shut it down.
    tokio::time::timeout(Duration::from_secs(3), body.frame())
        .await
        .expect("a frame within 3s")
        .expect("stream should yield a frame")
        .expect("frame should be Ok");

    // Signal shutdown; the stream must terminate promptly (a bounded number of
    // trailing frames, then end). Without the fix it emits a frame every second
    // forever, so `ended` stays false and this assertion fails.
    tx.send(true).unwrap();
    let mut ended = false;
    for _ in 0..5 {
        match tokio::time::timeout(Duration::from_secs(3), body.frame())
            .await
            .expect("stream must not hang after shutdown")
        {
            None => {
                ended = true;
                break;
            }
            Some(Ok(_)) => continue,
            Some(Err(_)) => {
                ended = true;
                break;
            }
        }
    }
    assert!(
        ended,
        "metrics SSE stream must end after shutdown is signaled"
    );
}
