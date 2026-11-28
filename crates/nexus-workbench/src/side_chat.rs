//! Side chats: a question about a session that does not add to it. A side chat
//! is a fork of the session's conversation (`fork_thread`'s copy and fork point) in the session's own folder, with no
//! worktree of its own, shown beside the session and never in the sidebar. The session keeps running meanwhile.
//!
//! Because both work in one folder, a side chat:
//! - starts in the session's permission mode, and its agent is told it is a side chat;
//! - is titled from its first message until its agent names it ([`Workbench::title_side_chat`]);
//! - keeps no checkpoints, so it shows no per-turn changes and reverts no files: restoring the shared folder would
//!   undo the session's own work. T3 Code's side chats do the same;
//! - can't be forked, and has no side chats of its own.
//!
//! "Bring back" is a fork's "Send back to source" (`Workbench::send_back`, [`crate::send_back::side_chat_message`]):
//! queued behind the session's running turn, never interrupting it. Closing one the person never wrote in deletes it
//! ([`Workbench::close_side_chat`]); deleting the session deletes its side chats (`delete_thread`).

use crate::event::{StoredEvent, ThreadStatus, ThreadSummary, WbEvent};
use crate::store::ThreadRecord;
use crate::{
    Emit, Result, Workbench, conversation, copied_until, forks_at_end, mentions, now_ms, said_of, short_id, title_of,
};

/// What a core that opens side chats (`wb_side_chat`, `wb_side_chat_close`) says in its features. An app hides
/// `/btw`, `/side` and the side pane's "Side chat" for a machine without it.
pub const FEATURE: &str = "side-chats";

/// The title a side chat starts with, until its first message titles it.
pub const TITLE: &str = "Side chat";

/// What a side chat's agent is told ahead of its first message: what it is, and that the session may be changing the
/// same files.
const NOTE: &str = "(Note from the Nexus app: this is a side chat. The person is asking about the session it was \
opened from, which may still be working in this same folder. Answer what they ask. Do not change files or run commands \
that change them unless the person asks you to.)\n\n";

/// What a side chat's agent is told first: [`NOTE`], and when it can't go on from the session's own conversation (a
/// turn is running there), what was said up to the session's last finished turn.
pub(crate) fn seed(at_end: bool, events: &[StoredEvent]) -> String {
    if at_end {
        NOTE.to_string()
    } else {
        format!(
            "{NOTE}What was said in that session up to its last finished turn:\n\n{}\n\nTheir message follows.\n\n",
            said_of(events)
        )
    }
}

/// Whether the person wrote in the side chat: a message of theirs logged since it was made (`since`). The
/// conversation copied from its session is logged at its first times.
fn used(events: &[StoredEvent], since: i64) -> bool {
    events
        .iter()
        .any(|stored| stored.at >= since && matches!(stored.event, WbEvent::User { from: None, .. }))
}

impl Workbench {
    /// Opens a side chat on session `session_id`: a new session hidden from the sidebar that starts with what the
    /// session knows, in the session's folder. At the session's end (it is idle) its agent forks the session's own
    /// conversation, as a fork from the latest turn does; while a turn runs there, it starts a new conversation told
    /// what was said up to the last finished turn. Its first message is the person's question, sent separately.
    pub async fn side_chat(&self, session_id: &str) -> Result<ThreadSummary> {
        let (session, all) = {
            let store = self.store();
            let session = store
                .thread(session_id)?
                .ok_or_else(|| format!("No session {session_id}."))?;
            if !self.in_scope(&session.scope) {
                return Err("That session is not in this organization.".into());
            }
            (session, store.events_after(session_id, 0)?)
        };
        if session.summary.provider.is_some() {
            return Err("This build cannot start a side chat with a saved local provider.".into());
        }
        if session.summary.side_chat {
            return Err("A side chat has no side chats of its own. Open one on its session.".into());
        }
        if session
            .summary
            .worktree
            .as_ref()
            .is_some_and(|worktree| worktree.removed)
        {
            return Err("This session's worktree was removed, so a side chat has no folder to work in.".into());
        }
        let sync_identity = self.inherited_sync_identity(&session);
        // The session's last finished turn that was not reverted (0: none yet).
        let (kept, rewind) = conversation(&all, session.summary.agent);
        let at = kept.last().map_or(0, |(seq, _)| *seq);
        let events = copied_until(&all, at);
        let at_end = forks_at_end(&all, at, rewind.is_some(), session.native_started);

        let now = now_ms();
        let s = &session.summary;
        let record = ThreadRecord {
            summary: ThreadSummary {
                name: None,
                id: format!("t_{}", short_id()),
                project_id: s.project_id.clone(),
                title: TITLE.into(),
                agent: s.agent,
                model: s.model.clone(),
                // The session's own permissions: a side chat asks only what its session would ask.
                permission_mode: s.permission_mode,
                effort: s.effort,
                fast: false,
                via: s.via,
                provider: None,
                account: s.account.clone(),
                status: ThreadStatus::Idle,
                cost_usd: 0.0,
                nexus_sessions: vec![],
                context_tokens: None,
                context_window: None,
                created_at: now,
                updated_at: now,
                listed_at: Some(now),
                acted_at: Some(now),
                archived: false,
                pinned: false,
                // The session's folder: `worktree_of` reads it from the session, so the side chat never owns it.
                worktree: None,
                pull_requests: Vec::new(),
                pending: None,
                capabilities: None,
                parent_thread_id: None,
                source_thread_id: Some(s.id.clone()),
                side_chat: true,
                spawned_by: None,
                group_id: None,
                depth: 0,
                cross_project: false,
                orchestration: false,
                waits_on: vec![],
                pending_results: 0,
                start_choices: vec![],
                halt: None,
                crew_paused: false,
                crew_tripped: false,
                crew_spend_trip: None,
                in_background: false,
                background: vec![],
                background_total: 0,
                at_depth_limit: false,
                handoff_from: None,
                limited: None,
                aside: Default::default(),
            },
            native_id: uuid::Uuid::new_v4().to_string(),
            native_started: false,
            scope: session.scope.clone(),
            fork_from: at_end.then(|| session.native_id.clone()),
            seed: Some(seed(at_end, &events)),
            automatic_title: true,
            delivered_seq: 0,
            halt: None,
        };
        let turn_count = events
            .iter()
            .filter(|stored| matches!(stored.event, WbEvent::TurnEnd { .. }))
            .count();
        let notice = WbEvent::Notice {
            text: format!(
                "Side chat on \"{}\" after {turn_count} {}. It works in the session's folder, with the session's \
                 permissions.",
                s.title,
                if turn_count == 1 { "turn" } else { "turns" }
            ),
        };
        let copied = events.iter().map(|stored| (stored.at, &stored.event));
        let sync_identity =
            sync_identity.filter(|identity| self.session_sync_identity(&session.scope).as_ref() == Some(identity));
        self.store()
            .insert_thread_with_identity(&record, copied.chain([(now, &notice)]), sync_identity.as_ref())?;
        if self.in_scope(&record.scope) {
            self.inner.host.emit(Emit::Thread(Box::new(record.summary.clone())));
        }
        Ok(record.summary)
    }

    /// Closes side chat `side_id`: deleted when the person never wrote in it and its agent is not running, kept
    /// otherwise (it has Delete). True when it was deleted.
    pub async fn close_side_chat(&self, side_id: &str) -> Result<bool> {
        let record = self.record(side_id)?;
        if !self.in_scope(&record.scope) {
            return Err("That session is not in this organization.".into());
        }
        if !record.summary.side_chat {
            return Err("That session is not a side chat.".into());
        }
        let events = self.reader().events_after(side_id, 0)?;
        if used(&events, record.summary.created_at) || self.is_live(side_id) {
            return Ok(false);
        }
        self.delete_thread(side_id).await?;
        Ok(true)
    }

    /// Whether an agent of one of session `session_id`'s side chats is running: it works in the session's folder, so
    /// that folder's worktree stays (`remove_worktree`, the worktree cleanup).
    pub(crate) fn side_chat_live(&self, session_id: &str) -> bool {
        self.reader()
            .side_chats(session_id)
            .is_ok_and(|sides| sides.iter().any(|side| self.is_live(&side.summary.id)))
    }

    /// Titles side chat `record` from the person's first message `text`, as a new session is titled, the moment it is
    /// sent. Its agent's title replaces it later: in a forked conversation the agent often skips the title line,
    /// which left the side chat "Side chat" for good. A slash command or a renamed side chat keeps its title.
    pub(crate) fn title_side_chat(&self, record: &ThreadRecord, text: &str) -> Result<()> {
        let text = mentions::plain(text);
        if !record.summary.side_chat
            || !record.automatic_title
            || record.summary.title != TITLE
            || text.trim().is_empty()
            || text.trim_start().starts_with('/')
        {
            return Ok(());
        }
        let title = title_of(&text);
        self.change(&record.summary.id, |record| {
            if record.automatic_title && record.summary.title == TITLE {
                record.summary.title = title;
            }
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(seq: i64, at: i64, event: WbEvent) -> StoredEvent {
        StoredEvent {
            thread_id: "t_side".into(),
            seq,
            at,
            event,
        }
    }

    fn user(seq: i64, at: i64, text: &str) -> StoredEvent {
        stored(
            seq,
            at,
            WbEvent::User {
                text: text.into(),
                images: vec![],
                from: None,
            },
        )
    }

    #[test]
    fn only_its_own_messages_count() {
        // Copied from the session, at its own times.
        let copied = vec![user(1, 10, "fix it")];
        assert!(!used(&copied, 100));
        let mut own = copied.clone();
        own.push(user(2, 110, "why?"));
        assert!(used(&own, 100));
    }

    #[test]
    fn the_seed_tells_the_agent_it_is_a_side_chat() {
        assert_eq!(seed(true, &[]), NOTE);
        let mid_turn = seed(false, &[user(1, 10, "fix it")]);
        assert!(mid_turn.starts_with(NOTE), "{mid_turn}");
        assert!(
            mid_turn.contains("up to its last finished turn:\n\n<conversation>\nUser: fix it\n</conversation>"),
            "{mid_turn}"
        );
        assert!(mid_turn.ends_with("Their message follows.\n\n"), "{mid_turn}");
    }
}
