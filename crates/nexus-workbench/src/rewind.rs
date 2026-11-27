//! Rewind the conversation before a person's message without changing files.
//! Exact turn boundaries keep the provider's native context. Other boundaries start from the retained transcript.
//! This also works before the first turn, between messages in one turn, and without a repository.

use crate::event::{SessionAgent, StoredEvent, ThreadStatus, WbEvent};
use crate::store::ThreadRecord;
use crate::{
    Emit, Result, Reverting, Rewind, StopReason, Workbench, conversation, copied_until, now_ms, reverted_spans,
};

impl Workbench {
    /// A refused native rewind must never send another prompt into the discarded conversation.
    pub(crate) fn restart_rewound_conversation(
        &self,
        record: &mut ThreadRecord,
        events: &[StoredEvent],
        sending: i64,
    ) -> Result<Option<String>> {
        self.restart_conversation(record, events, sending, REWOUND)
    }

    /// Starts a new native conversation for the message numbered `sending`, told first what was said before it, after
    /// `lead` (why it starts again).
    pub(crate) fn restart_conversation(
        &self,
        record: &mut ThreadRecord,
        events: &[StoredEvent],
        sending: i64,
        lead: &str,
    ) -> Result<Option<String>> {
        record.native_id = uuid::Uuid::new_v4().to_string();
        record.native_started = false;
        record.fork_from = None;
        record.seed = transcript(&copied_until(events, sending - 1), lead);
        record.summary.context_tokens = None;
        record.summary.context_window = None;
        record.summary.updated_at = now_ms();
        // An empty cut records the native reset without dimming the replacement prompt already logged by send.
        let before = events.last().map_or(1, |event| event.seq + 1);
        let event = self
            .store()
            .save_thread_with_event(record, &WbEvent::Rewind { before, cursor: None })?;
        if self.emits_scope(&record.scope) {
            self.inner.host.emit(Emit::Event(event));
        }
        Ok(record.seed.clone())
    }

    pub async fn rewind_message(&self, thread_id: &str, seq: i64) -> Result<()> {
        // Hold through validation, stopping the idle process, and the atomic write. Sends cannot race the cut.
        let _reverting = Reverting::hold(self, thread_id)?;
        self.can_rewind_message(thread_id, seq)?;
        if let Some(live) = self.live(thread_id) {
            self.stop_live(&live, StopReason::Stop, None);
        }
        self.settle(thread_id).await?;
        self.checkpoint_written(thread_id).await;
        self.can_rewind_message(thread_id, seq)?;

        let store = self.store();
        let mut record = store.thread(thread_id)?.ok_or("This session no longer exists.")?;
        let all = store.events_after(thread_id, 0)?;
        let cursor = record
            .native_started
            .then(|| native_cursor(&all, seq, record.summary.agent))
            .flatten();
        if cursor.is_none() {
            record.native_id = uuid::Uuid::new_v4().to_string();
            record.native_started = false;
            record.fork_from = None;
            record.seed = transcript(&copied_until(&all, seq - 1), REWOUND);
        }
        record.summary.status = ThreadStatus::Idle;
        record.summary.context_tokens = None;
        record.summary.context_window = None;
        record.summary.updated_at = now_ms();
        let event = store.save_thread_with_event(&record, &WbEvent::Rewind { before: seq, cursor })?;
        if self.emits_scope(&record.scope) {
            self.inner.host.emit(Emit::Event(event));
            self.inner.host.emit(Emit::Thread(Box::new(record.summary)));
        }
        drop(store);
        if let Err(error) = self.reset_cursors(thread_id) {
            eprintln!("nexus-workbench: {thread_id}: {error}");
        }
        Ok(())
    }

    fn can_rewind_message(&self, thread_id: &str, seq: i64) -> Result<()> {
        let record = self.record(thread_id)?;
        if !self.emits_scope(&record.scope) {
            return Err("That session is not in this organization.".into());
        }
        let live_busy = self.live(thread_id).is_some_and(|live| {
            let state = live.state();
            !state.turns.is_empty() || !state.background.is_empty()
        });
        if record.summary.status.busy() || live_busy {
            return Err("Stop the agent and its background work before rewinding this conversation.".into());
        }
        self.refuse_revert_with_crew(thread_id)?;
        let events = self.reader().events_after(thread_id, 0)?;
        let undone = reverted_spans(&events);
        if !events.iter().any(|stored| {
            stored.seq == seq
                && matches!(stored.event, WbEvent::User { from: None, .. })
                && !undone.iter().any(|(after, end)| *after < seq && seq < *end)
        }) {
            return Err("This message is no longer part of the conversation.".into());
        }
        Ok(())
    }
}

/// Native APIs rewind whole turns. Use them only when that cannot remove a message before the selected one.
fn native_cursor(events: &[StoredEvent], before: i64, agent: SessionAgent) -> Option<String> {
    let spans = reverted_spans(events);
    let kept = |stored: &&StoredEvent| {
        !spans
            .iter()
            .any(|(after, end)| *after < stored.seq && stored.seq < *end)
    };
    let prior: Vec<_> = events
        .iter()
        .filter(|stored| stored.seq < before)
        .filter(kept)
        .collect();
    let turn = prior
        .iter()
        .rfind(|stored| matches!(stored.event, WbEvent::TurnEnd { .. }))?
        .seq;
    if prior.iter().any(|stored| {
        stored.seq > turn
            && matches!(
                stored.event,
                WbEvent::User { .. } | WbEvent::TurnStart { .. } | WbEvent::Text { .. } | WbEvent::Tool { .. }
            )
    }) {
        return None;
    }
    let mut cut = events.to_vec();
    cut.push(StoredEvent {
        thread_id: String::new(),
        seq: events.last()?.seq + 1,
        at: 0,
        event: WbEvent::Revert { turn, files: 0 },
    });
    match conversation(&cut, agent).1 {
        Some(Rewind::At(cursor)) => Some(cursor),
        _ => None,
    }
}

/// Why a rewound conversation starts again: the files stay as they are now.
const REWOUND: &str = "The person rewound this conversation. Continue from the saved transcript below. \
     Files have not been restored; inspect them before relying on earlier file state.";

/// Keep the full saved text, including queue edits, rather than silently clipping an earlier part of the conversation.
/// `lead` says why the conversation starts again.
fn transcript(events: &[StoredEvent], lead: &str) -> Option<String> {
    let mut edits = std::collections::HashMap::new();
    let mut cancelled = std::collections::HashSet::new();
    for event in events {
        match &event.event {
            WbEvent::QueueEdit { seq, text } => {
                edits.insert(*seq, text);
            }
            WbEvent::QueueCancel { seq } => {
                cancelled.insert(*seq);
            }
            _ => {}
        }
    }
    let mut lines = Vec::new();
    for stored in events {
        match &stored.event {
            WbEvent::User { text, from, .. } => {
                if cancelled.contains(&stored.seq) {
                    continue;
                }
                let text = edits.get(&stored.seq).copied().unwrap_or(text);
                let role = if from.is_some() { "Linked session" } else { "User" };
                lines.push(format!("{role}: {text}"));
            }
            WbEvent::Text { text, parent: None, .. } => lines.push(format!("Assistant: {text}")),
            _ => {}
        }
    }
    (!lines.is_empty()).then(|| {
        format!(
            "{lead} Earlier image attachments and tool outputs are not included in this text transcript.\n\n\
         <conversation>\n{}\n</conversation>\n\nThe new message follows.\n\n",
            lines
                .join("\n")
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        )
    })
}
