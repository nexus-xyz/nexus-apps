//! The page's failures that setup fixes: the GitHub CLI missing or signed out shows as a setup line whose fix runs in
//! the Workbench terminal, and the page reads again when it ends (`crate::github_cli`). Any other failure of the read
//! is the red ✕ with Try again and Copy; the rest keep their titled box.

use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_pull_requests::pull_page::{ErrorKind, PullError};

use super::{PullRequestPage, problem_box};
use crate::github_cli::Block;
use crate::palette::Palette;
use crate::{pull_page as model, setup};

/// The setup lines' scope: their keys are `pr:gh-auth` and `pr:gh-install`.
const SCOPE: &str = "pr";
/// What the lines' first sentence ends with.
const DOING: &str = "see this pull request";

/// Whether the read's failure is `gh`'s own, which Try again may get past.
fn read_failure(kind: ErrorKind) -> bool {
    matches!(kind, ErrorKind::Github | ErrorKind::Timeout | ErrorKind::Invalid)
}

impl PullRequestPage {
    /// The read failed: `block` is what setup fixes, if anything ([`Block::of_pull`], found off the UI thread).
    pub(super) fn read_failed(&mut self, problem: PullError, block: Option<Block>, cx: &mut Context<Self>) {
        if let Some(block) = &block {
            self.arm(block, cx);
        }
        self.problem = Some(problem);
        self.block = block;
    }

    /// When `block`'s fix is done, the page reads again: the blocked read goes on by itself. Once per fix.
    pub(super) fn arm(&mut self, block: &Block, cx: &mut Context<Self>) {
        let key = block.key(SCOPE);
        if self.armed.as_ref() == Some(&key) {
            return;
        }
        self.armed = Some(key.clone());
        let page = cx.entity().downgrade();
        setup::on_done(key, cx, move |_, cx| {
            let reloaded = page.update(cx, |page, cx| {
                page.armed = None;
                page.forget_signed_out();
                page.load(cx);
            });
            if let Err(error) = reloaded {
                eprintln!("nexus-desktop-gpui: pull request page: {error}");
            }
        });
    }

    /// The write failures a sign-in fixed: they go with it.
    fn forget_signed_out(&mut self) {
        let signed_out = |problem: &PullError| problem.kind == ErrorKind::SignedOut;
        if self.merge_problem.as_ref().is_some_and(signed_out) {
            self.merge_problem = None;
        }
        self.thread_problems.retain(|_, problem| !signed_out(problem));
        self.stack.forget(signed_out);
    }

    /// The read's failure, under the head: the setup line for `gh`, the red ✕ for `gh`'s other failures, else its box.
    pub(super) fn render_read_problem(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        // The view's one white button, while no page with its own merge button is shown.
        let main = self.page.is_none();
        let line = self.block.as_ref().map(|block| {
            let line = block.line(SCOPE, DOING);
            if main { line.main() } else { line }
        });
        let setup_line = setup::slot("pull-gh", line, &self.fixer, palette, cx);
        if setup_line.is_some() {
            return setup_line;
        }
        let problem = self.problem.as_ref()?;
        if !read_failure(problem.kind) {
            return Some(problem_box(problem, palette).into_any_element());
        }
        let page = cx.entity().downgrade();
        let line = crate::github_cli::failed(
            "pr:read",
            model::error_title(problem.kind),
            &problem.message,
            move |_, cx| {
                if let Err(error) = page.update(cx, |page, cx| page.load(cx)) {
                    eprintln!("nexus-desktop-gpui: pull request page: {error}");
                }
            },
        );
        Some(line.render(&self.fixer, palette, cx).into_any_element())
    }

    /// A write's failure by the control that made it: a lapsed sign-in as its setup line, else its box.
    pub(super) fn render_problem(&self, problem: &PullError, palette: Palette, cx: &App) -> AnyElement {
        if problem.kind == ErrorKind::SignedOut {
            let line = Block::SignedOut.line(SCOPE, "change this pull request");
            return div()
                .mt(px(8.))
                .child(line.render(&self.fixer, palette, cx))
                .into_any_element();
        }
        problem_box(problem, palette).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn only_gh_failures_of_the_read_offer_try_again() {
        assert!(read_failure(ErrorKind::Github) && read_failure(ErrorKind::Timeout));
        assert!(!read_failure(ErrorKind::NotFound) && !read_failure(ErrorKind::Session));
        assert!(!read_failure(ErrorKind::SignedOut) && !read_failure(ErrorKind::Unavailable));
    }
}
