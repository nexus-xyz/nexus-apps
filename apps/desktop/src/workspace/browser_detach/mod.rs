//! Source-authorized local CEF page owners, separate from native presentation lifetimes.
use super::{Workspace, runs::Source};
use crate::{
    side::browser::{Browser, RetainedBrowser},
    windows::retention::Identity,
};
use gpui_kit::{App, Entity, Window, WindowId};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
};
mod model;
mod outputs;
mod presentation;
mod returning;
mod routing;
#[cfg(test)]
mod tests;
pub(crate) use model::Key;
use model::{Entry, Origin, Presentation, Registry};
pub(crate) use outputs::draft_read_finished;
pub(in crate::workspace) use returning::Ticket;
pub(crate) use returning::{closed, source_closed};
pub(crate) use routing::admit;

fn init(cx: &mut App) {
    if !cx.has_global::<Registry>() {
        cx.set_global(Registry::default());
    }
}
impl Workspace {
    pub(super) fn browser_detach_key(&self, session: &str) -> Option<Key> {
        let code = self.code.as_ref()?;
        let (data, scope) = code.store_source();
        let origin = crate::side::browser::agent::LocalOrigin {
            data: data.to_path_buf(),
            scope: scope.into(),
            machine: None,
        };
        if !self.owns_browser_origin(&origin, session) {
            return None;
        }
        if self.session_account_gateway().is_none() {
            return None;
        }
        Some(Key {
            identity: self.terminal_identity.clone()?,
            source: Source::Local {
                data: origin.data,
                scope: origin.scope,
            },
            session: session.into(),
            tool: "browser",
        })
    }
    pub(super) fn browser_origin(&self, session: &str, window: WindowId, cx: &App) -> Option<Origin> {
        let key = self.browser_detach_key(session)?;
        #[cfg(test)]
        let runs = self.browser_detach_runs.clone().or_else(|| self.runs(session))?;
        #[cfg(not(test))]
        let runs = self.runs(session)?;
        if runs.source() != key.source {
            return None;
        }
        let origin = Origin {
            key,
            proof: self.gateway_account_authority.clone()?,
            backend: self.session_account_gateway()?,
            source_valid: runs.source_guard(),
            runs,
            draft: self.browser_draft(session, cx)?,
            window,
        };
        origin.current().then_some(origin)
    }
}
fn alive(id: u64, cx: &App) -> bool {
    cx.try_global::<Registry>()
        .and_then(|registry| registry.id(id))
        .is_some_and(Entry::current)
}
/// Refuse a duplicate presentation, including ownership temporarily parked between windows.
pub(super) fn occupied(key: &Key, window: WindowId, cx: &App) -> bool {
    let Some(entry) = cx.try_global::<Registry>().and_then(|registry| registry.key(key)) else {
        return false;
    };
    match &entry.presentation {
        Presentation::Attached { window: owner, browser } => {
            owner.window_id() != window && browser.upgrade().is_some_and(|browser| !browser.read(cx).is_retired())
        }
        _ => true,
    }
}
pub(super) fn register(
    origin: Origin,
    browser: &Entity<Browser>,
    window: &Window,
    restored: bool,
    cx: &mut App,
) -> bool {
    init(cx);
    revoke_stale(cx);
    if !origin.current() || cx.global::<Registry>().quitting {
        return false;
    }
    let existing = cx.global::<Registry>().entries.iter().find_map(|entry| {
        if entry.origin.key != origin.key || !entry.current() {
            return None;
        }
        match &entry.presentation {
            Presentation::Attached { browser: old, .. }
                if old.upgrade().is_some_and(|old| old.entity_id() == browser.entity_id()) =>
            {
                Some(entry.id)
            }
            Presentation::Parked | Presentation::Returning { .. } if restored => Some(entry.id),
            _ => None,
        }
    });
    let source = origin.source_valid.clone();
    let proof = origin.proof.clone();
    let identity = origin.key.identity.clone();
    let backend = origin.backend.clone();
    let live = existing
        .and_then(|id| cx.global::<Registry>().id(id).map(|entry| entry.live.clone()))
        .unwrap_or_else(|| Arc::new(AtomicBool::new(true)));
    let activity_live = live.clone();
    browser.update(cx, |browser, _| {
        browser.bind_activity_source(std::sync::Arc::new(move || {
            activity_live.load(std::sync::atomic::Ordering::SeqCst)
                && source()
                && proof.current()
                && backend.matches_identity(&identity)
        }));
    });
    if let Some(id) = existing {
        let entry = cx
            .global_mut::<Registry>()
            .id_mut(id)
            .expect("matched exact Browser entry");
        entry.origin = origin;
        entry.presentation = Presentation::Attached {
            window: window.window_handle(),
            browser: browser.downgrade(),
        };
        return true;
    }
    // Preserve legacy attached copies. They remain distinct exact-entity owners; detach refuses until one remains.
    // A moving, detached or parked owner cannot acquire a duplicate presentation.
    if cx
        .global::<Registry>()
        .entries
        .iter()
        .any(|entry| entry.origin.key == origin.key && !matches!(entry.presentation, Presentation::Attached { .. }))
    {
        return false;
    }
    let registry = cx.global_mut::<Registry>();
    let id = registry.allocate();
    registry.entries.push(Entry {
        id,
        origin,
        pending: Default::default(),
        pending_bytes: 0,
        preparing: 0,
        loading_output: false,
        problem: None,
        live,
        presentation: Presentation::Attached {
            window: window.window_handle(),
            browser: browser.downgrade(),
        },
    });
    true
}
pub(super) fn park(browser: &Entity<Browser>, cx: &mut App) {
    if !cx.has_global::<Registry>() {
        return;
    }
    let id = cx
        .global::<Registry>()
        .entries
        .iter()
        .find_map(|entry| match &entry.presentation {
            Presentation::Attached { browser: old, .. }
                if old.upgrade().is_some_and(|old| old.entity_id() == browser.entity_id()) =>
            {
                Some(entry.id)
            }
            _ => None,
        });
    if let Some(id) = id {
        if let Some(entry) = cx.global_mut::<Registry>().id_mut(id) {
            entry.presentation = Presentation::Parked;
        }
    }
}
fn saved(id: u64, cx: &App) -> Option<Rc<RefCell<Option<RetainedBrowser>>>> {
    let entry = cx.try_global::<Registry>()?.id(id)?;
    if !entry.current() {
        return None;
    }
    match &entry.presentation {
        Presentation::Opening { saved } | Presentation::Returning { saved } => Some(saved.clone()),
        _ => None,
    }
}
pub(crate) fn keeps_app_alive(cx: &App) -> bool {
    cx.try_global::<Registry>().is_some_and(|registry| {
        !registry.quitting
            && (registry.opening_return > 0
                || registry.entries.iter().any(|entry| {
                    entry.current()
                        && matches!(
                            entry.presentation,
                            Presentation::Detached { .. }
                                | Presentation::Opening { .. }
                                | Presentation::Returning { .. }
                        )
                }))
    })
}
pub(crate) fn identity_changed(identity: Option<&Identity>, cx: &mut App) {
    init(cx);
    let revoked = {
        let registry = cx.global_mut::<Registry>();
        let mut revoked = Vec::new();
        let mut at = 0;
        while at < registry.entries.len() {
            let entry = &registry.entries[at];
            let keep = !registry.quitting
                && identity.is_some_and(|identity| *identity == entry.origin.key.identity)
                && entry.current();
            if keep {
                at += 1;
            } else {
                let entry = registry.entries.remove(at);
                entry.revoke(); // Before any native close: callbacks cannot hand stale pages back.
                if let Presentation::Detached { window, .. } = &entry.presentation {
                    registry.closing.insert(window.window_id());
                }
                revoked.push(entry);
            }
        }
        revoked
    };
    release_revoked(revoked, cx);
}
fn release_revoked(revoked: Vec<Entry>, cx: &mut App) {
    for entry in revoked {
        match entry.presentation {
            Presentation::Detached { window, browser } => {
                browser.update(cx, |browser, cx| drop(browser.retain(cx)));
                let _ = window.update(cx, |_, window, _| window.remove_window());
            }
            Presentation::Attached { browser, .. } => {
                if let Some(browser) = browser.upgrade() {
                    browser.update(cx, |browser, cx| drop(browser.retain(cx)));
                }
            }
            // Opening/Returning own only retained pages. Dropping the entry drops that ownership.
            _ => {}
        }
    }
}
pub(crate) fn quit(cx: &mut App) {
    init(cx);
    cx.global_mut::<Registry>().quitting = true;
    identity_changed(None, cx);
}

pub(super) fn owner(key: &Key, cx: &App) -> Option<u64> {
    cx.try_global::<Registry>()?
        .key(key)
        .filter(|entry| entry.current())
        .map(|entry| entry.id)
}
pub(super) fn is_detached(key: &Key, cx: &App) -> bool {
    cx.try_global::<Registry>()
        .and_then(|registry| registry.key(key))
        .is_some_and(|entry| {
            entry.current()
                && matches!(
                    entry.presentation,
                    Presentation::Detached { .. } | Presentation::Opening { .. } | Presentation::Returning { .. }
                )
        })
}
pub(super) fn begin(key: &Key, browser: gpui_kit::EntityId, retained: RetainedBrowser, cx: &mut App) -> Option<u64> {
    let entry = cx.global_mut::<Registry>().entries.iter_mut().find(|entry| {
        entry.origin.key == *key
            && matches!(&entry.presentation, Presentation::Attached { browser: old, .. }
            if old.upgrade().is_some_and(|old| old.entity_id() == browser))
    })?;
    if !entry.current() {
        return None;
    }
    entry.presentation = Presentation::Opening {
        saved: Rc::new(RefCell::new(Some(retained))),
    };
    Some(entry.id)
}
pub(super) fn take_return(key: &Key, cx: &mut App) -> Option<(u64, RetainedBrowser)> {
    let entry = cx.try_global::<Registry>()?.key(key)?;
    if !entry.current() {
        return None;
    }
    let (id, slot) = match &entry.presentation {
        Presentation::Returning { saved } => (entry.id, saved.clone()),
        _ => return None,
    };
    let retained = slot.borrow_mut().take()?;
    cx.global_mut::<Registry>().id_mut(id)?.presentation = Presentation::Opening { saved: slot };
    Some((id, retained))
}
pub(super) fn rollback(owner: u64, cx: &mut App) -> Option<(Key, RetainedBrowser)> {
    let slot = saved(owner, cx)?;
    let retained = slot.borrow_mut().take()?;
    let entry = cx.global_mut::<Registry>().id_mut(owner)?;
    if !entry.current() {
        return None;
    }
    let key = entry.origin.key.clone();
    entry.presentation = Presentation::Returning { saved: slot };
    Some((key, retained))
}
pub(super) fn attached(key: &Key, browser: &Entity<Browser>, window: &Window, cx: &mut App) -> bool {
    let Some(entry) = cx.global_mut::<Registry>().entries.iter_mut().find(|entry| {
        entry.origin.key == *key
            && matches!(
                entry.presentation,
                Presentation::Opening { .. } | Presentation::Returning { .. } | Presentation::Parked
            )
    }) else {
        return false;
    };
    if !entry.current()
        || !matches!(
            entry.presentation,
            Presentation::Opening { .. } | Presentation::Returning { .. } | Presentation::Parked
        )
    {
        return false;
    }
    if entry.pending.is_empty() {
        entry.problem = None;
    }
    entry.origin.window = window.window_handle().window_id();
    entry.presentation = Presentation::Attached {
        window: window.window_handle(),
        browser: browser.downgrade(),
    };
    true
}
pub(super) fn event_owner(key: &Key, browser: gpui_kit::EntityId, cx: &App) -> Option<u64> {
    cx.try_global::<Registry>()?.entries.iter().find_map(|entry| {
        if entry.origin.key != *key || !entry.current() {
            return None;
        }
        match &entry.presentation {
            Presentation::Attached { browser: current, .. }
                if current.upgrade().is_some_and(|current| current.entity_id() == browser) =>
            {
                Some(entry.id)
            }
            _ => None,
        }
    })
}
pub(super) fn focus_key(key: &Key, cx: &mut App) {
    if let Some(id) = owner(key, cx) {
        presentation::focus(id, cx);
    }
}
pub(super) fn open(owner: u64, cx: &mut App) -> bool {
    presentation::open(owner, cx)
}
/// Defer source ingress because the source Workspace is still borrowed by its click listener.
pub(in crate::workspace) fn return_to_session(key: &Key, cx: &mut App) {
    let current = cx
        .try_global::<Registry>()
        .and_then(|registry| registry.key(key))
        .filter(|entry| entry.current())
        .and_then(|entry| match &entry.presentation {
            Presentation::Detached { window, .. } => Some((entry.id, window.window_id())),
            _ => None,
        });
    if let Some((owner, expected)) = current {
        cx.defer(move |cx| returning::return_from(owner, expected, false, cx));
    }
}
pub(super) fn retry(owner: u64, cx: &mut App) {
    outputs::retry(owner, cx);
}
pub(super) fn notice(key: &Key, cx: &App) -> Option<(u64, String, bool)> {
    outputs::notice(key, cx)
}
pub(super) fn route_event(
    owner: u64,
    browser: gpui_kit::EntityId,
    event: &crate::side::browser::BrowserEvent,
    cx: &mut App,
) {
    routing::event(owner, browser, event, cx);
}
pub(super) fn allow_new(key: &Key, window: WindowId, cx: &App) -> bool {
    !occupied(key, window, cx)
}

#[cfg(test)]
pub(in crate::workspace) fn fixture_return_factory(
    factory: Rc<dyn Fn(&mut App) -> Option<(gpui_kit::AnyWindowHandle, Entity<Workspace>)>>,
    cx: &mut App,
) {
    init(cx);
    cx.global_mut::<Registry>().app_window_factory = Some(factory);
}

#[cfg(test)]
pub(crate) fn fixture_app_window(cx: &mut App) -> Option<Option<(gpui_kit::AnyWindowHandle, Entity<Workspace>)>> {
    let factory = cx.try_global::<Registry>()?.app_window_factory.clone()?;
    Some(factory(cx))
}
pub(crate) fn revoke_stale(cx: &mut App) {
    if !cx.has_global::<Registry>() {
        return;
    }
    let revoked = {
        let registry = cx.global_mut::<Registry>();
        let mut removed = Vec::new();
        let mut at = 0;
        while at < registry.entries.len() {
            if !registry.entries[at].current() {
                let entry = registry.entries.remove(at);
                entry.revoke();
                removed.push(entry);
            } else {
                at += 1;
            }
        }
        for entry in &removed {
            if let Presentation::Detached { window, .. } = &entry.presentation {
                registry.closing.insert(window.window_id());
            }
        }
        removed
    };
    release_revoked(revoked, cx);
}
