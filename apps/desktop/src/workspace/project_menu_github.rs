//! The project menu's GitHub section when it has no repositories to list: still reading them, GitHub not connected
//! (a quiet row that opens the GitHub step in Settings › Cloud), or a real failure (the red ✕ and Try again).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Workspace, glyph};
use crate::pages::PageId;
use crate::palette::Palette;
use crate::setup::{Fix, SetupLine, Tone};

/// What the GitHub section shows in place of its repositories.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum GitHubNote {
    Reading,
    NotConnected,
    Failed(SharedString),
}

/// The note for the repositories' read: None once they are listed, or when this build runs no cloud.
pub(super) fn github_note(read: Option<Result<(), SharedString>>, cloud: bool) -> Option<GitHubNote> {
    match read {
        None if cloud => Some(GitHubNote::Reading),
        None | Some(Ok(())) => None,
        Some(Err(error)) if error.as_ref() == crate::cloud::GITHUB_NOT_CONNECTED => Some(GitHubNote::NotConnected),
        Some(Err(error)) => Some(GitHubNote::Failed(error)),
    }
}

impl Workspace {
    pub(super) fn render_github_note(&self, note: GitHubNote, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        match note {
            GitHubNote::Reading => div()
                .px(px(10.))
                .py(px(6.))
                .text_size(px(12.))
                .text_color(palette.faint)
                .child("Reading your repositories from GitHub…")
                .into_any_element(),
            GitHubNote::NotConnected => self.github_connect_row(palette, cx),
            GitHubNote::Failed(error) => {
                let this = cx.entity().downgrade();
                let again = Fix::act(move |window, cx| {
                    if let Err(error) = this.update(cx, |this, cx| this.load_cloud_repos(window, cx)) {
                        eprintln!("nexus-desktop-gpui: GitHub repositories: {error}");
                    }
                });
                let line = SetupLine::new(
                    "project:github-repos",
                    Lucide::X,
                    "GitHub didn't list your repositories.",
                )
                .tone(Tone::Failed)
                .why(error)
                .fix("Try again", again);
                div()
                    .px(px(4.))
                    .py(px(2.))
                    .child(line.render(&self.fixer(cx), palette, cx))
                    .into_any_element()
            }
        }
    }

    /// "GitHub isn't connected" and Connect ›, which opens the GitHub step in Settings › Cloud. The menu reads the
    /// repositories again when it next opens.
    fn github_connect_row(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let fixer = self.fixer(cx);
        let fix = Fix::open_at(PageId::Cloud, crate::pages::CLOUD_GITHUB_ANCHOR);
        h_flex()
            .id("launch-github-connect")
            .w_full()
            .h(px(30.))
            .px(px(10.))
            .gap(px(10.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|style| super::menu_item_hover(style, palette))
            .child(glyph(Lucide::Github, palette.icon).size(px(15.)).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(13.5))
                    .text_color(palette.muted)
                    .child("GitHub isn't connected"),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(2.))
                    .text_size(px(12.5))
                    .text_color(palette.muted)
                    .child("Connect")
                    .child(glyph(Lucide::ChevronRight, palette.icon).size(px(13.))),
            )
            // Not a listener: the fix updates the workspace itself.
            .on_click(move |_, window, cx| fixer.run("project:github".into(), fix.clone(), window, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn no_github_sign_in_is_a_quiet_row_not_an_error() {
        let missing = Some(Err(SharedString::from(crate::cloud::GITHUB_NOT_CONNECTED)));
        assert_eq!(github_note(missing, true), Some(GitHubNote::NotConnected));
    }

    #[test]
    fn a_real_failure_stays_a_failure() {
        let failed = Some(Err(SharedString::from("GitHub answered 502")));
        assert_eq!(
            github_note(failed, true),
            Some(GitHubNote::Failed("GitHub answered 502".into()))
        );
        assert_eq!(github_note(None, true), Some(GitHubNote::Reading));
        assert_eq!(github_note(None, false), None);
        assert_eq!(github_note(Some(Ok(())), true), None);
    }
}
