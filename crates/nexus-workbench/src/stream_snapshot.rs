//! Source-authorized full partial text, read together with its durable completion boundary.
use crate::event::{StoredEvent, TaskStatus, ThreadSummary, WbEvent};
use crate::{Live, Result, Workbench};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub parent: Option<String>,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub thread_id: String,
    pub summary: ThreadSummary,
    pub incarnation: Option<String>,
    pub revision: u64,
    pub through_seq: i64,
    pub events: Vec<StoredEvent>,
    pub more: bool,
    pub messages: Vec<Message>,
}
/// Independent of LiveState: store -> live registry -> partial is the only nested lock order.
/// In particular, reading a log must not erase logical text, unlike sent-suffix bookkeeping.
pub(crate) struct Partial {
    pub(crate) incarnation: String,
    revision: u64,
    messages: Vec<Message>,
    finalized: HashSet<(String, Option<String>)>,
    stopped: bool,
    overflow: bool,
}
impl Default for Partial {
    fn default() -> Self {
        Self {
            incarnation: uuid::Uuid::new_v4().to_string(),
            revision: 0,
            messages: vec![],
            finalized: HashSet::new(),
            stopped: false,
            overflow: false,
        }
    }
}
impl Partial {
    pub(crate) fn delta(&mut self, id: &str, parent: Option<&str>, text: &str) {
        if self.overflow || self.stopped || self.finalized.contains(&(id.into(), parent.map(str::to_string))) {
            return;
        }
        // An oversized partial waits for the durable Text rather than growing a second unbounded copy.
        let other: usize = self
            .messages
            .iter()
            .filter(|m| m.id != id || m.parent.as_deref() != parent)
            .map(|m| m.text.len())
            .sum();
        if other.saturating_add(text.len()) > 1024 * 1024
            || (self.messages.len() >= 64
                && !self
                    .messages
                    .iter()
                    .any(|m| m.id == id && m.parent.as_deref() == parent))
        {
            self.messages.retain(|m| m.id != id || m.parent.as_deref() != parent);
            self.finalized.insert((id.into(), parent.map(str::to_string)));
            if self.finalized.len() > 4096 {
                self.finalized.clear();
                self.messages.clear();
                self.overflow = true;
            }
            self.revision += 1;
            return;
        }
        if let Some(message) = self
            .messages
            .iter_mut()
            .find(|m| m.id == id && m.parent.as_deref() == parent)
        {
            message.text = text.into();
        } else {
            self.messages.push(Message {
                id: id.into(),
                parent: parent.map(str::to_string),
                text: text.into(),
            });
        }
        self.revision += 1;
    }
    pub(crate) fn event(&mut self, event: &WbEvent) {
        if self.overflow {
            return;
        }
        if matches!(event, WbEvent::TurnStart { .. }) {
            self.stopped = false;
        }

        let mut removed = vec![];
        self.messages.retain(|m| {
            let remove = match event {
                WbEvent::Text { id, parent, .. } => m.id == *id && m.parent == *parent,
                WbEvent::TurnEnd { .. } => m.parent.is_none(),
                WbEvent::Task {
                    tool_use_id: Some(call),
                    status,
                    ..
                } if *status != TaskStatus::Running => m.parent.as_deref() == Some(call),
                WbEvent::Session { .. } | WbEvent::Error { .. } => true,
                _ => false,
            };
            if remove {
                removed.push((m.id.clone(), m.parent.clone()));
            }
            !remove
        });
        // Even an unseen final message refuses an out-of-order delta with the same identity.
        if let WbEvent::Text { id, parent, .. } = event {
            removed.push((id.clone(), parent.clone()));
        }
        self.finalized.extend(removed);
        if self.finalized.len() > 4096 {
            self.finalized.clear();
            self.messages.clear();
            self.overflow = true;
        }

        self.revision += 1;
    }
    pub(crate) fn stop(&mut self) {
        self.messages.clear();
        self.stopped = true;
        self.revision += 1;
    }
}
impl Workbench {
    /// A coherent durable/partial cut. No launch/account identity is serialized.
    pub fn thread_stream(&self, thread_id: &str, after_seq: i64) -> Result<Snapshot> {
        if after_seq < 0 {
            return Err("Invalid stream event cursor.".into());
        }
        let store = self.store();
        let record = store.thread(thread_id)?.ok_or("No such session.")?;
        let account_free = crate::session_scope::SessionScope::decode(&record.scope).is_local()
            && self.inner.host.shows_account_free_sessions();
        let identity = if account_free {
            None
        } else {
            Some(
                self.inner
                    .host
                    .session_choice_identity()
                    .ok_or("Sign in to read live text.")?,
            )
        };
        if !self.emits_scope(&record.scope) {
            return Err("That session is not in this organization.".into());
        }
        // Retain the registry lock across the cut: old process A cannot be substituted for new B.
        let lives = self.inner.live.lock().unwrap_or_else(|p| p.into_inner());
        let live = lives.get(thread_id);
        if live.is_some_and(|live| live.scope != record.scope || (!account_free && live.identity != identity)) {
            return Err("The running session belongs to another sign-in.".into());
        }
        let mut events = store.stream_events_after(thread_id, after_seq)?;
        let more = events.len() > 256;
        events.truncate(256);
        let through_seq = store.last_event_seq(thread_id)?;
        if after_seq > through_seq {
            return Err("The stream event cursor is ahead of this store.".into());
        }
        let (incarnation, revision, messages) = if let Some(live) = live {
            let partial = live.partial.lock().unwrap_or_else(|p| p.into_inner());
            (
                Some(partial.incarnation.clone()),
                partial.revision,
                if more { vec![] } else { partial.messages.clone() },
            )
        } else {
            (None, 0, vec![])
        };
        if !self.emits_scope(&record.scope) || (!account_free && self.inner.host.session_choice_identity() != identity)
        {
            return Err("The session's sign-in changed during the read.".into());
        }
        Ok(Snapshot {
            thread_id: thread_id.into(),
            summary: record.summary,
            incarnation,
            revision,
            through_seq,
            events,
            more,
            messages,
        })
    }
    /// Called before throttling/suffix conversion, and only for the exact registered producer.
    pub(crate) fn partial_delta(&self, thread: &str, live: &Live, id: &str, parent: Option<&str>, text: &str) {
        let _store = self.store();
        let lives = self.inner.live.lock().unwrap_or_else(|p| p.into_inner());
        if lives
            .get(thread)
            .is_some_and(|current| std::ptr::eq(current.as_ref(), live))
        {
            live.partial
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .delta(id, parent, text);
        }
    }
}

#[cfg(test)]
mod tests;
