//! Mid-turn steers from a parent session to a running direct child.
//!
//! Only agents with `SessionFeatures::steers_mid_turn` accept a steer mid-turn.
//! Every other busy target returns `OrchCode::Busy` with [`BUSY`].

use super::{OrchCode, OrchError, refuse};
use crate::Workbench;
use crate::event::ThreadStatus;
use crate::harness::harness;
use crate::store::ThreadRecord;

/// Refusal text when `message_session` cannot deliver to a busy target.
pub(crate) const BUSY: &str = "That session is at work and cannot take a message now. Wait for its turn to end, or \
                               call stop_session and message it again.";

/// Refusal text when `message_session` cannot deliver to a busy peer. A peer is not a child, so `stop_session` is
/// not offered.
pub(crate) const PEER_BUSY: &str =
    "That session is at work and cannot take a message now. Wait for its turn to end, then message it again.";

/// Refusal text when `message_child` targets an archived child session.
pub(crate) const ARCHIVED: &str = "That session is archived and cannot take messages. Restore it first, or start a \
                                   new session.";

/// Whether a session is idle, in a live turn, or unavailable for a message.
pub(super) enum BusyState {
    /// No active turn, no pending input, not stopping, and not starting.
    Idle,
    /// Live process with an active turn or lined-up input, neither done nor stopping.
    ActiveTurn,
    /// Starting, stopping, or exited while still registered.
    Unavailable,
}

/// How `message_child` delivers to a direct child based on its current run state.
pub(super) enum ChildDelivery {
    /// The child has no active turn or pending input. Start a normal message turn.
    Idle,
    /// The child runs an agent that takes steers mid-turn. Deliver at its next step.
    Steer,
    /// The child is starting, stopping, done, or runs an agent that cannot steer mid-turn.
    Busy,
}

/// Formats a parent's steer so the child sees which session sent it and knows it is not from the person.
///
/// It follows the same "Message from ... not the person's" framing as the idle and peer paths, and avoids every
/// notice marker other readers inspect (`Workbench::steer_notice`).
pub(crate) fn steer_text(parent_title: &str, parent_id: &str, text: &str) -> String {
    format!(
        "Message from session \"{parent_title}\" ({parent_id}). It is that session's request, not the person's.\n\n{text}"
    )
}

impl Workbench {
    /// Classifies whether `thread_id` is idle, in an active live turn, or unavailable.
    pub(super) fn busy_state(&self, thread_id: &str) -> Result<BusyState, OrchError> {
        if let Some(live) = self.live(thread_id) {
            let state = live.state();
            if state.done || state.stop.is_some() {
                return Ok(BusyState::Unavailable);
            }
            if !state.turns.is_empty() || !state.unsent.is_empty() {
                return Ok(BusyState::ActiveTurn);
            }
            return Ok(BusyState::Idle);
        }
        let starting = self
            .reader()
            .thread(thread_id)?
            .is_some_and(|record| record.summary.status == ThreadStatus::Starting);
        Ok(if starting {
            BusyState::Unavailable
        } else {
            BusyState::Idle
        })
    }

    /// Classifies how a direct child can accept a parent's message right now.
    pub(super) fn child_delivery(&self, child: &ThreadRecord) -> Result<ChildDelivery, OrchError> {
        Ok(match self.busy_state(&child.summary.id)? {
            BusyState::Idle => ChildDelivery::Idle,
            BusyState::ActiveTurn if harness(child.summary.agent).session_features().steers_mid_turn => {
                ChildDelivery::Steer
            }
            BusyState::ActiveTurn | BusyState::Unavailable => ChildDelivery::Busy,
        })
    }

    /// Steers a running direct child with a marked notice from `parent`.
    ///
    /// Refunds the counted automatic turn on failure. A halt refusal returns `halted`.
    /// Any other delivery failure returns `busy`.
    pub(super) fn steer_running_child(
        &self,
        parent: &ThreadRecord,
        child_id: &str,
        text: &str,
        root: &str,
        pauses: bool,
    ) -> Result<(), OrchError> {
        let marked = steer_text(&parent.summary.title, &parent.summary.id, text);
        if let Err(error) = self.steer_notice(child_id, &marked) {
            self.give_back_turn(root);
            if super::halts::is_refusal(&error) {
                return refuse(OrchCode::Halted, error);
            }
            return refuse(OrchCode::Busy, BUSY);
        }
        if pauses {
            self.emit_tree(root);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steer_text_marks_parent_and_avoids_special_notice_markers() {
        let text = steer_text("Coordinator", "t_parent", "Include the command in your notice.");
        assert_eq!(
            text,
            "Message from session \"Coordinator\" (t_parent). It is that session's request, not the \
             person's.\n\nInclude the command in your notice."
        );
        assert!(!text.starts_with(crate::moving::MOVED));
        assert!(!text.to_lowercase().contains("compacted the conversation"));
    }

    #[test]
    fn busy_text_matches_contract_and_sentence_limit() {
        assert_eq!(
            BUSY,
            "That session is at work and cannot take a message now. Wait for its turn to end, or call stop_session \
             and message it again."
        );
        for sentence in BUSY.split(". ") {
            assert!(sentence.split_whitespace().count() <= 20, "{sentence}");
        }
    }

    #[test]
    fn failed_steer_refunds_the_counted_automatic_turn_and_returns_busy() {
        use std::sync::Arc;

        use futures::future::BoxFuture;
        use serde_json::json;

        use crate::event::{ThreadSummary, Usage};
        use crate::linked::Limits;
        use crate::store::Store;
        use crate::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd};

        struct NoopHost;
        impl Host for NoopHost {
            fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, crate::Result<Launch>> {
                Box::pin(async { Err("unused".into()) })
            }
            fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
                Box::pin(async { true })
            }
            fn price(&self, _: &str, _: &Usage) -> Option<f64> {
                None
            }
            fn emit(&self, _: Emit) {}
        }

        let store = Store::in_memory().expect("in-memory store");
        let config = Config {
            scope: "gw|org".into(),
            orchestration: Limits {
                max_auto_turns: Some(2),
                ..Default::default()
            },
            ..Config::default()
        };
        let wb = Workbench::new(store, Arc::new(NoopHost), config).expect("workbench");
        let record = |id: &str, title: &str, parent: Option<&str>| -> ThreadRecord {
            let summary: ThreadSummary = serde_json::from_value(json!({
                "id": id, "projectId": "p_1", "title": title, "agent": "claude", "model": null,
                "permissionMode": "acceptEdits", "via": "nexus", "status": "idle", "costUsd": 0.0,
                "nexusSessions": [], "contextTokens": null, "contextWindow": null,
                "createdAt": 0, "updatedAt": 0, "archived": false,
                "orchestration": true, "parentThreadId": parent,
            }))
            .expect("thread summary");
            ThreadRecord {
                summary,
                native_id: format!("native_{id}"),
                native_started: false,
                scope: "gw|org".into(),
                fork_from: None,
                seed: None,
                automatic_title: false,
                delivered_seq: 0,
                halt: None,
            }
        };
        let parent = record("t_parent", "Coordinator", None);
        let child = record("t_child", "Fixer", Some("t_parent"));
        {
            let store = wb.store();
            store.insert_thread(&parent).expect("insert parent");
            store.insert_thread(&child).expect("insert child");
        }
        assert_eq!(wb.list_children("t_parent").expect("list").limits.turns_left, Some(2));
        wb.trees().entry("t_parent".into()).or_default().auto_turns += 1;
        assert_eq!(wb.list_children("t_parent").expect("list").limits.turns_left, Some(1));

        let err = wb
            .steer_running_child(&parent, "t_child", "Redirect", "t_parent", false)
            .expect_err("non-live child refuses steer");
        assert_eq!(err.code, OrchCode::Busy);
        assert_eq!(err.message, BUSY);
        assert_eq!(wb.list_children("t_parent").expect("list").limits.turns_left, Some(2));
    }
}
