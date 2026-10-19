//! The UI's handle on the running app: what it may read (no secrets) and what
//! applying an edited config must do (validate, keep unseen passwords, persist,
//! rebuild routing) — or refuse to do.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use zicade_config::{
    AuthConfig, AuthMode, Config, PacConfig, RoutingMode, UpstreamConfig, load_file,
};
use zicade_control::{ConfigApplyHook, Control, MetricsSource, StatusSnapshot};
use zicade_observe::channel_layer;

fn temp_config_path() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zicade-control-test-{nanos}-{n}"));
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

fn control_with(cfg: &Config) -> (Control, PathBuf) {
    let path = temp_config_path();
    zicade_config::save_file(&path, cfg).expect("seed config file");
    let (_layer, logs) = channel_layer(64);
    (Control::new(cfg.clone(), path.clone(), logs), path)
}

/// A control wired to a hook that records the config it last saw.
fn control_with_spy(cfg: &Config) -> (Control, PathBuf, Arc<Mutex<Option<Config>>>) {
    let (control, path) = control_with(cfg);
    let spy: Arc<Mutex<Option<Config>>> = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&spy);
    let hook: ConfigApplyHook = Arc::new(move |c: &Config| {
        *seen.lock().unwrap() = Some(c.clone());
    });
    (control.with_apply_hook(hook), path, spy)
}

#[test]
fn redacted_config_never_contains_passwords() {
    let (control, _) = control_with(&config_with_passwords());

    let shown = zicade_config::to_json_string(&control.config_redacted()).unwrap();

    assert!(!shown.contains("upstream-secret"), "leaked: {shown}");
    assert!(!shown.contains("pac-secret"), "leaked: {shown}");
    assert!(shown.contains("alice"), "usernames stay visible: {shown}");
}

/// The UI edits the redacted config, so an apply without a password must keep
/// the saved one rather than wipe it.
#[test]
fn apply_without_password_keeps_the_saved_one() {
    let (control, path) = control_with(&config_with_passwords());
    let mut edited = control.config_redacted();
    edited.routing.upstream.as_mut().unwrap().port = 3128;

    control.apply(edited).expect("valid edit applies");

    let on_disk = load_file(&path).unwrap();
    let upstream = on_disk.routing.upstream.unwrap();
    assert_eq!(upstream.port, 3128);
    assert_eq!(upstream.auth.password.as_deref(), Some("upstream-secret"));
    let pac = on_disk.routing.pac.unwrap();
    assert_eq!(pac.auth.password.as_deref(), Some("pac-secret"));
}

#[test]
fn apply_with_new_password_replaces_the_saved_one() {
    let (control, path) = control_with(&config_with_passwords());
    let mut edited = control.config_redacted();
    edited.routing.upstream.as_mut().unwrap().auth.password = Some("rotated".to_owned());

    control.apply(edited).expect("valid edit applies");

    let upstream = load_file(&path).unwrap().routing.upstream.unwrap();
    assert_eq!(upstream.auth.password.as_deref(), Some("rotated"));
}

#[test]
fn apply_valid_config_runs_the_hook_with_it() {
    let (control, _, spy) = control_with_spy(&Config::default());
    let mut edited = Config::default();
    edited.listen.port = 5544;

    control.apply(edited).expect("valid edit applies");

    let seen = spy.lock().unwrap().clone();
    assert_eq!(seen.map(|c| c.listen.port), Some(5544));
}

/// An invalid edit must change nothing: not the routing, not the file on disk,
/// not what the UI reads back.
#[test]
fn apply_invalid_config_is_rejected_without_side_effects() {
    let (control, path, spy) = control_with_spy(&Config::default());
    let mut bad = Config::default();
    bad.listen.port = 0;

    let err = control.apply(bad).expect_err("port 0 must fail validation");

    assert!(
        !err.to_string().is_empty(),
        "the UI needs a message to show"
    );
    assert!(
        spy.lock().unwrap().is_none(),
        "hook ran for an invalid config"
    );
    assert_eq!(load_file(&path).unwrap().listen.port, 3129);
    assert_eq!(control.config_redacted().listen.port, 3129);
}

#[test]
fn apply_updates_the_reported_routing_mode() {
    let (control, _) = control_with(&Config::default());
    let mut edited = Config::default();
    edited.routing.mode = RoutingMode::Upstream;
    edited.routing.upstream = Some(UpstreamConfig {
        host: "proxy.example.com".to_owned(),
        port: 8080,
        auth: AuthConfig::default(),
    });

    control.apply(edited).expect("valid edit applies");

    assert_eq!(control.status().routing_mode, "upstream");
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

#[test]
fn status_overlays_live_metrics_onto_the_stored_snapshot() {
    let (control, _) = control_with(&Config::default());
    let control = control.with_metrics_source(Arc::new(FakeMetrics));
    control.set_status(StatusSnapshot {
        routing_mode: "pac".to_owned(),
        listen_addr: "127.0.0.1:3129".to_owned(),
        requests_total: 7,
        ..StatusSnapshot::default()
    });

    let status = control.status();

    assert_eq!(status.routing_mode, "pac");
    assert_eq!(status.listen_addr, "127.0.0.1:3129");
    assert_eq!(status.requests_total, 42);
    assert_eq!(status.active_connections, 3);
    assert_eq!(status.requests_failed, 5);
    assert_eq!((status.bytes_in, status.bytes_out), (1000, 250));
}

#[test]
fn status_reports_the_corp_network_gate_state() {
    let (control, _) = control_with(&Config::default());
    let cell = Arc::new(Mutex::new(None));
    let control = control.with_on_corp(Arc::clone(&cell));
    assert_eq!(
        control.status().on_corp,
        None,
        "unknown until the gate says"
    );

    *cell.lock().unwrap() = Some(true);

    assert_eq!(control.status().on_corp, Some(true));
}
