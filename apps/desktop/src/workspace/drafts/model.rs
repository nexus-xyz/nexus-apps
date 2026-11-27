//! Draft revisions and preparation tickets are independent of views and storage completion order.
use super::storage::{Change, DiskStore};
use crate::attachments::Attachment;
use crate::i18n::tr;
use nexus_workbench::composer_drafts::{ComposerDraft, Drafts};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Default)]
pub(in crate::workspace) struct ParkedDraft {
    pub text: String,
    pub images: Vec<Attachment>,
    pub files: Vec<Attachment>,
    pub problem: Option<String>,
}
impl ComposerDraft for ParkedDraft {
    fn text(&self) -> &str {
        &self.text
    }
    fn set_text(&mut self, text: String) {
        self.text = text;
    }
    fn has_attachments(&self) -> bool {
        !self.images.is_empty() || !self.files.is_empty()
    }
    fn snapshot(&self) -> Self {
        self.clone()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workspace) struct DraftOwner {
    pub scope: String,
    pub key: String,
    pub generation: u64,
    pub persistent: bool,
}
impl DraftOwner {
    pub fn same_key(&self, other: &Self) -> bool {
        self.scope == other.scope && self.key == other.key
    }
}
#[derive(Default)]
pub(super) struct Slot {
    pub revision: u64,
    pub generation: u64,
    pub loaded: bool,
    pub reading: bool,
    pub attempted: bool,
    pub touched: bool,
    pub dirty: bool,
    pub problem: Option<String>,
}
pub(super) struct ScopeStore {
    pub drafts: Drafts<ParkedDraft>,
    pub slots: HashMap<String, Slot>,
    pub disk: Option<Arc<DiskStore>>,
    pub persistent: bool,
    pub writing: bool,
    next_revision: u64,
    aliases: HashMap<String, String>,
    tickets: HashMap<(String, u64), (String, u64)>,
}
impl ScopeStore {
    pub fn new(disk: Option<Arc<DiskStore>>, persistent: bool) -> Self {
        Self {
            drafts: Drafts::default(),
            slots: HashMap::new(),
            persistent: persistent && disk.is_some(),
            disk,
            writing: false,
            next_revision: 0,
            aliases: HashMap::new(),
            tickets: HashMap::new(),
        }
    }
    pub fn enable(&mut self, persistent: bool) {
        let enabled = persistent && self.disk.is_some();
        if let Some(disk) = &self.disk {
            disk.enable(enabled);
        }
        if enabled != self.persistent {
            for slot in self.slots.values_mut().filter(|s| !s.touched) {
                self.next_revision += 1;
                slot.revision = self.next_revision;
                slot.reading = false;
                if enabled {
                    slot.loaded = false;
                    slot.attempted = false;
                }
            }
        }
        self.persistent = enabled;
    }
    pub fn key(&self, key: &str) -> String {
        let mut key = key.to_string();
        for _ in 0..16 {
            match self.aliases.get(&key) {
                Some(next) if next != &key => key = next.clone(),
                _ => break,
            }
        }
        key
    }
    pub fn ensure(&mut self, key: &str) -> String {
        let key = self.key(key);
        self.slots.entry(key.clone()).or_insert_with(|| Slot {
            loaded: !self.persistent,
            ..Default::default()
        });
        key
    }
    pub fn get(&self, key: &str) -> ParkedDraft {
        self.drafts.get(&self.key(key)).cloned().unwrap_or_default()
    }
    pub fn revision(&self, key: &str) -> u64 {
        self.slots.get(&self.key(key)).map_or(0, |slot| slot.revision)
    }
    pub fn generation(&self, key: &str) -> u64 {
        self.slots.get(&self.key(key)).map_or(0, |slot| slot.generation)
    }
    pub fn put(&mut self, key: &str, draft: ParkedDraft, invalidate: bool) -> u64 {
        let key = self.ensure(key);
        self.next_revision += 1;
        let slot = self.slots.get_mut(&key).expect("ensured draft slot");
        slot.revision = self.next_revision;
        slot.loaded = true;
        slot.reading = false;
        slot.attempted = true;
        slot.touched = true;
        slot.dirty = true;
        slot.problem = draft.problem.clone();
        if invalidate {
            slot.generation += 1;
        }
        self.drafts.put(&key, draft);
        self.next_revision
    }
    pub fn begin_read(&mut self, key: &str, reopen: bool) -> Option<(String, u64)> {
        let key = self.ensure(key);
        let slot = self.slots.get_mut(&key)?;
        if !self.persistent || slot.loaded || slot.reading || slot.touched || (slot.attempted && !reopen) {
            return None;
        }
        slot.reading = true;
        slot.attempted = true;
        Some((key, slot.revision))
    }
    pub fn can_restore(&self, key: &str, revision: u64) -> bool {
        self.persistent
            && self
                .slots
                .get(key)
                .is_some_and(|slot| slot.reading && !slot.touched && slot.revision == revision)
    }
    pub fn restore(&mut self, key: &str, revision: u64, draft: Option<ParkedDraft>) -> bool {
        if !self.can_restore(key, revision) {
            return false;
        }
        self.next_revision += 1;
        let draft = draft.unwrap_or_default();
        let slot = self.slots.get_mut(key).expect("active read slot");
        slot.revision = self.next_revision;
        slot.loaded = true;
        slot.reading = false;
        slot.problem = draft.problem.clone();
        self.drafts.put(key, draft);
        true
    }
    pub fn read_failed(&mut self, key: &str, revision: u64, finished: bool) {
        if !self.can_restore(key, revision) {
            return;
        }
        let slot = self.slots.get_mut(key).expect("active read slot");
        slot.problem =
            Some(tr("Draft storage could not be opened. Your current draft stays in this window.").to_string());
        if finished {
            slot.reading = false;
        }
    }
    pub fn changes(&self) -> Vec<Change> {
        self.slots
            .iter()
            .filter(|(key, slot)| slot.dirty && !key.starts_with("draft-"))
            .map(|(key, slot)| Change {
                key: key.clone(),
                revision: slot.revision,
                draft: self.drafts.get(key).cloned().unwrap_or_default(),
            })
            .collect()
    }
    pub fn acknowledge(&mut self, changes: &[(String, u64)], success: bool) {
        self.writing = false;
        for (key, revision) in changes {
            if let Some(slot) = self.slots.get_mut(key)
                && slot.revision == *revision
            {
                if success {
                    slot.dirty = false;
                    slot.problem = None;
                } else {
                    slot.problem = Some(
                        tr("Draft storage could not save. Your draft stays available in this window.").to_string(),
                    );
                }
            }
        }
    }
    pub fn resolve_ticket(&self, key: &str, generation: u64) -> Option<(String, u64)> {
        let mut owner = (key.to_string(), generation);
        for _ in 0..16 {
            match self.tickets.get(&owner) {
                Some(next) if next != &owner => owner = next.clone(),
                _ => break,
            }
        }
        let key = self.key(&owner.0);
        (self.generation(&key) == owner.1).then_some((key, owner.1))
    }
    pub fn promote_snapshot(&mut self, from: &str, to: &str, revision: u64, draft: ParkedDraft) {
        if from == to {
            return;
        }
        if self.revision(from) == revision {
            self.rename(from, to);
        } else if self.revision(to) == 0 {
            // Home was reused after navigation. Only the detached snapshot belongs to this chat.
            self.put(to, draft, false);
        }
    }
    pub fn rename(&mut self, from: &str, to: &str) {
        let from = self.key(from);
        let to = self.key(to);
        if from == to {
            return;
        }
        let original = self.get(&from);
        let generation = self.generation(&from);
        let use_source = self.revision(&to) <= self.revision(&from);
        self.put(&from, ParkedDraft::default(), true);
        if use_source {
            self.put(&to, original, false);
            if let Some(slot) = self.slots.get_mut(&to) {
                slot.generation = generation;
            }
        }
        let target_generation = self.generation(&to);
        if use_source || target_generation == 0 {
            self.tickets
                .insert((from.clone(), generation), (to.clone(), target_generation));
        }
        // Passing Code ids are never reused. Home/new-target keys are reused, so only old preparation tickets move.
        if from.starts_with("draft-") {
            self.aliases.insert(from.clone(), to);
            self.slots.remove(&from);
        }
    }
}

/// Only accepted attachment ids leave a draft; later attachments and edits remain.
pub(super) fn after_send(mut current: ParkedDraft, submitted: &ParkedDraft) -> ParkedDraft {
    current.text = nexus_workbench::composer_drafts::after_send_text(&current.text, &submitted.text);
    current
        .images
        .retain(|item| !submitted.images.iter().any(|sent| sent.id == item.id));
    current
        .files
        .retain(|item| !submitted.files.iter().any(|sent| sent.id == item.id));
    current.problem = None;
    current
}

/// Merge only edits made by this window. Another window's newer attachments must not be overwritten by stale UI.
pub(super) fn merge_local(
    mut current: ParkedDraft,
    visible: ParkedDraft,
    previous_text: &str,
    previous: &[(bool, u64)],
) -> ParkedDraft {
    if visible.text != previous_text {
        current.text = visible.text;
        current.problem = None;
    }
    for (image, kept, shown) in [
        (true, &mut current.images, visible.images),
        (false, &mut current.files, visible.files),
    ] {
        let before = previous
            .iter()
            .filter(|(kind, _)| *kind == image)
            .map(|(_, id)| *id)
            .collect::<Vec<_>>();
        kept.retain(|item| !before.contains(&item.id) || shown.iter().any(|shown| shown.id == item.id));
        for item in shown {
            if !before.contains(&item.id) && !kept.iter().any(|known| known.id == item.id) {
                kept.push(item);
            }
        }
    }
    current
}

#[cfg(test)]
mod tests;
