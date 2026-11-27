//! The pull request form of a session with its own worktree, as the Nexus app's top bar has it (`workbench/topbar.tsx`):
//! a title (also the commit message for what isn't committed yet) and a description, then one button that commits,
//! pushes the branch and opens its pull request into the worktree's base (or, once open, pushes to it), with the
//! person's GitHub CLI sign-in. The base is the worktree's, as in the Nexus app; it has no draft switch, and neither does
//! the core's call. "Write it" drafts the title and description from the branch (`write_it.rs`).

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::git_panel::{GitAnswer, GitCall, popover, request_word, submit_button};
use crate::code::{CodeSession, Status};
use crate::github_cli::{self, Block};
use crate::palette::Palette;
use crate::setup;
use crate::workspace::Workspace;
use crate::workspace::code_dock::menu_box;
use crate::workspace::write_it::write_button;

pub(in crate::workspace) struct PrForm {
    pub(super) id: String,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    /// The branch it goes into, short (`main`), once the worktree's state is read.
    base: Option<String>,
    problem: Option<Problem>,
    working: bool,
    /// "Write it" is drafting the title and description.
    drafting: bool,
    _subscriptions: Vec<Subscription>,
}

impl PrForm {
    /// The GitHub CLI's setup line is up: the person fixes it outside the form, which stays open meanwhile.
    fn waits_on_setup(&self) -> bool {
        matches!(self.problem, Some(Problem::Setup(_)))
    }
}

#[cfg(test)]
impl PrForm {
    /// The description as typed.
    pub(in crate::workspace) fn description(&self, cx: &App) -> String {
        self.body.read(cx).value().to_string()
    }

    /// Types `text` into the description.
    pub(in crate::workspace) fn type_description(&self, text: &str, window: &mut Window, cx: &mut App) {
        self.body
            .update(cx, |state, cx| state.set_value(text.to_string(), window, cx));
    }

    /// Marks the create as sent, as the submit does.
    pub(in crate::workspace) fn start_create(&mut self) {
        self.working = true;
    }

    pub(in crate::workspace) fn working(&self) -> bool {
        self.working
    }
}

/// What the form says under its fields.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Problem {
    /// Why the form can't open one from this window: quiet text.
    Note(SharedString),
    /// The GitHub CLI is missing or signed out: its setup line, whose fix submits again when it ends.
    Setup(Block),
    /// The create or "Write it" failed: the red ✕, its first line, Try again and Copy.
    Failed { text: SharedString, retry: Retry },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Retry {
    Submit,
    Write,
}

/// The create failed with `error`: setup when it is the GitHub CLI's (`Block::of_text`), else a failure.
fn submit_problem(error: String) -> Problem {
    match Block::of_text(&error) {
        Some(block) => Problem::Setup(block),
        None => Problem::Failed {
            text: error.into(),
            retry: Retry::Submit,
        },
    }
}

/// The setup lines' scope: their keys are `pr-form:gh-auth` and `pr-form:gh-install`.
const SCOPE: &str = "pr-form";

/// What the setup line's first sentence ends with: "Sign in to GitHub to open this pull request."
fn setup_doing(open: bool, word: &str) -> String {
    if open {
        format!("push to this {word}")
    } else {
        format!("open this {word}")
    }
}

/// A failure's first words: what didn't happen.
fn failed_title(retry: Retry, open: bool, word: &str) -> String {
    match (retry, open) {
        (Retry::Write, _) => "Couldn't write the title and description.".into(),
        (Retry::Submit, true) => format!("Couldn't push to the {word}."),
        (Retry::Submit, false) => format!("Couldn't open the {word}."),
    }
}

/// The form's submit: nothing without a title.
pub(super) fn pull_request_call(title: &str, body: &str) -> Option<GitCall> {
    let title = title.trim();
    (!title.is_empty()).then(|| GitCall::PullRequest {
        title: title.to_string(),
        body: body.to_string(),
    })
}

/// `origin/main` as the form names it: `main`.
fn short_base(name: &str) -> &str {
    name.split_once('/').map_or(name, |(_, rest)| rest)
}

/// What the button will do, in the Nexus app's words.
fn hint(branch: &str, base: Option<&str>, open: bool, word: &str) -> String {
    let cli = if word == "merge request" {
        "GitLab CLI"
    } else {
        "GitHub CLI"
    };
    let what = if open {
        format!("updates its {word}")
    } else {
        format!("opens a {word} into {}", base.unwrap_or("its base"))
    };
    format!("Commits, pushes {branch} and {what}, with your {cli} sign-in.")
}

impl Workspace {
    /// Open the pull request form for session `id`, its title the session's.
    pub(in crate::workspace) fn open_pr_form(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session_by_id(id) else {
            return;
        };
        let session_title = session.title.clone();
        let title = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(session_title, window, cx);
            state
        });
        let body = cx.new(|cx| TextareaState::new(window, cx).rows(5));
        let submit = cx.subscribe_in(&title, window, |this: &mut Workspace, _, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit_pr(cx);
            }
        });
        title.update(cx, |state, cx| state.focus(window, cx));
        // The form closes any open overlay (the git panel it was opened from, too), but is not one: a popup opened
        // over it from the keyboard (the palette, a link menu) leaves the typed title, description and a running
        // create in place, as before the one-overlay slot (`overlay.rs`).
        self.close_overlay();
        self.pr_form = Some(PrForm {
            id: id.to_string(),
            title,
            body,
            base: None,
            problem: None,
            working: false,
            drafting: false,
            _subscriptions: vec![submit],
        });
        // The base it goes into, from the worktree's state.
        match self.runner.clone() {
            Some(runner) => {
                let answer = GitCall::State.send(&runner, id.to_string(), None);
                let id = id.to_string();
                cx.spawn(async move |this, cx| {
                    let Ok(Ok(GitAnswer::Git(git))) = answer.await else {
                        return;
                    };
                    let _ = this.update(cx, |this, cx| {
                        if let Some(form) = this.pr_form.as_mut()
                            && form.id == id
                        {
                            form.base = git.base.map(|base| short_base(&base.name).to_string());
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            None => {
                let note = self.read_only_note();
                if let Some(form) = self.pr_form.as_mut() {
                    form.problem = Some(Problem::Note(note));
                }
            }
        }
        cx.notify();
    }

    /// "Write it": the title and description drafted from the branch's commits and diff. They land only in the form
    /// they were asked from; on an error both stay as they were.
    fn write_pr(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.pr_form.as_ref() else {
            return;
        };
        if form.drafting {
            return;
        }
        let original_title = form.title.read(cx).value().to_string();
        let original_body = form.body.read(cx).value().to_string();
        let (id, title) = (form.id.clone(), form.title.clone());
        let Some(work) = self.write_draft(&id, None, nexus_workbench::draft::Kind::PullRequest) else {
            return;
        };
        if let Some(form) = self.pr_form.as_mut() {
            form.drafting = true;
            form.problem = None;
        }
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let drafted = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(form) = this.pr_form.as_mut() else {
                    return;
                };
                if form.id != id || form.title != title {
                    return;
                }
                form.drafting = false;
                if form.title.read(cx).value().as_ref() != original_title
                    || form.body.read(cx).value().as_ref() != original_body
                {
                    cx.notify();
                    return;
                }
                match drafted {
                    Ok(draft) => {
                        form.title
                            .update(cx, |state, cx| state.set_value(draft.title, window, cx));
                        form.body
                            .update(cx, |state, cx| state.set_value(draft.body, window, cx));
                    }
                    Err(error) => {
                        form.problem = Some(Problem::Failed {
                            text: error.into(),
                            retry: Retry::Write,
                        })
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn submit_pr(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.pr_form.as_mut() else {
            return;
        };
        let Some(call) = pull_request_call(
            form.title.read(cx).value().as_ref(),
            form.body.read(cx).value().as_ref(),
        ) else {
            return;
        };
        let Some(runner) = self.runner.clone() else {
            return;
        };
        if form.working {
            return;
        }
        form.working = true;
        form.problem = None;
        let id = form.id.clone();
        let answer = call.send(&runner, id.clone(), None);
        cx.spawn(async move |this, cx| {
            let answer = answer.await.unwrap_or_else(|_| Err("It was cancelled.".into()));
            let _ = this.update(cx, |this, cx| {
                match answer {
                    Ok(_) => {
                        if matches!(this.pr_form.as_ref(), Some(form) if form.id == id) {
                            this.pr_form = None;
                        }
                        this.refresh_code(cx);
                    }
                    Err(error) => match this.pr_form.as_mut() {
                        Some(form) if form.id == id => {
                            form.working = false;
                            let problem = submit_problem(error);
                            if let Problem::Setup(block) = &problem {
                                let block = block.clone();
                                form.problem = Some(problem);
                                this.retry_after(&block, cx);
                            } else {
                                form.problem = Some(problem);
                            }
                        }
                        _ => this.composer_note = Some(error.into()),
                    },
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// When `block`'s fix ends well, the form submits again by itself, with what is typed in it. Once per fix.
    fn retry_after(&mut self, block: &Block, cx: &mut Context<Self>) {
        let key = block.key(SCOPE);
        if setup::state::waiting(&key, cx) {
            return;
        }
        let this = cx.entity().downgrade();
        setup::on_done(key, cx, move |_, cx| {
            let retried = this.update(cx, |this, cx| {
                let blocked = this
                    .pr_form
                    .as_ref()
                    .is_some_and(|form| matches!(form.problem, Some(Problem::Setup(_))));
                if blocked {
                    this.submit_pr(cx);
                }
            });
            if let Err(error) = retried {
                eprintln!("nexus-desktop-gpui: pull request form: {error}");
            }
        });
    }

    /// What the form says under its fields: a quiet note, the GitHub CLI's setup line, or a failure.
    /// `open`: the branch's request is open, and the form pushes to it; `word` names it ("pull request").
    fn render_pr_problem(
        &self,
        problem: Option<&Problem>,
        open: bool,
        word: &str,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let line = match problem {
            Some(Problem::Note(note)) => {
                return Some(
                    div()
                        .text_size(px(12.))
                        .text_color(palette.muted)
                        .child(note.clone())
                        .into_any_element(),
                );
            }
            None => None,
            Some(Problem::Setup(block)) => Some(block.line(SCOPE, &setup_doing(open, word)).main()),
            Some(Problem::Failed { text, retry }) => {
                let this = cx.entity().downgrade();
                let retry = *retry;
                Some(github_cli::failed(
                    "pr-form:failed",
                    &failed_title(retry, open, word),
                    text,
                    move |window, cx| {
                        let retried = this.update(cx, |this, cx| match retry {
                            Retry::Submit => this.submit_pr(cx),
                            Retry::Write => this.write_pr(window, cx),
                        });
                        if let Err(error) = retried {
                            eprintln!("nexus-desktop-gpui: pull request form: {error}");
                        }
                    },
                ))
            }
        };
        let fixer = self.fixer(cx);
        setup::slot("pr-form-problem", line, &fixer, palette, cx)
    }

    /// The form under the top bar's pull request button.
    pub(super) fn render_pr_form(
        &self,
        session: &CodeSession,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let form = self.pr_form.as_ref()?;
        if form.id != session.id {
            return None;
        }
        let word = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .map_or("pull request", |project| request_word(&project.remotes));
        // The branch's pull request is open, however it was opened: the form pushes to it.
        let open = nexus_pull_requests::chip::branch_pull_open(
            session.pull_request.as_deref(),
            crate::pr_watch::known(&session.id).as_ref(),
        );
        let busy = matches!(session.status, Status::Running | Status::NeedsYou);
        // While the GitHub CLI's fix is the way on, it is the form's one white button; the form submits after it.
        let setup = matches!(&form.problem, Some(Problem::Setup(block)) if block.runs_here());
        let can =
            self.runner.is_some() && !form.working && !busy && !setup && !form.title.read(cx).value().trim().is_empty();
        let problem = self.render_pr_problem(form.problem.as_ref(), open, word, palette, cx);
        let label = |text: &'static str| div().text_size(px(12.)).text_color(palette.muted).child(text);
        let field = |child: AnyElement| {
            div()
                .px(px(10.))
                .py(px(6.))
                .rounded(px(8.))
                .bg(palette.selected)
                .child(child)
        };
        let button = if form.working {
            "Opening…".to_string()
        } else if open {
            "Push".to_string()
        } else {
            format!("Open {word}")
        };
        let body = menu_box("pr-form", 384., palette)
            .p(px(10.))
            .gap(px(8.))
            .text_size(px(13.))
            // Not while a setup line waits on its fix: the person answers it in the Workbench terminal, and the form
            // keeps what they typed.
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                if matches!(this.pr_form.as_ref(), Some(form) if !form.working && !form.waits_on_setup()) {
                    this.pr_form = None;
                    cx.notify();
                }
            }))
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(label("Title, also the commit message for what isn't committed yet"))
                    .child(field(Input::new(&form.title).appearance(false).into_any_element())),
            )
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(label("Description"))
                    .child(field(Textarea::new(&form.body).appearance(false).into_any_element())),
            )
            .child(div().text_size(px(12.)).text_color(palette.faint).child(hint(
                session.branch.as_deref().unwrap_or("the branch"),
                form.base.as_deref(),
                open,
                word,
            )))
            .children(problem)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(4.))
                    .child({
                        let reason = self.account_control_reason();
                        let write = reason.is_none() && self.runner.is_some() && !form.drafting;
                        write_button("pr-write", form.drafting, write, reason, palette).when(write, |button| {
                            button.on_click(cx.listener(|this, _, window, cx| this.write_pr(window, cx)))
                        })
                    })
                    .child(submit_button("pr-submit", button, can, palette).when(can, |button| {
                        button.on_click(cx.listener(|this, _, _, cx| this.submit_pr(cx)))
                    })),
            );
        Some(popover(body, palette))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Block, GitCall, Problem, Retry, failed_title, hint, pull_request_call, setup_doing, short_base, submit_problem,
    };
    use nexus_workbench::source_host::Host;

    #[test]
    fn a_missing_or_signed_out_github_cli_is_setup_and_anything_else_a_failure() {
        let signed_out = Host::GitHub.error("To get started with GitHub CLI, please run:  gh auth login");
        assert_eq!(submit_problem(signed_out), Problem::Setup(Block::SignedOut));
        assert!(matches!(
            submit_problem(Host::GitHub.needs("Opening a pull request")),
            Problem::Setup(Block::Missing(_))
        ));
        assert_eq!(
            submit_problem("! [rejected] main -> main (fetch first)".into()),
            Problem::Failed {
                text: "! [rejected] main -> main (fetch first)".into(),
                retry: Retry::Submit
            }
        );
    }

    #[test]
    fn the_lines_say_what_the_form_was_doing() {
        assert_eq!(setup_doing(false, "pull request"), "open this pull request");
        assert_eq!(setup_doing(true, "pull request"), "push to this pull request");
        assert_eq!(
            failed_title(Retry::Submit, false, "pull request"),
            "Couldn't open the pull request."
        );
        assert_eq!(
            failed_title(Retry::Submit, true, "merge request"),
            "Couldn't push to the merge request."
        );
    }

    #[test]
    fn create_calls_the_core_with_the_trimmed_title_and_the_body_as_written() {
        assert_eq!(
            pull_request_call("  Add the meter ", "Why\n\nIt helps.\n"),
            Some(GitCall::PullRequest {
                title: "Add the meter".into(),
                body: "Why\n\nIt helps.\n".into()
            })
        );
        // No title: nothing is called.
        assert_eq!(pull_request_call(" ", "body"), None);
    }

    #[test]
    fn the_hint_names_the_branch_the_base_and_the_cli() {
        assert_eq!(short_base("origin/main"), "main");
        assert_eq!(short_base("main"), "main");
        assert_eq!(
            hint("ada/x", Some("main"), false, "pull request"),
            "Commits, pushes ada/x and opens a pull request into main, with your GitHub CLI sign-in."
        );
        assert_eq!(
            hint("x", None, true, "merge request"),
            "Commits, pushes x and updates its merge request, with your GitLab CLI sign-in."
        );
    }
}
