//! M5 acceptance tests for `zicade-web`.
//!
//! Driven entirely through `tower::ServiceExt::oneshot` against the in-memory
//! `Router` — no real socket is bound, so the tests are deterministic and safe.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use zicade_config::{Config, from_json_str, load_file};
use zicade_observe::channel_layer;
use zicade_web::{AppState, router};

const TOKEN: &str = "test-token-0123456789abcdef";

/// A unique temp directory for on-disk config, no external tempfile crate.
fn temp_config_path() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zicade-web-test-{nanos}-{n}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("config.json")
}

/// Build state with a known config already persisted to disk.
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
        .expect("collect body")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("utf8 body")
}

#[tokio::test]
async fn get_config_returns_current() {
    let cfg = Config::default();
    let state = state_with_config(cfg.clone());
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let got: Config = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(got, cfg);
}

/// A config with the WebUI auth gate turned ON.
fn auth_on() -> Config {
    let mut cfg = Config::default();
    cfg.web.auth_required = true;
    cfg
}

#[tokio::test]
async fn put_config_without_token_succeeds_when_auth_off() {
    // Default config => web.authRequired == false => no token needed.
    let cfg = Config::default();
    assert!(!cfg.web.auth_required, "precondition: auth off by default");
    let state = state_with_config(cfg.clone());
    let path = state.config_path().to_owned();
    let config_handle = state.config_arc();
    let app = router(state);

    let mut new_cfg = cfg.clone();
    new_cfg.listen.port = 8123;
    let body = zicade_config::to_json_string(&new_cfg).unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "auth off: PUT must succeed without a token"
    );
    assert_eq!(config_handle.lock().unwrap().listen.port, 8123);
    assert_eq!(load_file(&path).unwrap(), new_cfg);
}

#[tokio::test]
async fn put_config_without_token_is_rejected() {
    let cfg = auth_on();
    let state = state_with_config(cfg.clone());
    let path = state.config_path().to_owned();
    let app = router(state);

    let mut new_cfg = cfg.clone();
    new_cfg.listen.port = 9999;
    let body = zicade_config::to_json_string(&new_cfg).unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        resp.status() == StatusCode::UNAUTHORIZED || resp.status() == StatusCode::FORBIDDEN,
        "expected 401/403, got {}",
        resp.status()
    );
    // On-disk file must be unchanged.
    let on_disk = load_file(&path).unwrap();
    assert_eq!(on_disk, cfg, "file must not change without a token");
}

#[tokio::test]
async fn put_config_with_token_persists_and_applies() {
    let cfg = auth_on();
    let state = state_with_config(cfg.clone());
    let path = state.config_path().to_owned();
    let config_handle = state.config_arc();
    let app = router(state);

    let mut new_cfg = cfg.clone();
    new_cfg.listen.port = 4567;
    let body = zicade_config::to_json_string(&new_cfg).unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/config")
                .header("content-type", "application/json")
                .header("X-Zicade-Token", TOKEN)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    // In-memory config updated.
    assert_eq!(config_handle.lock().unwrap().listen.port, 4567);
    // On-disk file updated.
    let on_disk = load_file(&path).unwrap();
    assert_eq!(on_disk, new_cfg);
}

#[tokio::test]
async fn put_invalid_config_is_400() {
    let cfg = Config::default();
    let state = state_with_config(cfg.clone());
    let path = state.config_path().to_owned();
    let app = router(state);

    // port 0 fails validation on any platform.
    let bad = r#"{ "listen": { "host": "127.0.0.1", "port": 0 },
                   "routing": { "mode": "direct" },
                   "logging": { "level": "info", "format": "json" } }"#;

    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/config")
                .header("X-Zicade-Token", TOKEN)
                .body(Body::from(bad))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    // Must not persist the bad config.
    let on_disk = load_file(&path).unwrap();
    assert_eq!(on_disk, cfg);
}

#[tokio::test]
async fn sse_logs_endpoint_streams() {
    let state = state_with_config(Config::default());
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/events/logs")
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
}

/// Events logged BEFORE the UI connects must be replayed to a fresh SSE
/// subscriber (parity with the console, which saw them live), fields included.
#[tokio::test]
async fn sse_logs_replays_buffered_backlog() {
    let cfg = Config::default();
    let path = temp_config_path();
    zicade_config::save_file(&path, &cfg).expect("seed config file");
    let (_layer, logs) = channel_layer(64);
    logs.push(zicade_observe::LogEvent {
        seq: 0, // the store assigns the real seq on push
        timestamp: 1_700_000_000_000,
        level: "INFO".to_owned(),
        target: "test".to_owned(),
        message: "buffered before the UI connected".to_owned(),
        fields: std::collections::BTreeMap::from([("answer".to_owned(), "42".to_owned())]),
    });
    let state = AppState::new(cfg, path, TOKEN.to_owned(), logs);
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/events/logs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The stream never ends (no shutdown signal attached), so read only the
    // first frame — it must be the replayed backlog event.
    let mut body = resp.into_body();
    let frame = body
        .frame()
        .await
        .expect("a first frame must arrive immediately")
        .expect("frame reads cleanly");
    let bytes = frame.into_data().expect("data frame");
    let text = String::from_utf8(bytes.to_vec()).expect("utf8 frame");
    assert!(
        text.contains("buffered before the UI connected"),
        "backlog must be replayed first, got: {text}"
    );
    assert!(
        text.contains("\"answer\":\"42\""),
        "structured fields must survive the replay, got: {text}"
    );
}

#[tokio::test]
async fn get_index_serves_html() {
    let state = state_with_config(Config::default());
    let app = router(state);

    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(ct.starts_with("text/html"), "content-type was {ct}");
    let body = body_string(resp).await;
    assert!(body.contains("Zicade"), "index should mention Zicade");
    // Self-contained: links the vendored Pico stylesheet same-origin, never a CDN.
    assert!(
        body.contains("/assets/pico.min.css"),
        "index must link the local Pico stylesheet"
    );
    assert!(
        body.contains("/assets/app.js"),
        "index must link the local app script (extracted, served same-origin)"
    );
    assert!(
        !body.contains("cdn") && !body.contains("http://") && !body.contains("https://"),
        "index must not reference any external URL"
    );
    // Progressive-disclosure inputs for every config section are present.
    for field in [
        "listen.host",
        "listen.port",
        "routing.mode",
        "upstream.host",
        "upstream.port",
        "upstream.auth.mode",
        "upstream.auth.package",
        "upstream.auth.username",
        "upstream.auth.password",
        "pac.source",
        "pac.path",
        "pac.url",
        "pac.failPolicy",
        "pac.auth.mode",
        "logging.level",
        "logging.format",
        "web.authRequired",
    ] {
        assert!(body.contains(field), "index form must expose {field}");
    }
}

#[tokio::test]
async fn serves_embedded_pico_css() {
    // The UI must be fully self-contained: Pico CSS is vendored and served
    // same-origin (no external CDN), so index.html can link it locally.
    let state = state_with_config(Config::default());
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/assets/pico.min.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK, "pico.min.css must be served");
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(ct.starts_with("text/css"), "content-type was {ct}");
    let body = body_string(resp).await;
    assert!(!body.is_empty(), "css body must not be empty");
    assert!(
        body.contains(":root") || body.contains("--pico"),
        "served body should be the Pico stylesheet"
    );
}

#[test]
fn parses_config_json_helper_available() {
    // Guards the re-export surface used by callers of the crate.
    let cfg = from_json_str(r#"{"listen":{"host":"127.0.0.1","port":3129}}"#).unwrap();
    assert_eq!(cfg.listen.port, 3129);
}
