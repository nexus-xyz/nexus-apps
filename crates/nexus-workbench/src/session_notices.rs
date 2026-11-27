//! Completion notices: a session's last completion counts within a recent window ([`RECENT_MS`]).
//! Metadata changes cannot prove a turn finished on an older host.

use crate::event::ThreadStatus;

pub const RECENT_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub const MAX_RECENT: usize = 50;

/// The last actual completion, when it qualifies for the recent Inbox list.
pub fn completion(
    status: ThreadStatus,
    archived: bool,
    created_at: i64,
    listed_at: Option<i64>,
    now: i64,
) -> Option<i64> {
    let at = listed_at?;
    (status == ThreadStatus::Idle && !archived && at > created_at && now.saturating_sub(at) <= RECENT_MS).then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_reported_recent_completion_makes_a_notice() {
        assert_eq!(
            completion(ThreadStatus::Idle, false, 1000, Some(2000), 3000),
            Some(2000)
        );
        assert_eq!(completion(ThreadStatus::Idle, false, 1000, None, 3000), None);
        assert_eq!(completion(ThreadStatus::Idle, false, 1000, Some(1000), 3000), None);
        assert_eq!(completion(ThreadStatus::Idle, true, 1000, Some(2000), 3000), None);
        for status in [
            ThreadStatus::Running,
            ThreadStatus::Starting,
            ThreadStatus::Approval,
            ThreadStatus::Error,
            ThreadStatus::Interrupted,
        ] {
            assert_eq!(completion(status, false, 1000, Some(2000), 3000), None);
        }
        assert_eq!(
            completion(ThreadStatus::Idle, false, 1000, Some(2000), 2000 + RECENT_MS),
            Some(2000)
        );
        assert_eq!(
            completion(ThreadStatus::Idle, false, 1000, Some(2000), 2001 + RECENT_MS),
            None
        );
    }
}

/// Each app stores its own marks within the Gateway, organization, and person scope.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Marks {
    pub baseline: i64,
    pub items: std::collections::BTreeMap<String, Mark>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    pub read_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done_at: Option<i64>,
}

impl Marks {
    pub fn new(now: i64) -> Self {
        Self {
            baseline: now,
            items: Default::default(),
        }
    }

    /// Ignore malformed records without discarding valid neighboring marks.
    pub fn parse(text: &str, now: i64) -> Self {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            return Self::new(now);
        };
        let Some(baseline) = value.get("baseline").and_then(serde_json::Value::as_i64) else {
            return Self::new(now);
        };
        let Some(items) = value.get("items").and_then(serde_json::Value::as_object) else {
            return Self::new(now);
        };
        let mut marks = Self::new(baseline);
        for (id, value) in items {
            if let Some(read_at) = value.get("readAt").and_then(serde_json::Value::as_i64) {
                marks.items.insert(
                    id.clone(),
                    Mark {
                        read_at,
                        done_at: value.get("doneAt").and_then(serde_json::Value::as_i64),
                    },
                );
            }
        }
        marks.bound();
        marks
    }

    /// Stale windows cannot erase a newer read or done mark.
    pub fn merge(&mut self, other: &Self) {
        self.baseline = self.baseline.min(other.baseline);
        for (id, mark) in &other.items {
            let held = self.items.entry(id.clone()).or_insert_with(|| mark.clone());
            held.read_at = held.read_at.max(mark.read_at);
            held.done_at = held.done_at.max(mark.done_at);
        }
        self.bound();
    }

    pub fn mark(&mut self, id: &str, done: bool, now: i64) {
        let mark = self.items.entry(id.into()).or_insert(Mark {
            read_at: now,
            done_at: None,
        });
        mark.read_at = mark.read_at.max(now);
        if done {
            mark.done_at = Some(mark.done_at.unwrap_or(0).max(now));
        }
        self.bound();
    }

    /// A question still needs an answer after it is read or marked done.
    pub fn shown(&self, id: &str, needs_you: bool, dismissible: bool) -> bool {
        self.items.get(id).is_none_or(|mark| mark.done_at.is_none()) || (needs_you && !dismissible)
    }

    pub fn read(&self, id: &str, needs_you: bool, at: i64) -> bool {
        self.items.get(id).is_some_and(|mark| mark.read_at != 0) || (!needs_you && at <= self.baseline)
    }

    fn bound(&mut self) {
        let mut order = self
            .items
            .iter()
            .map(|(id, mark)| (id.clone(), mark.read_at.max(mark.done_at.unwrap_or(0))))
            .collect::<Vec<_>>();
        order.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        for (id, _) in order.into_iter().skip(500) {
            self.items.remove(&id);
        }
    }
}

#[cfg(test)]
mod mark_tests {
    use super::*;

    #[test]
    fn read_and_done_merge_independently_and_keep_the_first_baseline() {
        let mut read = Marks::new(1000);
        read.mark("one", false, 4000);
        let mut done = Marks::new(2000);
        done.mark("one", true, 3000);
        read.merge(&done);
        done.merge(&read);
        assert_eq!(read, done);
        assert_eq!(read.baseline, 1000);
        assert_eq!(
            read.items["one"],
            Mark {
                read_at: 4000,
                done_at: Some(3000)
            }
        );
    }

    #[test]
    fn pending_requests_stay_visible_and_later_completions_are_new() {
        let mut marks = Marks::new(1000);
        marks.mark("question", true, 3000);
        assert!(marks.shown("question", true, false));
        assert!(marks.read("question", true, 2000));
        assert!(!marks.shown("question", true, true));
        marks.mark("finished:2000", true, 3000);
        assert!(!marks.shown("finished:2000", false, false));
        assert!(marks.shown("finished:4000", false, false));
        assert!(!marks.read("finished:4000", false, 4000));
        assert!(marks.read("old", false, 500));
        assert!(!marks.read("pending-old", true, 500));
    }

    #[test]
    fn malformed_records_are_ignored_and_only_the_latest_five_hundred_remain() {
        assert_eq!(Marks::parse("broken", 1000), Marks::new(1000));
        let mut marks = Marks::parse(
            r#"{"baseline":1000,"items":{"good":{"readAt":2000},"bad":{"readAt":"bad"}}}"#,
            3000,
        );
        assert_eq!(marks.items.len(), 1);
        for at in 3000..3600 {
            marks.mark(&format!("notice:{at}"), false, at);
        }
        assert_eq!(marks.items.len(), 500);
        assert!(!marks.items.contains_key("good"));
        assert!(marks.items.contains_key("notice:3599"));
        let text = serde_json::to_string(&marks).expect("serializable marks");
        assert_eq!(Marks::parse(&text, 5000), marks);
    }
}
