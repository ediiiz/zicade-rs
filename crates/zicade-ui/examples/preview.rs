//! Opens the window with sample data, for looking at the design without a
//! running proxy:
//!
//! ```text
//! cargo run -p zicade-ui --example preview -- [overview|connections|logs|settings] [light|dark]
//! ```
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a developer preview; failing loudly is the point"
)]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use zicade_config::{
    AuthConfig, AuthMode, Config, CorpNetworkConfig, PacConfig, PacSource, RoutingMode,
};
use zicade_control::{
    ConnectionKind, ConnectionSnapshot, ConnectionState, Control, MetricsSource, StatusSnapshot,
};
use zicade_observe::{LogEvent, LogStore, channel_layer};
use zicade_ui::{MainView, Page, bind_keys};

/// Traffic that keeps moving, so rates and the chart have something to show.
#[derive(Default)]
struct SampleTraffic {
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    connections: Mutex<Vec<ConnectionSnapshot>>,
}

impl MetricsSource for SampleTraffic {
    fn total_requests(&self) -> u64 {
        1_284
    }
    fn active_connections(&self) -> usize {
        9
    }
    fn failed_requests(&self) -> u64 {
        3
    }
    fn bytes_in(&self) -> u64 {
        self.bytes_in.load(Ordering::Relaxed)
    }
    fn bytes_out(&self) -> u64 {
        self.bytes_out.load(Ordering::Relaxed)
    }
    fn connections(&self) -> Vec<ConnectionSnapshot> {
        self.connections.lock().unwrap().clone()
    }
}

fn connection(
    id: u64,
    target: &str,
    route: &str,
    kind: ConnectionKind,
    state: ConnectionState,
    secs: u64,
) -> ConnectionSnapshot {
    ConnectionSnapshot {
        id,
        kind,
        target: target.to_owned(),
        route: route.to_owned(),
        client: Some(SocketAddr::from(([127, 0, 0, 1], 52_000 + id as u16))),
        opened_at: SystemTime::now(),
        duration: Duration::from_secs(secs),
        bytes_up: 1_500 * id,
        bytes_down: 48_000 * id * id,
        state,
    }
}

fn sample_connections() -> Vec<ConnectionSnapshot> {
    let get = |status| ConnectionKind::Http {
        method: "GET".to_owned(),
        status,
    };
    vec![
        connection(
            12,
            "teams.microsoft.com:443",
            "wp8080:8080",
            ConnectionKind::Tunnel,
            ConnectionState::Open,
            412,
        ),
        connection(
            11,
            "github.com:443",
            "direct",
            ConnectionKind::Tunnel,
            ConnectionState::Open,
            95,
        ),
        connection(
            10,
            "outlook.office365.com:443",
            "wp8080:8080",
            ConnectionKind::Tunnel,
            ConnectionState::Open,
            1_830,
        ),
        connection(
            9,
            "crates.io:443",
            "direct",
            ConnectionKind::Tunnel,
            ConnectionState::Connecting,
            0,
        ),
        connection(
            8,
            "intranet.corp.example:80",
            "direct",
            get(Some(200)),
            ConnectionState::Closed,
            1,
        ),
        connection(
            7,
            "static.example.net:443",
            "wp8080:8080",
            ConnectionKind::Tunnel,
            ConnectionState::Failed("upstream CONNECT failed: 407 after 3 legs".to_owned()),
            2,
        ),
        connection(
            6,
            "update.example.com:80",
            "direct",
            get(Some(304)),
            ConnectionState::Closed,
            0,
        ),
        connection(
            5,
            "api.example.com:443",
            "wp8080:8080",
            ConnectionKind::Tunnel,
            ConnectionState::Closed,
            64,
        ),
    ]
}

fn sample_config() -> Config {
    let mut config = Config::default();
    config.routing.mode = RoutingMode::Pac;
    config.routing.pac = Some(PacConfig {
        source: PacSource::Auto,
        auth: AuthConfig {
            mode: AuthMode::Negotiate,
            ..AuthConfig::default()
        },
        ..PacConfig::default()
    });
    config.routing.corp_network = Some(CorpNetworkConfig {
        enabled: true,
        dns_suffixes: vec!["corp.example".to_owned()],
        poll_seconds: 30,
    });
    config
}

fn sample_logs(logs: &LogStore) {
    let lines = [
        ("INFO", "zicade", "zicade started in tray mode"),
        (
            "INFO",
            "zicade::netmon",
            "on the corporate network; using PAC routing",
        ),
        ("INFO", "zicade_proxy::tunnel", "tunnel established"),
        (
            "WARN",
            "zicade_proxy::http_forward",
            "upstream CONNECT failed",
        ),
        (
            "DEBUG",
            "zicade_proxy::upstream",
            "upstream HTTP-forward complete",
        ),
        (
            "ERROR",
            "zicade_routing::pac",
            "PAC download timed out after 10s",
        ),
        ("INFO", "zicade_proxy::tunnel", "tunnel closed"),
    ];
    let start = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    for (i, (level, target, message)) in lines.into_iter().enumerate() {
        let mut fields = BTreeMap::new();
        if message.starts_with("tunnel") {
            fields.insert("target".to_owned(), "github.com:443".to_owned());
        }
        logs.push(LogEvent {
            seq: 0,
            timestamp: start - 60_000 + i as u64 * 7_000,
            level: level.to_owned(),
            target: target.to_owned(),
            message: message.to_owned(),
            fields,
        });
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let page = match args.next().as_deref() {
        Some("connections") => Page::Connections,
        Some("logs") => Page::Logs,
        Some("settings") => Page::Settings,
        _ => Page::Overview,
    };
    let mode = match args.next().as_deref() {
        Some("light") => Some(ThemeMode::Light),
        Some("dark") => Some(ThemeMode::Dark),
        _ => None,
    };

    let path = std::env::temp_dir().join("zicade-preview-config.json");
    let config = sample_config();
    zicade_config::save_file(&path, &config).unwrap();
    let (_layer, logs) = channel_layer(256);
    sample_logs(&logs);
    let traffic = Arc::new(SampleTraffic::default());
    *traffic.connections.lock().unwrap() = sample_connections();
    let ticking = Arc::clone(&traffic);
    std::thread::spawn(move || {
        let mut tick = 0u64;
        loop {
            tick = tick.wrapping_add(1);
            let wave = tick % 20;
            ticking
                .bytes_in
                .fetch_add(180_000 + wave * 40_000, Ordering::Relaxed);
            ticking
                .bytes_out
                .fetch_add(12_000 + wave * 1_500, Ordering::Relaxed);
            for c in ticking.connections.lock().unwrap().iter_mut() {
                if c.state == ConnectionState::Open {
                    c.bytes_down += 20_000 * c.id;
                    c.bytes_up += 900;
                    c.duration += Duration::from_millis(250);
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
    let control = Control::new(config, path, logs).with_metrics_source(traffic);
    control.set_status(StatusSnapshot {
        routing_mode: "pac".to_owned(),
        listen_addr: "127.0.0.1:3129".to_owned(),
        on_corp: Some(true),
        ..StatusSnapshot::default()
    });

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx: &mut App| {
            init(cx);
            bind_keys(cx);
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Zicade".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1100.), px(760.)),
                    cx,
                ))),
                ..Default::default()
            };
            let (_, view) = open_window(options, cx, |window, cx| {
                cx.new(|cx| MainView::new(Ok(control), window, cx))
            })
            .expect("open the preview window");
            view.update(cx, |view, cx| view.show(page, cx));
            if let Some(mode) = mode {
                Theme::change(mode, None, cx);
            }
            cx.activate(true);
        });
}
