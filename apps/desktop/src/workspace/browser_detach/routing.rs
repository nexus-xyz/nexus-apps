//! Calls and receipts stay on the frozen local session even when another window is frontmost.
use super::{
    alive,
    model::{Origin, Presentation, Registry},
};
use crate::side::browser::{
    BrowserEvent,
    agent::{Guard, LocalOrigin},
};
use gpui_kit::{App, EntityId};
use std::sync::{Arc, atomic::Ordering};

pub(crate) fn admit(
    origin: &LocalOrigin,
    session: &str,
    producer: &Guard,
    cx: &App,
) -> Option<Result<crate::windows::AdmittedBrowser, String>> {
    let registry = cx.try_global::<Registry>()?;
    let matches = |entry: &&super::model::Entry| entry.origin.matches(origin, session);
    let entry = registry
        .entries
        .iter()
        .filter(matches)
        .find(|entry| {
            entry.current()
                && match &entry.presentation {
                    Presentation::Detached { browser, .. } => !browser.read(cx).is_retired(),
                    Presentation::Attached { browser, .. } => {
                        browser.upgrade().is_some_and(|browser| !browser.read(cx).is_retired())
                    }
                    _ => false,
                }
        })
        .or_else(|| {
            registry
                .entries
                .iter()
                .find(|entry| entry.origin.matches(origin, session))
        })?;
    if registry.quitting || !entry.current() || !producer() {
        return Some(Err("This browser's source authority is no longer available.".into()));
    }
    let (window, browser) = match &entry.presentation {
        Presentation::Detached { window, browser } => (*window, browser.clone()),
        Presentation::Attached { window, browser } => match browser.upgrade() {
            Some(browser) if !browser.read(cx).is_retired() => (*window, browser),
            _ => return None,
        },
        _ => {
            return Some(Err(
                "This session browser is moving or parked. Return to its session first.".into(),
            ));
        }
    };
    let (proof, identity, backend, source, live, producer) = (
        entry.origin.proof.clone(),
        entry.origin.key.identity.clone(),
        entry.origin.backend.clone(),
        entry.origin.source_valid.clone(),
        entry.live.clone(),
        producer.clone(),
    );
    let guard: Guard = Arc::new(move || {
        live.load(Ordering::SeqCst) && proof.current() && backend.matches_identity(&identity) && source() && producer()
    });
    Some(Ok((window, browser, guard)))
}
fn continuation_guard(id: u64, cx: &App) -> Option<Guard> {
    let registry = cx.try_global::<Registry>()?;
    let entry = registry.id(id)?;
    if registry.quitting || !entry.current() {
        return None;
    }
    let (live, proof, identity, backend, source) = (
        entry.live.clone(),
        entry.origin.proof.clone(),
        entry.origin.key.identity.clone(),
        entry.origin.backend.clone(),
        entry.origin.source_valid.clone(),
    );
    Some(Arc::new(move || {
        live.load(Ordering::SeqCst) && proof.current() && backend.matches_identity(&identity) && source()
    }))
}
fn event_origin(id: u64, browser: EntityId, cx: &App) -> Option<Origin> {
    let registry = cx.try_global::<Registry>()?;
    let entry = registry.id(id)?;
    if registry.quitting || !entry.current() {
        return None;
    }
    let same = match &entry.presentation {
        Presentation::Detached { browser: current, .. } => current.entity_id() == browser,
        Presentation::Attached { browser: current, .. } => {
            current.upgrade().is_some_and(|current| current.entity_id() == browser)
        }
        _ => false,
    };
    same.then(|| entry.origin.clone())
}
pub(in crate::workspace) fn event(id: u64, browser: EntityId, event: &BrowserEvent, cx: &mut App) {
    let Some(origin) = event_origin(id, browser, cx) else {
        return;
    };
    match event {
        BrowserEvent::Text(text) => {
            if !origin.current() {
                return;
            }
            let target = origin.draft.receipt(cx);
            let text = text.clone();
            cx.defer(move |cx| {
                super::outputs::deliver(
                    id,
                    origin,
                    super::outputs::Receipt {
                        target,
                        output: super::outputs::Output::Text(text),
                    },
                    cx,
                )
            });
        }
        BrowserEvent::Screenshot(image) => {
            if !super::outputs::reserve(id, cx) {
                cx.refresh_windows();
                return;
            }
            let target = origin.draft.receipt(cx);
            let (taken, after, folders) =
                crate::attachments::take(crate::attachments::Target::Code, vec![image.clone()]);
            cx.spawn(async move |cx| {
                let mut prepared = cx
                    .background_executor()
                    .spawn(async move { crate::attachments::prepare(taken) })
                    .await;
                prepared.errors.extend(folders);
                if let Some(after) = after {
                    prepared.errors.push(after);
                }
                cx.update(|cx| {
                    // An accepted capture may complete after Return; authority and owner ID, not its old view, own the receipt.
                    if !alive(id, cx) || !origin.current() {
                        return;
                    }
                    super::outputs::prepared(
                        id,
                        origin,
                        super::outputs::Receipt {
                            target,
                            output: super::outputs::Output::Attachments(prepared),
                        },
                        cx,
                    );
                });
            })
            .detach();
        }
        BrowserEvent::StopAgent => {
            if let Some(valid) = continuation_guard(id, cx) {
                origin.runs.interrupt_guarded(origin.key.session.clone(), valid);
            }
        }
        BrowserEvent::Permission => notify(&origin.key, cx),
        BrowserEvent::Activity(tab) => presentation_event(origin.key, Some(tab.clone()), cx),
        BrowserEvent::Navigated => presentation_event(origin.key, None, cx),
    }
}
pub(super) fn notify(key: &super::Key, cx: &mut App) {
    let key = key.clone();
    cx.defer(move |cx| {
        for (window, workspace) in crate::windows::windows(cx) {
            if workspace.read(cx).browser_detach_key(&key.session).as_ref() != Some(&key) {
                continue;
            }
            let _ = window.update(cx, |_, _, cx| workspace.update(cx, |_, cx| cx.notify()));
        }
    });
}

/// Preserve the source session's automatic/manual Browser navigation state without selecting a different session.
fn presentation_event(key: super::Key, activity: Option<String>, cx: &mut App) {
    cx.defer(move |cx| {
        for (window, workspace) in crate::windows::windows(cx) {
            if workspace.read(cx).browser_detach_key(&key.session).as_ref() != Some(&key) {
                continue;
            }
            let _ = window.update(cx, |_, _, cx| {
                workspace.update(cx, |workspace, cx| {
                    if let Some(tab) = &activity {
                        workspace.native_browser_activity(&key.session, tab, cx);
                    } else if matches!(&workspace.view, super::super::View::Code(view) if view.id == key.session) {
                        workspace.manual_workbench();
                    }
                    cx.notify();
                })
            });
        }
    });
}
