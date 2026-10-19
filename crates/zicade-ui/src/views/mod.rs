//! The window's content: a header with the page tabs, and the selected page.

mod config;
mod connections;
mod logs;
mod status;

use gpui_kit::component::tab::TabBar;
use gpui_kit::component::{ActiveTheme, StyledExt, Theme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zicade_control::Control;

use self::config::ConfigView;
use self::connections::ConnectionsView;
use self::logs::LogsView;
use self::status::StatusView;

/// The pages, in tab order.
const PAGES: [&str; 4] = ["Status", "Configuration", "Logs", "Connections"];

enum Body {
    /// The proxy failed to start; there is nothing to show but why.
    Failed(SharedString),
    Running {
        status: Entity<StatusView>,
        config: Entity<ConfigView>,
        logs: Entity<LogsView>,
        connections: Entity<ConnectionsView>,
    },
}

/// The window's root view: page tabs over status, configuration, logs, and
/// connections, or the reason the proxy failed to start.
pub struct MainView {
    page: usize,
    body: Body,
    _subscriptions: Vec<Subscription>,
}

impl MainView {
    pub fn new(
        control: Result<Control, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Theme::sync_system_appearance(Some(window), cx);
        let follow_theme = cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        });
        let body = match control {
            Ok(control) => Body::Running {
                status: cx.new(|cx| StatusView::new(control.clone(), cx)),
                config: cx.new(|cx| ConfigView::new(control.clone(), window, cx)),
                logs: cx.new(|cx| LogsView::new(control.logs(), cx)),
                connections: cx.new(|cx| ConnectionsView::new(control.clone(), cx)),
            },
            Err(reason) => Body::Failed(reason.into()),
        };
        Self {
            page: 0,
            body,
            _subscriptions: vec![follow_theme],
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = TabBar::new("pages")
            .underline()
            .selected_index(self.page)
            .children(PAGES)
            .on_click(cx.listener(|this, page: &usize, _, cx| {
                this.page = *page;
                cx.notify();
            }));
        h_flex()
            .w_full()
            .px_4()
            .pt_2()
            .gap_6()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().text_lg().font_semibold().pb_2().child("Zicade"))
            .when(matches!(self.body, Body::Running { .. }), |this| {
                this.child(tabs)
            })
    }
}

impl Render for MainView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = match &self.body {
            Body::Failed(reason) => failed(reason.clone(), cx).into_any_element(),
            Body::Running {
                status,
                config,
                logs,
                connections,
            } => match self.page {
                0 => status.clone().into_any_element(),
                1 => config.clone().into_any_element(),
                2 => logs.clone().into_any_element(),
                _ => connections.clone().into_any_element(),
            },
        };
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.header(cx))
            .child(div().flex_1().min_h_0().child(page))
    }
}

/// What the window shows when the proxy could not start.
fn failed(reason: SharedString, cx: &App) -> impl IntoElement {
    v_flex()
        .size_full()
        .p_6()
        .gap_2()
        .child(
            div()
                .text_lg()
                .font_semibold()
                .text_color(cx.theme().danger)
                .child("Zicade could not start"),
        )
        .child(
            div()
                .id("start-error")
                .test_support()
                .aria_label(reason.clone())
                .text_sm()
                .child(reason),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Fix the configuration file and start Zicade again."),
        )
}
