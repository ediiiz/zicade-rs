//! The Connections page and the traffic totals, fed by a metrics source the
//! test controls.

mod common;

use std::sync::atomic::Ordering;

use common::{WAIT, connection, fixture, label, open};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use zicade_control::ConnectionState;

#[gpui_kit::test]
fn the_connections_page_lists_open_and_failed_connections(cx: &mut TestAppContext) {
    let f = fixture();
    f.traffic.show(vec![
        connection(2, "example.com:443", ConnectionState::Open),
        connection(
            1,
            "refused.example:443",
            ConnectionState::Failed("connection refused".to_owned()),
        ),
    ]);
    let handle = open(Ok(f.control.clone()), cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-2", cx);
        assert_eq!(
            label(window, ("connection", 2usize)).as_deref(),
            Some("Open, HTTPS example.com:443 via direct, up 1.0 KB, down 2.0 KB")
        );
        assert_eq!(
            label(window, ("connection", 1usize)).as_deref(),
            Some(
                "Failed: connection refused, HTTPS refused.example:443 via direct, up 1.0 KB, down 2.0 KB"
            )
        );
        assert_eq!(
            label(window, "connection-summary").as_deref(),
            Some("1 open, 1 finished")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn new_connections_and_bytes_show_up_without_reopening(cx: &mut TestAppContext) {
    let f = fixture();
    let handle = open(Ok(f.control.clone()), cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-2", cx);
    })
    .unwrap();

    f.traffic.show(vec![connection(
        9,
        "late.example:443",
        ConnectionState::Open,
    )]);
    f.traffic.bytes_in.store(1536, Ordering::Relaxed);

    cx.wait_for(handle, WAIT, |window, _| {
        window.try_find(("connection", 9usize)).is_some()
    })
    .await;
    cx.update_window(handle, |_, window, cx| {
        window.press("ctrl-1", cx);
    })
    .unwrap();
    cx.wait_for(handle, WAIT, |window, _| {
        window
            .try_find("stat-downloaded")
            .is_some_and(|stat| stat.label() == Some("1.5 KB"))
    })
    .await;
}
