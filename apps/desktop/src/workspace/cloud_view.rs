//! Cloud sessions on the Code screen, as the Nexus app's `new-session.tsx`, `cloud-ui.tsx`, `cloud-repos.tsx` and
//! `archive.ts` draw them: Cloud in the new session's "where it runs" menu, the card that stands in for the composer
//! until the setup is done, the repository picker, the session's machine coming up (one line above the composer with
//! its step or the queue's line, Try again and End), waking a sleeping one when the person types or sends in its
//! session (never when it only opens: people often open a session just to read it), the messages
//! typed while it starts, and the end of a cloud session, which deletes its machine after asking when it holds
//! commits that are on no remote. The lifecycle itself is `crate::cloud`'s.

use crate::workspace::overlay::{Overlay, OverlaySlot};
use std::collections::HashSet;

use futures::channel::oneshot;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::code_dock::text_button;
use super::code_view::{ASLEEP_UNREAD, CodeView, elapsed};
use super::new_code::LaunchMenu;
use super::{SettingsSection, View, Workspace, glyph};
use crate::cloud::setup_line::{self, Blocker};
use crate::cloud::waiting::{Message, Push, Waiting};
use crate::cloud::words::{self, Check, End, StartLine};
use crate::cloud::{CloudAccount, CloudRepo, CloudStart};
use crate::code_run::{CodeEvent, NewSession};
use crate::machines::{CloudView, HistoryRead, NOT_HERE};
use crate::pages::PageId;
use crate::pages::ui;
use crate::palette::Palette;

/// Why a cloud session cannot start while it has no repository.
const PICK_FIRST: &str = "Pick a repository first.";

/// The cloud parts of the Code screen.
pub(super) struct CloudUi {
    /// Cloud is picked in the new session's "where it runs" menu.
    pub on: bool,
    /// The repository picked for the new cloud session.
    pub repo: Option<CloudRepo>,
    /// A repository picked for this computer that has no copy here: the new session offers to clone it, or to run it
    /// in the cloud.
    pub missing: Option<CloudRepo>,
    /// The server `missing` was picked for; None for this computer.
    pub missing_on: Option<String>,
    repos: Option<Result<Vec<CloudRepo>, SharedString>>,
    search: Entity<InputState>,
    pub query: String,
    /// The project menu's lit row, for Up, Down and Enter from its search (`project_picks`).
    pub cursor: usize,
    /// The project menu's list, scrolled to the lit row.
    pub list_scroll: ScrollHandle,
    /// The project menu's scope while something is typed: all rows, the projects, or GitHub's repositories.
    pub scope: super::project_menu::Scope,
    /// The project menu shows every GitHub repository, not only the latest pushed ("Show all").
    pub github_all: bool,
    /// The person's cloud setup on this computer (`cloud_account`).
    pub account: Option<Result<CloudAccount, SharedString>>,
    /// The question before a machine goes with commits that are on no remote.
    pub question: Option<Question>,
    /// The new cloud session starting now, one at a time: ending it drops its waiting messages.
    pub starting: Option<CloudStart>,
    /// A start that failed while another view was open: its messages wait here for New session, so they are not lost.
    kept: Option<KeptStart>,
    /// The sleeping machines that typing in their session's composer woke: each wakes once per sleep, and leaves this
    /// once it is seen awake (`settle_wakes`).
    woken: HashSet<String>,
    /// The sleeping machines a message was sent to: their session shows as working until the machine runs and takes
    /// it, or its wake fails (`settle_wakes`).
    sending: HashSet<String>,
    _search: Subscription,
}

/// The words of a cloud start that failed: its first message and the ones that waited behind it, with their files.
#[derive(Debug)]
struct KeptStart {
    repo: String,
    text: String,
    attachments: crate::attachments::Draft,
}

/// What a note says when the waiting queue is full.
pub(super) fn waiting_full_note() -> String {
    format!(
        "Up to {} messages can wait while the machine starts.",
        crate::cloud::waiting::MAX_WAITING
    )
}

/// The failed `start`'s messages in one piece, in order, as Edit puts them in the composer. Its queue ends.
fn keep_start(start: &CloudStart) -> KeptStart {
    let mut kept = KeptStart {
        repo: start.repo.clone(),
        text: start.new.text.clone(),
        attachments: start.new.attachments.clone(),
    };
    for message in start.waiting.take_all() {
        kept.text = [kept.text.as_str(), message.text.as_str()]
            .into_iter()
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        kept.attachments.images.extend(message.attachments.images);
        kept.attachments.files.extend(message.attachments.files);
    }
    start.waiting.end();
    kept.attachments.upload = None;
    kept
}

/// "Archive anyway" or "Delete anyway", and what it asks about.
pub(super) struct Question {
    machine: String,
    end: End,
    text: SharedString,
}

impl CloudUi {
    /// The repository picked and the list read belong to one account: forget both.
    pub fn reset_repos(&mut self) {
        self.repo = None;
        self.repos = None;
    }

    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> CloudUi {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search projects and repositories, or paste a URL"));
        let subscription = cx.subscribe(&search, |this: &mut Workspace, input, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                let query = input.read(cx).value().to_string();
                // A new search lights its first row.
                if super::picker_navigation::query_changed(&mut this.cloud_ui.query, &query) {
                    this.cloud_ui.cursor = 0;
                }
                cx.notify();
            }
        });
        CloudUi {
            on: false,
            repo: None,
            missing: None,
            missing_on: None,
            repos: None,
            search,
            query: String::new(),
            cursor: 0,
            list_scroll: ScrollHandle::new(),
            scope: Default::default(),
            github_all: false,
            account: None,
            question: None,
            starting: None,
            kept: None,
            woken: HashSet::new(),
            sending: HashSet::new(),
            _search: subscription,
        }
    }
}

/// Runs `work` on the runtime the cloud machines use, and hands its answer to `then` on the UI thread.
pub(super) fn on_runtime<T: Send + 'static>(
    window: &mut Window,
    cx: &mut Context<Workspace>,
    work: impl Future<Output = T> + Send + 'static,
    then: impl FnOnce(&mut Workspace, T, &mut Window, &mut Context<Workspace>) + 'static,
) {
    let (answer, answered) = oneshot::channel();
    crate::code_run::runtime().spawn(async move {
        let _ = answer.send(work.await);
    });
    cx.spawn_in(window, async move |this, cx| {
        if let Ok(value) = answered.await {
            let _ = this.update_in(cx, |this, window, cx| {
                then(this, value, window, cx);
                cx.notify();
            });
        }
    })
    .detach();
}

/// Whether an account that arrives is not the one the cloud pick and repository list were read for: another
/// organization, or the first account after a sign-out or a Gateway switch (`clear_session` leaves none before it).
pub(super) fn new_owner(before: Option<&str>, org: &str) -> bool {
    before != Some(org)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Why a sleeping cloud session's history did not show: Gateway could not be read.
pub(super) const HISTORY_FAILED: &str = "Its history didn't load.";

/// What a sleeping cloud session not read yet says ([`Workspace::asleep_note`]).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum AsleepNote {
    /// Its history is on its way from Gateway.
    Opening,
    /// Gateway holds none: [`ASLEEP_UNREAD`].
    Unread,
    /// Gateway could not be read, and why.
    Failed(SharedString),
}

impl Workspace {
    /// Cloud picked in the "where it runs" menu: a machine of its own starts with the session, so there is no project
    /// to pick. The setup and the repositories are read now.
    pub(super) fn pick_cloud(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.close_launch_menu();
        self.cloud_ui.on = true;
        self.code_form.plain = false;
        // The project picked on this computer goes to the cloud with its repository, or the one that had no copy here.
        let here = self
            .code_form
            .project
            .as_deref()
            .and_then(|id| self.projects.iter().find(|project| project.id == id))
            .and_then(super::repo_groups::cloud_repo_of);
        self.cloud_ui.missing_on = None;
        if let Some(repo) = self.cloud_ui.missing.take().or(here) {
            self.cloud_ui.repo = Some(repo);
        }
        self.load_cloud_account(window, cx);
        if self.cloud_ui.repos.is_none() {
            self.load_cloud_repos(window, cx);
        }
        // With no repository yet, the project menu opens under its chip, with the GitHub repositories: the next thing
        // to pick.
        if self.cloud_ui.repo.is_none() {
            self.toggle_launch_menu(LaunchMenu::Project, window, cx);
        }
        cx.notify();
    }

    /// "Run on this computer", or another machine picked: the session runs there.
    pub(super) fn leave_cloud(&mut self, cx: &mut Context<Self>) {
        self.cloud_ui.on = false;
        if !self.offers_agent(self.code_form.agent) {
            self.code_form.take_agent(super::new_code::CodeForm::default().agent);
        }
        cx.notify();
    }

    /// Read the cloud setup again (`cloud_account`): availability, and the Claude and GitHub sign-ins.
    pub(super) fn load_cloud_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cloud) = crate::cloud::get() else {
            self.cloud_ui.account = Some(Err(NOT_HERE.into()));
            return;
        };
        on_runtime(
            window,
            cx,
            async move { cloud.account().await },
            |this, account, window, cx| this.cloud_account_read(Ok(account), window, cx),
        );
    }

    /// Try again on a cloud setup that failed to read: the machines are listed again first, so the answer is fresh.
    pub(super) fn read_cloud_account_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cloud) = crate::cloud::get() else {
            return;
        };
        self.cloud_ui.account = None;
        on_runtime(
            window,
            cx,
            async move {
                let synced = cloud.sync().await;
                (synced, cloud.account().await)
            },
            |this, (synced, account), window, cx| {
                this.cloud_account_read(setup_line::read_again(synced, account), window, cx)
            },
        );
        cx.notify();
    }

    /// The cloud setup arrived. Once cloud machines are available again, a cloud start that waited for them goes on
    /// (`setup::done`).
    fn cloud_account_read(
        &mut self,
        account: Result<CloudAccount, SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = SharedString::from(setup_line::KEY);
        let was_blocked = self
            .cloud_ui
            .account
            .as_ref()
            .is_some_and(|before| Blocker::of(before).is_some());
        let available = Blocker::of(&account).is_none();
        self.cloud_ui.account = Some(account);
        if available && (was_blocked || crate::setup::state::waiting(&key, cx)) {
            // After this update: what resumes may update the workspace.
            window.defer(cx, move |window, cx| crate::setup::done(&key, window, cx));
        }
    }

    pub(super) fn load_cloud_repos(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cloud) = crate::cloud::get() else {
            return;
        };
        self.cloud_ui.repos = None;
        // An answer for the organization before a switch is dropped: the list is the new organization's.
        let org = self.account.as_ref().map(|account| account.org.clone());
        on_runtime(
            window,
            cx,
            async move { cloud.repos(None).await },
            move |this, repos, _, _| {
                if this.account.as_ref().map(|account| account.org.clone()) == org {
                    this.cloud_ui.repos = Some(repos.map_err(SharedString::from));
                }
            },
        );
    }

    /// Another organization: its repositories are not the last one's, and a repository picked there cannot start a
    /// session here. The list is read again when the menu is in use.
    pub(super) fn reset_cloud_repos(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cloud_ui.reset_repos();
        if self.cloud_ui.on {
            self.load_cloud_repos(window, cx);
            // A sign-in or another organization: whether cloud machines are available is the new account's.
            self.load_cloud_account(window, cx);
        }
    }

    /// The steps the new cloud session still needs, for its agent; empty once the setup is done or not read yet.
    fn cloud_missing(&self) -> Vec<Check> {
        match &self.cloud_ui.account {
            Some(Ok(account)) => {
                words::missing(self.code_form.agent, account.claude.connected, account.github.connected)
            }
            _ => Vec::new(),
        }
    }

    /// Why a new cloud session cannot start now, as one line; None when it can.
    pub(super) fn cloud_blocker(&self) -> Option<SharedString> {
        if crate::cloud::get().is_none() {
            return Some(NOT_HERE.into());
        }
        if let Some(refused) = words::plan_refused_line(self.code_form.agent, self.code_form.effective_via()) {
            return Some(refused.into());
        }
        match &self.cloud_ui.account {
            None => Some("Reading your cloud setup…".into()),
            Some(Err(error)) => Some(error.clone()),
            Some(Ok(account)) if !account.availability.available => {
                Some(words::unavailable_line(account.availability.reason.as_deref()).into())
            }
            Some(Ok(_)) if !self.cloud_missing().is_empty() => Some("Finish setting up Cloud first.".into()),
            Some(Ok(_)) if self.cloud_ui.repo.is_none() => Some(PICK_FIRST.into()),
            Some(Ok(_)) => None,
        }
    }

    /// What stands in for the composer while Cloud is picked: the setup card until it is done. None: the composer. The
    /// repository is picked from the project menu under its chip (`render_project_menu`), so the composer shows meanwhile.
    pub(super) fn render_cloud_instead(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.cloud_ui.on || self.code_form.starting.is_some() {
            return None;
        }
        if crate::cloud::get().is_none() {
            return Some(self.render_cloud_read_only(palette, cx));
        }
        match &self.cloud_ui.account {
            None => {
                return Some(
                    card(palette)
                        .child(note_line("Reading your cloud setup…", palette))
                        .into_any_element(),
                );
            }
            Some(account) => {
                if let Some(blocker) = Blocker::of(account) {
                    return Some(self.render_cloud_blocked(&blocker, palette, cx).into_any_element());
                }
            }
        }
        let missing = self.cloud_missing();
        if !missing.is_empty() {
            return Some(self.render_setup_card(&missing, palette, cx).into_any_element());
        }
        None
    }

    /// Cloud machines can't run for the person: the setup line with its fix (sign in, ask, try again), and "Run on this
    /// computer" in every state, as the setup card has it.
    fn render_cloud_blocked(&self, blocker: &Blocker, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let org = self
            .account
            .as_ref()
            .map(|account| account.org_name.clone())
            .unwrap_or_else(|| "your organization".into());
        let this = cx.entity().downgrade();
        let retry = crate::setup::Fix::act(move |window, cx| {
            if let Err(error) = this.update(cx, |this, cx| this.read_cloud_account_again(window, cx)) {
                eprintln!("nexus-desktop-gpui: cloud: read again: {error}");
            }
        });
        let line = setup_line::line(blocker, &org, retry);
        let fixer = self.fixer(cx);
        card(palette)
            .p(px(6.))
            .gap(px(4.))
            .children(crate::setup::slot("cloud-setup", Some(line), &fixer, palette, cx))
            .child(
                h_flex().justify_end().px(px(10.)).pb(px(6.)).child(
                    ui::button("cloud-run-here", "Run on this computer", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.leave_cloud(cx))),
                ),
            )
    }

    /// `CloudSetupCard`: what is missing, and the way there.
    fn render_setup_card(&self, missing: &[Check], palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let names: Vec<&str> = missing.iter().map(|check| check.name()).collect();
        card(palette)
            .child(
                h_flex()
                    .gap(px(10.))
                    .items_start()
                    .child(glyph(Lucide::Cloud, palette.icon))
                    .child(
                        v_flex()
                            .flex_1()
                            .gap(px(2.))
                            .child(title_line("Finish setting up Cloud first", palette))
                            .child(note_line(
                                format!("Still to do: {}. You do this once, on this computer.", names.join(", ")),
                                palette,
                            )),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .justify_end()
                    .child(
                        ui::button("cloud-run-here", "Run on this computer", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.leave_cloud(cx))),
                    )
                    .child(
                        ui::primary_button("cloud-open-settings", "Open Cloud settings", palette).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.open_settings(SettingsSection::Page(PageId::Cloud), window, cx)
                            }),
                        ),
                    ),
            )
    }

    /// The search over the project menu's projects and repositories; the cloud's repository picker shares it.
    pub(super) fn repo_search(&self) -> &Entity<InputState> {
        &self.cloud_ui.search
    }

    pub(super) fn repo_query(&self) -> &str {
        self.cloud_ui.query.trim()
    }

    /// The person's GitHub repositories that match the search, latest pushed first. None while they are read; an error
    /// when GitHub did not answer; nothing when this build runs no cloud.
    pub(super) fn github_picks(&self) -> Option<Result<Vec<CloudRepo>, SharedString>> {
        crate::cloud::get()?;
        let query = self.repo_query();
        match self.cloud_ui.repos.as_ref()? {
            Err(error) => Some(Err(error.clone())),
            Ok(repos) => Some(Ok(words::pick_repos(repos, query))),
        }
    }

    /// Every GitHub repository read, latest pushed first, for the project menu's own search (`project_rows`). None
    /// while they are read; an error when GitHub did not answer; nothing when this build runs no cloud.
    pub(super) fn github_repos(&self) -> Option<Result<Vec<CloudRepo>, SharedString>> {
        crate::cloud::get()?;
        match self.cloud_ui.repos.as_ref()? {
            Err(error) => Some(Err(error.clone())),
            Ok(repos) => Some(Ok(words::pick_repos(repos, ""))),
        }
    }

    /// Opens the project menu's clone form for the repository picked (`url`, shown by `name`), on this computer or on
    /// `server`. Only where it goes is asked: the repository is not typed in.
    pub(super) fn clone_here(
        &mut self,
        url: String,
        name: String,
        server: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_overlay(Overlay::LaunchMenu(super::new_code::LaunchMenu::Project));
        let source = match &server {
            Some(server) => {
                self.open_server_folder_in_menu(server, true, window, cx);
                self.server_folder
                    .as_mut()
                    .and_then(|folder| folder.clone.as_mut())
                    .map(|clone| {
                        clone.picked = Some(name.into());
                        clone.source.clone()
                    })
            }
            None => {
                self.open_clone_in_menu(window, cx);
                self.clone_form.as_mut().map(|form| {
                    form.picked = Some(name.into());
                    form.source.clone()
                })
            }
        };
        if let Some(source) = source {
            source.update(cx, |input, cx| input.set_value(url, window, cx));
        }
        cx.notify();
    }

    /// The cloud session's repository, picked from the project menu: the menu closes and the composer takes the keys.
    pub(super) fn pick_repo(&mut self, repo: CloudRepo, window: &mut Window, cx: &mut Context<Self>) {
        self.cloud_ui.repo = Some(repo);
        self.overlay.close_launch_menu();
        if self.composer_note.as_ref().is_some_and(|note| note == PICK_FIRST) {
            self.composer_note = None;
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Start a cloud session with `prompt` (`startCloudSession`): the session view opens at once with its composer, the
    /// message waits in the transcript, its machine's line shows above the composer, and the session takes its id
    /// once its agent has the message.
    pub(super) fn start_cloud(&mut self, prompt: String, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(blocker) = self.cloud_blocker() {
            // No repository yet: the project menu opens, as the way on.
            if self.cloud_ui.repo.is_none()
                && blocker == PICK_FIRST
                && self.overlay.launch_menu() != Some(LaunchMenu::Project)
            {
                self.toggle_launch_menu(LaunchMenu::Project, window, cx);
            }
            self.composer_note = Some(blocker);
            cx.notify();
            return false;
        }
        let Some(repo) = self.cloud_ui.repo.clone() else {
            return false;
        };
        // Referenced artifacts go as a block in the text: the agent reads them with the `nexus` CLI.
        let mut attachments = std::mem::take(&mut self.draft);
        let prompt = attachments.take_artifacts(&prompt, self.gateway.url());
        let form = &self.code_form;
        let new = NewSession {
            project_id: String::new(),
            agent: form.agent,
            model: form.model.clone(),
            permission_mode: form.mode.as_str().to_string(),
            effort: form.effort,
            via: form.effective_via(),
            provider: None,
            worktree: false,
            review: None,
            base: None,
            text: prompt,
            attachments,
        };
        let start = CloudStart {
            request_id: uuid::Uuid::new_v4().to_string(),
            repo: repo.url,
            new: new.clone(),
            waiting: Waiting::default(),
            cloning: Default::default(),
        };
        self.begin_launch_motion(cx);
        let launch_motion = self.launch_motion.clone();
        self.reset_chat();
        self.launch_motion = launch_motion;
        self.title = words::session_title(&new.text).into();
        let mut view = CodeView::pending(new, window, cx);
        if let Some(pending) = &mut view.pending {
            pending.cloud = Some(start.clone());
        }
        let draft = view.id.clone();
        self.view = View::Code(view);
        self.scroll.scroll_to_bottom();
        self.run_cloud_start(draft, start, window, cx);
        true
    }

    /// Make the cloud session, on the runtime; what it reports comes back as the runner's events do. A start whose
    /// session was ended meanwhile reports nothing: `end_cloud` already said so.
    pub(super) fn run_cloud_start(
        &mut self,
        draft: String,
        mut start: CloudStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(cloud), events) = (crate::cloud::get(), self.relay.clone()) else {
            return;
        };
        self.begin_upload(&mut start.new.attachments, &start.new.text, window, cx);
        if let View::Code(view) = &mut self.view
            && let Some(pending) = &mut view.pending
            && pending
                .cloud
                .as_ref()
                .is_some_and(|held| held.request_id == start.request_id)
        {
            pending.new.attachments = start.new.attachments.clone();
            pending.cloud = Some(start.clone());
        }
        self.upload_view();
        self.code_form.starting = Some(draft.clone());
        self.cloud_ui.starting = Some(start.clone());
        crate::code_run::runtime().spawn(async move {
            let machines = crate::machines::get();
            let made = crate::cloud::start_session(&machines, &cloud, &crate::desktop_settings::load, &start).await;
            let event = match made {
                _ if start.waiting.ended() => CodeEvent::Changed,
                Ok(id) => CodeEvent::Created { draft, id },
                Err(error) => CodeEvent::StartFailed { draft, error },
            };
            let _ = events.unbounded_send(event);
        });
    }

    /// A cloud start failed and its session's view is not open: keep what the person typed for New session.
    pub(super) fn keep_failed_start(&mut self, start: &CloudStart, window: &mut Window, cx: &mut Context<Self>) {
        let kept = keep_start(start);
        match self.cloud_ui.kept.as_mut() {
            Some(before) => {
                before.text = [before.text.as_str(), kept.text.as_str()]
                    .into_iter()
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                before.attachments.images.extend(kept.attachments.images);
                before.attachments.files.extend(kept.attachments.files);
            }
            None => self.cloud_ui.kept = Some(kept),
        }
        if matches!(self.view, View::NewCode) {
            self.restore_kept_start(window, cx);
        }
    }

    /// New session opens with the messages of a cloud start that failed behind the person's back: on Cloud, with its
    /// repository, and what is already typed stays first.
    pub(super) fn restore_kept_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kept) = self.cloud_ui.kept.take() else {
            return;
        };
        self.cloud_ui.on = true;
        self.cloud_ui.repo = Some(CloudRepo {
            name: kept.repo.clone(),
            url: kept.repo,
            private: false,
            pushed_at: None,
        });
        self.draft.images.extend(kept.attachments.images);
        self.draft.files.extend(kept.attachments.files);
        let typed = self.composer.read(cx).value().to_string();
        let text = super::code_view::restored_draft(&typed, &kept.text);
        self.composer
            .update(cx, |state, cx| state.set_value(text.clone(), window, cx));
        self.composer_empty = text.trim().is_empty();
    }

    /// The open view's cloud session while it starts.
    pub(super) fn starting_cloud(&self) -> Option<&CloudStart> {
        match &self.view {
            View::Code(view) => view.pending.as_ref().and_then(|pending| pending.cloud.as_ref()),
            _ => None,
        }
    }

    /// A message sent in a cloud session that is still starting: it waits behind the first, and goes once the session
    /// is made. Made already (its first events not read yet), it goes now.
    pub(super) fn send_while_starting(&mut self, prompt: String) {
        let Some(start) = self.starting_cloud() else {
            return;
        };
        let (session, waiting) = (start.session_id(), start.waiting.clone());
        let mut attachments = std::mem::take(&mut self.draft);
        let message = Message {
            id: 0,
            text: attachments.take_artifacts(&prompt, self.gateway.url()),
            attachments,
        };
        match waiting.push(message) {
            Push::Late(message) => {
                if let Some(runs) = self.runs(&session) {
                    runs.send(session, message.text, message.attachments, false);
                }
            }
            // `submit_code` stops a full queue before the composer is cleared; this is its backstop.
            Push::Full(message) => {
                self.draft = message.attachments;
                self.composer_note = Some(waiting_full_note().into());
            }
            Push::Queued => {}
        }
    }

    /// Takes waiting message `ix` back: it goes into the composer again, unless something is typed there.
    pub(super) fn take_back_waiting(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let taken = self.starting_cloud().and_then(|start| start.waiting.take_back(id));
        if let Some(message) = taken
            && self.composer_empty
        {
            self.draft = message.attachments;
            self.composer
                .update(cx, |state, cx| state.set_value(message.text.clone(), window, cx));
            self.composer_empty = message.text.trim().is_empty();
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// The cloud machine of the open session, while the session itself is not read yet: a new one coming up, or one
    /// whose machine sleeps.
    pub(super) fn coming_machine(&self, view: &CodeView) -> Option<CloudView> {
        let session = match view.pending.as_ref().and_then(|pending| pending.cloud.as_ref()) {
            Some(start) => start.session_id(),
            None if view.blocks.is_empty() => view.id.clone(),
            None => return None,
        };
        self.cloud_machines
            .iter()
            .find(|machine| machine.info.session_id.as_deref() == Some(session.as_str()))
            .cloned()
    }

    /// Whether the open session's cloud machine is on its way: its line shows above the composer.
    pub(super) fn machine_coming(&self, view: &CodeView) -> bool {
        self.coming_machine(view)
            .is_some_and(|machine| words::machine_line(&machine, now_ms()).is_some())
    }

    /// The cloud machine session `session` runs on.
    pub(super) fn cloud_machine_of_session(&self, session: &str) -> Option<&CloudView> {
        self.cloud_machines
            .iter()
            .find(|machine| machine.info.session_id.as_deref() == Some(session))
    }

    /// Whether the open session's cloud machine sleeps (or is going to sleep).
    pub(super) fn open_session_asleep(&self) -> bool {
        match &self.view {
            View::Code(view) => self
                .cloud_machine_of_session(&view.id)
                .is_some_and(|machine| words::is_asleep(&machine.info)),
            _ => false,
        }
    }

    /// The person typed in the open session's composer: a sleeping cloud machine starts waking now, so it is (nearly)
    /// up by the time they send. Once per sleep; opening the session alone never wakes it.
    pub(super) fn wake_on_typing(&mut self, typed: bool) {
        let View::Code(view) = &self.view else {
            return;
        };
        let Some(machine) = self.cloud_machine_of_session(&view.id) else {
            return;
        };
        let (id, asleep) = (machine.id.clone(), words::is_asleep(&machine.info));
        if take_wake(&mut self.cloud_ui.woken, &id, asleep, typed)
            && let Some(cloud) = crate::cloud::get()
            && let Err(error) = cloud.wake(&id)
        {
            self.composer_note = Some(error.into());
        }
    }

    /// A message goes to session `session`: when its machine sleeps, the send wakes it and waits for it
    /// (`Cloud::call`), and the session shows as working meanwhile, with no question asked.
    pub(super) fn sending_to_cloud(&mut self, session: &str) {
        let Some(machine) = self.cloud_machine_of_session(session) else {
            return;
        };
        if words::is_asleep(&machine.info) {
            let id = machine.id.clone();
            self.cloud_ui.woken.insert(id.clone());
            self.cloud_ui.sending.insert(id);
        }
    }

    /// What a cloud session not read yet says while its machine sleeps: its history coming from Gateway,
    /// none there (history off, or a session from before it was kept), or why Gateway could not be read.
    pub(super) fn asleep_note(&self, view: &CodeView) -> AsleepNote {
        let Some(machine) = self.cloud_machine_of_session(&view.id) else {
            return AsleepNote::Unread;
        };
        match self.machine_source.history(&machine.id, &view.id) {
            Some(HistoryRead::Reading) => AsleepNote::Opening,
            // Not asked yet: the first read asks, once the cloud machines run here.
            None if self.machine_source.cloud_machines().is_some() => AsleepNote::Opening,
            Some(HistoryRead::Failed(error)) => AsleepNote::Failed(error.into()),
            _ => AsleepNote::Unread,
        }
    }

    /// The open sleeping session's note: "Opening…", [`ASLEEP_UNREAD`], or why its history did not load, with Try
    /// again.
    pub(super) fn render_asleep_note(&self, note: AsleepNote, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let line = |text: SharedString| div().text_sm().text_color(palette.faint).child(text);
        match note {
            AsleepNote::Opening => line("Opening…".into()).into_any_element(),
            AsleepNote::Unread => line(ASLEEP_UNREAD.into()).into_any_element(),
            AsleepNote::Failed(error) => v_flex()
                .gap(px(4.))
                .child(line(HISTORY_FAILED.into()))
                .child(div().text_xs().text_color(palette.faint).child(error))
                .child(
                    text_button("history-again", "Try again", false, palette)
                        .on_click(cx.listener(|this, _, _, cx| this.read_history_again(cx))),
                )
                .into_any_element(),
        }
    }

    /// Try again on the open session's history: Gateway is asked once more.
    fn read_history_again(&mut self, cx: &mut Context<Self>) {
        let View::Code(view) = &self.view else {
            return;
        };
        if let Some(machine) = self.cloud_machine_of_session(&view.id) {
            self.machine_source.read_history_again(&machine.id, &view.id);
        }
        cx.notify();
    }

    /// Whether the open session's message waits for its machine to wake.
    pub(super) fn cloud_delivering(&self, session: &str) -> bool {
        self.cloud_machine_of_session(session)
            .is_some_and(|machine| self.cloud_ui.sending.contains(&machine.id))
    }

    /// The machines changed: a machine seen awake wakes again on the next typing once it sleeps again, and a message
    /// sent to a sleeping one stops showing as working once the machine runs (the session's own status follows) or
    /// its wake failed.
    pub(super) fn settle_wakes(&mut self) {
        let machines = &self.cloud_machines;
        let find = |id: &String| machines.iter().find(|machine| machine.id == *id);
        self.cloud_ui
            .woken
            .retain(|id| find(id).is_some_and(|machine| words::is_asleep(&machine.info)));
        self.cloud_ui.sending.retain(|id| find(id).is_some_and(delivering));
    }

    /// A cloud session's machine on its way, as one line above the composer (in place of `CloudMachineProgress`'s
    /// card): what it does now, or why it stopped with Try again. End deletes the machine at any time. A new session's
    /// line also says why its start failed. None once the machine runs.
    pub(super) fn render_start_line(
        &self,
        view: &CodeView,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let machine = self.coming_machine(view);
        let pending = view.pending.as_ref();
        let line = match pending.and_then(|pending| pending.cloud.as_ref()) {
            Some(start) => words::start_line(
                machine.as_ref(),
                pending.and_then(|pending| pending.failed.as_deref()),
                start.cloning.get().as_ref(),
                now_ms(),
            ),
            None => words::machine_line(machine.as_ref()?, now_ms())?,
        };
        let row = h_flex()
            .w_full()
            .gap(px(8.))
            .px(px(4.))
            .text_sm()
            .text_color(palette.muted);
        // End deletes the machine, once Gateway made one; before that, Edit takes the message back to New session.
        let start = pending.and_then(|pending| pending.cloud.as_ref());
        let end = match &machine {
            Some(machine) => {
                let session = start.map(CloudStart::session_id).unwrap_or_else(|| view.id.clone());
                let machine = machine.id.clone();
                Some(
                    text_button("start-end", "End", false, palette)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.end_cloud(&session, &machine, End::Delete, false, window, cx)
                        }))
                        .into_any_element(),
                )
            }
            None if start.is_some() && matches!(line, StartLine::Failed(_)) => Some(
                text_button("start-edit", "Edit", false, palette)
                    .on_click(cx.listener(|this, _, window, cx| this.edit_pending(window, cx)))
                    .into_any_element(),
            ),
            None => None,
        };
        let element = match line {
            StartLine::Busy(text) => {
                let dot = div().size(px(7.)).rounded_full().bg(palette.foreground).with_animation(
                    "start-dot",
                    Animation::new(crate::motion::PULSE)
                        .repeat()
                        .with_max_fps(crate::motion::LOOP_FPS)
                        .with_easing(pulsating_between(0.3, 1.0)),
                    |dot, delta| dot.opacity(delta),
                );
                row.child(dot)
                    .child(div().min_w(px(0.)).truncate().child(text))
                    .children(
                        view.working_since
                            .map(|since| div().text_color(palette.faint).child(elapsed(since.elapsed()))),
                    )
                    .child(div().flex_1())
                    .children(end)
                    .into_any_element()
            }
            StartLine::Failed(error) => {
                let machine = machine.map(|machine| machine.id);
                row.text_color(palette.destructive)
                    .child(glyph(Lucide::CircleAlert, palette.destructive))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(error))
                    .child(
                        text_button("start-retry", "Try again", true, palette).on_click(cx.listener(
                            move |this, _, window, cx| match &machine {
                                Some(machine) => this.retry_machine(machine, window, cx),
                                None => this.retry_pending(window, cx),
                            },
                        )),
                    )
                    .children(end)
                    .into_any_element()
            }
        };
        Some(element)
    }

    /// Try again on a cloud machine whose create or wake failed: it resumes from the step that failed. A new session
    /// still on its way starts again with it.
    fn retry_machine(&mut self, machine: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.starting_cloud().is_some() {
            self.retry_pending(window, cx);
            return;
        }
        if let Some(cloud) = crate::cloud::get()
            && let Err(error) = cloud.retry(machine)
        {
            self.composer_note = Some(error.into());
        }
        cx.notify();
    }

    /// Archive or delete session `session` on cloud machine `machine`: its machine is deleted. When it holds commits
    /// that are on no remote, nothing goes, and a question asks first (`endCloudSessions`).
    pub(super) fn end_cloud(
        &mut self,
        session: &str,
        machine: &str,
        end: End,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cloud) = crate::cloud::get() else {
            self.composer_note = Some(NOT_HERE.into());
            cx.notify();
            return;
        };
        let title = self
            .session_by_id(session)
            .map(|session| session.title.clone())
            .or_else(|| {
                self.cloud_machines
                    .iter()
                    .find(|candidate| candidate.id == machine)
                    .and_then(|candidate| candidate.info.title.clone())
            })
            .unwrap_or_else(|| "Session".into());
        self.cloud_ui.question = None;
        let (machine, session) = (machine.to_string(), session.to_string());
        let id = machine.clone();
        on_runtime(
            window,
            cx,
            async move {
                let machines = crate::machines::get();
                crate::cloud::end_session(&machines, &cloud, &id, force).await
            },
            move |this, ended, window, cx| match ended {
                Ok(crate::cloud::Ended::Gone) => {
                    // Ended while it started: its waiting messages go, and so does its start.
                    if let Some(start) = this.cloud_ui.starting.take_if(|start| start.session_id() == session) {
                        start.waiting.end();
                        this.code_form.starting = None;
                    }
                    let open = this.starting_cloud().filter(|start| start.session_id() == session);
                    if let Some(start) = open {
                        start.waiting.end();
                    }
                    if open.is_some() || matches!(&this.view, View::Code(open) if open.id == session) {
                        this.open_new_code(window, cx);
                    }
                    this.composer_note = Some(words::ended_text(&title, end).into());
                    this.machines_changed(cx);
                }
                Ok(crate::cloud::Ended::Unpushed(unpushed)) => {
                    this.cloud_ui.question = Some(Question {
                        text: words::unpushed_text(&title, &unpushed, end).into(),
                        machine,
                        end,
                    });
                }
                Err(error) => {
                    let doing = match end {
                        End::Archive => "archive",
                        End::Delete => "delete",
                    };
                    this.composer_note = Some(format!("Couldn\u{2019}t {doing}: {error}").into());
                }
            },
        );
    }

    /// The cloud machine holding session `session` (`machineForSession`), when one does.
    pub(super) fn cloud_machine_for(&self, session: &str) -> Option<String> {
        self.cloud_machines
            .iter()
            .find(|machine| machine.info.session_id.as_deref() == Some(session))
            .map(|machine| machine.id.clone())
    }

    /// The question about commits on no remote, with its "anyway" action.
    pub(super) fn render_cloud_question(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let question = self.cloud_ui.question.as_ref()?;
        let (machine, end) = (question.machine.clone(), question.end);
        let session = self
            .cloud_machines
            .iter()
            .find(|candidate| candidate.id == machine)
            .and_then(|candidate| candidate.info.session_id.clone())
            .unwrap_or_default();
        Some(
            h_flex()
                .gap(px(10.))
                .px(px(16.))
                .pt(px(10.))
                .text_size(px(13.5))
                .text_color(palette.muted)
                .child(div().flex_1().min_w(px(0.)).child(question.text.clone()))
                .child(
                    ui::danger_button("cloud-end-anyway", words::anyway(end), palette).on_click(cx.listener(
                        move |this, _, window, cx| this.end_cloud(&session, &machine, end, true, window, cx),
                    )),
                )
                .child(
                    ui::button("cloud-end-keep", "Keep", palette).on_click(cx.listener(|this, _, _, cx| {
                        this.cloud_ui.question = None;
                        cx.notify();
                    })),
                )
                .into_any_element(),
        )
    }
}

/// A card in the composer's place.
fn card(palette: Palette) -> Div {
    v_flex()
        .w_full()
        .gap(px(10.))
        .p(px(16.))
        .rounded(px(16.))
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border)
}

fn title_line(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .text_size(px(14.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.foreground)
        .child(text.into())
}

fn note_line(text: impl Into<SharedString>, palette: Palette) -> Div {
    div().text_size(px(13.)).text_color(palette.muted).child(text.into())
}

/// Whether typing in a session on `machine` sends a wake: only something typed, only while the machine sleeps, and
/// once until it is seen awake again (`settle_wakes` takes it out of `woken`).
fn take_wake(woken: &mut HashSet<String>, machine: &str, asleep: bool, typed: bool) -> bool {
    asleep && typed && woken.insert(machine.to_string())
}

/// Whether a message sent to `machine` while it slept is still on its way: until the machine runs, unless its wake
/// failed or the machine is gone.
fn delivering(machine: &CloudView) -> bool {
    let failed = machine
        .progress
        .as_ref()
        .is_some_and(|progress| progress.error.is_some())
        || matches!(
            machine.info.state,
            crate::cloud::CloudState::Failed | crate::cloud::CloudState::Gone
        );
    !failed && !matches!(machine.status, nexus_remote::machine::Status::Connected { .. })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{delivering, keep_start, new_owner, take_wake};
    use crate::cloud::CloudStart;
    use crate::cloud::waiting::{Message, Push, Waiting};
    use crate::code_run::NewSession;
    use nexus_workbench::event::{SessionAgent, Via};

    /// A start that fails while another session is open still has its typed messages.
    #[test]
    fn a_failed_start_keeps_every_message_for_new_session() {
        let mut start = CloudStart {
            request_id: "r".into(),
            repo: "acme/app".into(),
            new: NewSession {
                review: None,
                base: None,
                project_id: String::new(),
                agent: SessionAgent::Claude,
                model: None,
                permission_mode: "default".into(),
                effort: None,
                via: Via::Nexus,
                provider: None,
                worktree: false,
                text: "first".into(),
                attachments: Default::default(),
            },
            waiting: Waiting::default(),
            cloning: Default::default(),
        };
        start.new.attachments.upload = Some(std::sync::Arc::default());
        for text in ["second", "third"] {
            let message = Message {
                id: 0,
                text: text.into(),
                ..Message::default()
            };
            assert!(matches!(start.waiting.push(message), Push::Queued));
        }
        let kept = keep_start(&start);
        assert_eq!(kept.text, "first\n\nsecond\n\nthird");
        assert_eq!(kept.repo, "acme/app");
        assert!(
            kept.attachments.upload.is_none(),
            "a restored draft must start a fresh upload"
        );
        // Nothing is sent from the queue after that: the words live in one place.
        assert!(start.waiting.ended());
        assert!(start.waiting.texts().is_empty());
    }

    /// The messages kept from a start that failed behind the person's back reach the composer when New
    /// session opens later, as the note says.
    #[gpui_kit::test]
    fn kept_messages_reach_the_composer_when_new_session_opens_later(cx: &mut gpui_kit::TestAppContext) {
        // This fixture checks draft restoration, not discovery on the machine running the test.
        let _agents = crate::code_run::fixture_agents(Vec::new());
        use crate::workspace::Workspace;

        cx.update(gpui_kit::init);
        let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
        // Background HTTP replies must not wake GPUI's deterministic test scheduler.
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let gateway: std::sync::Arc<dyn crate::backend::Backend> = std::sync::Arc::new(gateway);
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            Workspace::with_machine_source(
                gateway,
                events,
                arrivals,
                false,
                super::super::machine_source::MachineSource::isolated(),
                window,
                cx,
            )
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.cloud_ui.kept = Some(super::KeptStart {
                text: "first\n\nsecond".into(),
                repo: "acme/app".into(),
                attachments: Default::default(),
            });
            workspace.open_new_code(window, cx);
            assert!(workspace.cloud_ui.kept.is_none(), "the kept start is taken");
            assert!(workspace.cloud_ui.on);
            assert_eq!(workspace.composer.read(cx).value().as_ref(), "first\n\nsecond");
        });
    }

    #[gpui_kit::test]
    fn fixture_machine_notifications_stay_separate_from_background_sources(cx: &mut gpui_kit::TestAppContext) {
        use super::super::{Workspace, machine_source::MachineSource};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        cx.update(gpui_kit::init);
        let source = MachineSource::isolated();
        let watched = source.machines.clone();
        let foreign = MachineSource::isolated().machines;
        let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            Workspace::with_machine_source(Arc::new(gateway), events, arrivals, false, source, window, cx)
        });
        cx.run_until_parked();
        let notifications = Arc::new(AtomicUsize::new(0));
        let observed = notifications.clone();
        let _subscription = cx.update(|_, cx| {
            cx.observe(&workspace, move |_, _| {
                observed.fetch_add(1, Ordering::SeqCst);
            })
        });
        // A real worker can publish its status without waking this fixture's deterministic scheduler.
        std::thread::spawn(move || {
            foreign
                .sink("other")
                .status(&nexus_remote::machine::Status::Connecting { attempt: 1 })
        })
        .join()
        .expect("foreign source publishes without entering the fixture scheduler");
        cx.run_until_parked();
        assert_eq!(
            notifications.load(Ordering::SeqCst),
            0,
            "foreign changes do not notify this workspace"
        );
        // The isolated watcher still runs; its controlled changes reach the same handler as production.
        watched
            .sink("fixture")
            .status(&nexus_remote::machine::Status::Connecting { attempt: 1 });
        cx.run_until_parked();
        assert!(
            notifications.load(Ordering::SeqCst) > 0,
            "the fixture's own source still notifies the workspace"
        );
    }

    #[test]
    fn a_new_organization_or_a_first_account_is_a_new_owner() {
        assert!(new_owner(Some("a"), "b"), "organization switch");
        assert!(
            new_owner(None, "a"),
            "first account after a sign-out or a Gateway switch"
        );
        assert!(!new_owner(Some("a"), "a"), "the same organization again keeps the pick");
    }

    /// A pick belongs to one account: a sign-out or a Gateway switch (`clear_session`) takes it with the account.
    #[gpui_kit::test]
    fn a_picked_repository_does_not_outlive_its_session(cx: &mut gpui_kit::TestAppContext) {
        use crate::cloud::CloudRepo;
        use crate::workspace::Workspace;

        let _agents = crate::code_run::fixture_agents(Vec::new());
        cx.update(gpui_kit::init);
        // This tests session cleanup, so live Gateway replies and machine changes must not wake its scheduler.
        let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let gateway: std::sync::Arc<dyn crate::backend::Backend> = std::sync::Arc::new(gateway);
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::with_machine_source(
                gateway,
                events,
                arrivals,
                false,
                super::super::machine_source::MachineSource::isolated(),
                window,
                cx,
            );
            workspace._code_poll = gpui_kit::Task::ready(());
            workspace
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _, cx| {
            workspace.cloud_ui.repo = Some(CloudRepo {
                name: "acme/api".into(),
                url: "https://github.com/acme/api".into(),
                private: false,
                pushed_at: None,
            });
            workspace.clear_session(cx);
            assert!(workspace.cloud_ui.repo.is_none());
        });
        cx.run_until_parked();
    }

    #[test]
    fn opening_a_sleeping_session_does_not_wake_it() {
        let mut woken = HashSet::new();
        // Opened and read, nothing typed.
        assert!(!take_wake(&mut woken, "m", true, false));
        assert!(woken.is_empty());
    }

    #[test]
    fn typing_wakes_a_sleeping_machine_once_per_sleep() {
        let mut woken = HashSet::new();
        assert!(take_wake(&mut woken, "m", true, true));
        // More keys while the machines still read it as asleep: no second wake.
        assert!(!take_wake(&mut woken, "m", true, true));
        // Another sleeping session's machine wakes on its own first key.
        assert!(take_wake(&mut woken, "n", true, true));
        // Seen awake (`settle_wakes`), then asleep again: the next typing wakes it again.
        woken.remove("m");
        assert!(take_wake(&mut woken, "m", true, true));
    }

    #[test]
    fn an_awake_machine_is_not_woken_by_typing() {
        let mut woken = HashSet::new();
        assert!(!take_wake(&mut woken, "m", false, true));
        assert!(woken.is_empty());
    }

    #[test]
    fn a_message_to_a_sleeping_machine_shows_as_working_until_it_runs_or_fails() {
        use crate::cloud::{CloudInfo, CloudProgress, CloudState, CloudStep};
        use crate::machines::CloudView;
        use nexus_remote::machine::Status;
        let machine = |state, status, error: Option<&str>| CloudView {
            id: "m".into(),
            name: "cloud".into(),
            info: CloudInfo {
                provider: "fixture".into(),
                size: "default".into(),
                state,
                error: None,
                sleeping_since: None,
                session_id: Some("t_1".into()),
                title: None,
                queued_until: None,
                stop_reason: None,
                repo: None,
            },
            status,
            progress: error.map(|error| CloudProgress {
                machine: "m".into(),
                step: CloudStep::Start,
                detail: None,
                error: Some(error.into()),
            }),
            read: false,
        };
        assert!(delivering(&machine(CloudState::Sleeping, Status::Closed, None)));
        assert!(delivering(&machine(CloudState::Starting, Status::Closed, None)));
        assert!(!delivering(&machine(CloudState::Failed, Status::Closed, None)));
        assert!(!delivering(&machine(
            CloudState::Starting,
            Status::Closed,
            Some("the provider said no")
        )));
    }
}
