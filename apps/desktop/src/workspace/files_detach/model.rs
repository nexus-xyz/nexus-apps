//! Plain source authority and replaceable Files presentations.
use super::super::runs::{Runs, Source};
use crate::{
    backend::ExecutionAuthority,
    code::{CodeStore, SessionFolder},
    side::{
        Place,
        files::{Files, FilesGuard, FilesState},
    },
    windows::retention::Identity,
};
use gpui_kit::{AnyWindowHandle, Entity, Global, Task, WeakEntity, WindowId};
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Key {
    pub(in crate::workspace) identity: Identity,
    pub(in crate::workspace) source: Source,
    pub(in crate::workspace) session: String,
    pub(in crate::workspace) folder: PathBuf,
    pub(in crate::workspace) base: Option<String>,
}
#[derive(Clone)]
pub(in crate::workspace) struct Origin {
    pub key: Key,
    pub guard: FilesGuard,
    pub running: Arc<dyn Fn() -> Option<bool> + Send + Sync>,
    pub baseline: i64,
    pub completion: Arc<dyn Fn(i64) -> Option<nexus_workbench::store::TurnEndPage> + Send + Sync>,
    // The command/source owner keeps its original managed store and generation alive, not a Workspace.
    pub _runs: Runs,
    pub folder: SessionFolder,
    pub title: String,
    pub preferred: WindowId,
}
impl Origin {
    pub fn current(&self) -> bool {
        (self.guard)()
    }
    pub fn place(&self) -> Place {
        Place::Here(Ok(self.folder.clone()))
    }
}
pub(super) struct SourceProof {
    pub account: ExecutionAuthority,
    pub backend: crate::workspace::gateway_access::AccountGateway,
    pub runner: FilesGuard,
    #[cfg(test)]
    pub after_read: Option<Arc<dyn Fn(bool) + Send + Sync>>,
}
/// The lock protects only the original indexed row reads; it never spans file I/O, a callback or an await.
pub(super) fn source_guard(
    key: &Key,
    folder: &SessionFolder,
    reader: CodeStore,
    created: i64,
    project: String,
    proof: SourceProof,
) -> Result<FilesGuard, String> {
    let SourceProof {
        account: proof,
        backend,
        runner,
        #[cfg(test)]
        after_read,
    } = proof;
    let canonical = folder
        .path
        .canonicalize()
        .map_err(|error| format!("The original session folder is unavailable: {error}"))?;
    if !canonical.is_dir() {
        return Err("The original session folder is unavailable.".into());
    }
    let reader = Mutex::new(reader);
    let identity = key.identity.clone();
    let session = key.session.clone();
    let original = folder.clone();
    let live = AtomicBool::new(true);
    Ok(Arc::new(move || {
        if !live.load(Ordering::SeqCst) {
            return false;
        }
        let authorized = || proof.current() && backend.matches_identity(&identity) && runner() && proof.current();
        if !authorized() {
            live.store(false, Ordering::SeqCst);
            return false;
        }
        let still_original = {
            let Ok(reader) = reader.lock() else {
                live.store(false, Ordering::SeqCst);
                return false;
            };
            reader
                .summary(&session)
                .ok()
                .flatten()
                .is_some_and(|summary| summary.created_at == created && summary.project_id == project)
                && reader
                    .folder(&session)
                    .is_ok_and(|folder| folder.path == original.path && folder.base == original.base)
        };
        #[cfg(test)]
        if let Some(observed) = &after_read {
            observed(still_original);
        }
        // Release the reader before touching the filesystem, and check authority again after that work.
        let current = still_original
            && original
                .path
                .canonicalize()
                .is_ok_and(|path| path == canonical && path.is_dir())
            && authorized();
        if !current {
            live.store(false, Ordering::SeqCst);
        }
        current && live.load(Ordering::SeqCst)
    }))
}
pub(super) type Saved = Rc<RefCell<Option<FilesState>>>;
pub(super) enum Presentation {
    Attached {
        window: AnyWindowHandle,
        files: WeakEntity<Files>,
    },
    Opening {
        saved: Saved,
    },
    Detached {
        window: AnyWindowHandle,
        files: Entity<Files>,
    },
    Returning {
        saved: Saved,
    },
}
pub(super) struct Entry {
    pub id: u64,
    pub origin: Origin,
    pub live: Arc<AtomicBool>,
    pub presentation: Presentation,
    pub problem: Option<String>,
    pub was_running: bool,
    pub pending_refresh: bool,
    pub scanned: i64,
    pub completed: i64,
    pub refreshed: i64,
    pub probe: Option<Task<()>>,
}
impl Entry {
    pub fn current(&self) -> bool {
        self.live.load(Ordering::SeqCst) && self.origin.current() && self.live.load(Ordering::SeqCst)
    }
    pub fn revoke(&self) {
        self.live.store(false, Ordering::SeqCst);
    }
    pub fn guard(&self) -> FilesGuard {
        let (live, source) = (self.live.clone(), self.origin.guard.clone());
        Arc::new(move || live.load(Ordering::SeqCst) && source() && live.load(Ordering::SeqCst))
    }
}
#[derive(Default)]
pub(super) struct Registry {
    pub entries: Vec<Entry>,
    pub next: u64,
    pub quitting: bool,
    pub opening_return: usize,
    pub closing: std::collections::HashSet<WindowId>,
    pub watch: Option<Task<()>>,
    #[cfg(test)]
    pub fail_next_open: bool,
    #[cfg(test)]
    pub fail_after_construct: bool,
    #[cfg(test)]
    pub app_window_factory:
        Option<Rc<dyn Fn(&mut gpui_kit::App) -> Option<(AnyWindowHandle, Entity<super::super::Workspace>)>>>,
}
impl Global for Registry {}
impl Registry {
    pub fn key(&self, key: &Key) -> Option<&Entry> {
        self.entries.iter().find(|entry| &entry.origin.key == key)
    }
    pub fn id(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }
    pub fn id_mut(&mut self, id: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| entry.id == id)
    }
    pub fn allocate(&mut self) -> u64 {
        self.next = self.next.checked_add(1).expect("Files owner counter exhausted");
        self.next
    }
}
