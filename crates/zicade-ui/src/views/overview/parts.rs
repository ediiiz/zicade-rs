//! Small pieces of the overview: section titles, the totals strip's cells,
//! and the routing sentence.

use gpui_kit::component::{ActiveTheme, StyledExt, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zicade_config::RoutingMode;
use zicade_control::{Control, StatusSnapshot};

pub(super) fn section_title(text: &'static str) -> impl IntoElement {
    div().text_sm().font_semibold().child(text)
}

/// One cell of the totals strip; its value carries `id` and an accessible
/// label so screen readers (and the window tests) can read it.
pub(super) fn total(
    id: &'static str,
    label: &'static str,
    value: String,
    color: Option<Hsla>,
    leading: bool,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .flex_1()
        .min_w_0()
        .px_4()
        .py_3()
        .gap_1()
        .when(!leading, |this| this.border_l_1())
        .border_color(theme.border)
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(
            div()
                .id(id)
                .test_support()
                .aria_label(value.clone())
                .text_lg()
                .font_semibold()
                .text_color(color.unwrap_or(theme.foreground))
                .truncate()
                .child(value),
        )
}

/// How traffic is routed right now, in words.
pub(super) fn routing_sentence(control: &Control, status: &StatusSnapshot) -> String {
    let config = control.config_redacted();
    let route = match config.routing.mode {
        RoutingMode::Direct => "Connecting to sites directly".to_owned(),
        RoutingMode::Upstream => match &config.routing.upstream {
            Some(upstream) => format!("Through {}:{}", upstream.host, upstream.port),
            None => "Through the upstream proxy".to_owned(),
        },
        RoutingMode::Pac => "Routing each request with the PAC script".to_owned(),
    };
    match status.on_corp {
        Some(true) => format!("{route}, on the corporate network"),
        Some(false) => "Off the corporate network, so connecting directly".to_owned(),
        None => route,
    }
}
