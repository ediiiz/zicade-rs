//! The persistent navigation column: app identity, the four destinations, and
//! where the proxy is listening.

use gpui_kit::assets::IconName;
use gpui_kit::component::sidebar::{Sidebar, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::{ActiveTheme, StyledExt, h_flex, v_flex};
use gpui_kit::*;
use zicade_control::Control;

use super::{MainView, Page};

pub(super) fn render(
    current: Page,
    control: Option<&Control>,
    cx: &mut Context<MainView>,
) -> impl IntoElement {
    let items = Page::ALL.map(|page| {
        SidebarMenuItem::new(page.title())
            .icon(icon(page))
            .active(page == current)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.show(page, cx)))
    });
    Sidebar::new("nav")
        .w_56()
        .header(identity(cx))
        .child(SidebarMenu::new().children(items))
        .footer(status(control, cx))
}

fn icon(page: Page) -> IconName {
    match page {
        Page::Overview => IconName::LayoutDashboard,
        Page::Connections => IconName::ArrowLeftRight,
        Page::Logs => IconName::ScrollText,
        Page::Settings => IconName::Settings,
    }
}

fn identity(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .gap_2()
        .py_1()
        .child(
            div()
                .size_7()
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme.radius)
                .bg(theme.primary)
                .text_color(theme.primary_foreground)
                .font_semibold()
                .child("Z"),
        )
        .child(
            v_flex()
                .child(div().text_sm().font_semibold().child("Zicade"))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Local proxy"),
                ),
        )
}

/// Where the proxy listens, so it can be copied into a browser's settings.
fn status(control: Option<&Control>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let listen = control
        .map(|control| control.status().listen_addr)
        .unwrap_or_default();
    h_flex()
        .w_full()
        .gap_2()
        .py_1()
        .child(div().size_2().flex_none().rounded_full().bg(theme.success))
        .child(
            v_flex()
                .min_w_0()
                .child(div().text_xs().font_medium().child("Running"))
                .child(
                    div()
                        .id("sidebar-listen")
                        .test_support()
                        .aria_label(listen.clone())
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .child(listen),
                ),
        )
}
