//! The throughput chart's numbers: a scale with round tick values, labels that
//! say how long ago, and the geometry that slides the drawing left between
//! one-second samples so the line scrolls without changing shape.

/// Points the visible window spans: one per second, from a minute ago to now
/// inclusive, so evenly spread labels land on 60/45/30/15 s and now.
pub const WINDOW: usize = 61;

/// The y axis: `steps` equal intervals of `step` bytes/s from zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    pub step: f64,
    pub steps: usize,
}

impl Scale {
    /// The value at the top grid line.
    pub fn top(self) -> f64 {
        self.step * self.steps as f64
    }
}

/// The y scale for a series peaking at `peak` bytes/s: two to four steps of a
/// 1-2-5 sequence in binary units, the smallest such step that covers the
/// peak. Every grid line then reads as a round number and the line uses most
/// of the height. Never finer than 1 KB/s, so an idle chart still reads.
pub fn scale(peak: f64) -> Scale {
    const MOST: usize = 4;
    let mut unit = 1024.0;
    while unit <= 1024f64.powi(5) {
        for size in [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0] {
            let step = size * unit;
            let steps = ((peak / step).ceil() as usize).max(2);
            if steps <= MOST {
                return Scale { step, steps };
            }
        }
        unit *= 1024.0;
    }
    Scale {
        step: peak / MOST as f64,
        steps: MOST,
    }
}

/// The scale to show next, given the one on screen: kept while the peak fits
/// under its top and is at least 40% of it, so ordinary wobble never rescales
/// the chart; refitted at once when the line outgrows it or shrinks well
/// below it.
pub fn rescale(current: Option<Scale>, peak: f64) -> Scale {
    match current {
        Some(held) if peak <= held.top() && peak >= held.top() * 0.4 => held,
        _ => scale(peak),
    }
}

/// A grid-line value: `"2 MB/s"`, without a `.0` the round steps never need.
pub fn axis_rate(bytes_per_sec: f64) -> String {
    crate::throughput::format_bytes_per_sec(bytes_per_sec).replace(".0 ", " ")
}

/// The axis label for visible slot `index` (0 = a minute ago): `"now"` at
/// the right edge, then seconds back to `"1 min ago"` at the left.
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

/// The (in, out) samples to draw, oldest first: one more than the window, so
/// the newest starts just past the right edge and slides in. History older
/// than what was measured is empty. Values are never blended, so the line
/// keeps its shape as it moves.
pub fn series(history: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let wanted = WINDOW + 1;
    let start = history.len().saturating_sub(wanted);
    let mut drawn = vec![(0.0, 0.0); wanted - (history.len() - start)];
    drawn.extend_from_slice(&history[start..]);
    drawn
}

/// How far left to shift the drawing, as a fraction of the visible width,
/// `progress` (0..=1) of the way from the previous sample to the newest: one
/// slot per sample, holding at a full slot if the next sample is late.
pub fn slide(progress: f64) -> f32 {
    progress.clamp(0.0, 1.0) as f32 / (WINDOW - 1) as f32
}

/// The rate to show in the legend: counting from the previous sample to the
/// newest as the drawing slides, so the number and the line move together.
pub fn rate_at(history: &[(f64, f64)], progress: f64) -> (f64, f64) {
    let progress = progress.clamp(0.0, 1.0);
    match history {
        [] => (0.0, 0.0),
        [only] => *only,
        [.., (a_in, a_out), (b_in, b_out)] => (
            a_in + (b_in - a_in) * progress,
            a_out + (b_out - a_out) * progress,
        ),
    }
}
