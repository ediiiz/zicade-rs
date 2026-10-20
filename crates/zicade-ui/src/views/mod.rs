//! The window: a persistent sidebar beside the current page.

mod config;
mod connections;
mod logs;
mod overview;
mod sidebar;

use gpui_kit::component::{ActiveTheme, StyledExt, Theme, h_flex, v_flex};
use gpui_kit::*;
use zicade_control::Control;

use self::config::ConfigView;
use self::connections::ConnectionsView;
use self::logs::LogsView;
use self::overview::{Navigate, OverviewView};

actions!(
    zicade,
    [
        ShowOverview,
        ShowConnections,
        ShowLogs,
        ShowSettings,
        SaveSettings
    ]
);

/// Register the window's keyboard shortcuts. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-1", ShowOverview, None),
        KeyBinding::new("ctrl-2", ShowConnections, None),
        KeyBinding::new("ctrl-3", ShowLogs, None),
        KeyBinding::new("ctrl-4", ShowSettings, None),
        KeyBinding::new("ctrl-s", SaveSettings, None),
    ]);
}

/// The window's destinations, in sidebar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    Connections,
    Logs,
    Settings,
}

impl Page {
    pub(crate) const ALL: [Page; 4] = [
        Page::Overview,
        Page::Connections,
        Page::Logs,
        Page::Settings,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Page::Overview => "Overview",
            Page::Connections => "Connections",
            Page::Logs => "Logs",
            Page::Settings => "Settings",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Page::Overview => "How the proxy is routing and what is going through it.",
            Page::Connections => "Requests and tunnels through the proxy, newest first.",
            Page::Logs => "What the proxy is doing, newest at the bottom.",
            Page::Settings => "Saved changes apply to the running proxy.",
        }
    }
}

struct Pages {
    overview: Entity<OverviewView>,
    connections: Entity<ConnectionsView>,
    logs: Entity<LogsView>,
    settings: Entity<ConfigView>,
}

/// The window's root view: navigation and the current page, or the reason the
/// proxy failed to start.
pub struct MainView {
    page: Page,
    control: Option<Control>,
    pages: Result<Pages, SharedString>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl MainView {
    pub fn new(
        control: Result<Control, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Theme::sync_system_appearance(Some(window), cx);
        let mut subscriptions = vec![cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        })];
        let pages = match &control {
            Ok(control) => {
                let overview = cx.new(|cx| OverviewView::new(control.clone(), cx));
                subscriptions.push(cx.subscribe(&overview, |this, _, Navigate(page), cx| {
                    this.show(*page, cx);
                }));
                Ok(Pages {
                    overview,
                    connections: cx.new(|cx| ConnectionsView::new(control.clone(), window, cx)),
                    logs: cx.new(|cx| LogsView::new(control.logs(), window, cx)),
                    settings: cx.new(|cx| ConfigView::new(control.clone(), window, cx)),
                })
            }
            Err(reason) => Err(reason.clone().into()),
        };
        // Keyboard shortcuts work from the moment the window opens.
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            page: Page::Overview,
            control: control.ok(),
            pages,
            focus,
            _subscriptions: subscriptions,
        }
    }

    /// Switch to `page`.
    pub fn show(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        cx.notify();
    }

    /// The page on screen.
    pub fn page(&self) -> Page {
        self.page
    }

    fn save_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(pages) = &self.pages {
            let settings = pages.settings.clone();
            self.show(Page::Settings, cx);
            settings.update(cx, |settings, cx| settings.save(window, cx));
        }
    }

    fn content(&self, cx: &mut Context<Self>) -> AnyElement {
        let pages = match &self.pages {
            Ok(pages) => pages,
            Err(reason) => return failed(reason.clone(), cx).into_any_element(),
        };
        let body = match self.page {
            Page::Overview => pages.overview.clone().into_any_element(),
            Page::Connections => pages.connections.clone().into_any_element(),
            Page::Logs => pages.logs.clone().into_any_element(),
            Page::Settings => pages.settings.clone().into_any_element(),
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                v_flex()
                    .px_6()
                    .pt_5()
                    .pb_4()
                    .gap_1()
                    .child(div().text_xl().font_semibold().child(self.page.title()))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.page.description()),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

impl Focusable for MainView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MainView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = if self.pages.is_ok() {
            Some(sidebar::render(self.page, self.control.as_ref(), cx).into_any_element())
        } else {
            None
        };
        h_flex()
            .id("zicade")
            .key_context("Zicade")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ShowOverview, _, cx| this.show(Page::Overview, cx)))
            .on_action(
                cx.listener(|this, _: &ShowConnections, _, cx| this.show(Page::Connections, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowLogs, _, cx| this.show(Page::Logs, cx)))
            .on_action(cx.listener(|this, _: &ShowSettings, _, cx| this.show(Page::Settings, cx)))
            .on_action(cx.listener(|this, _: &SaveSettings, window, cx| {
                this.save_settings(window, cx);
            }))
            .items_stretch()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .children(nav)
            .child(self.content(cx))
    }
}

/// What the window shows when the proxy could not start.
fn failed(reason: SharedString, cx: &App) -> impl IntoElement {
    v_flex()
        .size_full()
        .justify_center()
        .items_center()
        .p_8()
        .gap_2()
        .child(
            div()
                .text_lg()
                .font_semibold()
                .child("Zicade couldn’t start"),
        )
        .child(
            div()
                .id("start-error")
                .test_support()
                .aria_label(reason.clone())
                .max_w_128()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(reason),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Fix the configuration file, then start Zicade again."),
        )
}
