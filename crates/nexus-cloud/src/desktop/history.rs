//! A cloud session's history as Gateway keeps it: every event of every workbench thread on a
//! cloud machine, which the machine sends Gateway as its log appends them. The apps read it from Gateway whether the
//! machine is awake or asleep (`GET /v1/cloud/machines/{id}/history/{thread}/events`), so a session whose machine
//! sleeps opens with its messages, on any device, without waking it.
//!
//! The event numbers are the machine's own (`seq`, from 1, no gaps), so a page from Gateway and the live relay's
//! events (`wb:event`) are one log: whichever arrives second for a number is the same event.

use serde_json::Value;

/// The most events Gateway answers in one page.
pub const MAX_PAGE: usize = 500;

/// Which page of a thread's history to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// The latest events: what opens a session.
    Tail,
    /// The page ending just before event `n`: scrolling back.
    Before(i64),
    /// The page starting just after event `n`: catching up.
    After(i64),
}

impl Page {
    /// The page's query, with `limit` (at most [`MAX_PAGE`]).
    pub fn query(self, limit: usize) -> String {
        let limit = limit.clamp(1, MAX_PAGE);
        match self {
            Page::Tail => format!("limit={limit}"),
            Page::Before(seq) => format!("before={seq}&limit={limit}"),
            Page::After(seq) => format!("after={seq}&limit={limit}"),
        }
    }
}

/// One stored event: its workbench JSON exactly as the machine logged it, or the size of a body over Gateway's
/// per-event cap (256 KiB), which Gateway does not keep. Its number holds its place either way.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEvent {
    pub seq: i64,
    /// Unix ms, the machine's time.
    pub at: i64,
    /// The workbench event's JSON text (`events.body`); None when it was too large to keep.
    pub body: Option<String>,
    /// The size of a body Gateway did not keep.
    pub omitted_bytes: Option<u64>,
}

/// One page of a thread's history, in ascending `seq` order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HistoryPage {
    /// False when the organization turned cloud history off (`cloudHistory`): Gateway keeps nothing, and the app
    /// reads a session only from its machine, awake.
    pub enabled: bool,
    /// The thread's stored range; None when nothing is stored.
    pub first_seq: Option<i64>,
    pub last_seq: Option<i64>,
    pub events: Vec<HistoryEvent>,
}

impl HistoryPage {
    /// Gateway's answer, read leniently: a missing `enabled` counts as on, an event without a number is skipped, and
    /// the events come back sorted by number whatever order they were sent in.
    pub fn from_json(value: &Value) -> HistoryPage {
        let number = |key: &str| value[key].as_i64();
        let mut events: Vec<HistoryEvent> = value
            .get("events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|event| {
                Some(HistoryEvent {
                    seq: event.get("seq").and_then(Value::as_i64).filter(|seq| *seq >= 1)?,
                    at: event.get("at").and_then(Value::as_i64).unwrap_or_default(),
                    body: event.get("body").and_then(Value::as_str).map(str::to_string),
                    omitted_bytes: event.get("omittedBytes").and_then(Value::as_u64),
                })
            })
            .collect();
        events.sort_by_key(|event| event.seq);
        events.dedup_by_key(|event| event.seq);
        HistoryPage {
            enabled: value.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            first_seq: number("firstSeq"),
            last_seq: number("lastSeq"),
            events,
        }
    }

    /// Older events exist before this page.
    pub fn more_before(&self) -> bool {
        match (self.events.first(), self.first_seq) {
            (Some(first), Some(stored)) => first.seq > stored,
            _ => false,
        }
    }

    /// Newer events exist after this page.
    pub fn more_after(&self) -> bool {
        match (self.events.last(), self.last_seq) {
            (Some(last), Some(stored)) => last.seq < stored,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_page_reads_its_events_in_order_and_says_what_is_around_it() {
        let page = HistoryPage::from_json(&json!({
            "machineId": "cm_1", "threadId": "t_1", "enabled": true, "firstSeq": 1, "lastSeq": 43,
            "events": [
                { "seq": 42, "at": 20, "body": null, "omittedBytes": 1048576 },
                { "seq": 41, "at": 10, "body": "{\"kind\":\"notice\",\"text\":\"hi\"}" },
                { "seq": 41, "at": 10, "body": "{\"kind\":\"notice\",\"text\":\"hi\"}" },
                { "at": 5, "body": "{}" },
            ]
        }));
        assert!(page.enabled);
        assert_eq!(
            page.events,
            vec![
                HistoryEvent {
                    seq: 41,
                    at: 10,
                    body: Some("{\"kind\":\"notice\",\"text\":\"hi\"}".into()),
                    omitted_bytes: None,
                },
                HistoryEvent {
                    seq: 42,
                    at: 20,
                    body: None,
                    omitted_bytes: Some(1_048_576),
                },
            ]
        );
        assert!(page.more_before());
        assert!(page.more_after());
    }

    #[test]
    fn history_turned_off_or_empty_reads_as_nothing_around() {
        let off = HistoryPage::from_json(&json!({
            "enabled": false, "firstSeq": null, "lastSeq": null, "events": []
        }));
        assert!(!off.enabled);
        assert!(off.events.is_empty());
        assert!(!off.more_before() && !off.more_after());
        let empty = HistoryPage::from_json(&json!({ "events": [] }));
        assert!(empty.enabled, "a Gateway that does not say is on");
        assert_eq!((empty.first_seq, empty.last_seq), (None, None));
    }

    #[test]
    fn a_pages_query_names_its_side_and_caps_the_limit() {
        assert_eq!(Page::Tail.query(100), "limit=100");
        assert_eq!(Page::Before(41).query(600), "before=41&limit=500");
        assert_eq!(Page::After(7).query(0), "after=7&limit=1");
    }
}
