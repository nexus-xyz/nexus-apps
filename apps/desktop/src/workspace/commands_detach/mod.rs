//! Existing local Code Commands retain their original owner across fresh native presentations.
use super::{Workspace, runs::Source};
use crate::{
    side::shared_terminals::{Guard, RetainedSharedTerminals, SharedTerminals},
    windows::retention::Identity,
};
use gpui_kit::{App, Entity, Window, WindowId};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
mod model;
mod presentation;
mod returning;
#[cfg(test)]
mod tests;
pub(crate) use model::Key;
pub(in crate::workspace) use model::Origin;
use model::{Entry, Presentation, Registry};
pub(in crate::workspace) use returning::Ticket;
pub(crate) use returning::{closed, source_closed};
impl Workspace {
    /// A pending ticket is a locator only; the original accepted reader must be present.
    pub(in crate::workspace) fn commands_key(&self, session: &str) -> Option<Key> {
        if !self.navigation_allowed()
            || self
                .gateway_account_authority
                .as_ref()
                .is_none_or(|proof| !proof.current())
            || self.machine_source.machine_of(session).is_some()
            || crate::machines::get().machine_of(session).is_some()
            || self.cloud_machine_of_session(session).is_some()
        {
            return None;
        }
        if self.session_account_gateway().is_none() {
            return None;
        }
        let identity = self.terminal_identity.as_ref()?;
        let account = self.account.as_ref()?;
        if self.current_identity(account).as_ref() != Some(identity) {
            return None;
        }
        let code = self.code.as_ref()?;
        if code.is_local_session(session) {
            return None;
        }
        let (data, scope) = code.store_source();
        if scope != format!("{}|{}", self.gateway.url(), account.org) {
            return None;
        }
        code.summary(session).ok()??;
        let folder = code.folder(session).ok()?;
        Some(Key {
            identity: identity.clone(),
            source: Source::Local {
                data: data.to_path_buf(),
                scope: scope.into(),
            },
            session: session.into(),
            folder: folder.path,
            base: folder.base,
        })
    }
    pub(in crate::workspace) fn commands_attached_source(
        &self,
        session: &str,
        window: WindowId,
    ) -> Result<(Key, Rc<crate::code_run::CodeRunner>, Guard), String> {
        let origin = self.commands_origin(session, window)?;
        Ok((origin.key, origin.runner, origin.guard))
    }
    pub(in crate::workspace) fn commands_origin(&self, session: &str, window: WindowId) -> Result<Origin, String> {
        let key = self
            .commands_key(session)
            .ok_or("This local Commands source is not ready.")?;
        let runner = self.runner.clone().ok_or("The original command owner is not ready.")?;
        if !runner.commands_in_process() {
            return Err("Only in-process local Commands can move to a window.".into());
        }
        let (data, scope) = runner.store_source();
        if key.source != (Source::Local { data, scope }) {
            return Err("This session's original command owner is unavailable.".into());
        }
        let code = self.code.as_ref().ok_or("The original local reader is unavailable.")?;
        let summary = code.summary(session)?.ok_or("The original session is unavailable.")?;
        let folder = code.folder(session)?;
        let proof = self
            .gateway_account_authority
            .clone()
            .ok_or("The account is not verified.")?;
        let guard = model::source_guard(
            &key,
            &folder,
            code.reader(),
            summary.created_at,
            summary.project_id,
            model::SourceProof {
                account: proof,
                backend: self.session_account_gateway().ok_or("The account is not verified.")?,
                runner: runner.source_guard(),
                #[cfg(test)]
                after_read: None,
            },
        )?;
        if !guard() {
            return Err("The original Commands source was revoked.".into());
        }
        let live = Arc::new(AtomicBool::new(true));
        let fence = live.clone();
        let source = guard;
        let guard: Guard = Arc::new(move || {
            fence.load(std::sync::atomic::Ordering::SeqCst)
                && source()
                && fence.load(std::sync::atomic::Ordering::SeqCst)
        });
        let project = folder
            .path
            .file_name()
            .unwrap_or(folder.path.as_os_str())
            .to_string_lossy();
        let title = format!("{project} · {}", summary.title);
        Ok(Origin {
            key,
            guard,
            live,
            runner,
            title,
            preferred: window,
        })
    }
}
fn init(cx: &mut App) {
    if cx.has_global::<Registry>() {
        return;
    }
    cx.set_global(Registry::default());
    // Auth broadcasts are immediate, while an external source-store deletion also needs to retire idle previews.
    let watch = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            cx.update(|cx| {
                revoke_stale(cx);
            });
        }
    });
    cx.global_mut::<Registry>().watch = Some(watch);
}
pub(super) fn current(id: u64, cx: &App) -> bool {
    let guard = cx
        .try_global::<Registry>()
        .and_then(|registry| registry.id(id))
        .map(Entry::guard);
    guard.is_some_and(|guard| guard())
        && cx
            .try_global::<Registry>()
            .and_then(|registry| registry.id(id))
            .is_some_and(Entry::live)
}
pub(super) fn origin(id: u64, cx: &App) -> Option<Origin> {
    if !current(id, cx) {
        return None;
    }
    cx.try_global::<Registry>()?.id(id).map(|entry| entry.origin.clone())
}
pub(super) fn guard(id: u64, cx: &App) -> Option<Guard> {
    if !current(id, cx) {
        return None;
    }
    cx.try_global::<Registry>()?.id(id).map(Entry::guard)
}
pub(super) fn owner(key: &Key, cx: &App) -> Option<u64> {
    let id = cx.try_global::<Registry>()?.key(key)?.id;
    current(id, cx).then_some(id)
}
fn live(id: u64, cx: &App) -> bool {
    cx.try_global::<Registry>()
        .is_some_and(|registry| !registry.quitting && registry.id(id).is_some_and(Entry::live))
}
fn presentation(id: u64, cx: &App) -> Option<Presentation> {
    cx.try_global::<Registry>()?
        .id(id)
        .map(|entry| entry.presentation.clone())
}
pub(super) fn reserved(key: &Key, window: WindowId, cx: &App) -> bool {
    let Some(id) = owner(key, cx) else {
        return false;
    };
    let Some(presentation) = presentation(id, cx) else {
        return false;
    };
    let reserved = match presentation {
        Presentation::Attached {
            window: target,
            commands,
        } => {
            target.window_id() != window
                && commands
                    .upgrade()
                    .is_some_and(|commands| commands.read(cx).is_current())
        }
        _ => true,
    };
    reserved && live(id, cx)
}
pub(super) fn live_target(key: &Key, cx: &App) -> Option<(u64, gpui_kit::AnyWindowHandle)> {
    let id = owner(key, cx)?;
    let (window, commands) = match presentation(id, cx)? {
        Presentation::Attached { window, commands } => (window, commands.upgrade()?),
        Presentation::Detached { window, commands } => (window, commands),
        _ => return None,
    };
    (commands.read(cx).is_current() && live(id, cx)).then_some((id, window))
}
pub(super) fn target_commands(owner: u64, expected: WindowId, cx: &App) -> Option<Entity<SharedTerminals>> {
    if !current(owner, cx) {
        return None;
    }
    let (window, commands) = match presentation(owner, cx)? {
        Presentation::Attached { window, commands } => (window, commands.upgrade()?),
        Presentation::Detached { window, commands } => (window, commands),
        _ => return None,
    };
    (window.window_id() == expected && commands.read(cx).is_current() && live(owner, cx)).then_some(commands)
}
pub(super) fn open(owner: u64, cx: &mut App) -> bool {
    presentation::open(owner, cx)
}
pub(super) fn request_return(key: &Key, cx: &mut App) {
    returning::request(key, cx);
}
pub(super) fn moving(key: &Key, cx: &App) -> bool {
    let Some(id) = owner(key, cx) else {
        return false;
    };
    presentation(id, cx).is_some_and(|presentation| !matches!(presentation, Presentation::Attached { .. }))
        && live(id, cx)
}
pub(super) fn begin(
    origin: Origin,
    commands: &Entity<SharedTerminals>,
    state: RetainedSharedTerminals,
    cx: &mut App,
) -> Result<u64, String> {
    init(cx);
    revoke_stale(cx);
    if !origin.current() || cx.global::<Registry>().quitting {
        return Err("The Commands source was revoked.".into());
    }
    if let Some(entry) = cx
        .global_mut::<Registry>()
        .entries
        .iter_mut()
        .find(|entry| entry.origin.key == origin.key)
    {
        let same = matches!(&entry.presentation, Presentation::Attached { commands: old, .. }
            if old.upgrade().is_some_and(|old| old.entity_id() == commands.entity_id()));
        if !same || !entry.live() {
            return Err("This Commands tool is already owned by another window.".into());
        }
        entry.origin.preferred = origin.preferred;
        entry.presentation = Presentation::Opening {
            saved: Rc::new(RefCell::new(Some(state))),
        };
        return Ok(entry.id);
    }
    let registry = cx.global_mut::<Registry>();
    let id = registry.allocate();
    let live = origin.live.clone();
    registry.entries.push(Entry {
        id,
        origin,
        live,
        presentation: Presentation::Opening {
            saved: Rc::new(RefCell::new(Some(state))),
        },
        problem: None,
    });
    Ok(id)
}
pub(super) fn saved(id: u64, cx: &App) -> Option<model::Saved> {
    if !current(id, cx) {
        return None;
    }
    match presentation(id, cx)? {
        Presentation::Opening { saved } | Presentation::Returning { saved } => Some(saved),
        _ => None,
    }
}
pub(super) fn take_saved(id: u64, returning_only: bool, cx: &mut App) -> Option<RetainedSharedTerminals> {
    if !current(id, cx) {
        return None;
    }
    let presentation = presentation(id, cx)?;
    if returning_only && !matches!(presentation, Presentation::Returning { .. }) {
        return None;
    }
    let slot = match presentation {
        Presentation::Opening { saved } | Presentation::Returning { saved } => saved,
        _ => return None,
    };
    if !live(id, cx) {
        return None;
    }
    let state = slot.borrow_mut().take()?;
    cx.global_mut::<Registry>().id_mut(id)?.presentation = Presentation::Opening { saved: slot };
    Some(state)
}
pub(super) fn attached(id: u64, commands: &Entity<SharedTerminals>, window: &Window, cx: &mut App) -> bool {
    if !current(id, cx) {
        return false;
    }
    let Some(entry) = cx.global_mut::<Registry>().id_mut(id) else {
        return false;
    };
    if !entry.live()
        || !matches!(
            entry.presentation,
            Presentation::Opening { .. } | Presentation::Returning { .. }
        )
    {
        return false;
    }
    entry.origin.preferred = window.window_handle().window_id();
    entry.presentation = Presentation::Attached {
        window: window.window_handle(),
        commands: commands.downgrade(),
    };
    entry.problem = None;
    true
}
/// Navigation parks only owners created by an earlier detach; ordinary attached SharedTerminals has no app resource owner.
pub(super) fn park(key: &Key, commands: gpui_kit::EntityId, state: RetainedSharedTerminals, cx: &mut App) {
    let Some(id) = owner(key, cx) else {
        return;
    };
    let same = matches!(presentation(id, cx), Some(Presentation::Attached { commands: old, .. }) if old.upgrade().is_some_and(|old| old.entity_id() == commands));
    if !same || !live(id, cx) {
        return;
    }
    if let Some(entry) = cx.global_mut::<Registry>().id_mut(id) {
        entry.presentation = Presentation::Returning {
            saved: Rc::new(RefCell::new(Some(state))),
        };
    }
}
pub(super) fn focus(key: &Key, cx: &mut App) {
    let Some(id) = owner(key, cx) else {
        return;
    };
    let window = match presentation(id, cx) {
        Some(Presentation::Attached { window, .. } | Presentation::Detached { window, .. }) => window,
        _ => return,
    };
    if live(id, cx) {
        let _ = window.update(cx, |_, window, _| window.activate_window());
    }
}
pub(super) fn notice(key: &Key, cx: &App) -> Option<String> {
    let id = owner(key, cx)?;
    cx.try_global::<Registry>()?.id(id)?.problem.clone()
}
pub(crate) fn keeps_app_alive(cx: &App) -> bool {
    let Some(registry) = cx.try_global::<Registry>() else {
        return false;
    };
    if registry.quitting {
        return false;
    }
    if registry.opening_return > 0 {
        return true;
    }
    let ids = registry
        .entries
        .iter()
        .filter(|entry| !matches!(entry.presentation, Presentation::Attached { .. }))
        .map(|entry| entry.id)
        .collect::<Vec<_>>();
    ids.into_iter().any(|id| current(id, cx))
}
fn release(entries: Vec<Entry>, cx: &mut App) {
    for entry in entries {
        match entry.presentation {
            Presentation::Detached { window, commands } => {
                commands.update(cx, |commands, cx| commands.retire(cx));
                let _ = window.update(cx, |_, window, _| window.remove_window());
            }
            Presentation::Attached { commands, .. } => {
                if let Some(commands) = commands.upgrade() {
                    commands.update(cx, |commands, cx| commands.retire(cx));
                }
            }
            _ => {}
        }
    }
}
/// Evaluate the frozen source callbacks before taking the registry's mutable borrow.
pub(crate) fn identity_changed(identity: Option<&Identity>, cx: &mut App) {
    let Some(registry) = cx.try_global::<Registry>() else {
        return;
    };
    let quitting = registry.quitting;
    let candidates = registry
        .entries
        .iter()
        .map(|entry| (entry.id, entry.origin.key.identity.clone(), entry.guard()))
        .collect::<Vec<_>>();
    let retired = candidates
        .into_iter()
        .filter_map(|(id, accepted, guard)| (quitting || identity != Some(&accepted) || !guard()).then_some(id))
        .collect::<Vec<_>>();
    remove_retired(&retired, cx);
}
pub(crate) fn revoke_stale(cx: &mut App) {
    let Some(registry) = cx.try_global::<Registry>() else {
        return;
    };
    let candidates = registry
        .entries
        .iter()
        .map(|entry| (entry.id, entry.guard()))
        .collect::<Vec<_>>();
    let retired = candidates
        .into_iter()
        .filter_map(|(id, guard)| (!guard()).then_some(id))
        .collect::<Vec<_>>();
    remove_retired(&retired, cx);
}
fn remove_retired(ids: &[u64], cx: &mut App) {
    let removed = {
        let registry = cx.global_mut::<Registry>();
        let mut removed = Vec::new();
        let mut at = 0;
        while at < registry.entries.len() {
            if ids.contains(&registry.entries[at].id) {
                let entry = registry.entries.remove(at);
                entry.revoke();
                if let Presentation::Detached { window, .. } = &entry.presentation {
                    registry.closing.insert(window.window_id());
                }
                removed.push(entry);
            } else {
                at += 1;
            }
        }
        removed
    };
    release(removed, cx);
}

pub(crate) fn quit(cx: &mut App) {
    if !cx.has_global::<Registry>() {
        return;
    }
    cx.global_mut::<Registry>().quitting = true;
    cx.global_mut::<Registry>().watch = None;
    identity_changed(None, cx);
}
#[cfg(test)]
pub(crate) fn fixture_app_window(cx: &mut App) -> Option<Option<(gpui_kit::AnyWindowHandle, Entity<Workspace>)>> {
    let factory = cx.try_global::<Registry>()?.app_window_factory.clone()?;
    Some(factory(cx))
}
