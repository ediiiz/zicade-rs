//! The first thing the window shows: is the proxy routing the way it should,
//! how much is flowing, and what is in flight. Sampled once a second; the
//! chart animates between samples unless the system asks for less motion.

use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::*;
use zicade_control::{ConnectionSnapshot, ConnectionState, Control, StatusSnapshot};

mod chart;
mod parts;

use self::parts::{routing_sentence, section_title, total};
use super::Page;
use super::connections::kind_label;
use crate::chart_model::WINDOW;
use crate::connection_rates::ConnectionRates;
use crate::throughput::{Throughput, format_bytes, format_bytes_per_sec, format_count};

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
    /// When the newest throughput sample was taken; the chart glides from
    /// the previous sample towards it over the next second.
    sampled: Instant,
}

impl EventEmitter<Navigate> for OverviewView {}

impl OverviewView {
    pub(crate) fn new(control: Control, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            control,
            status: StatusSnapshot::default(),
            routing: SharedString::default(),
            // One more than the chart shows: it glides from the oldest.
            throughput: Throughput::new(WINDOW + 1),
            open: Vec::new(),
            open_count: 0,
            rates: ConnectionRates::default(),
            started: Instant::now(),
            sampled: Instant::now(),
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
        self.sampled = Instant::now();
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Glide between samples frame by frame; with reduced motion, show
        // each sample as it lands.
        let progress = if cx.reduce_motion() {
            1.0
        } else {
            window.request_animation_frame();
            self.sampled.elapsed().as_secs_f64() / REFRESH.as_secs_f64()
        };
        let content = v_flex()
            .px_6()
            .pb_6()
            .gap_6()
            .child(self.summary(cx))
            .child(chart::throughput(&self.throughput.history(), progress, cx))
            .child(self.totals(cx))
            .child(self.in_flight(cx));
        div().size_full().overflow_y_scrollbar().child(content)
    }
}
