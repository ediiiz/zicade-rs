//! Tests for the live config-apply hook (`AppState::with_apply_hook`).
//!
//! A `PUT /api/config` must run the hook with the NEW config on success, and
//! must NOT run it when validation rejects the body. Driven through the
//! in-memory `Router` via `tower::ServiceExt::oneshot`, mirroring `web.rs`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use zicade_config::Config;
use zicade_observe::channel_layer;
use zicade_web::{AppState, ConfigApplyHook, router};

const TOKEN: &str = "test-token-0123456789abcdef";

/// A unique temp directory for on-disk config, no external tempfile crate.
fn temp_config_path() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zicade-web-hook-test-{nanos}-{n}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("config.json")
}

/// Build state (config already persisted to disk) wired to a spy hook that
/// records the config it was last invoked with.
fn state_with_spy(cfg: Config) -> (AppState, Arc<Mutex<Option<Config>>>) {
    let path = temp_config_path();
    zicade_config::save_file(&path, &cfg).expect("seed config file");
    let (_layer, logs) = channel_layer(64);

    let spy: Arc<Mutex<Option<Config>>> = Arc::new(Mutex::new(None));
    let spy_hook = Arc::clone(&spy);
    let hook: ConfigApplyHook = Arc::new(move |c: &Config| {
        *spy_hook.lock().unwrap() = Some(c.clone());
    });
    let state = AppState::new(cfg, path, TOKEN.to_owned(), logs).with_apply_hook(hook);
    (state, spy)
}

#[tokio::test]
async fn put_valid_config_invokes_apply_hook() {
    let cfg = Config::default();
    let (state, spy) = state_with_spy(cfg.clone());
    let app = router(state);

    let mut new_cfg = cfg;
    new_cfg.listen.port = 5544;
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

    assert_eq!(resp.status(), StatusCode::OK);
    let captured = spy.lock().unwrap().clone();
    assert_eq!(
        captured.map(|c| c.listen.port),
        Some(5544),
        "apply hook must receive the new config on a valid PUT"
    );
}

#[tokio::test]
async fn put_invalid_config_does_not_invoke_apply_hook() {
    let (state, spy) = state_with_spy(Config::default());
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
    assert!(
        spy.lock().unwrap().is_none(),
        "apply hook must NOT run for an invalid config"
    );
}
