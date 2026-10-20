//! The UI loop: one window at a time, opened or brought forward on request,
//! and an app that outlives it (the proxy keeps running in the tray).

use gpui_kit::*;
use tokio::sync::mpsc::UnboundedReceiver;
use zicade_control::Control;

use crate::views::MainView;
use crate::{Launch, UiCommand};

/// Run the gpui loop until [`UiCommand::Quit`].
pub(crate) fn run(launch: Launch) {
    let Launch {
        control,
        show_window,
        commands,
        reduce_motion,
    } = launch;
    application()
        .with_assets(crate::assets::Assets)
        // The default on Windows quits when the last window closes, which
        // would take the tray app down with the window.
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx: &mut App| {
            init(cx);
            cx.set_reduce_motion(reduce_motion);
            crate::views::bind_keys(cx);
            let mut main = MainWindow::new(control);
            if show_window {
                main.show(cx);
            }
            cx.spawn(async move |cx| follow_tray(commands, main, cx).await)
                .detach();
        })
}

/// Carry out the tray's commands until [`UiCommand::Quit`] (which quits the
/// app) or until the tray side goes away.
pub async fn follow_tray(
    mut commands: UnboundedReceiver<UiCommand>,
    mut main: MainWindow,
    cx: &mut AsyncApp,
) {
    while let Some(command) = commands.recv().await {
        match command {
            UiCommand::Show => cx.update(|cx| main.show(cx)),
            UiCommand::Quit => {
                cx.update(|cx| cx.quit());
                return;
            }
        }
    }
}

/// The app's single main window, which may be closed and reopened.
pub struct MainWindow {
    control: Result<Control, String>,
    handle: Option<AnyWindowHandle>,
}

impl MainWindow {
    /// Nothing is opened until [`MainWindow::show`].
    pub fn new(control: Result<Control, String>) -> Self {
        Self {
            control,
            handle: None,
        }
    }

    /// Bring the window forward, or open a fresh one if it was closed.
    pub fn show(&mut self, cx: &mut App) {
        if let Some(handle) = self.handle
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        self.handle = self.open(cx);
    }

    /// The window last opened, if any (it may since have been closed).
    pub fn handle(&self) -> Option<AnyWindowHandle> {
        self.handle
    }

    fn open(&self, cx: &mut App) -> Option<AnyWindowHandle> {
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Zicade".into()),
                ..Default::default()
            }),
            // Window geometry is platform-facing, so it is in pixels. The
            // minimum keeps the sidebar plus every Connections column usable.
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1100.), px(760.)),
                cx,
            ))),
            window_min_size: Some(size(px(880.), px(560.))),
            ..Default::default()
        };
        let control = self.control.clone();
        match open_window(options, cx, move |window, cx| {
            cx.new(|cx| MainView::new(control, window, cx))
        }) {
            Ok((handle, _)) => {
                cx.activate(true);
                Some(handle)
            }
            Err(err) => {
                tracing::error!(error = %format!("{err:#}"), "could not open the Zicade window");
                None
            }
        }
    }
}
