//! The throughput chart's numbers: a scale with round tick values, labels that
//! say how long ago, and frames that glide between one-second samples so the
//! line scrolls instead of jumping.

/// Points the chart shows: one per second, from a minute ago to now
/// inclusive, so evenly spread labels land on 60/45/30/15 s and now.
pub const WINDOW: usize = 61;

/// Grid intervals on the y axis; the scale top is three round steps.
const INTERVALS: f64 = 3.0;

/// The y-axis top for a series peaking at `peak` bytes/s: three equal steps
/// of a 1-2-5 sequence in binary units, so every tick label is a round
/// number. Never below 3 KB/s, so an idle chart still has a readable scale.
pub fn scale_top(peak: f64) -> f64 {
    let mut unit = 1024.0;
    loop {
        for step in [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0] {
            let top = step * unit * INTERVALS;
            if top >= peak {
                return top;
            }
        }
        unit *= 1024.0;
        if unit > 1024f64.powi(5) {
            return peak;
        }
    }
}

/// The label for point `index` of a frame: `"now"` at the right edge, then
/// seconds back to `"1 min ago"` at the left.
pub fn when_label(index: usize) -> String {
    let ago = WINDOW.saturating_sub(index + 1);
    if ago == 0 {
        "now".to_owned()
    } else if ago >= 60 {
        "1 min ago".to_owned()
    } else {
        format!("{ago} s ago")
    }
}

/// The `WINDOW` (in, out) points to draw, `progress` of the way (0..=1) from
/// the previous sample to the newest.
///
/// History older than what was measured is empty (zero), so the window always
/// spans the full width. Each point moves towards its right-hand neighbour,
/// which scrolls the line left as `progress` grows; at 1 the frame equals the
/// next sample's frame at 0, so successive seconds join without a jump.
pub fn frame(history: &[(f64, f64)], progress: f64) -> Vec<(f64, f64)> {
    let progress = progress.clamp(0.0, 1.0);
    let wanted = WINDOW + 1;
    let start = history.len().saturating_sub(wanted);
    let mut padded = vec![(0.0, 0.0); wanted - (history.len() - start)];
    padded.extend_from_slice(&history[start..]);
    padded
        .windows(2)
        .map(|pair| {
            let ((a_in, a_out), (b_in, b_out)) = (pair[0], pair[1]);
            (
                a_in + (b_in - a_in) * progress,
                a_out + (b_out - a_out) * progress,
            )
        })
        .collect()
}
