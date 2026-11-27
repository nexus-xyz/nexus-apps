//! Local clone state and requests for the project dialog and New session’s project menu.
//! A finished clone opens its project with a new session.

use crate::workspace::menu_item_hover;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Workspace, glyph};
use crate::palette::Palette;

/// How often the form reads git's progress while a clone runs.
pub(super) const POLL: Duration = Duration::from_millis(300);
/// Where a clone goes when nothing else was chosen (`clone.ts` `DEFAULT_PARENT`).
pub(super) const DEFAULT_PARENT: &str = "~";

pub(super) struct CloneForm {
    pub(super) source: Entity<InputState>,
    pub(super) parent: Entity<InputState>,
    /// The repository picked from GitHub, by name: Repository shows it, not a field to type in.
    pub(super) picked: Option<SharedString>,
    /// The clone running, by its request id.
    pub(super) running: Option<String>,
    /// Git's stage and percentage, while it runs.
    pub(super) progress: Option<(String, Option<u8>)>,
    pub(super) problem: Option<SharedString>,
    /// Open in New session's project menu, not under the sidebar's Add a project button.
    pub(super) in_menu: bool,
    _poll: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// Whether the form may clone: a source without spaces and a folder (`clone.ts` `cloneReady`).
pub(super) fn ready(source: &str, parent: &str) -> bool {
    let source = source.trim();
    !source.is_empty() && !source.contains(char::is_whitespace) && !parent.trim().is_empty()
}

/// Git's progress in words: `Receiving objects · 45%`, the stage alone without one, `Starting` before any.
pub(super) fn progress_label(progress: Option<&(String, Option<u8>)>) -> String {
    match progress {
        None => "Starting".into(),
        Some((stage, None)) => stage.clone(),
        Some((stage, Some(percent))) => format!("{stage} · {percent}%"),
    }
}

/// A labelled field of a clone form (Repository, Into), disabled while the clone runs.
pub(super) fn clone_field(label: &'static str, input: &Entity<InputState>, running: bool, palette: Palette) -> Div {
    h_flex()
        .gap(px(8.))
        .child(div().w(px(70.)).flex_none().text_color(palette.faint).child(label))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .h(px(28.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .border_1()
                .border_color(palette.border)
                .font_family("monospace")
                .text_size(px(12.))
                .child(Input::new(input).appearance(false).disabled(running)),
        )
}

/// The Repository row of a clone whose repository was picked from GitHub: its name, which cannot be edited.
pub(super) fn picked_repo(name: SharedString, palette: Palette) -> Div {
    h_flex()
        .gap(px(8.))
        .child(
            div()
                .w(px(70.))
                .flex_none()
                .text_color(palette.faint)
                .child("Repository"),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w(px(0.))
                .h(px(28.))
                .gap(px(6.))
                .text_color(palette.foreground)
                .child(glyph(Lucide::FolderGit2, palette.icon).size(px(14.)))
                .child(div().truncate().child(name)),
        )
}

/// A running clone's bar, and git's stage in words under it.
pub(super) fn clone_progress(progress: Option<&(String, Option<u8>)>, palette: Palette) -> Div {
    let percent = progress.and_then(|(_, percent)| *percent);
    v_flex()
        .gap(px(4.))
        .child(
            div().h(px(3.)).w_full().rounded(px(2.)).bg(palette.selected).child(
                div()
                    .h_full()
                    .rounded(px(2.))
                    .bg(palette.strong)
                    .w(relative(percent.map_or(0.15, |percent| f32::from(percent) / 100.))),
            ),
        )
        .child(
            div()
                .text_xs()
                .text_color(palette.faint)
                .child(progress_label(progress)),
        )
}

impl Workspace {
    /// The clone form in New session's project menu, as its picker's "Clone a repository" opens it
    /// (`new-session.tsx`). A clone that runs keeps its form where it is.
    pub(super) fn open_clone_in_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.cloning() {
            self.open_clone_form(true, window, cx);
        }
        cx.notify();
    }

    /// The project menu opens afresh: a clone form it left open, with no clone running, goes.
    pub(super) fn drop_idle_menu_clone(&mut self) {
        if self
            .clone_form
            .as_ref()
            .is_some_and(|form| form.in_menu && form.running.is_none())
        {
            self.clone_form = None;
        }
    }

    /// A new clone form, its Into the folder the last clone on this computer went under (`clone.ts` `savedParent`).
    pub(super) fn open_clone_form(&mut self, in_menu: bool, window: &mut Window, cx: &mut Context<Self>) {
        {
            let saved = crate::paths::clone_parent(None).unwrap_or_else(|| DEFAULT_PARENT.to_string());
            let source = cx.new(|cx| InputState::new(window, cx).placeholder("https://… or owner/name"));
            let parent = cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder("~/code");
                state.set_value(saved, window, cx);
                state
            });
            let submit = |this: &mut Workspace,
                          _: &Entity<InputState>,
                          event: &InputEvent,
                          _: &mut Window,
                          cx: &mut Context<Workspace>| {
                if let InputEvent::PressEnter { .. } = event {
                    this.start_clone(cx);
                } else if let InputEvent::Change = event {
                    if let Some(form) = &mut this.clone_form {
                        form.problem = None;
                    }
                    cx.notify();
                }
            };
            let subscriptions = vec![
                cx.subscribe_in(&source, window, submit),
                cx.subscribe_in(&parent, window, submit),
            ];
            source.update(cx, |state, cx| state.focus(window, cx));
            self.clone_form = Some(CloneForm {
                source,
                parent,
                picked: None,
                running: None,
                progress: None,
                problem: None,
                in_menu,
                _poll: None,
                _subscriptions: subscriptions,
            });
        }
    }

    pub(super) fn start_clone(&mut self, cx: &mut Context<Self>) {
        let read_only = self.read_only_note();
        let Some(form) = &mut self.clone_form else {
            return;
        };
        let source = form.source.read(cx).value().trim().to_string();
        let parent = form.parent.read(cx).value().trim().to_string();
        if form.running.is_some() || !ready(&source, &parent) {
            return;
        }
        let Some(runner) = &self.runner else {
            form.problem = Some(read_only);
            cx.notify();
            return;
        };
        let request = format!(
            "clone-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |time| time.as_nanos())
        );
        crate::paths::save_clone_parent(None, &parent);
        runner.clone_project(source, parent, request.clone());
        form.running = Some(request);
        form.progress = None;
        form.problem = None;
        form._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let alive = this.update(cx, |this, cx| {
                    let Some(form) = &mut this.clone_form else {
                        return false;
                    };
                    let Some(request) = form.running.clone() else {
                        return false;
                    };
                    if let Some(progress) = this.runner.as_ref().and_then(|runner| runner.clone_progress(&request)) {
                        form.progress = Some(progress);
                        cx.notify();
                    }
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// Put a folder picked with the system's picker in the form's Into field.
    pub(super) fn choose_clone_parent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self
            .clone_form
            .as_ref()
            .filter(|form| form.running.is_none())
            .map(|form| form.parent.clone())
        else {
            return;
        };
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Clone into".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(form) = &this.clone_form {
                    if form.parent != parent || form.running.is_some() {
                        return;
                    }
                    let path = path.to_string_lossy().into_owned();
                    form.parent.update(cx, |state, cx| state.set_value(path, window, cx));
                }
            });
        })
        .detach();
    }

    /// Whether a clone from this window's form is running: its result comes back here (`windows.rs`).
    pub(super) fn cloning(&self) -> bool {
        self.clone_form.as_ref().is_some_and(|form| form.running.is_some())
    }

    /// A clone ended: its project opens with a new session in it, or the form says why it failed.
    pub(super) fn cloned(&mut self, cloned: Result<String, String>, window: &mut Window, cx: &mut Context<Self>) {
        match cloned {
            Ok(project) => {
                if self.clone_form.as_ref().is_some_and(|form| !form.in_menu) {
                    self.project_dialog = None;
                }
                self.clone_form = None;
                self.refresh_code(cx);
                self.open_new_code_in(project, window, cx);
            }
            Err(error) => {
                if let Some(form) = &mut self.clone_form {
                    form.running = None;
                    form.progress = None;
                    form.problem = Some(error.into());
                } else {
                    self.composer_note = Some(error.into());
                }
            }
        }
    }

    /// The clone form in New session's project menu, while it is open there.
    pub(super) fn render_menu_clone(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.clone_form.as_ref().filter(|form| form.in_menu)?;
        Some(self.render_clone_body(form, palette, cx).into_any_element())
    }

    /// Clone a repository: Repository, Into with Choose, git's progress while it runs, why it failed, and Clone.
    fn render_clone_body(&self, form: &CloneForm, palette: Palette, cx: &mut Context<Self>) -> Div {
        let running = form.running.is_some();
        let can_clone = !running
            && ready(
                form.source.read(cx).value().as_ref(),
                form.parent.read(cx).value().as_ref(),
            );
        let field = |label: &'static str, input: &Entity<InputState>| clone_field(label, input, running, palette);
        let choose = div()
            .id("clone-choose")
            .flex_none()
            .px(px(10.))
            .h(px(28.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(palette.border)
            .when(!running, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| menu_item_hover(style, palette))
                    .on_click(cx.listener(|this, _, window, cx| this.choose_clone_parent(window, cx)))
            })
            .child("Choose");
        let clone_button = div()
            .id("clone-go")
            .flex_none()
            .px(px(12.))
            .h(px(28.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(palette.border)
            .text_color(if can_clone { palette.foreground } else { palette.faint })
            .when(can_clone, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| menu_item_hover(style, palette))
                    .on_click(cx.listener(|this, _, _, cx| this.start_clone(cx)))
            })
            .child(if running { "Cloning…" } else { "Clone" });
        let progress = clone_progress(form.progress.as_ref(), palette);
        v_flex()
            .gap(px(8.))
            .p(px(8.))
            .text_size(px(13.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .text_color(palette.foreground)
                    .child(glyph(Lucide::GitBranch, palette.icon).size(px(15.)))
                    .child("Clone a repository"),
            )
            .child(match &form.picked {
                Some(name) => picked_repo(name.clone(), palette),
                None => field("Repository", &form.source),
            })
            .child(field("Into", &form.parent).child(choose))
            .when(running, |body| body.child(progress))
            .when_some(form.problem.clone(), |body, problem| {
                body.child(div().text_xs().text_color(palette.destructive).child(problem))
            })
            .child(h_flex().justify_end().child(clone_button))
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::{progress_label, ready};

    #[test]
    fn clone_needs_a_source_and_a_folder() {
        assert!(ready("owner/name", "~"));
        assert!(ready(" https://github.com/a/b.git ", "/tmp"));
        assert!(!ready("", "~"));
        assert!(!ready("owner name", "~"));
        assert!(!ready("owner/name", " "));
    }

    #[test]
    fn progress_reads_as_words() {
        assert_eq!(progress_label(None), "Starting");
        assert_eq!(
            progress_label(Some(&("Resolving deltas".into(), None))),
            "Resolving deltas"
        );
        assert_eq!(
            progress_label(Some(&("Receiving objects".into(), Some(45)))),
            "Receiving objects · 45%"
        );
    }
}
