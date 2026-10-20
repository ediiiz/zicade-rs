//! Saving settings: offered only when something changed, reachable from the
//! keyboard, and undoable before it happens.

mod common;

use common::{WAIT, fixture, label, open};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use zicade_config::load_file;

#[gpui_kit::test]
fn nothing_to_save_until_something_changes(cx: &mut TestAppContext) {
    let f = fixture();
    let handle = open(Ok(f.control.clone()), cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-4", cx);
        window.click("save", cx);
        assert_ne!(
            label(window, "save-outcome").as_deref(),
            Some("Saved and applied."),
            "an unchanged form must not save"
        );

        window.click("listen-host", cx);
        window.input("0", cx);
        assert_eq!(
            label(window, "save-outcome").as_deref(),
            Some("Unsaved changes")
        );
    })
    .unwrap();

    assert!(f.applied.lock().unwrap().is_none());
}

#[gpui_kit::test]
async fn ctrl_s_saves_from_anywhere_in_the_window(cx: &mut TestAppContext) {
    let f = fixture();
    let handle = open(Ok(f.control.clone()), cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-4", cx);
        window.click("listen-host", cx);
        window.input("0", cx);
        window.press("ctrl-s", cx);
    })
    .unwrap();
    cx.wait_for(handle, WAIT, |window, _| {
        window
            .try_find("save-outcome")
            .is_some_and(|outcome| outcome.label() == Some("Saved and applied."))
    })
    .await;

    assert_eq!(load_file(&f.path).unwrap().listen.host, "127.0.0.10");
}

#[gpui_kit::test]
fn discard_changes_puts_back_what_the_proxy_runs_with(cx: &mut TestAppContext) {
    let f = fixture();
    let handle = open(Ok(f.control.clone()), cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-4", cx);
        window.click("listen-port", cx);
        window.input("7", cx);
        window.click("reload", cx);
        assert_eq!(window.find("listen-port").value(), Some("3229"));
        assert_ne!(
            label(window, "save-outcome").as_deref(),
            Some("Unsaved changes")
        );
    })
    .unwrap();
}
