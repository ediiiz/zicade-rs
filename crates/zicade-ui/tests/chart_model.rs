//! The throughput chart's numbers: a scale people can read, labels that say
//! when, and frames that glide between one-second samples.

use zicade_ui::chart_model::{WINDOW, frame, scale_top, when_label};

#[test]
fn the_scale_tops_out_at_a_round_value_above_the_peak() {
    // Three intervals of a 1-2-5 step: tick labels read 0, 1, 2, 3 MB/s.
    let mb = 1024.0 * 1024.0;
    assert_eq!(scale_top(2.7 * mb), 3.0 * mb);
    assert_eq!(scale_top(3.1 * mb), 6.0 * mb);
    assert_eq!(scale_top(40.0 * 1024.0), 60.0 * 1024.0);
}

#[test]
fn an_idle_chart_still_has_a_readable_scale() {
    assert_eq!(scale_top(0.0), 3.0 * 1024.0);
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
fn a_frame_always_spans_the_whole_window() {
    // Two samples so far: the rest of the minute is empty history.
    let frame = frame(&[(10.0, 1.0), (20.0, 2.0)], 1.0);

    assert_eq!(frame.len(), WINDOW);
    assert_eq!(frame[0], (0.0, 0.0));
    assert_eq!(frame[WINDOW - 1], (20.0, 2.0));
}

#[test]
fn frames_glide_from_one_sample_to_the_next() {
    let history = [(0.0, 0.0), (100.0, 10.0)];

    let start = frame(&history, 0.0);
    let halfway = frame(&history, 0.5);
    let end = frame(&history, 1.0);

    assert_eq!(start[WINDOW - 1], (0.0, 0.0));
    assert_eq!(halfway[WINDOW - 1], (50.0, 5.0));
    assert_eq!(end[WINDOW - 1], (100.0, 10.0));
    // Each point moves towards its right-hand neighbour: the line scrolls.
    assert_eq!(halfway[WINDOW - 2], (0.0, 0.0));
}

#[test]
fn progress_outside_zero_to_one_is_clamped() {
    let history = [(0.0, 0.0), (100.0, 10.0)];

    assert_eq!(frame(&history, 7.0)[WINDOW - 1], (100.0, 10.0));
    assert_eq!(frame(&history, -1.0)[WINDOW - 1], (0.0, 0.0));
}
