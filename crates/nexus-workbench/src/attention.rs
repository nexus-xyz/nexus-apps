//! A session's attention in the sidebar: what its row tells the person, and the "last seen" map it reads.
//!
//! Attention follows the status and the last completion or request for attention
//! ([`ThreadSummary::listed_at`]). Later metadata or usage updates do not count as another completion.
//!
//! "Last seen" keeps, per session, the `updated_at` the person saw. The previous desktop app kept it in its
//! webview's localStorage (key [`STORAGE_KEY`]). The app stores [`Seen::to_json`] on its own, in the same shape.
//!
//! No I/O and no screen code here: the app reads and writes the stored text and draws the mark.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::event::{LimitStop, ThreadStatus, ThreadSummary};

/// Where the previous desktop app kept the "last seen" map in localStorage.
pub const STORAGE_KEY: &str = "nexus.code.seen";
/// A streaming session the person is viewing is marked seen on every update. The write to storage is rate limited to
/// one per this many milliseconds, not the mark.
pub const WRITE_EVERY_MS: i64 = 1000;

/// The one state a row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attention {
    Needs,
    Working,
    Error,
    Limited,
    Unread,
}

impl Attention {
    /// What the row says, in words (`ATTENTION_LABEL`).
    pub fn label(self) -> &'static str {
        match self {
            Attention::Needs => "Needs you",
            Attention::Working => "Working",
            Attention::Error => "Error",
            Attention::Limited => "Limited",
            Attention::Unread => "Finished, unread",
        }
    }
}

/// What the rule reads from a session's summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    pub status: ThreadStatus,
    pub updated_at: i64,
    /// The last completion or request for attention; None from older hosts.
    pub listed_at: Option<i64>,
}

impl Activity {
    pub fn of(thread: &ThreadSummary) -> Activity {
        Activity {
            status: thread.status,
            updated_at: thread.updated_at,
            listed_at: thread.listed_at,
        }
    }
}

/// The one state a row shows, or None when there is nothing to tell. `limited`: the person's plan stopped the last turn
/// at its usage limit (`ThreadSummary::limited`); the row says so while nothing runs (`limit_stop::shown`). `seen_at`: None
/// when never looked at. `pull_needs_you`: the session's pull request has a failed check, a review asking for changes
/// or a conflict the person has not looked at yet.
pub fn attention_of(
    activity: &Activity,
    limited: Option<LimitStop>,
    seen_at: Option<i64>,
    pull_needs_you: bool,
) -> Option<Attention> {
    if pull_needs_you && activity.status != ThreadStatus::Error {
        return Some(Attention::Needs);
    }
    if crate::limit_stop::shown(limited, activity.status).is_some() {
        return Some(Attention::Limited);
    }
    match activity.status {
        ThreadStatus::Approval => Some(Attention::Needs),
        ThreadStatus::Running | ThreadStatus::Starting => Some(Attention::Working),
        ThreadStatus::Error => Some(Attention::Error),
        ThreadStatus::Idle => {
            let moved = activity.listed_at.unwrap_or(activity.updated_at);
            seen_at.filter(|seen| moved > *seen).map(|_| Attention::Unread)
        }
        ThreadStatus::Interrupted => None,
    }
}

/// A session's mark in the sidebar (brand v5 P1, "Session states"): a shape, never a hue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// A turning arc.
    Running,
    /// A filled dot: it needs the person, or finished since they last looked.
    You,
    /// A red cross.
    Failed,
    /// A hollow ring.
    Idle,
    /// A faint ring with a dot going round it: the turn ended, and work its agent started still runs.
    Background,
    /// [`Mark::Background`] with a dot in its middle: also a reply the person has not read.
    BackgroundUnread,
}

/// Whether a session's agent left work running past its turn, from its summary: its listed background work, a
/// background sub-agent or an owed turn (`in_background`, which older hosts also send), or linked sessions it waits on.
pub fn busy_in_background(thread: &ThreadSummary) -> bool {
    !thread.background.is_empty() || thread.in_background || !thread.waits_on.is_empty()
}

/// The mark once background work counts (desktop design "Sessions working in the background"): it outranks an
/// unread reply and a failed turn, never what the person must answer or a turn that runs.
pub fn with_background(mark: Mark, attention: Option<Attention>, background: bool) -> Mark {
    if !background {
        return mark;
    }
    match (mark, attention) {
        (Mark::You, Some(Attention::Unread)) => Mark::BackgroundUnread,
        (Mark::Idle | Mark::Failed, _) => Mark::Background,
        (mark, _) => mark,
    }
}

/// The mark for a session: what it needs from the person first, else how it stands (`sidebarMarkOf`).
pub fn mark_of(status: Option<ThreadStatus>, attention: Option<Attention>) -> Mark {
    if attention == Some(Attention::Error) || status == Some(ThreadStatus::Error) {
        return Mark::Failed;
    }
    if matches!(
        attention,
        Some(Attention::Needs | Attention::Limited | Attention::Unread)
    ) || status == Some(ThreadStatus::Approval)
    {
        return Mark::You;
    }
    if attention == Some(Attention::Working) || matches!(status, Some(ThreadStatus::Running | ThreadStatus::Starting)) {
        return Mark::Running;
    }
    Mark::Idle
}

/// When the person last saw each session: the `updated_at` they saw, by session id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    at: BTreeMap<String, i64>,
}

impl Seen {
    /// The stored map. Anything else reads as empty, and an entry that is not a number is dropped.
    pub fn from_json(text: &str) -> Seen {
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
            return Seen::default();
        };
        Seen {
            at: map
                .into_iter()
                .filter_map(|(id, at)| Some((id, at.as_f64()? as i64)))
                .collect(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.at).unwrap_or_else(|_| "{}".into())
    }

    /// When the person last saw `id`; None when never.
    pub fn get(&self, id: &str) -> Option<i64> {
        self.at.get(id).copied()
    }

    /// The person is looking at `id` as of `updated_at`. Never moves backwards. True when it changed.
    pub fn mark(&mut self, id: &str, updated_at: i64) -> bool {
        if self.get(id).is_some_and(|seen| seen >= updated_at) {
            return false;
        }
        self.at.insert(id.to_string(), updated_at);
        true
    }

    /// Records a first sighting for sessions without an entry, so a fresh install does not light up every old
    /// session (`withBaseline`). `sessions`: each session's id and `updated_at`. True when it changed.
    pub fn baseline<'a>(&mut self, sessions: impl IntoIterator<Item = (&'a str, i64)>) -> bool {
        let mut changed = false;
        for (id, updated_at) in sessions {
            if !self.at.contains_key(id) {
                self.at.insert(id.to_string(), updated_at);
                changed = true;
            }
        }
        changed
    }

    /// Takes `other`'s later times, per session (`mergeSeen`): windows and processes share the storage, each with its
    /// own copy. True when it changed.
    pub fn merge(&mut self, other: &Seen) -> bool {
        let mut changed = false;
        for (id, at) in &other.at {
            if self.get(id).is_none_or(|mine| mine < *at) {
                self.at.insert(id.clone(), *at);
                changed = true;
            }
        }
        changed
    }
}

/// What to do with a changed map: write it now, or once, at the end of a burst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    Now,
    /// Write in this many milliseconds, then call [`Throttle::wrote`].
    After(i64),
    /// A write is already waiting; it takes this change with it.
    Waiting,
}

/// The rate limit on writes: the first mark after a quiet spell is written at once; a burst of them is written once,
/// at its end (`commit` in `attention.ts`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Throttle {
    last: Option<i64>,
    waiting: bool,
}

impl Throttle {
    /// The map changed at `now` (ms).
    pub fn changed(&mut self, now: i64) -> Write {
        let since = self.last.map_or(i64::MAX, |last| now - last);
        if since >= WRITE_EVERY_MS && !self.waiting {
            return Write::Now;
        }
        if self.waiting {
            return Write::Waiting;
        }
        self.waiting = true;
        Write::After(WRITE_EVERY_MS - since)
    }

    /// The map was written at `now` (ms).
    pub fn wrote(&mut self, now: i64) {
        self.last = Some(now);
        self.waiting = false;
    }

    /// A write is waiting for the end of its burst.
    pub fn waiting(&self) -> bool {
        self.waiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ThreadStatus::*;

    const STOP: Option<LimitStop> = Some(LimitStop {
        resets_at: Some(50),
        scheduled: true,
    });

    fn act(status: ThreadStatus, updated_at: i64) -> Activity {
        Activity {
            status,
            updated_at,
            listed_at: None,
        }
    }

    fn listed(status: ThreadStatus, updated_at: i64, listed_at: i64) -> Activity {
        Activity {
            listed_at: Some(listed_at),
            ..act(status, updated_at)
        }
    }

    fn seen(entries: &[(&str, i64)]) -> Seen {
        let mut seen = Seen::default();
        for (id, at) in entries {
            seen.at.insert((*id).to_string(), *at);
        }
        seen
    }

    #[test]
    fn maps_the_live_statuses() {
        assert_eq!(
            attention_of(&act(Approval, 1), None, Some(1), false),
            Some(Attention::Needs)
        );
        assert_eq!(
            attention_of(&act(Running, 1), None, Some(1), false),
            Some(Attention::Working)
        );
        assert_eq!(
            attention_of(&act(Starting, 1), None, Some(1), false),
            Some(Attention::Working)
        );
        assert_eq!(
            attention_of(&act(Error, 1), None, Some(1), false),
            Some(Attention::Error)
        );
    }

    #[test]
    fn calls_an_idle_session_unread_only_when_it_moved_after_you_last_saw_it() {
        assert_eq!(
            attention_of(&act(Idle, 20), None, Some(10), false),
            Some(Attention::Unread)
        );
        assert_eq!(attention_of(&act(Idle, 10), None, Some(10), false), None);
        // Never looked at (no baseline yet): nothing to claim.
        assert_eq!(attention_of(&act(Idle, 20), None, None, false), None);
    }

    #[test]
    fn keeps_a_read_completion_read_after_metadata_and_usage_updates() {
        let finished = listed(Idle, 20, 20);
        let mut marks = Seen::default();
        marks.baseline([("a", finished.updated_at)]);
        let seen_at = marks.get("a");
        // A rename, then usage, then an archive: each moves updated_at, never listed_at.
        for updated_at in [30, 40, 50] {
            assert_eq!(attention_of(&listed(Idle, updated_at, 20), None, seen_at, false), None);
        }
    }

    #[test]
    fn shows_a_later_completion_as_unread_even_after_metadata_changes() {
        let mut marks = Seen::default();
        marks.baseline([("a", 20)]);
        let completed = listed(Idle, 50, 40);
        assert_eq!(
            attention_of(&completed, None, marks.get("a"), false),
            Some(Attention::Unread)
        );
        assert_eq!(attention_of(&completed, None, Some(50), false), None);
        assert_eq!(attention_of(&listed(Idle, 60, 40), None, Some(50), false), None);
    }

    #[test]
    fn keeps_older_hosts_compatible_without_listed_at() {
        let finished = act(Idle, 20);
        assert_eq!(attention_of(&finished, None, Some(10), false), Some(Attention::Unread));
        assert_eq!(attention_of(&finished, None, Some(20), false), None);
    }

    #[test]
    fn says_nothing_for_a_stopped_session() {
        assert_eq!(attention_of(&act(Interrupted, 20), None, Some(10), false), None);
    }

    #[test]
    fn is_limited_while_the_plans_limit_holds_the_stopped_turn_read_or_not() {
        assert_eq!(
            attention_of(&act(Idle, 20), STOP, Some(10), false),
            Some(Attention::Limited)
        );
        assert_eq!(
            attention_of(&act(Idle, 20), STOP, Some(20), false),
            Some(Attention::Limited)
        );
        assert_eq!(
            attention_of(&act(Running, 20), STOP, Some(10), false),
            Some(Attention::Working)
        );
    }

    #[test]
    fn needs_you_when_the_pull_request_does_unless_the_session_failed() {
        assert_eq!(
            attention_of(&act(Idle, 10), None, Some(10), true),
            Some(Attention::Needs)
        );
        assert_eq!(
            attention_of(&act(Running, 10), None, Some(10), true),
            Some(Attention::Needs)
        );
        assert_eq!(
            attention_of(&act(Error, 10), None, Some(10), true),
            Some(Attention::Error)
        );
    }

    #[test]
    fn labels_each_state() {
        assert_eq!(Attention::Unread.label(), "Finished, unread");
        assert_eq!(Attention::Limited.label(), "Limited");
        assert_eq!(Attention::Needs.label(), "Needs you");
    }

    #[test]
    fn the_activity_of_a_summary_reads_its_completion() {
        let json = r#"{"id":"a","projectId":"p","title":"t","agent":"claude","model":null,"permissionMode":"default",
            "via":"nexus","status":"idle","costUsd":0,"nexusSessions":[],"contextTokens":null,"contextWindow":null,
            "createdAt":1,"updatedAt":30,"listedAt":20,"archived":false}"#;
        let thread: ThreadSummary = serde_json::from_str(json).expect("a summary");
        assert_eq!(
            Activity::of(&thread),
            Activity {
                status: Idle,
                updated_at: 30,
                listed_at: Some(20),
            }
        );
    }

    // sidebar-mark.test.ts
    #[test]
    fn puts_a_failure_first_then_what_is_for_you_then_work_under_way() {
        assert_eq!(mark_of(Some(Running), Some(Attention::Error)), Mark::Failed);
        assert_eq!(mark_of(Some(Error), None), Mark::Failed);
        assert_eq!(mark_of(Some(Running), Some(Attention::Needs)), Mark::You);
        assert_eq!(mark_of(Some(Idle), Some(Attention::Unread)), Mark::You);
        assert_eq!(mark_of(Some(Idle), Some(Attention::Limited)), Mark::You);
        assert_eq!(mark_of(Some(Approval), None), Mark::You);
        assert_eq!(mark_of(Some(Starting), None), Mark::Running);
        assert_eq!(mark_of(Some(Idle), Some(Attention::Working)), Mark::Running);
    }

    #[test]
    fn reads_idle_otherwise() {
        assert_eq!(mark_of(Some(Idle), None), Mark::Idle);
        assert_eq!(mark_of(Some(Interrupted), None), Mark::Idle);
        assert_eq!(mark_of(None, None), Mark::Idle);
    }

    #[test]
    fn background_work_outranks_an_unread_reply_and_a_failure_but_not_the_person_or_a_turn() {
        use Attention::*;
        let bg = |mark, attention| with_background(mark, attention, true);
        assert_eq!(bg(Mark::Idle, None), Mark::Background);
        assert_eq!(bg(Mark::Failed, Some(Error)), Mark::Background);
        assert_eq!(bg(Mark::You, Some(Unread)), Mark::BackgroundUnread);
        assert_eq!(bg(Mark::You, Some(Needs)), Mark::You);
        assert_eq!(bg(Mark::You, Some(Limited)), Mark::You);
        assert_eq!(bg(Mark::Running, Some(Working)), Mark::Running);
        assert_eq!(with_background(Mark::Idle, None, false), Mark::Idle);
        assert_eq!(with_background(Mark::You, Some(Unread), false), Mark::You);
    }

    #[test]
    fn a_summary_is_busy_in_the_background_by_its_list_or_by_an_older_hosts_fields() {
        let json = r#"{"id":"a","projectId":"p","title":"t","agent":"claude","model":null,"permissionMode":"default",
            "via":"nexus","status":"idle","costUsd":0,"nexusSessions":[],"contextTokens":null,"contextWindow":null,
            "createdAt":1,"updatedAt":30,"archived":false}"#;
        let mut thread: ThreadSummary = serde_json::from_str(json).expect("a summary");
        assert!(!busy_in_background(&thread));
        thread.in_background = true;
        assert!(busy_in_background(&thread));
        thread.in_background = false;
        thread.waits_on = vec!["child".into()];
        assert!(busy_in_background(&thread));
        thread.waits_on.clear();
        thread.background = vec![crate::event::BackgroundWork {
            kind: crate::event::BackgroundKind::Shell,
            title: "cargo test".into(),
            started_at: None,
        }];
        assert!(busy_in_background(&thread));
    }

    #[test]
    fn baseline_adds_first_sightings_only() {
        let mut marks = seen(&[("a", 5)]);
        assert!(!marks.baseline([("a", 9)]));
        assert_eq!(marks, seen(&[("a", 5)]));
        assert!(marks.baseline([("a", 9), ("b", 7)]));
        assert_eq!(marks, seen(&[("a", 5), ("b", 7)]));
    }

    #[test]
    fn merges_two_windows_marks_keeping_the_later_time_per_session() {
        let mut mine = seen(&[("a", 5), ("b", 9)]);
        assert!(!mine.merge(&seen(&[("a", 3)])));
        assert!(mine.merge(&seen(&[("a", 7), ("c", 1)])));
        assert_eq!(mine, seen(&[("a", 7), ("b", 9), ("c", 1)]));
    }

    #[test]
    fn a_mark_never_moves_backwards() {
        let mut marks = Seen::default();
        assert!(marks.mark("a", 10));
        assert!(!marks.mark("a", 10));
        assert!(!marks.mark("a", 5));
        assert!(marks.mark("a", 11));
        assert_eq!(marks.get("a"), Some(11));
    }

    #[test]
    fn reads_and_writes_the_stored_shape() {
        let marks = Seen::from_json(r#"{"a":1731164400000,"b":7,"bad":"x"}"#);
        assert_eq!(marks, seen(&[("a", 1_731_164_400_000), ("b", 7)]));
        assert_eq!(Seen::from_json(&marks.to_json()), marks);
        assert_eq!(Seen::from_json("[1]"), Seen::default());
        assert_eq!(Seen::from_json("not json"), Seen::default());
    }

    // attention-writes.test.ts: the mark is instant; the storage write is rate limited.
    /// Runs marks at `times` (ms), and returns the write times, firing a waiting write when its time comes.
    fn writes(times: impl IntoIterator<Item = i64>, end: i64) -> Vec<i64> {
        let mut throttle = Throttle::default();
        let mut due: Option<i64> = None;
        let mut written = Vec::new();
        let fire = |throttle: &mut Throttle, due: &mut Option<i64>, until: i64, written: &mut Vec<i64>| {
            if let Some(at) = due.filter(|at| *at <= until) {
                throttle.wrote(at);
                written.push(at);
                *due = None;
            }
        };
        for now in times {
            fire(&mut throttle, &mut due, now, &mut written);
            match throttle.changed(now) {
                Write::Now => {
                    throttle.wrote(now);
                    written.push(now);
                }
                Write::After(delay) => due = Some(now + delay),
                Write::Waiting => {}
            }
        }
        fire(&mut throttle, &mut due, end, &mut written);
        written
    }

    #[test]
    fn writes_a_burst_of_200_marks_about_three_times_not_200() {
        let start = 10_000_000;
        let times = (1..=200).map(|i| start + i * 5);
        let written = writes(times, start + 200 * 5 + 2000);
        assert!(written.len() <= 3, "{written:?}");
        assert!(written.last().is_some_and(|last| *last >= start + 1000));
    }

    #[test]
    fn writes_the_first_mark_at_once_and_the_next_one_at_the_end_of_the_window() {
        let mut throttle = Throttle::default();
        assert_eq!(throttle.changed(0), Write::Now);
        throttle.wrote(0);
        assert_eq!(throttle.changed(0), Write::After(1000));
        assert!(throttle.waiting());
        assert_eq!(throttle.changed(500), Write::Waiting);
        throttle.wrote(1000);
        assert!(!throttle.waiting());
        assert_eq!(throttle.changed(2500), Write::Now);
    }

    #[test]
    fn keeps_what_another_window_marked() {
        // The write merges the stored map into this window's before writing, as `flushSeen` does.
        let mut mine = Seen::default();
        mine.mark("a", 1);
        let stored = Seen::from_json(r#"{"a":1,"b":7}"#);
        mine.mark("a", 2);
        mine.merge(&stored);
        assert_eq!(mine, seen(&[("a", 2), ("b", 7)]));
    }
}
