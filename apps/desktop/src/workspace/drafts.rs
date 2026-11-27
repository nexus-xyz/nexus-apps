//! Complete composer drafts, scoped by Gateway, organization and sign-in identity.
//! Storage never reads or writes when local caching is disabled. View changes retain in-flight preparation owners.
use super::{View, Workspace};
use crate::attachments;
use gpui_kit::*;
use nexus_workbench::composer_drafts::{self, ComposerDraft};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
mod browser;
mod gateway;
pub(in crate::workspace) use browser::BrowserDraft;
mod model;
pub(super) use gateway::promote_execution_draft;
#[cfg(test)]
pub(in crate::workspace) use gateway::{fixture_memory_scope, fixture_scopes, fixture_submission_state};
mod storage;
use model::ScopeStore;
pub(super) use model::{DraftOwner, ParkedDraft};
use storage::{Change, DiskStore};

#[derive(Default)]
pub(super) struct ComposerDrafts {
    scopes: HashMap<String, ScopeStore>,
    pending_since: Option<Instant>,
    saving: Option<Task<()>>,
}
impl Global for ComposerDrafts {}
pub(super) fn init(cx: &mut App) {
    if !cx.has_global::<ComposerDrafts>() {
        cx.set_global(ComposerDrafts::default());
    }
}
fn ensure_scope(owner: &DraftOwner, update_policy: bool, cx: &mut App) {
    init(cx);
    let needed = cx
        .global::<ComposerDrafts>()
        .scopes
        .get(&owner.scope)
        .is_none_or(|scope| {
            (update_policy && scope.persistent != owner.persistent && scope.disk.is_some())
                || !scope.slots.contains_key(&scope.key(&owner.key))
        });
    if !needed {
        return;
    }
    let root = crate::paths::composer_drafts_root().map(|root| root.join(&owner.scope));
    cx.update_global::<ComposerDrafts, _>(|global, _| {
        let scope = global.scopes.entry(owner.scope.clone()).or_insert_with(|| {
            ScopeStore::new(
                root.map(|root| Arc::new(DiskStore::new(root, owner.persistent))),
                owner.persistent,
            )
        });
        if update_policy {
            scope.enable(owner.persistent);
        }
        scope.ensure(&owner.key);
    });
}
struct WriteJob {
    scope: String,
    disk: Arc<DiskStore>,
    changes: Vec<Change>,
}
fn write_pending(cx: &mut App, force: bool) -> Task<()> {
    init(cx);
    let jobs = cx.update_global::<ComposerDrafts, _>(|global, _| {
        global.pending_since = None;
        global.saving = None;
        global
            .scopes
            .iter_mut()
            .filter_map(|(id, scope)| {
                if !scope.persistent || (scope.writing && !force) {
                    return None;
                }
                let disk = scope.disk.clone()?;
                let changes = scope.changes();
                if changes.is_empty() {
                    return None;
                }
                scope.writing = true;
                Some(WriteJob {
                    scope: id.clone(),
                    disk,
                    changes,
                })
            })
            .collect::<Vec<_>>()
    });
    cx.spawn(async move |cx| {
        for job in jobs {
            let keys = job
                .changes
                .iter()
                .map(|change| (change.key.clone(), change.revision))
                .collect::<Vec<_>>();
            let disk = job.disk;
            let write = cx.background_executor().spawn(async move { disk.write(job.changes) });
            // Quitting awaits the complete last write; normal editing has a three-second deadline.
            let result = if force {
                write.await
            } else {
                let deadline = cx.background_executor().timer(Duration::from_secs(3));
                match futures::future::select(write, deadline).await {
                    futures::future::Either::Left((result, _)) => result,
                    futures::future::Either::Right(_) => Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "draft save timed out",
                    )),
                }
            };
            let retry = cx.update(|cx| {
                cx.update_global::<ComposerDrafts, _>(|global, _| {
                    let Some(scope) = global.scopes.get_mut(&job.scope) else {
                        return false;
                    };
                    match result {
                        Ok(applied) => scope.acknowledge(&applied, true),
                        Err(_) => scope.acknowledge(&keys, false),
                    }
                    scope.writing = false;
                    scope.persistent
                        && scope
                            .slots
                            .iter()
                            .any(|(key, slot)| !key.starts_with("draft-") && slot.dirty && slot.problem.is_none())
                })
            });
            if retry && !force {
                cx.update(|cx| schedule_save(false, cx));
            }
        }
    })
}
/// App shutdown waits for the most recent snapshots, even when an older write is still running.
pub(crate) fn flush(cx: &mut App) -> Task<()> {
    write_pending(cx, true)
}
fn schedule_save(immediate: bool, cx: &mut App) {
    if immediate {
        write_pending(cx, false).detach();
        return;
    }
    let now = Instant::now();
    let since = cx.global::<ComposerDrafts>().pending_since.unwrap_or(now);
    let waited = now.duration_since(since).as_millis() as u64;
    let delay = Duration::from_millis(composer_drafts::save_delay_ms(0, waited));
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(delay).await;
        cx.update(|cx| write_pending(cx, false).detach());
    });
    cx.update_global::<ComposerDrafts, _>(|global, _| {
        global.pending_since = Some(since);
        global.saving = Some(task);
    });
}
fn request_load(owner: &DraftOwner, reopen: bool, cx: &mut App) {
    let wanted =
        cx.global::<ComposerDrafts>()
            .scopes
            .get(&owner.scope)
            .is_some_and(|scope| {
                scope.persistent
                    && scope.slots.get(&scope.key(&owner.key)).is_some_and(|slot| {
                        !slot.loaded && !slot.reading && !slot.touched && (!slot.attempted || reopen)
                    })
            });
    if !wanted {
        return;
    }
    let read = cx.update_global::<ComposerDrafts, _>(|global, _| {
        let scope = global.scopes.get_mut(&owner.scope)?;
        let disk = scope.disk.clone()?;
        let (key, revision) = scope.begin_read(&owner.key, reopen)?;
        Some((disk, key, revision))
    });
    let Some((disk, key, revision)) = read else { return };
    let scope_id = owner.scope.clone();
    cx.spawn(async move |cx| {
        for (attempt, delay) in [0, 2_000, 8_000].into_iter().enumerate() {
            if delay > 0 {
                cx.background_executor().timer(Duration::from_millis(delay)).await;
            }
            let current = cx.update(|cx| {
                cx.global::<ComposerDrafts>()
                    .scopes
                    .get(&scope_id)
                    .is_some_and(|scope| scope.can_restore(&key, revision))
            });
            if !current {
                return;
            }
            let disk = disk.clone();
            let read_key = key.clone();
            let read = cx.background_executor().spawn(async move { disk.read(&read_key) });
            let deadline = cx.background_executor().timer(Duration::from_secs(3));
            let result = match futures::future::select(read, deadline).await {
                futures::future::Either::Left((result, _)) => result,
                futures::future::Either::Right(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "draft read timed out",
                )),
            };
            let done = cx.update(|cx| {
                let done = cx.update_global::<ComposerDrafts, _>(|global, _| {
                    let Some(scope) = global.scopes.get_mut(&scope_id) else {
                        return true;
                    };
                    match result {
                        Ok(draft) => {
                            scope.restore(&key, revision, draft);
                            true
                        }
                        Err(_) => {
                            scope.read_failed(&key, revision, attempt == 2);
                            false
                        }
                    }
                });
                super::browser_detach::draft_read_finished(&scope_id, &key, cx);
                done
            });
            if done {
                return;
            }
        }
    })
    .detach();
}
#[derive(Clone)]
pub(super) struct Submission {
    owner: DraftOwner,
    draft: ParkedDraft,
    revision: u64,
    recovered: Arc<std::sync::atomic::AtomicBool>,
}
fn signature(draft: &attachments::Draft) -> Vec<(bool, u64)> {
    draft
        .images
        .iter()
        .map(|a| (true, a.id))
        .chain(draft.files.iter().map(|a| (false, a.id)))
        .collect()
}
#[derive(Clone)]
pub(super) struct DetachedDraft {
    owner: DraftOwner,
    draft: ParkedDraft,
    revision: u64,
}
impl Workspace {
    pub(super) fn detach_draft(&self) -> Option<DetachedDraft> {
        Some(DetachedDraft {
            owner: self.draft_at.clone()?,
            draft: ParkedDraft {
                text: self.draft_text.to_string(),
                images: self.draft.images.clone(),
                files: self.draft.files.clone(),
                problem: None,
            },
            revision: self.draft_revision,
        })
    }
    pub(super) fn promote_detached_draft(&mut self, previous: Option<DetachedDraft>, id: &str, cx: &mut Context<Self>) {
        let (Some(previous), Some(account)) = (previous, self.account.as_ref()) else {
            return;
        };
        let Ok(key) = serde_json::to_string(&(&account.org, Some(id))) else {
            return;
        };
        let key = format!("chat:{key}");
        ensure_scope(&previous.owner, false, cx);
        cx.update_global::<ComposerDrafts, _>(|global, _| {
            if let Some(store) = global.scopes.get_mut(&previous.owner.scope) {
                store.promote_snapshot(&previous.owner.key, &key, previous.revision, previous.draft);
            }
        });
        schedule_save(true, cx);
    }
    fn draft_key(&self) -> Option<String> {
        // The panel's composer keeps its text in its own state, outside the draft store.
        if self.panel {
            return None;
        }
        match &self.view {
            View::Code(view) => Some(view.id.clone()),
            View::NewCode => {
                let org = if self.is_local() {
                    nexus_workbench::session_scope::SessionScope::local_text()
                } else {
                    self.account.as_ref()?.org.as_str()
                };
                let project = self.code_form.project.as_deref();
                let machine = if self.cloud_ui.on {
                    Some(composer_drafts::CLOUD.to_string())
                } else {
                    project.and_then(|id| crate::machines::get().machine_of(id))
                };
                Some(composer_drafts::new_session_key(
                    org,
                    machine.as_deref(),
                    project,
                    composer_drafts::MAIN_WINDOW,
                ))
            }
            View::Chat => Some(format!(
                "chat:{}",
                serde_json::to_string(&(&self.account.as_ref()?.org, self.current.as_deref())).ok()?
            )),
            _ => None,
        }
    }
    pub(super) fn draft_owner(&self, cx: &App) -> Option<DraftOwner> {
        let local = self.editing_local_code();
        let org = if local {
            nexus_workbench::session_scope::SessionScope::local_text()
        } else {
            &self.account.as_ref()?.org
        };
        let proof = if matches!(self.view, View::Chat) {
            Some(self.gateway_account_authority.as_ref()?)
        } else {
            None
        };
        if proof.is_some_and(|proof| !proof.current()) {
            return None;
        }
        let marker = if local {
            Some("local-workspace".into())
        } else {
            self.gateway.marker()
        };
        let gateway = if local { "" } else { self.gateway.url() };
        if proof.is_some_and(|proof| !proof.current()) {
            return None;
        }
        let persisted = composer_drafts::storage_scope(gateway, org, marker.as_deref());
        let scope = persisted.clone().or_else(|| {
            composer_drafts::storage_scope(self.gateway.url(), org, Some("memory-only"))
                .map(|id| format!("memory-{id}-{}", self.draft_memory_identity))
        })?;
        let key = self.draft_key()?;
        let store = cx
            .try_global::<ComposerDrafts>()
            .and_then(|global| global.scopes.get(&scope));
        let generation = store.map_or(0, |store| store.generation(&key));
        let key = store.map_or(key.clone(), |store| store.key(&key));
        Some(DraftOwner {
            scope,
            key,
            generation,
            persistent: persisted.is_some() && crate::managed::local_cache(crate::managed::policy()),
        })
    }
    fn capture_draft(&self, cx: &App) -> ParkedDraft {
        ParkedDraft {
            text: self.composer.read(cx).value().to_string(),
            images: self.draft.images.clone(),
            files: self.draft.files.clone(),
            problem: None,
        }
    }
    fn apply_draft(
        &mut self,
        owner: DraftOwner,
        draft: ParkedDraft,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let different = self.composer.read(cx).value().as_ref() != draft.text;
        self.draft_at = Some(owner);
        self.draft_revision = revision;
        self.draft_text = draft.text.clone().into();
        self.draft.images = draft.images;
        self.draft.files = draft.files;
        self.draft_attachments = signature(&self.draft);
        self.composer_empty = self.draft_text.trim().is_empty();
        if different {
            self.composer
                .update(cx, |state, cx| state.set_value(draft.text, window, cx));
        }
        self.capture_gateway_draft();
    }
    pub(super) fn sync_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(owner) = self.draft_owner(cx) else { return };
        ensure_scope(&owner, true, cx);
        let changed_key = self.draft_at.as_ref().is_none_or(|previous| !previous.same_key(&owner));
        let current = self.capture_draft(cx);
        let current_signature = signature(&self.draft);
        if changed_key {
            if let Some(previous) = self.draft_at.clone() {
                let edited = current.text != self.draft_text.as_ref() || current_signature != self.draft_attachments;
                if edited {
                    ensure_scope(&previous, false, cx);
                    cx.update_global::<ComposerDrafts, _>(|global, _| {
                        if let Some(store) = global.scopes.get_mut(&previous.scope) {
                            let merged = model::merge_local(
                                store.get(&previous.key),
                                current,
                                &self.draft_text,
                                &self.draft_attachments,
                            );
                            store.put(&previous.key, merged, false);
                        }
                    });
                    schedule_save(!current_signature.is_empty(), cx);
                }
            } else if !current.is_empty() {
                cx.update_global::<ComposerDrafts, _>(|global, _| {
                    if let Some(store) = global.scopes.get_mut(&owner.scope) {
                        store.put(&owner.key, current, false);
                    }
                });
                schedule_save(!current_signature.is_empty(), cx);
            }
            request_load(&owner, true, cx);
        } else if current.text != self.draft_text.as_ref() || current_signature != self.draft_attachments {
            let immediate = !current_signature.is_empty() || current_signature != self.draft_attachments;
            let (merged, revision) = cx.update_global::<ComposerDrafts, _>(|global, _| {
                let Some(store) = global.scopes.get_mut(&owner.scope) else {
                    return (ParkedDraft::default(), 0);
                };
                let merged = model::merge_local(
                    store.get(&owner.key),
                    current,
                    &self.draft_text,
                    &self.draft_attachments,
                );
                let revision = store.put(&owner.key, merged.clone(), false);
                (merged, revision)
            });
            self.apply_draft(owner.clone(), merged, revision, window, cx);
            schedule_save(immediate, cx);
            return;
        }
        let (draft, revision) = cx
            .global::<ComposerDrafts>()
            .scopes
            .get(&owner.scope)
            .map(|store| (store.get(&owner.key), store.revision(&owner.key)))
            .unwrap_or_default();
        if changed_key || revision != self.draft_revision {
            self.apply_draft(owner.clone(), draft, revision, window, cx);
        }
        request_load(&owner, false, cx);
        let pending = cx.global::<ComposerDrafts>();
        let save = pending.saving.is_none()
            && pending.scopes.get(&owner.scope).is_some_and(|scope| {
                scope.persistent
                    && !scope.writing
                    && scope
                        .slots
                        .iter()
                        .any(|(key, slot)| !key.starts_with("draft-") && slot.dirty && slot.problem.is_none())
            });
        if save {
            schedule_save(false, cx);
        }
    }
    /// Park the previous identity before clearing the screen. A new sign-in must start with an empty owner.
    pub(super) fn leave_draft_identity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(owner) = self.draft_at.take() {
            let visible = self.capture_draft(cx);
            cx.update_global::<ComposerDrafts, _>(|global, _| {
                if let Some(store) = global.scopes.get_mut(&owner.scope) {
                    let merged = model::merge_local(
                        store.get(&owner.key),
                        visible,
                        &self.draft_text,
                        &self.draft_attachments,
                    );
                    store.put(&owner.key, merged, false);
                }
            });
            schedule_save(true, cx);
        }
        // Unknown credentials never reuse a RAM scope across sign-ins, including late preparation callbacks.
        self.draft_memory_identity = uuid::Uuid::new_v4();
        self.draft = Default::default();
        self.draft_revision = 0;
        self.draft_text = "".into();
        self.draft_attachments.clear();
        self.composer.update(cx, |state, cx| state.set_value("", window, cx));
    }
    pub(super) fn draft_problem(&self, cx: &App) -> Option<SharedString> {
        let owner = self.draft_at.as_ref()?;
        let store = cx.try_global::<ComposerDrafts>()?.scopes.get(&owner.scope)?;
        store.slots.get(&store.key(&owner.key))?.problem.clone().map(Into::into)
    }
    pub(super) fn rename_draft(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        let Some(owner) = self.draft_at.clone() else { return };
        ensure_scope(&owner, false, cx);
        cx.update_global::<ComposerDrafts, _>(|global, _| {
            if let Some(store) = global.scopes.get_mut(&owner.scope) {
                store.rename(from, to);
            }
        });
        if let Some(current) = &mut self.draft_at
            && current.scope == owner.scope
            && current.key == from
        {
            current.key = to.to_string();
            self.draft_revision = 0;
        }
        schedule_save(true, cx);
    }
    pub(super) fn merge_attachments(
        &mut self,
        owner: Option<DraftOwner>,
        prepared: attachments::Prepared,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let Some(owner) = owner else {
            return attachments::merge(&mut self.draft, prepared);
        };
        ensure_scope(&owner, false, cx);
        let resolved = cx
            .global::<ComposerDrafts>()
            .scopes
            .get(&owner.scope)
            .and_then(|store| store.resolve_ticket(&owner.key, owner.generation));
        let Some((key, generation)) = resolved else { return None };
        let mut owner = owner;
        owner.key = key;
        owner.generation = generation;
        if self
            .draft_owner(cx)
            .as_ref()
            .is_some_and(|current| current.same_key(&owner))
        {
            let problem = attachments::merge(&mut self.draft, prepared);
            let current = self.capture_draft(cx);
            cx.update_global::<ComposerDrafts, _>(|global, _| {
                if let Some(store) = global.scopes.get_mut(&owner.scope) {
                    let merged = model::merge_local(
                        store.get(&owner.key),
                        current,
                        &self.draft_text,
                        &self.draft_attachments,
                    );
                    store.put(&owner.key, merged, false);
                }
            });
            // The next render applies the merged snapshot, including edits from another window.
            self.draft_text = self.composer.read(cx).value();
            self.draft_attachments = signature(&self.draft);
            schedule_save(true, cx);
            return problem;
        }
        cx.update_global::<ComposerDrafts, _>(|global, _| {
            if let Some(store) = global.scopes.get_mut(&owner.scope) {
                let mut parked = store.get(&owner.key);
                let mut draft = attachments::Draft {
                    images: parked.images,
                    files: parked.files,
                    ..Default::default()
                };
                let _ = attachments::merge(&mut draft, prepared);
                parked.images = draft.images;
                parked.files = draft.files;
                store.put(&owner.key, parked, false);
            }
        });
        schedule_save(true, cx);
        None
    }
    /// The first Gateway answer names its chat. Later edits and preparation follow it, while Home stays reusable.
    pub(super) fn promote_chat_draft(&mut self, previous: Option<DraftOwner>, cx: &mut Context<Self>) {
        let (Some(previous), Some(next)) = (previous, self.draft_owner(cx)) else {
            return;
        };
        if previous.scope != next.scope || previous.key == next.key {
            return;
        }
        ensure_scope(&previous, false, cx);
        cx.update_global::<ComposerDrafts, _>(|global, _| {
            if let Some(store) = global.scopes.get_mut(&previous.scope) {
                store.rename(&previous.key, &next.key);
            }
        });
        if self.draft_at.as_ref().is_some_and(|owner| owner.same_key(&previous)) {
            self.draft_at = Some(next);
            self.draft_revision = 0;
        }
        schedule_save(true, cx);
    }

    pub(super) fn draft_submission(&self, cx: &App) -> Option<Submission> {
        Some(Submission {
            owner: self.draft_owner(cx)?,
            draft: self.capture_draft(cx),
            revision: self.draft_revision,
            recovered: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
    /// Return a refused message to its original draft, even when another place is open.
    pub(super) fn restore_submitted_draft(
        &mut self,
        submitted: Submission,
        problem: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if submitted.recovered.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        ensure_scope(&submitted.owner, false, cx);
        let restored = cx.update_global::<ComposerDrafts, _>(|global, _| {
            let store = global.scopes.get_mut(&submitted.owner.scope)?;
            let mut current = store.get(&submitted.owner.key);
            let newer = store.revision(&submitted.owner.key) > submitted.revision;
            if current.text.is_empty() {
                current.text = submitted.draft.text.clone();
            } else if current.text != submitted.draft.text && !submitted.draft.text.is_empty() {
                current.text = format!("{}\n\n{}", current.text, submitted.draft.text);
            }
            for (kept, items) in [
                (&mut current.images, &submitted.draft.images),
                (&mut current.files, &submitted.draft.files),
            ] {
                for item in items {
                    if !kept.iter().any(|kept| kept.id == item.id) {
                        kept.push(item.clone());
                    }
                }
            }
            if let Some(problem) = problem
                && (!newer || current.problem.is_none())
            {
                current.problem = Some(problem.to_string());
            }
            let revision = store.put(&submitted.owner.key, current.clone(), false);
            let mut owner = submitted.owner.clone();
            owner.generation = store.generation(&owner.key);
            Some((owner, current, revision))
        });
        // The recovery key is frozen. A current account check controls presentation only, never its destination.
        if self
            .draft_owner(cx)
            .as_ref()
            .is_some_and(|current| current.same_key(&submitted.owner))
        {
            if let Some((owner, draft, revision)) = restored {
                self.apply_draft(owner, draft, revision, window, cx);
            }
        }
        schedule_save(true, cx);
        cx.notify();
    }

    pub(super) fn draft_submitted(
        &mut self,
        submission: Option<Submission>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(submitted) = submission else { return };
        ensure_scope(&submitted.owner, false, cx);
        let next = cx.update_global::<ComposerDrafts, _>(|global, _| {
            let store = global.scopes.get_mut(&submitted.owner.scope)?;
            let left = model::after_send(store.get(&submitted.owner.key), &submitted.draft);
            let revision = store.put(&submitted.owner.key, left.clone(), true);
            Some((left, revision))
        });
        if self
            .draft_owner(cx)
            .as_ref()
            .is_some_and(|current| current.same_key(&submitted.owner))
            && let Some((left, revision)) = next
        {
            let mut owner = submitted.owner;
            owner.generation = cx
                .global::<ComposerDrafts>()
                .scopes
                .get(&owner.scope)
                .map_or(0, |s| s.generation(&owner.key));
            self.apply_draft(owner, left, revision, window, cx);
        }
        schedule_save(true, cx);
    }
}
