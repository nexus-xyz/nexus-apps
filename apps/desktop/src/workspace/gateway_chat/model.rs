//! Execution data has no presentation handles. One transport side owns all of its turn numbers.
use super::super::{Entry, chat_rewind::EditSent, drafts::DetachedDraft};
use crate::{
    attachments::Draft,
    gateway::ChatChoices,
    model::{Meta, ModelChoice, PageBlock, Role, Stored},
};
use gpui_kit::{App, SharedString};
use nexus_workbench::chat_queue::Item;
use std::sync::atomic::{AtomicU64, Ordering};

/// One counter for every run of every window, so a revision never repeats, even across owners.
static REVISION: AtomicU64 = AtomicU64::new(0);

/// The latest revision given out: a message stamped after this reads as changed.
pub(super) fn revision() -> u64 {
    REVISION.load(Ordering::SeqCst)
}

fn next_revision() -> u64 {
    REVISION.fetch_add(1, Ordering::SeqCst) + 1
}

#[derive(Clone)]
pub(super) struct Message {
    /// When it was made or last changed: projection applies only the messages changed since it last ran.
    pub revision: u64,
    pub id: usize,
    pub role: Role,
    pub text: SharedString,
    pub streamed: String,
    pub streaming: bool,
    pub stopped: bool,
    pub error: Option<SharedString>,
    pub page: Option<PageBlock>,
    pub writing_page: bool,
    pub searching_web: bool,
    pub web_sources: Vec<nexus_desktop_core::chat::WebSource>,
    pub meta: Meta,
    pub attachments: Draft,
    pub stored: Stored,
}
impl Message {
    pub fn user(id: usize, text: String, attachments: Draft) -> Self {
        Self {
            revision: next_revision(),
            id,
            role: Role::User,
            text: text.into(),
            streamed: String::new(),
            streaming: false,
            stopped: false,
            error: None,
            page: None,
            writing_page: false,
            searching_web: false,
            web_sources: Vec::new(),
            meta: Meta::default(),
            attachments,
            stored: Stored::default(),
        }
    }
    pub fn answer(id: usize) -> Self {
        let mut value = Self::user(id, String::new(), Draft::default());
        value.role = Role::Assistant;
        value.streaming = true;
        value
    }

    /// Mark it changed. Every change to a message in a run calls this, or projection misses the change.
    pub fn touch(&mut self) {
        self.revision = next_revision();
    }

    pub fn capture(entry: &Entry) -> Self {
        Self {
            revision: next_revision(),
            id: entry.id,
            role: entry.role,
            text: entry.text.clone(),
            streamed: entry.streamed.clone(),
            streaming: entry.streaming,
            stopped: entry.stopped,
            error: entry.error.clone(),
            page: entry.page.clone(),
            writing_page: entry.writing_page,
            searching_web: entry.searching_web,
            web_sources: entry.web_sources.clone(),
            meta: entry.meta.clone(),
            attachments: entry.attachments.clone(),
            stored: entry.stored.clone(),
        }
    }
    pub fn present(&self, cx: &mut App) -> Entry {
        let mut entry = if self.role == Role::User {
            Entry::user(self.id, self.text.to_string())
        } else {
            Entry::answer(self.id, &self.streamed, self.streaming, cx)
        };
        self.apply(&mut entry, cx);
        entry
    }
    pub fn apply(&self, entry: &mut Entry, cx: &mut App) {
        entry.text = self.text.clone();
        if entry.streamed != self.streamed {
            // A growing answer copies only its new text, not all of it.
            let appended = self.streaming && self.streamed.starts_with(&entry.streamed);
            let tail = &self.streamed[if appended { entry.streamed.len() } else { 0 }..];
            if let Some(answer) = &entry.answer {
                answer.update(cx, |answer, cx| {
                    if appended {
                        answer.push_str(tail, cx);
                    } else {
                        answer.set_text(&self.streamed, cx);
                    }
                });
            }
            if appended {
                entry.streamed.push_str(tail);
            } else {
                entry.streamed.clone_from(&self.streamed);
            }
        }
        entry.streaming = self.streaming;
        entry.stopped = self.stopped;
        entry.error = self.error.clone();
        entry.page = self.page.clone();
        entry.writing_page = self.writing_page;
        entry.searching_web = self.searching_web;
        entry.web_sources = self.web_sources.clone();
        entry.meta = self.meta.clone();
        entry.attachments = self.attachments.clone();
        entry.stored = self.stored.clone();
    }
}
#[derive(Clone)]
pub(super) struct Run {
    pub key: u64,
    pub turn: u64,
    pub id: Option<String>,
    pub title: SharedString,
    pub entries: Vec<Message>,
    pub model: Option<ModelChoice>,
    pub choices: ChatChoices,
    pub web_search_flag: Option<bool>,
    pub session_web_search: bool,
    pub panel: bool,
    pub archived: bool,
    pub running: bool,
    pub accepted: bool,
    pub queue_key: String,
    pub sent: Option<(u64, Item<Draft>)>,
    pub edit: Option<EditSent>,
    pub restore_edit: Option<EditSent>,
    pub finished: Option<(u64, bool, Option<PageBlock>)>,
    pub draft: Option<DetachedDraft>,
}
impl Run {
    pub fn end(&mut self, stopped: bool) {
        self.running = false;
        if let Some(entry) = self.entries.last_mut() {
            entry.streaming = false;
            entry.writing_page = false;
            entry.searching_web = false;
            entry.stopped = stopped;
            entry.touch();
        }
    }
    /// Everything but its messages and parked draft: projection reads the messages in place, without a copy.
    pub fn without_entries(&self) -> Self {
        Self {
            key: self.key,
            turn: self.turn,
            id: self.id.clone(),
            title: self.title.clone(),
            entries: Vec::new(),
            model: self.model.clone(),
            choices: self.choices.clone(),
            web_search_flag: self.web_search_flag,
            session_web_search: self.session_web_search,
            panel: self.panel,
            archived: self.archived,
            running: self.running,
            accepted: self.accepted,
            queue_key: self.queue_key.clone(),
            sent: self.sent.clone(),
            edit: self.edit.clone(),
            restore_edit: self.restore_edit.clone(),
            finished: self.finished.clone(),
            draft: None,
        }
    }
}
