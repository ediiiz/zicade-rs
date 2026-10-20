//! Each open connection's live rate comes from the change in its byte counts
//! between two snapshots.

use std::time::{Duration, SystemTime};

use zicade_control::{ConnectionKind, ConnectionSnapshot, ConnectionState};
use zicade_ui::connection_rates::ConnectionRates;
use zicade_ui::throughput::{format_bytes, format_duration};

fn conn(id: u64, up: u64, down: u64) -> ConnectionSnapshot {
    ConnectionSnapshot {
        id,
        kind: ConnectionKind::Tunnel,
        target: "example.com:443".to_owned(),
        route: "direct".to_owned(),
        client: None,
        opened_at: SystemTime::UNIX_EPOCH,
        duration: Duration::ZERO,
        bytes_up: up,
        bytes_down: down,
        state: ConnectionState::Open,
    }
}

#[test]
fn a_rate_needs_two_snapshots_of_the_same_connection() {
    let mut rates = ConnectionRates::default();
    rates.update(&[conn(1, 0, 0)], Duration::from_secs(10));
    assert_eq!(rates.rate(1), None);

    rates.update(&[conn(1, 500, 4_000)], Duration::from_secs(12));

    assert_eq!(rates.rate(1), Some((250.0, 2_000.0)));
}

#[test]
fn connections_that_disappear_are_forgotten() {
    let mut rates = ConnectionRates::default();
    rates.update(&[conn(1, 0, 0), conn(2, 0, 0)], Duration::from_secs(1));
    rates.update(&[conn(1, 10, 10), conn(2, 10, 10)], Duration::from_secs(2));

    rates.update(&[conn(2, 20, 20)], Duration::from_secs(3));

    assert_eq!(rates.rate(1), None);
    assert_eq!(rates.rate(2), Some((10.0, 10.0)));
    assert_eq!(rates.tracked(), 1);
}

#[test]
fn byte_totals_use_binary_units() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(1536), "1.5 KB");
    assert_eq!(format_bytes(300 * 1024 * 1024), "300 MB");
}

#[test]
fn durations_read_at_a_glance() {
    assert_eq!(format_duration(Duration::from_millis(400)), "0s");
    assert_eq!(format_duration(Duration::from_secs(59)), "59s");
    assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
    assert_eq!(format_duration(Duration::from_secs(3_725)), "1h 02m");
}
