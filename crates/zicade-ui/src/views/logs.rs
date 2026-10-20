//! The live log: the store's backlog, then every new event, newest at the
//! bottom, filterable by level and text.

use std::ops::Range;

use chrono::{Local, TimeZone};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::{ActiveTheme, Icon, Sizable, h_flex, v_flex};
use gpui_kit::*;
use tokio::sync::broadcast::error::RecvError;
use zicade_observe::{LogEvent, LogStore};

use crate::log_buffer::{LogBuffer, describe};

/// Lines kept on screen; older ones scroll away for good.
const MAX_LINES: usize = 2000;

/// Which levels the list shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Levels {
    All,
    Warnings,
    Errors,
}

const LEVELS: [(Levels, &str); 3] = [
    (Levels::All, "All"),
    (Levels::Warnings, "Warnings"),
    (Levels::Errors, "Errors"),
];

impl Levels {
    fn admits(self, level: &str) -> bool {
        match self {
            Levels::All => true,
            Levels::Warnings => matches!(level, "WARN" | "ERROR"),
            Levels::Errors => level == "ERROR",
        }
    }
}

pub(crate) struct LogsView {
    buffer: LogBuffer,
    scroll: UniformListScrollHandle,
    levels: Levels,
    /// Keep the newest line in view as lines arrive.
    follow: bool,
    search: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl LogsView {
    pub(crate) fn new(logs: &LogStore, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Subscribe before reading the backlog so nothing falls in between;
        // the buffer drops the overlap.
        let mut live = logs.subscribe();
        let mut buffer = LogBuffer::new(MAX_LINES);
        for event in logs.recent() {
            buffer.push(event);
        }
        cx.spawn(async move |this, cx| {
            loop {
                let event = match live.recv().await {
                    Ok(event) => event,
                    // Fell behind a burst; the skipped lines are gone, keep going.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                };
                let alive = this.update(cx, |this, cx| {
                    this.buffer.push(event);
                    if this.follow {
                        this.scroll.scroll_to_bottom();
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search logs"));
        let refilter = cx.subscribe_in(&search, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let scroll = UniformListScrollHandle::new();
        scroll.scroll_to_bottom();
        Self {
            buffer,
            scroll,
            levels: Levels::All,
            follow: true,
            search,
            _subscriptions: vec![refilter],
        }
    }

    fn shown(&self, cx: &App) -> Vec<&LogEvent> {
        let needle = self.search.read(cx).value().trim().to_lowercase();
        self.buffer
            .events()
            .filter(|e| self.levels.admits(&e.level))
            .filter(|e| needle.is_empty() || describe(e).to_lowercase().contains(&needle))
            .collect()
    }

    fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (danger, warning, muted, normal, mono) = (
            theme.danger,
            theme.warning,
            theme.muted_foreground,
            theme.foreground,
            theme.mono_font_family.clone(),
        );
        self.shown(cx)
            .into_iter()
            .enumerate()
            .skip(range.start)
            .take(range.len())
            .map(|(index, event)| {
                let level_color = match event.level.as_str() {
                    "ERROR" => danger,
                    "WARN" => warning,
                    _ => muted,
                };
                let time = local_time(event);
                let mut message = event.message.clone();
                for (key, value) in &event.fields {
                    message.push_str(&format!(" {key}={value}"));
                }
                h_flex()
                    .id(("log-line", index))
                    .test_support()
                    .aria_label(format!("{time} {}", describe(event)))
                    .h_6()
                    .px_6()
                    .gap_3()
                    .text_xs()
                    .font_family(mono.clone())
                    .whitespace_nowrap()
                    .child(div().w_16().flex_none().text_color(muted).child(time))
                    .child(
                        div()
                            .w_12()
                            .flex_none()
                            .text_color(level_color)
                            .child(event.level.clone()),
                    )
                    .child(
                        div()
                            .w_48()
                            .flex_none()
                            .truncate()
                            .text_color(muted)
                            .child(event.target.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(normal)
                            .child(message),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = LEVELS
            .iter()
            .position(|(levels, _)| *levels == self.levels)
            .unwrap_or(0);
        h_flex()
            .px_6()
            .pb_3()
            .gap_3()
            .child(
                TabBar::new("log-levels")
                    .segmented()
                    .small()
                    .selected_index(selected)
                    .children(LEVELS.map(|(_, label)| label))
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        this.levels = LEVELS.get(*index).map_or(Levels::All, |(l, _)| *l);
                        cx.notify();
                    })),
            )
            .child(
                Input::new(&self.search)
                    .id("log-search")
                    .small()
                    .w_64()
                    .prefix(Icon::new(IconName::Search).small())
                    .cleanable(true),
            )
            .child(div().flex_1())
            .child(
                Switch::new("log-follow")
                    .small()
                    .checked(self.follow)
                    .label("Follow new lines")
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.follow = *on;
                        if *on {
                            this.scroll.scroll_to_bottom();
                        }
                        cx.notify();
                    })),
            )
    }
}

impl Render for LogsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.shown(cx).len();
        let body = if count == 0 {
            let message = if self.buffer.is_empty() {
                "Nothing logged yet."
            } else {
                "No lines match."
            };
            div()
                .px_6()
                .py_8()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(message)
                .into_any_element()
        } else {
            uniform_list(
                "log-lines",
                count,
                cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        };
        v_flex().size_full().child(self.toolbar(cx)).child(
            div()
                .flex_1()
                .min_h_0()
                .border_t_1()
                .border_color(cx.theme().border)
                .py_1()
                .child(body),
        )
    }
}

/// `HH:MM:SS` in local time.
fn local_time(event: &LogEvent) -> String {
    i64::try_from(event.timestamp)
        .ok()
        .and_then(|ms| Local.timestamp_millis_opt(ms).single())
        .map(|time| time.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}
