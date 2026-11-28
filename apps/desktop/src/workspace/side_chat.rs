//! A side chat has its own log, composer, approvals and draft. Its core owns the fork and Bring back semantics.
mod budget;
mod commands;
mod composer;
mod controls;
mod creation;
mod dock;
mod echo;
mod history;
mod keys;
mod message_composer;
mod operations;
mod partials;
mod permission;
mod rename;
mod render;
mod retention;
mod uploads;
mod workspace;
use super::runs::Runs;
use super::{View, Workspace};
use crate::code::{self, Block};
pub(in crate::workspace) use creation::Creation;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::text::TextViewState;
use gpui_kit::*;
use nexus_workbench::event::{StoredEvent, ThreadSummary};
pub(in crate::workspace) use retention::{RetainedChat, RetainedSides};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub(super) enum SideEvent {
    Closed(bool),
    /// Open the draft's images full size at `index`, in the window's image viewer.
    OpenImages(Vec<crate::attachments::Attachment>, usize),
}
impl EventEmitter<SideEvent> for SideChat {}

pub(super) struct SideChat {
    retired: bool,
    ready: bool,
    allow_partial_stream: bool,
    draft_revision: u64,
    draft_text: String,
    pending: operations::Pending<operations::Completion>,
    _completion: Option<Task<()>>,
    summary: ThreadSummary,
    parent_title: String,
    menu_open: bool,
    renaming: Option<rename::Renaming>,
    /// When the head's title last changed on screen, for its motion (`motion::retitle`).
    retitled: Option<std::time::Instant>,
    /// A read has landed: a title that changes after it changed on screen, not as the side chat loaded.
    loaded: bool,
    /// The composer menu that is open, if any.
    settings: Option<composer::Section>,
    attach_open: bool,
    permissions_collapsed: bool,
    working_since: Option<std::time::Instant>,
    history: history::History,
    dock_height: super::floating_dock::DockHeight,
    models: Vec<composer::Choice>,
    model_input: Entity<InputState>,
    models_loading: bool,
    catalog_problem: Option<String>,
    rewrite: Option<composer::Rewrite>,
    runs: Runs,
    events: Vec<StoredEvent>,
    blocks: Vec<Block>,
    texts: HashMap<usize, Entity<TextViewState>>,
    streamed: Option<(String, Entity<TextViewState>)>,
    input: Entity<TextareaState>,
    draft: crate::attachments::Draft,
    /// Messages sent but not yet handed to the core, oldest first (`commands.rs`, `flush_outbox`).
    outbox: std::collections::VecDeque<operations::Outgoing>,
    /// The sent message's files while they go up in pieces (`uploads.rs`).
    uploading: Option<crate::attachments::Draft>,
    /// Sent messages the history shows before a read brings them (`echo.rs`).
    echo: Vec<echo::Echo>,
    /// Wakes the read loop at once, as a send lands.
    wake: Option<futures::channel::mpsc::UnboundedSender<()>>,
    /// Up and Down's walk through what was sent (`keys.rs`).
    recall: Option<super::prompt_history::Walk>,
    /// Files are dragged over the window, and a drop attaches them here (`Workspace::show_file_drag`).
    file_drag: bool,
    /// The used-up budget's line for this side chat's payer (`budget.rs`).
    budget: Option<String>,
    /// A send held for the budget, and whether it was queued: it goes once the budget lifts.
    held: Option<bool>,
    answers: Vec<Entity<InputState>>,
    request: Option<String>,
    permission_reason: Entity<InputState>,
    remember_open: bool,
    secret: Entity<InputState>,
    secret_id: Option<String>,
    sending: bool,
    deleting: bool,
    problem: Option<String>,
    note: Option<String>,
    brought_back: bool,
    scroll: history::HistoryList,
    poll: Option<Task<()>>,
    partial_poll: Option<Task<()>>,
    partial_epoch: u64,
    partial_revision: Option<(String, u64)>,
    _subscriptions: Vec<Subscription>,
}

impl SideChat {
    fn new(
        summary: ThreadSummary,
        parent_title: String,
        runs: Runs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Ask about this session…")
                .submit_on_enter(true)
                .auto_grow(1, 6)
        });
        let secret = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Paste the secret…")
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if this.retired {
                return;
            }
            match event {
                // As the session composer: Enter sends into the running turn, Ctrl+Enter (Cmd+Enter) queues it for after.
                InputEvent::PressEnter {
                    shift: false,
                    secondary,
                } => this.send(*secondary, window, cx),
                InputEvent::Change => {
                    let text = this.input.read(cx).value().to_string();
                    if text != this.draft_text {
                        this.draft_text = text;
                        this.draft_revision = this.draft_revision.wrapping_add(1);
                    }
                }
                _ => {}
            }
        });
        let model_input = cx.new(|cx| InputState::new(window, cx).placeholder("Model identifier"));
        let mut chat = Self {
            retired: false,
            ready: true,
            allow_partial_stream: true,
            draft_revision: 0,
            draft_text: String::new(),
            pending: Default::default(),
            _completion: None,
            settings: None,
            attach_open: false,
            permissions_collapsed: false,
            working_since: None,
            history: Default::default(),
            dock_height: Default::default(),
            models: vec![],
            model_input,
            models_loading: false,
            catalog_problem: None,
            rewrite: None,
            summary,
            parent_title,
            menu_open: false,
            renaming: None,
            retitled: None,
            loaded: false,
            runs,
            events: vec![],
            blocks: vec![],
            texts: HashMap::new(),
            streamed: None,
            input,
            draft: Default::default(),
            outbox: Default::default(),
            uploading: None,
            echo: vec![],
            wake: None,
            recall: None,
            file_drag: false,
            budget: None,
            held: None,
            answers: vec![],
            request: None,
            permission_reason: cx.new(|cx| InputState::new(window, cx).placeholder("Reason (optional)")),
            remember_open: false,
            secret,
            secret_id: None,
            sending: false,
            deleting: false,
            problem: None,
            note: None,
            brought_back: false,
            scroll: history::HistoryList::new(),
            poll: None,
            partial_poll: None,
            partial_epoch: 0,
            partial_revision: None,
            _subscriptions: vec![subscription],
        };
        chat.watch_commands(window, cx);
        chat
    }

    pub fn active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.retired || !self.pending.valid() || !active {
            self.poll = None;
            self.suspend_partials();
            return;
        }
        if self.poll.is_some() {
            return;
        }
        let (wake, mut woken) = futures::channel::mpsc::unbounded::<()>();
        self.wake = Some(wake);
        self.poll = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let reads = this.update(cx, |this, _| {
                    if this.retired || !this.pending.valid() {
                        return None;
                    }
                    let id = &this.summary.id;
                    let after = this.events.last().map_or(0, |e| e.seq);
                    Some((
                        this.runs
                            .call("wb_thread_events", json!({"threadId":id,"afterSeq":after})),
                        this.runs.call("wb_overview", json!({})),
                    ))
                });
                let Ok(Some((events, overview))) = reads else {
                    break;
                };
                let result = async {
                    let events = events.await.map_err(|_| "The side chat read was lost.".to_string())??;
                    let events: Vec<StoredEvent> = serde_json::from_value(events).map_err(|e| e.to_string())?;
                    let overview = overview
                        .await
                        .map_err(|_| "The side chat state was lost.".to_string())??;
                    let summaries: Vec<ThreadSummary> =
                        serde_json::from_value(overview["threads"].clone()).map_err(|e| e.to_string())?;
                    Ok::<_, String>((events, summaries))
                }
                .await;
                if this
                    .update_in(cx, |this, window, cx| {
                        if this.retired || !this.pending.valid() {
                            return;
                        }
                        match result {
                            Ok((events, summaries)) => this.apply_read(events, summaries, window, cx),
                            Err(error) => {
                                this.suspend_partials();
                                this.ready = false;
                                this.problem = Some(error);
                            }
                        }
                        this.preserve_gone_rewrite(window, cx);
                        this.follow_secret(window, cx);
                        this.sync_stream(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                // The next read in two seconds, or at once when a send lands (`wake`).
                let mut timer = std::pin::pin!(cx.background_executor().timer(Duration::from_secs(2)));
                if let futures::future::Either::Right((None, _)) =
                    futures::future::select(timer.as_mut(), futures::StreamExt::next(&mut woken)).await
                {
                    timer.await;
                }
            }
        }));
    }

    fn apply_read(
        &mut self,
        events: Vec<StoredEvent>,
        summaries: Vec<ThreadSummary>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(parent) = summaries
            .iter()
            .find(|s| Some(s.id.as_str()) == self.summary.source_thread_id.as_deref())
        {
            self.parent_title = parent.title.clone();
        }
        let Some(summary) = summaries.into_iter().find(|s| s.id == self.summary.id) else {
            cx.emit(SideEvent::Closed(true));
            return;
        };
        if !summary.side_chat
            || summary.source_thread_id != self.summary.source_thread_id
            || events.iter().any(|event| event.thread_id != self.summary.id)
        {
            self.ready = false;
            self.problem = Some("The side chat source changed. Reopen it from its parent session.".into());
            return;
        }
        self.ready = true;
        let shown = self.shown_title();
        if summary.updated_at >= self.summary.updated_at {
            self.summary = summary;
        }
        if !events.is_empty() {
            if !self.allow_partial_stream
                && events.iter().any(|event| {
                    matches!(
                        &event.event,
                        nexus_workbench::event::WbEvent::Text { parent: None, .. }
                            | nexus_workbench::event::WbEvent::TurnEnd { .. }
                            | nexus_workbench::event::WbEvent::Session { .. }
                            | nexus_workbench::event::WbEvent::Error { .. }
                    )
                })
            {
                self.streamed = None;
            }
            if events.iter().any(|stored| {
                stored.at >= self.summary.created_at
                    && matches!(
                        &stored.event,
                        nexus_workbench::event::WbEvent::Text { parent: None, .. }
                    )
            }) {
                self.brought_back = false;
            }
            let after = self.events.last().map_or(0, |event| event.seq);
            self.events.extend(events.into_iter().filter(|event| event.seq > after));
            self.blocks = own_blocks(&self.events, self.summary.created_at);
            let placeholder = if self.is_empty() {
                "Ask about this session…"
            } else {
                "Ask a follow-up…"
            };
            self.input
                .update(cx, |input, cx| input.set_placeholder(placeholder, window, cx));
            for (ix, block) in self.blocks.iter().enumerate() {
                if let Block::Text(text) = block {
                    match self.texts.get(&ix) {
                        Some(view) => view.update(cx, |view, cx| view.set_text(text, cx)),
                        None => {
                            self.texts.insert(ix, cx.new(|cx| TextViewState::markdown(text, cx)));
                        }
                    }
                }
            }
            self.follow_request(window, cx);
        }
        if self.loaded && self.shown_title() != shown {
            self.retitled = Some(std::time::Instant::now());
        }
        self.loaded = true;

        self.preserve_gone_rewrite(window, cx);
        self.follow_secret(window, cx);
        self.start_partials(window, cx);
        self.echo_read();
        // A restored side chat waits for its first read before its line moves on.
        self.flush_outbox(window, cx);
    }

    fn follow_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = code::pending_approval(&self.blocks);
        let id = pending.and_then(|b| match b {
            Block::Approval { request_id, .. } => Some(request_id.clone()),
            _ => None,
        });
        if self.request == id {
            return;
        }
        self.request = id;
        self.remember_open = false;
        self.permissions_collapsed = false;
        self.permission_reason
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.answers.clear();
        if let Some(Block::Approval { input, .. }) = pending {
            for question in super::code_dock::questions_of(input) {
                let input = cx.new(|cx| {
                    let mut field = InputState::new(window, cx).placeholder("Your answer");
                    field.set_masked(question.secret, window, cx);
                    field
                });
                self.answers.push(input);
            }
        }
    }

    fn follow_secret(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = match &self.runs {
            Runs::Here(r) => r.secret_request(&self.summary.id).map(|s| s.id),
            Runs::There(_) => None,
        };
        if id != self.secret_id {
            self.secret_id = id;
            self.secret.update(cx, |field, cx| field.set_value("", window, cx));
        }
    }

    pub fn attach_paths(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.attach_files(paths.iter().map(|p| crate::attachments::from_path(p)).collect(), cx);
    }

    fn attach_files(&mut self, files: Vec<crate::attachments::Incoming>, cx: &mut Context<Self>) {
        // A message in flight has left the composer: files attach for the next one meanwhile.
        if self.retired || !self.pending.valid() {
            return;
        }
        let work = cx.background_spawn(async move { crate::attachments::prepare(files) });
        cx.spawn(async move |this, cx| {
            let prepared = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.retired || !this.pending.valid() {
                    return;
                }
                this.problem = crate::attachments::merge(&mut this.draft, prepared);
                this.draft_revision = this.draft_revision.wrapping_add(1);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn sync_stream(&mut self, cx: &mut Context<Self>) {
        if self.retired || !self.pending.valid() || !self.ready || !self.allow_partial_stream {
            return;
        }
        if self.set_stream(crate::stream::text(&self.summary.id), cx) {
            cx.notify();
        }
    }

    /// Keep the Markdown parser and measured blocks while a partial answer grows.
    fn set_stream(&mut self, text: Option<String>, cx: &mut Context<Self>) -> bool {
        match (text, &mut self.streamed) {
            (Some(text), Some((old, view))) if *old != text => {
                view.update(cx, |state, cx| state.set_text(&text, cx));
                *old = text;
                true
            }
            (Some(text), None) => {
                let view = cx.new(|cx| TextViewState::markdown(&text, cx));
                self.streamed = Some((text, view));
                true
            }
            (None, Some(_)) => {
                self.streamed = None;
                true
            }
            _ => false,
        }
    }

    fn answer(&mut self, decision: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Block::Approval {
            request_id,
            tool,
            input,
            ..
        }) = code::pending_approval(&self.blocks)
        else {
            return;
        };
        let mut args = json!({"threadId":self.summary.id,"requestId":request_id,"decision":decision});
        if tool == super::code_dock::QUESTION_TOOL && decision == "allow" {
            let questions = super::code_dock::questions_of(input);
            let values: Vec<_> = self
                .answers
                .iter()
                .zip(&questions)
                .map(|(v, q)| {
                    let text = v.read(cx).value().to_string();
                    if q.multi {
                        text.split(';')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect()
                    } else {
                        vec![text]
                    }
                })
                .collect();
            if values.iter().any(|v| v.is_empty() || v[0].trim().is_empty()) {
                self.problem = Some("Answer each question before sending.".into());
                cx.notify();
                return;
            }
            args["answers"] = Value::Object(super::code_dock::question_answers(&questions, &values));
        }
        self.command("wb_approve", args, false, None, window, cx);
    }
}

/// The side panel shows its own conversation; copied parent history only informs the agent.
fn own_blocks(events: &[StoredEvent], since: i64) -> Vec<Block> {
    code::blocks_at(
        events
            .iter()
            .filter(|s| s.at >= since)
            .map(|s| (s.seq, s.at, s.event.clone())),
    )
}

/// Recognize only complete command names; /sideways is an agent message.
pub(super) fn side_question(prompt: &str) -> Option<&str> {
    let (command, rest) = prompt.split_once(char::is_whitespace).unwrap_or((prompt, ""));
    matches!(command, "/btw" | "/side").then(|| rest.trim())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod history_tests;
#[cfg(test)]
mod lifecycle_tests;
