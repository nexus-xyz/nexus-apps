//! Live chat state outlives navigation. Only explicit Stop or a sign-in change cancels transport work.
use super::{Entry, Workspace};
use crate::attachments::Draft;
use crate::gateway::{ChatChoices, Event};
use crate::model::{ModelChoice, Role, SessionSummary};
use gpui_kit::*;
use nexus_workbench::chat_queue::{Item, Outcome, Queues};

pub(super) struct DetachedChat {
    turn: u64,
    draft: Option<super::drafts::DetachedDraft>,
    id: Option<String>,
    title: SharedString,
    entries: Vec<Entry>,
    model: Option<ModelChoice>,
    choices: ChatChoices,
    archived: bool,
    running: bool,
    queue_key: String,
    sent: Option<(u64, Item<Draft>)>,
    edit: Option<super::chat_rewind::EditSent>,
}

pub(super) fn event_turn(event: &Event) -> Option<u64> {
    match event {
        Event::Created { turn, .. }
        | Event::Asked { turn, .. }
        | Event::Delta { turn, .. }
        | Event::WritingPage { turn }
        | Event::SearchingWeb { turn }
        | Event::Answered { turn, .. }
        | Event::AnswerFailed { turn, .. } => Some(*turn),
        _ => None,
    }
}

/// One reducer for the answer on screen and all detached answers.
pub(super) fn update_entries(entries: &mut [Entry], event: &Event, cx: &mut App) {
    match event {
        Event::Asked { question, .. } => {
            if let Some(entry) = entries.iter_mut().rev().nth(1).filter(|entry| entry.role == Role::User) {
                entry.stored = question.clone();
            }
        }
        Event::Delta { text, .. } => {
            if let Some(entry) = entries.last_mut() {
                entry.searching_web = false;
                entry.streamed.push_str(text);
                if let Some(answer) = &entry.answer {
                    answer.update(cx, |state, cx| state.push_str(text, cx));
                }
            }
        }
        Event::WritingPage { .. } => {
            if let Some(entry) = entries.last_mut() {
                entry.writing_page = true;
            }
        }
        Event::SearchingWeb { .. } => {
            if let Some(entry) = entries.last_mut() {
                entry.searching_web = true;
            }
        }
        Event::Answered {
            content, page, sources, ..
        } => {
            if let Some(entry) = entries.last_mut() {
                entry.streaming = false;
                entry.writing_page = false;
                entry.searching_web = false;
                entry.page = page.clone();
                entry.web_sources = sources.clone();
                if entry.streamed != *content {
                    if let Some(answer) = &entry.answer {
                        answer.update(cx, |state, cx| state.set_text(content, cx));
                    }
                    entry.streamed = content.clone();
                }
            }
        }
        Event::AnswerFailed { error, .. } => {
            if let Some(entry) = entries.last_mut() {
                entry.streaming = false;
                entry.writing_page = false;
                entry.searching_web = false;
                entry.error = Some(error.clone().into());
            }
        }
        _ => {}
    }
}

/// Settle the same in-flight receipt whether its chat is visible or detached.
pub(super) fn settle_receipt<P>(
    queues: &mut Queues<P>,
    sent: &mut Option<(u64, Item<P>)>,
    key: &str,
    turn: u64,
    outcome: Outcome,
) -> Option<Item<P>> {
    let unsent = sent
        .take_if(|(asked, _)| *asked == turn)
        .filter(|_| outcome != Outcome::Complete)
        .map(|(_, item)| item);
    let next = queues.settle(key, outcome);
    if let Some(item) = unsent {
        queues.update(key, |queue| queue.put_back(item));
    }
    next
}

impl Workspace {
    /// An attachment read belongs to its original entry, even when a running chat leaves the screen, or a chat left
    /// keeps its entries (`kept_chats.rs`).
    pub(super) fn chat_entry_mut(&mut self, id: usize) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .chain(
                self.detached_chats
                    .values_mut()
                    .flat_map(|chat| chat.entries.iter_mut()),
            )
            .chain(self.kept_chats.entries_mut())
            .find(|entry| entry.id == id)
    }

    /// Move a running view into memory, keeping its request and queue alive.
    pub(super) fn detach_live_chat(&mut self) -> bool {
        if self.detach_gateway_chat() {
            return true;
        }
        let Some(turn) = self.asking else { return false };
        let choices = self.chat_choices();
        let queue_key = self.current.clone().unwrap_or_else(|| format!("pending-chat:{turn}"));
        if self.current.is_none() {
            self.chat_queue.queues.rename("", &queue_key);
        }
        self.chat_queue.editing = None;
        let run = DetachedChat {
            turn,
            draft: self.detach_draft(),
            id: self.current.clone(),
            title: self.title.clone(),
            entries: std::mem::take(&mut self.entries),
            model: self.model.clone(),
            choices,
            archived: self.chat_archived,
            running: true,
            queue_key,
            sent: self.chat_queue.sent.take(),
            edit: self.edit_sent.take(),
        };
        self.asking = None;
        self.detached_chats.insert(turn, run);
        true
    }

    /// Whether chat `id` is answering now: the open chat while it asks, or one left running in the background.
    pub(super) fn chat_answering(&self, id: &str) -> bool {
        if self.gateway_chat_answering(id) {
            return true;
        }
        (self.asking.is_some() && self.current.as_deref() == Some(id))
            || self
                .detached_chats
                .values()
                .any(|run| run.running && run.id.as_deref() == Some(id))
    }

    /// Deleting a chat is an explicit cancellation, unlike leaving its page.
    pub(super) fn cancel_chat(&mut self, id: &str) {
        if self.cancel_gateway_chat(id) {
            return;
        }
        if self.current.as_deref() == Some(id)
            && let Some(turn) = self.asking.take()
        {
            if let Some(account) = self.account_gateway() {
                account.stop(turn);
            }
        }
        let turns: Vec<_> = self
            .detached_chats
            .iter()
            .filter(|(_, run)| run.id.as_deref() == Some(id))
            .map(|(turn, _)| *turn)
            .collect();
        for turn in turns {
            if let Some(account) = self.account_gateway() {
                account.stop(turn);
            }
            self.detached_chats.remove(&turn);
        }
        self.chat_queue.queues.clear(id);
    }

    /// Cancel every running answer (an organization switch or a sign-out). The open chat stays on screen, so its
    /// answer ends as Stop leaves it: what streamed stays, marked as stopped, and its Thinking animation ends.
    pub(super) fn cancel_live_chats(&mut self) {
        self.revoke_gateway_chat();
        if let Some(turn) = self.asking.take() {
            if let Some(account) = self.account_gateway() {
                account.stop(turn);
            }
            if let Some(entry) = self.entries.last_mut() {
                entry.streaming = false;
                entry.stopped = true;
            }
        }
        let account = self.account_gateway();
        for (_, run) in self.detached_chats.drain() {
            if run.running
                && let Some(account) = &account
            {
                account.stop(run.turn);
            }
        }
        self.chat_queue.queues = Default::default();
        self.chat_queue.sent = None;
    }

    pub(super) fn handle_detached_chat(&mut self, event: &Event, cx: &mut Context<Self>) -> bool {
        let Some(account) = self.account_gateway() else {
            return false;
        };
        let Some(turn) = event_turn(event) else { return false };
        let Some(mut run) = self.detached_chats.remove(&turn) else {
            return false;
        };
        update_entries(&mut run.entries, event, cx);
        match event {
            Event::Created { id, title, .. } => {
                self.promote_detached_draft(run.draft.take(), id, cx);
                self.chat_queue.queues.rename(&run.queue_key, id);
                run.queue_key = id.clone();
                run.id = Some(id.clone());
                run.title = title.clone().into();
                if !self.sessions.iter().any(|session| session.id == *id) {
                    self.sessions.insert(
                        0,
                        SessionSummary {
                            id: id.clone(),
                            title: title.clone(),
                            day: Some(crate::model::today()),
                            at_ms: Some(crate::code::now_ms()),
                        },
                    );
                }
            }
            Event::Asked { .. } => {
                run.sent = None;
                run.edit = None;
            }
            Event::Answered { .. } | Event::AnswerFailed { .. } => {
                run.running = false;
                let outcome = if matches!(event, Event::Answered { .. }) {
                    Outcome::Complete
                } else {
                    Outcome::Failed
                };
                if outcome == Outcome::Complete {
                    run.edit = None;
                }
                let next = settle_receipt(
                    &mut self.chat_queue.queues,
                    &mut run.sent,
                    &run.queue_key,
                    turn,
                    outcome,
                );
                account.list_sessions();
                if let Some(item) = next {
                    self.turn += 1;
                    run.turn = self.turn;
                    run.running = true;
                    self.next_entry += 1;
                    run.entries
                        .push(Entry::user(self.next_entry, item.text.clone()).with_attachments(item.payload.clone()));
                    self.next_entry += 1;
                    run.entries.push(Entry::answer(self.next_entry, "", true, cx));
                    account.ask(
                        run.turn,
                        run.id.clone(),
                        item.text.clone(),
                        run.model.as_ref().map(|model| model.id.clone()),
                        item.payload.clone(),
                        run.choices.clone(),
                    );
                    run.sent = Some((run.turn, item));
                } else if outcome == Outcome::Complete {
                    // Gateway persisted it. Reopening reads its stored metadata instead of retaining a second transcript.
                    cx.notify();
                    return true;
                }
            }
            _ => {}
        }
        self.detached_chats.insert(run.turn, run);
        cx.notify();
        true
    }

    pub(super) fn resume_live_chat(&mut self, id: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.resume_gateway_chat(id, window, cx) {
            return true;
        }
        let turn = self
            .detached_chats
            .iter()
            .filter(|(_, run)| run.id.as_deref() == id)
            .map(|(turn, _)| *turn)
            .max();
        let Some(turn) = turn else { return false };
        let Some(run) = self.detached_chats.remove(&turn) else {
            return false;
        };
        if run.id.is_none() {
            self.chat_queue.queues.rename(&run.queue_key, "");
        }
        self.current = run.id;
        self.title = run.title;
        self.entries = run.entries;
        self.asking = run.running.then_some(turn);
        self.model = run.model;
        self.choices.team = Some(run.choices.team.unwrap_or_default());
        self.choices.web_search = Some(run.choices.web_search);
        self.choices.session_web_search = Some(run.choices.web_search);
        self.chat_archived = run.archived;
        self.chat_queue.sent = run.sent;
        self.edit_sent = run.edit;
        self.opening = false;
        if !run.running {
            self.edit_not_sent(turn, window, cx);
        }
        self.scroll.scroll_to_bottom();
        self.focus_composer(window, cx);
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    #[test]
    fn offscreen_completion_drains_only_its_queue_without_replaying_the_sent_message() {
        let mut queues = Queues::default();
        queues.enqueue("a", "first".into(), 1);
        let first = queues.update("a", |queue| queue.take()).expect("first queued item");
        let mut sent = Some((7, first));
        queues.enqueue("a", "follow-up".into(), 2);
        queues.enqueue("b", "other chat".into(), 3);
        let next =
            settle_receipt(&mut queues, &mut sent, "a", 7, Outcome::Complete).expect("follow-up sends offscreen");
        assert_eq!(next.text, "follow-up");
        assert!(sent.is_none());
        assert_eq!(
            queues.get("b").expect("other queue remains").items()[0].text,
            "other chat"
        );
    }
    #[test]
    fn an_unaccepted_hidden_send_returns_to_the_front_and_waits_for_resume() {
        let mut queues = Queues::default();
        queues.enqueue("a", "first".into(), 1);
        let first = queues.update("a", |queue| queue.take()).expect("first queued item");
        let mut sent = Some((8, first));
        queues.enqueue("a", "later".into(), 2);
        assert!(settle_receipt(&mut queues, &mut sent, "a", 8, Outcome::Failed).is_none());
        let queue = queues.get("a").expect("failed draft retained");
        assert!(queue.held());
        assert_eq!(
            queue.items().iter().map(|item| item.text.as_str()).collect::<Vec<_>>(),
            ["first", "later"]
        );
    }

    #[gpui_kit::test]
    fn detached_and_visible_answers_use_identical_stream_and_completion_rules(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut entries = vec![Entry::user(1, "question".into()), Entry::answer(2, "", true, cx)];
            update_entries(&mut entries, &Event::SearchingWeb { turn: 7 }, cx);
            assert!(entries[1].searching_web);
            update_entries(
                &mut entries,
                &Event::Delta {
                    turn: 7,
                    text: "partial".into(),
                },
                cx,
            );
            assert_eq!(entries[1].streamed, "partial");
            assert!(!entries[1].searching_web);
            update_entries(&mut entries, &Event::WritingPage { turn: 7 }, cx);
            update_entries(
                &mut entries,
                &Event::Answered {
                    turn: 7,
                    content: "stored final".into(),
                    page: None,
                    sources: Vec::new(),
                },
                cx,
            );
            assert_eq!(entries[1].streamed, "stored final");
            assert!(!entries[1].streaming);
            assert!(!entries[1].searching_web);
            assert!(!entries[1].writing_page);
        });
    }
    #[gpui_kit::test]
    fn a_failed_hidden_answer_keeps_partial_text_and_clears_activity(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut entries = vec![Entry::answer(2, "partial", true, cx)];
            update_entries(&mut entries, &Event::SearchingWeb { turn: 9 }, cx);
            update_entries(
                &mut entries,
                &Event::AnswerFailed {
                    turn: 9,
                    error: "offline".into(),
                },
                cx,
            );
            assert_eq!(entries[0].streamed, "partial");
            assert_eq!(entries[0].error.as_deref(), Some("offline"));
            assert!(!entries[0].streaming);
            assert!(!entries[0].searching_web);
        });
    }
}
