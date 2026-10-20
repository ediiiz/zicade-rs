//! The first thing the window shows: is the proxy routing the way it should,
//! how much is flowing, and what is in flight. Refreshed once a second.

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::chart::AreaChart;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Icon, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::*;
use zicade_control::{ConnectionSnapshot, ConnectionState, Control, StatusSnapshot};

mod parts;

use self::parts::{routing_sentence, section_title, total};
use super::Page;
use super::connections::kind_label;
use crate::connection_rates::ConnectionRates;
use crate::throughput::{Throughput, format_bytes, format_bytes_per_sec, format_count};

/// Seconds of throughput history the chart shows.
const HISTORY_SECONDS: usize = 60;
const REFRESH: Duration = Duration::from_secs(1);
/// Open connections listed before "All connections" takes over.
const RECENT: usize = 6;

/// Asks the window to show another page.
pub(crate) struct Navigate(pub Page);

pub(crate) struct OverviewView {
    control: Control,
    status: StatusSnapshot,
    routing: SharedString,
    throughput: Throughput,
    open: Vec<ConnectionSnapshot>,
    open_count: usize,
    rates: ConnectionRates,
    started: Instant,
}

impl EventEmitter<Navigate> for OverviewView {}

/// One chart point: (in, out) bytes per second, labelled by age.
#[derive(Clone)]
struct Point {
    label: SharedString,
    rx: f64,
    tx: f64,
}

impl OverviewView {
    pub(crate) fn new(control: Control, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            control,
            status: StatusSnapshot::default(),
            routing: SharedString::default(),
            throughput: Throughput::new(HISTORY_SECONDS),
            open: Vec::new(),
            open_count: 0,
            rates: ConnectionRates::default(),
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
        self.routing = routing_sentence(&self.control, &self.status).into();
        let at = self.started.elapsed();
        self.throughput
            .sample(self.status.bytes_in, self.status.bytes_out, at);
        let all = self.control.connections();
        self.rates.update(&all, at);
        let open: Vec<_> = all
            .into_iter()
            .filter(|c| matches!(c.state, ConnectionState::Open | ConnectionState::Connecting))
            .collect();
        self.open_count = open.len();
        self.open = open.into_iter().take(RECENT).collect();
    }

    fn summary(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_6()
            .p_4()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().size_2().rounded_full().bg(theme.success))
                            .child(div().text_lg().font_semibold().child("Running")),
                    )
                    .child(
                        div()
                            .id("stat-routing")
                            .test_support()
                            .aria_label(self.routing.clone())
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(self.routing.clone()),
                    ),
            )
            .child(
                v_flex()
                    .items_end()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Proxy address"),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                div()
                                    .id("stat-listen")
                                    .test_support()
                                    .aria_label(self.status.listen_addr.clone())
                                    .font_family(theme.mono_font_family.clone())
                                    .text_sm()
                                    .child(self.status.listen_addr.clone()),
                            )
                            .child(
                                Clipboard::new("copy-address")
                                    .value(self.status.listen_addr.clone())
                                    .tooltip("Copy proxy address")
                                    .accessibility_label("Copy proxy address"),
                            ),
                    ),
            )
    }

    fn throughput(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let (rate_in, rate_out) = self.throughput.latest().unwrap_or_default();
        let rate = |icon: IconName, color: Hsla, text: String| {
            h_flex()
                .gap_1()
                .child(Icon::new(icon).small().text_color(color))
                .child(div().text_sm().font_medium().child(text))
        };
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_4()
                    .child(section_title("Throughput"))
                    .child(div().flex_1())
                    .child(rate(
                        IconName::ArrowDown,
                        theme.chart_1,
                        format_bytes_per_sec(rate_in),
                    ))
                    .child(rate(
                        IconName::ArrowUp,
                        theme.chart_2,
                        format_bytes_per_sec(rate_out),
                    )),
            )
            .child(self.chart(cx))
    }

    fn chart(&self, cx: &App) -> AnyElement {
        let history = self.throughput.history();
        if history.len() < 2 {
            return div()
                .h_48()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Measuring…")
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
            .h_48()
            .w_full()
            .child(
                AreaChart::new(points)
                    .x(|p: &Point| p.label.clone())
                    .y(|p: &Point| p.rx)
                    .stroke(theme.chart_1)
                    .fill(theme.chart_1.opacity(0.15))
                    .y(|p: &Point| p.tx)
                    .stroke(theme.chart_2)
                    .fill(theme.chart_2.opacity(0.15))
                    .linear()
                    .tick_margin(15)
                    .y_tick_format(|v: f64| SharedString::from(format_bytes_per_sec(v)))
                    .appear(false),
            )
            .into_any_element()
    }

    /// One strip of totals rather than a field of equally loud cards.
    fn totals(&self, cx: &App) -> impl IntoElement {
        let s = &self.status;
        let theme = cx.theme();
        let failed_color = (s.requests_failed > 0).then_some(theme.danger);
        h_flex()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .child(total(
                "stat-requests",
                "Requests",
                format_count(s.requests_total),
                None,
                true,
                cx,
            ))
            .child(total(
                "stat-failed",
                "Failed",
                format_count(s.requests_failed),
                failed_color,
                false,
                cx,
            ))
            .child(total(
                "stat-active",
                "Client connections",
                format_count(s.active_connections as u64),
                None,
                false,
                cx,
            ))
            .child(total(
                "stat-downloaded",
                "Received",
                format_bytes(s.bytes_in),
                None,
                false,
                cx,
            ))
            .child(total(
                "stat-uploaded",
                "Sent",
                format_bytes(s.bytes_out),
                None,
                false,
                cx,
            ))
    }

    fn in_flight(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let rows: Vec<AnyElement> = self
            .open
            .iter()
            .map(|c| {
                let (up, down) = self.rates.rate(c.id).unwrap_or_default();
                h_flex()
                    .id(("in-flight", c.id as usize))
                    .py_1p5()
                    .gap_3()
                    .text_sm()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(div().flex_1().min_w_0().truncate().child(c.target.clone()))
                    .child(div().w_20().text_color(muted).child(kind_label(&c.kind)))
                    .child(
                        div()
                            .w_40()
                            .truncate()
                            .text_color(muted)
                            .child(c.route.clone()),
                    )
                    .child(div().w_40().text_right().child(format!(
                        "↓ {}  ↑ {}",
                        format_bytes_per_sec(down),
                        format_bytes_per_sec(up)
                    )))
                    .into_any_element()
            })
            .collect();
        let header = h_flex()
            .gap_2()
            .child(section_title("In flight"))
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(format_count(self.open_count as u64)),
            )
            .child(div().flex_1())
            .child(
                Button::new("all-connections")
                    .ghost()
                    .small()
                    .label("All connections")
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                        cx.emit(Navigate(Page::Connections));
                    })),
            );
        let body = if rows.is_empty() {
            div()
                .py_3()
                .text_sm()
                .text_color(muted)
                .child("Nothing in flight right now.")
                .into_any_element()
        } else {
            v_flex().children(rows).into_any_element()
        };
        v_flex().gap_2().child(header).child(body)
    }
}

impl Render for OverviewView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = v_flex()
            .px_6()
            .pb_6()
            .gap_6()
            .child(self.summary(cx))
            .child(self.throughput(cx))
            .child(self.totals(cx))
            .child(self.in_flight(cx));
        div().size_full().overflow_y_scrollbar().child(content)
    }
}
