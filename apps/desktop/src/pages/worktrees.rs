//! Settings › Projects & worktrees (the Nexus app's `settings/worktrees.tsx`: `worktree-settings.tsx`,
//! `worktree-cleanup-settings.tsx`, `branch-prefix-settings.tsx`, `setup-settings.tsx`, `project-scripts-settings.tsx`): where Code sessions' checkouts go, the rules that clean them up, what their branches
//! start with, and each project with its setup. The folders, rules and prefix are the Nexus app's (`settings.json`),
//! changed here through `nexus-desktop-core` while this build owns the store; the Code sessions this build runs use
//! them at once. The projects are in the Code sessions' store (`workbench.sqlite3`). While this build owns the store,
//! Edit opens one project's settings (`project_editor.rs`); otherwise they are listed only.
//!
//! Whichever app owns the store sweeps worktrees by these rules, a minute after it takes the store and then every hour
//! (`code_run.rs` here). Clean up now runs a sweep at once, and the page shows what the last sweep did. The scheduled
//! sessions are their own view (`schedules.rs`): they change here while this build owns the store.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use nexus_desktop_core::settings::{Settings, WorktreeRoot};
use nexus_remote::cleanup::{LastCleanup, Swept};
use nexus_workbench::cleanup::{Listed, Rule as SweptRule, Rules};
use nexus_workbench::scripts::ProjectScript;
// A project's commands from their JSON list; a list that does not read is none, as the workbench reads it.
use nexus_workbench::scripts::read as read_scripts;

use super::desktop_app::{in_home, off_thread, read_only_note, save_settings, tilde};
use super::project_editor::{ProjectEditor, Saved};
use super::project_host::ProjectHost;
use super::schedules::SchedulesSection;
use super::{Ctx, DeviceCtx, Load, palette, ui};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

/// The Nexus app's defaults (`workbench::worktree_roots`, `worktree-cleanup.ts`).
const WORKTREES_DEFAULT: &str = "nexus-worktrees";
const REVIEWS_DEFAULT: &str = "nexus-reviews";
const IDLE_DAYS_DEFAULT: u32 = 14;

pub fn build(ctx: DeviceCtx, account: Option<Ctx>, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let schedules = cx.new(|cx| SchedulesSection::new(ctx.clone(), account.clone(), window, cx));
    let view = cx.new(|cx| {
        let prefix = cx.new(|cx| InputState::new(window, cx).placeholder("nexus/"));
        let idle_days = cx.new(|cx| InputState::new(window, cx).placeholder(IDLE_DAYS_DEFAULT.to_string()));
        let subscriptions = vec![
            cx.subscribe_in(
                &prefix,
                window,
                |this: &mut WorktreesPage, _, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => this.save_prefix(cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &idle_days,
                window,
                |this: &mut WorktreesPage, _, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => this.save_idle_days(cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ),
        ];
        let mut page = WorktreesPage {
            ctx,
            account,
            settings: Settings::default(),
            projects: Load::Loading,
            server_projects: Vec::new(),
            schedules,
            editing: None,
            cleaned: None,
            cleaning: false,
            listed: None,
            removing: Vec::new(),
            prefix,
            prefix_error: None,
            idle_days,
            synced: false,
            saving: false,
            error: None,
            _subscriptions: subscriptions,
        };
        page.load(cx);
        page
    });
    Some(view.into())
}

/// The idle days as typed: a whole number from 1 to 365, or what is wrong with it (`parseIdleDays`).
fn parse_idle_days(text: &str) -> Result<u32, String> {
    let days: u32 = text.trim().parse().map_err(|_| "Type a number of days.".to_string())?;
    if !(1..=nexus_desktop_core::settings::MAX_IDLE_DAYS).contains(&days) {
        return Err(format!(
            "Pick 1 to {} days.",
            nexus_desktop_core::settings::MAX_IDLE_DAYS
        ));
    }
    Ok(days)
}

/// The rules with one switch turned: `merged`, `archived`, or idle (on with `days`).
fn rules_with(rules: &Rules, rule: Rule, on: bool, days: u32) -> Rules {
    let mut rules = rules.clone();
    match rule {
        Rule::Merged => rules.merged = on,
        Rule::Archived => rules.archived = on,
        Rule::Idle => rules.idle_days = on.then_some(days),
    }
    rules
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    Merged,
    Archived,
    Idle,
}

/// The branch a session named from "fix the login bug" gets (`exampleBranch`).
fn example_branch(prefix: &str) -> String {
    format!("{prefix}fix-the-login-bug-89ab")
}

/// One project in the store.
#[derive(Debug, Clone, PartialEq)]
struct ProjectRow {
    id: String,
    name: String,
    path: String,
    branch: Option<String>,
    pinned: bool,
    archived: bool,
    setup: Option<String>,
    /// Its commands (`project_scripts`).
    scripts: Vec<ProjectScript>,
    /// The server it is on, by id and name; None: this computer.
    server: Option<(String, String)>,
}

/// The store's projects, pinned first and archived last (`pinnedProjectsFirst`). `Ok(None)`: no store here.
fn read_projects() -> Result<Option<Vec<ProjectRow>>, String> {
    let Some(store) = crate::local_store::open()? else {
        return Ok(None);
    };
    let mut rows = Vec::new();
    for project in store.projects().map_err(|error| error.to_string())? {
        let setup = store.project_setup(&project.id).map_err(|error| error.to_string())?;
        let scripts = store.project_scripts(&project.id).map_err(|error| error.to_string())?;
        rows.push(ProjectRow {
            setup: setup.filter(|script| !script.trim().is_empty()),
            scripts: read_scripts(scripts.as_deref()),
            id: project.id,
            name: project.name,
            path: project.path,
            branch: project.branch,
            pinned: project.pinned,
            archived: project.archived,
            server: None,
        });
    }
    rows.sort_by_key(|row| (row.archived, !row.pinned));
    Ok(Some(rows))
}

/// Each connected server's projects, read from its host (`wb_project_setup`, `wb_project_scripts`), pinned first and
/// archived last, server by server. A project whose host does not answer lists without its setup and commands.
async fn read_server_projects(machines: std::sync::Arc<crate::machines::Machines>) -> Vec<ProjectRow> {
    use crate::pages::project_host::on_server;
    let mut rows = Vec::new();
    let connected = machines
        .servers()
        .into_iter()
        .filter(|server| matches!(server.status, nexus_remote::machine::Status::Connected { .. }));
    for server in connected {
        let mut on_this = Vec::new();
        for project in machines.projects_on(&server.id) {
            let args = serde_json::json!({ "projectId": project.id });
            let setup = on_server::<Option<String>>(&machines, &server.id, "wb_project_setup", args.clone());
            let scripts = on_server::<Vec<ProjectScript>>(&machines, &server.id, "wb_project_scripts", args);
            let (setup, scripts) = futures::join!(setup, scripts);
            on_this.push(ProjectRow {
                setup: setup.ok().flatten().filter(|script| !script.trim().is_empty()),
                scripts: scripts.unwrap_or_default(),
                id: project.id,
                name: project.name,
                path: project.path,
                branch: project.branch,
                pinned: project.pinned,
                archived: project.archived,
                server: Some((server.id.clone(), server.name.clone())),
            });
        }
        on_this.sort_by_key(|row| (row.archived, !row.pinned));
        rows.extend(on_this);
    }
    rows
}

/// The row's faint line: where it is, and what it has set up.
fn project_detail(project: &ProjectRow) -> String {
    let mut parts = match &project.server {
        // A server's path is there: no `~` of this computer.
        Some((_, name)) => vec![format!("On {name}"), project.path.clone()],
        None => vec![tilde(&project.path)],
    };
    if let Some(branch) = &project.branch {
        parts.push(branch.clone());
    }
    parts.push(
        if project.setup.is_some() {
            "setup script"
        } else {
            "no setup script"
        }
        .to_string(),
    );
    match project.scripts.len() {
        0 => {}
        1 => parts.push("1 command".into()),
        n => parts.push(format!("{n} commands")),
    }
    parts.join(" · ")
}

/// One line for a sweep: what went and what stayed (the Nexus app's `cleanupHeadline`).
fn cleanup_headline(last: &LastCleanup) -> String {
    let count = |n: usize| format!("{n} worktree{}", if n == 1 { "" } else { "s" });
    match (last.removed.len(), last.kept.len()) {
        (0, 0) => "Nothing to clean up.".into(),
        (removed, 0) => format!("Removed {}.", count(removed)),
        (0, kept) => format!("Kept {}.", count(kept)),
        (removed, kept) => format!("Removed {}. Kept {kept}.", count(removed)),
    }
}

/// Why a rule matched, as the list says it (`RULE_LABELS`).
fn rule_label(rule: SweptRule) -> &'static str {
    match rule {
        SweptRule::Merged => "Pull request merged",
        SweptRule::Archived => "Archived",
        SweptRule::Idle => "Idle",
    }
}

/// A swept session's line: its rule, where it is, and what happened to it.
fn swept_detail(swept: &Swept, removed: bool) -> String {
    let place = swept.machine.as_deref().unwrap_or("This computer");
    let path = match swept.machine {
        Some(_) => swept.entry.path.clone(),
        None => tilde(&swept.entry.path),
    };
    let what = if removed {
        "Removed".to_string()
    } else {
        format!("Kept: {}", swept.entry.kept.as_deref().unwrap_or(""))
    };
    format!("{} · {place} · {path} · {what}", rule_label(swept.entry.rule))
}

/// A session idle this long reads as unused in the list (the idle rule's default).
const UNUSED_AFTER_DAYS: i64 = IDLE_DAYS_DEFAULT as i64;

/// Why a worktree looks unused, as its row says it; None while a session works in it.
fn unused_reason(listed: &Listed, now: i64) -> Option<String> {
    if listed.live {
        return None;
    }
    if listed.missing {
        return Some("Folder already gone".into());
    }
    if listed.thread_id.is_none() {
        return Some("No session uses it".into());
    }
    if listed.archived {
        return Some("Session archived".into());
    }
    let days = listed.updated_at.map(|at| (now - at) / (24 * 60 * 60 * 1000))?;
    (days >= UNUSED_AFTER_DAYS).then(|| format!("Idle {days} days"))
}

/// Unused ones first, the longest unused first; then the rest, the most recent first.
fn sort_listed(listed: &mut [Listed], now: i64) {
    listed.sort_by_key(|item| {
        let at = item.updated_at.unwrap_or(0);
        match unused_reason(item, now) {
            Some(_) => (0, at),
            None => (1, -at),
        }
    });
}

/// A row's second line: its branch, its folder, and whether it is in use.
fn listed_detail(listed: &Listed, now: i64) -> String {
    let state = match unused_reason(listed, now) {
        Some(reason) => reason,
        None if listed.live => "Agent running".into(),
        None => "In use".into(),
    };
    let branch = listed.branch.as_deref().unwrap_or("detached");
    format!("{branch} · {} · {state}", tilde(&listed.path))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

struct WorktreesPage {
    ctx: DeviceCtx,
    account: Option<Ctx>,
    settings: Settings,
    projects: Load<Option<Vec<ProjectRow>>>,
    /// Each connected server's projects, listed after this computer's; none until read.
    server_projects: Vec<ProjectRow>,
    schedules: Entity<SchedulesSection>,
    /// The project open for editing, by id.
    editing: Option<(String, Entity<ProjectEditor>)>,
    /// What Clean up now did, until the page closes; else the runner's last sweep.
    cleaned: Option<LastCleanup>,
    /// Clean up now is running.
    cleaning: bool,
    /// Every worktree (`wb_worktrees`), unused ones first; None while it is read.
    listed: Option<Result<Vec<Listed>, String>>,
    /// The worktrees being removed, by path.
    removing: Vec<String>,
    /// "Branch names": the prefix as typed.
    prefix: Entity<InputState>,
    prefix_error: Option<SharedString>,
    /// "Session idle": the days as typed.
    idle_days: Entity<InputState>,
    /// The fields show the saved values (set once they are read).
    synced: bool,
    /// One save at a time.
    saving: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl WorktreesPage {
    fn load(&mut self, cx: &mut Context<Self>) {
        self.projects = Load::Loading;
        self.synced = false;
        self.schedules.update(cx, |schedules, cx| schedules.load(cx));
        // Later, as `SchedulesSection::load`: the page is built inside a Workspace update, and `projects_now` reads
        // the Workspace.
        cx.spawn(async move |this, cx| this.update(cx, |this, cx| this.read_worktrees(cx)).ok())
            .detach();
        off_thread(
            cx,
            || (crate::desktop_settings::load(), read_projects()),
            |this, (settings, projects), _| {
                this.settings = settings;
                this.projects = match projects {
                    Ok(projects) => Load::Ready(projects),
                    Err(error) => Load::Failed(error.into()),
                };
            },
        );
        self.read_server_projects(cx);
    }

    /// Read the projects again after an edit, keeping the list on screen meanwhile.
    fn reload_projects(&mut self, cx: &mut Context<Self>) {
        off_thread(cx, read_projects, |this, projects, _| {
            if let Ok(projects) = projects {
                this.projects = Load::Ready(projects);
            }
        });
        self.read_server_projects(cx);
    }

    /// The servers' projects, read from their hosts; the list on screen stays meanwhile.
    fn read_server_projects(&mut self, cx: &mut Context<Self>) {
        if self.account.is_none() {
            return;
        }
        let read = read_server_projects(crate::machines::get());
        cx.spawn(async move |this, cx| {
            let rows = read.await;
            this.update(cx, |this, cx| {
                this.server_projects = rows;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The projects' commands, asked now: this build may have lost or taken the store since the page opened.
    fn projects_now(&self, cx: &App) -> Option<crate::code_run::Projects> {
        (self.ctx.code)(cx).projects
    }

    /// Open a project's settings below its row, or close them.
    fn edit(&mut self, project: &ProjectRow, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.as_ref().is_some_and(|(id, _)| *id == project.id) {
            self.editing = None;
            cx.notify();
            return;
        }
        // A server's project is its host's to change; one here, the runner's while this build owns the store.
        let host = match &project.server {
            Some((machine, _)) if self.account.is_some() => ProjectHost::Server {
                machines: crate::machines::get(),
                machine: machine.clone(),
            },
            Some(_) => return,
            None => match self.projects_now(cx) {
                Some(projects) => ProjectHost::Here(projects),
                None => {
                    self.editing = None;
                    cx.notify();
                    return;
                }
            },
        };
        let saved = Saved {
            id: project.id.clone(),
            name: project.name.clone(),
            pinned: project.pinned,
            archived: project.archived,
            git: project.branch.is_some(),
            setup: project.setup.clone(),
            scripts: project.scripts.clone(),
        };
        let page = cx.entity().downgrade();
        let changed = move |cx: &mut App| {
            let _ = page.update(cx, |page, cx| page.reload_projects(cx));
        };
        let editor = cx.new(|cx| ProjectEditor::new(host, saved, changed, window, cx));
        self.editing = Some((project.id.clone(), editor));
        cx.notify();
    }

    /// Clean up now: a sweep with the rules saved, here and on each connected server.
    fn clean_up_now(&mut self, cx: &mut Context<Self>) {
        let Some(projects) = self.projects_now(cx) else { return };
        if self.cleaning {
            return;
        }
        self.cleaning = true;
        self.error = None;
        let answer = projects.clean_up_now();
        cx.spawn(async move |this, cx| {
            let result = answer
                .await
                .unwrap_or_else(|_| Err("The Code sessions stopped before they answered.".into()));
            this.update(cx, |this, cx| {
                this.cleaning = false;
                this.read_worktrees(cx);
                match result {
                    Ok(last) => this.cleaned = Some(last),
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Every worktree, read again: on open, after a sweep and after a removal.
    fn read_worktrees(&mut self, cx: &mut Context<Self>) {
        let Some(projects) = self.projects_now(cx) else {
            self.listed = Some(Err("The app that runs the Code sessions lists their worktrees.".into()));
            return;
        };
        let answer = projects.worktrees();
        cx.spawn(async move |this, cx| {
            let result = answer
                .await
                .unwrap_or_else(|_| Err("The Code sessions stopped before they answered.".into()))
                .map(|mut listed| {
                    sort_listed(&mut listed, now_ms());
                    listed
                });
            this.update(cx, |this, cx| {
                this.listed = Some(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Removes `listed` one after another, then reads the list again. A worktree that would lose work stays, and
    /// its reason shows.
    fn remove(&mut self, listed: Vec<Listed>, cx: &mut Context<Self>) {
        let Some(projects) = self.projects_now(cx) else { return };
        let listed: Vec<Listed> = listed
            .into_iter()
            .filter(|item| !item.live && !self.removing.contains(&item.path))
            .collect();
        if listed.is_empty() {
            return;
        }
        self.error = None;
        self.removing.extend(listed.iter().map(|item| item.path.clone()));
        cx.spawn(async move |this, cx| {
            let mut problems = Vec::new();
            for item in listed {
                let label = tilde(&item.path);
                let path = item.path.clone();
                let result = projects
                    .remove_worktree(item)
                    .await
                    .unwrap_or_else(|_| Err("The Code sessions stopped before they answered.".into()));
                if let Err(why) = result {
                    problems.push(format!("{label}: {why}"));
                }
                let _ = this.update(cx, |this, cx| {
                    this.removing.retain(|removing| *removing != path);
                    cx.notify();
                });
            }
            this.update(cx, |this, cx| {
                if !problems.is_empty() {
                    this.error = Some(problems.join("\n").into());
                }
                this.read_worktrees(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// "Your worktrees": each one with Remove, unused ones first, and Remove unused for all of those at once.
    fn render_listed(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let now = now_ms();
        let head = ui::section_head("Your worktrees", palette);
        let note = ui::note(
            "Every session's checkout, and folders in the worktrees folders no session uses. Remove takes one off the \
             disk only when nothing would be lost; its branch stays.",
            palette,
        );
        let listed = match &self.listed {
            None => {
                return v_flex()
                    .child(head)
                    .child(note)
                    .child(ui::note("Reading worktrees…", palette));
            }
            Some(Err(error)) => return v_flex().child(head).child(note).child(ui::note(error.clone(), palette)),
            Some(Ok(listed)) => listed,
        };
        if listed.is_empty() {
            return v_flex()
                .child(head)
                .child(note)
                .child(ui::note("No worktrees.", palette));
        }
        let unused: Vec<Listed> = listed
            .iter()
            .filter(|item| unused_reason(item, now).is_some() && !self.removing.contains(&item.path))
            .cloned()
            .collect();
        let remove_unused = if unused.is_empty() {
            ui::disabled_button("worktrees-remove-unused", "Remove unused", palette).into_any_element()
        } else {
            let count = unused.len();
            ui::button("worktrees-remove-unused", format!("Remove unused ({count})"), palette)
                .on_click(cx.listener(move |this, _, _, cx| this.remove(unused.clone(), cx)))
                .into_any_element()
        };
        let rows = listed.iter().enumerate().map(|(index, item)| {
            let title = match (&item.title, &item.thread_id) {
                (Some(title), Some(_)) => title.clone(),
                _ => "No session".to_string(),
            };
            let removing = self.removing.contains(&item.path);
            let button = if item.live || removing {
                let text = if removing { "Removing…" } else { "Remove" };
                ui::disabled_button(("worktree-remove", index), text, palette).into_any_element()
            } else {
                let item = item.clone();
                ui::button(("worktree-remove", index), "Remove", palette)
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(vec![item.clone()], cx)))
                    .into_any_element()
            };
            ui::row(palette)
                .child(ui::two_lines(title, listed_detail(item, now), palette).flex_1())
                .child(button)
        });
        v_flex()
            .child(head)
            .child(note)
            .child(ui::setting(
                "Unused worktrees",
                "No session uses them, their session is archived, or it was idle for two weeks.",
                remove_unused,
                palette,
            ))
            .children(rows)
    }

    /// The last sweep's row, Clean up now, and what the sweep did.
    fn render_last_cleanup(&self, rules: &Rules, palette: Palette, cx: &mut Context<Self>) -> Div {
        let projects = self.projects_now(cx);
        let last = self
            .cleaned
            .clone()
            .or_else(|| projects.as_ref().and_then(|projects| projects.last_cleanup()));
        let detail = match &last {
            Some(last) => {
                let at = chrono::DateTime::from_timestamp_millis(last.at)
                    .map(|at| at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                format!("{} {at}", cleanup_headline(last))
            }
            None if projects.is_some() => "Not run since this build took the Code sessions.".into(),
            None => "The app that runs the Code sessions keeps its report.".into(),
        };
        let button = if projects.is_some() && rules.any() && !self.cleaning {
            ui::button("cleanup-now", "Clean up now", palette)
                .on_click(cx.listener(|this, _, _, cx| this.clean_up_now(cx)))
                .into_any_element()
        } else {
            let text = if self.cleaning {
                "Cleaning up…"
            } else {
                "Clean up now"
            };
            ui::disabled_button("cleanup-now", text, palette).into_any_element()
        };
        let mut section = v_flex().child(ui::setting("Last cleanup", detail, button, palette));
        if let Some(last) =
            last.filter(|last| !last.removed.is_empty() || !last.kept.is_empty() || !last.problems.is_empty())
        {
            let swept = last
                .removed
                .iter()
                .map(|swept| (swept, true))
                .chain(last.kept.iter().map(|swept| (swept, false)));
            section = section
                .child(ui::label("What it did", palette))
                .children(swept.map(|(swept, removed)| {
                    ui::row(palette)
                        .child(ui::two_lines(swept.entry.title.clone(), swept_detail(swept, removed), palette).flex_1())
                }))
                .children(
                    last.problems
                        .iter()
                        .map(|problem| ui::error_line(problem.clone(), palette)),
                );
        }
        section
    }

    /// Save one change, then show the settings as saved.
    fn save(
        &mut self,
        change: impl FnOnce(&mut Settings) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.saving {
            return;
        }
        self.saving = true;
        self.error = None;
        save_settings(&self.ctx, cx, change, |this, saved, _| {
            this.saving = false;
            match saved {
                Ok((settings, ())) => {
                    this.settings = settings;
                    this.synced = false;
                }
                Err(error) => this.error = Some(error.into()),
            }
        });
        cx.notify();
    }

    fn set_root(&mut self, root: WorktreeRoot, path: Option<String>, cx: &mut Context<Self>) {
        self.save(move |settings| settings.set_worktree_root(root, path), cx);
    }

    /// "Change…": the folder picker, then the save.
    fn pick_root(&mut self, root: WorktreeRoot, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Use this folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.set_root(root, Some(path.to_string_lossy().into_owned()), cx);
            });
        })
        .detach();
    }

    fn rules(&self) -> Rules {
        self.settings.worktree_cleanup.clone().unwrap_or_default()
    }

    fn typed_days(&self, cx: &App) -> Result<u32, String> {
        parse_idle_days(self.idle_days.read(cx).value().as_ref())
    }

    fn toggle(&mut self, rule: Rule, cx: &mut Context<Self>) {
        let rules = self.rules();
        let on = match rule {
            Rule::Merged => !rules.merged,
            Rule::Archived => !rules.archived,
            Rule::Idle => rules.idle_days.is_none(),
        };
        let days = self.typed_days(cx).unwrap_or(IDLE_DAYS_DEFAULT);
        let next = rules_with(&rules, rule, on, days);
        self.save(move |settings| settings.set_cleanup_rules(next), cx);
    }

    /// The idle days typed, saved while the idle rule is on.
    fn save_idle_days(&mut self, cx: &mut Context<Self>) {
        let rules = self.rules();
        let Some(saved) = rules.idle_days else { return };
        match self.typed_days(cx) {
            Ok(days) if days != saved => {
                let next = rules_with(&rules, Rule::Idle, true, days);
                self.save(move |settings| settings.set_cleanup_rules(next), cx);
            }
            Ok(_) => {}
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
            }
        }
    }

    fn save_prefix(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let typed = self.prefix.read(cx).value().to_string();
        self.saving = true;
        self.prefix_error = None;
        save_settings(
            &self.ctx,
            cx,
            move |settings| settings.set_branch_prefix(Some(typed)).map(|_| ()),
            |this, saved, _| {
                this.saving = false;
                match saved {
                    Ok((settings, ())) => {
                        this.settings = settings;
                        this.synced = false;
                    }
                    Err(error) => this.prefix_error = Some(error.into()),
                }
            },
        );
        cx.notify();
    }

    fn reset_prefix(&mut self, cx: &mut Context<Self>) {
        self.save(|settings| settings.set_branch_prefix(None).map(|_| ()), cx);
    }

    /// Put the saved prefix and days in their fields, once after each read or save.
    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.synced || matches!(self.projects, Load::Loading) {
            return;
        }
        self.synced = true;
        let prefix = self.settings.branch_prefix_or_default();
        let days = self.rules().idle_days.unwrap_or(IDLE_DAYS_DEFAULT).to_string();
        self.prefix.update(cx, |state, cx| state.set_value(prefix, window, cx));
        self.idle_days.update(cx, |state, cx| state.set_value(days, window, cx));
    }

    fn folder(
        &self,
        root: WorktreeRoot,
        title: &'static str,
        note: &'static str,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let (chosen, default, id) = match root {
            WorktreeRoot::Worktrees => (&self.settings.worktrees_dir, WORKTREES_DEFAULT, "worktrees"),
            WorktreeRoot::Reviews => (&self.settings.reviews_dir, REVIEWS_DEFAULT, "reviews"),
        };
        let path = chosen.clone().unwrap_or_else(|| in_home(default));
        let editable = crate::desktop_settings::writable() && !self.saving;
        let mut controls = h_flex().gap(px(6.)).flex_none();
        if chosen.is_some() && editable {
            controls = controls.child(
                ui::button(SharedString::from(format!("{id}-reset")), "Reset", palette)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_root(root, None, cx))),
            );
        }
        controls = controls.child(if editable {
            ui::button(SharedString::from(format!("{id}-change")), "Change…", palette)
                .on_click(cx.listener(move |this, _, window, cx| this.pick_root(root, window, cx)))
                .into_any_element()
        } else {
            ui::disabled_button(SharedString::from(format!("{id}-change")), "Change…", palette).into_any_element()
        });
        h_flex()
            .justify_between()
            .gap(px(16.))
            .py(px(12.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).text_color(palette.strong).child(title))
                    .child(div().text_size(px(12.)).text_color(palette.faint).child(note))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_family(ui::MONO)
                            .text_color(palette.muted)
                            .truncate()
                            .child(tilde(&path)),
                    ),
            )
            .child(controls)
    }

    fn rule_switch(
        &self,
        id: &'static str,
        rule: Rule,
        on: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let locked = !crate::desktop_settings::writable() || self.saving;
        let switch = ui::switch(id, on, locked, palette);
        if locked {
            switch.into_any_element()
        } else {
            switch
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(rule, cx)))
                .into_any_element()
        }
    }

    fn render_prefix(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let typed = self.prefix.read(cx).value().to_string();
        let saved = self.settings.branch_prefix_or_default();
        let problem = nexus_workbench::git::check_branch_prefix(typed.trim()).err();
        let detail = problem.clone().unwrap_or_else(|| {
            if typed.trim().is_empty() {
                format!("No prefix: {}", example_branch(""))
            } else {
                format!("For example {}", example_branch(typed.trim()))
            }
        });
        let editable = crate::desktop_settings::writable() && !self.saving;
        let mut controls = h_flex().gap(px(6.)).flex_none().child(
            div()
                .w(px(160.))
                .child(ui::input_box(&self.prefix, palette).font_family(ui::MONO)),
        );
        let dirty = typed.trim() != saved;
        let writable = crate::desktop_settings::writable();
        if writable {
            let active = editable && dirty && problem.is_none();
            let focus = self
                .ctx
                .settings_fields
                .focus("branch-prefix-save", cx)
                .tab_stop(active);
            let label = if self.saving {
                "Saving…"
            } else if self.prefix_error.is_some() && dirty {
                "Retry"
            } else if !dirty {
                "Saved"
            } else {
                "Save"
            };
            let save = if active {
                ui::button("prefix-save", label, palette).on_click(cx.listener(|this, _, _, cx| this.save_prefix(cx)))
            } else {
                ui::disabled_button("prefix-save", label, palette)
            };
            controls = controls.child(save.track_focus(&focus));
            let focus = self
                .ctx
                .settings_fields
                .focus("branch-prefix-revert", cx)
                .tab_stop(editable && dirty);
            let revert = if editable && dirty {
                ui::button("prefix-revert", "Revert", palette).on_click(cx.listener(|this, _, window, cx| {
                    let saved = this.settings.branch_prefix_or_default().to_string();
                    this.prefix.update(cx, |state, cx| state.set_value(saved, window, cx));
                    this.prefix_error = None;
                    cx.notify();
                }))
            } else {
                ui::disabled_button("prefix-revert", "Revert", palette)
            };
            controls = controls.child(revert.track_focus(&focus));
        }
        if editable && !dirty && self.settings.branch_prefix.is_some() {
            controls = controls.child(
                ui::button("prefix-reset", "Reset", palette)
                    .on_click(cx.listener(|this, _, _, cx| this.reset_prefix(cx))),
            );
        }
        v_flex()
            .child(ui::setting("Prefix", detail, controls.flex_wrap(), palette))
            .children(self.prefix_error.clone().map(|error| ui::error_line(error, palette)))
            .when(self.prefix_error.is_some() && typed.trim() != saved, |column| {
                column.child(ui::note(
                    format!("Not saved. The saved prefix is {saved}. Your draft stays above."),
                    palette,
                ))
            })
    }

    fn render_projects(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let owned = self.projects_now(cx).is_some();
        let section = v_flex()
            .child(ui::section_head("Projects", palette))
            .when(self.ctx.local_workspace, |section| {
                section.child(ui::requires_sign_in_row(
                    "Server projects",
                    "View and edit projects saved on your servers.",
                    palette,
                ))
            });
        let servers = &self.server_projects;
        let (here, failed): (&[ProjectRow], Option<SharedString>) = match &self.projects {
            Load::Loading => return section.child(ui::skeleton(3, palette)),
            Load::Failed(error) if servers.is_empty() => return section.child(ui::failed(error.clone(), palette)),
            Load::Failed(error) => (&[], Some(error.clone())),
            Load::Ready(None) if servers.is_empty() => {
                return section.child(ui::empty(
                    "No Code sessions on this computer yet",
                    "Open a project in Code, here or in the Nexus app, and it shows up here.",
                    palette,
                ));
            }
            Load::Ready(None) => (&[], None),
            Load::Ready(Some(projects)) if projects.is_empty() && servers.is_empty() => {
                return section.child(ui::empty(
                    "No projects yet",
                    "Open a project in Code to set its setup script and commands.",
                    palette,
                ));
            }
            Load::Ready(Some(projects)) => (projects.as_slice(), None),
        };
        section
            .child(ui::note(
                "Each project's setup script runs in a new session's worktree before the agent starts, for example npm \
                 ci. Its output shows in the session; if it fails, the session still starts. Its commands run in a new \
                 terminal of the session's folder. A server's projects are kept on that server.",
                palette,
            ))
            .children(failed.map(|error| ui::failed(error, palette)))
            .children(here.iter().chain(servers).map(|project| {
                let editor = self
                    .editing
                    .as_ref()
                    .filter(|(id, _)| *id == project.id)
                    .map(|(_, editor)| editor.clone());
                let open = editor.is_some();
                let row = ui::row(palette)
                    .child(ui::two_lines(project.name.clone(), project_detail(project), palette).flex_1())
                    .when(project.pinned, |row| row.child(ui::badge("Pinned", palette)))
                    .when(project.archived, |row| row.child(ui::badge("Archived", palette)))
                    // A server's host owns its projects: it edits them whichever app owns this computer's store.
                    .when(owned || project.server.is_some(), |row| {
                        let clicked = project.clone();
                        row.child(
                            ui::ghost_button(
                                SharedString::from(format!("project-edit-{}", project.id)),
                                if open { "Done" } else { "Edit" },
                                palette,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| this.edit(&clicked, window, cx))),
                        )
                    });
                v_flex().child(row).children(editor)
            }))
    }
}

impl Render for WorktreesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_fields(window, cx);
        let palette = palette(cx);
        let rules = self.rules();
        let idle_detail = match self.typed_days(cx) {
            Err(problem) => problem,
            Ok(_) => "No activity for this many days.".to_string(),
        };
        v_flex()
            .w_full()
            .child(ui::lead(
                "Where Code sessions work, how their worktrees are cleaned up, and each project's setup.",
                palette,
            ))
            .children(read_only_note(palette))
            .child(ui::section_head("Worktrees", palette))
            .child(ui::note(
                "Where Code puts a session's own checkout. Sessions already started stay where they are.",
                palette,
            ))
            .child(self.ctx.settings_fields.field(
                "worktrees",
                self.folder(
                    WorktreeRoot::Worktrees,
                    "New work",
                    "Branches you start, to open a pull request",
                    palette,
                    cx,
                ),
                None,
                cx,
            ))
            .child(self.folder(
                WorktreeRoot::Reviews,
                "Reviews",
                "Pull requests you check out to review",
                palette,
                cx,
            ))
            .child(self.ctx.settings_fields.field(
                "worktree-cleanup",
                ui::section_head("Worktree cleanup", palette),
                None,
                cx,
            ))
            .child(ui::note(
                "Remove a session's worktree by itself, only when nothing would be lost. Its branch stays. The app \
                 that runs Code sessions checks every hour.",
                palette,
            ))
            .child(ui::setting(
                "Pull request merged",
                "The session's pull request merged, and its branch has nothing newer.",
                self.rule_switch("cleanup-merged", Rule::Merged, rules.merged, palette, cx),
                palette,
            ))
            .child(ui::setting(
                "Session archived",
                "You archived the session.",
                self.rule_switch("cleanup-archived", Rule::Archived, rules.archived, palette, cx),
                palette,
            ))
            .child(ui::setting(
                "Session idle",
                idle_detail,
                h_flex()
                    .gap(px(12.))
                    .flex_none()
                    // The days only count while the rule is on: faint until then.
                    .child(
                        div()
                            .w(px(56.))
                            .when(rules.idle_days.is_none(), |field| field.opacity(0.5))
                            .child(ui::input_box(&self.idle_days, palette)),
                    )
                    .child(self.rule_switch("cleanup-idle", Rule::Idle, rules.idle_days.is_some(), palette, cx)),
                palette,
            ))
            .child(self.render_last_cleanup(&rules, palette, cx))
            .child(self.render_listed(palette, cx))
            .child(ui::section_head("Branch names", palette))
            .child(ui::note(
                "What a new worktree's branch starts with, when you don't name it. Sessions you already started keep \
                 theirs.",
                palette,
            ))
            .child(self.ctx.settings_fields.field(
                "branch-prefix",
                self.render_prefix(palette, cx),
                Some(self.prefix.read(cx).focus_handle(cx)),
                cx,
            ))
            .children(self.error.clone().map(|error| ui::error_line(error, palette)))
            .child(
                self.ctx
                    .settings_fields
                    .field("setup-scripts", self.render_projects(palette, cx), None, cx),
            )
            .child(self.schedules.clone())
            .child(
                h_flex().mt(px(10.)).child(
                    ui::button("worktrees-refresh", "Refresh", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn listed(thread: bool, archived: bool, days_ago: i64, live: bool) -> Listed {
        Listed {
            path: format!("/w/repo/{thread}-{archived}-{days_ago}-{live}"),
            branch: Some("nexus/fix".into()),
            thread_id: thread.then(|| "t_1".into()),
            title: thread.then(|| "Fix it".into()),
            archived,
            updated_at: Some(NOW - days_ago * 24 * 60 * 60 * 1000),
            live,
            missing: false,
        }
    }

    const NOW: i64 = 100 * 24 * 60 * 60 * 1000;

    #[test]
    fn says_why_a_worktree_is_unused_and_lists_those_first() {
        assert_eq!(
            unused_reason(&listed(false, false, 0, false), NOW).as_deref(),
            Some("No session uses it")
        );
        assert_eq!(
            unused_reason(&listed(true, true, 0, false), NOW).as_deref(),
            Some("Session archived")
        );
        assert_eq!(
            unused_reason(&listed(true, false, 20, false), NOW).as_deref(),
            Some("Idle 20 days")
        );
        assert_eq!(unused_reason(&listed(true, false, 2, false), NOW), None);
        // A running agent's worktree is in use, archived or not.
        assert_eq!(unused_reason(&listed(true, true, 30, true), NOW), None);
        let mut all = vec![
            listed(true, false, 1, false),
            listed(false, false, 3, false),
            listed(true, false, 0, false),
            listed(true, true, 9, false),
        ];
        sort_listed(&mut all, NOW);
        let order: Vec<_> = all
            .iter()
            .map(|item| (item.thread_id.is_some(), item.archived, item.updated_at))
            .collect();
        assert_eq!(
            order,
            [
                (true, true, Some(NOW - 9 * 86_400_000)),
                (false, false, Some(NOW - 3 * 86_400_000)),
                (true, false, Some(NOW)),
                (true, false, Some(NOW - 86_400_000)),
            ]
        );
    }

    #[test]
    fn saves_the_folders_rules_and_prefix() {
        let file = crate::desktop_settings::TempFile::new("worktrees");
        file.update(|settings| settings.set_worktree_root(WorktreeRoot::Worktrees, Some("/w".into())))
            .expect("a folder");
        let rules = rules_with(&Rules::default(), Rule::Merged, true, IDLE_DAYS_DEFAULT);
        let rules = rules_with(&rules, Rule::Idle, true, parse_idle_days(" 30 ").expect("days"));
        file.update(|settings| settings.set_cleanup_rules(rules.clone()))
            .expect("rules");
        file.update(|settings| settings.set_branch_prefix(Some("me/".into())).map(|_| ()))
            .expect("a prefix");
        let saved = nexus_desktop_core::settings::load(file.path());
        assert_eq!(saved.worktrees_dir.as_deref(), Some("/w"));
        assert_eq!(saved.worktree_cleanup, Some(rules));
        assert_eq!(saved.branch_prefix.as_deref(), Some("me/"));

        // Off again: no rule is kept at all, and the default prefix is not saved.
        let none = rules_with(
            &rules_with(&saved.worktree_cleanup.unwrap_or_default(), Rule::Merged, false, 0),
            Rule::Idle,
            false,
            0,
        );
        file.update(|settings| settings.set_cleanup_rules(none)).expect("off");
        file.update(|settings| settings.set_branch_prefix(None).map(|_| ()))
            .expect("reset");
        let saved = nexus_desktop_core::settings::load(file.path());
        assert_eq!((saved.worktree_cleanup, saved.branch_prefix), (None, None));
        assert!(
            file.update(|settings| settings.set_branch_prefix(Some("a b".into())).map(|_| ()))
                .is_err()
        );
    }

    #[test]
    fn checks_the_idle_days_as_typed() {
        assert_eq!(parse_idle_days("14"), Ok(14));
        assert!(parse_idle_days("0").is_err());
        assert!(parse_idle_days("400").is_err());
        assert!(parse_idle_days("soon").is_err());
        assert_eq!(example_branch("nexus/"), "nexus/fix-the-login-bug-89ab");
    }

    #[test]
    fn reads_a_projects_commands() {
        let scripts = read_scripts(Some(
            r#"[{"id":"c-1","name":"Test","command":"npm test"},{"id":"c-2","name":"Dev","command":"npm run dev","shortcut":"Mod+Alt+KeyR"}]"#,
        ));
        assert_eq!(scripts.len(), 2);
        assert_eq!(scripts[1].shortcut.as_deref(), Some("Mod+Alt+KeyR"));
        assert!(read_scripts(Some("not json")).is_empty());
        assert!(read_scripts(None).is_empty());
    }

    /// Each connected server's projects list with their server, their setup and their commands, read on its host.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lists_a_servers_projects_read_on_its_host() {
        use crate::machines::tests::{Fake, machines, settings, until};
        let fake = Fake::new(nexus_workbench::api::PROTOCOL);
        let machines = machines(&fake);
        machines.sync(&settings(&[("m1", "me@box")]));
        until("the overview", || !machines.projects().is_empty()).await;
        if let Ok(mut host) = fake.host.lock() {
            host.project_setup = Some("npm ci".into());
            host.scripts = serde_json::json!([{ "id": "c-1", "name": "Test", "command": "npm test" }]);
        }
        let rows = read_server_projects(machines.clone()).await;
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!((row.id.as_str(), row.name.as_str()), ("p1", "api"));
        assert_eq!(row.server, Some(("m1".to_string(), "m1".to_string())));
        assert_eq!(row.setup.as_deref(), Some("npm ci"));
        assert_eq!(row.scripts.len(), 1);
        assert_eq!(
            project_detail(row),
            "On m1 · /srv/api · main · setup script · 1 command"
        );
        // A server that leaves lists nothing.
        machines.sync(&settings(&[]));
        assert!(read_server_projects(machines).await.is_empty());
    }

    #[test]
    fn says_what_a_sweep_did() {
        let entry = |rule, kept: Option<&str>| nexus_workbench::cleanup::Entry {
            thread_id: "t-1".into(),
            title: "Fix login".into(),
            path: "/srv/w/fix".into(),
            branch: "nexus/fix".into(),
            rule,
            kept: kept.map(String::from),
        };
        let mut last = LastCleanup::default();
        assert_eq!(cleanup_headline(&last), "Nothing to clean up.");
        last.removed.push(Swept {
            machine: None,
            entry: entry(SweptRule::Merged, None),
        });
        assert_eq!(cleanup_headline(&last), "Removed 1 worktree.");
        last.kept.push(Swept {
            machine: Some("build-box".into()),
            entry: entry(SweptRule::Idle, Some("Uncommitted changes")),
        });
        assert_eq!(cleanup_headline(&last), "Removed 1 worktree. Kept 1.");
        assert_eq!(
            swept_detail(&last.kept[0], false),
            "Idle · build-box · /srv/w/fix · Kept: Uncommitted changes"
        );
        last.removed.clear();
        assert_eq!(cleanup_headline(&last), "Kept 1 worktree.");
    }
}
