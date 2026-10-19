//! Every request and tunnel the proxy is handling, plus the recently finished
//! ones, with live per-connection rates. Refreshed once a second.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zicade_control::{ConnectionKind, ConnectionSnapshot, ConnectionState, Control};

use crate::connection_rates::ConnectionRates;
use crate::throughput::{format_bytes, format_bytes_per_sec, format_duration};

const REFRESH: Duration = Duration::from_secs(1);
const ROW_HEIGHT: f32 = 26.;

/// Column widths; the target column takes the rest.
const STATE_W: f32 = 120.;
const KIND_W: f32 = 90.;
const ROUTE_W: f32 = 170.;
const TIME_W: f32 = 70.;
const BYTES_W: f32 = 80.;
const RATE_W: f32 = 170.;

pub(crate) struct ConnectionsView {
    control: Control,
    all: Vec<ConnectionSnapshot>,
    rates: ConnectionRates,
    started: Instant,
    show_finished: bool,
}

impl ConnectionsView {
    pub(crate) fn new(control: Control, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            control,
            all: Vec::new(),
            rates: ConnectionRates::default(),
            started: Instant::now(),
            show_finished: true,
        };
        view.refresh();
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
        self.all = self.control.connections();
        self.rates.update(&self.all, self.started.elapsed());
    }

    fn shown(&self) -> Vec<&ConnectionSnapshot> {
        self.all
            .iter()
            .filter(|c| self.show_finished || is_live(c))
            .collect()
    }

    fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let colors = Colors {
            open: theme.success,
            failed: theme.danger,
            muted: theme.muted_foreground,
            normal: theme.foreground,
            stripe: theme.list_even,
        };
        self.shown()
            .into_iter()
            .enumerate()
            .skip(range.start)
            .take(range.len())
            .map(|(index, c)| row(c, self.rates.rate(c.id), index, &colors))
            .collect()
    }
}

impl Render for ConnectionsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.all.iter().filter(|c| is_live(c)).count();
        let summary = format!("{open} open, {} finished", self.all.len() - open);
        let count = self.shown().len();
        let header = h_flex()
            .gap_4()
            .child(
                div()
                    .id("connection-summary")
                    .test_support()
                    .aria_label(summary.clone())
                    .text_sm()
                    .font_semibold()
                    .child(summary),
            )
            .child(div().flex_1())
            .child(
                Switch::new("show-finished")
                    .checked(self.show_finished)
                    .label("Show finished")
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.show_finished = *on;
                        cx.notify();
                    })),
            );
        let body = if count == 0 {
            div()
                .p_4()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No connections yet. Point a browser or app at the proxy to see them here.")
                .into_any_element()
        } else {
            uniform_list(
                "connections",
                count,
                cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
            )
            .size_full()
            .into_any_element()
        };
        v_flex()
            .size_full()
            .px_4()
            .py_3()
            .gap_2()
            .child(header)
            .child(column_titles(cx.theme().muted_foreground))
            .child(div().flex_1().min_h_0().child(body))
    }
}

struct Colors {
    open: Hsla,
    failed: Hsla,
    muted: Hsla,
    normal: Hsla,
    stripe: Hsla,
}

fn is_live(c: &ConnectionSnapshot) -> bool {
    matches!(c.state, ConnectionState::Connecting | ConnectionState::Open)
}

fn column_titles(color: Hsla) -> impl IntoElement {
    h_flex()
        .px_2()
        .gap_2()
        .text_xs()
        .text_color(color)
        .child(cell(STATE_W, "State"))
        .child(cell(KIND_W, "Kind"))
        .child(div().flex_1().child("Target"))
        .child(cell(ROUTE_W, "Route"))
        .child(cell(TIME_W, "Time"))
        .child(cell(BYTES_W, "↑ Sent"))
        .child(cell(BYTES_W, "↓ Received"))
        .child(cell(RATE_W, "Now"))
}

fn cell(width: f32, text: impl Into<SharedString>) -> Div {
    div().w(px(width)).flex_none().truncate().child(text.into())
}

fn row(
    c: &ConnectionSnapshot,
    rate: Option<(f64, f64)>,
    index: usize,
    colors: &Colors,
) -> AnyElement {
    let (state, state_color) = match &c.state {
        ConnectionState::Connecting => ("Connecting", colors.muted),
        ConnectionState::Open => ("Open", colors.open),
        ConnectionState::Closed => ("Closed", colors.muted),
        ConnectionState::Failed(_) => ("Failed", colors.failed),
    };
    let reason = match &c.state {
        ConnectionState::Failed(reason) => Some(reason.clone()),
        _ => None,
    };
    let now = match rate {
        Some((up, down)) if is_live(c) => {
            format!(
                "↓ {}  ↑ {}",
                format_bytes_per_sec(down),
                format_bytes_per_sec(up)
            )
        }
        _ => String::new(),
    };
    let target = div()
        .flex_1()
        .min_w_0()
        .truncate()
        .child(c.target.clone())
        .children(reason.clone().map(|reason| {
            div()
                .text_color(colors.failed)
                .child(SharedString::from(format!(" — {reason}")))
        }));
    h_flex()
        .id(("connection", c.id as usize))
        .test_support()
        .aria_label(describe(c))
        .h(px(ROW_HEIGHT))
        .px_2()
        .gap_2()
        .text_sm()
        .text_color(if is_live(c) {
            colors.normal
        } else {
            colors.muted
        })
        .when(index % 2 == 1, |this| this.bg(colors.stripe))
        .child(cell(STATE_W, state).text_color(state_color))
        .child(cell(KIND_W, kind_label(&c.kind)))
        .child(h_flex().flex_1().min_w_0().child(target))
        .child(cell(ROUTE_W, c.route.clone()))
        .child(cell(TIME_W, format_duration(c.duration)))
        .child(cell(BYTES_W, format_bytes(c.bytes_up)))
        .child(cell(BYTES_W, format_bytes(c.bytes_down)))
        .child(cell(RATE_W, now))
        .into_any_element()
}

/// `HTTPS` for a tunnel, `GET 200` for a plain-HTTP request.
fn kind_label(kind: &ConnectionKind) -> String {
    match kind {
        ConnectionKind::Tunnel => "HTTPS".to_owned(),
        ConnectionKind::Http {
            method,
            status: Some(status),
        } => format!("{method} {status}"),
        ConnectionKind::Http { method, .. } => method.clone(),
    }
}

/// One sentence per row for screen readers (and the window tests).
fn describe(c: &ConnectionSnapshot) -> String {
    let state = match &c.state {
        ConnectionState::Connecting => "Connecting".to_owned(),
        ConnectionState::Open => "Open".to_owned(),
        ConnectionState::Closed => "Closed".to_owned(),
        ConnectionState::Failed(reason) => format!("Failed: {reason}"),
    };
    format!(
        "{state}, {} {} via {}, up {}, down {}",
        kind_label(&c.kind),
        c.target,
        c.route,
        format_bytes(c.bytes_up),
        format_bytes(c.bytes_down)
    )
}
