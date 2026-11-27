//! Detached browser output appends only to its original authenticated Code-session draft.
#[cfg(test)]
use super::ParkedDraft;
use super::{ComposerDrafts, DraftOwner, Workspace, ensure_scope, schedule_save};
use crate::attachments;
use gpui_kit::{App, BorrowAppContext};
use nexus_workbench::composer_drafts;

#[derive(Clone)]
pub(in crate::workspace) struct BrowserDraft {
    owner: DraftOwner,
}
impl Workspace {
    pub(in crate::workspace) fn browser_draft(&self, thread: &str, cx: &App) -> Option<BrowserDraft> {
        let proof = self.gateway_account_authority.as_ref()?;
        if !proof.current() {
            return None;
        }
        let identity = self.terminal_identity.as_ref()?;
        if !identity.matches_credential(self.gateway.url(), self.gateway.marker().as_deref()) {
            return None;
        }
        let marker = self.gateway.marker()?;
        if !proof.current() {
            return None;
        }
        let scope = composer_drafts::storage_scope(self.gateway.url(), identity.org(), Some(&marker))?;
        let store = cx
            .try_global::<ComposerDrafts>()
            .and_then(|global| global.scopes.get(&scope));
        let generation = store.map_or(0, |store| store.generation(thread));
        Some(BrowserDraft {
            owner: DraftOwner {
                scope,
                key: thread.into(),
                generation,
                persistent: crate::managed::local_cache(crate::managed::policy()),
            },
        })
    }
}
impl BrowserDraft {
    /// A receipt snapshots generation from the frozen key, never from whichever composer is visible.
    pub(in crate::workspace) fn receipt(&self, cx: &App) -> Self {
        let mut receipt = self.clone();
        receipt.owner.generation = cx
            .try_global::<ComposerDrafts>()
            .and_then(|global| global.scopes.get(&self.owner.scope))
            .map_or(0, |store| store.generation(&self.owner.key));
        receipt
    }
    pub(in crate::workspace) fn generation_current(&self, cx: &App) -> bool {
        cx.try_global::<ComposerDrafts>()
            .and_then(|global| global.scopes.get(&self.owner.scope))
            .map_or(self.owner.generation == 0, |store| {
                store.generation(&self.owner.key) == self.owner.generation
            })
    }
    pub(in crate::workspace) fn ready(&self, cx: &App) -> bool {
        if !self.generation_current(cx) {
            return false;
        }
        cx.try_global::<ComposerDrafts>()
            .and_then(|global| global.scopes.get(&self.owner.scope))
            .map_or(!self.owner.persistent, |store| {
                !store.persistent
                    || store
                        .slots
                        .get(&store.key(&self.owner.key))
                        .is_some_and(|slot| slot.loaded || slot.touched)
            })
    }
    pub(in crate::workspace) fn reading(&self, cx: &App) -> bool {
        cx.try_global::<ComposerDrafts>()
            .and_then(|global| global.scopes.get(&self.owner.scope))
            .and_then(|store| store.slots.get(&store.key(&self.owner.key)))
            .is_some_and(|slot| slot.reading)
    }
    pub(in crate::workspace) fn load(&self, retry: bool, cx: &mut App) {
        ensure_scope(&self.owner, false, cx);
        super::request_load(&self.owner, retry, cx);
    }
    pub(in crate::workspace) fn target(&self, scope: &str, key: &str, cx: &App) -> bool {
        self.owner.scope == scope
            && cx
                .try_global::<ComposerDrafts>()
                .and_then(|global| global.scopes.get(scope))
                .is_some_and(|store| store.key(&self.owner.key) == key)
    }
    #[cfg(test)]
    pub(in crate::workspace) fn revision(&self, cx: &App) -> u64 {
        cx.global::<ComposerDrafts>()
            .scopes
            .get(&self.owner.scope)
            .map_or(0, |store| store.revision(&self.owner.key))
    }
    /// Hold a real production read ticket over a private on-disk fixture, without a timer or outcome injection.
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_hold_read(
        &self,
        root: std::path::PathBuf,
        saved: ParkedDraft,
        cx: &mut App,
    ) -> HeldBrowserRead {
        let disk = std::sync::Arc::new(super::storage::DiskStore::new(root, true));
        disk.write(vec![super::storage::Change {
            key: self.owner.key.clone(),
            revision: 1,
            draft: saved,
        }])
        .expect("private saved draft fixture");
        let (key, revision) = cx.update_global::<ComposerDrafts, _>(|global, _| {
            let mut store = super::ScopeStore::new(Some(disk.clone()), true);
            let read = store
                .begin_read(&self.owner.key, false)
                .expect("production held read ticket");
            global.scopes.insert(self.owner.scope.clone(), store);
            read
        });
        HeldBrowserRead {
            disk,
            scope: self.owner.scope.clone(),
            key,
            revision,
        }
    }
    pub(in crate::workspace) fn text(&self, text: &str, cx: &mut App) -> bool {
        self.append(Some(text), None, cx)
    }
    pub(in crate::workspace) fn attachments(&self, prepared: attachments::Prepared, cx: &mut App) -> bool {
        self.append(None, Some(prepared), cx)
    }
    fn append(&self, text: Option<&str>, prepared: Option<attachments::Prepared>, cx: &mut App) -> bool {
        if !self.ready(cx) {
            return false;
        }
        ensure_scope(&self.owner, false, cx);
        cx.update_global::<ComposerDrafts, _>(|global, _| {
            let Some(store) = global.scopes.get_mut(&self.owner.scope) else {
                return;
            };
            let mut current = store.get(&self.owner.key);
            if let Some(text) = text {
                current.text = if current.text.trim().is_empty() {
                    text.into()
                } else {
                    format!("{}\n\n{text}", current.text.trim_end())
                };
            }
            if let Some(prepared) = prepared {
                let mut draft = attachments::Draft {
                    images: std::mem::take(&mut current.images),
                    files: std::mem::take(&mut current.files),
                    ..Default::default()
                };
                current.problem = attachments::merge(&mut draft, prepared);
                current.images = draft.images;
                current.files = draft.files;
            }
            store.put(&self.owner.key, current, false);
        });
        schedule_save(true, cx);
        true
    }
    #[cfg(test)]
    pub(in crate::workspace) fn snapshot(&self, cx: &App) -> ParkedDraft {
        cx.global::<ComposerDrafts>()
            .scopes
            .get(&self.owner.scope)
            .expect("original browser draft scope")
            .get(&self.owner.key)
    }
}

#[cfg(test)]
pub(in crate::workspace) struct HeldBrowserRead {
    disk: std::sync::Arc<super::storage::DiskStore>,
    scope: String,
    key: String,
    revision: u64,
}
#[cfg(test)]
impl HeldBrowserRead {
    pub(in crate::workspace) fn finish(self, cx: &mut App) -> bool {
        let saved = self.disk.read(&self.key).expect("actual saved draft fixture read");
        let restored = cx.update_global::<ComposerDrafts, _>(|global, _| {
            global
                .scopes
                .get_mut(&self.scope)
                .expect("held draft scope")
                .restore(&self.key, self.revision, saved)
        });
        crate::workspace::browser_detach::draft_read_finished(&self.scope, &self.key, cx);
        restored
    }
}
