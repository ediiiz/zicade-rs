//! The real window, driven headlessly: real views over a real [`Control`]
//! with a config file on disk, operated through hit-tested clicks and typed
//! text. No OS input is synthesized, so nothing reaches other applications.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, ElementId, Point, TestAppContext, Window, WindowBounds,
    WindowOptions, init, open_window, px, size,
};
use zicade_config::{Config, ListenConfig, RoutingMode, load_file};
use zicade_control::{ConfigApplyHook, Control, StatusSnapshot};
use zicade_observe::{LogEvent, LogStore, channel_layer};
use zicade_ui::{MainView, MainWindow, UiCommand, follow_tray};

const WAIT: Duration = Duration::from_secs(5);

fn temp_config_path() -> PathBuf {
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

struct Fixture {
    control: Control,
    path: PathBuf,
    logs: LogStore,
    applied: Arc<Mutex<Option<Config>>>,
}

fn fixture() -> Fixture {
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
    let control = Control::new(config, path.clone(), logs.clone()).with_apply_hook(hook);
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
    }
}

fn log(seq: u64, message: &str) -> LogEvent {
    LogEvent {
        seq,
        timestamp: 0,
        level: "INFO".to_owned(),
        target: "zicade".to_owned(),
        message: message.to_owned(),
        fields: Default::default(),
    }
}

fn open(control: Result<Control, String>, cx: &mut TestAppContext) -> AnyWindowHandle {
    cx.update(init);
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

fn label(window: &Window, id: impl Into<ElementId>) -> Option<String> {
    window.find(id).label().map(str::to_owned)
}

#[gpui_kit::test]
fn the_status_page_shows_the_running_proxy(cx: &mut TestAppContext) {
    let f = fixture();
    let handle = open(Ok(f.control.clone()), cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(label(window, "stat-routing").as_deref(), Some("direct"));
        assert_eq!(
            label(window, "stat-listen").as_deref(),
            Some("127.0.0.1:3229")
        );
        assert_eq!(label(window, "stat-requests").as_deref(), Some("0"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn a_saved_edit_is_validated_written_and_applied(cx: &mut TestAppContext) {
    let f = fixture();
    let handle = open(Ok(f.control.clone()), cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.within("pages").click(1usize, cx);
        // direct -> upstream: the second option.
        window.within("routing-mode").click("input", cx);
        window.press("down", cx);
        window.press("enter", cx);
    })
    .unwrap();
    // The upstream section appears once the select commits.
    cx.wait_for(handle, WAIT, |window, _| {
        window.try_find("upstream-host").is_some()
    })
    .await;
    cx.update_window(handle, |_, window, cx| {
        window.click("upstream-host", cx);
        window.input("proxy.test.invalid", cx);
        window.click("upstream-port", cx);
        window.input("8080", cx);
        window.click("save", cx);
    })
    .unwrap();
    cx.wait_for(handle, WAIT, |window, _| {
        window
            .try_find("save-outcome")
            .is_some_and(|outcome| outcome.label() == Some("Saved and applied."))
    })
    .await;

    let on_disk = load_file(&f.path).unwrap();
    assert_eq!(on_disk.routing.mode, RoutingMode::Upstream);
    let upstream = on_disk.routing.upstream.unwrap();
    assert_eq!(
        (upstream.host.as_str(), upstream.port),
        ("proxy.test.invalid", 8080)
    );
    let applied = f.applied.lock().unwrap().clone();
    assert_eq!(
        applied.map(|c| c.routing.mode),
        Some(RoutingMode::Upstream),
        "the running proxy must get the new routing"
    );
    assert_eq!(f.control.status().routing_mode, "upstream");
}

#[gpui_kit::test]
fn an_unparseable_field_is_reported_and_nothing_is_saved(cx: &mut TestAppContext) {
    let f = fixture();
    let handle = open(Ok(f.control.clone()), cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.within("pages").click(1usize, cx);
        window.click("listen-port", cx);
        window.input("x", cx);
        window.click("save", cx);
        assert_eq!(
            label(window, "save-outcome").as_deref(),
            Some("Listen port: must be a number from 1 to 65535")
        );
    })
    .unwrap();

    assert_eq!(load_file(&f.path).unwrap().listen.port, 3229);
    assert!(f.applied.lock().unwrap().is_none());
}

#[gpui_kit::test]
async fn the_logs_page_shows_the_backlog_then_live_lines(cx: &mut TestAppContext) {
    let f = fixture();
    f.logs.push(log(0, "from before the window opened"));
    let handle = open(Ok(f.control.clone()), cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.within("pages").click(2usize, cx);
    })
    .unwrap();
    f.logs.push(log(0, "arrived while watching"));

    cx.wait_for(handle, WAIT, |window, _| {
        window
            .try_find(("log-line", 1usize))
            .and_then(|line| line.label().map(str::to_owned))
            .is_some_and(|text| text.ends_with("arrived while watching"))
    })
    .await;
    cx.update_window(handle, |_, window, _| {
        let first = label(window, ("log-line", 0usize)).unwrap_or_default();
        assert!(first.ends_with("from before the window opened"), "{first}");
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_failed_start_shows_why_instead_of_the_pages(cx: &mut TestAppContext) {
    let handle = open(Err("failed to bind the proxy listener".to_owned()), cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            label(window, "start-error").as_deref(),
            Some("failed to bind the proxy listener")
        );
        assert!(window.try_find("pages").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn show_opens_one_window_and_reopens_it_after_a_close(cx: &mut TestAppContext) {
    let f = fixture();
    cx.update(init);
    let mut main = MainWindow::new(Ok(f.control.clone()));

    cx.update(|cx| main.show(cx));
    cx.update(|cx| main.show(cx));
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        1,
        "Show twice = one window"
    );

    let first = main.handle().unwrap();
    cx.update_window(first, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);

    cx.update(|cx| main.show(cx));
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        1,
        "closed window reopens"
    );
}

#[gpui_kit::test]
async fn tray_commands_open_the_window_and_quit_ends_the_loop(cx: &mut TestAppContext) {
    let f = fixture();
    cx.update(init);
    let (tray, commands) = tokio::sync::mpsc::unbounded_channel();
    let main = MainWindow::new(Ok(f.control.clone()));
    let following = cx.spawn(|mut cx| async move { follow_tray(commands, main, &mut cx).await });

    tray.send(UiCommand::Show).unwrap();
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        1,
        "Open Zicade opens the window"
    );

    tray.send(UiCommand::Quit).unwrap();
    following.await;
}
