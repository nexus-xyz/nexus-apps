//! A Code session's git panel, opened from its project and branch in the top bar, as the Nexus app has it
//! (`workbench/git-panel.tsx`, `git-panel-logic.ts`): the folder, the branch with how far ahead and behind it is, Commit
//! & push (with Commit only, Push and the pull request form beside it), and what isn't committed yet, which opens the
//! Workbench's Changes. It reads and writes through `nexus-workbench`, as the Nexus app does.
//!
//! The commit form's "Write it" drafts the message (`write_it.rs`).
//!
//! The branch row switches to another local branch or makes a new one, and a Pull row offers a pull (or a rebase when
//! both sides moved) once the remote is ahead, as the Nexus app's do. Their rules are `nexus_workbench::sync`'s.
//!
//! Not here yet: a folder of clones' repositories (listed, but not picked).

use crate::workspace::menu_item_hover;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::GitState;
use nexus_workbench::git::{Stat, Tracking};
use nexus_workbench::sync::{self, Pull};

use super::TopPanel;
use crate::code::{CodeSession, Status};
use crate::code_run::CodeRunner;
use crate::palette::Palette;
use crate::workspace::code_dock::menu_box;
use crate::workspace::side_pane::Tool;
use crate::workspace::write_it::write_button;
use crate::workspace::{Workspace, glyph};

/// What the panel and the pull request form ask of the core, apart from the screen so a test can say what each button
/// would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GitCall {
    /// The folder as git sees it, from the disk.
    State,
    /// The same after fetching the remote, so ahead and behind catch up (the core skips it mid-turn or within a minute).
    Fetch,
    /// Commit everything as `message`, then push when `push`; an empty message with `push` only pushes.
    Commit { message: String, push: bool },
    /// Commit as `title`, push and open (or update) the session's pull request.
    PullRequest { title: String, body: String },
    /// Check out another local branch.
    Switch { branch: String },
    /// Make a branch at the current commit and check it out.
    Create { branch: String },
    /// Bring the branch up to date with its upstream; `rebase` replays its own commits on top.
    Pull { rebase: bool },
}

/// What a [`GitCall`] answers.
pub(super) enum GitAnswer {
    Git(GitState),
    /// The pull request was opened or updated: the session now links it.
    Opened,
    Pulled(Pull),
}

impl GitCall {
    /// Whether it changes the folder: the panel waits for it, and takes no other write meanwhile.
    fn writes(&self) -> bool {
        !matches!(self, GitCall::State | GitCall::Fetch)
    }

    /// Ask the core about session `thread`'s folder, or `repo`, a repository directly inside it.
    pub(super) fn send(
        self,
        runner: &CodeRunner,
        thread: String,
        repo: Option<String>,
    ) -> futures::channel::oneshot::Receiver<Result<GitAnswer, String>> {
        runner.git(move |api| async move {
            let repo = repo.as_deref();
            match self {
                GitCall::State => api.git_state(&thread, repo).await.map(GitAnswer::Git),
                GitCall::Fetch => api.git_fetch(&thread, repo).await.map(GitAnswer::Git),
                GitCall::Commit { message, push } => {
                    api.commit(&thread, repo, &message, push).await.map(GitAnswer::Git)
                }
                GitCall::PullRequest { title, body } => api
                    .pull_request(&thread, &title, &body)
                    .await
                    .map(|_| GitAnswer::Opened),
                GitCall::Switch { branch } => api.switch_branch(&thread, repo, &branch).await.map(GitAnswer::Git),
                GitCall::Create { branch } => api.branch_create(&thread, repo, &branch).await.map(GitAnswer::Git),
                GitCall::Pull { rebase } => api.git_pull(&thread, repo, rebase).await.map(GitAnswer::Pulled),
            }
        })
    }
}

/// The commit form's submit: nothing without a message.
pub(super) fn commit_call(message: &str, push: bool) -> Option<GitCall> {
    let message = message.trim();
    (!message.is_empty()).then(|| GitCall::Commit {
        message: message.to_string(),
        push,
    })
}

/// "Push <branch>": a commit with no message pushes only.
pub(super) fn push_call() -> GitCall {
    GitCall::Commit {
        message: String::new(),
        push: true,
    }
}

/// The new-branch form's submit: nothing without a name.
pub(super) fn create_call(name: &str) -> Option<GitCall> {
    let name = name.trim();
    (!name.is_empty()).then(|| GitCall::Create {
        branch: name.to_string(),
    })
}

/// Picking `name` in the branch list: nothing for the branch already checked out.
pub(super) fn switch_call(git: &GitState, name: &str) -> Option<GitCall> {
    (git.branch.as_deref() != Some(name)).then(|| GitCall::Switch {
        branch: name.to_string(),
    })
}

/// Whether the panel offers a new branch: in a repository, and not in a session's own worktree, which keeps its
/// branch. `own_worktree`: the panel shows the session's folder, and the session has its own worktree.
pub(super) fn can_create(git: &GitState, own_worktree: bool) -> bool {
    git.repos.is_none() && !own_worktree
}

/// The checked-out branch against what it is compared with: its upstream, else (a worktree) the branch it was made from.
pub(super) fn tracked(git: &GitState) -> Option<&Tracking> {
    git.upstream.as_ref().or(git.base.as_ref())
}

/// The counts beside the branch, `↑1 ↓3`, and their words for a tooltip; none when there is nothing either way.
pub(super) fn ahead_behind(tracking: Option<&Tracking>) -> Option<(String, String)> {
    let tracking = tracking.filter(|tracking| tracking.ahead > 0 || tracking.behind > 0)?;
    let commits = |n: u32| format!("{n} commit{}", if n == 1 { "" } else { "s" });
    let mut text = Vec::new();
    let mut words = Vec::new();
    if tracking.ahead > 0 {
        text.push(format!("↑{}", tracking.ahead));
        words.push(format!("{} not on {} yet", commits(tracking.ahead), tracking.name));
    }
    if tracking.behind > 0 {
        text.push(format!("↓{}", tracking.behind));
        words.push(format!(
            "{} on {} not here yet",
            commits(tracking.behind),
            tracking.name
        ));
    }
    Some((text.join(" "), format!("{} (as of the last fetch)", words.join(", "))))
}

/// The panel's folder: the last part of its path.
fn folder_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).find(|part| !part.is_empty()).unwrap_or("")
}

/// The words for where the project's pull requests live: GitHub's pull requests, or GitLab's merge requests.
pub(in crate::workspace) fn request_word(remotes: &[String]) -> &'static str {
    if remotes.iter().any(|remote| remote.contains("gitlab")) {
        "merge request"
    } else {
        "pull request"
    }
}

pub(in crate::workspace) struct GitPanel {
    pub(super) id: String,
    /// A repository inside the session's folder the panel points at, by name; none: the folder itself.
    pub(super) repo: Option<String>,
    git: Option<GitState>,
    pub(super) problem: Option<SharedString>,
    working: bool,
    /// The commit form, while open: its message, and whether the branch is pushed after.
    commit: Option<(Entity<InputState>, bool)>,
    /// Commit & push's chevron menu, while open.
    more: bool,
    /// "Write it" is drafting the commit message.
    drafting: bool,
    /// The branch list, while open.
    branches: bool,
    pub(super) editors: Option<Vec<nexus_workbench::editors::Editor>>,
    pub(super) editor_menu: bool,
    pub(super) editor_choice: Option<String>,
    /// The new-branch form's name, while open.
    new_branch: Option<Entity<InputState>>,
    _subscriptions: Vec<Subscription>,
}

/// While the agent runs a turn, nothing that writes.
fn busy(session: &CodeSession) -> bool {
    matches!(session.status, Status::Running | Status::NeedsYou)
}

const WAIT: &str = "Wait for the agent to finish its turn";

impl Workspace {
    /// Open the git panel for session `id`, or close it when it is open.
    pub(super) fn toggle_git_panel(&mut self, id: &str, cx: &mut Context<Self>) {
        if matches!(self.overlay.code_top(), Some(TopPanel::Git(panel)) if panel.id == id) {
            self.overlay.close_code_top();
            cx.notify();
            return;
        }
        self.open_overlay(Overlay::CodeTop(TopPanel::Git(GitPanel {
            id: id.to_string(),
            repo: None,
            git: None,
            problem: None,
            working: false,
            commit: None,
            more: false,
            drafting: false,
            branches: false,
            editors: None,
            editor_menu: false,
            editor_choice: None,
            new_branch: None,
            _subscriptions: Vec::new(),
        })));
        self.load_editors(cx);
        match self.runner.clone() {
            // What is on disk at once, then again once the remote is fetched.
            Some(runner) => {
                self.git_call(&runner, id, GitCall::State, cx);
                self.git_call(&runner, id, GitCall::Fetch, cx);
            }
            None => {
                let note = self.read_only_note();
                if let Some(TopPanel::Git(panel)) = self.overlay.code_top_mut() {
                    panel.problem = Some(note);
                }
            }
        }
        cx.notify();
    }

    /// Ask the core, and show its answer only while the panel still shows session `id`.
    fn git_call(&mut self, runner: &CodeRunner, id: &str, call: GitCall, cx: &mut Context<Self>) {
        let writes = call.writes();
        let repo = match self.overlay.code_top() {
            Some(TopPanel::Git(panel)) => panel.repo.clone(),
            _ => None,
        };
        let answer = call.send(runner, id.to_string(), repo.clone());
        if writes && let Some(TopPanel::Git(panel)) = self.overlay.code_top_mut() {
            panel.working = true;
            panel.problem = None;
            panel.more = false;
            panel.branches = false;
        }
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let answer = answer.await.unwrap_or_else(|_| Err("It was cancelled.".into()));
            let _ = this.update(cx, |this, cx| {
                let Some(TopPanel::Git(panel)) = this.overlay.code_top_mut() else {
                    return;
                };
                // A commit started on one repository shows nothing on another picked meanwhile.
                if panel.id != id || panel.repo != repo {
                    return;
                }
                if writes {
                    panel.working = false;
                }
                match answer {
                    Ok(GitAnswer::Git(git)) => {
                        panel.git = Some(git);
                        if writes {
                            panel.commit = None;
                            panel.new_branch = None;
                        }
                    }
                    // A rebase that conflicted was undone, so the branch is as it was: say why.
                    Ok(GitAnswer::Pulled(pull)) => {
                        panel.git = Some(pull.git);
                        panel.problem = pull.conflict.map(|files| sync::conflict_text(&files).into());
                    }
                    Ok(GitAnswer::Opened) => {}
                    Err(error) => panel.problem = Some(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Point the panel at `repo` inside the session's folder, or back at the folder itself.
    fn pick_git_repo(&mut self, repo: Option<String>, cx: &mut Context<Self>) {
        let Some(TopPanel::Git(panel)) = self.overlay.code_top_mut() else {
            return;
        };
        panel.repo = repo;
        panel.git = None;
        panel.commit = None;
        panel.more = false;
        panel.drafting = false;
        panel.branches = false;
        panel.new_branch = None;
        panel.problem = None;
        let id = panel.id.clone();
        if let Some(runner) = self.runner.clone() {
            self.git_call(&runner, &id, GitCall::State, cx);
            self.git_call(&runner, &id, GitCall::Fetch, cx);
        }
        cx.notify();
    }

    fn open_commit_form(&mut self, push: bool, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Commit message"));
        let submit = cx.subscribe_in(&input, window, |this: &mut Workspace, _, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit_commit(cx);
            }
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        if let Some(TopPanel::Git(panel)) = self.overlay.code_top_mut() {
            panel.commit = Some((input, push));
            panel.drafting = false;
            panel.more = false;
            panel.branches = false;
            panel.new_branch = None;
            panel.problem = None;
            panel._subscriptions = vec![submit];
        }
        cx.notify();
    }

    fn submit_commit(&mut self, cx: &mut Context<Self>) {
        let Some(TopPanel::Git(panel)) = self.overlay.code_top() else {
            return;
        };
        let Some((input, push)) = &panel.commit else {
            return;
        };
        let id = panel.id.clone();
        let blocked = panel.working || self.session_by_id(&id).is_none_or(busy);
        let Some(call) = commit_call(input.read(cx).value().as_ref(), *push) else {
            return;
        };
        if blocked {
            return;
        }
        if let Some(runner) = self.runner.clone() {
            self.git_call(&runner, &id, call, cx);
        }
    }

    /// "Write it": the commit message drafted from what isn't committed. It lands only in the form it was asked from,
    /// on the same session and repository; on an error the message stays as it was.
    fn write_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(TopPanel::Git(panel)) = self.overlay.code_top() else {
            return;
        };
        let Some((input, _)) = &panel.commit else {
            return;
        };
        if panel.drafting || panel.working {
            return;
        }
        let original = input.read(cx).value().to_string();
        let (id, repo, input) = (panel.id.clone(), panel.repo.clone(), input.clone());
        let Some(work) = self.write_draft(&id, repo.clone(), nexus_workbench::draft::Kind::Commit) else {
            return;
        };
        if let Some(TopPanel::Git(panel)) = self.overlay.code_top_mut() {
            panel.drafting = true;
            panel.problem = None;
        }
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let drafted = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(TopPanel::Git(panel)) = this.overlay.code_top_mut() else {
                    return;
                };
                let here = panel.id == id
                    && panel.repo == repo
                    && panel.commit.as_ref().is_some_and(|(open, _)| *open == input);
                if !here {
                    return;
                }
                panel.drafting = false;
                if input.read(cx).value().as_ref() != original {
                    cx.notify();
                    return;
                }
                match drafted {
                    Ok(draft) => input.update(cx, |state, cx| state.set_value(draft.title, window, cx)),
                    Err(error) => panel.problem = Some(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs a write on the panel's folder, unless one is under way or the agent is mid-turn.
    fn git_write(&mut self, call: GitCall, cx: &mut Context<Self>) {
        let Some(TopPanel::Git(panel)) = self.overlay.code_top() else {
            return;
        };
        let id = panel.id.clone();
        if panel.working || self.session_by_id(&id).is_none_or(busy) {
            return;
        }
        if let Some(runner) = self.runner.clone() {
            self.git_call(&runner, &id, call, cx);
        }
    }

    fn open_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("New branch name"));
        let submit = cx.subscribe_in(&input, window, |this: &mut Workspace, _, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit_new_branch(cx);
            }
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        if let Some(TopPanel::Git(panel)) = self.overlay.code_top_mut() {
            panel.new_branch = Some(input);
            panel.branches = false;
            panel.commit = None;
            panel.problem = None;
            panel._subscriptions = vec![submit];
        }
        cx.notify();
    }

    fn submit_new_branch(&mut self, cx: &mut Context<Self>) {
        let Some(TopPanel::Git(panel)) = self.overlay.code_top() else {
            return;
        };
        let Some(call) = panel
            .new_branch
            .as_ref()
            .and_then(|input| create_call(input.read(cx).value().as_ref()))
        else {
            return;
        };
        self.git_write(call, cx);
    }

    fn push_only(&mut self, cx: &mut Context<Self>) {
        let Some(TopPanel::Git(panel)) = self.overlay.code_top() else {
            return;
        };
        let id = panel.id.clone();
        if let Some(runner) = self.runner.clone() {
            self.git_call(&runner, &id, push_call(), cx);
        }
    }

    /// The panel under the top bar's project and branch.
    pub(super) fn render_git_panel(
        &self,
        session: &CodeSession,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Some(TopPanel::Git(panel)) = self.overlay.code_top() else {
            return None;
        };
        if panel.id != session.id {
            return None;
        }
        let busy = busy(session);
        let ready = self.runner.is_some() && !busy && !panel.working;
        let git = panel.git.as_ref();
        let branch = git.and_then(|git| git.branch.clone());
        let row = |key: &'static str, icon: Lucide, label: SharedString, enabled: bool| {
            h_flex()
                .id(key)
                .h(px(30.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .when(enabled, |row| {
                    row.cursor_pointer().hover(|style| menu_item_hover(style, palette))
                })
                .when(!enabled, |row| row.text_color(palette.faint))
                .child(glyph(icon, palette.icon).size(px(15.)))
                .child(div().flex_1().min_w(px(0.)).truncate().child(label))
        };
        let gap = || div().h(px(6.));
        let mut body = menu_box("git-panel", 288., palette)
            .p(px(6.))
            .text_size(px(13.))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                if matches!(this.overlay.code_top(), Some(TopPanel::Git(_))) {
                    this.overlay.close_code_top();
                    cx.notify();
                }
            }));

        if panel.repo.is_some() {
            let project: SharedString = self
                .projects
                .iter()
                .find(|project| project.id == session.project_id)
                .map_or("Back".into(), |project| project.name.clone().into());
            body = body.child(
                row("git-back", Lucide::ChevronLeft, project, true)
                    .on_click(cx.listener(|this, _, _, cx| this.pick_git_repo(None, cx))),
            );
        }
        let folder: SharedString = git.map_or("…".into(), |git| folder_name(&git.path).to_string().into());
        let path: Option<SharedString> = git.map(|git| git.path.clone().into());
        body = body.child(
            row("git-folder", Lucide::Folder, folder, false)
                .text_color(palette.foreground)
                .when_some(path, |row, path| {
                    row.tooltip(move |window, cx| Tooltip::new(path.clone()).build(window, cx))
                }),
        );
        body = body.child(gap());

        let listing = git.is_some_and(|git| git.repos.is_some());
        if let Some(repos) = git.and_then(|git| git.repos.as_ref()) {
            if repos.is_empty() {
                body = body.child(note("No git repositories in this folder.", palette));
            }
            for (index, repo) in repos.iter().enumerate() {
                let name = repo.name.clone();
                body = body.child(
                    h_flex()
                        .id(("git-repo", index))
                        .h(px(30.))
                        .px(px(8.))
                        .gap(px(8.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .hover(|style| menu_item_hover(style, palette))
                        .on_click(cx.listener(move |this, _, _, cx| this.pick_git_repo(Some(name.clone()), cx)))
                        .child(glyph(Lucide::Folder, palette.icon).size(px(15.)))
                        .child(div().flex_none().max_w(px(140.)).truncate().child(repo.name.clone()))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_size(px(12.))
                                .font_family("monospace")
                                .text_color(palette.faint)
                                .child(repo.branch.clone().unwrap_or_else(|| "Detached HEAD".into())),
                        )
                        .children(
                            repo.uncommitted
                                .filter(|stat| stat.files > 0)
                                .map(|stat| render_stat(stat, palette)),
                        ),
                );
            }
        } else {
            let label: SharedString = match git {
                Some(git) => git.branch.clone().unwrap_or_else(|| "Detached HEAD".into()).into(),
                None => session.branch.clone().unwrap_or_else(|| "…".into()).into(),
            };
            let sync = git.and_then(|git| ahead_behind(tracked(git)));
            // The session's own worktree keeps its branch: no list, and no new branch.
            let own_worktree = panel.repo.is_none() && session.branch.is_some();
            let switchable = git.is_some_and(|git| git.branches.len() > 1);
            let creatable = git.is_some_and(|git| can_create(git, own_worktree));
            let pickable = switchable || creatable;
            let can_pick = pickable && ready;
            body = body.child(
                h_flex()
                    .id("git-branch")
                    .h(px(30.))
                    .px(px(8.))
                    .gap(px(8.))
                    .child(glyph(Lucide::GitBranch, palette.icon).size(px(15.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .font_family("monospace")
                            .text_size(px(12.5))
                            .child(label),
                    )
                    .children(sync.map(|(text, words)| {
                        div()
                            .id("git-sync")
                            .flex_none()
                            .font_family("monospace")
                            .text_size(px(12.))
                            .text_color(palette.muted)
                            .tooltip(move |window, cx| Tooltip::new(words.clone()).build(window, cx))
                            .child(text)
                    }))
                    .when(pickable, |row| {
                        row.child(
                            div()
                                .id("git-branch-pick")
                                .flex_none()
                                .size(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(6.))
                                .when(panel.branches, |button| button.bg(palette.selected))
                                .tooltip(move |window, cx| {
                                    Tooltip::new(if busy { WAIT } else { "Switch branch" }).build(window, cx)
                                })
                                .when(can_pick, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| menu_item_hover(style, palette))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(TopPanel::Git(panel)) = this.overlay.code_top_mut() {
                                                panel.branches = !panel.branches;
                                                panel.more = false;
                                            }
                                            cx.notify();
                                        }))
                                })
                                .when(!can_pick, |button| button.opacity(0.45))
                                .child(glyph(Lucide::ChevronDown, palette.icon).size(px(14.))),
                        )
                    }),
            );
            if panel.branches
                && let Some(git) = git
            {
                body = body.child(self.render_branch_list(git, creatable, palette, cx));
            }
            if let Some(input) = &panel.new_branch {
                let can = ready && !input.read(cx).value().trim().is_empty();
                body = body.child(
                    v_flex()
                        .gap(px(6.))
                        .p(px(4.))
                        .child(
                            div()
                                .h(px(30.))
                                .px(px(10.))
                                .flex()
                                .items_center()
                                .rounded(px(8.))
                                .bg(palette.selected)
                                .font_family("monospace")
                                .child(Input::new(input).appearance(false)),
                        )
                        .child(
                            h_flex().justify_end().child(
                                submit_button("git-branch-create", "Create and switch", can, palette)
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(if busy {
                                            WAIT
                                        } else {
                                            "Made at the current commit and switched to; uncommitted changes come along"
                                        })
                                        .build(window, cx)
                                    })
                                    .when(can, |button| {
                                        button.on_click(cx.listener(|this, _, _, cx| this.submit_new_branch(cx)))
                                    }),
                            ),
                        ),
                );
            }
            if let Some(offer) = git.and_then(sync::pull_offer) {
                let rebase = offer.rebase;
                let title: SharedString = if busy { WAIT.into() } else { offer.title.into() };
                body = body.child(
                    row("git-pull", Lucide::ArrowDown, offer.label.into(), ready)
                        .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
                        .when(ready, |row| {
                            row.on_click(
                                cx.listener(move |this, _, _, cx| this.git_write(GitCall::Pull { rebase }, cx)),
                            )
                        }),
                );
            }
        }

        if !listing {
            body = match &panel.commit {
                Some((input, push)) => {
                    let can = ready && !input.read(cx).value().trim().is_empty();
                    body.child(
                        v_flex()
                            .gap(px(6.))
                            .p(px(4.))
                            .child(
                                div()
                                    .h(px(30.))
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(8.))
                                    .bg(palette.selected)
                                    .child(Input::new(input).appearance(false)),
                            )
                            .child(
                                h_flex()
                                    .justify_end()
                                    .gap(px(4.))
                                    .child({
                                        let reason = self.account_control_reason();
                                        let write = reason.is_none() && !panel.drafting && !panel.working;
                                        write_button("git-commit-write", panel.drafting, write, reason, palette).when(
                                            write,
                                            |button| {
                                                button.on_click(
                                                    cx.listener(|this, _, window, cx| this.write_commit(window, cx)),
                                                )
                                            },
                                        )
                                    })
                                    .child(
                                        submit_button(
                                            "git-commit-submit",
                                            if *push { "Commit & push" } else { "Commit" },
                                            can,
                                            palette,
                                        )
                                        .when(busy, |button| {
                                            button.tooltip(|window, cx| Tooltip::new(WAIT).build(window, cx))
                                        })
                                        .when(can, |button| {
                                            button.on_click(cx.listener(|this, _, _, cx| this.submit_commit(cx)))
                                        }),
                                    ),
                            ),
                    )
                }
                None if session.review && panel.repo.is_none() => body.child(
                    row(
                        "git-commit",
                        Lucide::ArrowUp,
                        "Commit".into(),
                        ready && branch.is_some(),
                    )
                    .when(busy, |row| {
                        row.tooltip(|window, cx| Tooltip::new(WAIT).build(window, cx))
                    })
                    .when(ready && branch.is_some(), |row| {
                        row.on_click(cx.listener(|this, _, window, cx| this.open_commit_form(false, window, cx)))
                    }),
                ),
                None => {
                    let can = ready && branch.is_some();
                    body.child(
                        h_flex()
                            .gap(px(2.))
                            .child(
                                row("git-commit-push", Lucide::ArrowUp, "Commit & push".into(), can)
                                    .flex_1()
                                    .when(busy, |row| {
                                        row.tooltip(|window, cx| Tooltip::new(WAIT).build(window, cx))
                                    })
                                    .when(can, |row| {
                                        row.on_click(
                                            cx.listener(|this, _, window, cx| this.open_commit_form(true, window, cx)),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .id("git-more")
                                    .flex_none()
                                    .size(px(30.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(8.))
                                    .when(panel.more, |button| button.bg(palette.selected))
                                    .when(ready && git.is_some(), |button| {
                                        button
                                            .cursor_pointer()
                                            .hover(|style| menu_item_hover(style, palette))
                                            .tooltip(|window, cx| Tooltip::new("More git actions").build(window, cx))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if let Some(TopPanel::Git(panel)) = this.overlay.code_top_mut() {
                                                    panel.more = !panel.more;
                                                }
                                                cx.notify();
                                            }))
                                    })
                                    .when(!(ready && git.is_some()), |button| button.opacity(0.45))
                                    .child(glyph(Lucide::ChevronDown, palette.icon).size(px(14.))),
                            ),
                    )
                    .when(panel.more, |body| {
                        body.child(self.render_git_more(session, branch.clone(), palette, cx))
                    })
                }
            };
            // The Changes view shows the session's own folder.
            let stat = git
                .and_then(|git| git.uncommitted)
                .filter(|stat| stat.files > 0 && panel.repo.is_none());
            body = body.child(
                row("git-changes", Lucide::FileDiff, "Changes".into(), stat.is_some())
                    .children(stat.map(|stat| render_stat(stat, palette)))
                    .when(stat.is_some(), |row| {
                        row.on_click(cx.listener(|this, _, window, cx| {
                            this.overlay.close_code_top();
                            this.pick_tool(Tool::Changes, window, cx);
                            cx.notify();
                        }))
                    }),
            );
        }
        body = body.child(self.render_editor_picker(session, panel, palette, cx));
        body = body.when_some(panel.problem.clone(), |body, problem| {
            body.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .text_color(palette.destructive)
                    .child(problem),
            )
        });
        Some(popover(body, palette))
    }

    /// The branch row's list: the local branches, the checked-out one ticked, then New branch… where one can be made.
    fn render_branch_list(
        &self,
        git: &GitState,
        creatable: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let item = |id: ElementId| {
            h_flex()
                .id(id)
                .h(px(28.))
                .pl(px(31.))
                .pr(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| menu_item_hover(style, palette).text_color(palette.foreground))
        };
        let mut list = v_flex().id("git-branches").max_h(px(240.)).overflow_y_scroll();
        for (index, name) in git.branches.iter().enumerate() {
            let current = git.branch.as_deref() == Some(name.as_str());
            let call = switch_call(git, name);
            list = list.child(
                item(("git-branch-item", index).into())
                    .text_color(if current { palette.foreground } else { palette.muted })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .font_family("monospace")
                            .text_size(px(12.5))
                            .child(name.clone()),
                    )
                    .when(current, |row| {
                        row.child(glyph(Lucide::Check, palette.icon).size(px(14.)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| match call.clone() {
                        Some(call) => this.git_write(call, cx),
                        None => {
                            if let Some(TopPanel::Git(panel)) = this.overlay.code_top_mut() {
                                panel.branches = false;
                            }
                            cx.notify();
                        }
                    })),
            );
        }
        if creatable {
            list = list.child(
                item("git-new-branch".into())
                    .text_color(palette.muted)
                    .child("New branch…")
                    .on_click(cx.listener(|this, _, window, cx| this.open_new_branch(window, cx))),
            );
        }
        list
    }

    /// Commit & push's chevron: Commit only, Push the branch, and the pull request form.
    fn render_git_more(
        &self,
        session: &CodeSession,
        branch: Option<String>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let item = |key: &'static str, label: SharedString| {
            h_flex()
                .id(key)
                .h(px(28.))
                .pl(px(31.))
                .pr(px(8.))
                .rounded(px(8.))
                .text_color(palette.muted)
                .cursor_pointer()
                .hover(|style| menu_item_hover(style, palette).text_color(palette.foreground))
                .child(div().truncate().child(label))
        };
        let word = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .map_or("pull request", |project| request_word(&project.remotes));
        let in_repo = matches!(self.overlay.code_top(), Some(TopPanel::Git(panel)) if panel.repo.is_some());
        v_flex()
            .child(
                item("git-commit-only", "Commit only".into())
                    .on_click(cx.listener(|this, _, window, cx| this.open_commit_form(false, window, cx))),
            )
            .children(branch.map(|branch| {
                item("git-push", format!("Push {branch}").into())
                    .on_click(cx.listener(|this, _, _, cx| this.push_only(cx)))
            }))
            // A pull request comes from a session's own worktree, not its project's folder.
            .when(session.branch.is_some() && !in_repo, |menu| {
                let open = nexus_pull_requests::chip::branch_pull_open(
                    session.pull_request.as_deref(),
                    crate::pr_watch::known(&session.id).as_ref(),
                );
                let label = if open {
                    format!("Push to the {word}")
                } else {
                    format!("Open a {word}")
                };
                let id = session.id.clone();
                menu.child(
                    item("git-open-pr", label.into()).on_click(cx.listener(move |this, _, window, cx| {
                        this.open_pr_form(&id, window, cx);
                    })),
                )
            })
    }
}

fn note(text: &'static str, palette: Palette) -> Div {
    div()
        .px(px(8.))
        .py(px(6.))
        .text_size(px(12.))
        .text_color(palette.faint)
        .child(text)
}

/// What a commit would take, as +added −removed lines.
fn render_stat(stat: Stat, palette: Palette) -> impl IntoElement {
    h_flex()
        .flex_none()
        .gap(px(6.))
        .font_family("monospace")
        .text_size(px(12.))
        .child(
            div()
                .text_color(gpui_kit::rgb(0x3a8f3e))
                .child(format!("+{}", stat.insertions)),
        )
        .child(
            div()
                .text_color(palette.destructive)
                .child(format!("−{}", stat.deletions)),
        )
}

/// A form's primary button.
pub(super) fn submit_button(
    id: &'static str,
    label: impl Into<SharedString>,
    enabled: bool,
    palette: Palette,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.))
        .px(px(12.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .bg(palette.primary)
        .text_color(palette.primary_foreground)
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .when(!enabled, |button| button.opacity(0.4))
        .when(enabled, |button| {
            button.cursor_pointer().hover(|style| style.opacity(0.88))
        })
        .child(label.into())
}

/// A top bar panel, anchored under its button at the button's right edge.
pub(super) fn popover(body: impl IntoElement, _palette: Palette) -> AnyElement {
    div()
        .absolute()
        .right_0()
        .top(px(28.))
        .child(
            deferred(
                anchored()
                    .anchor(Anchor::TopRight)
                    .offset(point(px(0.), px(4.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(body),
            )
            .with_priority(1),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{
        GitCall, GitState, Tracking, ahead_behind, can_create, commit_call, create_call, folder_name, push_call,
        request_word, switch_call, tracked,
    };

    fn tracking(ahead: u32, behind: u32) -> Tracking {
        Tracking {
            name: "origin/main".into(),
            ahead,
            behind,
        }
    }

    #[test]
    fn commit_and_push_call_the_core_with_the_trimmed_message() {
        assert_eq!(
            commit_call("  Fix the meter \n", true),
            Some(GitCall::Commit {
                message: "Fix the meter".into(),
                push: true
            })
        );
        assert_eq!(
            commit_call("Fix", false),
            Some(GitCall::Commit {
                message: "Fix".into(),
                push: false
            })
        );
        // No message: nothing is called.
        assert_eq!(commit_call("   ", true), None);
    }

    #[test]
    fn a_new_branch_needs_a_name_and_the_checked_out_branch_is_not_switched_to() {
        assert_eq!(create_call("   "), None);
        assert_eq!(
            create_call("  feature/x "),
            Some(GitCall::Create {
                branch: "feature/x".into()
            })
        );
        let git = GitState {
            path: "/repo".into(),
            branch: Some("main".into()),
            branches: vec!["main".into(), "feature".into()],
            uncommitted: None,
            repos: None,
            upstream: None,
            base: None,
        };
        assert_eq!(switch_call(&git, "main"), None);
        assert_eq!(
            switch_call(&git, "feature"),
            Some(GitCall::Switch {
                branch: "feature".into()
            })
        );
        assert!(can_create(&git, false));
        // A session's own worktree keeps its branch, and a folder of repositories has none of its own.
        assert!(!can_create(&git, true));
        let folder = GitState {
            repos: Some(Vec::new()),
            ..git
        };
        assert!(!can_create(&folder, false));
        assert!(GitCall::Pull { rebase: false }.writes() && !GitCall::Fetch.writes());
    }

    #[test]
    fn push_alone_is_a_commit_with_no_message() {
        assert_eq!(
            push_call(),
            GitCall::Commit {
                message: String::new(),
                push: true
            }
        );
    }

    #[test]
    fn ahead_and_behind_read_as_the_nexus_app_has_them() {
        assert_eq!(ahead_behind(Some(&tracking(0, 0))), None);
        assert_eq!(ahead_behind(None), None);
        assert_eq!(
            ahead_behind(Some(&tracking(1, 3))),
            Some((
                "↑1 ↓3".into(),
                "1 commit not on origin/main yet, 3 commits on origin/main not here yet (as of the last fetch)".into()
            ))
        );
        assert_eq!(
            ahead_behind(Some(&tracking(0, 2))).map(|(text, _)| text),
            Some("↓2".into())
        );
    }

    #[test]
    fn the_branch_is_compared_with_its_upstream_else_its_base() {
        let mut git = GitState {
            path: "/code/nexus/".into(),
            branch: Some("work".into()),
            branches: vec![],
            uncommitted: None,
            repos: None,
            upstream: None,
            base: Some(tracking(2, 0)),
        };
        assert_eq!(tracked(&git).map(|tracking| tracking.ahead), Some(2));
        git.upstream = Some(tracking(0, 1));
        assert_eq!(tracked(&git).map(|tracking| tracking.behind), Some(1));
        assert_eq!(folder_name(&git.path), "nexus");
    }

    #[test]
    fn gitlab_projects_say_merge_request() {
        assert_eq!(request_word(&["git@gitlab.com:a/b.git".into()]), "merge request");
        assert_eq!(request_word(&["https://github.com/a/b".into()]), "pull request");
        assert_eq!(request_word(&[]), "pull request");
    }
}
