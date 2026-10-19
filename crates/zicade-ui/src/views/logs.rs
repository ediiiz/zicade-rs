//! The live log: the store's backlog, then every new event, newest at the
//! bottom.

use std::ops::Range;

use chrono::{Local, TimeZone};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::*;
use tokio::sync::broadcast::error::RecvError;
use zicade_observe::{LogEvent, LogStore};

use crate::log_buffer::{LogBuffer, describe};

/// Lines kept on screen; older ones scroll away for good.
const MAX_LINES: usize = 2000;
const ROW_HEIGHT: f32 = 20.;

pub(crate) struct LogsView {
    buffer: LogBuffer,
    scroll: UniformListScrollHandle,
}

impl LogsView {
    pub(crate) fn new(logs: &LogStore, cx: &mut Context<Self>) -> Self {
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
                    this.scroll.scroll_to_bottom();
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        let scroll = UniformListScrollHandle::new();
        scroll.scroll_to_bottom();
        Self { buffer, scroll }
    }

    fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (danger, warning, muted, normal) = (
            theme.danger,
            theme.warning,
            theme.muted_foreground,
            theme.foreground,
        );
        self.buffer
            .events()
            .enumerate()
            .skip(range.start)
            .take(range.len())
            .map(|(index, event)| {
                let color = match event.level.as_str() {
                    "ERROR" => danger,
                    "WARN" => warning,
                    "DEBUG" | "TRACE" => muted,
                    _ => normal,
                };
                let time = local_time(event);
                let line = describe(event);
                h_flex()
                    .id(("log-line", index))
                    .test_support()
                    .aria_label(format!("{time} {line}"))
                    .h(px(ROW_HEIGHT))
                    .gap_2()
                    .text_xs()
                    .font_family("Consolas")
                    .whitespace_nowrap()
                    .child(div().flex_none().text_color(muted).child(time))
                    .child(div().text_color(color).truncate().child(line))
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for LogsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.buffer.is_empty() {
            div()
                .p_4()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No log lines yet.")
                .into_any_element()
        } else {
            uniform_list(
                "log-lines",
                self.buffer.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        };
        v_flex().size_full().px_4().py_2().child(body)
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
