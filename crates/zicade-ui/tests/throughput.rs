//! Throughput is derived from cumulative byte counters sampled about once a
//! second; the figures shown must survive restarts, stalls, and big numbers.

use std::time::Duration;

use zicade_ui::throughput::{Throughput, format_bytes_per_sec, format_count};

#[test]
fn rates_are_bytes_per_second_between_samples() {
    let mut t = Throughput::new(60);
    t.sample(1_000, 500, Duration::from_secs(10));
    t.sample(3_000, 1_500, Duration::from_secs(12));

    assert_eq!(t.latest(), Some((1_000.0, 500.0)));
}

#[test]
fn the_first_sample_has_no_rate_yet() {
    let mut t = Throughput::new(60);
    t.sample(9_999, 9_999, Duration::from_secs(1));

    assert_eq!(t.latest(), None);
    assert!(t.history().is_empty());
}

#[test]
fn a_counter_reset_shows_zero_not_a_negative_rate() {
    let mut t = Throughput::new(60);
    t.sample(10_000, 10_000, Duration::from_secs(1));
    t.sample(100, 100, Duration::from_secs(2));

    assert_eq!(t.latest(), Some((0.0, 0.0)));
}

#[test]
fn samples_without_elapsed_time_are_ignored() {
    let mut t = Throughput::new(60);
    t.sample(0, 0, Duration::from_secs(5));
    t.sample(1_000, 1_000, Duration::from_secs(5));
    t.sample(2_000, 2_000, Duration::from_secs(6));

    assert_eq!(t.history().len(), 1);
    assert_eq!(t.latest(), Some((2_000.0, 2_000.0)));
}

#[test]
fn history_keeps_only_the_newest_points() {
    let mut t = Throughput::new(3);
    for second in 0..10u64 {
        t.sample(second * 100, 0, Duration::from_secs(second));
    }

    assert_eq!(t.history().len(), 3);
}

#[test]
fn rates_use_binary_units() {
    assert_eq!(format_bytes_per_sec(0.0), "0 B/s");
    assert_eq!(format_bytes_per_sec(1023.0), "1023 B/s");
    assert_eq!(format_bytes_per_sec(1536.0), "1.5 KB/s");
    assert_eq!(format_bytes_per_sec(20.0 * 1024.0 * 1024.0), "20 MB/s");
    assert_eq!(format_bytes_per_sec(5.0 * 1024.0_f64.powi(4)), "5120 GB/s");
}

#[test]
fn counts_get_thousands_separators() {
    assert_eq!(format_count(0), "0");
    assert_eq!(format_count(999), "999");
    assert_eq!(format_count(1_234_567), "1,234,567");
}
