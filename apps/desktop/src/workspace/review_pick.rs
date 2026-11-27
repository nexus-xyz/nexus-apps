//! New session's "Review a pull request", as the Nexus app's (`new-session.tsx`'s `PullRequestList`): the workspace menu's
//! third choice checks someone's pull request out in the reviews folder. Picked, the menu lists the repository's open
//! pull requests (`wb_pull_requests`, the GitHub or GitLab CLI), searchable by title, author and branch. A typed or
//! pasted number or link works too (`nexus_workbench::pull_link`), and a link for another repository says so. The
//! session starts with `WorktreeArgs::review`.

use crate::workspace::overlay::OverlaySlot;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::git::PullRequestInfo;
use nexus_workbench::pull_link::{pull_number, repo_of_pull, repo_of_remote};
use nexus_workbench::source_host::Host;
use serde_json::json;

use super::Workspace;
use super::code_dock::menu_row;
use crate::palette::Palette;

/// The review choice of the form: whether it is picked, the pull request, and the picker's search and list.
#[derive(Default)]
pub(super) struct ReviewPick {
    /// "Review a pull request" is the workspace.
    pub on: bool,
    /// The pull request to check out, once picked.
    pub number: Option<u64>,
    input: Option<(Entity<InputState>, Subscription)>,
    query: String,
    /// The open pull requests of `project`; None while they are read.
    pulls: Option<Result<Vec<PullRequestInfo>, String>>,
    project: Option<String>,
    _read: Option<Task<()>>,
}

impl ReviewPick {
    /// Back to no review: another project, or the folder or a worktree picked.
    pub fn clear(&mut self) {
        self.on = false;
        self.number = None;
    }
}

/// What the list shows for `query`: the number typed or pasted (None for a link to another repository), whether that
/// is why it is None, and the pull requests that match. The repository comes from the project remotes, independent of the list.
pub(super) fn matches<'a>(
    pulls: &'a [PullRequestInfo],
    query: &str,
    repo: Option<&str>,
) -> (Option<u64>, bool, Vec<&'a PullRequestInfo>) {
    let bare = query
        .trim()
        .trim_start_matches(['#', '!'])
        .bytes()
        .all(|byte| byte.is_ascii_digit());
    let typed = if repo.is_none() && !bare {
        None
    } else {
        pull_number(query, repo)
    };
    let other_repo = typed.is_none() && pull_number(query, None).is_some();
    let words = query.trim().to_lowercase();
    let shown = pulls
        .iter()
        .filter(|pull| {
            words.is_empty()
                || typed == Some(pull.number)
                || format!("{} {} {}", pull.title, pull.author, pull.head)
                    .to_lowercase()
                    .contains(&words)
        })
        .collect();
    (typed, other_repo, shown)
}

/// Where a project's pull requests live, by its remotes, as the git panel names them: GitLab's merge requests, else
/// GitHub's pull requests.
pub(super) fn host_of(remotes: &[String]) -> Host {
    match super::code_title::git_panel::request_word(remotes) {
        "merge request" => Host::GitLab,
        _ => Host::GitHub,
    }
}

impl Workspace {
    fn review_repo(&self) -> Option<String> {
        self.code_form
            .project
            .as_ref()
            .and_then(|id| self.projects.iter().find(|project| &project.id == id))
            .and_then(|project| project.remotes.iter().find_map(|remote| repo_of_remote(remote)))
    }

    /// The picked project's host.
    pub(super) fn review_host(&self) -> Host {
        self.code_form
            .project
            .as_ref()
            .and_then(|id| self.projects.iter().find(|project| &project.id == id))
            .map_or(Host::GitHub, |project| host_of(&project.remotes))
    }

    /// "Review a pull request" picked in the workspace menu: the menu stays open on the list, read for this project.
    pub(super) fn choose_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.code_form.worktree = false;
        self.code_form.review.on = true;
        let review = &mut self.code_form.review;
        if review.input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search, or paste a number or link"));
            let subscription = cx.subscribe_in(
                &input,
                window,
                |this, state, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.code_form.review.query = state.read(cx).value().to_string();
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        let pulls = match &this.code_form.review.pulls {
                            Some(Ok(pulls)) => pulls.as_slice(),
                            _ => &[],
                        };
                        let (typed, _, shown) =
                            matches(pulls, &this.code_form.review.query, this.review_repo().as_deref());
                        if let Some(number) = typed.or_else(|| shown.first().map(|pull| pull.number)) {
                            this.pick_review(number, window, cx);
                        }
                    }
                    _ => {}
                },
            );
            review.input = Some((input, subscription));
        }
        if let Some((input, _)) = &review.input {
            input.update(cx, |state, cx| state.focus(window, cx));
        }
        self.read_pulls(cx);
        cx.notify();
    }

    /// The project's open pull requests, read once per project.
    fn read_pulls(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.code_form.project.clone() else {
            return;
        };
        if self.code_form.review.project.as_ref() == Some(&project) && self.code_form.review.pulls.is_some() {
            return;
        }
        let Some(runs) = self.runs(&project) else {
            self.code_form.review.pulls = Some(Err("The Nexus app runs the sessions: review from there.".into()));
            return;
        };
        self.code_form.review.project = Some(project.clone());
        self.code_form.review.pulls = None;
        let read = runs.call("wb_pull_requests", json!({ "projectId": project }));
        self.code_form.review._read = Some(cx.spawn(async move |this, cx| {
            let result = read
                .await
                .unwrap_or_else(|_| Err("The read was lost.".into()))
                .and_then(|value| serde_json::from_value::<Vec<PullRequestInfo>>(value).map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                if this.code_form.review.project.as_ref() == Some(&project) {
                    this.code_form.review.pulls = Some(result);
                    cx.notify();
                }
            });
        }));
    }

    /// A pull request picked: the menu closes, and an empty first message becomes the obvious ask.
    fn pick_review(&mut self, number: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.code_form.review.number = Some(number);
        self.overlay.close_launch_menu();
        if self.composer_empty {
            self.composer
                .update(cx, |state, cx| state.set_value("Review this pull request", window, cx));
            self.composer_empty = false;
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// The list under the workspace menu's choices while "Review a pull request" is picked.
    pub(super) fn render_review_list(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let review = &self.code_form.review;
        let host = self.review_host();
        let request = host.request();
        let note = |text: String| {
            div()
                .px(px(10.))
                .py(px(6.))
                .text_size(px(12.5))
                .text_color(palette.muted)
                .child(text)
        };
        let field = h_flex()
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
                    review
                        .input
                        .as_ref()
                        .map(|(input, _)| Input::new(input).appearance(false)),
                ),
            );
        let mut list = v_flex().id("launch-review-list").max_h(px(280.)).overflow_y_scroll();
        let pulls: &[PullRequestInfo] = match &review.pulls {
            Some(Ok(pulls)) => pulls,
            _ => &[],
        };
        let (typed, other_repo, shown) = matches(pulls, &review.query, self.review_repo().as_deref());
        let listed = typed.is_some_and(|typed| shown.iter().any(|pull| pull.number == typed));
        if let Some(typed) = typed.filter(|_| !listed) {
            let reference = host.reference(typed);
            list = list.child(
                menu_row(
                    "launch-review-typed",
                    format!("Review {reference}").into(),
                    None,
                    review.number == Some(typed),
                    palette,
                )
                .on_click(cx.listener(move |this, _, window, cx| this.pick_review(typed, window, cx))),
            );
        }
        if other_repo {
            let repo = pulls
                .first()
                .and_then(|pull| repo_of_pull(&pull.url))
                .unwrap_or_default();
            list = list.child(note(format!(
                "That link is for another repository ({repo} is this project's)."
            )));
        }
        match &review.pulls {
            None => list = list.child(note(format!("Finding open {request}s…"))),
            Some(Err(error)) => list = list.child(note(error.clone()).text_color(palette.destructive)),
            Some(Ok(pulls)) if pulls.is_empty() && typed.is_none() => {
                list = list.child(note(format!("No open {request}s")))
            }
            Some(Ok(_)) if shown.is_empty() && typed.is_none() => {
                list = list.child(note("No match. Paste its number or link.".into()))
            }
            Some(Ok(_)) => {}
        }
        for pull in shown {
            let number = pull.number;
            list = list.child(
                menu_row(
                    SharedString::from(format!("launch-review-{number}")),
                    format!("{} {}", pull.host.reference(number), pull.title).into(),
                    Some(format!("{} · {}", pull.author, pull.head).into()),
                    review.number == Some(number),
                    palette,
                )
                .on_click(cx.listener(move |this, _, window, cx| this.pick_review(number, window, cx))),
            );
        }
        v_flex()
            .border_t_1()
            .border_color(palette.border)
            .mt(px(4.))
            .pt(px(4.))
            .child(field)
            .child(list)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::matches;
    use nexus_workbench::git::PullRequestInfo;

    fn pull(number: u64, title: &str) -> PullRequestInfo {
        PullRequestInfo {
            number,
            title: title.into(),
            url: format!("https://github.com/acme/nexus/pull/{number}"),
            author: "ana".into(),
            head: format!("ana/branch-{number}"),
            base: "main".into(),
            host: Default::default(),
        }
    }

    #[test]
    fn empty_or_failed_lists_cannot_accept_another_repositories_link() {
        let (typed, other, _) = matches(&[], "https://github.com/other/repo/pull/42", Some("acme/nexus"));
        assert_eq!((typed, other), (None, true));
        let (typed, other, _) = matches(&[], "https://github.com/acme/nexus/pull/42", None);
        assert_eq!((typed, other), (None, true));
        assert_eq!(matches(&[], "#42", None).0, Some(42));
        assert_eq!(
            matches(&[], "https://github.com/acme/nexus/pull/42", Some("acme/nexus")).0,
            Some(42)
        );
    }

    #[test]
    fn the_list_matches_words_numbers_and_this_repositorys_links() {
        let pulls = [pull(7, "Fix the retry"), pull(12, "Add a filter")];
        let numbers = |query| {
            let (typed, other, shown) = matches(&pulls, query, Some("acme/nexus"));
            (typed, other, shown.iter().map(|pull| pull.number).collect::<Vec<_>>())
        };
        assert_eq!(numbers(""), (None, false, vec![7, 12]));
        assert_eq!(numbers("RETRY"), (None, false, vec![7]));
        assert_eq!(numbers("branch-12"), (None, false, vec![12]));
        assert_eq!(numbers("#12"), (Some(12), false, vec![12]));
        assert_eq!(numbers("99"), (Some(99), false, vec![]));
        assert_eq!(
            numbers("https://github.com/acme/nexus/pull/7"),
            (Some(7), false, vec![7])
        );
        assert_eq!(numbers("https://github.com/other/repo/pull/7"), (None, true, vec![]));
    }
}
