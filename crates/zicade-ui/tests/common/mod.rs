//! Shared fixture for the window tests: a real [`Control`] over a config file
//! on disk, with traffic the test controls.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, ElementId, Point, TestAppContext, Window, WindowBounds,
    WindowOptions, init, open_window, px, size,
};
use zicade_config::{Config, ListenConfig};
use zicade_control::{
    ConfigApplyHook, ConnectionKind, ConnectionSnapshot, ConnectionState, Control, MetricsSource,
    StatusSnapshot,
};
use zicade_observe::{LogEvent, LogStore, channel_layer};
use zicade_ui::{MainView, bind_keys};

pub const WAIT: Duration = Duration::from_secs(5);

pub fn temp_config_path() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zicade-ui-window-{nanos}-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("config.json")
}

/// Traffic the test controls: byte totals and the connection list.
#[derive(Default)]
pub struct FakeTraffic {
    pub bytes_in: AtomicU64,
    pub connections: Mutex<Vec<ConnectionSnapshot>>,
}

impl FakeTraffic {
    pub fn show(&self, connections: Vec<ConnectionSnapshot>) {
        *self.connections.lock().unwrap() = connections;
    }
}

impl MetricsSource for FakeTraffic {
    fn total_requests(&self) -> u64 {
        0
    }
    fn active_connections(&self) -> usize {
        0
    }
    fn failed_requests(&self) -> u64 {
        0
    }
    fn bytes_in(&self) -> u64 {
        self.bytes_in.load(Ordering::Relaxed)
    }
    fn bytes_out(&self) -> u64 {
        0
    }
    fn connections(&self) -> Vec<ConnectionSnapshot> {
        self.connections.lock().unwrap().clone()
    }
}

pub fn connection(id: u64, target: &str, state: ConnectionState) -> ConnectionSnapshot {
    ConnectionSnapshot {
        id,
        kind: ConnectionKind::Tunnel,
        target: target.to_owned(),
        route: "direct".to_owned(),
        client: None,
        opened_at: SystemTime::now(),
        duration: Duration::from_secs(3),
        bytes_up: 1024,
        bytes_down: 2048,
        state,
    }
}

pub struct Fixture {
    pub control: Control,
    pub path: PathBuf,
    pub logs: LogStore,
    pub applied: Arc<Mutex<Option<Config>>>,
    pub traffic: Arc<FakeTraffic>,
}

pub fn fixture() -> Fixture {
    let config = Config {
        listen: ListenConfig {
            host: "127.0.0.1".to_owned(),
            port: 3229,
        },
        ..Config::default()
    };
    let path = temp_config_path();
    zicade_config::save_file(&path, &config).unwrap();
    let (_layer, logs) = channel_layer(64);
    let applied = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&applied);
    let hook: ConfigApplyHook = Arc::new(move |c: &Config| *seen.lock().unwrap() = Some(c.clone()));
    let traffic = Arc::new(FakeTraffic::default());
    let control = Control::new(config, path.clone(), logs.clone())
        .with_apply_hook(hook)
        .with_metrics_source(Arc::clone(&traffic) as Arc<dyn MetricsSource>);
    control.set_status(StatusSnapshot {
        routing_mode: "direct".to_owned(),
        listen_addr: "127.0.0.1:3229".to_owned(),
        ..StatusSnapshot::default()
    });
    Fixture {
        control,
        path,
        logs,
        applied,
        traffic,
    }
}

pub fn log(seq: u64, message: &str) -> LogEvent {
    LogEvent {
        seq,
        timestamp: 0,
        level: "INFO".to_owned(),
        target: "zicade".to_owned(),
        message: message.to_owned(),
        fields: Default::default(),
    }
}

pub fn open(control: Result<Control, String>, cx: &mut TestAppContext) -> AnyWindowHandle {
    cx.update(|cx| {
        init(cx);
        bind_keys(cx);
    });
    cx.update(|cx| {
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(980.), px(1400.)),
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        open_window(options, cx, |window, cx| {
            cx.new(|cx| MainView::new(control, window, cx))
        })
        .unwrap()
        .0
    })
}

pub fn label(window: &Window, id: impl Into<ElementId>) -> Option<String> {
    window.find(id).label().map(str::to_owned)
}
