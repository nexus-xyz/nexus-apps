//! Plain exact-source return tickets; explicit user navigation may cancel their route, not their snapshot.
use super::{
    Key, Workspace, current,
    model::{Presentation, Registry},
};
use gpui_kit::{AnyWindowHandle, App, Context, Entity, Window, WindowId};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone)]
pub(in crate::workspace) struct Ticket {
    pub owner: u64,
    pub key: Key,
}
fn target(owner: u64, cx: &mut App) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
    let origin = super::origin(owner, cx)?;
    if cx.global::<Registry>().quitting {
        return None;
    }
    cx.global_mut::<Registry>().opening_return += 1;
    let opened = crate::windows::files_return_window(&origin.key, origin.preferred, cx);
    let pending = cx.global::<Registry>().opening_return.saturating_sub(1);
    cx.global_mut::<Registry>().opening_return = pending;
    let (window, workspace, created) = opened?;
    if !current(owner, cx) || cx.global::<Registry>().quitting {
        if created {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
        return None;
    }
    Some((window, workspace))
}
fn queue(owner: u64, window: AnyWindowHandle, workspace: Entity<Workspace>, cx: &mut App) {
    let Some(key) = super::origin(owner, cx).map(|origin| origin.key) else {
        return;
    };
    let _ = window.update(cx, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            if !workspace.navigation_allowed() || !current(owner, cx) {
                return;
            }
            // Keep an independent Browser ticket: a fresh source may be waiting for both tools.
            workspace.navigation_explicit = true;
            workspace.files_return = Some(Ticket { owner, key });
            window.activate_window();
            cx.notify();
        })
    });
}
pub(super) fn return_from(owner: u64, expected: WindowId, native_closed: bool, cx: &mut App) {
    let files = cx
        .try_global::<Registry>()
        .and_then(|registry| registry.id(owner))
        .and_then(|entry| {
            if !entry.current() || cx.global::<Registry>().quitting {
                return None;
            }
            match &entry.presentation {
                Presentation::Detached { window, files } if window.window_id() == expected => Some(files.clone()),
                _ => None,
            }
        });
    let Some(files) = files else { return };
    let destination = target(owner, cx);
    if destination.is_none() && !native_closed {
        if let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) {
            entry.problem =
                Some("Couldn't open the session window. Your Files view is still here; try Return again.".into());
        }
        cx.refresh_windows();
        return;
    }
    if !current(owner, cx) || cx.global::<Registry>().quitting {
        return;
    }
    let state = files.update(cx, |files, cx| files.retain(cx));
    let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) else {
        return;
    };
    if !entry.current() {
        return;
    }
    let old = match &entry.presentation {
        Presentation::Detached { window, .. } if window.window_id() == expected => *window,
        _ => return,
    };
    entry.presentation = Presentation::Returning {
        saved: Rc::new(RefCell::new(Some(state))),
    };
    entry.problem = None;
    if !native_closed {
        cx.global_mut::<Registry>().closing.insert(expected);
        let _ = old.update(cx, |_, window, _| window.remove_window());
    }
    if let Some((window, workspace)) = destination {
        queue(owner, window, workspace, cx);
    } else if !super::presentation::open(owner, cx) {
        if let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) {
            entry.problem =
                Some("Files is kept for its original session. Reopen Nexus and that session to return it.".into());
        }
    }
}
pub(crate) fn closed(id: WindowId, cx: &mut App) -> bool {
    if !cx.has_global::<Registry>() {
        return false;
    }
    if cx.global_mut::<Registry>().closing.remove(&id) {
        return true;
    }
    let owner = cx
        .global::<Registry>()
        .entries
        .iter()
        .find_map(|entry| match &entry.presentation {
            Presentation::Detached { window, .. } if window.window_id() == id => Some(entry.id),
            _ => None,
        });
    let Some(owner) = owner else { return false };
    if current(owner, cx) && !cx.global::<Registry>().quitting {
        return_from(owner, id, true, cx);
    } else if let Some(at) = cx
        .global::<Registry>()
        .entries
        .iter()
        .position(|entry| entry.id == owner)
    {
        let entry = cx.global_mut::<Registry>().entries.remove(at);
        entry.revoke();
        if let Presentation::Detached { files, .. } = entry.presentation {
            files.update(cx, |files, cx| files.retire(cx));
        }
    }
    true
}
pub(crate) fn source_closed(id: WindowId, cx: &mut App) {
    if !cx.has_global::<Registry>() {
        return;
    }
    let removed = {
        let registry = cx.global_mut::<Registry>();
        let mut removed = Vec::new();
        let mut at = 0;
        while at < registry.entries.len() {
            if matches!(&registry.entries[at].presentation, Presentation::Attached { window, .. } if window.window_id() == id)
            {
                let entry = registry.entries.remove(at);
                entry.revoke();
                removed.push(entry);
            } else {
                at += 1;
            }
        }
        removed
    };
    for entry in removed {
        if let Presentation::Attached { files, .. } = entry.presentation {
            if let Some(files) = files.upgrade() {
                files.update(cx, |files, cx| files.retire(cx));
            }
        }
    }
}
pub(in crate::workspace) fn request(key: &Key, cx: &mut App) {
    let current = cx
        .try_global::<Registry>()
        .and_then(|registry| registry.key(key))
        .filter(|entry| entry.current())
        .and_then(|entry| match &entry.presentation {
            Presentation::Detached { window, .. } => Some((entry.id, window.window_id())),
            _ => None,
        });
    if let Some((owner, expected)) = current {
        cx.defer(move |cx| return_from(owner, expected, false, cx));
    }
}
impl Workspace {
    pub(in crate::workspace) fn sync_files_return(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ticket) = self.files_return.clone() else {
            return;
        };
        if !current(ticket.owner, cx) {
            self.files_return = None;
            return;
        }
        if self.files_key(&ticket.key.session).as_ref() != Some(&ticket.key) {
            return;
        }
        self.sync_draft(window, cx);
        self.files_return = None;
        self.navigation_restoring = true;
        self.open_code(&ticket.key.session, window, cx);
        self.navigation_restoring = false;
        if !matches!(&self.view, super::super::View::Code(view) if view.id == ticket.key.session) {
            return;
        }
        self.show_returned_files(&ticket.key, window, cx);
        cx.notify();
    }
}
