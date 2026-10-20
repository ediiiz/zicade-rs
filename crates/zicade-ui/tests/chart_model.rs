//! The throughput chart's numbers: a scale people can read, labels that say
//! when, and a drawing that slides left between samples without changing
//! shape.

use zicade_ui::chart_model::{
    Scale, WINDOW, axis_rate, rate_at, rescale, scale, series, slide, when_label,
};

#[test]
fn the_scale_is_two_to_four_round_steps_just_above_the_peak() {
    let kb = 1024.0;
    let mb = kb * kb;
    assert_eq!(scale(2.7 * mb), Scale { step: mb, steps: 3 });
    assert_eq!(scale(3.1 * mb), Scale { step: mb, steps: 4 });
    assert_eq!(
        scale(40.0 * kb),
        Scale {
            step: 10.0 * kb,
            steps: 4
        }
    );
    assert_eq!(
        scale(4.5 * mb),
        Scale {
            step: 2.0 * mb,
            steps: 3
        }
    );
    assert_eq!(scale(2.7 * mb).top(), 3.0 * mb);
}

#[test]
fn an_idle_chart_still_has_a_readable_scale() {
    assert_eq!(
        scale(0.0),
        Scale {
            step: 1024.0,
            steps: 2
        }
    );
}

#[test]
fn the_scale_holds_still_unless_the_line_outgrows_or_dwarfs_it() {
    let mb = 1024.0 * 1024.0;
    let held = rescale(None, 2.7 * mb);
    assert_eq!(held.top(), 3.0 * mb);

    // Wobbling below the top keeps the scale: no jumps every second.
    assert_eq!(rescale(Some(held), 2.0 * mb), held);
    assert_eq!(rescale(Some(held), 1.3 * mb), held);
    // Outgrowing it rescales at once, so nothing is cut off.
    assert_eq!(rescale(Some(held), 3.5 * mb).top(), 4.0 * mb);
    // A peak far below it lets it shrink, so a quiet line is not flat.
    assert_eq!(rescale(Some(held), 0.5 * mb), scale(0.5 * mb));
}

#[test]
fn axis_values_read_as_round_numbers() {
    let mb = 1024.0 * 1024.0;
    assert_eq!(axis_rate(2.0 * mb), "2 MB/s");
    assert_eq!(axis_rate(0.0), "0 B/s");
    assert_eq!(axis_rate(500.0 * 1024.0), "500 KB/s");
}

#[test]
fn labels_say_how_long_ago() {
    assert_eq!(when_label(WINDOW - 1), "now");
    assert_eq!(when_label(WINDOW - 16), "15 s ago");
    assert_eq!(when_label(0), "1 min ago");
    // Five evenly spread labels land on round seconds.
    let step = (WINDOW - 1) / 4;
    let labels: Vec<_> = (0..5).map(|i| when_label(i * step)).collect();
    assert_eq!(
        labels,
        ["1 min ago", "45 s ago", "30 s ago", "15 s ago", "now"]
    );
}

#[test]
fn the_series_is_the_samples_themselves_padded_to_one_more_than_the_window() {
    // One sample beyond the window is drawn off the right edge, ready to
    // slide in; older history than measured is empty.
    let drawn = series(&[(10.0, 1.0), (20.0, 2.0)]);

    assert_eq!(drawn.len(), WINDOW + 1);
    assert_eq!(drawn[0], (0.0, 0.0));
    assert_eq!(drawn[WINDOW - 1], (10.0, 1.0));
    assert_eq!(drawn[WINDOW], (20.0, 2.0));
}

#[test]
fn drawn_values_are_the_measured_ones_never_blends() {
    // The shape must not morph: sliding moves every point by the same
    // distance instead of blending each value towards its neighbour.
    let history: Vec<_> = (0..80).map(|i| (f64::from(i), 0.0)).collect();

    let drawn = series(&history);

    assert_eq!(drawn.first(), Some(&(18.0, 0.0)));
    assert_eq!(drawn.last(), Some(&(79.0, 0.0)));
}

#[test]
fn the_drawing_slides_one_slot_per_sample() {
    let slot = 1.0 / (WINDOW - 1) as f32;

    assert_eq!(slide(0.0), 0.0);
    assert!((slide(0.5) - slot / 2.0).abs() < 1e-6);
    assert!((slide(1.0) - slot).abs() < 1e-6);
    assert!(
        (slide(7.0) - slot).abs() < 1e-6,
        "a late sample holds, never overshoots"
    );
    assert_eq!(slide(-1.0), 0.0);
}

#[test]
fn the_legend_rate_counts_from_the_previous_sample_to_the_newest() {
    let history = [(0.0, 0.0), (100.0, 10.0)];

    assert_eq!(rate_at(&history, 0.0), (0.0, 0.0));
    assert_eq!(rate_at(&history, 0.5), (50.0, 5.0));
    assert_eq!(rate_at(&history, 1.0), (100.0, 10.0));
    assert_eq!(rate_at(&[], 0.5), (0.0, 0.0));
}
