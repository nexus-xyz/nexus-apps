//! One local session owner; native views are replaceable presentations.
use super::super::{
    drafts::BrowserDraft,
    runs::{Runs, Source},
};
use crate::{
    backend::ExecutionAuthority,
    side::browser::{Browser, RetainedBrowser},
    windows::retention::Identity,
};
use gpui_kit::{AnyWindowHandle, Entity, Global, WeakEntity, WindowId};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Key {
    pub(in crate::workspace) identity: Identity,
    pub(in crate::workspace) source: Source,
    pub(in crate::workspace) session: String,
    pub(in crate::workspace) tool: &'static str,
}
#[derive(Clone)]
pub(in crate::workspace) struct Origin {
    pub key: Key,
    pub proof: ExecutionAuthority,
    pub backend: crate::workspace::gateway_access::AccountGateway,
    pub runs: Runs,
    pub source_valid: crate::side::browser::agent::Guard,
    pub draft: BrowserDraft,
    pub window: WindowId,
}
impl Origin {
    pub fn current(&self) -> bool {
        self.proof.current()
            && (self.source_valid)()
            && self.backend.matches_identity(&self.key.identity)
            && self.runs.source() == self.key.source
    }
    pub fn matches(&self, origin: &crate::side::browser::agent::LocalOrigin, session: &str) -> bool {
        self.key.session == session
            && origin.machine.is_none()
            && matches!(&self.key.source,
            Source::Local { data, scope } if *data == origin.data && *scope == origin.scope)
    }
}
pub(super) type Saved = Rc<RefCell<Option<RetainedBrowser>>>;
pub(super) enum Presentation {
    Attached {
        window: AnyWindowHandle,
        browser: WeakEntity<Browser>,
    },
    Parked,
    Opening {
        saved: Saved,
    },
    Detached {
        window: AnyWindowHandle,
        browser: Entity<Browser>,
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
    pub pending: std::collections::VecDeque<super::outputs::Receipt>,
    pub pending_bytes: usize,
    pub preparing: usize,
    pub loading_output: bool,
    pub problem: Option<String>,
}
impl Entry {
    pub fn current(&self) -> bool {
        self.live.load(Ordering::SeqCst) && self.origin.current()
    }
    pub fn revoke(&self) {
        self.live.store(false, Ordering::SeqCst);
    }
}
#[derive(Default)]
pub(super) struct Registry {
    pub next: u64,
    pub entries: Vec<Entry>,
    pub quitting: bool,
    pub opening_return: usize,
    pub closing: std::collections::HashSet<WindowId>,
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
    pub fn key_mut(&mut self, key: &Key) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| &entry.origin.key == key)
    }
    pub fn id(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }
    pub fn id_mut(&mut self, id: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| entry.id == id)
    }
    pub fn allocate(&mut self) -> u64 {
        self.next = self.next.checked_add(1).expect("browser owner counter exhausted");
        self.next
    }
}
