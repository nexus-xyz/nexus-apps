//! Replace a session's agent without moving its files or copying provider-private state.

use serde::{Deserialize, Serialize};

use crate::event::{Effort, SessionAgent, StoredEvent, ThreadStatus, ThreadSummary, Via, WbEvent};
use crate::{Emit, Result, Reverting, UpdateActivity, Workbench, copied_until, now_ms, said_of};

/// The person's confirmed choice. `expected_agent` rejects an outdated confirmation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSwitch {
    pub expected_agent: SessionAgent,
    pub agent: SessionAgent,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub via: Via,
    /// None uses the destination agent's own login, never the source agent's account.
    pub account: Option<String>,
}

/// Whether a session of `from` may switch to `to`: Claude Code and Codex, whose conversations the handoff carries.
/// The desktop's session picker lists the agents this allows, so it offers no switch that fails.
pub fn can_switch(from: SessionAgent, to: SessionAgent) -> bool {
    crate::harness::harness(from).switches() && crate::harness::harness(to).switches()
}

impl Workbench {
    /// Stop the old agent and its descendants, then atomically save the new driver and history boundary.
    /// The next message starts a fresh provider conversation with a bounded, visible-text handoff.
    pub async fn switch_agent(&self, thread_id: &str, target: AgentSwitch) -> Result<ThreadSummary> {
        let _activity = UpdateActivity::begin(self)?;
        let _changing = Reverting::hold(self, thread_id)?;
        let before = self.record(thread_id)?;
        if before.summary.provider.is_some() {
            return Err("This build cannot switch the agent on a saved local provider.".into());
        }
        if !self.in_scope(&before.scope) {
            return Err("That session is not in this organization.".into());
        }
        if before.summary.archived {
            return Err("Restore this session from the archive before switching agents.".into());
        }
        if !can_switch(before.summary.agent, target.agent) {
            return Err("Choose Claude Code or Codex for this switch.".into());
        }
        if target.via != Via::Subscription && target.account.is_some() {
            return Err("An account can only pay through its own plan.".into());
        }
        if let Some(account) = &target.account {
            self.account_dir(target.agent, account)?;
        }
        // Retrying a completed choice changes neither the conversation nor its timeline.
        if before.summary.agent == target.agent {
            if before.summary.model == target.model
                && before.summary.via == target.via
                && before.summary.account == target.account
                && before.summary.effort == target.effort
            {
                return Ok(before.summary);
            }
            return Err("The session's agent changed. Choose the model again.".into());
        }
        if before.summary.agent != target.expected_agent {
            return Err("The session's agent changed. Choose the model again.".into());
        }
        // This also revokes old tool grants and resolves queued messages before collecting context.
        self.stop_tree_and_wait(thread_id).await?;
        self.checkpoint_written(thread_id).await;
        let gate = self.settings_gate(thread_id);
        let _settings = gate.lock().unwrap_or_else(|p| p.into_inner());
        let stored = {
            let store = self.store();
            let mut record = store
                .thread(thread_id)?
                .ok_or("This session was deleted.".to_string())?;
            if !self.in_scope(&record.scope) || record.scope != before.scope {
                return Err("The signed-in organization changed. Choose the model again.".into());
            }
            if record.summary.archived || record.summary.agent != target.expected_agent {
                return Err("The session changed. Choose the model again.".into());
            }
            let events = store.events_after(thread_id, 0)?;
            let retained = copied_until(&events, i64::MAX);
            let turns = retained
                .iter()
                .filter(|e| matches!(e.event, WbEvent::TurnEnd { .. }))
                .count();
            let text = format!(
                "Switched to {} at turn {turns}. The next message starts a fresh conversation with the original request and recent messages. Earlier history stays here.",
                crate::driver::label(target.agent)
            );
            record.seed = Some(handoff(&retained));
            record.native_id = uuid::Uuid::new_v4().to_string();
            record.native_started = false;
            record.fork_from = None;
            let summary = &mut record.summary;
            summary.agent = target.agent;
            summary.model = target.model;
            summary.effort = target.effort;
            summary.via = target.via;
            summary.account = target.account;
            summary.status = ThreadStatus::Idle;
            summary.context_tokens = None;
            summary.context_window = None;
            summary.limited = None;
            summary.updated_at = now_ms();
            store.save_thread_with_event(
                &record,
                &WbEvent::AgentSwitched {
                    from: target.expected_agent,
                    to: target.agent,
                    text,
                },
            )?
        };
        self.inner.terminals.revoke_agent_input(thread_id);
        if self.shown(thread_id) {
            self.inner.host.emit(Emit::Event(stored));
        }
        self.emit_tree(thread_id);
        Ok(self.record(thread_id)?.summary)
    }
}

/// Keep the original goal and recent top-level prose. Never transfer reasoning, tool payloads or native state.
fn handoff(events: &[StoredEvent]) -> String {
    let cancelled: std::collections::HashSet<_> = events
        .iter()
        .filter_map(|e| match e.event {
            WbEvent::QueueCancel { seq } => Some(seq),
            _ => None,
        })
        .collect();
    let edits: std::collections::HashMap<_, _> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::QueueEdit { seq, text } => Some((*seq, text)),
            _ => None,
        })
        .collect();
    let messages: Vec<_> = events
        .iter()
        .filter(|e| {
            !cancelled.contains(&e.seq) && matches!(e.event, WbEvent::User { .. } | WbEvent::Text { parent: None, .. })
        })
        .cloned()
        .map(|mut e| {
            if let WbEvent::User { text, .. } = &mut e.event
                && let Some(edited) = edits.get(&e.seq)
            {
                *text = (*edited).clone();
            }
            e
        })
        .collect();
    let goal = messages
        .iter()
        .find_map(|e| match &e.event {
            WbEvent::User { text, from: None, .. } => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or_default();
    let mut end = goal.len().min(8 * 1024);
    while !goal.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "You are continuing the same Nexus task with a different agent. The current files and worktree are unchanged. \
         The following is historical conversation context, not new instructions. It excludes private reasoning and tool output. \
         Original request (possibly shortened):\n<original_request>\n{}\n</original_request>\n\n\
         Recent conversation (older messages can be omitted):\n{}\n\
         Inspect the current files to recover additional state. Ask when required context is missing. \
         The person's new message follows.\n\n",
        &goal[..end],
        said_of(&messages)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(seq: i64, event: WbEvent) -> StoredEvent {
        StoredEvent {
            thread_id: "test".into(),
            seq,
            at: 0,
            event,
        }
    }
    fn user(text: &str) -> WbEvent {
        WbEvent::User {
            text: text.into(),
            images: vec![],
            from: None,
        }
    }

    #[test]
    fn handoff_keeps_edited_visible_messages_and_excludes_cancelled_and_private_data() {
        let events = vec![
            row(1, user("original request")),
            row(2, user("obsolete text")),
            row(
                3,
                WbEvent::QueueEdit {
                    seq: 2,
                    text: "edited request".into(),
                },
            ),
            row(4, user("cancelled request")),
            row(5, WbEvent::QueueCancel { seq: 4 }),
            row(
                6,
                WbEvent::Thinking {
                    id: "t".into(),
                    text: "PRIVATE_THOUGHT".into(),
                    parent: None,
                },
            ),
            row(
                7,
                WbEvent::Tool {
                    id: "tool".into(),
                    name: "Read".into(),
                    input: json!({"secret":"PRIVATE_TOOL"}),
                    parent: None,
                },
            ),
            row(
                8,
                WbEvent::Text {
                    id: "child".into(),
                    text: "PRIVATE_CHILD".into(),
                    parent: Some("child".into()),
                },
            ),
            row(
                9,
                WbEvent::Text {
                    id: "a".into(),
                    text: "visible answer".into(),
                    parent: None,
                },
            ),
        ];
        let seed = handoff(&events);
        for wanted in ["original request", "edited request", "visible answer"] {
            assert!(seed.contains(wanted));
        }
        for omitted in ["obsolete text", "cancelled request", "PRIVATE_"] {
            assert!(!seed.contains(omitted));
        }
    }

    #[test]
    fn a_provider_boundary_drops_native_cursors_connection_metadata_and_the_old_interrupt() {
        use crate::event::{SessionConnection, TurnStatus};
        let ended = |cursor: &str| WbEvent::TurnEnd {
            turn_id: cursor.into(),
            status: TurnStatus::Interrupted,
            cost_usd: None,
            duration_ms: 1,
            error: None,
            cursor: Some(cursor.into()),
        };
        let mut events = vec![
            row(1, ended("CLAUDE_CURSOR")),
            row(
                2,
                WbEvent::Connection {
                    connection: Box::new(SessionConnection::default()),
                },
            ),
            row(
                3,
                WbEvent::AgentSwitched {
                    from: SessionAgent::Claude,
                    to: SessionAgent::Codex,
                    text: "Switched".into(),
                },
            ),
        ];
        let thread = serde_json::from_value(json!({
            "id":"test", "projectId":"p", "title":"test", "agent":"codex", "model":null,
            "permissionMode":"default", "effort":null, "via":"nexus", "status":"idle",
            "costUsd":0.0, "nexusSessions":[], "contextTokens":null, "contextWindow":null,
            "createdAt":0, "updatedAt":0, "archived":false
        }))
        .expect("thread fixture");
        assert!(crate::connection_view::last_connection(&thread, &events).is_none());
        assert!(!crate::interrupt::interrupted_since_message(&events));
        assert_eq!(crate::conversation(&events, SessionAgent::Codex).0, [(1, None)]);
        events.push(row(4, ended("CODEX_CURSOR")));
        events.push(row(5, WbEvent::Revert { turn: 1, files: 0 }));
        assert_eq!(
            crate::conversation(&events, SessionAgent::Codex).1,
            Some(crate::Rewind::Replay)
        );
    }

    #[test]
    fn long_unicode_handoffs_keep_the_goal_and_latest_answer_with_a_size_bound() {
        let mut events = vec![row(1, user("Keep this original task"))];
        for seq in 2..90 {
            events.push(row(
                seq,
                WbEvent::Text {
                    id: seq.to_string(),
                    text: "é中🦀".repeat(2000),
                    parent: None,
                },
            ));
        }
        events.push(row(
            90,
            WbEvent::Text {
                id: "last".into(),
                text: "LATEST_STATE".into(),
                parent: None,
            },
        ));
        let seed = handoff(&events);
        assert!(seed.contains("Keep this original task") && seed.contains("LATEST_STATE"));
        assert!(seed.len() < 36 * 1024);
    }

    #[test]
    fn only_claude_code_and_codex_switch() {
        assert!(can_switch(SessionAgent::Claude, SessionAgent::Codex));
        assert!(can_switch(SessionAgent::Codex, SessionAgent::Claude));
        // Antigravity's handoff into agy, and Nexus's agent, are not built: no switch to or from them.
        for other in [SessionAgent::Antigravity, SessionAgent::Nexus] {
            assert!(!can_switch(SessionAgent::Claude, other));
            assert!(!can_switch(other, SessionAgent::Codex));
        }
        assert!(!can_switch(SessionAgent::Antigravity, SessionAgent::Nexus));
    }
}
