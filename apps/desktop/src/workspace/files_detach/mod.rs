//! Local Code Files owners retain plain snapshots across replaceable native windows.
use super::{Workspace, runs::Source};
use crate::{
    code::CodeStore,
    side::files::{Files, FilesGuard, FilesState},
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
    /// Only a verified concrete local reader can name this resource. A pending/remote session has no fallback.
    pub(in crate::workspace) fn files_key(&self, session: &str) -> Option<Key> {
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
    fn files_source(
        &self,
        session: &str,
    ) -> Result<
        (
            Key,
            crate::code::SessionFolder,
            FilesGuard,
            Option<super::runs::Runs>,
            String,
        ),
        String,
    > {
        let key = self
            .files_key(session)
            .ok_or("This local session source is not ready.")?;
        #[cfg(test)]
        let runs = self.files_fixture_runs.clone().or_else(|| self.runs(session));
        #[cfg(not(test))]
        let runs = self.runs(session);
        if runs.as_ref().is_some_and(|runs| runs.source() != key.source) {
            return Err("This session's original local source is unavailable.".into());
        }
        let Source::Local { data, scope } = &key.source else {
            return Err("Files detachment is local only.".into());
        };
        let reader =
            CodeStore::open_reading_in(data, scope.clone())?.ok_or("The original source reader is unavailable.")?;
        let summary = reader.summary(session)?.ok_or("This session is no longer available.")?;
        let folder = reader.folder(session)?;
        if folder.path != key.folder || folder.base != key.base {
            return Err("This session's original folder changed.".into());
        }
        let proof = self
            .gateway_account_authority
            .clone()
            .ok_or("The account is not verified.")?;
        // Ordinary attached reader mode remains usable while another app owns the runtime.
        // A retained native owner, below, always requires a concrete original local Runs.
        let runner = runs
            .as_ref()
            .map_or_else(|| Arc::new(|| true) as FilesGuard, |runs| runs.source_guard());
        let guard = model::source_guard(
            &key,
            &folder,
            reader,
            summary.created_at,
            summary.project_id,
            model::SourceProof {
                account: proof,
                backend: self.session_account_gateway().ok_or("The account is not verified.")?,
                runner,
                #[cfg(test)]
                after_read: None,
            },
        )?;
        if !guard() {
            return Err("This local session source is no longer available.".into());
        }
        Ok((key, folder, guard, runs, summary.title))
    }
    pub(in crate::workspace) fn files_attached_source(
        &self,
        session: &str,
    ) -> Result<(Key, crate::side::Place, FilesGuard), String> {
        let (key, folder, guard, _, _) = self.files_source(session)?;
        Ok((key, crate::side::Place::Here(Ok(folder)), guard))
    }
    pub(in crate::workspace) fn files_origin(&self, session: &str, window: WindowId) -> Result<Origin, String> {
        let (key, folder, guard, runs, title) = self.files_source(session)?;
        let runs = runs.ok_or("This local session's runner is not ready; Files can still read in this window.")?;
        // Observe the same scoped indexed session as the attached Code pane, even after its Workspace closes.
        let Source::Local { data, scope } = &key.source else {
            return Err("Files detachment is local only.".into());
        };
        let reader = std::sync::Mutex::new(
            CodeStore::open_reading_in(data, scope.clone())?.ok_or("The original source reader is unavailable.")?,
        );
        // This indexed baseline precedes fresh Files reconstruction. Any completion racing the
        // first worker observation remains above it; earlier completions are covered by the fresh read.
        if !guard() {
            return Err("The original Files source was revoked.".into());
        }
        let (baseline, completion_reader) = {
            let reader = reader.lock().map_err(|_| "The original reader lock is unavailable.")?;
            let baseline = reader
                .event_sequence(session)?
                .ok_or("The original session is unavailable.")?;
            (baseline, reader.reader())
        };
        if !guard() {
            return Err("The original Files source was revoked.".into());
        }
        let (completion_session, completion_guard) = (session.to_owned(), guard.clone());
        let completion = Arc::new(move |after| {
            if !completion_guard() {
                return None;
            }
            let page = completion_reader
                .completed_turn_page(&completion_session, after)
                .ok()??;
            completion_guard().then_some(page)
        });
        let (session, valid) = (session.to_owned(), guard.clone());
        let running = Arc::new(move || {
            if !valid() {
                return None;
            }
            let running = {
                let reader = reader.lock().ok()?;
                crate::code::Status::of(reader.summary(&session).ok()??.status) == crate::code::Status::Running
            };
            valid().then_some(running)
        });
        Ok(Origin {
            key,
            guard,
            running,
            baseline,
            completion,
            _runs: runs,
            folder,
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
                probe_completed(true, cx);
            });
        }
    });
    cx.global_mut::<Registry>().watch = Some(watch);
}
pub(super) fn current(id: u64, cx: &App) -> bool {
    cx.try_global::<Registry>()
        .and_then(|registry| registry.id(id))
        .is_some_and(Entry::current)
}
pub(super) fn origin(id: u64, cx: &App) -> Option<Origin> {
    cx.try_global::<Registry>()?
        .id(id)
        .filter(|entry| entry.current())
        .map(|entry| entry.origin.clone())
}
pub(super) fn guard(id: u64, cx: &App) -> Option<FilesGuard> {
    cx.try_global::<Registry>()?
        .id(id)
        .filter(|entry| entry.current())
        .map(Entry::guard)
}
pub(super) fn owner(key: &Key, cx: &App) -> Option<u64> {
    cx.try_global::<Registry>()?
        .key(key)
        .filter(|entry| entry.current())
        .map(|entry| entry.id)
}
pub(super) fn reserved(key: &Key, window: WindowId, cx: &App) -> bool {
    cx.try_global::<Registry>()
        .and_then(|registry| registry.key(key))
        .is_some_and(|entry| {
            if !entry.current() {
                return false;
            }
            match &entry.presentation {
                Presentation::Attached { window: current, files } => {
                    current.window_id() != window && files.upgrade().is_some_and(|files| files.read(cx).is_current())
                }
                _ => true,
            }
        })
}
pub(super) fn live_target(key: &Key, cx: &App) -> Option<(u64, gpui_kit::AnyWindowHandle)> {
    let entry = cx.try_global::<Registry>()?.key(key).filter(|entry| entry.current())?;
    match &entry.presentation {
        Presentation::Detached { window, files } if files.read(cx).is_current() => Some((entry.id, *window)),
        Presentation::Attached { window, files }
            if files.upgrade().is_some_and(|files| files.read(cx).is_current()) =>
        {
            Some((entry.id, *window))
        }
        _ => None,
    }
}
pub(super) fn target_files(owner: u64, expected: WindowId, cx: &App) -> Option<Entity<Files>> {
    let entry = cx.try_global::<Registry>()?.id(owner).filter(|entry| entry.current())?;
    let files = match &entry.presentation {
        Presentation::Detached { window, files } if window.window_id() == expected => Some(files.clone()),
        Presentation::Attached { window, files } if window.window_id() == expected => files.upgrade(),
        _ => None,
    }?;
    files.read(cx).is_current().then_some(files)
}
pub(super) fn open(owner: u64, cx: &mut App) -> bool {
    presentation::open(owner, cx)
}
pub(super) fn request_return(key: &Key, cx: &mut App) {
    returning::request(key, cx);
}
pub(super) fn moving(key: &Key, cx: &App) -> bool {
    cx.try_global::<Registry>()
        .and_then(|registry| registry.key(key))
        .is_some_and(|entry| entry.current() && !matches!(entry.presentation, Presentation::Attached { .. }))
}
pub(super) fn begin(origin: Origin, files: &Entity<Files>, state: FilesState, cx: &mut App) -> Result<u64, String> {
    init(cx);
    revoke_stale(cx);
    if !origin.current() || cx.global::<Registry>().quitting {
        return Err("The Files source was revoked.".into());
    }
    if let Some(entry) = cx
        .global_mut::<Registry>()
        .entries
        .iter_mut()
        .find(|entry| entry.origin.key == origin.key)
    {
        let same = matches!(&entry.presentation, Presentation::Attached { files: old, .. }
            if old.upgrade().is_some_and(|old| old.entity_id() == files.entity_id()));
        if !same || !entry.current() {
            return Err("This Files tool is already owned by another window.".into());
        }
        entry.origin.preferred = origin.preferred;
        entry.presentation = Presentation::Opening {
            saved: Rc::new(RefCell::new(Some(state))),
        };
        return Ok(entry.id);
    }
    let was_running = (origin.running)().unwrap_or(false);
    let scanned = origin.baseline;
    let registry = cx.global_mut::<Registry>();
    let id = registry.allocate();
    registry.entries.push(Entry {
        id,
        origin,
        live: Arc::new(AtomicBool::new(true)),
        presentation: Presentation::Opening {
            saved: Rc::new(RefCell::new(Some(state))),
        },
        problem: None,
        was_running,
        pending_refresh: false,
        scanned,
        completed: 0,
        refreshed: 0,
        probe: None,
    });
    Ok(id)
}
pub(super) fn saved(id: u64, cx: &App) -> Option<model::Saved> {
    let entry = cx.try_global::<Registry>()?.id(id).filter(|entry| entry.current())?;
    match &entry.presentation {
        Presentation::Opening { saved } | Presentation::Returning { saved } => Some(saved.clone()),
        _ => None,
    }
}
pub(super) fn take_saved(id: u64, returning_only: bool, cx: &mut App) -> Option<FilesState> {
    let entry = cx.try_global::<Registry>()?.id(id).filter(|entry| entry.current())?;
    if returning_only && !matches!(entry.presentation, Presentation::Returning { .. }) {
        return None;
    }
    let slot = saved(id, cx)?;
    let state = slot.borrow_mut().take()?;
    cx.global_mut::<Registry>().id_mut(id)?.presentation = Presentation::Opening { saved: slot };
    Some(state)
}
pub(super) fn attached(id: u64, files: &Entity<Files>, window: &Window, cx: &mut App) -> bool {
    let Some(entry) = cx.global_mut::<Registry>().id_mut(id) else {
        return false;
    };
    if !entry.current()
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
        files: files.downgrade(),
    };
    entry.problem = None;
    true
}
/// Navigation parks only owners created by an earlier detach; ordinary attached Files has no app resource owner.
pub(super) fn park(key: &Key, files: gpui_kit::EntityId, state: FilesState, cx: &mut App) {
    let Some(registry) = cx.try_global::<Registry>() else {
        return;
    };
    let id = registry.entries.iter().find_map(|entry| {
        (entry.origin.key == *key
            && entry.current()
            && matches!(&entry.presentation,
            Presentation::Attached { files: old, .. } if old.upgrade().is_some_and(|old| old.entity_id() == files)))
        .then_some(entry.id)
    });
    if let Some(id) = id {
        if let Some(entry) = cx.global_mut::<Registry>().id_mut(id) {
            entry.presentation = Presentation::Returning {
                saved: Rc::new(RefCell::new(Some(state))),
            };
        }
    }
}
pub(super) fn focus(key: &Key, cx: &mut App) {
    let window = cx
        .try_global::<Registry>()
        .and_then(|registry| registry.key(key))
        .filter(|entry| entry.current())
        .and_then(|entry| match &entry.presentation {
            Presentation::Attached { window, .. } | Presentation::Detached { window, .. } => Some(*window),
            _ => None,
        });
    if let Some(window) = window {
        let _ = window.update(cx, |_, window, _| window.activate_window());
    }
}
pub(super) fn notice(key: &Key, cx: &App) -> Option<String> {
    cx.try_global::<Registry>()?
        .key(key)
        .filter(|entry| entry.current())?
        .problem
        .clone()
}
pub(crate) fn keeps_app_alive(cx: &App) -> bool {
    cx.try_global::<Registry>().is_some_and(|registry| {
        !registry.quitting
            && (registry.opening_return > 0
                || registry
                    .entries
                    .iter()
                    .any(|entry| entry.current() && !matches!(entry.presentation, Presentation::Attached { .. })))
    })
}
fn release(entries: Vec<Entry>, cx: &mut App) {
    for entry in entries {
        match entry.presentation {
            Presentation::Detached { window, files } => {
                files.update(cx, |files, cx| files.retire(cx));
                let _ = window.update(cx, |_, window, _| window.remove_window());
            }
            Presentation::Attached { files, .. } => {
                if let Some(files) = files.upgrade() {
                    files.update(cx, |files, cx| files.retire(cx));
                }
            }
            _ => {}
        }
    }
}
pub(crate) fn identity_changed(identity: Option<&Identity>, cx: &mut App) {
    if !cx.has_global::<Registry>() {
        return;
    }
    let removed = {
        let registry = cx.global_mut::<Registry>();
        let mut removed = Vec::new();
        let mut at = 0;
        while at < registry.entries.len() {
            if !registry.quitting
                && identity.is_some_and(|identity| *identity == registry.entries[at].origin.key.identity)
                && registry.entries[at].current()
            {
                at += 1;
            } else {
                let entry = registry.entries.remove(at);
                entry.revoke();
                if let Presentation::Detached { window, .. } = &entry.presentation {
                    registry.closing.insert(window.window_id());
                }
                removed.push(entry);
            }
        }
        removed
    };
    release(removed, cx);
}
pub(crate) fn revoke_stale(cx: &mut App) {
    if !cx.has_global::<Registry>() {
        return;
    }
    let removed = {
        let registry = cx.global_mut::<Registry>();
        let mut removed = Vec::new();
        let mut at = 0;
        while at < registry.entries.len() {
            if registry.entries[at].current() {
                at += 1;
            } else {
                let entry = registry.entries.remove(at);
                entry.revoke();
                if let Presentation::Detached { window, .. } = &entry.presentation {
                    registry.closing.insert(window.window_id());
                }
                removed.push(entry);
            }
        }
        removed
    };
    release(removed, cx);
    refresh_completed(cx);
    probe_completed(false, cx);
}
/// Status edges retain attached-pane behavior for approval, error and process exit.
fn refresh_completed(cx: &mut App) {
    let registry = cx.global_mut::<Registry>();
    for entry in &mut registry.entries {
        if !entry.current() {
            continue;
        }
        if let Some(running) = (entry.origin.running)() {
            entry.pending_refresh |= entry.was_running && !running;
            entry.was_running = running;
        }
    }
}
fn refresh_ready(id: u64, cx: &mut App) {
    let files = {
        let Some(entry) = cx.global_mut::<Registry>().id_mut(id) else {
            return;
        };
        if !entry.current() || entry.was_running || (!entry.pending_refresh && entry.completed <= entry.refreshed) {
            return;
        }
        let Presentation::Detached { files, .. } = &entry.presentation else {
            return;
        };
        // Coalesce the sampled status edge and its durable checkpoint into one real refresh.
        entry.pending_refresh = false;
        entry.refreshed = entry.completed;
        files.clone()
    };
    if current(id, cx) {
        files.update(cx, |files, cx| files.refresh(cx));
    }
}
/// At most one original-reader worker probe per owner. No global cache or Workspace resolver.
fn probe_completed(periodic: bool, cx: &mut App) {
    let probes = cx
        .global::<Registry>()
        .entries
        .iter()
        .filter(|entry| entry.current() && entry.probe.is_none() && (periodic || entry.pending_refresh))
        .map(|entry| (entry.id, entry.scanned, entry.origin.completion.clone(), entry.guard()))
        .collect::<Vec<_>>();
    for (id, scanned, completion, guard) in probes {
        let task = cx.spawn(async move |cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if !guard() {
                        return None;
                    }
                    // Four bounded 256-row pages per timer probe; remaining backlog waits for the next tick.
                    const PAGE_BUDGET: usize = 4;
                    let mut result = completion(scanned)?;
                    for _ in 1..PAGE_BUDGET {
                        if !result.more {
                            break;
                        }
                        if !guard() {
                            return None;
                        }
                        let page = completion(result.through_seq)?;
                        if page.through_seq < result.through_seq {
                            result = page;
                            break;
                        }
                        result.through_seq = page.through_seq;
                        result.turn_end = result.turn_end.max(page.turn_end);
                        result.more = page.more;
                    }
                    guard().then_some(result)
                })
                .await;
            cx.update(|cx| {
                let Some(entry) = cx.global_mut::<Registry>().id_mut(id) else {
                    return;
                };
                entry.probe = None;
                if !entry.current() {
                    return;
                }
                if let Some(page) = result {
                    if page.through_seq < entry.scanned {
                        // A deliberate log truncation must not strand the incremental cursor.
                        entry.completed = 0;
                        entry.refreshed = 0;
                        entry.pending_refresh = true;
                    }
                    entry.scanned = page.through_seq;
                    if let Some(seq) = page.turn_end {
                        entry.completed = entry.completed.max(seq);
                    }
                }
                // Preserve a new checkpoint while busy; consume it only when a real refresh is scheduled.
                refresh_completed(cx);
                refresh_ready(id, cx);
            });
        });
        if let Some(entry) = cx.global_mut::<Registry>().id_mut(id) {
            entry.probe = Some(task);
        }
    }
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
