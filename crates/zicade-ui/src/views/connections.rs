//! Every request and tunnel the proxy is handling, plus the recently finished
//! ones, with live per-connection rates. Refreshed once a second.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::{ActiveTheme, Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zicade_control::{ConnectionKind, ConnectionSnapshot, ConnectionState, Control};

use crate::connection_rates::ConnectionRates;
use crate::throughput::{format_bytes, format_bytes_per_sec, format_duration};

const REFRESH: Duration = Duration::from_secs(1);

/// Which connections the list shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Open,
    Failed,
}

const FILTERS: [(Filter, &str); 3] = [
    (Filter::All, "All"),
    (Filter::Open, "Open"),
    (Filter::Failed, "Failed"),
];

pub(crate) struct ConnectionsView {
    control: Control,
    all: Vec<ConnectionSnapshot>,
    rates: ConnectionRates,
    started: Instant,
    filter: Filter,
    search: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl ConnectionsView {
    pub(crate) fn new(control: Control, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter by host or route"));
        let refilter = cx.subscribe_in(&search, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let mut view = Self {
            control,
            all: Vec::new(),
            rates: ConnectionRates::default(),
            started: Instant::now(),
            filter: Filter::All,
            search,
            _subscriptions: vec![refilter],
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

    fn shown(&self, cx: &App) -> Vec<&ConnectionSnapshot> {
        let needle = self.search.read(cx).value().trim().to_lowercase();
        self.all
            .iter()
            .filter(|c| match self.filter {
                Filter::All => true,
                Filter::Open => is_live(c),
                Filter::Failed => matches!(c.state, ConnectionState::Failed(_)),
            })
            .filter(|c| {
                needle.is_empty()
                    || c.target.to_lowercase().contains(&needle)
                    || c.route.to_lowercase().contains(&needle)
            })
            .collect()
    }

    fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let colors = Colors {
            open: theme.success,
            failed: theme.danger,
            muted: theme.muted_foreground,
            normal: theme.foreground,
            border: theme.border,
        };
        let shown = self.shown(cx);
        shown
            .into_iter()
            .skip(range.start)
            .take(range.len())
            .map(|c| row(c, self.rates.rate(c.id), &colors))
            .collect()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.all.iter().filter(|c| is_live(c)).count();
        let summary = format!("{open} open, {} finished", self.all.len() - open);
        let selected = FILTERS
            .iter()
            .position(|(filter, _)| *filter == self.filter)
            .unwrap_or(0);
        h_flex()
            .px_6()
            .pb_3()
            .gap_3()
            .child(
                TabBar::new("connection-filter")
                    .segmented()
                    .small()
                    .selected_index(selected)
                    .children(FILTERS.map(|(_, label)| label))
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        this.filter = FILTERS.get(*index).map_or(Filter::All, |(f, _)| *f);
                        cx.notify();
                    })),
            )
            .child(
                Input::new(&self.search)
                    .id("connection-search")
                    .small()
                    .w_64()
                    .prefix(Icon::new(IconName::Search).small())
                    .cleanable(true),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("connection-summary")
                    .test_support()
                    .aria_label(summary.clone())
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(summary),
            )
    }
}

impl Render for ConnectionsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.shown(cx).len();
        let muted = cx.theme().muted_foreground;
        let body = if count == 0 {
            let message = if self.all.is_empty() {
                "No connections yet. Point a browser or app at the proxy address to see them here."
            } else {
                "No connections match."
            };
            div()
                .px_6()
                .py_8()
                .text_sm()
                .text_color(muted)
                .child(message)
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
            .child(self.toolbar(cx))
            .child(header(muted, cx.theme().border))
            .child(div().flex_1().min_h_0().child(body))
    }
}

struct Colors {
    open: Hsla,
    failed: Hsla,
    muted: Hsla,
    normal: Hsla,
    border: Hsla,
}

fn is_live(c: &ConnectionSnapshot) -> bool {
    matches!(c.state, ConnectionState::Connecting | ConnectionState::Open)
}

/// The fixed lanes every row shares, so columns line up from header to last
/// row; the target lane takes the rest.
fn lanes(
    state: impl IntoElement,
    kind: impl IntoElement,
    target: impl IntoElement,
    route: impl IntoElement,
    numbers: [AnyElement; 4],
) -> Div {
    let [time, sent, received, now] = numbers;
    h_flex()
        .w_full()
        .px_6()
        .gap_2()
        .child(div().w_24().flex_none().child(state))
        .child(div().w_16().flex_none().child(kind))
        .child(div().flex_1().min_w_0().child(target))
        .child(div().w_24().flex_none().truncate().child(route))
        .child(div().w_16().flex_none().text_right().child(time))
        .child(div().w_16().flex_none().text_right().child(sent))
        .child(div().w_16().flex_none().text_right().child(received))
        .child(div().w_24().flex_none().text_right().child(now))
}

fn header(muted: Hsla, border: Hsla) -> impl IntoElement {
    let number = |text: &'static str| div().child(text).into_any_element();
    lanes(
        "State",
        "Kind",
        "Target",
        "Route",
        [
            number("Time"),
            number("Sent"),
            number("Received"),
            number("Now"),
        ],
    )
    .py_1p5()
    .text_xs()
    .text_color(muted)
    .border_b_1()
    .border_color(border)
}

fn row(c: &ConnectionSnapshot, rate: Option<(f64, f64)>, colors: &Colors) -> AnyElement {
    let (state, state_color) = match &c.state {
        ConnectionState::Connecting => ("Connecting", colors.muted),
        ConnectionState::Open => ("Open", colors.open),
        ConnectionState::Closed => ("Closed", colors.muted),
        ConnectionState::Failed(_) => ("Failed", colors.failed),
    };
    // The live rate, download over upload, for connections still moving.
    let now = match rate {
        Some((up, down)) if is_live(c) => v_flex()
            .text_xs()
            .child(format!("↓ {}", format_bytes_per_sec(down)))
            .child(format!("↑ {}", format_bytes_per_sec(up)))
            .into_any_element(),
        _ => div().into_any_element(),
    };
    // State is spelled out next to its dot: never color alone.
    let state = h_flex()
        .gap_1p5()
        .child(div().size_1p5().flex_none().rounded_full().bg(state_color))
        .child(state);
    let target = v_flex()
        .child(div().truncate().child(c.target.clone()))
        .when_some(failure(c), |this, reason| {
            this.child(
                div()
                    .text_xs()
                    .truncate()
                    .text_color(colors.failed)
                    .child(reason),
            )
        });
    let number = |text: String| div().child(text).into_any_element();
    lanes(
        state,
        div().text_color(colors.muted).child(kind_label(&c.kind)),
        target,
        div().text_color(colors.muted).child(c.route.clone()),
        [
            number(format_duration(c.duration)),
            number(format_bytes(c.bytes_up)),
            number(format_bytes(c.bytes_down)),
            now,
        ],
    )
    .id(("connection", c.id as usize))
    .test_support()
    .aria_label(describe(c))
    .h_10()
    .text_sm()
    .text_color(if is_live(c) {
        colors.normal
    } else {
        colors.muted
    })
    .border_b_1()
    .border_color(colors.border)
    .into_any_element()
}

fn failure(c: &ConnectionSnapshot) -> Option<String> {
    match &c.state {
        ConnectionState::Failed(reason) => Some(reason.clone()),
        _ => None,
    }
}

/// `HTTPS` for a tunnel, `GET 200` for a plain-HTTP request.
pub(super) fn kind_label(kind: &ConnectionKind) -> String {
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
