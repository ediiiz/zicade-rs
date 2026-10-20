//! Starting in the background defers the UI toolkit until the window is first
//! asked for, so an idle tray app does not carry its memory.

use tokio::sync::mpsc::unbounded_channel;
use zicade_ui::{UiCommand, wait_for_show};

#[test]
fn the_first_show_starts_the_ui() {
    let (tray, mut commands) = unbounded_channel();
    tray.send(UiCommand::Show).unwrap();
    assert!(wait_for_show(&mut commands));
}

#[test]
fn quit_before_any_show_never_starts_the_ui() {
    let (tray, mut commands) = unbounded_channel();
    tray.send(UiCommand::Quit).unwrap();
    assert!(!wait_for_show(&mut commands));
}

#[test]
fn a_vanished_tray_never_starts_the_ui() {
    let (tray, mut commands) = unbounded_channel::<UiCommand>();
    drop(tray);
    assert!(!wait_for_show(&mut commands));
}
