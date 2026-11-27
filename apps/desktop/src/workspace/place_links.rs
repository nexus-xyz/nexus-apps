//! Notes that name a place become the way there (setup board P10): the composer's "No project yet." opens the project
//! menu, and a prompt link whose repository no project has offers to clone it or to pick a project.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;
use super::new_code::LaunchMenu;
use super::overlay::OverlaySlot;
use crate::pages::PageId;
use crate::palette::Palette;
use crate::setup::quiet::link;
use crate::setup::{Fix, SetupLine, Tone};

/// Why a new session can't start yet: no project is picked.
pub(super) const NO_PROJECT: &str = "No project yet.";

/// A repository a prompt link named that no project here has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MissingRepo {
    /// What git clones.
    pub url: String,
    /// As the note names it: `owner/name` on GitHub, else with its host.
    pub name: String,
}

impl MissingRepo {
    pub(super) fn new(host: &str, path: &str) -> Self {
        Self {
            url: format!("https://{host}/{path}.git"),
            name: if host == "github.com" {
                path.to_string()
            } else {
                format!("{host}/{path}")
            },
        }
    }
}

/// The note under New session after a prompt link: what it could not use, and the repository no project has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LinkNote {
    others: Vec<String>,
    pub missing: Option<MissingRepo>,
    pub text: SharedString,
}

impl LinkNote {
    /// None when there is nothing to say.
    pub(super) fn new(others: Vec<String>, missing: Option<MissingRepo>) -> Option<Self> {
        let mut parts = Vec::new();
        if let Some(missing) = &missing {
            parts.push(format!("No project has {}.", missing.name));
        }
        parts.extend(others.iter().cloned());
        (!parts.is_empty()).then(|| Self {
            others,
            missing,
            text: parts.join(" ").into(),
        })
    }

    /// A project was picked, or the repository is being cloned: only the other notes stay.
    fn without_missing(self) -> Option<Self> {
        Self::new(self.others, None)
    }
}

impl Workspace {
    /// Open the project menu, unless it is open.
    pub(super) fn open_project_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.launch_menu() != Some(LaunchMenu::Project) {
            self.toggle_launch_menu(LaunchMenu::Project, window, cx);
        }
    }

    /// A project was picked: the notes that asked for one go.
    pub(super) fn project_picked(&mut self) {
        if self.code_form.project.is_none() {
            return;
        }
        if self.composer_note.as_deref() == Some(NO_PROJECT) {
            self.composer_note = None;
        }
        if self.link_note.as_ref().is_some_and(|note| note.missing.is_some()) {
            self.link_note = self.link_note.take().and_then(LinkNote::without_missing);
        }
    }

    /// "Pick a project ›" after the composer's note, while the note says no project is picked.
    pub(super) fn render_pick_project_link(
        &self,
        note: Option<&SharedString>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        (note?.as_ref() == NO_PROJECT).then(|| {
            link("composer-pick-project", "Pick a project", palette)
                .on_click(cx.listener(|this, _, window, cx| this.open_project_menu(window, cx)))
                .into_any_element()
        })
    }

    /// The prompt link note's way on: "Clone it ›" and "Pick one ›", while it names a repository no project has.
    pub(super) fn render_link_note_actions(
        &self,
        note: &LinkNote,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let missing = note.missing.clone()?;
        Some(
            h_flex()
                .flex_none()
                .gap(px(12.))
                .text_size(px(13.))
                .child(link("link-note-clone", "Clone it", palette).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.link_note = this.link_note.take().and_then(LinkNote::without_missing);
                        let missing = missing.clone();
                        this.clone_here(missing.url, missing.name, None, window, cx);
                    },
                )))
                .child(
                    link("link-note-pick", "Pick one", palette)
                        .on_click(cx.listener(|this, _, window, cx| this.open_project_menu(window, cx))),
                )
                .into_any_element(),
        )
    }

    /// Fork's "with another subscription", when no subscription is linked: "Link one ›" opens Settings at the plans.
    pub(super) fn render_fork_no_subscription(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .flex_wrap()
            .gap(px(6.))
            .px(px(8.))
            .py(px(6.))
            .text_color(palette.muted)
            .child("No subscription linked.")
            .child(
                link("fork-link-subscription", "Link one", palette).on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.overlay.close_session_menu();
                    this.run_setup_fix(FORK_PLANS.into(), Fix::open(PageId::Subscriptions), window, cx);
                })),
            )
            .into_any_element()
    }

    /// Fork's subscriptions could not be read: the red ✕, and Try again reads them again.
    pub(super) fn render_fork_plans_failed(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let line = SetupLine::new(FORK_PLANS, Lucide::X, "Nexus couldn't read your subscriptions.")
            .tone(Tone::Failed)
            .fix(
                "Try again",
                Fix::act(move |_, cx| {
                    let read = this.update(cx, |this, cx| {
                        this.plan_usage = None;
                        this.probe_plans(cx);
                        cx.notify();
                    });
                    if let Err(error) = read {
                        eprintln!("nexus-desktop-gpui: read the subscriptions again: {error}");
                    }
                }),
            );
        div()
            .p(px(4.))
            .child(line.render(&self.fixer(cx), palette, cx))
            .into_any_element()
    }
}

/// The fork menu's subscriptions, as a setup line's key.
const FORK_PLANS: &str = "fork:subscriptions";

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn a_missing_repository_leads_the_note_and_goes_once_a_project_is_picked() {
        let missing = MissingRepo::new("github.com", "acme/cortex-sim");
        assert_eq!(missing.url, "https://github.com/acme/cortex-sim.git");
        let note = LinkNote::new(vec!["Your earlier draft is in the stash.".into()], Some(missing)).expect("a note");
        assert_eq!(
            note.text.as_ref(),
            "No project has acme/cortex-sim. Your earlier draft is in the stash."
        );
        let rest = note.without_missing().expect("the other note stays");
        assert_eq!(rest.text.as_ref(), "Your earlier draft is in the stash.");
        assert_eq!(rest.missing, None);
        assert_eq!(LinkNote::new(Vec::new(), None), None);
    }

    #[test]
    fn a_repository_off_github_keeps_its_host() {
        let missing = MissingRepo::new("gitlab.com", "acme/decoder");
        assert_eq!(missing.name, "gitlab.com/acme/decoder");
        assert_eq!(missing.url, "https://gitlab.com/acme/decoder.git");
    }
}
