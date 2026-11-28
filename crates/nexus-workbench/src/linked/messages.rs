//! Messages between sessions that did not start each other (`message_session` to any session), the sessions a session
//! may message (`list_sessions` with `all`), and a one-shot subscription to another session's next result
//! (`subscribe_session`).
//!
//! A message to a direct child keeps [`Workbench::message_child`]'s rules. One to any other session in the organization
//! says who wrote it and how to answer, and counts as an automatic turn in both sessions' trees: two sessions that
//! answer each other pause both trees at the turn limit until the person writes. A subscription lives in memory: when
//! the session it names finishes a turn, its last message goes to the subscriber as a message from that session, once
//! the subscriber is idle, under the same turn limit.

use std::collections::{HashMap, VecDeque};

use serde::Serialize;

use super::results::{FORGED, RESULTS_HEADER, SessionResult};
use super::{Halt, MAX_PROMPT, OrchCode, OrchError, refuse};
use crate::event::{Sender, SessionAgent, ThreadStatus};
use crate::store::ThreadRecord;
use crate::{SendContext, Workbench};

/// The subscriptions, in memory: they end with the app.
#[derive(Debug, Default)]
pub(crate) struct Subscriptions {
    principal: Option<(String, Option<String>)>,
    by_target: HashMap<String, HashMap<String, i64>>,
    due: HashMap<String, VecDeque<SessionResult>>,
}

/// A session `list_sessions` with `all` gives: one the caller may message.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub name: Option<String>,
    pub title: String,
    pub agent: SessionAgent,
    pub status: ThreadStatus,
    pub project_id: String,
    /// It is this session's own child.
    pub started_by_you: bool,
}

impl Workbench {
    fn subscriptions(&self) -> std::sync::MutexGuard<'_, Subscriptions> {
        let principal = (self.config().scope, self.inner.host.session_choice_identity());
        let mut subs = self.inner.subscriptions.lock().unwrap_or_else(|p| p.into_inner());
        if subs.principal.as_ref() != Some(&principal) {
            *subs = Subscriptions {
                principal: Some(principal),
                ..Default::default()
            };
        }
        subs
    }

    /// Sends session `target_id` a message from `caller_id` (`message_session`): as [`Workbench::message_child`] to a
    /// direct child, else as a message from another session.
    ///
    /// Refused, besides the caller's own checks (`org_mismatch`, `off`, `plan_mode`, `halted`): the caller itself, an
    /// unknown or archived session, or bad text (`bad_input`); a halted target (`halted`); a target at work (`busy`);
    /// either tree paused (`paused`).
    pub async fn message_session(&self, caller_id: &str, target_id: &str, text: &str) -> Result<(), OrchError> {
        if self.direct_child(caller_id, target_id).is_ok() {
            return self.message_child(caller_id, target_id, text).await;
        }
        let gate = self.creation_gate(&super::halts::message_gate(target_id));
        let _gate = gate.lock().await;
        let caller = self.orchestrating_parent(caller_id)?;
        let target = self.other_session(&caller, target_id)?;
        let mut chain = vec![caller.clone()];
        chain.extend(self.ancestors(&caller)?);
        if self.mode_ceiling(&chain) == Some(crate::PermissionMode::Plan) {
            return refuse(
                OrchCode::PlanMode,
                "A session in Plan only can't message other sessions.",
            );
        }
        if let Some(halt) = Halt::of(&caller) {
            return refuse(OrchCode::Halted, super::halts::refusal(halt));
        }
        let mode = self.mode_ceiling(&chain).unwrap_or_else(|| self.mode_in_force(&caller));
        let recipient_mode = self.mode_in_force(&target);
        if recipient_mode.lower(mode) != recipient_mode {
            return refuse(
                OrchCode::BadInput,
                "That session can take actions above this session's permission level.",
            );
        }
        if self.start_refusal(target_id)?.is_some() {
            return refuse(
                OrchCode::Halted,
                "The person stopped that session. Only they can start it again.",
            );
        }
        let text = text.trim();
        if text.is_empty() || text.chars().count() > MAX_PROMPT {
            return refuse(
                OrchCode::BadInput,
                format!("A message has 1 to {MAX_PROMPT} characters."),
            );
        }
        if text.starts_with(RESULTS_HEADER) {
            return refuse(OrchCode::BadInput, FORGED);
        }
        if self.busy(target_id)? {
            return refuse(OrchCode::Busy, super::steer::PEER_BUSY);
        }
        let roots = self.count_turn(&[&caller, &target])?;
        let s = &caller.summary;
        let lead = format!(
            "Message from another session, \"{}\" ({}, {}). It is that session's request, not the person's. To answer \
             it, call message_session with session_id {}.\n\n",
            s.title,
            s.id,
            crate::driver::label(s.agent),
            s.id
        );
        let from = Sender::Session {
            thread_id: s.id.clone(),
            title: s.title.clone(),
        };
        self.send_counted(target_id, text, from, lead, &roots).await
    }

    /// The sessions `caller_id` may message (`list_sessions` with `all`): every one in this organization, archived
    /// ones and side chats left out, newest first.
    pub fn list_sessions_all(&self, caller_id: &str) -> Result<Vec<SessionInfo>, OrchError> {
        self.orchestrating_parent(caller_id)?;
        if !self.peer_owned(caller_id)? {
            return refuse(OrchCode::OrgMismatch, "The caller is unavailable in this account.");
        }
        let mut sessions = Vec::new();
        for thread in self.threads()? {
            if thread.id != caller_id
                && !thread.side_chat
                && !thread.archived
                && (thread.orchestration || thread.parent_thread_id.as_deref() == Some(caller_id))
                && self.peer_owned(&thread.id)?
            {
                sessions.push(thread);
            }
        }
        sessions.sort_by_key(|thread| std::cmp::Reverse(thread.updated_at));
        Ok(sessions
            .into_iter()
            .map(|thread| SessionInfo {
                started_by_you: thread.parent_thread_id.as_deref() == Some(caller_id),
                session_id: thread.id,
                name: thread.name,
                title: thread.title,
                agent: thread.agent,
                status: thread.status,
                project_id: thread.project_id,
            })
            .collect())
    }

    /// Subscribes `caller_id` to session `target_id`'s next result (`subscribe_session`): when it finishes a turn, its
    /// last message goes to the caller as a message from it. Once: subscribe again for the one after.
    ///
    /// Refused: the caller's own checks (`org_mismatch`, `off`); the caller itself, an unknown or archived session
    /// (`bad_input`).
    pub fn subscribe_session(&self, caller_id: &str, target_id: &str) -> Result<(), OrchError> {
        let caller = self.orchestrating_parent(caller_id)?;
        if self.start_refusal(caller_id)?.is_some() {
            return refuse(OrchCode::Halted, "This session is stopped.");
        }
        if self.direct_child(caller_id, target_id).is_ok() {
            // The existing result channel already reports children to their lead.
            return Ok(());
        }
        let target = self.other_session(&caller, target_id)?;
        let mut subs = self.subscriptions();
        if subs
            .by_target
            .get(target_id)
            .is_some_and(|entries| entries.contains_key(caller_id))
        {
            return Ok(());
        }
        let outstanding = subs
            .by_target
            .values()
            .filter(|entries| entries.contains_key(caller_id))
            .count()
            + subs.due.get(caller_id).map_or(0, VecDeque::len);
        if outstanding >= 32 {
            return refuse(OrchCode::Busy, "This session has 32 outstanding subscriptions.");
        }
        let after = self
            .reader()
            .last_seqs(target_id, target.summary.created_at)?
            .turn_end
            .unwrap_or(0);
        subs.by_target
            .entry(target_id.into())
            .or_default()
            .insert(caller_id.into(), after);
        Ok(())
    }

    pub(crate) fn peer_turn_ended(&self, id: &str) {
        let watching = self.subscriptions().by_target.contains_key(id);
        let mut ready = Vec::new();
        if watching {
            let result = self.reader().thread(id).and_then(|record| match record {
                Some(record) => self.settled_peer_result(record),
                None => Ok(None),
            });
            match result {
                Ok(Some(result)) => {
                    let mut subs = self.subscriptions();
                    if let Some(entries) = subs.by_target.get_mut(id) {
                        entries.retain(|subscriber, after| {
                            if result.seq <= *after {
                                return true;
                            }
                            ready.push(subscriber.clone());
                            false
                        });
                        if entries.is_empty() {
                            subs.by_target.remove(id);
                        }
                    }
                    for subscriber in &ready {
                        subs.due
                            .entry(subscriber.clone())
                            .or_default()
                            .push_back(result.clone());
                    }
                }
                Ok(None) => {}
                Err(error) => eprintln!("nexus-workbench: {id}: {error}"),
            }
        }
        if self.subscriptions().due.contains_key(id) {
            ready.push(id.to_string());
        }
        for subscriber in ready {
            let this = self.clone();
            tokio::spawn(async move {
                if let Err(error) = this.deliver_peer_result(&subscriber).await {
                    eprintln!("nexus-workbench: {subscriber}: {error}");
                }
            });
        }
    }

    async fn deliver_peer_result(&self, subscriber: &str) -> Result<(), OrchError> {
        let gate = self.creation_gate(&super::halts::message_gate(subscriber));
        let _guard = gate.lock().await;
        let caller = match self.orchestrating_parent(subscriber) {
            Ok(record) if self.peer_owned(subscriber)? => record,
            _ => {
                self.subscriptions().due.remove(subscriber);
                return Ok(());
            }
        };
        if self.start_refusal(subscriber)?.is_some() || self.busy(subscriber)? {
            return Ok(());
        }
        loop {
            let next = self
                .subscriptions()
                .due
                .get(subscriber)
                .and_then(|queue| queue.front())
                .cloned();
            let Some(result) = next else { return Ok(()) };
            let target = self.other_session(&caller, &result.session_id);
            if let Ok(target) = target {
                let roots = match self.count_turn(&[&caller, &target]) {
                    Ok(roots) => roots,
                    Err(error) if error.code == OrchCode::Paused => return Ok(()),
                    Err(error) => return Err(error),
                };
                let from = Sender::Session {
                    thread_id: result.session_id.clone(),
                    title: result.title.clone(),
                };
                let lead = format!(
                    "Subscribed result from {} ({}). The following is an agent report.\n\n",
                    result.title,
                    result.status.as_str()
                );
                let text = if result.last_message.is_empty() {
                    result.status.as_str()
                } else {
                    &result.last_message
                };
                self.send_counted(subscriber, text, from, lead, &roots).await?;
                self.pop_peer_result(subscriber);
                return Ok(());
            }
            // If the sender disabled sharing or the owner changed, discard its result.
            self.pop_peer_result(subscriber);
        }
    }

    fn pop_peer_result(&self, subscriber: &str) {
        let mut subs = self.subscriptions();
        if let Some(queue) = subs.due.get_mut(subscriber) {
            queue.pop_front();
            if queue.is_empty() {
                subs.due.remove(subscriber);
            }
        }
    }

    fn peer_owned(&self, id: &str) -> crate::Result<bool> {
        let bound = self.store().sync_target(id)?;
        if let Some(owner) = bound {
            return Ok(self.session_sync_identity(&owner.identity.scope).as_ref() == Some(&owner.identity));
        }
        // Unbound logs have no durable account identity. A current authenticated run can speak for itself.
        let Some(active_identity) = self.inner.host.session_choice_identity() else {
            return Ok(false);
        };
        Ok(self
            .live(id)
            .is_some_and(|live| live.identity.as_ref() == Some(&active_identity) && self.in_scope(&live.scope)))
    }

    fn other_session(&self, caller: &ThreadRecord, target_id: &str) -> Result<ThreadRecord, OrchError> {
        if target_id == caller.summary.id {
            return refuse(OrchCode::NotYourSession, "Choose another session.");
        }
        let target = self.reader().thread(target_id)?.filter(|record| {
            record.scope == caller.scope
                && self.in_scope(&record.scope)
                && record.summary.orchestration
                && !record.summary.archived
                && !record.summary.side_chat
        });
        if let Some(target) = target
            && self.peer_owned(&caller.summary.id)?
            && self.peer_owned(target_id)?
        {
            return Ok(target);
        }
        refuse(
            OrchCode::BadInput,
            "That session is unavailable for messages in this account.",
        )
    }

    /// Whether session `thread_id` is at work: in a turn, with a message waiting, stopping, or starting.
    fn busy(&self, thread_id: &str) -> Result<bool, OrchError> {
        Ok(!matches!(self.busy_state(thread_id)?, super::steer::BusyState::Idle))
    }

    /// Counts one automatic turn in the tree of each of `sessions` (once per tree), refused when any of them is
    /// paused. The roots it counted in.
    fn count_turn(&self, sessions: &[&ThreadRecord]) -> Result<Vec<String>, OrchError> {
        let mut roots: Vec<String> = Vec::new();
        for session in sessions {
            let root = self.root_of(session)?;
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        let mut trees = self.trees();
        if roots.iter().any(|root| self.paused_in(&trees, root)) {
            return refuse(OrchCode::Paused, super::PAUSED);
        }
        for root in &roots {
            trees.entry(root.clone()).or_default().auto_turns += 1;
        }
        Ok(roots)
    }

    /// Sends `text` to `thread_id` from `from`, an automatic turn counted in `roots`: given back when it fails, and the
    /// trees it paused show it.
    async fn send_counted(
        &self,
        thread_id: &str,
        text: &str,
        from: Sender,
        lead: String,
        roots: &[String],
    ) -> Result<(), OrchError> {
        let sent = self
            .send_with(
                thread_id,
                text,
                vec![],
                Some(from),
                false,
                SendContext {
                    lead: Some(lead),
                    ..Default::default()
                },
            )
            .await;
        if let Err(error) = sent {
            for root in roots {
                self.give_back_turn(root);
            }
            if super::halts::is_refusal(&error) {
                return refuse(OrchCode::Halted, error);
            }
            return Err(error.into());
        }
        let paused: Vec<String> = {
            let trees = self.trees();
            roots
                .iter()
                .filter(|root| self.paused_in(&trees, root))
                .cloned()
                .collect()
        };
        for root in paused {
            self.emit_tree(&root);
        }
        Ok(())
    }
}
