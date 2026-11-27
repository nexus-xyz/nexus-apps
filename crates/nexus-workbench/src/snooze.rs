//! Putting a session aside: snoozed until a time, or settled once its pull request merged. Both keep the
//! session and its worktree; the sidebar only lists it in a folded group ("Snoozed", "Settled") instead of with the
//! active ones.
//!
//! A snooze is a wake time, stored with the session: the session is snoozed while the clock is before it, so it comes
//! back at its time with nothing to run. It comes back early when it needs the person (`Workbench::change_at` clears it
//! when the session asks for an approval or a question) and when its pull request watch tells news (`Workbench::wake`,
//! from the app's pr_watch.rs).
//!
//! Settling reads the merge the app's pull request watch already reads: no lookup of its own. A merged pull request
//! settles its session once (`settled_by` keeps its number), so a session the person resumed on the same branch is not
//! settled again by the same merge. A busy session never settles: the watch reads the pull request again when its turn
//! ends. "Keep in list" (`keep_listed`) opts the session out, and brings a settled one back.

use serde::{Deserialize, Serialize};

use crate::event::ThreadSummary;

/// Where the session stands, aside from the active list. Every field is stored; an active session writes none of
/// them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Aside {
    /// Snoozed until this time (ms since the epoch). A time in the past is awake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<i64>,
    /// When its pull request's merge settled it; None while it is in the active list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<i64>,
    /// The pull request whose merge settled it. Kept after the session comes back: that merge does not settle it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_by: Option<u64>,
    /// The person opted out: a merge never settles it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_listed: bool,
}

/// Whether the session is snoozed at `now`.
pub fn snoozed(summary: &ThreadSummary, now: i64) -> bool {
    summary.aside.snoozed_until.is_some_and(|until| until > now)
}

/// Whether the merge of pull request `number` settles the session: not busy, not archived, not settled yet, not opted
/// out, and not settled by that merge before.
pub fn settles(summary: &ThreadSummary, number: u64) -> bool {
    let aside = &summary.aside;
    !summary.status.busy()
        && !summary.archived
        && aside.settled_at.is_none()
        && !aside.keep_listed
        && aside.settled_by != Some(number)
}

/// Parse the custom form's local date and time. Reject unreadable, nonexistent, or past times.
pub fn custom_time<T: chrono::TimeZone>(value: &str, now: chrono::DateTime<T>) -> Option<i64> {
    let local = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M").ok()?;
    let at = now
        .timezone()
        .from_local_datetime(&local)
        .earliest()?
        .timestamp_millis();
    (at > now.timestamp_millis()).then_some(at)
}

/// Tomorrow at 09:00 in the person's local calendar, as the custom form reads it.
pub fn custom_default<T: chrono::TimeZone>(now: chrono::DateTime<T>) -> String {
    now.date_naive()
        .succ_opt()
        .unwrap_or(now.date_naive())
        .and_hms_opt(9, 0, 0)
        .expect("nine is a valid hour")
        .format("%Y-%m-%dT%H:%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ThreadStatus;

    #[test]
    fn custom_time_uses_local_time_and_requires_a_future_instant() {
        use chrono::TimeZone;
        let zone = chrono::FixedOffset::west_opt(3 * 3600).expect("a valid UTC offset");
        let now = zone.with_ymd_and_hms(2026, 12, 31, 22, 0, 0).unwrap();
        assert_eq!(custom_default(now), "2027-01-01T09:00");
        assert_eq!(
            custom_time("2027-01-01T09:00", now),
            Some(zone.with_ymd_and_hms(2027, 1, 1, 9, 0, 0).unwrap().timestamp_millis())
        );
        for value in [
            "",
            "garbage",
            "2026-12-31T22:00",
            "2026-12-30T09:00",
            "2026-02-30T09:00",
        ] {
            assert_eq!(custom_time(value, now), None, "{value}");
        }
    }

    fn summary() -> ThreadSummary {
        serde_json::from_value(serde_json::json!({
            "id": "t_1", "projectId": "p", "title": "Fix the login", "agent": "claude", "model": null,
            "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0, "nexusSessions": [],
            "contextTokens": null, "contextWindow": null, "createdAt": 0, "updatedAt": 0, "archived": false,
        }))
        .expect("a summary")
    }

    #[test]
    fn a_snooze_holds_until_its_time_and_not_after() {
        let mut thread = summary();
        assert!(!snoozed(&thread, 1_000), "nothing set: awake");
        thread.aside.snoozed_until = Some(5_000);
        assert!(snoozed(&thread, 1_000));
        assert!(snoozed(&thread, 4_999));
        assert!(!snoozed(&thread, 5_000), "back at its time");
        assert!(!snoozed(&thread, 9_000));
    }

    #[test]
    fn a_merge_settles_an_idle_session_once() {
        let mut thread = summary();
        thread.status = ThreadStatus::Idle;
        assert!(settles(&thread, 7));
        thread.aside.settled_at = Some(1);
        thread.aside.settled_by = Some(7);
        assert!(!settles(&thread, 7), "settled already");
        thread.aside.settled_at = None;
        assert!(
            !settles(&thread, 7),
            "resumed after that merge: the same merge does not settle it again"
        );
        assert!(settles(&thread, 8), "a later pull request's merge does");
    }

    #[test]
    fn a_busy_archived_or_kept_session_never_settles() {
        for status in [ThreadStatus::Starting, ThreadStatus::Running, ThreadStatus::Approval] {
            let mut thread = summary();
            thread.status = status;
            assert!(!settles(&thread, 1), "{status:?} never settles");
        }
        let mut archived = summary();
        archived.archived = true;
        assert!(!settles(&archived, 1));
        let mut kept = summary();
        kept.aside.keep_listed = true;
        assert!(!settles(&kept, 1), "Keep in list opts it out");
    }

    #[test]
    fn an_active_session_writes_none_of_it() {
        let json = serde_json::to_value(Aside::default()).expect("json");
        assert_eq!(json, serde_json::json!({}));
        let read: Aside = serde_json::from_value(serde_json::json!({})).expect("older summaries read");
        assert_eq!(read, Aside::default());
    }
}
