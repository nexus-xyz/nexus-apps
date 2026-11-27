//! Nexus runs the plan sign-in itself (`login.rs`). One press on Sign in, Sign in again, Add account or Connect starts
//! the agent's own login, and a card follows it: the browser opened, the person signs in there, Nexus reads the plan.
//! Only one sign-in runs at a time, and it holds [`Plans::busy`] as a removal does.
//!
//! The login ends one of two ways: it exits 0, or a read of just that login every [`POLL`] finds it signed in. Then
//! Nexus reads that login, puts it in its row, and reads every login again. A new account stays hidden until it signs
//! in; when its sign-in fails, is cancelled or closed, or the page closes, its folder goes.
//!
//! "Having trouble?" offers a terminal. Opening it stops the in-app sign-in. Nexus then checks the login
//! every [`POLL`] and when the window comes back to the front.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::future::LocalBoxFuture;
use gpui_kit::*;
use nexus_desktop_core::settings::Settings;
use nexus_workbench::accounts;
use nexus_workbench::event::SessionAgent;
use nexus_workbench::harness::{PlanDepth, PlanProbe, harness};
use nexus_workbench::plans::PlanUsage;

use super::login::{self, Event, Exit, Finish, Mode, Runner};
use super::{Load, Plans, account_controls, login_key, login_name};

/// How often a waiting sign-in reads its login.
pub(crate) const POLL: Duration = Duration::from_secs(3);

/// Where a sign-in is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Step {
    /// The login starts; the browser opens.
    Starting,
    /// The person signs in in the browser.
    Waiting,
    /// Signed in: Nexus reads the plan.
    Reading,
    /// A new account signed in to a login already here (its email). Its folder is gone.
    Duplicate(String),
    /// Signed in with nothing to pay with (its name). It stays.
    NoPlan(String),
    /// The login ended without signing in, with the last line it said.
    Failed(Option<String>),
    /// Nexus could not start the provider.
    NotStarted(String),
    /// The login waited [`login::DEADLINE`].
    TimedOut,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SignInSource {
    Account,
    NewAccount,
    Checklist,
}

/// The sign-in on the page.
pub(crate) struct SignIn {
    pub agent: SessionAgent,
    /// The extra account; None for the agent's own login.
    pub account: Option<String>,
    /// Sign in again on a known email: Claude pre-fills it.
    pub email: Option<String>,
    /// The account was made for this sign-in: hidden until it signs in, and deleted if it never does.
    pub new: bool,
    /// Show progress beside the setup step that started this login.
    pub from_checklist: bool,
    /// The command for a terminal.
    pub command: String,
    dir: Option<PathBuf>,
    pub step: Step,
    pub mode: Mode,
    /// The vetted page to sign in on.
    pub link: Option<String>,
    /// Codex's one-time code, and when it expires.
    pub code: Option<(String, Instant)>,
    /// The last line the login said: why it failed, or why a pasted code was refused.
    pub last_line: Option<String>,
    /// A code was pasted: the next line the login says is about it.
    pub code_sent: bool,
    /// Nexus's login was stopped for one in a terminal.
    pub terminal: bool,
    /// Claude: "The browser shows a code? Paste it here" is open.
    pub paste_open: bool,
    /// "Having trouble?", the terminal fallback, is open.
    pub terminal_open: bool,
    runner: Option<Runner>,
    /// Which run this is: what an older run says changes nothing.
    run: u64,
    /// A read of the login runs.
    reading: bool,
    /// The login exited 0 while a poll read ran. That read may predate the sign-in, so a read after the exit is owed.
    read_owed: bool,
}

impl SignIn {
    pub(crate) fn key(&self) -> String {
        login_key(self.agent, self.account.as_deref())
    }

    /// Whether it still runs: it holds [`Plans::busy`].
    pub(crate) fn running(&self) -> bool {
        matches!(self.step, Step::Starting | Step::Waiting | Step::Reading)
    }
}

/// What to start.
pub(crate) struct Request {
    pub agent: SessionAgent,
    /// The login's account. The folder (`dir`) is what the agent reads; tests read this.
    #[cfg_attr(not(test), allow(dead_code))]
    pub account: Option<String>,
    pub dir: Option<PathBuf>,
    pub email: Option<String>,
    pub mode: Mode,
}

type Start = Rc<dyn Fn(Request) -> (Runner, UnboundedReceiver<Event>)>;
type Read = Rc<dyn Fn(SessionAgent, Option<String>) -> LocalBoxFuture<'static, Result<PlanUsage, String>>>;
type Add = Rc<dyn Fn(SessionAgent) -> LocalBoxFuture<'static, Result<String, String>>>;
type Forget = Rc<dyn Fn(SessionAgent, &str)>;
type Select = Rc<dyn Fn(SessionAgent, Option<String>) -> LocalBoxFuture<'static, Result<Settings, String>>>;

/// The sign-in's work off the page. Tests give their own, so no agent runs and no file changes.
#[derive(Clone)]
pub(crate) struct Ops {
    /// Start the agent's login.
    pub start: Start,
    /// Read just one login.
    pub read: Read,
    /// Make a new account's folder; its id.
    pub add: Add,
    /// Delete a new account that never signed in.
    pub forget: Forget,
    /// New sessions of the agent start on the account.
    pub select: Select,
}

impl Ops {
    pub(super) fn local() -> Ops {
        Ops {
            start: Rc::new(|request| Runner::spawn(move || options(request))),
            read: Rc::new(|agent, account| Box::pin(super::off_thread(move || read_login(agent, account)))),
            add: Rc::new(|agent| {
                Box::pin(super::off_thread(move || {
                    let root = super::accounts_root().ok_or("This computer has no app data folder.")?;
                    accounts::add(&root, agent, &super::agent_home(agent))
                }))
            }),
            forget: Rc::new(|agent, id| {
                if let Some(root) = super::accounts_root()
                    && let Err(error) = accounts::remove(&root, agent, id)
                {
                    eprintln!("nexus-desktop-gpui: could not delete the unused account {id}: {error}");
                }
            }),
            select: Rc::new(|agent, account| {
                Box::pin(super::off_thread(move || {
                    crate::desktop_settings::update(move |settings| {
                        let root = super::accounts_root().ok_or("This computer has no app data folder.")?;
                        account_controls::select(settings, &root, agent, account)
                    })
                    .map(|(settings, ())| settings)
                }))
            }),
        }
    }

    /// Ops that do nothing, for tests about something else.
    #[cfg(test)]
    pub(crate) fn inert() -> Ops {
        Ops {
            start: Rc::new(|_| {
                let (runner, _) = Runner::fake();
                (runner, futures::channel::mpsc::unbounded().1)
            }),
            read: Rc::new(|_, _| Box::pin(async { Err("no reads in this test".into()) })),
            add: Rc::new(|_| Box::pin(async { Err("no accounts in this test".into()) })),
            forget: Rc::new(|_, _| {}),
            select: Rc::new(|_, _| Box::pin(async { Err("no settings in this test".into()) })),
        }
    }
}

/// The login to run, found on the runner's thread.
fn options(request: Request) -> Result<login::Options, String> {
    let program = super::find(request.agent).ok_or("The agent is not installed on this computer.")?;
    login::check_login_storage(request.agent)?;
    Ok(login::Options {
        agent: request.agent,
        program,
        program_args: vec![],
        env: super::plan_env(request.agent),
        cwd: request.dir.clone().unwrap_or_else(super::home),
        dir: request.dir,
        email: request.email,
        mode: request.mode,
        deadline: login::DEADLINE,
    })
}

/// One login's reading, as a check reads it.
fn read_login(agent: SessionAgent, account: Option<String>) -> Result<PlanUsage, String> {
    let program = super::find(agent).ok_or("The agent is not installed on this computer.")?;
    let env = super::plan_env(agent);
    let root = super::accounts_root();
    let dir = match (&account, &root) {
        (Some(id), Some(root)) => Some(accounts::dir(root, agent, id)?),
        (Some(_), None) => return Err("This computer has no app data folder.".into()),
        (None, _) => None,
    };
    let probe = PlanProbe {
        program,
        env,
        account,
        dir,
        cwd: None,
    };
    let read = harness(agent)
        .read_plan(probe, PlanDepth::Limits)
        .ok_or("This agent does not report subscription usage.")?;
    let mut plan = super::runtime()?.block_on(read);
    super::mark_active(
        std::slice::from_mut(&mut plan),
        &crate::desktop_settings::load(),
        root.as_deref(),
    );
    Ok(plan)
}

/// A new account's row before its first reading: signed out.
fn placeholder(agent: SessionAgent, id: &str) -> PlanUsage {
    PlanUsage::signed_out(agent, Some(id.into()))
}

/// The text under the card once a login signed in.
pub(crate) fn signed_in_note(name: &str, active: bool) -> String {
    if active {
        format!("Connected as {name}. New sessions start on it.")
    } else {
        format!("Connected as {name}. Choose it to start new sessions on it.")
    }
}

impl Plans {
    /// The new account whose row stays hidden while it signs in, by [`login_key`].
    pub(crate) fn hidden(&self) -> Option<String> {
        self.sign_in.as_ref().filter(|sign_in| sign_in.new).map(SignIn::key)
    }

    /// Sign in a login on the page: one press starts its agent's login. Sign in again pre-fills its email.
    pub(crate) fn sign_in(&mut self, plan: &PlanUsage, cx: &mut Context<Self>) {
        let email = plan.email.clone().filter(|_| plan.signed_in);
        self.begin(plan.agent, plan.account.clone(), email, SignInSource::Account, cx);
    }

    /// New session's Connect signs in to the selected profile in the browser.
    pub(crate) fn connect(&mut self, agent: SessionAgent, account: Option<String>, cx: &mut Context<Self>) {
        self.begin(agent, account, None, SignInSource::Account, cx);
    }

    /// A setup action uses the account selected for new sessions, including a signed-out named profile.
    pub(crate) fn connect_from_checklist(
        &mut self,
        agent: SessionAgent,
        account: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.begin(agent, account, None, SignInSource::Checklist, cx);
    }

    /// Add account: a new folder, then its sign-in at once. Its row stays hidden until it signs in.
    pub(crate) fn add(&mut self, agent: SessionAgent, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.close_sign_in(cx);
        self.busy = true;
        self.problem = None;
        self.removed_note = None;
        let adding = (self.backend.sign_in.add)(agent);
        cx.spawn(async move |this, cx| {
            let id = adding.await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match id {
                    Ok(id) => {
                        if let Load::Ready(list) = &mut page.plans {
                            list.push(placeholder(agent, &id));
                        }
                        page.begin(agent, Some(id), None, SignInSource::NewAccount, cx);
                    }
                    Err(error) => page.problem = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn begin(
        &mut self,
        agent: SessionAgent,
        account: Option<String>,
        email: Option<String>,
        source: SignInSource,
        cx: &mut Context<Self>,
    ) {
        // Nexus runs the sign-in only of an agent whose plan accounts it keeps (Claude Code, Codex).
        if self.busy || nexus_workbench::harness::harness(agent).accounts().is_none() {
            return;
        }
        let dir = match &account {
            Some(id) => match super::accounts_root()
                .ok_or_else(|| "This computer has no app data folder.".to_string())
                .and_then(|root| accounts::dir(&root, agent, id))
            {
                Ok(dir) => Some(dir),
                // A test account has no folder here; the runner it starts is a fake.
                Err(_) if cfg!(test) => None,
                Err(error) => {
                    self.problem = Some(error);
                    cx.notify();
                    return;
                }
            },
            None => None,
        };
        // A card left from an earlier sign-in goes, with an account it made.
        self.close_sign_in(cx);
        self.busy = true;
        self.problem = None;
        self.removed_note = None;
        self.copied = None;
        self.confirm_remove = None;
        self.sign_in = Some(SignIn {
            agent,
            account,
            email,
            new: source == SignInSource::NewAccount,
            from_checklist: source == SignInSource::Checklist,
            command: super::login_command(agent, dir.as_deref()),
            dir,
            step: Step::Starting,
            mode: Mode::Browser,
            link: None,
            code: None,
            last_line: None,
            code_sent: false,
            terminal: false,
            paste_open: false,
            terminal_open: false,
            runner: None,
            run: 0,
            reading: false,
            read_owed: false,
        });
        self.launch(Mode::Browser, cx);
    }

    /// The next run's number, for the sign-in on the page.
    fn next_run(&mut self) -> Option<u64> {
        self.sign_in_seq += 1;
        let run = self.sign_in_seq;
        let sign_in = self.sign_in.as_mut()?;
        sign_in.run = run;
        sign_in.reading = false;
        sign_in.read_owed = false;
        Some(run)
    }

    /// Start (or start again) Nexus's login for the sign-in on the page.
    fn launch(&mut self, mode: Mode, cx: &mut Context<Self>) {
        let Some(run) = self.next_run() else { return };
        let start = self.backend.sign_in.start.clone();
        let Some(sign_in) = self.sign_in.as_mut() else { return };
        sign_in.step = Step::Starting;
        sign_in.mode = mode;
        sign_in.link = None;
        sign_in.code = None;
        sign_in.last_line = None;
        sign_in.code_sent = false;
        sign_in.terminal = false;
        let (runner, mut events) = start(Request {
            agent: sign_in.agent,
            account: sign_in.account.clone(),
            dir: sign_in.dir.clone(),
            email: sign_in.email.clone(),
            mode,
        });
        sign_in.runner = Some(runner);
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                if this.update(cx, |page, cx| page.on_login(run, event, cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
        self.poll(run, cx);
        cx.notify();
    }

    /// Read the login every [`POLL`] while run `run` waits.
    fn poll(&mut self, run: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let waiting = this.update(cx, |page, cx| {
                    let Some(sign_in) = page.sign_in.as_ref().filter(|sign_in| sign_in.run == run) else {
                        return false;
                    };
                    if matches!(sign_in.step, Step::Starting | Step::Waiting) {
                        page.read_login(run, false, cx);
                        return true;
                    }
                    false
                });
                if !matches!(waiting, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
    }

    /// The window came back to the front: a sign-in in a terminal may have finished.
    pub(crate) fn window_active(&mut self, cx: &mut Context<Self>) {
        if let Some(sign_in) = self
            .sign_in
            .as_ref()
            .filter(|sign_in| sign_in.terminal && sign_in.running())
        {
            let run = sign_in.run;
            self.read_login(run, false, cx);
        }
    }

    fn on_login(&mut self, run: u64, event: Event, cx: &mut Context<Self>) {
        let Some(sign_in) = self.sign_in.as_mut().filter(|sign_in| sign_in.run == run) else {
            return;
        };
        match event {
            Event::Started(mode) => sign_in.mode = mode,
            Event::Link(link) => {
                sign_in.link = Some(link);
                if sign_in.step == Step::Starting {
                    sign_in.step = Step::Waiting;
                }
            }
            Event::Code { code, expires_at } => {
                sign_in.code = Some((code, expires_at));
                if sign_in.step == Step::Starting {
                    sign_in.step = Step::Waiting;
                }
            }
            Event::Line(line) => sign_in.last_line = Some(line),
            Event::Exited(exit) => {
                sign_in.runner = None;
                if !sign_in.running() || sign_in.step == Step::Reading {
                    return;
                }
                let said = sign_in.last_line.clone();
                match exit {
                    exit if exit.succeeded() => {
                        sign_in.step = Step::Reading;
                        self.read_login(run, true, cx);
                    }
                    Exit::Cancelled => {}
                    Exit::TimedOut => self.fail(Step::TimedOut),
                    Exit::NotStarted(error) => self.fail(Step::NotStarted(error)),
                    Exit::Status(_) => self.fail(Step::Failed(said)),
                }
            }
        }
        cx.notify();
    }

    /// The sign-in ended without signing in: the card says why, and the page is free again.
    fn fail(&mut self, step: Step) {
        if let Some(sign_in) = self.sign_in.as_mut() {
            sign_in.runner = None;
            sign_in.step = step;
        }
        self.busy = false;
    }

    /// Read just the signing-in login. `ended`: its login exited 0. One read runs at a time; an `ended` read asked
    /// for while a poll read runs is owed, and [`Self::on_read`] runs it when the poll read returns.
    fn read_login(&mut self, run: u64, ended: bool, cx: &mut Context<Self>) {
        let read = self.backend.sign_in.read.clone();
        let Some(sign_in) = self.sign_in.as_mut().filter(|sign_in| sign_in.run == run) else {
            return;
        };
        if sign_in.reading {
            sign_in.read_owed |= ended;
            return;
        }
        sign_in.reading = true;
        let reading = read(sign_in.agent, sign_in.account.clone());
        cx.spawn(async move |this, cx| {
            let plan = reading.await;
            this.update(cx, |page, cx| page.on_read(run, ended, plan, cx)).ok();
        })
        .detach();
    }

    fn on_read(&mut self, run: u64, ended: bool, plan: Result<PlanUsage, String>, cx: &mut Context<Self>) {
        let Some(sign_in) = self.sign_in.as_mut().filter(|sign_in| sign_in.run == run) else {
            return;
        };
        sign_in.reading = false;
        let owed = std::mem::take(&mut sign_in.read_owed);
        if !sign_in.running() {
            return;
        }
        if owed && !ended {
            // The login exited 0 while this poll read ran, so its answer may predate the sign-in. Read again.
            self.read_login(run, true, cx);
            return;
        }
        let plan = match plan {
            Ok(plan) => plan,
            Err(error) => {
                if ended {
                    self.fail(Step::Failed(Some(error)));
                    cx.notify();
                }
                return;
            }
        };
        let others: Vec<PlanUsage> = match &self.plans {
            Load::Ready(list) => list
                .iter()
                .filter(|each| !(each.agent == plan.agent && each.account == plan.account))
                .cloned()
                .collect(),
            _ => vec![],
        };
        let new = sign_in.new;
        match login::finish(&plan, &others, new, ended) {
            Finish::NotYet => return,
            Finish::Done => self.signed_in(plan, cx),
            Finish::Duplicate(email) => {
                // Its folder goes; the card stays to say why.
                let forget = self.backend.sign_in.forget.clone();
                if let Some(sign_in) = self.sign_in.as_mut() {
                    sign_in.runner = None;
                    if sign_in.new
                        && let Some(id) = sign_in.account.clone()
                    {
                        forget(sign_in.agent, &id);
                        sign_in.new = false;
                        let key = sign_in.key();
                        if let Load::Ready(list) = &mut self.plans {
                            list.retain(|each| super::fold_key(each) != key);
                        }
                    }
                }
                self.fail(Step::Duplicate(email));
            }
            Finish::NoPlan(name) => {
                if let Some(sign_in) = self.sign_in.as_mut() {
                    sign_in.new = false;
                }
                self.merge(plan);
                self.fail(Step::NoPlan(name));
                self.check(cx);
            }
        }
        cx.notify();
    }

    /// Put a login's new reading in its row, or add its row.
    fn merge(&mut self, plan: PlanUsage) {
        if let Load::Ready(list) = &mut self.plans {
            match list
                .iter_mut()
                .find(|each| each.agent == plan.agent && each.account == plan.account)
            {
                Some(slot) => *slot = plan,
                None => list.push(plan),
            }
        }
    }

    /// Signed in: the card folds, the row shows, and the agent's only account becomes the one sessions start on.
    fn signed_in(&mut self, plan: PlanUsage, cx: &mut Context<Self>) {
        let Some(sign_in) = self.sign_in.take() else { return };
        // Its login may still wait for the browser: it stops here.
        drop(sign_in.runner);
        let (agent, account) = (plan.agent, plan.account.clone());
        let key = login_key(agent, account.as_deref());
        self.removed.remove(&key);
        self.removal_errors.remove(&key);
        let only = match &self.plans {
            Load::Ready(list) => !list
                .iter()
                .any(|each| each.agent == agent && each.signed_in && each.account != account),
            _ => true,
        };
        let name = login_name(&plan);
        let active = plan.active;
        self.merge(plan);
        if only && !active {
            // Busy until the choice is saved, so nothing else changes the settings meanwhile.
            self.removed_note = Some((agent, signed_in_note(&name, true)));
            let selecting = (self.backend.sign_in.select)(agent, account);
            cx.spawn(async move |this, cx| {
                let saved = selecting.await;
                this.update(cx, |page, cx| {
                    page.busy = false;
                    match saved {
                        Ok(settings) => {
                            (page.settings_saved)(&settings, cx);
                            if let Load::Ready(list) = &mut page.plans {
                                for each in list.iter_mut().filter(|each| each.agent == agent) {
                                    each.active = each.account == super::chosen(&settings, agent);
                                }
                            }
                        }
                        Err(error) => {
                            page.removed_note = Some((agent, signed_in_note(&name, false)));
                            page.problem = Some(error);
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        } else {
            self.busy = false;
            self.removed_note = Some((agent, signed_in_note(&name, active)));
        }
        self.check(cx);
        cx.notify();
    }

    /// Cancel, or Close: the login stops, and a new account that never signed in goes.
    pub(crate) fn close_sign_in(&mut self, cx: &mut Context<Self>) {
        let Some(sign_in) = self.sign_in.take() else { return };
        if sign_in.running() {
            self.busy = false;
        }
        self.forget_new(sign_in);
        cx.notify();
    }

    /// Stop the sign-in's login and delete the account it made, when it never signed in.
    fn forget_new(&mut self, sign_in: SignIn) {
        let key = sign_in.key();
        if let Some(runner) = &sign_in.runner {
            runner.cancel();
        }
        drop(sign_in.runner);
        if sign_in.new
            && let Some(id) = &sign_in.account
        {
            (self.backend.sign_in.forget)(sign_in.agent, id);
            if let Load::Ready(list) = &mut self.plans {
                list.retain(|each| super::fold_key(each) != key);
            }
        }
    }

    /// Try again after a failure or a timeout.
    pub(crate) fn try_sign_in_again(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.sign_in.as_ref().is_none_or(SignIn::running) {
            return;
        }
        self.busy = true;
        self.launch(Mode::Browser, cx);
    }

    /// Codex: "Use a code instead", and "Get a new code" once one expired.
    pub(crate) fn use_device_code(&mut self, cx: &mut Context<Self>) {
        let Some(sign_in) = self
            .sign_in
            .as_mut()
            .filter(|sign_in| crate::agents::ui(sign_in.agent).device_code())
        else {
            return;
        };
        match &sign_in.runner {
            Some(runner) => {
                runner.device_code();
                sign_in.code = None;
            }
            None if !self.busy => {
                self.busy = true;
                self.launch(Mode::DeviceCode, cx);
            }
            None => {}
        }
        cx.notify();
    }

    /// Claude: the code the browser showed goes to the login, once. Nexus keeps no copy.
    pub(crate) fn submit_code(&mut self, code: String, cx: &mut Context<Self>) {
        let code = code.trim().to_string();
        let Some(sign_in) = self.sign_in.as_mut() else { return };
        let Some(runner) = &sign_in.runner else { return };
        if code.is_empty() {
            return;
        }
        runner.submit_code(code);
        sign_in.code_sent = true;
        sign_in.last_line = None;
        cx.notify();
    }

    /// Copy command or Open terminal: Nexus's login stops first, and Nexus reads the login while the card is open.
    fn use_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(sign_in) = self.sign_in.as_ref() else { return };
        if sign_in.terminal {
            return;
        }
        let was_running = sign_in.running();
        if !was_running && self.busy {
            return;
        }
        let Some(run) = self.next_run() else { return };
        let Some(sign_in) = self.sign_in.as_mut() else { return };
        sign_in.runner = None;
        sign_in.terminal = true;
        sign_in.step = Step::Waiting;
        sign_in.last_line = None;
        self.busy = true;
        self.poll(run, cx);
    }

    pub(crate) fn copy_sign_in_command(&mut self, cx: &mut Context<Self>) {
        let Some(command) = self.sign_in.as_ref().map(|sign_in| sign_in.command.clone()) else {
            return;
        };
        self.use_terminal(cx);
        self.copy(command, cx);
    }

    pub(crate) fn open_sign_in_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(sign_in) = self.sign_in.as_ref() else { return };
        let (agent, dir) = (sign_in.agent, sign_in.dir.clone());
        self.use_terminal(cx);
        let run = self.sign_in_seq;
        self.problem = None;
        cx.spawn(async move |this, cx| {
            let opened = super::off_thread(move || account_controls::connect(agent, dir.as_deref())).await;
            if let Err(error) = opened {
                this.update(cx, |page, cx| {
                    if let Some(sign_in) = page.sign_in.as_mut().filter(|sign_in| sign_in.run == run) {
                        sign_in.terminal = false;
                        page.fail(Step::NotStarted(error));
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    /// Open or close a part of the card: the code field (`paste`), or the terminal fallback.
    pub(crate) fn toggle_sign_in_part(&mut self, paste: bool, cx: &mut Context<Self>) {
        if let Some(sign_in) = self.sign_in.as_mut() {
            if paste {
                sign_in.paste_open = !sign_in.paste_open;
            } else {
                sign_in.terminal_open = !sign_in.terminal_open;
            }
            cx.notify();
        }
    }

    /// After a duplicate or a login with no plan: a new account, signed in at once.
    pub(crate) fn sign_in_another(&mut self, cx: &mut Context<Self>) {
        let Some(agent) = self.sign_in.as_ref().map(|sign_in| sign_in.agent) else {
            return;
        };
        self.close_sign_in(cx);
        self.add(agent, cx);
    }

    /// After a login with no plan: "Remove it", through the removal flow.
    pub(crate) fn remove_signed_in(&mut self, cx: &mut Context<Self>) {
        let Some(sign_in) = self.sign_in.as_ref().filter(|sign_in| !sign_in.running()) else {
            return;
        };
        let (agent, account) = (sign_in.agent, sign_in.account.clone());
        self.close_sign_in(cx);
        self.retry_remove(agent, account, cx);
    }
}

impl Drop for Plans {
    /// The page closed: a sign-in stops, and an account it made that never signed in goes.
    fn drop(&mut self) {
        if let Some(sign_in) = self.sign_in.take() {
            self.forget_new(sign_in);
        }
    }
}
