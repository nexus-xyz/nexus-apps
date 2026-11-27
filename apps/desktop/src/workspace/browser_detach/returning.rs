//! Return tickets carry source intent, never the original Workspace.
use super::{
    Key, alive, init,
    model::{Presentation, Registry},
    presentation,
};
use crate::workspace::{View, Workspace};
use gpui_kit::{AnyWindowHandle, App, Context, Entity, Window, WindowId};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone)]
pub(in crate::workspace) struct Ticket {
    pub owner: u64,
    pub key: Key,
}
fn target(owner: u64, cx: &mut App) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
    let origin = cx.try_global::<Registry>()?.id(owner)?.origin.clone();
    if !alive(owner, cx) || cx.global::<Registry>().quitting {
        return None;
    }
    cx.global_mut::<Registry>().opening_return += 1;
    let opened = crate::windows::browser_return_window(&origin.key, origin.window, cx);
    let remaining = cx.global::<Registry>().opening_return.saturating_sub(1);
    cx.global_mut::<Registry>().opening_return = remaining;
    let (window, workspace, created) = opened?;
    if !alive(owner, cx) || cx.global::<Registry>().quitting {
        if created {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
        return None;
    }
    Some((window, workspace))
}
fn queue(owner: u64, window: AnyWindowHandle, workspace: Entity<Workspace>, cx: &mut App) {
    let Some(key) = cx
        .global::<Registry>()
        .id(owner)
        .filter(|entry| entry.current())
        .map(|entry| entry.origin.key.clone())
    else {
        return;
    };
    let _ = window.update(cx, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            if !workspace.navigation_allowed() || !alive(owner, cx) {
                return;
            }
            workspace.navigation_explicit = true;
            workspace.browser_return = Some(Ticket { owner, key });
            window.activate_window();
            cx.notify();
        });
    });
}
/// Real native close and Return share this path. The expected window rejects stale button callbacks.
pub(super) fn return_from(owner: u64, expected: WindowId, native_closed: bool, cx: &mut App) {
    init(cx);
    let browser = cx.global::<Registry>().id(owner).and_then(|entry| {
        if !entry.current() || cx.global::<Registry>().quitting {
            return None;
        }
        match &entry.presentation {
            Presentation::Detached { window, browser } if window.window_id() == expected => Some(browser.clone()),
            _ => None,
        }
    });
    let Some(browser) = browser else { return };
    let destination = target(owner, cx);
    if destination.is_none() && !native_closed {
        if let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) {
            entry.problem =
                Some("Couldn't open the session window. Your Browser is still here; try Return again.".into());
        }
        cx.refresh_windows();
        return;
    }
    // Authority can change on a transport thread while a native window is being created.
    if !alive(owner, cx) || cx.global::<Registry>().quitting {
        return;
    }
    let retained = browser.update(cx, |browser, cx| browser.retain(cx));
    let slot = Rc::new(RefCell::new(Some(retained)));
    let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) else {
        return;
    };
    if !entry.current() {
        return;
    }
    let old_window = match &entry.presentation {
        Presentation::Detached { window, .. } if window.window_id() == expected => *window,
        _ => return,
    };
    entry.presentation = Presentation::Returning { saved: slot };
    if entry.pending.is_empty() {
        entry.problem = None;
    }
    if !native_closed {
        cx.global_mut::<Registry>().closing.insert(expected);
        let _ = old_window.update(cx, |_, window, _| window.remove_window());
    }
    if let Some((window, workspace)) = destination {
        queue(owner, window, workspace, cx);
    } else {
        // Native close already removed its presentation. Make one bounded rollback attempt using fresh entities.
        if !presentation::open(owner, cx) {
            if let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) {
                entry.problem = Some(
                    "The Browser is kept for its original session. Reopen Nexus and that session to return it.".into(),
                );
            }
            // QuitMode::Explicit + Returning ownership keeps the pages alive if both native opens failed.
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
    if alive(owner, cx) && !cx.global::<Registry>().quitting {
        return_from(owner, id, true, cx);
    } else if let Some(at) = cx
        .global::<Registry>()
        .entries
        .iter()
        .position(|entry| entry.id == owner)
    {
        let entry = cx.global_mut::<Registry>().entries.remove(at);
        entry.revoke();
        if let Presentation::Detached { browser, .. } = entry.presentation {
            browser.update(cx, |browser, cx| drop(browser.retain(cx)));
        }
    }
    true
}
/// A non-retained source view cannot leave a current stale Browser entity behind.
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
        if let Presentation::Attached { browser, .. } = entry.presentation {
            if let Some(browser) = browser.upgrade() {
                browser.update(cx, |browser, cx| drop(browser.retain(cx)));
            }
        }
    }
}
impl Workspace {
    pub(in crate::workspace) fn sync_browser_return(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ticket) = self.browser_return.clone() else {
            return;
        };
        if !alive(ticket.owner, cx) {
            self.browser_return = None;
            return;
        }
        let Some(key) = self.browser_detach_key(&ticket.key.session) else {
            return;
        };
        if key != ticket.key {
            return;
        } // Never resolve the session through a different local store or remote runner.
        self.sync_draft(window, cx); // Keep newer input in the view being left.
        self.browser_return = None;
        self.navigation_restoring = true;
        self.open_code(&ticket.key.session, window, cx);
        self.navigation_restoring = false;
        if !matches!(&self.view, View::Code(view) if view.id == ticket.key.session) {
            return;
        }
        self.show_returned_browser(&ticket.key, window, cx);
        cx.notify();
    }
}
