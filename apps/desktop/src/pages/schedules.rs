//! Settings › Projects & worktrees › Scheduled sessions (the Nexus app's `schedule-settings.tsx` and
//! `workbench/schedules.ts`): a prompt that starts a new session on a project at an interval, at a time on chosen days,
//! or from its webhook. The list shows each one's next run and last runs; the form adds or edits one.
//!
//! The schedules live in the Code sessions store, and `nexus-workbench` decides when they run. While this build owns
//! the store, the list and every change go through the runner's [`Schedules`] commands, and its upkeep starts the runs
//! (`code_run.rs`). Otherwise the list is read from the store and nothing changes here: the Nexus app runs them.
//!
//! [`Draft`] is the form as data, so the edit flow is tested without a window.

use std::time::Duration;

use chrono::{Local, TimeZone};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::driver::label as agent_label;
use nexus_workbench::event::{Effort, PermissionMode, SessionAgent, Via};
use nexus_workbench::schedules::{
    DEFAULT_RUNS_PER_DAY, Run, RunStatus, Schedule, ScheduleInput, ScheduleView, Trigger, WebhookSecret, When,
};

use super::desktop_app::{off_thread, owned_note};
use super::{Ctx, DeviceCtx, Load, palette, ui};
use crate::code_run::{Answer, Schedules};
use crate::palette::Palette;

const DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
/// The agents a schedule may start, as the Nexus app's form offers them.
const AGENTS: [SessionAgent; 2] = [SessionAgent::Claude, SessionAgent::Codex];
/// How often the list is read again, for the next runs and the last ones (the Nexus app's 30 seconds).
const REFRESH: Duration = Duration::from_secs(30);
const READ_ONLY: &str = "The Nexus app runs the schedules while it holds the Code sessions. They are read-only here.";

/// `09:30` from minutes after midnight.
pub(super) fn clock_label(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

/// Minutes after midnight from `9:30` or `09:30`; None when it is not a time of day.
pub(super) fn parse_clock(text: &str) -> Option<u16> {
    let (hours, minutes) = text.trim().split_once(':')?;
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if !(1..=2).contains(&hours.len()) || minutes.len() != 2 || !digits(hours) || !digits(minutes) {
        return None;
    }
    let (hours, minutes) = (hours.parse::<u16>().ok()?, minutes.parse::<u16>().ok()?);
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

/// "Every 2 hours", "Mon, Thu at 09:00", "Weekdays at 08:30", "Only when started".
pub(super) fn describe_when(when: &When) -> String {
    const DAY: u32 = 24 * 60;
    match when {
        When::Manual => "Only when started".into(),
        When::Interval { minutes } if minutes % DAY == 0 => match minutes / DAY {
            1 => "Every day".into(),
            days => format!("Every {days} days"),
        },
        When::Interval { minutes } if minutes % 60 == 0 => match minutes / 60 {
            1 => "Every hour".into(),
            hours => format!("Every {hours} hours"),
        },
        When::Interval { minutes } => format!("Every {minutes} minutes"),
        When::Weekly { days, minute } => {
            let mut days = days.clone();
            days.sort_unstable();
            let label = match days.as_slice() {
                [0, 1, 2, 3, 4, 5, 6] => "Every day".to_string(),
                [1, 2, 3, 4, 5] => "Weekdays".to_string(),
                [0, 6] => "Weekends".to_string(),
                days => days
                    .iter()
                    .filter_map(|day| DAY_NAMES.get(usize::from(*day)))
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            format!("{label} at {}", clock_label(*minute))
        }
    }
}

/// What a run did, in a line: "Started", "Skipped: …", "Failed: …".
pub(super) fn describe_run(run: &Run) -> String {
    let how = match run.trigger {
        Trigger::Webhook => " from its webhook",
        Trigger::Now => " with Run now",
        Trigger::Time => "",
    };
    match run.status {
        RunStatus::Started => format!("Started{how}"),
        RunStatus::Skipped => run.note.clone().unwrap_or_else(|| "Skipped".into()),
        RunStatus::Failed => format!(
            "Failed{how}: {}",
            run.note.as_deref().unwrap_or("the session did not start")
        ),
    }
}

/// A time as the list shows it: "Mon, Nov 9, 09:00", in this computer's time zone.
fn at(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map_or_else(String::new, |time| time.format("%a, %b %-d, %H:%M").to_string())
}

/// The form's input for a saved schedule (`inputOf`).
pub(super) fn input_of(schedule: &Schedule) -> ScheduleInput {
    ScheduleInput {
        project_id: schedule.project_id.clone(),
        title: Some(schedule.title.clone()),
        prompt: schedule.prompt.clone(),
        agent: schedule.agent,
        model: schedule.model.clone(),
        permission_mode: schedule.permission_mode,
        effort: schedule.effort,
        via: schedule.via,
        worktree: schedule.worktree,
        when: schedule.when.clone(),
        enabled: schedule.enabled,
        max_runs_per_day: Some(schedule.max_runs_per_day),
        catch_up: schedule.catch_up,
    }
}

/// The schedule turned off, or on again ("Turn off", "Turn on"): nothing else changes.
pub(super) fn toggled(schedule: &Schedule) -> ScheduleInput {
    ScheduleInput {
        enabled: !schedule.enabled,
        ..input_of(schedule)
    }
}

/// How a schedule's times are picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Interval,
    Weekly,
    Manual,
}

/// The add or edit form, as data. The text fields are strings as typed; [`Draft::input`] reads them.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Draft {
    /// None: a new schedule.
    pub id: Option<String>,
    pub project_id: String,
    pub agent: SessionAgent,
    /// Kept as saved: the form does not pick a model or effort, as the Nexus app's does not.
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub mode: PermissionMode,
    pub via: Via,
    pub kind: Kind,
    pub days: Vec<u8>,
    pub worktree: bool,
    pub catch_up: bool,
    pub enabled: bool,
    pub title: String,
    pub prompt: String,
    pub minutes: String,
    pub clock: String,
    pub max_runs: String,
}

impl Draft {
    /// A new schedule on `project_id`: every hour, in a new worktree, catching up a missed run (`blank()`).
    pub fn blank(project_id: Option<&str>, signed_in: bool) -> Draft {
        Draft {
            id: None,
            project_id: project_id.unwrap_or_default().to_string(),
            agent: SessionAgent::Claude,
            model: None,
            effort: None,
            mode: PermissionMode::Default,
            via: if signed_in { Via::Nexus } else { Via::Subscription },
            kind: Kind::Interval,
            days: vec![1, 2, 3, 4, 5],
            worktree: true,
            catch_up: true,
            enabled: true,
            title: String::new(),
            prompt: String::new(),
            minutes: "60".into(),
            clock: "09:00".into(),
            max_runs: DEFAULT_RUNS_PER_DAY.to_string(),
        }
    }

    /// The form for a saved schedule.
    pub fn edit(schedule: &Schedule) -> Draft {
        let mut draft = Draft {
            id: Some(schedule.id.clone()),
            project_id: schedule.project_id.clone(),
            agent: schedule.agent,
            model: schedule.model.clone(),
            effort: schedule.effort,
            mode: schedule.permission_mode,
            via: schedule.via,
            worktree: schedule.worktree,
            catch_up: schedule.catch_up,
            enabled: schedule.enabled,
            title: schedule.title.clone(),
            prompt: schedule.prompt.clone(),
            max_runs: schedule.max_runs_per_day.to_string(),
            ..Draft::blank(None, true)
        };
        match &schedule.when {
            When::Interval { minutes } => draft.minutes = minutes.to_string(),
            When::Weekly { days, minute } => {
                draft.kind = Kind::Weekly;
                draft.days = days.clone();
                draft.clock = clock_label(*minute);
            }
            When::Manual => draft.kind = Kind::Manual,
        }
        draft
    }

    /// Pick an agent: the saved model was the other agent's, so it goes back to the default.
    pub fn set_agent(&mut self, agent: SessionAgent) {
        if agent != self.agent {
            self.agent = agent;
            self.model = None;
        }
    }

    pub fn toggle_day(&mut self, day: u8) {
        match self.days.iter().position(|picked| *picked == day) {
            Some(at) => {
                self.days.remove(at);
            }
            None => {
                self.days.push(day);
                self.days.sort_unstable();
            }
        }
    }

    /// What is saved. The crate checks the rest (an interval's range, a title's length, the daily cap's range).
    pub fn input(&self, signed_in: bool) -> Result<ScheduleInput, String> {
        if !signed_in && self.via == Via::Nexus {
            return Err(ui::REQUIRES_SIGN_IN.into());
        }
        if self.project_id.is_empty() {
            return Err("Open a project in Code first.".into());
        }
        if self.prompt.trim().is_empty() {
            return Err("Give the schedule a prompt.".into());
        }
        let when = match self.kind {
            Kind::Manual => When::Manual,
            Kind::Interval => When::Interval {
                minutes: self
                    .minutes
                    .trim()
                    .parse()
                    .map_err(|_| "The minutes between runs is a whole number.".to_string())?,
            },
            Kind::Weekly => When::Weekly {
                days: self.days.clone(),
                minute: parse_clock(&self.clock).ok_or("The time is HH:MM, 00:00 to 23:59.")?,
            },
        };
        let max_runs_per_day = self
            .max_runs
            .trim()
            .parse()
            .map_err(|_| "The runs a day is a whole number.".to_string())?;
        Ok(ScheduleInput {
            project_id: self.project_id.clone(),
            title: Some(self.title.trim().to_string()).filter(|title| !title.is_empty()),
            prompt: self.prompt.clone(),
            agent: self.agent,
            model: self.model.clone(),
            permission_mode: self.mode,
            effort: self.effort,
            via: self.via,
            worktree: self.worktree,
            when,
            enabled: self.enabled,
            max_runs_per_day: Some(max_runs_per_day),
            catch_up: self.catch_up,
        })
    }
}

/// The store's open projects (id, name) and, when this build does not own it, its schedules for `scope`.
type Read = Result<(Vec<(String, String)>, Option<Vec<Schedule>>), String>;

fn read_store(scope: Option<String>, owned: bool) -> Read {
    let Some(store) = crate::local_store::open()? else {
        return Ok((Vec::new(), Some(Vec::new())));
    };
    let projects = store
        .projects()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|project| !project.archived)
        .map(|project| (project.id, project.name))
        .collect();
    let schedules = match (owned, scope) {
        (true, _) => None,
        (false, Some(scope)) => Some(store.schedules(&scope).map_err(|error| error.to_string())?),
        (false, None) => Some(Vec::new()),
    };
    Ok((projects, schedules))
}

/// The section, a view inside the Projects & worktrees page.
pub(super) struct SchedulesSection {
    ctx: DeviceCtx,
    account: Option<Ctx>,
    list: Load<Vec<ScheduleView>>,
    projects: Vec<(String, String)>,
    /// This build owns the store: the list changes here.
    owned: bool,
    note: Option<String>,
    plans: bool,
    draft: Option<Draft>,
    title: Entity<InputState>,
    prompt: Entity<TextareaState>,
    minutes: Entity<InputState>,
    clock: Entity<InputState>,
    max_runs: Entity<InputState>,
    /// A command runs for this schedule (None: the form).
    busy: Option<Option<String>>,
    /// The last command's failure, and whose (None: the form's).
    problem: Option<(Option<String>, String)>,
    /// A webhook's token, shown once after it is made.
    secret: Option<(String, WebhookSecret)>,
    /// What Run now did, under its schedule.
    started: Option<String>,
    /// Delete was pressed once on this schedule.
    confirm: Option<String>,
    _refresh: Task<()>,
}

impl SchedulesSection {
    pub fn new(ctx: DeviceCtx, account: Option<Ctx>, window: &mut Window, cx: &mut Context<Self>) -> SchedulesSection {
        let refresh = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(crate::wakeups::pace(REFRESH, cx)).await;
                let read = this.update(cx, |this, cx| {
                    if this.draft.is_none() && this.busy.is_none() {
                        this.load(cx);
                    }
                });
                if read.is_err() {
                    break;
                }
            }
        });
        // The page that holds it loads it (`worktrees.rs`).
        SchedulesSection {
            ctx,
            account,
            list: Load::Loading,
            projects: Vec::new(),
            owned: false,
            note: None,
            plans: false,
            draft: None,
            title: cx.new(|cx| InputState::new(window, cx).placeholder("Title (optional)")),
            prompt: cx.new(|cx| TextareaState::new(window, cx).rows(4)),
            minutes: cx.new(|cx| InputState::new(window, cx)),
            clock: cx.new(|cx| InputState::new(window, cx).placeholder("09:00")),
            max_runs: cx.new(|cx| InputState::new(window, cx)),
            busy: None,
            problem: None,
            secret: None,
            started: None,
            confirm: None,
            _refresh: refresh,
        }
    }

    /// Read the list again, once the update that asked for it ends: the page is built inside the workspace's update,
    /// and the access reads the workspace.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| this.update(cx, |this, cx| this.load_now(cx)).ok())
            .detach();
    }

    /// Read the list: through the runner while this build owns the store, else from the store.
    fn load_now(&mut self, cx: &mut Context<Self>) {
        let access = (self.ctx.code)(cx);
        self.owned = access.schedules.is_some();
        self.note = (!self.owned).then(|| access.note.unwrap_or_else(|| READ_ONLY.into()));
        self.plans = access.plans;
        if !self.owned {
            self.draft = None;
        }
        let (owned, schedules) = (self.owned, access.schedules.clone());
        off_thread(
            cx,
            move || read_store(access.scope, owned),
            move |this, read, _| match read {
                Ok((projects, schedules)) => {
                    this.projects = projects;
                    if let Some(schedules) = schedules {
                        this.list = Load::Ready(
                            schedules
                                .into_iter()
                                .map(|schedule| ScheduleView {
                                    schedule,
                                    webhook_url: None,
                                })
                                .collect(),
                        );
                    }
                }
                // When owned, the runner's answer below fills the list: this read only adds the projects.
                Err(error) if !owned => this.list = Load::Failed(error.into()),
                Err(_) => {}
            },
        );
        if let Some(schedules) = schedules {
            answer(cx, schedules.list(), |this, list, _| {
                this.list = match list {
                    Ok(list) => Load::Ready(list),
                    Err(error) => Load::Failed(error.into()),
                };
            });
        }
    }

    fn open_form(&mut self, draft: Draft, window: &mut Window, cx: &mut Context<Self>) {
        let set = |input: &Entity<InputState>, text: &str, window: &mut Window, cx: &mut Context<Self>| {
            let text = text.to_string();
            input.update(cx, |state, cx| state.set_value(text, window, cx));
        };
        set(&self.title, &draft.title, window, cx);
        set(&self.minutes, &draft.minutes, window, cx);
        set(&self.clock, &draft.clock, window, cx);
        set(&self.max_runs, &draft.max_runs, window, cx);
        let prompt = draft.prompt.clone();
        self.prompt.update(cx, |state, cx| state.set_value(prompt, window, cx));
        // The form opens ready for typing in its first field.
        self.title.update(cx, |state, cx| state.focus(window, cx));
        self.draft = Some(draft);
        self.problem = None;
        cx.notify();
    }

    /// The form with what is typed in its fields.
    fn typed(&self, cx: &App) -> Option<Draft> {
        let mut draft = self.draft.clone()?;
        draft.title = self.title.read(cx).value().to_string();
        draft.prompt = self.prompt.read(cx).value().to_string();
        draft.minutes = self.minutes.read(cx).value().to_string();
        draft.clock = self.clock.read(cx).value().to_string();
        draft.max_runs = self.max_runs.read(cx).value().to_string();
        Some(draft)
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.typed(cx) else { return };
        let Some(schedules) = self.schedules_now(cx) else {
            return;
        };
        let input = match draft.input(self.account.is_some()) {
            Ok(input) => input,
            Err(problem) => {
                self.problem = Some((None, problem));
                cx.notify();
                return;
            }
        };
        self.busy = Some(None);
        self.problem = None;
        answer(cx, schedules.save(draft.id, input), |this, saved, cx| {
            this.busy = None;
            match saved {
                Ok(_) => {
                    this.draft = None;
                    this.load(cx);
                }
                Err(problem) => this.problem = Some((None, problem)),
            }
        });
        cx.notify();
    }

    /// Run a command on schedule `id`, then read the list again.
    /// A webhook's new token, shown once; turning a webhook off clears only that schedule's token from the screen.
    fn show_secret(&mut self, id: String, secret: Option<WebhookSecret>) {
        match secret {
            Some(secret) => self.secret = Some((id, secret)),
            None if self.secret.as_ref().is_some_and(|(shown, _)| *shown == id) => self.secret = None,
            None => {}
        }
    }

    fn act<T: Send + 'static>(
        &mut self,
        id: String,
        command: impl FnOnce(Schedules) -> Answer<T>,
        done: impl FnOnce(&mut Self, T) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(schedules) = self.schedules_now(cx) else {
            return;
        };
        self.busy = Some(Some(id.clone()));
        self.problem = None;
        self.started = None;
        answer(cx, command(schedules), move |this, result, cx| {
            this.busy = None;
            match result {
                Ok(value) => done(this, value),
                Err(problem) => this.problem = Some((Some(id), problem)),
            }
            this.load(cx);
        });
        cx.notify();
    }

    /// The runner's commands, asked now: this build may have lost or taken the store since the page opened. None
    /// reads the list again, with the read-only note.
    fn schedules_now(&mut self, cx: &mut Context<Self>) -> Option<Schedules> {
        let schedules = (self.ctx.code)(cx).schedules;
        if schedules.is_none() {
            self.load(cx);
            cx.notify();
        }
        schedules
    }
}

/// Await a runner's answer and apply it to the view, then redraw. A view that closed first ignores it.
fn answer<V: 'static, T: Send + 'static>(
    cx: &mut Context<V>,
    answer: Answer<T>,
    apply: impl FnOnce(&mut V, Result<T, String>, &mut Context<V>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        let result = answer
            .await
            .unwrap_or_else(|_| Err("The Code sessions stopped before they answered.".into()));
        this.update(cx, |view, cx| {
            apply(view, result, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

/// A text field in a soft box.
fn field(input: impl IntoElement, width: Option<f32>, palette: Palette) -> Div {
    h_flex()
        .min_h(px(32.))
        .px(px(10.))
        .py(px(4.))
        .rounded(px(8.))
        .bg(palette.hover)
        .text_size(px(13.))
        .text_color(palette.foreground)
        .map(|field| match width {
            Some(width) => field.w(px(width)),
            None => field.flex_1().min_w(px(0.)),
        })
        .child(div().flex_1().min_w(px(0.)).child(input))
}

fn id(parts: &[&str]) -> ElementId {
    SharedString::from(format!("schedule-{}", parts.join("-"))).into()
}

impl SchedulesSection {
    fn render_row(&self, view: &ScheduleView, cx: &mut Context<Self>) -> Div {
        let palette = palette(cx);
        let schedule = &view.schedule;
        let project = self
            .projects
            .iter()
            .find(|(id, _)| *id == schedule.project_id)
            .map_or("A closed project", |(_, name)| name.as_str());
        let next = match (schedule.enabled, schedule.next_run_at) {
            (false, _) => "Off".to_string(),
            (true, Some(next)) => format!("Next: {}", at(next)),
            (true, None) => "Runs when started".to_string(),
        };
        let by = if schedule.created_by.is_some() {
            " · Added by an agent"
        } else {
            ""
        };
        let detail = format!(
            "{project} · {} · {} · {next}{by}",
            describe_when(&schedule.when),
            agent_label(schedule.agent)
        );
        let sid = schedule.id.as_str();
        let busy = self.busy.is_some();
        let button = |key: &str, text: &str| -> Stateful<Div> {
            if busy {
                ui::disabled_button(id(&[key, sid]), text.to_string(), palette)
            } else {
                ui::button(id(&[key, sid]), text.to_string(), palette)
            }
        };
        let mut actions = h_flex().flex_wrap().gap(px(6.));
        if self.owned {
            let (s, on) = (schedule.clone(), schedule.webhook);
            let toggle = s.clone();
            actions = actions
                .child(
                    button("toggle", if schedule.enabled { "Turn off" } else { "Turn on" }).when(!busy, |b| {
                        b.on_click(cx.listener(move |this, _, _, cx| {
                            let id = toggle.id.clone();
                            this.act(id.clone(), move |s| s.update(id, toggled), |_, _| {}, cx);
                        }))
                    }),
                )
                .child(button("run", "Run now").when(!busy, |b| {
                    let id = s.id.clone();
                    b.on_click(cx.listener(move |this, _, _, cx| {
                        let run = id.clone();
                        this.act(
                            id.clone(),
                            move |s| s.run_now(run),
                            |this, _| this.started = Some("Started a session. It is in Code.".into()),
                            cx,
                        );
                    }))
                }))
                .child(button("edit", "Edit").when(!busy, |b| {
                    let edit = s.clone();
                    b.on_click(cx.listener(move |this, _, window, cx| {
                        this.open_form(Draft::edit(&edit), window, cx);
                    }))
                }))
                .child(
                    button("webhook", if on { "Turn webhook off" } else { "Add a webhook" }).when(!busy, |b| {
                        let id = s.id.clone();
                        b.on_click(cx.listener(move |this, _, _, cx| {
                            let target = id.clone();
                            let key = id.clone();
                            this.act(
                                id.clone(),
                                move |s| s.webhook(target, !on),
                                move |this, secret| this.show_secret(key, secret),
                                cx,
                            );
                        }))
                    }),
                )
                .when(on, |actions| {
                    actions.child(button("token", "New token").when(!busy, |b| {
                        let id = s.id.clone();
                        b.on_click(cx.listener(move |this, _, _, cx| {
                            let target = id.clone();
                            let key = id.clone();
                            this.act(
                                id.clone(),
                                move |s| s.webhook(target, true),
                                move |this, secret| this.show_secret(key, secret),
                                cx,
                            );
                        }))
                    }))
                });
            let confirming = self.confirm.as_deref() == Some(sid);
            let delete = if busy {
                ui::disabled_button(id(&["delete", sid]), "Delete", palette)
            } else {
                let id_ = s.id.clone();
                ui::danger_button(
                    id(&["delete", sid]),
                    if confirming { "Delete it" } else { "Delete" },
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.confirm.as_deref() != Some(id_.as_str()) {
                        this.confirm = Some(id_.clone());
                        cx.notify();
                        return;
                    }
                    this.confirm = None;
                    let target = id_.clone();
                    this.act(id_.clone(), move |s| s.delete(target), |_, _| {}, cx);
                }))
            };
            actions = actions.child(delete);
        }
        let secret = self
            .secret
            .as_ref()
            .filter(|(key, _)| key == sid)
            .map(|(_, secret)| secret);
        let problem = self
            .problem
            .as_ref()
            .filter(|(key, _)| key.as_deref() == Some(sid))
            .map(|(_, problem)| problem.clone());
        v_flex()
            .gap(px(6.))
            .py(px(12.))
            .child(ui::two_lines(schedule.title.clone(), detail, palette))
            .when(self.owned, |row| row.child(actions))
            .when(schedule.webhook && secret.is_none(), |row| {
                row.child(
                    ui::note(
                        match &view.webhook_url {
                            Some(url) => format!("POST {url}"),
                            None if self.owned => "The webhook starts listening when Nexus is ready.".into(),
                            None => "Its webhook is on.".into(),
                        },
                        palette,
                    )
                    .text_size(px(12.))
                    .font_family(ui::MONO),
                )
            })
            .when_some(secret, |row, secret| {
                row.child(ui::note(
                    "Copy the token now: Nexus does not show it again. The address answers on this computer only.",
                    palette,
                ))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_family(ui::MONO)
                        .text_color(palette.foreground)
                        .child(format!(
                            "curl -X POST {} -H \"Authorization: Bearer {}\" --data '{{\"any\": \"context\"}}'",
                            secret.url, secret.token
                        )),
                )
            })
            .children(
                schedule
                    .runs
                    .iter()
                    .take(3)
                    .map(|run| ui::note(format!("{}: {}", at(run.at), describe_run(run)), palette).text_size(px(12.))),
            )
            .when_some(problem, |row, problem| row.child(ui::error_line(problem, palette)))
    }

    /// A click that changes the form (`change`, given `at`), keeping what is typed in its fields.
    fn edit(
        cx: &mut Context<Self>,
        change: fn(&mut Draft, usize),
        at: usize,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _: &ClickEvent, _, cx| {
            if let Some(mut draft) = this.typed(cx) {
                change(&mut draft, at);
                this.draft = Some(draft);
                cx.notify();
            }
        })
    }

    /// A switch with its line, in the form.
    fn switch(
        key: &'static str,
        text: &'static str,
        on: bool,
        change: fn(&mut Draft, usize),
        cx: &mut Context<Self>,
    ) -> Div {
        let palette = palette(cx);
        h_flex()
            .gap(px(8.))
            .items_center()
            .child(ui::switch(id(&["switch", key]), on, false, palette).on_click(Self::edit(cx, change, 0)))
            .child(ui::note(text, palette))
    }

    fn render_form(&self, draft: &Draft, cx: &mut Context<Self>) -> Div {
        let palette = palette(cx);
        let busy = self.busy.is_some();
        let row = || h_flex().flex_wrap().items_center().gap(px(6.));
        let projects = row()
            .child(ui::label("Project", palette).w(px(64.)))
            .children(self.projects.iter().map(|(pid, name)| {
                let pid = pid.clone();
                ui::chip(id(&["project", &pid]), name.clone(), draft.project_id == pid, palette).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if let Some(mut draft) = this.typed(cx) {
                            draft.project_id = pid.clone();
                            this.draft = Some(draft);
                            cx.notify();
                        }
                    },
                ))
            }));
        let kinds = [
            (Kind::Interval, "Every"),
            (Kind::Weekly, "On days at a time"),
            (Kind::Manual, "Only from its webhook or Run now"),
        ];
        let when = row()
            .child(ui::label("When", palette).w(px(64.)))
            .children(kinds.iter().enumerate().map(|(at, (kind, text))| {
                ui::chip(id(&["kind", text]), *text, draft.kind == *kind, palette).on_click(Self::edit(
                    cx,
                    |draft, at| {
                        draft.kind = [Kind::Interval, Kind::Weekly, Kind::Manual][at];
                    },
                    at,
                ))
            }));
        let times = match draft.kind {
            Kind::Interval => row()
                .child(div().w(px(64.)))
                .child(field(Input::new(&self.minutes).appearance(false), Some(80.), palette))
                .child(ui::note("minutes, 10 to 10080", palette)),
            Kind::Weekly => row()
                .child(div().w(px(64.)))
                .children((0..7u8).map(|day| {
                    ui::chip(
                        id(&["day", DAY_NAMES[usize::from(day)]]),
                        DAY_NAMES[usize::from(day)],
                        draft.days.contains(&day),
                        palette,
                    )
                    .on_click(Self::edit(
                        cx,
                        |draft, day| draft.toggle_day(day as u8),
                        usize::from(day),
                    ))
                }))
                .child(field(Input::new(&self.clock).appearance(false), Some(80.), palette)),
            Kind::Manual => row(),
        };
        let agents = row()
            .child(ui::label("Agent", palette).w(px(64.)))
            .children(AGENTS.iter().enumerate().map(|(at, agent)| {
                ui::chip(
                    id(&["agent", agent.as_str()]),
                    agent_label(*agent),
                    draft.agent == *agent,
                    palette,
                )
                .on_click(Self::edit(cx, |draft, at| draft.set_agent(AGENTS[at]), at))
            }));
        let modes = row().child(ui::label("Mode", palette).w(px(64.))).children(
            crate::workspace::MODES.iter().enumerate().map(|(at, (mode, text, _))| {
                ui::chip(id(&["mode", mode.as_str()]), *text, draft.mode == *mode, palette).on_click(Self::edit(
                    cx,
                    |draft, at| draft.mode = crate::workspace::MODES[at].0,
                    at,
                ))
            }),
        );
        let vias = [(Via::Nexus, "Through Nexus"), (Via::Subscription, "My subscription")];
        let via = (self.account.is_none() || self.plans || draft.via == Via::Subscription).then(|| {
            row()
                .child(ui::label("Billing", palette).w(px(64.)))
                .children(vias.into_iter().enumerate().map(|(at, (via, text))| {
                    if self.account.is_none() && via == Via::Nexus {
                        return h_flex()
                            .gap(px(6.))
                            .child(ui::disabled_button(id(&["via", via.as_str()]), text, palette))
                            .child(ui::requires_sign_in(palette))
                            .into_any_element();
                    }
                    ui::chip(id(&["via", via.as_str()]), text, draft.via == via, palette)
                        .on_click(Self::edit(
                            cx,
                            |draft, at| draft.via = [Via::Nexus, Via::Subscription][at],
                            at,
                        ))
                        .into_any_element()
                }))
        });
        let problem = self
            .problem
            .as_ref()
            .filter(|(key, _)| key.is_none())
            .map(|(_, problem)| problem.clone());
        v_flex()
            .gap(px(8.))
            .p(px(12.))
            .my(px(8.))
            .rounded(px(10.))
            .bg(palette.raised)
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(palette.strong)
                    .child(if draft.id.is_some() {
                        "Edit schedule"
                    } else {
                        "New schedule"
                    }),
            )
            .child(projects)
            .child(field(Input::new(&self.title).appearance(false), None, palette))
            .child(
                div()
                    .p(px(8.))
                    .rounded(px(8.))
                    .bg(palette.hover)
                    .text_size(px(13.))
                    .child(Textarea::new(&self.prompt).appearance(false)),
            )
            .child(
                ui::note(
                    "The prompt each run starts with. The session can't see anything else, so say everything it needs.",
                    palette,
                )
                .text_size(px(12.)),
            )
            .child(when)
            .child(times)
            .child(agents)
            .child(modes)
            .children(via)
            .child(
                row()
                    .child(ui::label("Limit", palette).w(px(64.)))
                    .child(field(Input::new(&self.max_runs).appearance(false), Some(64.), palette))
                    .child(ui::note("runs a day at most", palette)),
            )
            .child(Self::switch(
                "worktree",
                "Each run in a new worktree",
                draft.worktree,
                |draft, _| draft.worktree = !draft.worktree,
                cx,
            ))
            .child(Self::switch(
                "catch-up",
                "Run a missed run when Nexus opens",
                draft.catch_up,
                |draft, _| draft.catch_up = !draft.catch_up,
                cx,
            ))
            .when_some(problem, |form, problem| form.child(ui::error_line(problem, palette)))
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(6.))
                    // Not while saving: a failed save shows its problem in this form.
                    .child(if busy {
                        ui::disabled_button("schedule-cancel", "Cancel", palette)
                    } else {
                        ui::button("schedule-cancel", "Cancel", palette).on_click(cx.listener(|this, _, _, cx| {
                            this.draft = None;
                            this.problem = None;
                            cx.notify();
                        }))
                    })
                    .child(if busy {
                        ui::disabled_button("schedule-save", "Saving…", palette)
                    } else {
                        ui::primary_button("schedule-save", "Save", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                    }),
            )
    }
}

impl Render for SchedulesSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let mut section = v_flex()
            .w_full()
            .child(ui::section_head("Scheduled sessions", palette))
            .child(ui::note(
                "A prompt that starts a new session on a project at an interval, at a time on chosen days, or when \
                 its webhook is called. Schedules run while Nexus is open. A run Nexus missed runs once when it \
                 opens, unless the schedule skips missed runs. Each schedule runs at most its daily limit.",
                palette,
            ))
            .when_some(self.note.clone(), |section, note| {
                section.child(owned_note(note, palette).mt(px(10.)))
            })
            .when_some(self.started.clone(), |section, started| {
                section.child(ui::note(started, palette))
            });
        let draft = self.draft.clone();
        match &self.list {
            Load::Loading => section = section.child(ui::skeleton(2, palette)),
            Load::Failed(error) => section = section.child(ui::failed(error.clone(), palette)),
            Load::Ready(list) => {
                if list.is_empty() && draft.is_none() {
                    section = section.child(ui::empty(
                        "No schedules yet",
                        if !self.owned {
                            "Schedules added in the Nexus app show here."
                        } else if self.projects.is_empty() {
                            "Open a project in Code first."
                        } else {
                            "Add one to run a prompt by itself."
                        },
                        palette,
                    ));
                }
                let list = list.clone();
                for view in &list {
                    section = match &draft {
                        Some(draft) if draft.id.as_deref() == Some(view.schedule.id.as_str()) => {
                            section.child(self.render_form(draft, cx))
                        }
                        _ => section.child(self.render_row(view, cx)),
                    };
                }
            }
        }
        match &draft {
            Some(draft) if draft.id.is_none() => section.child(self.render_form(draft, cx)),
            _ if self.owned && !self.projects.is_empty() => section.child(h_flex().mt(px(10.)).child(
                ui::button("schedule-add", "Add a schedule", palette).on_click(cx.listener(|this, _, window, cx| {
                    let project = this.projects.first().map(|(id, _)| id.clone());
                    this.open_form(Draft::blank(project.as_deref(), this.account.is_some()), window, cx);
                })),
            )),
            _ => section,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn saved(when: When) -> Schedule {
        let input = ScheduleInput {
            project_id: "p-1".into(),
            title: Some("Triage".into()),
            prompt: "Triage the new issues.".into(),
            agent: SessionAgent::Codex,
            model: Some("gpt-6".into()),
            permission_mode: PermissionMode::Plan,
            effort: Some(Effort::High),
            via: Via::Subscription,
            worktree: false,
            when,
            enabled: true,
            max_runs_per_day: Some(4),
            catch_up: false,
        };
        Schedule::build(input, "s_1".into(), None, None, 1_000_000, &chrono::Utc).expect("a valid schedule")
    }

    #[test]
    fn local_schedules_default_to_subscription_and_reject_gateway_billing() {
        let mut draft = Draft::blank(Some("p-1"), false);
        draft.prompt = "Review the latest changes".into();
        assert_eq!(draft.input(false).expect("local schedule").via, Via::Subscription);
        draft.via = Via::Nexus;
        assert_eq!(draft.input(false).unwrap_err(), "Requires sign-in");
        assert_eq!(draft.input(true).expect("account schedule").via, Via::Nexus);
    }

    #[test]
    fn editing_without_changes_saves_the_same_schedule() {
        let schedule = saved(When::Weekly {
            days: vec![1, 3],
            minute: 9 * 60 + 30,
        });
        let draft = Draft::edit(&schedule);
        assert_eq!(draft.id.as_deref(), Some("s_1"));
        assert_eq!(draft.clock, "09:30");
        assert_eq!(draft.input(true), Ok(input_of(&schedule)));
    }

    #[test]
    fn an_edit_changes_only_what_was_changed() {
        let schedule = saved(When::Interval { minutes: 120 });
        let mut draft = Draft::edit(&schedule);
        assert_eq!(draft.minutes, "120");
        draft.kind = Kind::Weekly;
        draft.days = vec![];
        draft.toggle_day(5);
        draft.toggle_day(1);
        draft.clock = "7:05".into();
        draft.set_agent(SessionAgent::Claude);
        let input = draft.input(true).expect("a valid edit");
        assert_eq!(
            input.when,
            When::Weekly {
                days: vec![1, 5],
                minute: 7 * 60 + 5
            }
        );
        assert_eq!(input.agent, SessionAgent::Claude);
        assert_eq!(input.model, None, "the other agent's model goes back to the default");
        assert_eq!(input.effort, Some(Effort::High));
        assert_eq!(input.via, Via::Subscription);
        assert!(!input.worktree && !input.catch_up);
        // The crate takes the edit, and keeps the schedule's id.
        let again = Schedule::build(
            input,
            schedule.id.clone(),
            Some(&schedule),
            None,
            2_000_000,
            &chrono::Utc,
        );
        assert_eq!(again.map(|edited| edited.id), Ok("s_1".to_string()));
    }

    #[test]
    fn the_form_says_what_does_not_read() {
        let mut draft = Draft::blank(Some("p-1"), true);
        assert_eq!(draft.input(true).unwrap_err(), "Give the schedule a prompt.");
        draft.prompt = "Check the build.".into();
        assert!(draft.input(true).is_ok());
        draft.minutes = "an hour".into();
        assert!(draft.input(true).unwrap_err().contains("whole number"));
        draft.kind = Kind::Weekly;
        draft.clock = "25:00".into();
        assert!(draft.input(true).unwrap_err().contains("HH:MM"));
        assert_eq!(
            Draft::blank(None, true).input(true).unwrap_err(),
            "Open a project in Code first."
        );
    }

    #[test]
    fn a_new_schedule_takes_the_nexus_apps_defaults() {
        let mut draft = Draft::blank(Some("p-1"), true);
        draft.prompt = "Check the build.".into();
        let input = draft.input(true).expect("valid");
        assert_eq!(input.when, When::Interval { minutes: 60 });
        assert_eq!(input.max_runs_per_day, Some(DEFAULT_RUNS_PER_DAY));
        assert_eq!(input.title, None, "a blank title is the prompt's first line");
        assert!(input.enabled && input.worktree && input.catch_up);
    }

    #[test]
    fn turning_off_changes_only_enabled() {
        let schedule = saved(When::Manual);
        let off = toggled(&schedule);
        assert!(!off.enabled);
        assert_eq!(ScheduleInput { enabled: true, ..off }, input_of(&schedule));
    }

    #[test]
    fn says_times_and_runs_as_the_nexus_app_does() {
        assert_eq!(describe_when(&When::Interval { minutes: 60 }), "Every hour");
        assert_eq!(describe_when(&When::Interval { minutes: 180 }), "Every 3 hours");
        assert_eq!(describe_when(&When::Interval { minutes: 2880 }), "Every 2 days");
        assert_eq!(describe_when(&When::Interval { minutes: 45 }), "Every 45 minutes");
        let weekly = |days: Vec<u8>| describe_when(&When::Weekly { days, minute: 510 });
        assert_eq!(weekly(vec![1, 2, 3, 4, 5]), "Weekdays at 08:30");
        assert_eq!(weekly(vec![6, 0]), "Weekends at 08:30");
        assert_eq!(weekly(vec![4, 1]), "Mon, Thu at 08:30");
        assert_eq!(describe_when(&When::Manual), "Only when started");
        assert_eq!(parse_clock(" 9:30 "), Some(570));
        assert_eq!(parse_clock("24:00"), None);
        assert_eq!(parse_clock("9:5"), None);
        assert_eq!(parse_clock("+9:30"), None);
        assert_eq!(parse_clock("1:+5"), None);
        let run = |trigger, status, note: Option<&str>| Run {
            at: 0,
            trigger,
            status,
            thread_id: None,
            note: note.map(String::from),
        };
        assert_eq!(
            describe_run(&run(Trigger::Now, RunStatus::Started, None)),
            "Started with Run now"
        );
        assert_eq!(
            describe_run(&run(Trigger::Webhook, RunStatus::Failed, Some("no key"))),
            "Failed from its webhook: no key"
        );
        assert_eq!(describe_run(&run(Trigger::Time, RunStatus::Skipped, None)), "Skipped");
    }
}
