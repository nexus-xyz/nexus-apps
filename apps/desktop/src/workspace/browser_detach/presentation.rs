//! Fresh native presentation around the same retained CEF HostPages.
use super::{
    alive, init,
    model::{Presentation, Registry},
    saved,
};
use crate::side::browser::Browser;
use gpui_kit::component::{TitleBar, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::{cell::RefCell, rc::Rc};

pub(super) struct DetachedBrowser {
    pub owner: u64,
    pub browser: Option<Entity<Browser>>,
    focus: FocusHandle,
    _events: Option<Subscription>,
}
impl DetachedBrowser {
    fn new(owner: u64, browser: Option<Entity<Browser>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        window.set_window_title("Browser · Nexus");
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let events = browser.as_ref().map(|browser| {
            cx.subscribe_in(browser, window, move |_, browser, event, _, cx| {
                super::routing::event(owner, browser.entity_id(), event, cx);
            })
        });
        Self {
            owner,
            browser,
            focus,
            _events: events,
        }
    }
}
impl Render for DetachedBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<crate::pages::Theme>().0.solid();
        let available = alive(self.owner, cx);
        let owner = self.owner;
        let here = window.window_handle().window_id();
        let notice = cx
            .try_global::<Registry>()
            .and_then(|registry| registry.id(owner))
            .filter(|entry| entry.current())
            .and_then(|entry| {
                entry
                    .problem
                    .clone()
                    .map(|problem| (problem, !entry.pending.is_empty()))
            });

        v_flex()
            .id("detached-session-browser")
            .track_focus(&self.focus)
            .size_full()
            .bg(palette.background)
            .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                let modifiers = event.keystroke.modifiers;
                let move_key = event.keystroke.key == "n"
                    && !modifiers.shift
                    && modifiers.control
                    && if cfg!(target_os = "macos") {
                        modifiers.platform && !modifiers.alt
                    } else {
                        modifiers.alt && !modifiers.platform
                    };
                if move_key {
                    cx.stop_propagation();
                    cx.defer(move |cx| super::returning::return_from(owner, here, false, cx));
                }
            }))
            .child(
                TitleBar::new()
                    .h(px(crate::tokens::HEAD))
                    .border_b_0()
                    .bg(palette.background)
                    .child(
                        h_flex()
                            .size_full()
                            .pl(px(if cfg!(target_os = "macos") { 80. } else { 12. }))
                            .pr(px(12.))
                            .gap(px(10.))
                            .child(div().flex_1().child("Browser"))
                            .child(
                                crate::side::chip("return-browser-to-session", "Return to session", false, palette)
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.defer(move |cx| super::returning::return_from(owner, here, false, cx));
                                    })),
                            ),
                    ),
            )
            .children(notice.map(|(problem, retry)| {
                h_flex()
                    .gap(px(8.))
                    .p(px(10.))
                    .flex_wrap()
                    .child(crate::side::note(problem, palette))
                    .when(retry, |row| {
                        row.child(
                            crate::side::chip("retry-browser-output", "Retry output", false, palette).on_click(
                                cx.listener(move |_, _, _, cx| {
                                    let current = cx
                                        .try_global::<Registry>()
                                        .and_then(|registry| registry.id(owner))
                                        .is_some_and(|entry| {
                                            entry.current()
                                                && matches!(&entry.presentation,
                                        Presentation::Detached { window, .. } if window.window_id() == here)
                                        });
                                    if current {
                                        super::retry(owner, cx);
                                    }
                                }),
                            ),
                        )
                    })
            }))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .children(available.then(|| self.browser.clone()).flatten()),
            )
    }
}
/// The recoverable slot stays in the registry until a successful native presentation is installed.
pub(super) fn open(owner: u64, cx: &mut App) -> bool {
    init(cx);
    if !alive(owner, cx) || cx.global::<Registry>().quitting {
        return false;
    }
    let Some(slot) = saved(owner, cx) else { return false };
    #[cfg(test)]
    if std::mem::take(&mut cx.global_mut::<Registry>().fail_next_open) {
        return false;
    }
    let constructed = Rc::new(RefCell::new(None::<Entity<Browser>>));
    let capture = constructed.clone();
    let options = crate::windows::browser_window_options(cx);
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            let browser = if alive(owner, cx) {
                let retained = slot.borrow_mut().take();
                retained.map(|saved| cx.new(|cx| Browser::restore(saved, window, cx)))
            } else {
                None
            };
            *capture.borrow_mut() = browser.clone();
            DetachedBrowser::new(owner, browser, window, cx)
        })
    });
    let opened = opened.ok();
    #[cfg(test)]
    let rejected = std::mem::take(&mut cx.global_mut::<Registry>().fail_after_construct);
    #[cfg(not(test))]
    let rejected = false;
    let valid = alive(owner, cx) && !cx.global::<Registry>().quitting && !rejected;
    if valid {
        if let (Some((window, _)), Some(browser)) = (opened.as_ref(), constructed.borrow().clone()) {
            let origin = cx.global::<Registry>().id(owner).map(|entry| entry.origin.clone());
            if let Some(origin) = origin {
                let bound = browser.update(cx, |browser, _| {
                    let super::super::runs::Source::Local { data, scope } = &origin.key.source else {
                        return false;
                    };
                    browser.bind_permission_owner(
                        crate::side::browser::agent::LocalOrigin {
                            data: data.clone(),
                            scope: scope.clone(),
                            machine: None,
                        },
                        origin.key.identity.clone(),
                        origin.proof.clone(),
                    )
                });
                if bound {
                    cx.global_mut::<Registry>()
                        .id_mut(owner)
                        .expect("checked owner")
                        .presentation = Presentation::Detached {
                        window: *window,
                        browser,
                    };
                    let _ = window.update(cx, |_, window, _| window.activate_window());
                    return true;
                }
            }
        }
    }
    // The native constructor can have run before the platform reports failure. Recover from its fresh entity too.
    if let Some(browser) = constructed.borrow_mut().take() {
        let retained = browser.update(cx, |browser, cx| browser.retain(cx));
        if alive(owner, cx) {
            *slot.borrow_mut() = Some(retained);
        }
    }
    if let Some((window, _)) = opened {
        cx.global_mut::<Registry>().closing.insert(window.window_id());
        let _ = window.update(cx, |_, window, _| window.remove_window());
    }
    false
}
pub(super) fn focus(owner: u64, cx: &mut App) {
    let window = cx
        .try_global::<Registry>()
        .and_then(|registry| registry.id(owner))
        .and_then(|entry| {
            if !entry.current() {
                return None;
            }
            match &entry.presentation {
                Presentation::Detached { window, .. } | Presentation::Attached { window, .. } => Some(*window),
                _ => None,
            }
        });
    if let Some(window) = window {
        let _ = window.update(cx, |_, window, _| window.activate_window());
    }
}
