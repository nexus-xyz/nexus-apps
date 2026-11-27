//! A primary chat's execution side and plain model outlive its native presentation.
use super::{Workspace, drafts, live_chat};
use crate::{attachments::Draft, backend::Backend, gateway::Event, windows::retention::Identity};
use futures::StreamExt;
use gpui_kit::*;
use nexus_workbench::chat_queue::{Item, Outcome, Queues};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};
mod entries;
mod first_token;
mod lifecycle;
mod model;
mod projection;
pub(super) use entries::Projected;
use model::{Message, Run};

pub(super) struct State {
    backend: Arc<dyn Backend>,
    identity: Identity,
    revoked: bool,
    incarnation: u64,
    next_turn: u64,
    next_entry: usize,
    launched: HashSet<u64>,
    runs: HashMap<u64, Run>,
    queues: Queues<Draft>,
    rewrite: Option<(String, u64, String)>,
}
impl State {
    fn current(&mut self) -> bool {
        if !self.identity.matches_gateway(self.backend.as_ref()) {
            self.revoke();
        }
        !self.revoked
    }
    fn permits(&mut self, incarnation: u64) -> bool {
        self.current() && self.incarnation == incarnation
    }
    fn revoke(&mut self) {
        if self.revoked {
            return;
        }
        self.revoked = true;
        self.incarnation = self.incarnation.wrapping_add(1);
        for turn in self.launched.drain() {
            self.backend.stop(turn);
        }
        self.runs.clear();
        self.queues = Queues::default();
        self.rewrite = None;
    }
    fn turn(&mut self) -> u64 {
        self.next_turn = self.next_turn.checked_add(1).expect("chat turn counter exhausted");
        self.next_turn
    }
    fn launch(&mut self, run: &Run, text: String, payload: Draft) {
        self.launched.insert(run.turn);
        self.backend.ask(
            run.turn,
            run.id.clone(),
            text,
            run.model.as_ref().map(|model| model.id.clone()),
            payload,
            run.choices.clone(),
        );
    }
    fn next(&mut self, run: &mut Run, item: Item<Draft>) {
        run.turn = self.turn();
        run.running = true;
        run.accepted = false;
        let web_search = if run.panel || run.id.is_none() {
            run.choices.web_search
        } else {
            run.session_web_search
        };
        run.choices.patch_web_search = run.id.is_some() && run.session_web_search != web_search;
        run.choices.web_search = web_search;
        // Advance only for an actual dispatch; preference capture and projection keep the prior baseline.
        run.session_web_search = web_search;
        self.next_entry += 1;
        run.entries
            .push(Message::user(self.next_entry, item.text.clone(), item.payload.clone()));
        self.next_entry += 1;
        run.entries.push(Message::answer(self.next_entry));
        self.launch(run, item.text.clone(), item.payload.clone());
        run.sent = Some((run.turn, item));
    }
    fn settle(&mut self, run: &mut Run, outcome: Outcome) {
        run.end(outcome == Outcome::Stopped);
        if outcome != Outcome::Complete && !run.accepted {
            run.restore_edit = run.edit.take();
        }
        run.finished = Some((
            run.turn,
            outcome == Outcome::Complete,
            run.entries.last().and_then(|entry| entry.page.clone()),
        ));
        if let Some(item) =
            live_chat::settle_receipt(&mut self.queues, &mut run.sent, &run.queue_key, run.turn, outcome)
        {
            self.next(run, item);
        }
    }
    fn receive(&mut self, mut event: Event, cx: &mut App) {
        loop {
            match event {
                Event::ReadSnapshot {
                    epoch,
                    expected,
                    revision,
                    event: inner,
                } => {
                    use std::sync::atomic::Ordering;
                    if epoch.load(Ordering::SeqCst) != expected
                        || revision
                            .as_ref()
                            .is_some_and(|(revision, expected)| revision.load(Ordering::SeqCst) != *expected)
                    {
                        return;
                    }
                    event = *inner;
                }
                _ => break,
            }
        }
        if !self.current() {
            return;
        }
        match &event {
            Event::SignedIn | Event::SignedOut | Event::SignInFailed(_) => {
                self.revoke();
                return;
            }
            Event::Account(account) => {
                if Identity::current(self.backend.as_ref(), account).as_ref() != Some(&self.identity) {
                    self.revoke();
                }
                return;
            }
            _ => {}
        }
        let Some(turn) = live_chat::event_turn(&event) else {
            return;
        };
        let Some(key) = self
            .runs
            .iter()
            .find(|(_, run)| run.running && run.turn == turn)
            .map(|(key, _)| *key)
        else {
            return;
        };
        let Some(mut run) = self.runs.remove(&key) else {
            return;
        };
        if matches!(event, Event::Answered { .. } | Event::AnswerFailed { .. }) {
            self.launched.remove(&turn);
        }
        match event {
            Event::Created { id, title, .. } => {
                if run.id.is_none() {
                    drafts::promote_execution_draft(run.draft.take(), &id, &self.identity_org(), cx);
                    self.queues.rename(&run.queue_key, &id);
                    run.queue_key = id.clone();
                    run.id = Some(id);
                    run.title = title.into();
                }
            }
            Event::Asked { question, .. } => {
                if let Some(entry) = run
                    .entries
                    .iter_mut()
                    .rev()
                    .nth(1)
                    .filter(|entry| entry.role == crate::model::Role::User)
                {
                    entry.stored = question;
                    entry.touch();
                }
                run.sent = None;
                run.edit = None;
                run.accepted = true;
            }
            Event::Delta { text, .. } => {
                if let Some(entry) = run.entries.last_mut() {
                    entry.streamed.push_str(&text);
                    entry.searching_web = false;
                    entry.touch();
                }
            }
            Event::WritingPage { .. } => {
                if let Some(entry) = run.entries.last_mut() {
                    entry.writing_page = true;
                    entry.touch();
                }
            }
            Event::SearchingWeb { .. } => {
                if let Some(entry) = run.entries.last_mut() {
                    entry.searching_web = true;
                    entry.touch();
                }
            }
            Event::Answered {
                content, page, sources, ..
            } => {
                if let Some(entry) = run.entries.last_mut() {
                    entry.streamed = content;
                    entry.page = page;
                    entry.web_sources = sources;
                    entry.touch();
                }
                run.edit = None;
                self.settle(&mut run, Outcome::Complete);
            }
            Event::AnswerFailed { error, .. } => {
                if let Some(entry) = run.entries.last_mut() {
                    entry.error = Some(error.into());
                    entry.touch();
                }
                self.settle(&mut run, Outcome::Failed);
            }
            _ => {}
        }
        self.runs.insert(key, run);
    }
    fn identity_org(&self) -> String {
        self.identity.org().to_string()
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.revoke();
    }
}

/// A non-rendering entity. Its task captures only a weak entity; no Window or Workspace is retained.
pub(super) struct Owner {
    state: Rc<RefCell<State>>,
    events: Rc<RefCell<futures::channel::mpsc::UnboundedReceiver<Event>>>,
    _events: Task<()>,
}
impl Owner {
    fn new(
        backend: Arc<dyn Backend>,
        events: futures::channel::mpsc::UnboundedReceiver<Event>,
        identity: Identity,
        cx: &mut Context<Self>,
    ) -> Self {
        let events = Rc::new(RefCell::new(events));
        let state = Rc::new(RefCell::new(State {
            backend,
            identity,
            revoked: false,
            incarnation: 1,
            next_turn: 0,
            next_entry: 0,
            launched: HashSet::new(),
            runs: HashMap::new(),
            queues: Queues::default(),
            rewrite: None,
        }));
        let receiver = events.clone();
        let task = cx.spawn(async move |owner, cx| {
            // The receiver borrow ends after each poll, before any await can suspend.
            let next = || futures::future::poll_fn(|task| receiver.borrow_mut().poll_next_unpin(task));
            while let Some(event) = next().await {
                if owner
                    .update(cx, |owner, cx| {
                        owner.receive(event, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            state,
            events,
            _events: task,
        }
    }
    fn receive(&mut self, event: Event, cx: &mut Context<Self>) {
        self.state.borrow_mut().receive(event, cx);
        cx.notify();
    }
    /// Before a requested draw, apply only enough queued events to reach the visible turn's first text.
    pub(super) fn drain_first_token(&mut self, key: u64, cx: &mut Context<Self>) -> usize {
        let mut received = 0;
        while received < 64 {
            if self
                .state
                .borrow()
                .runs
                .get(&key)
                .is_none_or(|run| !run.running || run.entries.last().is_some_and(|entry| !entry.streamed.is_empty()))
            {
                break;
            }
            let Ok(event) = self.events.borrow_mut().try_recv() else {
                break;
            };
            self.receive(event, cx);
            received += 1;
        }
        received
    }
}
#[derive(Clone)]
pub(super) struct Binding {
    owner: Entity<Owner>,
    state: Rc<RefCell<State>>,
    incarnation: u64,
}
/// Dropping a parked lease revokes even when a retired Workspace clone still exists.
pub(super) struct Retained {
    owner: Option<Entity<Owner>>,
    state: Rc<RefCell<State>>,
    selected: Option<u64>,
}
impl Drop for Retained {
    fn drop(&mut self) {
        if self.owner.is_some() {
            self.state.borrow_mut().revoke();
        }
    }
}

#[cfg(test)]
pub(in crate::workspace) mod tests;
