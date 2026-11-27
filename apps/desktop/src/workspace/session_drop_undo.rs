//! The way back from a drop, with no UI: a drop that changes a session says so in the notice at the foot
//! of the window, with Undo, as Archive does (`archive.rs`). A drop that only moves a session in its project shows no
//! notice: the move is in sight, and dragging it back undoes it.

use nexus_workbench::api::UpdateArgs;

use super::session_drop::{Plan, Put};

/// What the session was before the drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Before {
    pub pinned: bool,
    pub settled: bool,
    pub snoozed_until: Option<i64>,
    pub archived: bool,
}

/// What Undo puts back: the fields the drop changed, as they were, and the project's order before the drop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropUndo {
    pub id: String,
    pub pinned: Option<bool>,
    pub settled: Option<bool>,
    pub snoozed_until: Option<i64>,
    pub archived: Option<bool>,
    /// The project, by its lead's id, and its order before the drop (empty when it had none).
    pub order: Option<(String, Vec<String>)>,
}

impl DropUndo {
    /// The drop's way back from `args`, the fields it changed, given what the session was `before`.
    pub(super) fn of(args: &UpdateArgs, before: Before, order: Option<(String, Vec<String>)>) -> Self {
        DropUndo {
            id: args.thread_id.clone(),
            pinned: args.pinned.map(|_| before.pinned),
            settled: args.settled.map(|_| before.settled),
            // 0 wakes it: a session that was not snoozed is awake again.
            snoozed_until: args.snoozed_until.map(|_| before.snoozed_until.unwrap_or(0)),
            archived: args.archived.map(|_| before.archived),
            order,
        }
    }

    /// `wb_thread_update`'s arguments that put the fields back.
    pub(super) fn args(&self) -> UpdateArgs {
        UpdateArgs {
            thread_id: self.id.clone(),
            pinned: self.pinned,
            settled: self.settled,
            snoozed_until: self.snoozed_until,
            archived: self.archived,
            ..UpdateArgs::default()
        }
    }
}

impl super::Workspace {
    /// Undo on a drop's notice: the project's order as it was, then each session's fields. One drop of several
    /// sessions has one Undo for all of them.
    pub(super) fn undo_drop(&mut self, undo: Vec<DropUndo>, cx: &mut gpui_kit::Context<Self>) {
        for undo in undo {
            if let Some((project, order)) = &undo.order {
                self.groups.groups.set_session_order(project, order.clone());
                self.save_groups();
            }
            let args = undo.args();
            let changed = args.pinned.is_some()
                || args.settled.is_some()
                || args.snoozed_until.is_some()
                || args.archived.is_some();
            if changed {
                self.change_session(args, cx);
            }
        }
        cx.notify();
    }
}

/// The notice's words for `plan` on `count` sessions, the first titled `title`; None for a drop that only moves them.
/// `snoozed` is the snooze's wake time, in words.
pub(super) fn notice_text(plan: &Plan, title: &str, count: usize, snoozed: Option<&str>) -> Option<String> {
    let what = if count > 1 {
        format!("{count} sessions")
    } else {
        format!("“{title}”")
    };
    match (plan.put, plan.pin) {
        (Put::Settle, _) => Some(format!("Settled {what}")),
        (Put::Snooze(_), _) => Some(match (snoozed, count > 1) {
            (Some(wake), false) => format!("Snoozed until {wake}"),
            (Some(wake), true) => format!("Snoozed {what} until {wake}"),
            (None, _) => format!("Snoozed {what}"),
        }),
        (Put::Return, _) if count > 1 => Some(format!("{what} are back in their projects")),
        (Put::Return, _) => Some(format!("{what} is back in its project")),
        (Put::Stay, Some(true)) => Some(format!("Pinned {what}")),
        (Put::Stay, Some(false)) => Some(format!("Unpinned {what}")),
        (Put::Stay, None) | (Put::Archive, _) => None,
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    fn plan(put: Put, pin: Option<bool>) -> Plan {
        Plan { put, pin, spot: None }
    }

    #[test]
    fn undo_puts_back_only_the_fields_the_drop_changed() {
        let args = UpdateArgs {
            thread_id: "a".into(),
            settled: Some(true),
            snoozed_until: Some(0),
            ..UpdateArgs::default()
        };
        let before = Before {
            pinned: true,
            settled: false,
            snoozed_until: Some(99),
            archived: false,
        };
        let undo = DropUndo::of(&args, before, Some(("p".into(), vec!["a".into(), "b".into()])));
        assert_eq!(
            (undo.pinned, undo.settled, undo.snoozed_until, undo.archived),
            (None, Some(false), Some(99), None)
        );
        let back = undo.args();
        assert_eq!(back.thread_id, "a");
        assert_eq!(
            (back.settled, back.snoozed_until, back.pinned),
            (Some(false), Some(99), None)
        );
        let snooze = UpdateArgs {
            thread_id: "a".into(),
            snoozed_until: Some(5),
            ..UpdateArgs::default()
        };
        let awake = Before {
            snoozed_until: None,
            ..before
        };
        assert_eq!(DropUndo::of(&snooze, awake, None).snoozed_until, Some(0), "woken again");
    }

    #[test]
    fn a_drop_that_only_moves_a_session_says_nothing() {
        let text = |put, pin, count, wake| notice_text(&plan(put, pin), "Fix", count, wake);
        assert_eq!(text(Put::Stay, None, 1, None), None);
        assert_eq!(text(Put::Archive, None, 1, None), None, "archive.rs says it");
        assert_eq!(text(Put::Stay, Some(true), 1, None).as_deref(), Some("Pinned “Fix”"));
        assert_eq!(
            text(Put::Settle, Some(false), 1, None).as_deref(),
            Some("Settled “Fix”")
        );
        assert_eq!(
            text(Put::Snooze(None), None, 1, Some("tomorrow at 9:00")).as_deref(),
            Some("Snoozed until tomorrow at 9:00")
        );
        assert_eq!(
            text(Put::Return, Some(true), 1, None).as_deref(),
            Some("“Fix” is back in its project")
        );
    }

    #[test]
    fn a_drop_of_several_sessions_counts_them() {
        let text = |put, count, wake| notice_text(&plan(put, None), "Fix", count, wake);
        assert_eq!(text(Put::Settle, 3, None).as_deref(), Some("Settled 3 sessions"));
        assert_eq!(
            text(Put::Snooze(Some(1)), 2, Some("6:00 PM")).as_deref(),
            Some("Snoozed 2 sessions until 6:00 PM")
        );
        assert_eq!(
            text(Put::Return, 2, None).as_deref(),
            Some("2 sessions are back in their projects")
        );
    }
}
