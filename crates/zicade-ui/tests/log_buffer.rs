//! The log view replays the store's backlog, then follows the live stream; the
//! two overlap, and the view must not grow without bound.

use std::collections::BTreeMap;

use zicade_observe::LogEvent;
use zicade_ui::log_buffer::{LogBuffer, describe};

fn event(seq: u64, message: &str) -> LogEvent {
    LogEvent {
        seq,
        timestamp: 0,
        level: "INFO".to_owned(),
        target: "zicade::app".to_owned(),
        message: message.to_owned(),
        fields: BTreeMap::new(),
    }
}

fn messages(buffer: &LogBuffer) -> Vec<String> {
    buffer.events().map(|e| e.message.clone()).collect()
}

#[test]
fn live_events_already_in_the_backlog_are_not_shown_twice() {
    let mut buffer = LogBuffer::new(10);
    buffer.push(event(1, "a"));
    buffer.push(event(2, "b"));

    buffer.push(event(2, "b"));
    buffer.push(event(3, "c"));

    assert_eq!(messages(&buffer), ["a", "b", "c"]);
}

#[test]
fn only_the_newest_lines_are_kept() {
    let mut buffer = LogBuffer::new(2);
    for seq in 1..=5 {
        buffer.push(event(seq, &seq.to_string()));
    }

    assert_eq!(messages(&buffer), ["4", "5"]);
}

#[test]
fn a_line_shows_level_target_message_and_fields() {
    let mut e = event(1, "zicade started");
    e.level = "WARN".to_owned();
    e.fields
        .insert("proxy".to_owned(), "127.0.0.1:3129".to_owned());

    assert_eq!(
        describe(&e),
        "[WARN] zicade::app: zicade started proxy=127.0.0.1:3129"
    );
}
