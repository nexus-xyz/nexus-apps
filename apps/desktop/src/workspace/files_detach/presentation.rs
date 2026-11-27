//! A browser-free native Files window, rebuilt from a plain snapshot.
use super::{
    current, guard,
    model::{Presentation, Registry},
    origin, saved,
};
use crate::side::files::Files;
use gpui_kit::component::{TitleBar, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::{cell::RefCell, rc::Rc};

pub(super) struct DetachedFiles {
    owner: u64,
    files: Option<Entity<Files>>,
    focus: FocusHandle,
}
impl DetachedFiles {
    fn new(
        owner: u64,
        files: Option<Entity<Files>>,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        window.set_window_title(&format!("Files · {title} · Nexus"));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self { owner, files, focus }
    }
}
impl Render for DetachedFiles {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<crate::pages::Theme>().0.solid();
        let id = self.owner;
        let expected = window.window_handle().window_id();
        let available = current(id, cx);
        let metadata = cx
            .try_global::<Registry>()
            .and_then(|registry| registry.id(id))
            .map(|entry| (entry.origin.title.clone(), entry.problem.clone()));
        let (title, problem) = metadata.unwrap_or_else(|| ("Session unavailable".into(), None));
        v_flex()
            .id("detached-session-files")
            .size_full()
            .bg(palette.background)
            .track_focus(&self.focus)
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
                    cx.defer(move |cx| super::returning::return_from(id, expected, false, cx));
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
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .child(format!("Files · {title}")),
                            )
                            .child(
                                crate::side::chip("return-files-to-session", "Return to session", false, palette)
                                    .debug_selector(|| "return-files-to-session".into())
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.defer(move |cx| super::returning::return_from(id, expected, false, cx));
                                    })),
                            ),
                    ),
            )
            .children(problem.map(|problem| crate::side::note(problem, palette)))
            .when(!available, |body| {
                body.child(crate::side::note(
                    "The original session source is unavailable.",
                    palette,
                ))
            })
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .children(available.then(|| self.files.clone()).flatten()),
            )
    }
}
pub(super) fn open(id: u64, cx: &mut App) -> bool {
    if !current(id, cx) || cx.global::<Registry>().quitting {
        return false;
    }
    let (Some(slot), Some(origin), Some(guard)) = (saved(id, cx), origin(id, cx), guard(id, cx)) else {
        return false;
    };
    #[cfg(test)]
    if std::mem::take(&mut cx.global_mut::<Registry>().fail_next_open) {
        return false;
    }
    let constructed = Rc::new(RefCell::new(None::<Entity<Files>>));
    let captured = constructed.clone();
    let options = crate::windows::files_window_options(cx);
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            let files = if current(id, cx) {
                let state = slot.borrow_mut().take();
                state.map(|state| cx.new(|cx| Files::restore(origin.place(), state, guard.clone(), window, cx)))
            } else {
                None
            };
            *captured.borrow_mut() = files.clone();
            DetachedFiles::new(id, files, origin.title.clone(), window, cx)
        })
    })
    .ok();
    #[cfg(test)]
    let rejected = std::mem::take(&mut cx.global_mut::<Registry>().fail_after_construct);
    #[cfg(not(test))]
    let rejected = false;
    if current(id, cx) && !cx.global::<Registry>().quitting && !rejected {
        if let (Some((window, _)), Some(files)) = (opened.as_ref(), constructed.borrow().clone()) {
            cx.global_mut::<Registry>()
                .id_mut(id)
                .expect("current Files owner")
                .presentation = Presentation::Detached { window: *window, files };
            let _ = window.update(cx, |_, window, _| window.activate_window());
            return true;
        }
    }
    // A native constructor can consume the snapshot before the platform reports failure.
    if let Some(files) = constructed.borrow_mut().take() {
        let state = files.update(cx, |files, cx| files.retain(cx));
        if current(id, cx) {
            *slot.borrow_mut() = Some(state);
        }
    }
    if let Some((window, _)) = opened {
        cx.global_mut::<Registry>().closing.insert(window.window_id());
        let _ = window.update(cx, |_, window, _| window.remove_window());
    }
    false
}
