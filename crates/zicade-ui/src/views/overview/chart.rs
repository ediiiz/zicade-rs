//! The throughput section: a legend that names each line with its current
//! rate, and a minute-long chart with a readable scale that scrolls smoothly
//! between one-second samples.

use gpui_kit::component::chart::AreaChart;
use gpui_kit::component::{ActiveTheme, StyledExt, h_flex, v_flex};
use gpui_kit::*;

use super::parts::section_title;
use crate::chart_model::{frame, scale_top, when_label};
use crate::throughput::format_bytes_per_sec;

/// One drawn point: (in, out) bytes per second and how long ago.
#[derive(Clone)]
struct Point {
    when: SharedString,
    download: f64,
    upload: f64,
}

/// `history` is oldest-first rates; `progress` (0..=1) is how far the view is
/// from the previous sample towards the newest.
pub(super) fn throughput(history: &[(f64, f64)], progress: f64, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let points = frame(history, progress);
    let (download, upload) = points.last().copied().unwrap_or_default();
    let peak = points
        .iter()
        .map(|(rx, tx)| rx.max(*tx))
        .fold(0.0, f64::max);
    let top = scale_top(peak);
    let points: Vec<Point> = points
        .into_iter()
        .enumerate()
        .map(|(index, (download, upload))| Point {
            when: when_label(index).into(),
            download,
            upload,
        })
        .collect();
    let chart = AreaChart::new(points)
        .id("throughput-chart")
        .x(|p: &Point| p.when.clone())
        .y(|p: &Point| p.download)
        .stroke(theme.chart_1)
        .fill(theme.chart_1.opacity(0.15))
        .name("Download")
        .y(|p: &Point| p.upload)
        .stroke(theme.chart_2)
        .fill(theme.chart_2.opacity(0.15))
        .name("Upload")
        .linear()
        .y_domain(0.0, top)
        // The top is already a round step above the peak; headroom would
        // shift every tick off its round value.
        .y_padding(0.0, 0.0)
        .y_axis(true)
        .y_tick_count(4)
        .y_tick_format(|v: f64| SharedString::from(format_bytes_per_sec(v)))
        .x_tick_count(5)
        .tooltip_value(|_, _, v: f64| SharedString::from(format_bytes_per_sec(v)))
        .appear(false);
    v_flex()
        .gap_3()
        .child(
            h_flex()
                .gap_5()
                .child(section_title("Throughput"))
                .child(div().flex_1())
                .child(legend(
                    "rate-download",
                    "Download",
                    download,
                    theme.chart_1,
                    cx,
                ))
                .child(legend("rate-upload", "Upload", upload, theme.chart_2, cx)),
        )
        .child(div().h_56().w_full().child(chart))
}

/// A swatch in the line's color, its name, and its current rate.
fn legend(
    id: &'static str,
    name: &'static str,
    rate: f64,
    color: Hsla,
    cx: &App,
) -> impl IntoElement {
    let rate = format_bytes_per_sec(rate);
    h_flex()
        .gap_2()
        .text_sm()
        .child(div().w_3().h_1().rounded_full().bg(color))
        .child(div().text_color(cx.theme().muted_foreground).child(name))
        .child(
            div()
                .id(id)
                .test_support()
                .aria_label(rate.clone())
                .font_medium()
                .child(rate),
        )
}
