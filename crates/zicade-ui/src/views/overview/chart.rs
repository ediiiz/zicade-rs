//! The throughput section: a legend that names each line with its current
//! rate, and a minute-long chart that scrolls smoothly between one-second
//! samples.
//!
//! The scale, grid, and time labels are a still layer. Only the drawing
//! moves: it is one slot wider than the visible plot and slides left inside a
//! clipped frame, so the line keeps its shape as it scrolls instead of every
//! value being reshaped on each frame.

use gpui_kit::component::chart::AreaChart;
use gpui_kit::component::{ActiveTheme, StyledExt, h_flex, v_flex};
use gpui_kit::*;

use super::parts::section_title;
use crate::chart_model::{Scale, WINDOW, axis_rate, rate_at, series, slide, when_label};
use crate::throughput::format_bytes_per_sec;

/// Visible slots between time labels: 15 s apart.
const LABEL_EVERY: usize = (WINDOW - 1) / 4;

/// One drawn point: (in, out) bytes per second and how long ago.
#[derive(Clone)]
struct Point {
    when: SharedString,
    download: f64,
    upload: f64,
}

/// `history` is oldest-first rates; `scale` is held by the view so it only
/// changes when the line outgrows or dwarfs it; `progress` (0..=1) is how far
/// the view is from the previous sample towards the newest.
pub(super) fn throughput(
    history: &[(f64, f64)],
    scale: Scale,
    progress: f64,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let drawn = series(history);
    let (download, upload) = rate_at(history, progress);
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
        .child(
            v_flex()
                // Room for the top value label, which sits above its line.
                .pt_3()
                .gap_1()
                .child(
                    h_flex()
                        .items_stretch()
                        .gap_2()
                        .child(value_labels(scale, cx))
                        .child(plot(drawn, scale, progress, cx)),
                )
                .child(time_labels(cx)),
        )
}

/// The still grid with the sliding drawing clipped inside it.
fn plot(drawn: Vec<(f64, f64)>, scale: Scale, progress: f64, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let last = drawn.len().saturating_sub(1);
    let points: Vec<Point> = drawn
        .into_iter()
        .enumerate()
        .map(|(index, (download, upload))| Point {
            when: ago_title(last - index).into(),
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
        .x_axis(false)
        .y_axis(false)
        .grid(false)
        .y_domain(0.0, scale.top())
        .y_padding(0.0, 0.0)
        .tooltip_value(|_, _, v: f64| SharedString::from(format_bytes_per_sec(v)))
        .appear(false);
    let grid = (0..=scale.steps).map(|i| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(relative(1.0 - i as f32 / scale.steps as f32))
            .border_t_1()
            .border_color(theme.border)
    });
    let width = WINDOW as f32 / (WINDOW - 1) as f32;
    div()
        .flex_1()
        .min_w_0()
        .h_56()
        .relative()
        .children(grid)
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right_0()
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(-slide(progress)))
                        .w(relative(width))
                        .child(chart),
                ),
        )
}

/// Round values beside their grid lines, from 0 at the bottom to the top.
fn value_labels(scale: Scale, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    let labels = (0..=scale.steps).map(|i| {
        div()
            .absolute()
            .right_0()
            .bottom(relative(i as f32 / scale.steps as f32))
            .text_xs()
            .text_color(muted)
            .whitespace_nowrap()
            .child(axis_rate(scale.step * i as f64))
    });
    div().w_16().flex_none().relative().children(labels)
}

/// "1 min ago" … "now" under the plot, each starting at its moment.
fn time_labels(cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    let cells = (0..4).map(|i| div().flex_1().child(when_label(i * LABEL_EVERY)));
    h_flex()
        .gap_2()
        .text_xs()
        .text_color(muted)
        .child(div().w_16().flex_none())
        .child(
            h_flex()
                .flex_1()
                .children(cells)
                .child(div().child(when_label(WINDOW - 1))),
        )
}

/// The tooltip title for a point `ago` seconds old.
fn ago_title(ago: usize) -> String {
    match ago {
        0 => "now".to_owned(),
        ago => format!("{ago} s ago"),
    }
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
