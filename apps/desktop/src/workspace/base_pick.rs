//! New session's "New worktree … from": the branch a new worktree starts from, as T3 Code's base branch picker. With
//! New worktree picked, the workspace menu lists what the project's repository can start one from
//! (`wb_worktree_bases`: the default base first, `origin/main`, then the checked-out branch, then the rest, most recent
//! first), searchable by name. Nothing picked starts from main, else the repository's default base. The worktree is made
//! when the first message is sent (`nexus_workbench`'s `make_worktree`), so a session that never starts leaves none.

use crate::workspace::overlay::OverlaySlot;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::json;

use super::Workspace;
use super::code_dock::menu_row;
use crate::code_run::default_worktree_base;
use crate::palette::Palette;

/// The most branches the menu shows at once; the search narrows the rest.
const SHOWN: usize = 60;

/// The base choice of the form: the branch picked, and the picker's search and list.
#[derive(Default)]
pub(super) struct BasePick {
    /// The branch picked, for `project`; None: main, else the repository's default base.
    pub picked: Option<String>,
    input: Option<(Entity<InputState>, Subscription)>,
    query: String,
    /// What `project`'s repository can start a worktree from, default first; None while it is read.
    bases: Option<Result<Vec<String>, String>>,
    /// The project the list and the pick are for.
    project: Option<String>,
    _read: Option<Task<()>>,
}

impl BasePick {
    /// The branch to start from in `project`: the pick, when it was made for that project.
    pub fn for_project(&self, project: &str) -> Option<String> {
        self.picked.clone().filter(|_| self.project.as_deref() == Some(project))
    }

    /// Whether the branch search has the keys, as the test reads it.
    #[cfg(test)]
    pub fn search_focused(&self, window: &Window, cx: &App) -> bool {
        self.input
            .as_ref()
            .is_some_and(|(input, _)| input.read(cx).focus_handle(cx).is_focused(window))
    }
}

/// The bases whose name holds every word of `query` (any case), in the list's order.
pub(super) fn matches<'a>(bases: &'a [String], query: &str) -> Vec<&'a String> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    bases
        .iter()
        .filter(|base| {
            let name = base.to_lowercase();
            words.iter().all(|word| name.contains(word))
        })
        .collect()
}

impl Workspace {
    /// New worktree picked: the menu stays open on the list of bases, read for this project.
    pub(super) fn choose_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.code_form.worktree = true;
        self.code_form.review.clear();
        self.code_form.remember();
        let base = &mut self.code_form.base;
        if base.input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Start from… (search branches)"));
            let subscription = cx.subscribe_in(
                &input,
                window,
                |this, state, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.code_form.base.query = state.read(cx).value().to_string();
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        let first = match &this.code_form.base.bases {
                            Some(Ok(bases)) => matches(bases, &this.code_form.base.query).first().map(|b| (*b).clone()),
                            _ => None,
                        };
                        if let Some(first) = first {
                            this.pick_base(first, window, cx);
                        }
                    }
                    _ => {}
                },
            );
            base.input = Some((input, subscription));
        }
        // The branch search takes the keys, so typing filters the bases at once.
        if let Some((input, _)) = &self.code_form.base.input {
            input.update(cx, |state, cx| state.focus(window, cx));
        }
        self.read_bases(cx);
        cx.notify();
    }

    /// The project's bases, read once per project; another project drops the pick.
    pub(super) fn read_bases(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.code_form.project.clone() else {
            return;
        };
        let base = &mut self.code_form.base;
        if base.project.as_ref() == Some(&project) && matches!(base.bases, Some(Ok(_))) {
            return;
        }
        if base.project.as_ref() != Some(&project) {
            base.picked = None;
        }
        let Some(runs) = self.runs(&project) else {
            self.code_form.base.bases = Some(Err("The Nexus app runs the sessions: pick a branch from there.".into()));
            return;
        };
        let base = &mut self.code_form.base;
        base.project = Some(project.clone());
        base.bases = None;
        let read = runs.call("wb_worktree_bases", json!({ "projectId": project }));
        base._read = Some(cx.spawn(async move |this, cx| {
            let result = read
                .await
                .unwrap_or_else(|_| Err("The read was lost.".into()))
                .and_then(|value| serde_json::from_value::<Vec<String>>(value).map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                if this.code_form.base.project.as_ref() == Some(&project) {
                    this.code_form.base.bases = Some(result);
                    cx.notify();
                }
            });
        }));
    }

    /// A base picked: the menu closes and the composer has the focus.
    fn pick_base(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let default = match &self.code_form.base.bases {
            Some(Ok(bases)) => default_worktree_base(bases).cloned(),
            _ => None,
        };
        // The default base is no pick: it follows main, as an untouched form does.
        self.code_form.base.picked = (default.as_ref() != Some(&name)).then_some(name);
        self.overlay.close_launch_menu();
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// The chip's words for the workspace with New worktree picked: "New worktree from <base>" once a base is known.
    pub(super) fn worktree_label(&self) -> String {
        let base = &self.code_form.base;
        let from = base.picked.clone().or_else(|| match &base.bases {
            Some(Ok(bases)) => default_worktree_base(bases).cloned(),
            _ => None,
        });
        match from {
            Some(from) => format!("New worktree from {from}"),
            None => "New worktree from main".into(),
        }
    }

    /// The list under the workspace menu's choices while New worktree is picked.
    pub(super) fn render_base_list(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let base = &self.code_form.base;
        let note = |text: String| {
            div()
                .px(px(10.))
                .py(px(6.))
                .text_size(px(12.5))
                .text_color(palette.muted)
                .child(text)
        };
        let field = gpui_kit::component::h_flex()
            .mx(px(4.))
            .my(px(4.))
            .h(px(30.))
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(8.))
            .bg(palette.hover)
            .text_size(px(13.))
            .child(super::glyph(gpui_kit::assets::IconName::Search, palette.faint).size(px(14.)))
            .child(
                div().flex_1().min_w(px(0.)).children(
                    base.input
                        .as_ref()
                        .map(|(input, _)| Input::new(input).appearance(false)),
                ),
            );
        let mut list = v_flex().id("launch-base-list").max_h(px(280.)).overflow_y_scroll();
        match &base.bases {
            None => list = list.child(note("Reading branches…".into())),
            Some(Err(error)) => list = list.child(note(error.clone()).text_color(palette.destructive)),
            Some(Ok(bases)) => {
                let default = default_worktree_base(bases);
                let shown = matches(bases, &base.query);
                if shown.is_empty() {
                    list = list.child(note("No branch matches.".into()));
                }
                let picked = base.picked.as_ref().or(default);
                for (index, name) in shown.into_iter().take(SHOWN).enumerate() {
                    let hint = (Some(name) == default).then(|| SharedString::from("Default"));
                    let pick = name.clone();
                    list = list.child(
                        menu_row(
                            SharedString::from(format!("launch-base-{index}")),
                            name.clone().into(),
                            hint,
                            picked == Some(name),
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, window, cx| this.pick_base(pick.clone(), window, cx))),
                    );
                }
            }
        }
        v_flex()
            .border_t_1()
            .border_color(palette.border)
            .mt(px(4.))
            .pt(px(4.))
            .child(note("Start the new worktree from".into()))
            .child(field)
            .child(list)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn every_word_narrows_the_bases_in_order() {
        let bases: Vec<String> = ["origin/main", "main", "ada/worktrees", "origin/ada/fix"]
            .map(String::from)
            .to_vec();
        assert_eq!(matches(&bases, "").len(), 4);
        assert_eq!(matches(&bases, "ADA"), [&bases[2], &bases[3]]);
        assert_eq!(matches(&bases, "origin fix"), [&bases[3]]);
        assert!(matches(&bases, "nope").is_empty());
    }
}
