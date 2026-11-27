//! Chats opened before keep their built messages, so going back to one draws it without building its Markdown again,
//! and the sidebar's first chats are read ahead while nothing answers, so a first open needs no wait on Gateway.
//! A kept transcript is shown only when Gateway's copy still has the messages it was built from; the open
//! still asks Gateway again (`read_chat`), and a newer copy replaces it.

use super::Workspace;
use super::transcript::Entry;
use crate::model::Message;

/// How many chats keep their built messages besides the open one.
const KEPT: usize = 6;
/// How many of the sidebar's chats are read ahead.
const PRELOAD: usize = 6;

/// The open chat as built from Gateway's copy: the messages, and which entries they made.
struct Shown {
    id: String,
    messages: Vec<Message>,
    ends: (usize, usize),
    len: usize,
}

struct Kept {
    id: String,
    messages: Vec<Message>,
    entries: Vec<Entry>,
}

#[derive(Default)]
pub(super) struct KeptChats {
    shown: Option<Shown>,
    /// The least recently left first.
    kept: Vec<Kept>,
}

impl KeptChats {
    /// Chat `id` now shows `entries`, built from `messages`.
    pub fn shown(&mut self, id: &str, messages: Vec<Message>, entries: &[Entry]) {
        self.shown = Some(Shown {
            id: id.into(),
            messages,
            ends: ends(entries),
            len: entries.len(),
        });
    }

    /// A fresh read can confirm the warm read without rebuilding its entries.
    pub fn matches(&self, id: &str, messages: &[Message], entries: &[Entry]) -> bool {
        self.shown.as_ref().is_some_and(|shown| {
            shown.id == id
                && shown.messages == messages
                && shown.len == entries.len()
                && shown.ends == ends(entries)
                && entries.iter().all(|entry| !entry.streaming && !entry.arriving)
        })
    }

    /// The open chat changed here (a question, a rewind): its entries no longer are Gateway's copy.
    pub fn mutated(&mut self) {
        self.shown = None;
    }

    /// Leaving chat `id`: keep its entries when they are still as built, and take them out of `entries`.
    pub fn leave(&mut self, id: Option<&str>, entries: &mut Vec<Entry>) {
        let Some(shown) = self.shown.take() else { return };
        let unchanged = id == Some(shown.id.as_str())
            && entries.len() == shown.len
            && ends(entries) == shown.ends
            && entries.iter().all(|entry| !entry.streaming && !entry.arriving);
        if !unchanged || entries.is_empty() {
            return;
        }
        self.kept.retain(|kept| kept.id != shown.id);
        self.kept.push(Kept {
            id: shown.id,
            messages: shown.messages,
            entries: std::mem::take(entries),
        });
        if self.kept.len() > KEPT {
            self.kept.remove(0);
        }
    }

    /// The entries kept for chat `id`, when they were built from `messages`; a kept copy that differs goes.
    pub fn take(&mut self, id: &str, messages: &[Message]) -> Option<Vec<Entry>> {
        let at = self.kept.iter().position(|kept| kept.id == id)?;
        let kept = self.kept.remove(at);
        (kept.messages == messages).then_some(kept.entries)
    }

    /// Entries kept off screen, so an attachment that finishes loading still finds its entry.
    pub fn entries_mut(&mut self) -> impl Iterator<Item = &mut Entry> {
        self.kept.iter_mut().flat_map(|kept| kept.entries.iter_mut())
    }

    /// Another organization or sign-in: nothing kept is theirs.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The first and last entry ids: every build gives new ids, so a rebuild or a rewind changes them.
fn ends(entries: &[Entry]) -> (usize, usize) {
    (
        entries.first().map_or(0, |entry| entry.id),
        entries.last().map_or(0, |entry| entry.id),
    )
}

impl Workspace {
    /// Read the sidebar's first chats ahead while nothing answers: the list comes after each answer, and an answer's
    /// writes empty the warm cache.
    pub(super) fn preload_chats(&self) {
        if self.chat_busy() {
            return;
        }
        for session in self.sessions.iter().take(PRELOAD) {
            self.preload_chat(&session.id);
        }
    }

    /// Read chat `id` ahead, unless it is open: the pointer is on its row. Gateway skips one already saved.
    pub(super) fn preload_chat(&self, id: &str) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if self.current.as_deref() == Some(id) {
            return;
        }
        account_gateway.preload_session(id.into());
    }
}

#[cfg(test)]
#[path = "kept_chats_tests.rs"]
mod tests;
