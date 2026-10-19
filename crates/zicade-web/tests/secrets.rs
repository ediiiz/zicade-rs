//! The local API never hands out stored proxy credentials, and only answers
//! requests addressed to a loopback host (the DNS-rebinding defense).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use zicade_config::{
    AuthConfig, AuthMode, Config, PacConfig, RoutingMode, UpstreamConfig, load_file,
};
use zicade_observe::channel_layer;
use zicade_web::{AppState, router};

fn temp_config_path() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zicade-web-secrets-{nanos}-{n}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("config.json")
}

fn basic(user: &str, pass: &str) -> AuthConfig {
    AuthConfig {
        mode: AuthMode::Basic,
        username: Some(user.to_owned()),
        password: Some(pass.to_owned()),
        ..AuthConfig::default()
    }
}

/// Upstream mode with Basic credentials on both the upstream and the PAC slot.
fn config_with_passwords() -> Config {
    let mut cfg = Config::default();
    cfg.routing.mode = RoutingMode::Upstream;
    cfg.routing.upstream = Some(UpstreamConfig {
        host: "proxy.example".to_owned(),
        port: 8080,
        auth: basic("alice", "upstream-secret"),
    });
    cfg.routing.pac = Some(PacConfig {
        auth: basic("alice", "pac-secret"),
        ..PacConfig::default()
    });
    cfg
}

fn state_with(cfg: &Config) -> (AppState, PathBuf) {
    let path = temp_config_path();
    zicade_config::save_file(&path, cfg).expect("seed config file");
    let (_layer, logs) = channel_layer(64);
    let state = AppState::new(cfg.clone(), path.clone(), "token".to_owned(), logs);
    (state, path)
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn get_config_never_returns_passwords() {
    let (state, _) = state_with(&config_with_passwords());

    let resp = router(state)
        .oneshot(
            Request::builder()
                .uri("/api/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(!body.contains("upstream-secret"), "leaked: {body}");
    assert!(!body.contains("pac-secret"), "leaked: {body}");
    assert!(body.contains("alice"), "usernames stay visible: {body}");
}

/// The UI round-trips the redacted config, so a PUT without a password must
/// keep the saved one rather than wipe it.
#[tokio::test]
async fn put_without_password_keeps_the_saved_one() {
    let saved = config_with_passwords();
    let (state, path) = state_with(&saved);
    let mut edited = saved.clone();
    edited.routing.upstream.as_mut().unwrap().port = 3128;
    edited.routing.upstream.as_mut().unwrap().auth.password = None;
    edited.routing.pac.as_mut().unwrap().auth.password = None;

    let resp = router(state)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&edited).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let on_disk = load_file(&path).unwrap();
    let upstream = on_disk.routing.upstream.unwrap();
    assert_eq!(upstream.port, 3128);
    assert_eq!(upstream.auth.password.as_deref(), Some("upstream-secret"));
    let pac = on_disk.routing.pac.unwrap();
    assert_eq!(pac.auth.password.as_deref(), Some("pac-secret"));
}

#[tokio::test]
async fn put_with_new_password_replaces_the_saved_one() {
    let saved = config_with_passwords();
    let (state, path) = state_with(&saved);
    let mut edited = saved.clone();
    edited.routing.upstream.as_mut().unwrap().auth.password = Some("rotated".to_owned());

    let resp = router(state)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&edited).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let upstream = load_file(&path).unwrap().routing.upstream.unwrap();
    assert_eq!(upstream.auth.password.as_deref(), Some("rotated"));
}

async fn status_for_host(host: &str) -> StatusCode {
    let (state, _) = state_with(&Config::default());
    router(state)
        .oneshot(
            Request::builder()
                .uri("/api/config")
                .header("host", host)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// A DNS-rebinding page reaches the loopback server under its own hostname;
/// the Host header is how the server tells it apart from the real UI.
#[tokio::test]
async fn foreign_host_header_is_rejected() {
    assert_eq!(
        status_for_host("attacker.example:9090").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_for_host("127.0.0.1.attacker.example").await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn loopback_host_headers_are_accepted() {
    for host in [
        "127.0.0.1:9090",
        "localhost:9090",
        "localhost",
        "[::1]:9090",
    ] {
        assert_eq!(status_for_host(host).await, StatusCode::OK, "host {host}");
    }
}
