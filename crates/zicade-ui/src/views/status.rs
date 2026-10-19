//! Live status: counters, routing state, and a throughput chart, refreshed
//! once a second.

use std::time::{Duration, Instant};

use gpui_kit::component::chart::AreaChart;
use gpui_kit::component::{ActiveTheme, StyledExt, h_flex, v_flex};
use gpui_kit::*;
use zicade_control::{Control, StatusSnapshot};

use crate::throughput::{Throughput, format_bytes_per_sec, format_count};

/// Seconds of throughput history the chart shows.
const HISTORY_SECONDS: usize = 60;
const REFRESH: Duration = Duration::from_secs(1);

pub(crate) struct StatusView {
    control: Control,
    status: StatusSnapshot,
    throughput: Throughput,
    started: Instant,
}

/// One chart point: (in, out) bytes per second, labelled by age.
#[derive(Clone)]
struct Point {
    label: SharedString,
    rx: f64,
    tx: f64,
}

impl StatusView {
    pub(crate) fn new(control: Control, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            status: StatusSnapshot::default(),
            control,
            throughput: Throughput::new(HISTORY_SECONDS),
            started: Instant::now(),
        };
        view.refresh();
        // The loop ends when the window (and with it this view) is gone.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH).await;
                let alive = this.update(cx, |this, cx| {
                    this.refresh();
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        view
    }

    fn refresh(&mut self) {
        self.status = self.control.status();
        self.throughput.sample(
            self.status.bytes_in,
            self.status.bytes_out,
            self.started.elapsed(),
        );
    }

    fn chart(&self, cx: &App) -> AnyElement {
        let history = self.throughput.history();
        if history.len() < 2 {
            return div()
                .h(px(220.))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Collecting throughput…")
                .into_any_element();
        }
        let newest = history.len() - 1;
        let points: Vec<Point> = history
            .iter()
            .enumerate()
            .map(|(i, (rx, tx))| Point {
                label: format!("-{}s", newest - i).into(),
                rx: *rx,
                tx: *tx,
            })
            .collect();
        let theme = cx.theme();
        div()
            .h(px(220.))
            .w_full()
            .child(
                AreaChart::new(points)
                    .x(|p: &Point| p.label.clone())
                    .y(|p: &Point| p.rx)
                    .stroke(theme.chart_1)
                    .fill(theme.chart_1.opacity(0.2))
                    .y(|p: &Point| p.tx)
                    .stroke(theme.chart_2)
                    .fill(theme.chart_2.opacity(0.2))
                    .linear()
                    .tick_margin(10)
                    .y_tick_format(|v: f64| SharedString::from(format_bytes_per_sec(v)))
                    .appear(false),
            )
            .into_any_element()
    }
}

impl Render for StatusView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.status;
        let theme = cx.theme();
        let (rate_in, rate_out) = self.throughput.latest().unwrap_or_default();
        let corp = s.on_corp.map(|on| if on { "on" } else { "off" });
        let failed_color = if s.requests_failed > 0 {
            theme.danger
        } else {
            theme.foreground
        };
        let cards = h_flex()
            .flex_wrap()
            .gap_3()
            .child(stat(
                "stat-routing",
                "Routing",
                s.routing_mode.clone(),
                theme.foreground,
                cx,
            ))
            .children(
                corp.map(|corp| stat("stat-corp", "Corporate network", corp, theme.foreground, cx)),
            )
            .child(stat(
                "stat-listen",
                "Listening on",
                s.listen_addr.clone(),
                theme.foreground,
                cx,
            ))
            .child(stat(
                "stat-requests",
                "Requests",
                format_count(s.requests_total),
                theme.foreground,
                cx,
            ))
            .child(stat(
                "stat-failed",
                "Failed",
                format_count(s.requests_failed),
                failed_color,
                cx,
            ))
            .child(stat(
                "stat-active",
                "Active connections",
                format_count(s.active_connections as u64),
                theme.foreground,
                cx,
            ))
            .child(stat(
                "stat-in",
                "↓ In",
                format_bytes_per_sec(rate_in),
                theme.chart_1,
                cx,
            ))
            .child(stat(
                "stat-out",
                "↑ Out",
                format_bytes_per_sec(rate_out),
                theme.chart_2,
                cx,
            ));
        v_flex().size_full().p_4().gap_4().child(cards).child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child("Throughput (last 60 s)"),
                )
                .child(self.chart(cx)),
        )
    }
}

/// One labelled figure. The value carries `id` and an accessible label, so
/// screen readers (and the window tests) can read it.
fn stat(
    id: &'static str,
    label: &str,
    value: impl Into<SharedString>,
    color: Hsla,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let value = value.into();
    v_flex()
        .min_w(px(150.))
        .p_3()
        .gap_1()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(label.to_owned())),
        )
        .child(
            div()
                .id(id)
                .test_support()
                .aria_label(value.clone())
                .text_lg()
                .font_semibold()
                .text_color(color)
                .child(value),
        )
}
