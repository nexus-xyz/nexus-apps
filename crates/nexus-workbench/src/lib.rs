//! The Code workbench's core (a private plan doc): sessions of Claude Code or Codex run as child processes of
//! the app, one per live session, driven over Claude Code's stream-json control protocol ([`claude`]) or Codex's app
//! server ([`codex`]), behind one [`driver::Protocol`]. Every line the agent writes becomes provider-neutral events
//! ([`event`]) appended to the session's log ([`store`]) and handed to the [`Host`], which shows them. The host also
//! opens and ends the Nexus agent session around each run of the agent and prices each model call; this crate knows
//! nothing of Gateway or of the app's window toolkit.
//!
//! A session's agent process starts with its first message, resumes the conversation (`--resume`, `thread/resume`)
//! when a message arrives after it ended, and ends when the person stops it, after [`Config::idle_after`] without
//! work, or when the app quits ([`Workbench::shutdown`]). A turn that was under way when the app last stopped is
//! marked interrupted at the next start.

pub mod accounts;
pub mod acp;
pub mod agent_launch;
pub mod agent_models;
pub mod agent_switch;
pub mod agents;
pub mod allowance;
pub mod local_provider;
pub use nexus_local_agents::antigravity_models;
pub mod api;
pub mod app_socket;
pub mod app_store;
pub mod app_tools;
pub mod attachment_upload;
pub mod attachments;
pub mod attention;
pub mod browser_routing;
pub mod changes_source;
pub mod chat_queue;
mod checkpoint_scope;
pub mod cite;
pub mod claude;
pub mod claude_transcripts;
pub mod claude_trust;
pub use nexus_local_agents::claude_models;
pub mod cleanup;
pub mod cli_log;
pub mod clone;
pub mod cloud_agent;
pub mod cloud_setup;
pub mod codex;
pub use nexus_local_agents::codex_models;
mod commands;
pub mod composer_drafts;
pub mod computer;
pub mod connection;
pub mod connection_view;
pub mod context_breakdown;
pub mod created_pulls;
pub mod credential_env;
pub mod crew_chips;
pub mod crew_fold;
pub mod cycle;
pub mod diff_comment;
pub mod diff_source;
pub mod dirs;
pub mod draft;
pub mod draft_write;
pub mod driver;
pub mod event;
pub mod faults;
pub mod file_origin;
pub mod file_refs;
pub mod files;
pub mod find;
pub mod git;
pub mod gitlab;
pub mod harness;
pub mod hooks;
pub mod inline_media;
pub mod interrupt;
pub mod limit_stop;
pub mod lines;
pub mod link_menu;
pub mod linked;
pub mod markdown_newline;
pub mod mcp_bridge;
pub mod mcp_launch;
pub mod mcp_servers;
#[cfg(feature = "memory-mcp")]
pub mod memory;
pub mod mentions;
pub mod moving;
pub mod nexus;
pub mod nexus_runtime;
pub mod open;
pub mod orchestra;
pub mod output_files;
pub mod output_media;
pub mod owner;
pub mod page;
pub mod picker_values;
pub mod plan_book;
pub mod plan_steps;
pub mod plan_tier;
pub mod plans;
pub mod plugins;
mod private_fs;
pub mod process;
pub mod project_defaults;
pub mod project_files;
pub mod prompt_stash;
pub mod provider;
pub mod provision;
pub mod pty;
pub mod pull_link;
pub mod queued_delivery;
pub mod reader;
pub mod restack;
pub mod resume_recovery;
mod rewind;
pub mod rotate;
mod running_work;
pub mod runs_as;
pub mod schedules;
pub mod scripts;
pub mod secret_files;
pub mod send_back;
pub mod session_choice;
pub mod session_name;
pub mod session_notices;
pub mod session_results;
pub mod session_scope;
pub mod session_share;
pub mod session_sync;
pub mod session_tree;
pub mod setup;
pub mod setup_timeline;
#[cfg(target_os = "linux")]
pub mod setup_user;
pub mod shell_pty;
pub mod side_chat;
pub mod snooze;
pub mod source_host;
pub mod store;
pub mod stream_snapshot;
pub mod subagent;
pub mod sync;
pub mod terminal;
mod title;
mod tool_images;
pub mod tool_rows;
pub mod turn_files;
pub mod update_restart;
pub mod work_folds;

use crate::session_scope::SessionScope;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use serde_json::{Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};

use crate::agents::{Take, TaskBook};
use crate::changes_source::{checkpoint_prefix, seq_of};
pub use crate::diff_source::{Commit, DiffSource, DiffTargets};
use crate::driver::{Background, Out, Protocol, Say};
use crate::event::{
    AgentCapabilities, Decision, Delta, Effort, Image, LimitStop, Pending, PermissionMode, Project, Sender,
    SessionAgent, SetupStatus, StoredEvent, TaskOutput, TaskStatus, ThreadStatus, ThreadSummary, TurnStatus, Usage,
    Via, WbEvent, Worktree,
};
pub use crate::linked::handoff::HandOff;
pub use crate::linked::results::{
    BlockReason, Blocked, ChildInfo, Children, LimitsLeft, ResultStatus, Running, SessionResult, Wait, WaitAnswer,
    WaitMode,
};
pub use crate::linked::{Halt, Limits, Link, OrchCode, OrchError, StartChild, Subsessions};
use crate::plans::PlanUsage;
use crate::process::Spawn;
use crate::rotate::Accounts;
use crate::store::{Reader, Store, ThreadRecord};

pub type Result<T> = std::result::Result<T, String>;

/// Context the agent reads around a message, without adding it to the person's logged text.
#[derive(Default)]
pub(crate) struct SendContext<'a> {
    lead: Option<String>,
    mentions: &'a str,
    queued: bool,
    creating: bool,
    /// True only when the person typed this message (`send_message`, `send_message_as`, `send_from` with no sender),
    /// not for automated `queue` wakes such as the pull request watcher. Only it resets the watch notice budget.
    typed_by_person: bool,
    admitted_execution: Option<&'a AdmittedExecution>,
}

/// What a core that pins and archives projects (`wb_project_update`, [`Workbench::update_project`]) says in its
/// features: the app hides those actions on an older host.
pub const PROJECT_PIN_ARCHIVE: &str = "project-pin-archive";

/// How long a request about one task (`Workbench::task_stop` and the others) waits for the agent's answer.
const TASK_ASK_WAIT: Duration = Duration::from_secs(5);
/// The most of a command's output `Workbench::task_output` returns: its latest bytes.
pub const TASK_OUTPUT_MAX: usize = 8 * 1024;

/// A request about one task of the running agent.
#[derive(Clone, Copy)]
enum TaskAsk<'a> {
    /// Stop this task.
    Stop(&'a str),
    /// Move the foreground command of this tool call to the background.
    Background(&'a str),
    /// Read this background command's output.
    Output(&'a str),
}

/// The last `max` bytes of `text`, cut at a character's start.
fn tail(text: &str, max: usize) -> &str {
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// What the workbench asks of the app around it.
pub trait Host: Send + Sync + 'static {
    fn local_providers_opt_in(&self) -> &provider::LocalProvidersOptIn {
        &provider::LocalProvidersOptIn::Disabled
    }

    /// Desktop owners can present the account-free ledger alongside the current account.
    fn shows_account_free_sessions(&self) -> bool {
        false
    }

    fn computer_choices(
        &self,
        _agents: Vec<SessionAgent>,
        _claude_account: Option<PathBuf>,
    ) -> BoxFuture<'static, Result<computer::settings::NativeChoices>> {
        Box::pin(async { Err("Execution settings are not available on this computer.".into()) })
    }
    /// Execution choices for each requested agent's own selected profile.
    /// The older host entry point takes only a Claude profile.
    fn computer_choices_by_account(
        &self,
        agents: Vec<SessionAgent>,
        mut profiles: computer::settings::AccountProfiles,
    ) -> BoxFuture<'static, Result<computer::settings::NativeChoices>> {
        self.computer_choices(agents, profiles.remove(&SessionAgent::Claude))
    }
    /// An enrolled, immutable account's encrypted-session adapter (`session_sync::GatewayBackend`). None, the default,
    /// syncs nothing. Returning an adapter also binds newly created threads to its verified user.
    fn session_sync(&self) -> Option<Arc<dyn session_sync::Backend>> {
        None
    }
    /// A local setup action changed device availability; retry a failed backend without discarding a ready one.
    fn refresh_session_sync(&self) {}

    /// An opaque sign-in identity for session-choice preferences. None always asks and cannot remember choices.
    fn session_choice_identity(&self) -> Option<String> {
        None
    }
    /// A credential fingerprint for recovery, captured at launch. Never return a credential or a mutable account name.
    fn update_recovery_identity(&self) -> Option<String> {
        None
    }
    /// Before an agent process starts: choose the model, open the Nexus agent session, and say what to run (the
    /// agent's binary with `request.args` and the environment that routes it through Nexus).
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch>>;
    /// Current model choices for changing a saved server session.
    fn saved_server_models(&self, _provider: &nexus_providers::InstanceId) -> Result<Vec<String>> {
        Err("This computer cannot look up saved server models.".into())
    }
    /// After it exited: end the Nexus agent session. True when Gateway took the end, or answered that it never will;
    /// false when asking again later could still end it.
    fn ended(&self, nexus_session: String, end: ProcessEnd) -> BoxFuture<'static, bool>;
    /// Revokes a signed-out run's in-memory launcher grant.
    fn end_local_run(&self, _run: &str) {}
    /// One model call's list price in USD, if the model is known.
    fn price(&self, model: &str, usage: &Usage) -> Option<f64>;
    /// One model call's price at the catalog's highest list price: what a linked session's spend cap counts for a call
    /// [`Host::price`] can't price ([`linked::spend`]). None with no catalog.
    fn top_price(&self, _usage: &Usage) -> Option<f64> {
        None
    }
    /// Loads the catalog's list prices now. The hosts load them at each agent launch, so before its first session a
    /// fresh app has none, and a linked session started first would be refused ([`linked`]).
    fn ensure_prices(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    /// The plans a session on the person's plan can run on: each agent's own login and each extra account, with their
    /// limits ([`plans`]), for [`rotate`]. A host may answer for Claude only. None by default: sessions stay where they
    /// are.
    fn plans(&self) -> BoxFuture<'static, Vec<PlanUsage>> {
        Box::pin(async { Vec::new() })
    }
    /// Whether a commit the person makes from the workbench names Nexus as a co-author, as a session's own commits do
    ///. Desktop includes every Nexus organization member, regardless of role or email. False by default.
    fn marks_commits(&self) -> BoxFuture<'static, bool> {
        Box::pin(async { false })
    }
    /// Run a synchronous spawn operation or pipe poll while the owner is still signed out.
    fn while_saved_provider_allowed(&self, _action: &mut dyn FnMut()) -> Result<()> {
        Err(provider::saved_provider_refusal(false))
    }

    fn check_saved_provider_owner(&self) -> Result<()> {
        self.while_saved_provider_allowed(&mut || {})
    }

    /// The Sandbox switch (Settings › Agents › Antigravity), read as agy starts.
    fn antigravity_sandbox(&self) -> bool {
        false
    }
    fn emit(&self, emit: Emit);
}

/// The app's call to its own background service when Settings saves the worktree and review folders
/// (`{"worktrees", "reviews"}`): the service's new sessions use them ([`Workbench::set_worktree_roots`]).
pub const WORKTREE_ROOTS_CALL: &str = "host_worktree_roots";

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchRequest {
    pub thread_id: String,
    pub agent: SessionAgent,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub via: Via,
    /// A saved key or server on the computer that spawns the agent.
    pub provider: Option<nexus_providers::InstanceId>,
    /// Explicit remote billing intent, checked against the actual selected profile at each launch.
    pub required_payer: Option<computer::BillingPayer>,
    /// The extra account's folder (`CLAUDE_CONFIG_DIR`, [`accounts::account_variable`]) for a session on one; None:
    /// the agent's own login.
    pub account_dir: Option<PathBuf>,
    /// The agent's own arguments (the protocol, the session to resume); the host may add to them.
    pub args: Vec<String>,
    /// The organization the session belongs to (`ThreadRecord::scope`): the host refuses a launch whose scope is not
    /// the one it is signed in to, so a session never runs on another organization's token.
    pub scope: SessionScope,
    /// Shape counts of the first message (never its text) for Auto launch routing.
    pub prompt_counts: Option<nexus_client::auto_route::Counts>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    /// Safe launch-scoped attribution from the machine that actually launches this process.
    pub attribution: Option<event::SessionAttribution>,
    pub spawn: Spawn,
    pub nexus_session: Option<String>,
    /// The signed-out run's in-memory launcher ID.
    pub local_run: Option<String>,
    pub local_provider: Option<local_provider::Run>,
    /// The model chosen by the launch owner, if any.
    pub model: Option<String>,
    /// The MCP servers for an agent that takes them in its session ([`mcp_servers::in_session`]); empty for others.
    pub session_mcp: mcp_servers::SessionMcp,
}

/// Owns revocation across every path after the host hands the run to the workbench.
struct LocalRun {
    host: Arc<dyn Host>,
    id: String,
}

impl Drop for LocalRun {
    fn drop(&mut self) {
        self.host.end_local_run(&self.id);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessEnd {
    pub exit_code: i32,
    pub signal: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
/// What the workbench shows. Only the current scope's sessions are emitted: a session of another organization keeps
/// running, but its events reach no window until the app is back in that organization.
pub enum Emit {
    Event(StoredEvent),
    /// Streamed text, never stored (`wb:delta`).
    Delta(Delta),
    /// Boxed: a summary is the largest payload by far.
    Thread(Box<ThreadSummary>),
    Projects(Vec<Project>),
    /// The limits a session's agent reported for its plan (`wb:plan`), so the app shows them during the turn.
    Plan(plan_book::PlanReport),
}

impl Emit {
    /// Its event's name, as contract.ts's `Events` has it.
    pub fn name(&self) -> &'static str {
        match self {
            Emit::Event(_) => "wb:event",
            Emit::Delta(_) => "wb:delta",
            Emit::Thread(_) => "wb:thread",
            Emit::Projects(_) => "wb:projects",
            Emit::Plan(_) => "wb:plan",
        }
    }
}

/// An emit serializes as its payload alone: the JSON a window receives under [`Emit::name`].
impl serde::Serialize for Emit {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Emit::Event(event) => event.serialize(serializer),
            Emit::Delta(delta) => delta.serialize(serializer),
            Emit::Thread(thread) => thread.serialize(serializer),
            Emit::Projects(projects) => projects.serialize(serializer),
            Emit::Plan(report) => report.serialize(serializer),
        }
    }
}

/// Why a review session refuses to push: its branch tracks someone else's pull request.
const REVIEW_NO_PUSH: &str = "A review session doesn't push: the pull request belongs to its author.";

#[derive(Debug, Clone)]
pub struct Config {
    /// The sessions shown: the Gateway and organization they bill to.
    pub scope: String,
    /// An agent process with nothing to do is stopped after this long; its session resumes on the next message.
    pub idle_after: Duration,
    /// After an interrupt, how long the agent has to end its turn before it is stopped.
    pub interrupt_grace: Duration,
    /// Where sessions' worktrees go (`<this>/<repository>/<branch>`); None: sessions work in their project's folder only.
    pub worktrees: Option<PathBuf>,
    /// Where review sessions' worktrees go (`<this>/<repository>/pr-<number>`); None: no review sessions.
    pub reviews: Option<PathBuf>,
    /// What a new worktree's branch starts with when none is named ([`git::BRANCH_PREFIX`] unless the person set one).
    pub branch_prefix: String,
    /// Where extra plan accounts keep their logins (`<this>/<agent>/<id>`, [`accounts`]); None: the agents' own logins only.
    pub accounts: Option<PathBuf>,
    /// Where files attached to messages are kept (`<this>/<session>/<batch>/<n>-<name>`, [`attachments`]), removed with
    /// their session; None: under `~/.nexus/attachments`.
    pub attachments: Option<PathBuf>,
    /// The limits on subsessions ([`linked`]): none by default ([`Limits::default`]).
    pub orchestration: Limits,
    /// Whether a new session gets subsessions when its start does not say ([`Subsessions::on`]). The project's
    /// `orchestration` column is not read.
    pub subsessions: bool,
    pub subsessions_first_start: session_choice::Preference,
    /// "Disable cross-agent subsessions" ([`Subsessions::cross_agent_off`]): `start_session` and `schedule_session`
    /// refuse another agent than the caller's, on every route.
    pub cross_agent_off: bool,
    /// The deadlines of the agents' MCP server ([`orchestra`]).
    pub orchestra_timing: orchestra::Timing,
    /// This host offers command PTYs through the session's authenticated app tools.
    pub terminal: bool,
    /// Enables signed pairing invitations and native desktop-to-desktop control.
    pub remote_pairing_preview: bool,
    /// The loopback port schedules' webhooks listen on ([`schedules::trigger`]), or a free one when it is taken; None:
    /// no webhooks here.
    pub trigger_port: Option<u16>,
    /// Where the instructions Claude Code is given are written ([`driver::instructions_file`]), on the machine that runs
    /// it; None: the system's temporary folder (tests). Both hosts name a private folder of their own.
    pub instructions: Option<PathBuf>,
    pub memory: Option<nexus_agent_memory::Store>,
    /// The time Run now, webhooks and saved schedules count the daily cap by: the computer's clock, or a fixed one in
    /// tests ([`schedules::Clock`]).
    pub schedule_clock: schedules::Clock,
    /// The shell terminal commands run in and the setup script's `PATH` is asked of: the person's login shell, or
    /// `/bin/sh` in tests ([`process::Shell`]).
    pub shell: process::Shell,
    /// Where Nexus keeps its copy of each Claude Code transcript (`<this>/<session>/<id>.jsonl`,
    /// [`claude_transcripts`]); None: no copies.
    pub transcripts: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            scope: String::new(),
            idle_after: Duration::from_secs(30 * 60),
            interrupt_grace: Duration::from_secs(5),
            worktrees: None,
            reviews: None,
            branch_prefix: git::BRANCH_PREFIX.to_string(),
            accounts: None,
            attachments: None,
            orchestration: Limits::default(),
            subsessions: Subsessions::default().on,
            subsessions_first_start: Subsessions::default().first_start,
            cross_agent_off: Subsessions::default().cross_agent_off,
            orchestra_timing: orchestra::Timing::default(),
            terminal: false,
            remote_pairing_preview: computer::remote_pairing_preview(),
            trigger_port: None,
            instructions: None,
            memory: None,
            schedule_clock: schedules::Clock::default(),
            shell: process::Shell::Login,
            transcripts: None,
        }
    }
}

/// A fork's explicitly selected plan. A missing account uses the agent's default login.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkSubscription {
    pub agent: SessionAgent,
    pub account: Option<String>,
}

#[derive(Clone)]
pub(crate) struct CreationGuard {
    pub identity: session_sync::Identity,
    pub expires_at: i64,
    pub generation: i64,
    pub sender: String,
    pub selection: Option<(SessionAgent, computer::Selection)>,
}

/// The execution choice checked at admission, carried with the first message.
pub(crate) struct AdmittedExecution {
    agent: SessionAgent,
    profile: Option<PathBuf>,
    selection: computer::Selection,
}

impl AdmittedExecution {
    fn matches(&self, agent: SessionAgent, profile: Option<&Path>, selection: &computer::Selection) -> bool {
        self.agent == agent && self.profile.as_deref() == profile && &self.selection == selection
    }
}

#[derive(Debug, Clone)]
pub struct NewThread {
    pub project_id: String,
    pub agent: SessionAgent,
    pub model: Option<String>,
    pub permission_mode: PermissionMode,
    pub effort: Option<Effort>,
    pub via: Via,
    pub provider: Option<nexus_providers::InstanceId>,
    pub text: String,
    pub images: Vec<Image>,
    /// Work in a new git worktree rather than the project's folder.
    pub worktree: Option<NewWorktree>,
    /// The session that starts this one (a crew's lead); None: a session of the person's own.
    pub parent: Option<Link>,
    /// Its agent may start linked sessions of its own.
    pub orchestration: bool,
    /// Its unique name (session_name.rs), refused when invalid or taken; None: no name.
    pub name: Option<String>,
}

/// A new session's worktree: its branch (None: named from the first message) and what it starts from (None: the
/// repository's default branch). With `review`, it is that pull request instead, checked out under the reviews folder.
#[derive(Debug, Clone, Default)]
pub struct NewWorktree {
    pub branch: Option<String>,
    pub base: Option<String>,
    pub review: Option<NewReview>,
    /// The prefix of a branch named from the first message; None: [`Config::branch_prefix`].
    pub branch_prefix: Option<String>,
}

/// The pull request a review session checks out: its number, its URL and the branch it targets (`main`).
#[derive(Debug, Clone, PartialEq)]
pub struct NewReview {
    pub number: u64,
    pub url: String,
    pub base: String,
    /// The machine's tool PATH, for the git that completes a shallow project's history.
    pub tool_path: String,
}

#[derive(Debug, Clone, Default)]
pub struct ThreadPatch {
    pub title: Option<String>,
    pub model: Option<Option<String>>,
    pub permission_mode: Option<PermissionMode>,
    /// Some(None): back to the model's default.
    pub effort: Option<Option<Effort>>,
    /// Fast mode on or off (`driver::fast_allowed`).
    pub fast: Option<bool>,
    pub archived: Option<bool>,
    /// Whether its agent may start linked sessions.
    pub orchestration: Option<bool>,
    /// Held at the top of its project in the sidebar. Pinning alone leaves the session's last-active time as it was.
    pub pinned: Option<bool>,
    /// "Continue at reset" on a session its plan's limit stopped ([`Workbench::continue_at_resets`]), or off again. Like
    /// a pin, it leaves the last-active time as it was.
    pub continue_at_reset: Option<bool>,
    /// Some(Some(at)): snoozed until `at` (ms since the epoch); Some(None): awake again (snooze.rs). Not activity, as
    /// a pin.
    pub snoozed_until: Option<Option<i64>>,
    /// The person's "Keep in list": a merge never settles it, and a settled one comes back (snooze.rs). Not activity.
    pub keep_listed: Option<bool>,
    /// The person settles the session by hand (Some(true)), or brings a settled one back (Some(false)), as a merge does
    /// (snooze.rs). Settling by hand ends "Keep in list". Not activity.
    pub settled: Option<bool>,
    /// Some(Some(name)): the session's unique name (session_name.rs), refused when invalid or another session's;
    /// Some(None): no name again. Claude Code reads it at its next start: a running agent keeps the name it started
    /// with. Not activity, as a pin.
    pub name: Option<Option<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    /// The person stopped the agent.
    Stop,
    /// The agent had nothing to do for a while.
    Idle,
    /// The app is quitting.
    Quit,
    /// The session's Claude account reached its limits: its next message resumes it on another ([`rotate`]). Or agy's
    /// launch arguments changed: its next message resumes it in an agy started with them as they are now
    /// (`Workbench::antigravity_args_changed`).
    Move,
    /// The person restarted the agent ([`Workbench::restart`]): as `Stop`, and the log says so once it has ended.
    Restart,
}

/// What the log says after a restart.
pub const RESTARTED: &str = "Agent restarted. Your next message resumes the conversation in a new process, which reads MCP servers, skills and settings again.";

enum Input {
    Line(String),
    Close,
}

#[derive(Clone)]
enum InterruptSource {
    Person,
    Halt,
    Spend(String),
}

impl InterruptSource {
    fn interrupt(&self, live: &Live) {
        let request = live.next_request();
        live.tell_if(Say::Interrupt { request }, |state, _| {
            self.applies_to(state.turns.front())
        });
    }

    fn applies_to(&self, turn: Option<&Turn>) -> bool {
        match self {
            Self::Spend(id) => turn.is_some_and(|turn| !turn.person && turn.id == *id),
            Self::Person | Self::Halt => true,
        }
    }
}

struct Turn {
    id: String,
    started: Instant,
    /// The process stop claimed this turn. A concurrent result is interrupted if the stop wins the state lock.
    /// A result arriving after that still leaves the process stopping.
    stopping: bool,
    /// A queued message the agent has not been given yet: it gets it once the turn before has ended and its checkpoint
    /// was taken, so the checkpoint never holds this turn's first edits.
    held: Option<Say>,
    /// The person's message that asked for this turn: its `User` event's number. None for a turn the agent started.
    seq: Option<i64>,
    /// Its message is the person's, not a linked session's (`WbEvent::User::from`): a notice that it went unread is
    /// for them.
    person: bool,
    /// Reserved for Nexus's notice whose steer the agent refused (`Out::TurnQueued { person: false }`): when it begins,
    /// a halt or a spend trip interrupts it at once (`Workbench::notice_turn_begun`).
    notice: bool,
}

impl LiveState {
    fn has_active_work(&self) -> bool {
        !self.initialized
            || !self.turns.is_empty()
            || !self.pending.is_empty()
            || !self.unsent.is_empty()
            || !self.background.is_empty()
            || self.owed > 0
    }
}

/// A change sent to the running agent, saved to the session once the agent accepts it, or when the agent ends
/// without answering (the next start then runs with it).
enum Change {
    Mode(PermissionMode),
    Model(Option<String>),
    Effort(Option<Effort>),
    Fast(bool),
}

impl Change {
    fn apply(self, record: &mut ThreadRecord) {
        match self {
            Change::Mode(mode) => record.summary.permission_mode = mode,
            Change::Model(model) => record.summary.model = model,
            Change::Effort(effort) => record.summary.effort = effort,
            Change::Fast(on) => record.summary.fast = on,
        }
    }
}

/// What a live run's session status should be: Starting until the agent says hello, Approval while a question is
/// open, Running while a turn is under way, Idle between turns. None once its clean-up began (`finish` decides).
/// Formal model: a private formal model (`derived`).
fn live_status(state: &LiveState) -> Option<ThreadStatus> {
    if state.done {
        None
    } else if !state.initialized {
        Some(ThreadStatus::Starting)
    } else if !state.pending.is_empty() {
        Some(ThreadStatus::Approval)
    } else if !state.turns.is_empty() {
        Some(ThreadStatus::Running)
    } else {
        Some(ThreadStatus::Idle)
    }
}

/// The changes the agent has not answered, with their lines' numbers, in the order they were sent.
fn unanswered_changes(changes: &mut HashMap<String, (u64, Change)>) -> Vec<(u64, Change)> {
    let mut changes: Vec<(u64, Change)> = changes.drain().map(|(_, change)| change).collect();
    changes.sort_by_key(|(n, _)| *n);
    changes
}

/// An open approval: the input the agent asked about, and the sub-agent that asked (None: the session's own agent).
struct Asked {
    /// The tool asked about: a question or a plan is not a permission ([`driver::answers_ask`]).
    tool: String,
    input: Value,
    owner: Option<String>,
}

struct LiveState {
    pid: Option<u32>,
    /// Open approvals, by request id.
    pending: HashMap<String, Asked>,
    /// The turn under way first, then the messages queued behind it.
    turns: VecDeque<Turn>,
    /// Work the agent left running past its turn (background shells, sub-agents), by task id: the agent is not idle
    /// while any runs.
    background: HashSet<String>,
    /// The tasks this process reported (agents.rs): what the log says about them.
    book: TaskBook,
    /// The session's cost before this process.
    base_cost: f64,
    /// Claude Code's own figure for this process's finished turns.
    confirmed: f64,
    /// Priced calls since the last finished turn.
    estimate: f64,
    nexus_session: Option<String>,
    local_run: Option<LocalRun>,
    initialized: bool,
    /// A name the store accepted while the agent was busy: `/rename` goes out once it is next idle, with the name
    /// stored then ([`Workbench::forward_due_name`]).
    name_due: bool,
    stop: Option<StopReason>,
    last_activity: Instant,
    /// When each stream (the main answer: `None`; a sub-agent: its tool call) last reached the screen.
    last_delta: HashMap<Option<String>, Instant>,
    /// The cost and context of model calls not yet in the session's summary, and when it was last written (`usage_due`).
    unwritten: Option<Unwritten>,
    usage_written: Option<Instant>,
    /// The text of each streamed message the screen has, by message id and parent (`delta_suffix`).
    streamed: HashMap<StreamKey, Sent>,
    stderr: String,
    /// The driver ended the run after an error it showed (`Out::Close`): its exit is no crash to report again.
    explained: bool,
    requests: u64,
    /// A stop or a quit interrupted the turn under way (Claude Code ended it before exiting).
    cut_short: bool,
    /// The process has exited and was reaped: its pid may belong to another process now, so it is never signalled.
    reaped: bool,
    /// Changes waiting for the agent's answer, by request id, with their lines' numbers (`said`).
    changes: HashMap<String, (u64, Change)>,
    /// Lines said to the agent so far: each line's number, in the order the agent reads them (`Live::tell_if`).
    said: u64,
    /// The number of the line whose mode the session saved last: a mode from an older line is not saved over it
    /// (a private formal model
    mode_said: u64,
    /// The process runs and reads its input: set with `pid`, unless a stop came first (then it is killed unread).
    /// Until then a stop has no process to signal, and cancels the launch instead.
    spawned: bool,
    /// The last turn's checkpoint, true once written: the next turn waits for it (`release`).
    checkpoint: Option<watch::Receiver<bool>>,
    /// A message waits for the checkpoint (`release`): the agent has no turn yet, so an interrupt sent now would be lost.
    releasing: bool,
    /// The message waiting for the checkpoint (`releasing`) was queued behind the turn that just ended: the person's
    /// Interrupt now was meant for that turn, and spares it (`Workbench::stop_turn`).
    release_queued: bool,
    /// An interrupt came while a message was waiting for the checkpoint: it goes out right after the message.
    interrupt_after_release: Option<InterruptSource>,
    /// Include the cap interrupt in the next model prompt without starting another turn.
    spend_notice_pending: bool,
    /// Messages lined up for the agent, in the order they go out, each with its `User` event's number; the first
    /// stays until it is out (`line_up`, `Workbench::send_lined_up`).
    unsent: VecDeque<(Option<i64>, Say)>,
    /// The person's messages the agent was given and has not said it read (`Out::Taken`, `read_back`), oldest first: each `User`
    /// event's number and its text.
    unread: VecDeque<(i64, String)>,
    /// Follow-ups sent into the active turn without a read receipt yet.
    steering: HashSet<i64>,
    /// The run's clean-up began (`finish`, or a failed launch): it takes no more messages, and leaves
    /// [`Inner::live`] once it has written its last event. A message sent meanwhile waits for it.
    done: bool,
    /// Where the agent's conversation stands in the turn under way (`Out::Cursor`): its `TurnEnd` keeps it.
    cursor: Option<String>,
    /// What the agent accepts, from its version (`Out::Init`): the session's summary shows it while the agent runs.
    capabilities: Option<AgentCapabilities>,
    connection: event::SessionConnection,
    last_connection: Option<event::SessionConnection>,
    account_revision: u64,
    mcp_revision: u64,
    skills_revision: u64,
    /// Requests about one task waiting for the agent's answer (`Out::Answered`), by request id. A sender dropped
    /// unanswered (the run ended) answers "The agent stopped.".
    task_asks: HashMap<String, oneshot::Sender<std::result::Result<Value, String>>>,
    /// Turns Claude Code owes: one for each background sub-agent that ended between turns, which it answers in a turn
    /// of its own. A linked session is not finished while it owes one ([`linked::results`]).
    owed: u32,
    /// When it last came to owe one.
    owed_since: Instant,
    /// The id of a notice's reserved turn (`Turn::notice`) that just began: checked against halts and spend trips once
    /// the agent's lines that began it are handled (`Workbench::notice_turn_begun`).
    notice_due: Option<String>,
    /// This run's grant on the agents' MCP server ([`orchestra`]): made with the run, and cleared first when it stops,
    /// when its agent exits, and when it ends, so a call from an older run is refused.
    grant: Option<orchestra::Grant>,
    /// The person's plan refused this run's last call at its usage limit, which resets then (epoch milliseconds, when
    /// the agent said); None once a call goes through or the turn ends. A turn that fails meanwhile stopped at the
    /// limit.
    refused: Option<Option<i64>>,
    /// The login folder this run started on (`LaunchRequest::account_dir`), so the plan it reports is kept for that
    /// login (`plan_tier`).
    account_dir: Option<PathBuf>,
    /// The run's bypasses of the Nexus browser, and the outside browser tools already noted ([`browser_routing`]).
    browser: browser_routing::RunState,
    /// A resume launch's message, sent again to a fresh conversation if the agent can't reopen this one
    /// ([`resume_recovery`]).
    resend: Option<resume_recovery::Resend>,
    /// Where this run's Claude Code keeps its transcripts, copied by Nexus at each turn's end ([`claude_transcripts`]).
    transcript: Option<claude_transcripts::Place>,
}

/// The messages an echo of `texts` stands for, taken off `unread` (`LiveState::unread`): the oldest whose text it
/// holds, and the ones right after it that it holds too (Codex's joined message). The agent reads in order, so older
/// ones it skipped never get an echo (a slash command Claude Code runs itself): they leave unconfirmed. A message of
/// only images matches an echo with no text.
fn read_back(unread: &mut VecDeque<(i64, String)>, texts: &[String]) -> Vec<i64> {
    let holds = |text: &str| match text.trim() {
        "" => texts.iter().all(|t| t.trim().is_empty()),
        text => texts.iter().any(|t| t.trim() == text),
    };
    let Some(first) = unread.iter().position(|(_, text)| holds(text)) else {
        return vec![];
    };
    let mut end = first + 1;
    while texts.len() > 1 && end < unread.len() && holds(&unread[end].1) {
        end += 1;
    }
    unread.drain(..end).skip(first).map(|(seq, _)| seq).collect()
}

impl LiveState {
    /// Puts `say` (the message numbered `seq`) in line for the agent. True when it is first in line: the caller then
    /// sends the line (`Workbench::send_lined_up`). Otherwise whoever is sending the line sends this one too, after
    /// the ones before it.
    fn line_up(&mut self, seq: Option<i64>, say: Say) -> bool {
        self.unsent.push_back((seq, say));
        self.unsent.len() == 1
    }

    /// A run that has started, is not ending, and has no turn, queued message or open approval: free to take a linked
    /// session's results as a message ([`linked::results`]).
    fn idle(&self) -> bool {
        self.spawned
            && !self.done
            && self.stop.is_none()
            && self.turns.is_empty()
            && self.unsent.is_empty()
            && self.pending.is_empty()
    }

    /// The last turn's checkpoint while it is being written. Not one whose task is gone (it panicked) without
    /// writing it: nothing would ever end that wait.
    fn pending_checkpoint(&self) -> Option<watch::Receiver<bool>> {
        self.checkpoint
            .clone()
            .filter(|written| !*written.borrow() && written.has_changed().is_ok())
    }

    /// Whether `change`, from the line numbered `n`, may be saved now: a mode only over the mode of an older line,
    /// since the agent runs in the mode of the last line it read. A mode that may is noted as saved. Called under
    /// the run's status lock, with the save in the same hold, so the saves land in the order of their lines.
    fn newer(&mut self, n: u64, change: &Change) -> bool {
        match change {
            Change::Mode(_) if n <= self.mode_said => false,
            Change::Mode(_) => {
                self.mode_said = n;
                true
            }
            Change::Model(_) | Change::Effort(_) | Change::Fast(_) => true,
        }
    }

    /// Of the unanswered `changes`, those [`LiveState::newer`] lets through, in order.
    fn newer_changes(&mut self, changes: Vec<(u64, Change)>) -> Vec<Change> {
        changes
            .into_iter()
            .filter(|(n, change)| self.newer(*n, change))
            .map(|(_, change)| change)
            .collect()
    }

    /// What the session shows as pending: of the unanswered `changes`, the last of each kind. Not a mode older than
    /// the saved one (`mode_said`), which will never be saved over it.
    fn pending(&self) -> Option<Pending> {
        let mut changes: Vec<&(u64, Change)> = self.changes.values().collect();
        changes.sort_by_key(|(n, _)| *n);
        let mut pending = Pending::default();
        for (n, change) in changes {
            match change {
                Change::Mode(mode) if *n > self.mode_said => pending.permission_mode = Some(*mode),
                Change::Mode(_) => {}
                Change::Model(model) => pending.model = Some(model.clone()),
                Change::Effort(effort) => pending.effort = Some(*effort),
                Change::Fast(on) => pending.fast = Some(*on),
            }
        }
        (pending != Pending::default()).then_some(pending)
    }
}

struct Live {
    partial: Mutex<stream_snapshot::Partial>,
    /// The shell commands of this run that may open a pull request, until their results come (created_pulls.rs).
    created: Mutex<created_pulls::Commands>,
    identity: Option<String>,
    agent: SessionAgent,
    saved_provider: bool,
    /// Antigravity's launch arguments as this run started ([`harness::antigravity::args`]); empty for the other agents.
    antigravity_args: Vec<String>,
    recovery_identity: String,
    /// The session's scope (`ThreadRecord::scope`), which never changes: read once at the start, so showing each event
    /// and delta needs no store query.
    scope: String,
    tx: mpsc::UnboundedSender<Input>,
    state: Mutex<LiveState>,
    exited: watch::Receiver<bool>,
    /// The agent's protocol for this run: what goes on its stdin, what its stdout means.
    protocol: Mutex<Box<dyn Protocol>>,
    /// Held across each status write of this run (`refresh_status`, a turn end, the final one), so the writes land
    /// in the order of the states they were computed from: a stale "Running" can't land after a new question, a
    /// turn end or the exit. Taken before `state`, never while holding it.
    status: Mutex<()>,
}

impl Live {
    fn mark_reaped(&self) {
        let run = {
            let mut state = self.state();
            state.reaped = true;
            state.grant = None;
            state.local_run.take()
        };
        drop(run);
    }

    fn state(&self) -> MutexGuard<'_, LiveState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn status_lock(&self) -> MutexGuard<'_, ()> {
        self.status.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn protocol(&self) -> MutexGuard<'_, Box<dyn Protocol>> {
        self.protocol.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn send(&self, line: String) {
        let _ = self.tx.send(Input::Line(line));
    }

    /// Says `say` to the agent: the lines go out, and what it means at once comes back.
    fn tell(&self, say: Say) -> Vec<Out> {
        self.tell_if(say, |_, _| true).map(|(_, outs)| outs).unwrap_or_default()
    }

    /// [`Live::tell`], numbering the line: `ready` sees the run's state and the number first and may call it off
    /// (None). Under the protocol lock until the lines are out, so the numbers follow the order the agent reads.
    fn tell_if(&self, say: Say, ready: impl FnOnce(&mut LiveState, u64) -> bool) -> Option<(u64, Vec<Out>)> {
        let mut protocol = self.protocol();
        let n = {
            let mut state = self.state();
            let n = state.said + 1;
            if !ready(&mut state, n) {
                return None;
            }
            state.said = n;
            n
        };
        let outs = protocol
            .say(say)
            .into_iter()
            .filter_map(|out| match out {
                Out::Reply(line) => {
                    self.send(line);
                    None
                }
                out => Some(out),
            })
            .collect();
        Some((n, outs))
    }

    fn next_request(&self) -> String {
        let mut state = self.state();
        state.requests += 1;
        format!("nexus-{}", state.requests)
    }
}

struct Inner {
    #[cfg(all(test, unix))]
    close_launch_stderr: std::sync::atomic::AtomicBool,
    session_sync: tokio::sync::Mutex<session_sync::Coordinator>,
    session_sync_threads: Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    session_sync_stop: watch::Sender<bool>,
    store: Arc<Mutex<Store>>,
    /// Reads that must not wait for the store's lock (the log replayed on open, scope checks on every event).
    reader: Reader,
    host: Arc<dyn Host>,
    config: Mutex<Config>,
    live: Mutex<HashMap<String, Arc<Live>>>,
    /// Set under `live` before an update installs; agent launch checks it under the same lock.
    update_restart: std::sync::atomic::AtomicBool,
    update_activities: std::sync::atomic::AtomicUsize,
    /// Nexus sessions whose agent was running when the app last stopped without ending them, and when each started.
    lost: Mutex<Vec<(String, i64)>>,
    /// The limits and active account of each agent that supports extra subscription accounts.
    accounts: HashMap<SessionAgent, Mutex<Accounts>>,
    /// Sessions a revert is putting back, and messages on their way to an agent: the two exclude each other.
    reverting: Mutex<RevertGate>,
    /// Each session's last turn checkpoint while `checkpoint_after` writes it: what a revert, an archive, a delete or
    /// a new run's first turn waits for. It outlives the run that armed it.
    checkpoints: Mutex<HashMap<String, watch::Receiver<bool>>>,
    /// Retries of one creation share a lock; unrelated sessions still start concurrently.
    creations: Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    settings_gates: Mutex<HashMap<String, std::sync::Weak<Mutex<()>>>>,
    /// Each tree's counters since the person last wrote to its root, by the root's id ([`linked`]). Held across a
    /// linked session's checks and its row's insert, so two starts at once count each other.
    trees: Mutex<HashMap<String, linked::Tree>>,
    subscriptions: Mutex<linked::messages::Subscriptions>,
    /// Linked sessions' results on their way to their parents, and the parents' waits ([`linked::results`]).
    links: Mutex<linked::results::Links>,
    /// The agents' MCP server, bound on the first start of a session with orchestration on ([`orchestra`]).
    orchestra: tokio::sync::OnceCell<orchestra::http::Server>,
    /// Schedules' webhook listener, bound when the first webhook is on ([`schedules::trigger`]).
    triggers: tokio::sync::OnceCell<schedules::trigger::Server>,
    terminals: terminal::Terminals,
    session_choices: Mutex<session_choice::Choices>,
    /// Where hooks come from ([`hooks`]): set only by the app that owns the store.
    hooks: Mutex<Option<Arc<dyn hooks::Source>>>,
    /// Whether agy offers its `auto` mode on this computer's sign-in, as its last run said ([`Out::AutoOffered`]);
    /// None until a run says. agy's arguments depend on it ([`harness::antigravity::args`]).
    agy_auto: Mutex<Option<bool>>,
}

/// Protect session preparation, setup scripts, and messages before they register a live agent.
struct UpdateActivity(Arc<Inner>);

impl UpdateActivity {
    fn begin(workbench: &Workbench) -> Result<Self> {
        let _lives = workbench.inner.live.lock().unwrap_or_else(|p| p.into_inner());
        if workbench.inner.update_restart.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("Nexus is restarting for an update. Send your message after it opens.".into());
        }
        workbench
            .inner
            .update_activities
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Self(workbench.inner.clone()))
    }
}

impl Drop for UpdateActivity {
    fn drop(&mut self) {
        self.0
            .update_activities
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The workbench. Cheap to clone; every clone is the same workbench.
#[derive(Clone)]
pub struct Workbench {
    inner: Arc<Inner>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The id of the session that creation request `request_id` (a UUID) makes.
pub fn creation_thread_id(request_id: &str) -> Result<String> {
    let request = uuid::Uuid::parse_str(request_id).map_err(|_| "Invalid session creation ID.".to_string())?;
    Ok(format!("t_{request}"))
}

fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// A session's title from its first message: the first line, at most 60 characters.
fn title_of(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("New session");
    let mut title: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        title.push('…');
    }
    title
}

/// Reverts and messages, per session ([`Inner::reverting`]). A message is on its way from `send_message`'s first
/// check until `start` has handed it over (its turn then counts as busy), and a revert from its first check until its
/// `Revert` event is logged. Neither starts while the other is under way: a message that started the agent mid-revert
/// would run with the reverted turns still in its conversation, while its files change (a private formal model R2).
#[derive(Default)]
struct RevertGate {
    reverting: HashSet<String>,
    sending: HashMap<String, usize>,
    saved_provider_starts: HashMap<String, bool>,
}

impl RevertGate {
    fn begin_revert(&mut self, thread_id: &str) -> Result<()> {
        self.check_saved_provider_start(thread_id, false)?;
        if self.reverting.contains(thread_id) {
            return Err("This session's files are already being reverted or moved.".into());
        }
        if self.sending.get(thread_id).is_some_and(|n| *n > 0) {
            return Err("A message is on its way to the agent. Wait for its turn to end, then revert.".into());
        }
        self.reverting.insert(thread_id.to_string());
        Ok(())
    }

    fn check_saved_provider_start(&self, thread_id: &str, creating: bool) -> Result<()> {
        if !creating && self.saved_provider_starts.contains_key(thread_id) {
            return Err("This session is still starting. Send the message again when it opens.".into());
        }
        Ok(())
    }

    fn begin_send(&mut self, thread_id: &str, creating: bool) -> Result<()> {
        self.check_saved_provider_start(thread_id, creating)?;
        if self.reverting.contains(thread_id) {
            return Err("The session's files are being reverted or moved. Send the message again in a moment.".into());
        }
        *self.sending.entry(thread_id.to_string()).or_default() += 1;
        Ok(())
    }

    fn end_send(&mut self, thread_id: &str) {
        if let Some(n) = self.sending.get_mut(thread_id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.sending.remove(thread_id);
            }
        }
    }
}

enum GateUse {
    Revert,
    Send,
    SavedProviderStart,
}

/// An operation under way in [`Inner::reverting`] while this lives.
struct Gated {
    workbench: Workbench,
    thread_id: String,
    operation: GateUse,
}

impl Gated {
    fn gate(workbench: &Workbench) -> MutexGuard<'_, RevertGate> {
        workbench
            .inner
            .reverting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Drop for Gated {
    fn drop(&mut self) {
        let mut gate = Self::gate(&self.workbench);
        match self.operation {
            GateUse::Revert => {
                gate.reverting.remove(&self.thread_id);
            }
            GateUse::Send => gate.end_send(&self.thread_id),
            GateUse::SavedProviderStart => {
                gate.saved_provider_starts.remove(&self.thread_id);
            }
        }
    }
}

/// A session being reverted.
struct Reverting;

impl Reverting {
    fn hold(workbench: &Workbench, thread_id: &str) -> Result<Gated> {
        Gated::gate(workbench).begin_revert(thread_id)?;
        Ok(Gated {
            workbench: workbench.clone(),
            thread_id: thread_id.to_string(),
            operation: GateUse::Revert,
        })
    }
}

/// A message on its way to a session's agent.
struct Sending;

impl Sending {
    fn hold(workbench: &Workbench, thread_id: &str, creating: bool) -> Result<Gated> {
        Gated::gate(workbench).begin_send(thread_id, creating)?;
        Ok(Gated {
            workbench: workbench.clone(),
            thread_id: thread_id.to_string(),
            operation: GateUse::Send,
        })
    }
}

/// The reserved first send may be rolled back until a process actually starts.
struct SavedProviderStart;

impl SavedProviderStart {
    fn hold(workbench: &Workbench, thread_id: &str) -> Gated {
        Gated::gate(workbench)
            .saved_provider_starts
            .insert(thread_id.to_string(), false);
        Gated {
            workbench: workbench.clone(),
            thread_id: thread_id.to_string(),
            operation: GateUse::SavedProviderStart,
        }
    }
}

/// How long a revert, an archive or a delete waits for the last turn's checkpoint (it takes milliseconds).
const CHECKPOINT_WAIT: Duration = Duration::from_secs(10);

/// What a revert asks of the agent's own conversation at its next start.
#[derive(Debug, Clone, PartialEq)]
enum Rewind {
    /// Claude Code resumes up to this message; Codex drops this turn and every later one.
    At(String),
    /// The log holds no such point: the agent is told in the next message.
    Tell,
    /// A conversation rewind failed or ended without a cursor. Restart from the retained text before sending again.
    Replay,
}

/// A session's log read for reverts: the turns not reverted (each turn end's seq and cursor, oldest first), and what
/// the latest revert still asks of the agent, until a turn after it reached the agent's conversation.
///
/// A rewind point is used only when it is exact; otherwise the agent is told (a private formal model R3 and R4).
fn conversation(events: &[StoredEvent], agent: SessionAgent) -> (Vec<(i64, Option<String>)>, Option<Rewind>) {
    let revert_cursor = harness::harness(agent).revert_cursor();
    let mut kept: Vec<(i64, Option<String>)> = Vec::new();
    // Turns whose message carried the revert note.
    let mut told: HashSet<i64> = HashSet::new();
    let mut rewind = None;
    let mut message_rewind = false;
    let mut provider_boundary = 0;
    for stored in events {
        match &stored.event {
            WbEvent::TurnEnd { cursor, status, .. } => {
                let failed = cursor.is_none() && *status == TurnStatus::Error;
                if !failed && rewind == Some(Rewind::Tell) {
                    told.insert(stored.seq);
                }
                // A turn that failed before the agent said anything: it may not have read the note, and a rewind point
                // it refused (a flag a newer CLI dropped, a message it can't find) would fail every start the same
                // way. So the note waits for the next turn, and a rewind point gives way to the note.
                if message_rewind && rewind.is_some() && cursor.is_none() {
                    rewind = Some(Rewind::Replay);
                } else if failed {
                    if rewind.is_some() {
                        rewind = Some(Rewind::Tell);
                    }
                } else if cursor.is_some() || rewind == Some(Rewind::Tell) {
                    // The agent ran a turn since the revert: its conversation already goes on from the reverted point
                    // (or it was told, with that turn's message).
                    rewind = None;
                    message_rewind = false;
                } else if rewind.is_some() {
                    // A turn with no cursor that ran (Nexus closed during it): whether its start rewound is unknown,
                    // and rewinding again could forget it. The note loses nothing.
                    rewind = Some(Rewind::Tell);
                }
                kept.push((stored.seq, cursor.clone()));
            }
            WbEvent::Revert { turn, .. } => {
                let at = kept.iter().position(|(seq, _)| seq > turn).unwrap_or(kept.len());
                let dropped = kept.split_off(at);
                // Back to the latest turn: only the files changed, the conversation is as it was.
                if dropped.is_empty() {
                    continue;
                }
                let point = match revert_cursor {
                    // The turn's own last message (Nexus: its message count). A turn with none: an earlier turn's could
                    // drop a kept turn between them whose cursor was lost.
                    harness::RevertCursor::LastKept => kept.last().and_then(|(_, cursor)| cursor.clone()),
                    // The first reverted turn, by id: it goes, with every turn after it. One with no id, or a cut that
                    // drops a note while what the note was about stays in Codex's thread, can't be exact.
                    harness::RevertCursor::FirstDropped => dropped
                        .first()
                        .and_then(|(_, cursor)| cursor.clone())
                        .filter(|_| !dropped.iter().any(|(seq, _)| told.contains(seq))),
                    harness::RevertCursor::NotOffered => None,
                };
                rewind = if *turn < provider_boundary || (message_rewind && rewind == Some(Rewind::Replay)) {
                    Some(Rewind::Replay)
                } else {
                    Some(point.map_or(Rewind::Tell, Rewind::At))
                };
            }
            WbEvent::AgentSwitched { .. } => {
                provider_boundary = stored.seq;
                // Native cursors belong to the previous provider's conversation.
                for (_, cursor) in &mut kept {
                    *cursor = None;
                }
                told.clear();
                rewind = None;
                message_rewind = false;
            }
            WbEvent::Rewind { before, cursor } => {
                message_rewind = cursor.is_some();
                kept.retain(|(seq, _)| seq < before);
                if cursor.is_none() {
                    // These turns belong to the previous native conversation. Their cursors cannot be reused.
                    for (_, cursor) in &mut kept {
                        *cursor = None;
                    }
                    told.clear();
                }
                rewind = cursor.clone().map(Rewind::At);
            }
            _ => {}
        }
    }
    (kept, rewind)
}

/// The stretches of a session's log that reverts undid: events with `after < seq < end` (`end`: the revert's own seq).
fn reverted_spans(events: &[StoredEvent]) -> Vec<(i64, i64)> {
    events
        .iter()
        .filter_map(|stored| match stored.event {
            WbEvent::Revert { turn, .. } => Some((turn, stored.seq)),
            WbEvent::Rewind { before, .. } => Some((before - 1, stored.seq)),
            _ => None,
        })
        .collect()
}

/// Put before the next message when the agent can't forget the reverted turns itself.
const REVERT_NOTE: &str = "(Note from the Nexus app: the person reverted this folder's files to how they were after an \
earlier turn of this conversation. The changes made in the turns after it are gone; check the files before relying on \
them.)";

/// The most of a diff shown at once.
const MAX_DIFF: usize = 2 << 20;

/// A session's folder as git sees it (the session's git panel).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitState {
    pub path: String,
    /// None: a detached HEAD, or not a git repository.
    pub branch: Option<String>,
    /// The local branches to switch to, most recent first; none for a session with its own worktree.
    pub branches: Vec<String>,
    /// What a commit would take. None when git can't say (not a repository, no commit yet).
    pub uncommitted: Option<git::Stat>,
    /// None in a repository. In a folder that isn't in one: the repositories directly inside it, changed ones first.
    pub repos: Option<Vec<Repo>>,
    /// The branch against the one it tracks, as last fetched. None: no upstream (never pushed), or a detached HEAD.
    pub upstream: Option<git::Tracking>,
    /// A session's own worktree against the branch it was made from (`origin/main`). None elsewhere.
    pub base: Option<git::Tracking>,
}

/// A repository directly inside a session's folder, as its git panel lists it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    /// Its folder's name: the `repo` the panel's git commands take.
    pub name: String,
    pub branch: Option<String>,
    pub uncommitted: Option<git::Stat>,
}

/// [`Workbench::draft_context`]'s reading, in `dir`.
fn draft_context_in(dir: &Path, kind: draft::Kind, made_from: Option<String>) -> Result<draft::Context> {
    let branch = git::checked_out(dir);
    let mut untracked = git::untracked_files(dir)?;
    untracked.truncate(draft::UNTRACKED_NAMES);
    let mut context = draft::Context {
        kind,
        branch: branch.clone(),
        base: None,
        subjects: vec![],
        commits: vec![],
        template: None,
        diff: String::new(),
        cut: false,
        untracked,
    };
    match kind {
        draft::Kind::Commit => {
            context.subjects = git::subjects(dir, "HEAD", draft::STYLE_SUBJECTS);
            // A repository with no commit yet has no HEAD to compare with: its files are all untracked.
            if !context.subjects.is_empty() {
                (context.diff, context.cut) = git::patch_since(dir, "HEAD", draft::DIFF_CAP)?;
            }
            if context.diff.is_empty() && context.untracked.is_empty() {
                return Err("Nothing to commit.".into());
            }
        }
        draft::Kind::PullRequest => {
            let base = branch
                .as_deref()
                .and_then(|branch| git::base_ref(dir, branch))
                .or(made_from)
                .ok_or_else(|| "This branch has no base to compare with.".to_string())?;
            let start = git::merge_base(dir, &base, "HEAD")?;
            context.commits = git::subjects(dir, &format!("{start}..HEAD"), draft::BRANCH_COMMITS);
            context.commits.reverse();
            context.subjects = git::subjects(dir, &base, draft::STYLE_SUBJECTS);
            (context.diff, context.cut) = git::patch_since(dir, &start, draft::DIFF_CAP)?;
            context.template = git::pull_request_template(dir, draft::TEMPLATE_CAP);
            if context.diff.is_empty() && context.untracked.is_empty() && context.commits.is_empty() {
                return Err(format!(
                    "Nothing to describe yet: this branch has no changes against {base}."
                ));
            }
            context.base = Some(base);
        }
    }
    Ok(context)
}

fn read_git(dir: &Path, own_worktree: bool) -> GitState {
    let path = dir.to_string_lossy().into_owned();
    if !git::in_repo(dir) {
        return GitState {
            path,
            branch: None,
            branches: vec![],
            uncommitted: None,
            repos: Some(child_repos(dir)),
            upstream: None,
            base: None,
        };
    }
    GitState {
        path,
        branch: git::checked_out(dir),
        branches: if own_worktree {
            vec![]
        } else {
            git::local_branches(dir).unwrap_or_default()
        },
        uncommitted: git::uncommitted(dir).ok(),
        repos: None,
        upstream: git::tracking(dir),
        base: if own_worktree { git::base_tracking(dir) } else { None },
    }
}

/// The repositories directly inside `dir`, read 8 at a time: changed ones first, then by name.
fn child_repos(dir: &Path) -> Vec<Repo> {
    let paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| git::is_repo(path))
        .collect();
    let read = |path: &PathBuf| Repo {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        branch: git::branch(path),
        uncommitted: git::uncommitted(path).ok(),
    };
    // Dealt out in turn, not in slices: big checkouts sit side by side in name order, and one worker would get them all.
    let workers = 8;
    let paths = &paths;
    let mut repos: Vec<Repo> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|first| scope.spawn(move || paths.iter().skip(first).step_by(workers).map(read).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    });
    let clean = |repo: &Repo| repo.uncommitted.is_none_or(|stat| stat.files == 0);
    repos.sort_by(|a, b| (clean(a), &a.name).cmp(&(clean(b), &b.name)));
    repos
}

/// `name` as a repository directly inside `dir`: one folder name (the webview sends it), never a path out of `dir`.
fn child_repo(dir: &Path, name: &str) -> Result<PathBuf> {
    let mut parts = Path::new(name).components();
    match (parts.next(), parts.next()) {
        (Some(std::path::Component::Normal(part)), None) if git::is_repo(&dir.join(part)) => Ok(dir.join(part)),
        _ => Err(format!("{name} is not a repository in this session's folder.")),
    }
}

/// Whether one folder is the other or inside it: an agent working in either sees the files a switch in the other
/// rewrites.
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// What a session changed, as a unified diff.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub diff: String,
    /// Cut at 2 MiB.
    pub truncated: bool,
}

/// Where a review session's worktree for pull request `number` goes: `<root>/<repository>/pr-<number>` on branch
/// `review/pr-<number>`, with `-2`, `-3`… when that folder or branch is already taken (a second review of it).
fn review_place(root: &Path, repo: &Path, number: u64) -> (PathBuf, String) {
    let parent = root.join(repo.file_name().unwrap_or_default());
    (1..)
        .map(|n| {
            let name = if n == 1 {
                format!("pr-{number}")
            } else {
                format!("pr-{number}-{n}")
            };
            (parent.join(&name), format!("review/{name}"))
        })
        .find(|(path, branch)| !path.exists() && !git::has_branch(repo, branch))
        .unwrap_or_else(|| (parent.join(format!("pr-{number}")), format!("review/pr-{number}")))
}

/// A worktree's branch when none is named: `prefix` (`nexus/`) and the first message's first words, then a piece of the
/// session id so two alike sessions don't collide.
fn branch_for(prefix: &str, text: &str, id: &str) -> String {
    let words: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(5)
        .map(str::to_ascii_lowercase)
        .collect();
    let slug: String = words.join("-").chars().take(40).collect();
    let tail = &id[id.len().saturating_sub(4)..];
    if slug.is_empty() {
        format!("{prefix}session-{tail}")
    } else {
        format!("{prefix}{}-{tail}", slug.trim_end_matches('-'))
    }
}

/// What "Continue" sends: after an interruption, and the plan limit stop's Continue now (`limit_stop.rs`).
pub const CONTINUE: &str = "Continue where you left off.";

/// A plan's limit counts as reset this long after the time its agent gave: the plan's clock and this computer's may
/// differ.
const RESET_GRACE_MS: i64 = 30_000;

/// An agent with background work still running is stopped only after this many times [`Config::idle_after`] without a
/// word from it (six hours by default).
const BACKGROUND_IDLE_FACTOR: u32 = 12;
/// The notice for an agent that exited (stopped, idle, quit or moved) with work still running in the background, which
/// ends with it. A crash says so as an error, and quitting mid-turn has its own notice. tasks.ts reads this exact text.
const BACKGROUND_ENDED: &str = "What the agent left running in the background stopped with it.";

/// The images Claude Code takes, and how many go with one message (the webview scales them down to fit first).
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
const MAX_IMAGES: usize = 8;
/// What a message's images may total, decoded: Gateway's limit, as the kit's `CHAT_IMAGE_LIMITS.totalBytes`.
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

fn check_images(images: &[Image]) -> Result<()> {
    if images.len() > MAX_IMAGES {
        return Err(format!("A message takes at most {MAX_IMAGES} images."));
    }
    let bytes: usize = images.iter().map(|image| decoded_len(&image.data)).sum();
    if bytes > MAX_IMAGE_BYTES {
        return Err(format!(
            "A message's images may total at most {} MB.",
            MAX_IMAGE_BYTES / (1024 * 1024)
        ));
    }
    match images
        .iter()
        .find(|image| !IMAGE_TYPES.contains(&image.media_type.as_str()))
    {
        Some(image) => Err(format!(
            "{} is not a PNG, JPEG, GIF or WebP image.",
            image.name.as_deref().unwrap_or("An image")
        )),
        None => Ok(()),
    }
}

/// The bytes `base64` decodes to, counted as the webview counts them (`dataUrlBytes`).
fn decoded_len(base64: &str) -> usize {
    let padding = base64.bytes().rev().take_while(|&byte| byte == b'=').count();
    (base64.len() * 3 / 4).saturating_sub(padding)
}

/// Why a run ended when its session's log could not be written (a full disk, most often).
const UNSAVED: &str = "This session's log could not be saved, so its agent was stopped. Free some disk space, then send \
                       your message again.";

/// A source's log as a fork copies it: up to event `at`, without the reverts and the turns they undid. Where the
/// source's conversation stood means nothing in the fork's, so no turn keeps its cursor.
fn copied_until(all: &[StoredEvent], at: i64) -> Vec<StoredEvent> {
    let reverted = reverted_spans(all);
    all.iter()
        .filter(|stored| stored.seq <= at)
        .filter(|stored| !matches!(stored.event, WbEvent::Revert { .. } | WbEvent::Rewind { .. }))
        .filter(|stored| {
            !reverted
                .iter()
                .any(|(after, end)| *after < stored.seq && stored.seq < *end)
        })
        .cloned()
        .map(|mut stored| {
            if let WbEvent::TurnEnd { cursor, .. } = &mut stored.event {
                *cursor = None;
            }
            stored
        })
        .collect()
}

/// Whether a fork at event `at` goes on from the source's own conversation (Claude Code's `--fork-session`, Codex's
/// `thread/fork`): only at its end. Nothing may have been said after `at` (a running turn has), the agent has nothing
/// reverted still to forget (`rewinds`), and its conversation has started.
fn forks_at_end(all: &[StoredEvent], at: i64, rewinds: bool, native_started: bool) -> bool {
    all.iter()
        .filter(|stored| stored.seq > at)
        .all(|stored| !matches!(stored.event, WbEvent::User { .. } | WbEvent::TurnStart { .. }))
        && !rewinds
        && native_started
}

/// The most of an earlier conversation a fork's agent is told ([`seed_of`]); older messages are left out.
const MAX_SEED: usize = 24 * 1024;

/// What a fork from an earlier point tells its agent ahead of the first message: the person's messages and the
/// agent's answers up to there, newest kept when it is long.
fn seed_of(events: &[StoredEvent]) -> String {
    format!(
        "This conversation is a fork of an earlier one, from a point before it ended. The files are as they were at \
         that point. What was said up to there:\n\n{}\n\nThe new message follows.\n\n",
        said_of(events)
    )
}

/// The person's messages and the agent's answers in `events`, in a `<conversation>` block, newest kept when it is
/// long ([`MAX_SEED`]).
fn said_of(events: &[StoredEvent]) -> String {
    let mut lines: Vec<String> = events
        .iter()
        .filter_map(|stored| match &stored.event {
            // A linked session's message is not the person's.
            WbEvent::User {
                text,
                from: Some(Sender::Session { title, .. }),
                ..
            } => Some(format!("Session \"{title}\": {text}")),
            WbEvent::User { text, .. } => Some(format!("User: {text}")),
            WbEvent::Text { text, .. } => Some(format!("You: {text}")),
            _ => None,
        })
        .collect();
    let mut total = 0;
    let keep = lines
        .iter()
        .rev()
        .take_while(|line| {
            total += line.len() + 1;
            total <= MAX_SEED
        })
        .count();
    let earlier_left_out = lines.len() > keep.max(1);
    if keep == 0 && !lines.is_empty() {
        // The newest line alone is over the limit (a long final answer, a pasted log): its end stays, cut to fit, so
        // the agent still has the point the person forked from.
        let last = lines.pop().unwrap_or_default();
        let mut from = last.len().saturating_sub(MAX_SEED);
        while !last.is_char_boundary(from) {
            from += 1;
        }
        lines = vec![format!("…{}", &last[from..])];
    } else {
        lines.drain(..lines.len() - keep);
    }
    let left_out = if earlier_left_out {
        "(Earlier messages left out.)\n"
    } else {
        ""
    };
    format!("<conversation>\n{left_out}{}\n</conversation>", lines.join("\n"))
}

impl Workbench {
    /// Opens the workbench over `store`. Sessions that were busy when the app last stopped are marked interrupted.
    pub fn new(store: Store, host: Arc<dyn Host>, config: Config) -> Result<Workbench> {
        let reader = store.reader();
        let store = Arc::new(Mutex::new(store));
        let terminals = terminal::Terminals::with_store(store.clone())?;
        terminals.context(&config.scope, host.session_choice_identity());
        let workbench = Workbench {
            inner: Arc::new(Inner {
                #[cfg(all(test, unix))]
                close_launch_stderr: std::sync::atomic::AtomicBool::new(false),
                session_sync: tokio::sync::Mutex::new(session_sync::Coordinator::default()),
                session_sync_threads: Mutex::new(HashMap::new()),
                session_sync_stop: watch::channel(false).0,
                reader,
                store,
                host,
                config: Mutex::new(config),
                live: Mutex::new(HashMap::new()),
                update_restart: std::sync::atomic::AtomicBool::new(false),
                update_activities: std::sync::atomic::AtomicUsize::new(0),
                lost: Mutex::new(Vec::new()),
                accounts: SessionAgent::ALL
                    .into_iter()
                    .filter(|agent| harness::harness(*agent).accounts().is_some())
                    .map(|agent| (agent, Mutex::new(Accounts::new(agent))))
                    .collect(),
                reverting: Mutex::new(RevertGate::default()),
                checkpoints: Mutex::new(HashMap::new()),
                creations: Mutex::new(HashMap::new()),
                settings_gates: Mutex::new(HashMap::new()),
                trees: Mutex::new(HashMap::new()),
                subscriptions: Mutex::default(),
                links: Mutex::new(linked::results::Links::default()),
                orchestra: tokio::sync::OnceCell::new(),
                triggers: tokio::sync::OnceCell::new(),
                terminals,
                session_choices: Mutex::new(session_choice::Choices::default()),
                hooks: Mutex::new(None),
                agy_auto: Mutex::new(None),
            }),
        };
        // A shared-terminal command that starts or ends changes what its session left running: show it again.
        let inner = Arc::downgrade(&workbench.inner);
        workbench.inner.terminals.on_change(Arc::new(move |thread_id: &str| {
            if let Some(inner) = inner.upgrade() {
                Workbench { inner }.emit_summary(thread_id);
            }
        }));
        let inner = Arc::downgrade(&workbench.inner);
        workbench.inner.terminals.on_exit(Arc::new(move |entry| {
            if let Some(inner) = inner.upgrade() {
                Workbench { inner }.deliver_terminal_watch(entry);
            }
        }));
        workbench.recover()?;
        Ok(workbench)
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.inner.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn reader(&self) -> &Reader {
        &self.inner.reader
    }

    fn live(&self, thread_id: &str) -> Option<Arc<Live>> {
        self.inner
            .live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(thread_id)
            .cloned()
    }

    fn all_live(&self) -> Vec<(String, Arc<Live>)> {
        self.inner
            .live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(id, live)| (id.clone(), live.clone()))
            .collect()
    }

    /// Whether session `thread_id` is in the store (in any scope).
    pub fn has_thread(&self, thread_id: &str) -> bool {
        matches!(self.store().thread(thread_id), Ok(Some(_)))
    }

    /// Where attached files are kept ([`Config::attachments`]).
    pub fn attachments_root(&self) -> Option<PathBuf> {
        self.config().attachments
    }

    fn config(&self) -> Config {
        self.inner
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Where new sessions' worktrees go, and new review sessions'; sessions already made keep theirs.
    pub fn set_worktree_roots(&self, worktrees: Option<PathBuf>, reviews: Option<PathBuf>) {
        let mut config = self
            .inner
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        config.worktrees = worktrees;
        config.reviews = reviews;
    }

    /// What new worktrees' branches start with when none is named; sessions already made keep theirs.
    pub fn set_branch_prefix(&self, prefix: &str) -> Result<()> {
        git::check_branch_prefix(prefix)?;
        self.inner
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .branch_prefix = prefix.to_string();
        Ok(())
    }

    pub fn set_scope(&self, scope: &str) {
        self.inner
            .terminals
            .context(scope, self.inner.host.session_choice_identity());
        self.inner
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .scope = scope.to_string();
        if !SessionScope::decode(scope).is_local() {
            for (_, live) in self.all_live().into_iter().filter(|(_, live)| live.saved_provider) {
                self.stop_live(&live, StopReason::Stop, None);
            }
        }
    }

    pub fn persistent(&self) -> bool {
        self.store().persistent()
    }

    /// Ends, in Gateway, the Nexus sessions whose agent was running when the app last died (they would otherwise show
    /// as running forever). Call once the host can reach Gateway. Each leaves the ledger only once Gateway took its
    /// end: one it did not take, or that the app died again before ending, is ended at the next launch.
    pub async fn end_lost_sessions(&self) {
        let lost = std::mem::take(&mut *self.inner.lost.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        for (session, started_at) in lost {
            let end = ProcessEnd {
                exit_code: 1,
                signal: Some("lost".into()),
                duration_ms: u64::try_from(now_ms() - started_at).unwrap_or(0),
            };
            if self.inner.host.ended(session.clone(), end).await
                && let Err(error) = self.store().close_session(&session)
            {
                eprintln!("nexus-workbench: {session}: {error}");
            }
        }
    }

    fn recover(&self) -> Result<()> {
        let lost = self.store().open_sessions()?;
        *self.inner.lost.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = lost;
        // A setup script that was running when Nexus closed never ends: say so, or the log shows it running for good.
        // Bound first: the store's lock must be let go before `append` takes it again.
        let unfinished = self.store().unfinished_setups()?;
        for (thread_id, event) in unfinished {
            if let WbEvent::Setup { id, command, .. } = event {
                self.append(
                    &thread_id,
                    WbEvent::Setup {
                        id,
                        command,
                        status: SetupStatus::Failed,
                        output: "Nexus closed before the setup finished.\n".into(),
                        exit_code: None,
                        duration_ms: None,
                    },
                )?;
            }
        }
        // Linked sessions left with no turn end for their last message: before the busy ones, which they may be.
        self.recover_links()?;
        let busy = self.store().busy_threads()?;
        for mut record in busy {
            let id = record.summary.id.clone();
            // Read first: the store's lock must not be held while appending.
            let open = self.reader().open_approvals(&id)?;
            for request_id in open {
                self.append(
                    &id,
                    WbEvent::ApprovalResolved {
                        request_id,
                        decision: Decision::Expired,
                        message: None,
                    },
                )?;
            }
            if let Some(turn_id) = self.open_turn(&id)? {
                self.append(
                    &id,
                    WbEvent::TurnEnd {
                        turn_id,
                        status: TurnStatus::Interrupted,
                        cost_usd: None,
                        duration_ms: 0,
                        error: None,
                        cursor: None,
                    },
                )?;
            }
            self.append(
                &id,
                WbEvent::Notice {
                    text: "Nexus closed while this session was working.".into(),
                },
            )?;
            record.summary.status = ThreadStatus::Interrupted;
            record.summary.listed_at = Some(now_ms());
            self.store().save_thread(&record)?;
        }
        Ok(())
    }

    /// The last turn that started and did not end, if any.
    fn open_turn(&self, thread_id: &str) -> Result<Option<String>> {
        let mut open = None;
        let events = self.reader().events_after(thread_id, 0)?;
        for stored in events {
            match stored.event {
                WbEvent::TurnStart { turn_id } => open = Some(turn_id),
                WbEvent::TurnEnd { .. } => open = None,
                _ => {}
            }
        }
        Ok(open)
    }

    /// Stores the event, then emits it while still holding the store: two appends on one session from different
    /// tasks (the agent's stream and a queue edit, say) are pushed in the order of their `seq`, so a client that keeps
    /// only seqs above the last it has never drops one.
    fn append(&self, thread_id: &str, event: WbEvent) -> Result<StoredEvent> {
        self.append_bound(thread_id, None, event)
    }
    fn append_from(&self, thread_id: &str, live: &Live, event: WbEvent) -> Result<StoredEvent> {
        self.append_bound(thread_id, Some(live), event)
    }
    fn append_bound(&self, thread_id: &str, producer: Option<&Live>, event: WbEvent) -> Result<StoredEvent> {
        let store = self.store();
        let lives = self.inner.live.lock().unwrap_or_else(|p| p.into_inner());
        let live = lives.get(thread_id);
        if producer.is_some_and(|producer| live.is_none_or(|current| !std::ptr::eq(current.as_ref(), producer))) {
            return Err("The session's original process has retired.".into());
        }
        let stored = store.append(thread_id, now_ms(), &event)?;
        let mut created = Vec::new();
        if let Some(live) = live {
            live.partial.lock().unwrap_or_else(|p| p.into_inner()).event(&event);
            created = live.created.lock().unwrap_or_else(|p| p.into_inner()).event(&event);
        }
        drop(lives);
        if self.shown(thread_id) {
            self.inner.host.emit(Emit::Event(stored.clone()));
        }
        drop(store);
        if let Some(fire) = hooks::fire_for(thread_id, &stored) {
            self.fire_hooks(fire);
        }
        if !created.is_empty() {
            self.link_created(thread_id, created);
        }
        Ok(stored)
    }

    /// Saves the pull requests a finished shell command opened (created_pulls.rs), on another thread: an event may be
    /// stored while its caller holds the session's state, which saving the link reads.
    fn link_created(&self, thread_id: &str, urls: Vec<String>) {
        let (workbench, thread_id) = (self.clone(), thread_id.to_string());
        let link = move || {
            for url in urls {
                if let Err(error) = workbench.link_created_pull_request(&thread_id, &url) {
                    eprintln!("nexus-workbench: saving the pull request {url}: {error}");
                }
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(link);
            }
            Err(_) => {
                std::thread::spawn(link);
            }
        }
    }

    /// Whether the session can emit updates to this host's view.
    fn shown(&self, thread_id: &str) -> bool {
        // A running session's scope is on its live entry: this runs for every event and streamed delta.
        if let Some(live) = self.live(thread_id) {
            return self.emits_scope(&live.scope);
        }
        self.reader()
            .scope_of(thread_id)
            .ok()
            .flatten()
            .is_some_and(|scope| self.emits_scope(&scope))
    }

    fn emits_scope(&self, scope: &str) -> bool {
        self.in_scope(scope)
            || (SessionScope::decode(scope).is_local() && self.inner.host.shows_account_free_sessions())
    }

    /// Whether `scope` is the current one, without copying the config.
    fn in_scope(&self, scope: &str) -> bool {
        self.inner
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .scope
            == scope
    }

    fn record(&self, thread_id: &str) -> Result<ThreadRecord> {
        self.reader()
            .thread(thread_id)?
            .ok_or_else(|| format!("No session {thread_id}."))
    }

    /// Changes the session's record, saves it and shows it, with the changes its agent has not answered yet.
    fn change(&self, thread_id: &str, change: impl FnOnce(&mut ThreadRecord)) -> Result<ThreadSummary> {
        self.change_at(thread_id, true, change)
    }

    /// `change`, which moves the session's last-active time to now only when `touch` (a pin is not activity).
    fn change_at(&self, thread_id: &str, touch: bool, change: impl FnOnce(&mut ThreadRecord)) -> Result<ThreadSummary> {
        let store = self.store();
        let mut record = store
            .thread(thread_id)?
            .ok_or_else(|| format!("No session {thread_id}."))?;
        let before = record.summary.status;
        let before_mode = record.summary.permission_mode;
        change(&mut record);
        if touch {
            record.summary.updated_at = now_ms();
        }
        let status = record.summary.status;
        // Asking the person wakes a snoozed session early; a new turn brings a settled one back (snooze.rs).
        if status != before && status == ThreadStatus::Approval {
            record.summary.aside.snoozed_until = None;
        }
        if status.busy() && !before.busy() {
            record.summary.aside.settled_at = None;
        }
        if status != before
            && (matches!(
                status,
                ThreadStatus::Approval | ThreadStatus::Interrupted | ThreadStatus::Error
            ) || (status == ThreadStatus::Idle && before.busy()))
        {
            record.summary.listed_at = Some(record.summary.updated_at);
        }
        store.save_thread(&record)?;
        drop(store);
        if record.summary.permission_mode != before_mode {
            self.inner.terminals.revoke_agent_input(thread_id);
        }
        record.summary.pending = self.pending(thread_id);
        record.summary.capabilities = self.capabilities(thread_id);
        self.fill_live_fields(&mut record.summary);
        if self.emits_scope(&record.scope) {
            self.inner.host.emit(Emit::Thread(Box::new(record.summary.clone())));
        }
        Ok(record.summary)
    }

    /// The person sent the session a message: it moves to the top of its project in the sidebar
    /// ([`ThreadSummary::acted_at`]). Not activity: what the agent does next moves `updated_at`.
    pub(crate) fn person_acted(&self, thread_id: &str) -> Result<ThreadSummary> {
        self.change_at(thread_id, false, |record| record.summary.acted_at = Some(now_ms()))
    }

    /// Settles the session because its pull request `number` merged (snooze.rs). None when it does not settle: busy,
    /// archived, opted out, settled already, or settled by that merge before. Not activity.
    pub fn settle_merged(&self, thread_id: &str, number: u64) -> Result<Option<ThreadSummary>> {
        self.settle_merged_at(thread_id, number, now_ms())
    }

    /// Archives the session because its pull request `number` merged and the person turned on "Archive merged
    /// sessions" (Settings › Code). It settles first, so it archives only when [`Workbench::settle_merged`] settles it.
    /// Restored from the archive, it stays in Settled, and the same merge does not archive it again.
    pub fn archive_merged(&self, thread_id: &str, number: u64) -> Result<Option<ThreadSummary>> {
        if self.settle_merged(thread_id, number)?.is_none() {
            return Ok(None);
        }
        let patch = ThreadPatch {
            archived: Some(true),
            ..Default::default()
        };
        self.update(thread_id, patch).map(Some)
    }

    /// [`Workbench::settle_merged`] at `now`.
    pub fn settle_merged_at(&self, thread_id: &str, number: u64, now: i64) -> Result<Option<ThreadSummary>> {
        if !snooze::settles(&self.record(thread_id)?.summary, number) {
            return Ok(None);
        }
        let settled = self
            .change_at(thread_id, false, |record| {
                // Asked again under the store's lock: a turn may have started since.
                if snooze::settles(&record.summary, number) {
                    record.summary.aside.settled_at = Some(now);
                    record.summary.aside.settled_by = Some(number);
                }
            })
            .map(|summary| summary.aside.settled_at.is_some().then_some(summary))?;
        // Keyed by when it settled: a settle seen again runs nothing (`hooks::Fire`'s claim).
        if let Some(at) = settled.as_ref().and_then(|summary| summary.aside.settled_at) {
            self.fire_hooks(hooks::Fire::plain(hooks::HookEvent::Settled, thread_id, at.to_string()));
        }
        Ok(settled)
    }

    /// Wakes a snoozed session early: its pull request's watch told news (snooze.rs). None when it was not snoozed at
    /// `now`. Not activity.
    pub fn wake(&self, thread_id: &str, now: i64) -> Result<Option<ThreadSummary>> {
        if !snooze::snoozed(&self.record(thread_id)?.summary, now) {
            return Ok(None);
        }
        self.change_at(thread_id, false, |record| record.summary.aside.snoozed_until = None)
            .map(Some)
    }

    /// Adds `url` to the pull requests of the session's own worktree, as its latest: the watch found the branch's
    /// pull request, opened outside the app's form (the agent's `gh pr create`, maybe from a new branch). None when
    /// nothing changed: no worktree of its own, removed, a review, or a pull request it has already. Not activity.
    pub fn link_pull_request(&self, thread_id: &str, url: &str) -> Result<Option<ThreadSummary>> {
        let unlinked = |summary: &ThreadSummary| {
            summary.worktree.as_ref().is_some_and(|worktree| {
                !worktree.removed && worktree.review.is_none() && !worktree.pulls().contains(&url)
            })
        };
        if !unlinked(&self.record(thread_id)?.summary) {
            return Ok(None);
        }
        let mut linked = false;
        let summary = self.change_at(thread_id, false, |record| {
            // Asked again under the store's lock: the app's form may have saved it since.
            if unlinked(&record.summary)
                && let Some(worktree) = record.summary.worktree.as_mut()
            {
                worktree.add_pull(url);
                linked = true;
            }
        })?;
        Ok(linked.then_some(summary))
    }

    /// Saves `url`, a pull request the session's agent opened from its shell (created_pulls.rs), as the session's
    /// latest: on its worktree, or on the session when it works in its project's folder. The command printed it, so
    /// no branch is guessed. None when nothing changed: a review, or a pull request it has already. Not activity.
    pub fn link_created_pull_request(&self, thread_id: &str, url: &str) -> Result<Option<ThreadSummary>> {
        let unlinked = |summary: &ThreadSummary| {
            summary
                .worktree
                .as_ref()
                .is_none_or(|worktree| worktree.review.is_none())
                && !summary.pulls().contains(&url)
        };
        if !unlinked(&self.record(thread_id)?.summary) {
            return Ok(None);
        }
        let mut linked = false;
        let summary = self.change_at(thread_id, false, |record| {
            if unlinked(&record.summary) {
                record.summary.add_pull(url);
                linked = true;
            }
        })?;
        Ok(linked.then_some(summary))
    }

    fn set_status(&self, thread_id: &str, status: ThreadStatus) -> Result<ThreadSummary> {
        self.change(thread_id, |record| record.summary.status = status)
    }

    /// The changes the session's running agent has not answered yet (`LiveState::pending`).
    fn pending(&self, thread_id: &str) -> Option<Pending> {
        self.live(thread_id).and_then(|live| live.state().pending())
    }

    /// What the session's running agent accepts (`LiveState::capabilities`): None once its run is stopping or ending.
    fn capabilities(&self, thread_id: &str) -> Option<Box<AgentCapabilities>> {
        self.live(thread_id).and_then(|live| {
            let state = live.state();
            state
                .capabilities
                .clone()
                .filter(|_| !state.done && state.stop.is_none())
                .map(|mut capabilities| {
                    capabilities.connection = Some(Box::new(state.connection.clone()));
                    Box::new(capabilities)
                })
        })
    }

    // Projects.

    pub fn projects(&self) -> Result<Vec<Project>> {
        self.store().projects()
    }

    pub fn project(&self, project_id: &str) -> Result<Project> {
        self.store()
            .project(project_id)?
            .ok_or_else(|| "That project is no longer open.".to_string())
    }

    pub fn add_project(&self, path: &Path) -> Result<Project> {
        if !path.is_dir() {
            return Err(format!("{} is not a folder.", path.display()));
        }
        let project = self.store().add_project(path, now_ms())?;
        self.inner.host.emit(Emit::Projects(self.projects()?));
        Ok(project)
    }

    /// Saves the project's `orchestration` flag (`wb_project_orchestration_set`, kept for older apps). No start reads
    /// it: new sessions follow [`Config::subsessions`].
    pub fn set_project_orchestration(&self, project_id: &str, on: bool) -> Result<Project> {
        self.store().set_project_orchestration(project_id, on)?;
        self.inner.host.emit(Emit::Projects(self.projects()?));
        self.project(project_id)
    }

    /// Pins or unpins the project, and archives it or brings it back (`wb_project_update`, offered as
    /// [`PROJECT_PIN_ARCHIVE`]); None leaves that one as it is. Its sessions, settings, worktrees and files stay.
    pub fn update_project(&self, project_id: &str, pinned: Option<bool>, archived: Option<bool>) -> Result<Project> {
        self.project(project_id)?;
        self.store().set_project_state(project_id, pinned, archived)?;
        self.inner.host.emit(Emit::Projects(self.projects()?));
        self.project(project_id)
    }

    pub fn remove_project(&self, project_id: &str) -> Result<()> {
        self.store().remove_project(project_id)?;
        self.inner.host.emit(Emit::Projects(self.projects()?));
        Ok(())
    }

    // Accounts.

    fn accounts_root(&self) -> Result<PathBuf> {
        self.config()
            .accounts
            .ok_or_else(|| "Extra plan accounts aren't available here.".to_string())
    }

    /// The agent's extra accounts, oldest first.
    pub fn accounts(&self, agent: SessionAgent) -> Vec<String> {
        self.accounts_root()
            .map(|root| accounts::list(&root, agent))
            .unwrap_or_default()
    }

    /// A new extra account of `agent`, sharing what `home` (the agent's own folder) has; signed out until its login.
    pub fn add_account(&self, agent: SessionAgent, home: &Path) -> Result<String> {
        accounts::add(&self.accounts_root()?, agent, home)
    }

    /// Removes an extra account's folder. Its sessions stay, and say so when they next start.
    pub fn remove_account(&self, agent: SessionAgent, account: &str) -> Result<()> {
        accounts::remove(&self.accounts_root()?, agent, account)?;
        if let Some(mut rotation) = self.rotation_of(agent)
            && rotation.active.as_deref() == Some(account)
        {
            rotation.active = None;
        }
        Ok(())
    }

    /// The folder of an extra account of `agent`.
    pub fn account_dir(&self, agent: SessionAgent, account: &str) -> Result<PathBuf> {
        accounts::dir(&self.accounts_root()?, agent, account)
    }

    fn choice_profile(&self, agent: SessionAgent, account: Option<String>) -> Result<Option<PathBuf>> {
        account.map(|id| self.account_dir(agent, &id)).transpose()
    }

    /// The profile New Session would choose from the current rotation readings.
    fn active_choice_profile(&self, agent: SessionAgent) -> Option<PathBuf> {
        let preferred = self.rotation_of(agent)?.active.clone();
        let gone = preferred
            .as_deref()
            .is_some_and(|id| self.account_dir(agent, id).is_err());
        let account = self.rotation_of(agent)?.choose(&preferred, gone);
        self.choice_profile(agent, account).ok().flatten()
    }

    /// Available profiles for discovery of new sessions. One removed profile must not block the others.
    fn available_choice_profiles(&self, agents: &[SessionAgent]) -> computer::settings::AccountProfiles {
        agents
            .iter()
            .filter_map(|&agent| self.active_choice_profile(agent).map(|dir| (agent, dir)))
            .collect()
    }

    /// The Claude account a new session on the person's plan would start on ([`rotate`]); None: Claude Code's own
    /// login.
    pub fn active_account(&self) -> Option<String> {
        self.active_account_of(SessionAgent::Claude)
    }

    /// The account of `agent` (Claude or Codex) that a new session on the person's plan would start on ([`rotate`]);
    /// None: the agent's own login.
    pub fn active_account_of(&self, agent: SessionAgent) -> Option<String> {
        let rotation = self.rotation_of(agent)?;
        rotation.choose(&rotation.active, false)
    }

    /// Where new sessions on the person's Claude plan start (None: Claude Code's own login), as Settings chose it.
    pub fn use_account(&self, account: Option<String>) {
        self.use_account_of(SessionAgent::Claude, account);
    }

    /// Where new sessions of `agent` (Claude or Codex) on the person's plan start (None: the agent's own login).
    pub fn use_account_of(&self, agent: SessionAgent, account: Option<String>) {
        if let Some(mut rotation) = self.rotation_of(agent) {
            rotation.active = account;
        }
    }

    /// Settings' accounts and rotation, one [`accounts::Choice`] per agent. An account removed since falls back to the
    /// agent's own login, not a failed first start.
    pub fn apply_account_choices(&self, choices: &[accounts::Choice]) {
        for choice in choices {
            let account = choice
                .account
                .clone()
                .filter(|account| self.account_dir(choice.agent, account).is_ok());
            self.use_account_of(choice.agent, account);
            self.set_rotation_of(choice.agent, choice.rotate);
        }
    }

    /// Whether sessions move to the Claude account with the most left near a limit ([`rotate`]); on by default.
    pub fn set_rotation(&self, on: bool) {
        self.set_rotation_of(SessionAgent::Claude, on);
    }

    /// Whether sessions of `agent` take the account with the most left near a limit ([`rotate`]). On by default for
    /// Claude, off for Codex. A Codex session takes another account only when it starts.
    pub fn set_rotation_of(&self, agent: SessionAgent, on: bool) {
        if let Some(mut rotation) = self.rotation_of(agent) {
            rotation.manual = !on;
        }
    }

    /// Fresh probes of the plans (Settings asks every minute): what [`rotate`] decides with.
    pub fn checked_plans(&self, plans: &[PlanUsage]) {
        for state in self.inner.accounts.values() {
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .checked(plans);
        }
    }

    fn rotation_of(&self, agent: SessionAgent) -> Option<MutexGuard<'_, Accounts>> {
        self.inner
            .accounts
            .get(&agent)
            .map(|state| state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    fn plan_accounts(summary: &ThreadSummary) -> Option<harness::AccountFolders> {
        (summary.provider.is_none() && summary.via == Via::Subscription)
            .then(|| harness::harness(summary.agent).accounts())
            .flatten()
    }

    /// The rotation state of a plan session that can move between turns.
    fn live_plan_rotation(&self, summary: &ThreadSummary) -> Option<MutexGuard<'_, Accounts>> {
        Self::plan_accounts(summary).filter(|folders| folders.moves_running_session)?;
        self.rotation_of(summary.agent)
    }

    /// Chooses a start's account and logs any move. A linked session never changes the active account.
    async fn rotate(&self, record: &ThreadRecord, folders: harness::AccountFolders) -> Result<Option<String>> {
        let agent = record.summary.agent;
        let state = self
            .inner
            .accounts
            .get(&agent)
            .ok_or("This agent has no account rotation state.")?;
        let rotation = || state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // A linked session starts on its parent's account (`start_child`), and never moves the active one.
        let linked = record.summary.parent_thread_id.is_some();
        let settled = record.native_started || linked || record.summary.source_thread_id.is_some();
        if !folders.moves_running_session && settled {
            return Ok(record.summary.account.clone());
        }
        // A start-only agent with manual rotation skips the limits probe.
        let wants_probe = {
            let rotation = rotation();
            rotation.stale() && (folders.moves_running_session || !rotation.manual)
        };
        if wants_probe {
            let plans = self.inner.host.plans().await;
            if !plans.is_empty() {
                rotation().checked(&plans);
            }
        }
        let thread_id = &record.summary.id;
        let preferred = if settled {
            record.summary.account.clone()
        } else {
            rotation().active.clone()
        };
        let gone = preferred
            .as_deref()
            .is_some_and(|account| self.account_dir(agent, account).is_err());
        let (chosen, moved) = {
            let mut rotation = rotation();
            let chosen = rotation.choose_for(&preferred, gone, record.summary.model.as_deref());
            // An older session resumed on another account leaves the active one as it is.
            if !linked && rotation.active == preferred {
                rotation.active = chosen.clone();
            }
            let moved = (chosen != preferred).then(|| rotation.moved(&preferred, &chosen));
            (chosen, moved)
        };
        if let Some(text) = moved {
            self.append(thread_id, WbEvent::Notice { text })?;
        }
        if chosen != record.summary.account {
            let gate = self.settings_gate(thread_id);
            let _held = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            self.change(thread_id, |record| record.summary.account = chosen.clone())?;
        }
        Ok(chosen)
    }

    /// Refreshes stale readings if the session is near a limit and can move.
    async fn check_before_move(&self, summary: &ThreadSummary) {
        let wanted = self.live_plan_rotation(summary).is_some_and(|rotation| {
            rotation.stale() && rotation.near_limit_for(&summary.account, summary.model.as_deref())
        });
        if wanted {
            let plans = self.inner.host.plans().await;
            if !plans.is_empty()
                && let Some(mut rotation) = self.live_plan_rotation(summary)
            {
                rotation.checked(&plans);
            }
        }
    }

    /// Whether another account has room before the session's next turn.
    fn must_move(&self, summary: &ThreadSummary, live: &Live) -> bool {
        Self::between_turns(live)
            && self.live_plan_rotation(summary).is_some_and(|rotation| {
                rotation.choose_for(&summary.account, false, summary.model.as_deref()) != summary.account
            })
    }

    /// The agent is free to stop for a new process: started, with no turn, ask or background work, and not stopping.
    /// Background work would die with the agent: a stop waits for it.
    fn between_turns(live: &Live) -> bool {
        let state = live.state();
        state.initialized
            && state.turns.is_empty()
            && state.pending.is_empty()
            && state.background.is_empty()
            && state.stop.is_none()
    }

    /// Whether agy offers its `auto` mode, as its last run said.
    fn agy_auto(&self) -> Option<bool> {
        *self.inner.agy_auto.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Antigravity's launch arguments changed since this agy started: its Sandbox switch, its permission mode, or
    /// whether agy offers `auto`, which a run tells only once it runs. The next message, sent between turns, starts agy
    /// again with the new arguments on the same conversation, as a move to another account does. A message sent while
    /// a turn runs still goes to the agy running it.
    fn antigravity_args_changed(&self, summary: &ThreadSummary, live: &Live) -> bool {
        live.agent == SessionAgent::Antigravity
            && Self::between_turns(live)
            && live.antigravity_args
                != harness::antigravity::args(
                    summary.permission_mode,
                    self.inner.host.antigravity_sandbox(),
                    self.agy_auto(),
                )
    }

    /// The person's plan stopped the session's turn at its usage limit, which resets at `resets_at`: the session offers
    /// Continue now and, with a reset time, Continue at reset. Not at reset when another of the person's Claude
    /// accounts has room ([`rotate`] goes first): Continue now moves the session there.
    fn limit_stop(&self, thread_id: &str, resets_at: Option<i64>) -> Result<LimitStop> {
        let summary = self.record(thread_id)?.summary;
        let elsewhere = self.live_plan_rotation(&summary).is_some_and(|rotation| {
            rotation.choose_for(&summary.account, false, summary.model.as_deref()) != summary.account
        });
        Ok(LimitStop {
            resets_at: resets_at.filter(|_| !elsewhere),
            scheduled: false,
        })
    }

    /// Continues each session set to continue when its plan's limit resets ("Continue at reset") once that time is
    /// past, with the Continue the person would send. Sessions the app was closed through continue at the first call.
    /// The host calls it once a minute with the time (`now`, epoch milliseconds), so a session continues where it runs:
    /// a remote host's on the host. Each stop is cleared before its message goes, so it never continues twice.
    pub async fn continue_at_resets(&self, now: i64) {
        let scope = self.config().scope;
        let scheduled = self.store().scheduled_continues(&scope);
        let due = match scheduled {
            Ok(due) => due,
            Err(error) => {
                eprintln!("nexus-workbench: scheduled continues: {error}");
                return;
            }
        };
        for (thread_id, resets_at) in due {
            if now < resets_at.saturating_add(RESET_GRACE_MS) {
                break;
            }
            // Taken under the store's lock: of two calls, only the first finds it scheduled.
            let mut taken = false;
            let cleared = self.change_at(&thread_id, false, |record| {
                if record.summary.limited.is_some_and(|stop| stop.scheduled) {
                    record.summary.limited = None;
                    taken = true;
                }
            });
            if cleared.is_err() || !taken {
                continue;
            }
            if let Err(error) = self.resume(&thread_id).await {
                let text = format!("Nexus could not continue this session after its plan's limit reset: {error}");
                if let Err(error) = self.append(&thread_id, WbEvent::Notice { text }) {
                    eprintln!("nexus-workbench: {thread_id}: {error}");
                }
            }
        }
    }

    /// A message to the session takes the place of the Continue its plan's limit offered.
    fn clear_limit_stop(&self, thread_id: &str) -> Result<()> {
        if self.record(thread_id)?.summary.limited.is_some() {
            self.change_at(thread_id, false, |record| record.summary.limited = None)?;
        }
        Ok(())
    }

    // Sessions.

    /// The current scope's sessions, most recently active first.
    pub fn threads(&self) -> Result<Vec<ThreadSummary>> {
        let scope = self.config().scope;
        let records = self.store().threads(&scope)?;
        Ok(records
            .into_iter()
            .map(|record| {
                let mut summary = ThreadSummary {
                    pending: self.pending(&record.summary.id),
                    capabilities: self.capabilities(&record.summary.id),
                    ..record.summary
                };
                self.fill_live_fields(&mut summary);
                summary
            })
            .collect())
    }

    pub fn events(&self, thread_id: &str, after_seq: i64) -> Result<Vec<StoredEvent>> {
        // A screen reading the log may not have the streamed text so far (it opened the session mid-answer, or missed
        // a delta): the next delta of each stream is whole again.
        if let Some(live) = self.live(thread_id) {
            live.state().streamed.clear();
        }
        self.reader().events_after(thread_id, after_seq)
    }

    /// A page of the session's log (page.rs): the latest when `before` is None, else the one ending before it.
    pub fn events_page(&self, thread_id: &str, before: Option<i64>, limit: Option<usize>) -> Result<store::EventsPage> {
        // The latest page stands in for a read of the log (see `events`): the next delta of each stream is whole again.
        if before.is_none()
            && let Some(live) = self.live(thread_id)
        {
            live.state().streamed.clear();
        }
        self.reader().events_page(thread_id, before, limit)
    }

    /// The current scope's sessions whose messages match `query` (`Reader::search`). Reads without taking the store.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<store::SearchHit>> {
        self.reader().search(&self.config().scope, query, limit)
    }

    /// Creates a session and starts its agent with the first message.
    pub async fn create_thread(&self, new: NewThread) -> Result<ThreadSummary> {
        self.create_thread_id(new, None, String::new(), None, None).await
    }

    /// [`Workbench::create_thread`] with `context` said to the agent after the first message, but not logged: what
    /// the sessions and pull requests it mentions are ([`mentions::block`]).
    pub async fn create_thread_mentioning(&self, new: NewThread, context: String) -> Result<ThreadSummary> {
        self.create_thread_in_scope(new, context, None).await
    }

    pub(crate) async fn create_thread_in_scope(
        &self,
        new: NewThread,
        context: String,
        expected_scope: Option<&api::StartAuthority>,
    ) -> Result<ThreadSummary> {
        self.create_thread_id(new, None, context, None, expected_scope).await
    }

    /// One session per renderer request, including retries after a lost command response.
    pub async fn create_thread_once(&self, new: NewThread, request_id: &str) -> Result<ThreadSummary> {
        self.create_thread_once_mentioning(new, request_id, String::new()).await
    }

    /// [`Workbench::create_thread_once`] with `context`, as [`Workbench::create_thread_mentioning`] says.
    pub async fn create_thread_once_mentioning(
        &self,
        new: NewThread,
        request_id: &str,
        context: String,
    ) -> Result<ThreadSummary> {
        self.create_thread_once_mentioning_guarded(new, request_id, context, None)
            .await
    }

    pub(crate) async fn create_thread_once_mentioning_guarded(
        &self,
        new: NewThread,
        request_id: &str,
        context: String,
        guard: Option<CreationGuard>,
    ) -> Result<ThreadSummary> {
        self.create_thread_once_in_scope(new, request_id, context, guard, None)
            .await
    }

    pub(crate) async fn create_thread_once_in_scope(
        &self,
        new: NewThread,
        request_id: &str,
        context: String,
        guard: Option<CreationGuard>,
        expected_scope: Option<&api::StartAuthority>,
    ) -> Result<ThreadSummary> {
        if let Some(scope) = expected_scope {
            self.check_start_scope(scope)?;
        }
        if new.provider.is_some() {
            self.check_saved_provider_creation(guard.as_ref())?;
        }
        let id = creation_thread_id(request_id)?;
        let gate = self.creation_gate(&id);
        let _guard = gate.lock().await;
        let admitted = self.check_creation_admission(guard.as_ref()).await?;
        if let Some(scope) = expected_scope {
            self.check_start_scope(scope)?;
        }
        if self.store().thread(&id)?.is_some() {
            // record checks the organization; an ID never grants access to another scope.
            let record = self.record(&id)?;
            if record.summary.project_id != new.project_id || record.summary.agent != new.agent {
                return Err(format!(
                    "Creation ID {request_id} already started a session in project {} with {:?}; this request asks for project {} with {:?}.",
                    record.summary.project_id, record.summary.agent, new.project_id, new.agent
                ));
            }
            if record.summary.provider != new.provider {
                return Err("This creation request already saved a different provider choice.".into());
            }
            // A scope switch can stop the first send after the row was saved. Resume only that unsent row.
            if record.summary.status == ThreadStatus::Idle
                && self
                    .events(&id, 0)?
                    .iter()
                    .all(|stored| !matches!(stored.event, WbEvent::User { .. }))
            {
                self.send_creation_message(&id, &new.text, new.images, &context, admitted.as_ref())
                    .await?;
                return self.record(&id).map(|record| record.summary);
            }
            return Ok(record.summary);
        }
        self.create_thread_id(new, Some(id), context, guard, expected_scope)
            .await
    }

    /// The lock retries of one creation (the session `id`) share.
    fn creation_gate(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.inner.creations.lock().unwrap_or_else(|p| p.into_inner());
        gates.retain(|_, gate| gate.strong_count() > 0);
        let gate = gates
            .get(id)
            .and_then(std::sync::Weak::upgrade)
            .unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(())));
        gates.insert(id.to_string(), Arc::downgrade(&gate));
        gate
    }

    pub(crate) fn check_creation_guard(&self, guard: Option<&CreationGuard>) -> Result<()> {
        if let Some(guard) = guard {
            let trust = self
                .inner
                .host
                .session_sync()
                .map_or(session_sync::DeviceTrust::Approvals, |backend| backend.device_trust());
            let current = self.config().scope == guard.identity.scope
                && crate::now_ms() < guard.expires_at
                && self.session_sync_identity(&guard.identity.scope).as_ref() == Some(&guard.identity)
                && self.store().control_allows_in(&guard.identity, trust, &guard.sender)?
                && self
                    .store()
                    .computer_info(&guard.identity.scope, &guard.identity.user)?
                    .generation
                    == guard.generation;
            if !current {
                return Err("Computer request expired, was disabled, or its account changed.".into());
            }
        }
        Ok(())
    }
    async fn check_creation_admission(&self, guard: Option<&CreationGuard>) -> Result<Option<AdmittedExecution>> {
        self.check_creation_guard(guard)?;
        let Some(guard) = guard else { return Ok(None) };
        let backend = self.inner.host.session_sync().ok_or("Computer connection changed.")?;
        if !matches!(
            self.sync_policy(backend.as_ref()).await,
            Ok(session_sync::ContentPolicy::UserOnly | session_sync::ContentPolicy::UserAndOrgAdmins)
        ) {
            return Err("Encrypted session control is unavailable.".into());
        }
        let transport = backend.computer().ok_or("Computer connection changed.")?;
        if !matches!(
            tokio::time::timeout(Duration::from_secs(5), transport.admission_sender(guard.sender.clone())).await,
            Ok(Ok(true))
        ) {
            return Err("The requesting device is no longer trusted.".into());
        }
        let admitted = if let Some((agent, selection)) = &guard.selection {
            let profile = self.active_choice_profile(*agent);
            self.check_execution_selection(*agent, profile.clone(), selection)
                .await?;
            Some(AdmittedExecution {
                agent: *agent,
                profile,
                selection: selection.clone(),
            })
        } else {
            None
        };
        self.check_creation_guard(Some(guard))?;
        Ok(admitted)
    }

    async fn check_execution_selection(
        &self,
        agent: SessionAgent,
        profile: Option<PathBuf>,
        selection: &computer::Selection,
    ) -> Result<()> {
        let profiles = profile.into_iter().map(|dir| (agent, dir)).collect();
        let choices = self
            .inner
            .host
            .computer_choices_by_account(vec![agent], profiles)
            .await?;
        if !computer::settings::accepts(&choices.agents, agent, selection) {
            return Err("The selected model or payer is no longer available. Refresh the choices.".into());
        }
        Ok(())
    }

    pub(crate) fn check_saved_provider_creation(&self, guard: Option<&CreationGuard>) -> Result<()> {
        if !SessionScope::decode(&self.config().scope).is_local() {
            return Err(provider::saved_provider_refusal(true));
        }
        if !self.inner.host.local_providers_opt_in().enabled() || guard.is_some() {
            return Err(provider::saved_provider_refusal(false));
        }
        self.inner.host.check_saved_provider_owner()
    }

    pub(crate) fn check_saved_provider_launch(&self, thread_id: &str) -> Result<()> {
        let record = self.record(thread_id)?;
        self.check_saved_provider_record(&record)
    }

    fn check_saved_provider_record(&self, record: &ThreadRecord) -> Result<()> {
        if record.summary.provider.is_none() {
            return Ok(());
        }
        self.check_saved_provider_creation(None)?;
        if !SessionScope::decode(&record.scope).is_local() {
            return Err(provider::saved_provider_refusal(true));
        }
        Ok(())
    }

    pub(crate) fn check_start_scope(&self, source: &api::StartAuthority) -> Result<()> {
        if self.in_scope(&source.scope) && self.inner.host.session_choice_identity() == source.identity {
            Ok(())
        } else {
            Err("The account changed before the session could start.".into())
        }
    }

    async fn create_thread_id(
        &self,
        new: NewThread,
        id: Option<String>,
        context: String,
        guard: Option<CreationGuard>,
        expected_scope: Option<&api::StartAuthority>,
    ) -> Result<ThreadSummary> {
        if new.provider.is_some() {
            self.check_saved_provider_creation(guard.as_ref())?;
            if new.parent.is_some() {
                return Err("This computer cannot start a linked local provider session.".into());
            }
        }
        self.check_creation_admission(guard.as_ref()).await?;
        let is_local_provider = new.provider.is_some();
        let scope = if is_local_provider {
            SessionScope::Local.into_text()
        } else {
            guard
                .as_ref()
                .map(|g| g.identity.scope.clone())
                .unwrap_or_else(|| self.config().scope)
        };
        if let Some(expected_scope) = expected_scope
            && expected_scope.scope != scope
        {
            return Err("The account changed before the session could start.".into());
        }
        let sync_identity = if is_local_provider {
            None
        } else {
            guard
                .as_ref()
                .map(|g| g.identity.clone())
                .or_else(|| self.session_sync_identity(&scope))
        };
        let _update_activity = UpdateActivity::begin(self)?;
        if new.text.trim().is_empty() && new.images.is_empty() {
            return Err("Write a first message for the agent.".into());
        }
        check_images(&new.images)?;
        driver::check_mode(new.agent, new.permission_mode)?;
        if let Some(name) = &new.name {
            self.check_name(name, None)?;
        }
        let project = self
            .store()
            .project(&new.project_id)?
            .ok_or_else(|| "That project is no longer open.".to_string())?;
        let id = id.unwrap_or_else(|| format!("t_{}", short_id()));
        // A crew's lead starting it: one level below the lead.
        let depth = match &new.parent {
            Some(link) => self.record(&link.thread_id)?.summary.depth + 1,
            None => 0,
        };
        let worktree = match new.worktree {
            Some(wanted) if new.provider.is_none() => Some(self.make_worktree(&project, &id, &new.text, wanted).await?),
            _ => None,
        };
        self.check_creation_admission(guard.as_ref()).await?;
        let now = now_ms();
        let record = ThreadRecord {
            summary: ThreadSummary {
                id,
                project_id: project.id,
                title: title_of(&mentions::plain(&new.text)),
                name: new.name,
                agent: new.agent,
                model: new.model.filter(|m| !m.is_empty()),
                permission_mode: new.permission_mode,
                effort: new.effort,
                fast: false,
                via: new.via,
                provider: new.provider,
                account: None,
                status: ThreadStatus::Idle,
                cost_usd: 0.0,
                nexus_sessions: vec![],
                context_tokens: None,
                context_window: None,
                created_at: now,
                updated_at: now,
                listed_at: Some(now),
                acted_at: Some(now),
                archived: false,
                pinned: false,
                worktree,
                pull_requests: Vec::new(),
                pending: None,
                capabilities: None,
                parent_thread_id: new.parent.as_ref().map(|link| link.thread_id.clone()),
                source_thread_id: None,
                side_chat: false,
                spawned_by: new.parent.as_ref().map(|link| link.spawned_by.clone()),
                group_id: new.parent.and_then(|link| link.group_id),
                depth,
                orchestration: new.orchestration,
                waits_on: vec![],
                pending_results: 0,
                start_choices: vec![],
                halt: None,
                crew_paused: false,
                crew_tripped: false,
                crew_spend_trip: None,
                in_background: false,
                background: vec![],
                background_total: 0,
                at_depth_limit: false,
                handoff_from: None,
                limited: None,
                aside: Default::default(),
            },
            native_id: uuid::Uuid::new_v4().to_string(),
            native_started: false,
            scope,
            fork_from: None,
            seed: None,
            automatic_title: true,
            delivered_seq: 0,
            halt: None,
        };
        self.check_creation_admission(guard.as_ref()).await?;
        if let Some((_, selection)) = guard.as_ref().and_then(|g| g.selection.as_ref()) {
            self.store()
                .computer_require_payer(&record.summary.id, &record.scope, selection.payer)?;
        }
        let id = record.summary.id.clone();
        if let Some(scope) = expected_scope {
            self.check_start_scope(scope)?;
        }
        self.check_saved_provider_record(&record)?;
        let saved_provider_start =
            (is_local_provider && expected_scope.is_some()).then(|| SavedProviderStart::hold(self, &id));
        self.store().insert_thread(&record)?;
        self.register_session_sync(&id, sync_identity.as_ref());
        self.inner.host.emit(Emit::Thread(Box::new(record.summary.clone())));
        let outcome = async {
            // A new worktree's setup script runs first: the first message waits for it, without holding this up.
            let admitted = self.check_creation_admission(guard.as_ref()).await?;
            if let Some(scope) = expected_scope {
                self.check_start_scope(scope)?;
            }
            if self.begin_setup(&record.summary, &project.path)? {
                self.send_after_setup(id.clone(), new.text, new.images, context, admitted);
            } else {
                self.send_creation_message(&id, &new.text, new.images, &context, admitted.as_ref())
                    .await?;
            }
            self.record(&id).map(|record| record.summary)
        }
        .await;
        // These starts run in the project folder, so the first send is never deferred for worktree setup.
        // Return to the draft and remove the new row if there was no spawn, including after a stop or sign-in.
        if saved_provider_start.is_some()
            && let Some(scope) = expected_scope
        {
            let spawned = Gated::gate(self).saved_provider_starts.get(&id) == Some(&true);
            if !spawned {
                let refusal = outcome
                    .err()
                    .or_else(|| self.check_start_scope(scope).err())
                    .unwrap_or_else(|| "The local provider could not start. Check it in Settings.".into());
                self.delete_thread(&id)
                    .await
                    .map_err(|error| format!("{refusal} Could not remove the failed session: {error}"))?;
                return Err(refusal);
            }
        }
        outcome
    }

    /// A new worktree for session `id`, at `<worktrees>/<repository>/<branch>` (a free name: `-2`, `-3`… when taken).
    async fn make_worktree(&self, project: &Project, id: &str, text: &str, wanted: NewWorktree) -> Result<Worktree> {
        if let Some(review) = wanted.review {
            return self.make_review_worktree(project, review).await;
        }
        let config = self.config();
        let root = config
            .worktrees
            .ok_or_else(|| "Worktrees aren't available here.".to_string())?;
        let repo = PathBuf::from(&project.path);
        let prefix = wanted.branch_prefix.unwrap_or(config.branch_prefix);
        git::check_branch_prefix(&prefix)?;
        let branch = wanted
            .branch
            .map(|branch| branch.trim().to_string())
            .filter(|branch| !branch.is_empty())
            .unwrap_or_else(|| branch_for(&prefix, text, id));
        let worktree = tokio::task::spawn_blocking(move || {
            let base = wanted
                .base
                .filter(|base| !base.trim().is_empty())
                .or_else(|| git::default_base(&repo))
                .ok_or_else(|| format!("{} has no branch to start a worktree from.", repo.display()))?;
            let parent = root.join(repo.file_name().unwrap_or_default());
            let name = branch.replace('/', "-");
            let path = (1..)
                .map(|n| parent.join(if n == 1 { name.clone() } else { format!("{name}-{n}") }))
                .find(|path| !path.exists())
                .unwrap_or_else(|| parent.join(&name));
            git::add_worktree(&repo, &path, &branch, &base)?;
            Ok::<_, String>(Worktree {
                path: path.to_string_lossy().into_owned(),
                branch,
                base,
                removed: false,
                pull_request: None,
                pull_requests: Vec::new(),
                review: None,
            })
        })
        .await
        .map_err(|error| error.to_string())??;
        Ok(worktree)
    }

    /// A review session's worktree: the pull request's head on a branch `review/pr-<number>`, at
    /// `<reviews>/<repository>/pr-<number>` (`-2`, `-3`… when the folder or the branch is taken).
    async fn make_review_worktree(&self, project: &Project, review: NewReview) -> Result<Worktree> {
        let root = self
            .config()
            .reviews
            .ok_or_else(|| "Review sessions aren't available here.".to_string())?;
        let repo = PathBuf::from(&project.path);
        tokio::task::spawn_blocking(move || {
            let (path, branch) = review_place(&root, &repo, review.number);
            git::add_review_worktree(&repo, &path, &branch, review.number, &review.base, &review.tool_path)?;
            Ok::<_, String>(Worktree {
                path: path.to_string_lossy().into_owned(),
                branch,
                base: format!("origin/{}", review.base),
                removed: false,
                pull_request: None,
                pull_requests: Vec::new(),
                review: Some(crate::event::Review {
                    number: review.number,
                    url: review.url,
                }),
            })
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// Takes a session's worktree off the disk, unless that would lose work (uncommitted changes, unpushed commits) or
    /// its agent is running. The session stays, readable; its branch stays too.
    pub async fn remove_worktree(&self, thread_id: &str) -> Result<ThreadSummary> {
        let record = self.record(thread_id)?;
        let worktree = record
            .summary
            .worktree
            .clone()
            .filter(|worktree| !worktree.removed)
            .ok_or_else(|| "This session has no worktree to remove.".to_string())?;
        if self.is_live(thread_id) {
            return Err("Stop the session's agent first.".into());
        }
        if self.side_chat_live(thread_id) {
            return Err("Stop the agents of this session's side chats first: they work in its worktree.".into());
        }
        let project = self
            .store()
            .project(&record.summary.project_id)?
            .ok_or_else(|| "This session's project is no longer open.".to_string())?;
        let path = worktree.path.clone();
        let prefix = checkpoint_prefix(thread_id);
        tokio::task::spawn_blocking(move || {
            let repo = Path::new(&project.path);
            git::remove_worktree(repo, Path::new(&path))?;
            // The worktree's checkpoints went with it.
            git::drop_refs(repo, &prefix)
        })
        .await
        .map_err(|error| error.to_string())??;
        self.change(thread_id, |record| {
            if let Some(worktree) = record.summary.worktree.as_mut() {
                worktree.removed = true;
            }
        })
    }

    /// Deletes a session for good. Its agent is stopped and waited for; the worktree the app made for it is removed,
    /// with its branch unless that holds commits found nowhere else; its checkpoint refs are dropped; then it and its
    /// log leave the store. A worktree with uncommitted changes or unpushed commits refuses the whole delete, before
    /// anything is stopped. The project's own folder is never touched, nor the agent's own transcript of the
    /// conversation (Claude Code's or Codex's files).
    ///
    /// Once the agent is stopped and nothing would be lost, the session goes even when its files don't: Windows keeps
    /// a folder while a program holds a file in it open. Returns what stayed, in words for the person.
    pub async fn delete_thread(&self, thread_id: &str) -> Result<Vec<String>> {
        self.delete_thread_with_options(thread_id, false).await
    }

    /// With confirmation, remove the entire session worktree. Its local branch stays.
    pub async fn delete_thread_with_options(&self, thread_id: &str, discard_uncommitted: bool) -> Result<Vec<String>> {
        let record = self.record(thread_id)?;
        let project = self.store().project(&record.summary.project_id)?;
        let worktree = record.summary.worktree.clone();
        // The worktree git still knows of, even when its folder was deleted by hand (`remove_worktree` then prunes it,
        // or git would refuse to delete its branch).
        let known_worktree = worktree
            .as_ref()
            .filter(|worktree| !worktree.removed)
            .map(|worktree| PathBuf::from(&worktree.path));
        // A folder git no longer reads as a worktree has nothing to check: an earlier delete removed what it could.
        if !discard_uncommitted && let Some(dir) = known_worktree.clone().filter(|dir| git::is_worktree(dir)) {
            tokio::task::spawn_blocking(move || git::would_lose_work(&dir))
                .await
                .map_err(|error| error.to_string())??;
        }
        // Side chats use this folder. Stop them before removing it and keep their logs until removal succeeds.
        let side_chats = self.reader().side_chats(thread_id)?;
        for side in &side_chats {
            self.stop_tree_and_wait(&side.summary.id).await?;
            self.inner.terminals.close_thread(&side.summary.id);
        }
        // Stopped as the person's Stop does, with every session below it, children still starting included, and waited
        // for ([`linked::halts`]).
        self.stop_tree_and_wait(thread_id).await?;
        self.inner.terminals.close_thread(thread_id);
        // A checkpoint still being written would land after the refs are dropped, and stay (R5).
        self.checkpoint_written(thread_id).await;
        // Without its project the repository is unknown: the session goes, any refs or worktree stay behind.
        let mut left = Vec::new();
        if let Some(project) = project {
            let prefix = checkpoint_prefix(thread_id);
            left = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
                let repo = Path::new(&project.path);
                let mut left = Vec::new();
                if let Some(dir) = known_worktree {
                    // Checks again: the agent may have written more before it stopped.
                    if !discard_uncommitted && git::is_worktree(&dir) {
                        git::would_lose_work(&dir)?;
                    }
                    if let Err(error) = git::drop_worktree_with_force(repo, &dir, discard_uncommitted) {
                        if discard_uncommitted && git::is_worktree(&dir) {
                            return Err(error);
                        }
                        left.push(format!(
                            "Its folder {} is still there ({error}). Close what uses it, then delete it.",
                            dir.display()
                        ));
                    }
                }
                if !git::in_repo(repo) {
                    return Ok(left);
                }
                if !discard_uncommitted
                    && let Some(worktree) = worktree
                    && let Err(error) = git::delete_branch_if_safe(repo, &worktree.branch)
                {
                    left.push(format!("Its branch {} is still there ({error}).", worktree.branch));
                }
                if let Err(error) = git::drop_refs(repo, &prefix) {
                    left.push(format!("Its checkpoints are still in the repository ({error})."));
                }
                Ok(left)
            })
            .await
            .map_err(|error| error.to_string())??;
        }
        self.checkpoints().remove(thread_id);
        for side in side_chats {
            // Another window may have closed the side chat while the parent was stopping.
            if let Err(error) = Box::pin(self.delete_thread_with_options(&side.summary.id, false)).await
                && self.store().thread(&side.summary.id)?.is_some()
            {
                return Err(error);
            }
        }
        // Drain uploads only after the filesystem checks succeed. The tombstone then follows every upload.
        let sync = self.session_sync_lock(thread_id);
        let _sync = sync.lock().await;
        self.store().delete_thread(thread_id)?;
        self.forget_transcripts(thread_id);
        self.forget_links(thread_id);
        // The sessions it started become the person's own, unhalted, with their worktrees.
        self.free_children(thread_id)?;
        Ok(left)
    }

    /// A new session that goes on from `source` as it was when the turn that ended at event `turn` ended (None: its
    /// latest turn), in a new worktree holding the files as that turn left them. The conversation so far is copied
    /// into its log. Its agent starts with its first message: from the latest turn, as a fork of the source's own
    /// conversation (Claude Code's `--fork-session`, Codex's `thread/fork`); from an earlier one, as a new
    /// conversation that is first told what was said up to there. Needs the turn's snapshot (`changed_turns`), so
    /// only a session in a git repository forks.
    pub async fn fork_thread(&self, source_id: &str, turn: Option<i64>) -> Result<ThreadSummary> {
        self.fork_thread_with_subscription(source_id, turn, None).await
    }

    /// Fork with a selected subscription, seeding a fresh conversation when the login or agent changes.
    pub async fn fork_thread_with_subscription(
        &self,
        source_id: &str,
        turn: Option<i64>,
        subscription: Option<ForkSubscription>,
    ) -> Result<ThreadSummary> {
        // Capture one incarnation and its events together, before asynchronous filesystem work.
        let (source, all) = {
            let store = self.store();
            let source = store
                .thread(source_id)?
                .ok_or_else(|| format!("No session {source_id}."))?;
            if !self.emits_scope(&source.scope) {
                return Err("That session is not in this organization.".into());
            }
            (source, store.events_after(source_id, 0)?)
        };
        if source.summary.provider.is_some() {
            return Err("This build cannot fork a session with a saved local provider.".into());
        }
        // A side chat keeps no files of its own to fork from (side_chat.rs).
        if source.summary.side_chat {
            return Err("A side chat can't be forked. Bring its answer back to its session instead.".into());
        }
        if let Some(subscription) = &subscription {
            if !harness::harness(subscription.agent).allows_subscription_fork() {
                return Err("Choose a Claude or ChatGPT subscription.".into());
            }
            if let Some(account) = &subscription.account {
                self.account_dir(subscription.agent, account)?;
            }
        }
        let sync_identity = self.inherited_sync_identity(&source);
        let project = self
            .store()
            .project(&source.summary.project_id)?
            .ok_or_else(|| "This session's project is no longer open.".to_string())?;
        let root = self
            .config()
            .worktrees
            .ok_or_else(|| "Worktrees aren't available here.".to_string())?;
        let dir = self
            .workdir(&source)
            .ok_or_else(|| "This session's folder is gone.".to_string())?;
        let prefix = checkpoint_prefix(source_id);
        let names = {
            let (dir, prefix) = (dir.clone(), prefix.clone());
            tokio::task::spawn_blocking(move || git::refs(&dir, &prefix))
                .await
                .map_err(|error| error.to_string())?
                .unwrap_or_default()
        };
        // Reverted turns can't be forked from, and a fork leaves them out (`revert_to_turn`).
        let (kept, rewind) = conversation(&all, source.summary.agent);
        if let Some(turn) = turn
            && turn != 0
            && !kept.iter().any(|(seq, _)| *seq == turn)
        {
            return Err("That turn was reverted, so it can't be forked.".into());
        }
        // The turn's end, or the latest one recorded and not reverted (0: the folder before the first turn).
        let at = match turn {
            Some(turn) => names
                .iter()
                .find(|name| seq_of(name) == Some(turn))
                .and(Some(turn))
                .ok_or_else(|| "That turn's files weren't recorded, so it can't be forked.".to_string())?,
            None => names
                .iter()
                .filter_map(|name| seq_of(name))
                .filter(|seq| *seq == 0 || kept.iter().any(|(kept, _)| kept == seq))
                .max()
                .ok_or_else(|| "This session has no recorded files to fork from yet.".to_string())?,
        };
        let events = copied_until(&all, at);
        let same_login = subscription.as_ref().is_none_or(|plan| {
            source.summary.agent == plan.agent
                && source.summary.via == Via::Subscription
                && source.summary.account == plan.account
        });
        let at_end = same_login && forks_at_end(&all, at, rewind.is_some(), source.native_started);

        let id = format!("t_{}", short_id());
        let title = format!("Fork of {}", source.summary.title);
        let branch = branch_for(&self.config().branch_prefix, &title, &id);
        let repo = PathBuf::from(&project.path);
        let base = source.summary.worktree.as_ref().map(|worktree| worktree.base.clone());
        let refname = format!("{prefix}{at:010}");
        let worktree = tokio::task::spawn_blocking(move || {
            let base = base
                .or_else(|| git::default_base(&repo))
                .ok_or_else(|| format!("{} has no branch to start a worktree from.", repo.display()))?;
            let parent = root.join(repo.file_name().unwrap_or_default());
            let name = branch.replace('/', "-");
            let path = (1..)
                .map(|n| parent.join(if n == 1 { name.clone() } else { format!("{name}-{n}") }))
                .find(|path| !path.exists())
                .unwrap_or_else(|| parent.join(&name));
            git::fork_worktree(&repo, &path, &branch, &refname, &base)?;
            Ok::<_, String>(Worktree {
                path: path.to_string_lossy().into_owned(),
                branch,
                base,
                removed: false,
                pull_request: None,
                pull_requests: Vec::new(),
                review: None,
            })
        })
        .await
        .map_err(|error| error.to_string())??;

        let now = now_ms();
        let s = &source.summary;
        let record = ThreadRecord {
            summary: ThreadSummary {
                name: None,
                id: id.clone(),
                project_id: s.project_id.clone(),
                title,
                agent: subscription.as_ref().map_or(s.agent, |plan| plan.agent),
                model: if subscription.as_ref().is_none_or(|plan| plan.agent == s.agent) {
                    s.model.clone()
                } else {
                    None
                },
                permission_mode: s.permission_mode,
                effort: if subscription.as_ref().is_none_or(|plan| plan.agent == s.agent) {
                    s.effort
                } else {
                    None
                },
                fast: s.fast && subscription.as_ref().is_none_or(|plan| plan.agent == s.agent),
                via: if subscription.is_some() {
                    Via::Subscription
                } else {
                    s.via
                },
                provider: None,
                account: subscription
                    .as_ref()
                    .map_or_else(|| s.account.clone(), |plan| plan.account.clone()),
                status: ThreadStatus::Idle,
                cost_usd: 0.0,
                nexus_sessions: vec![],
                context_tokens: None,
                context_window: None,
                created_at: now,
                updated_at: now,
                listed_at: Some(now),
                acted_at: Some(now),
                archived: false,
                pinned: false,
                worktree: Some(worktree),
                pull_requests: Vec::new(),
                pending: None,
                capabilities: None,
                // A fork is the person's own session, whatever its source was; it remembers that source only to send
                // its result back.
                parent_thread_id: None,
                source_thread_id: Some(source_id.to_string()),
                side_chat: false,
                spawned_by: None,
                group_id: None,
                depth: 0,
                orchestration: s.orchestration,
                waits_on: vec![],
                pending_results: 0,
                start_choices: vec![],
                halt: None,
                crew_paused: false,
                crew_tripped: false,
                crew_spend_trip: None,
                in_background: false,
                background: vec![],
                background_total: 0,
                at_depth_limit: false,
                handoff_from: None,
                limited: None,
                aside: Default::default(),
            },
            native_id: uuid::Uuid::new_v4().to_string(),
            native_started: false,
            scope: source.scope.clone(),
            fork_from: at_end.then(|| source.native_id.clone()),
            seed: (!at_end).then(|| seed_of(&events)),
            automatic_title: false,
            delivered_seq: 0,
            halt: None,
        };
        let turn_count = events
            .iter()
            .filter(|stored| matches!(stored.event, WbEvent::TurnEnd { .. }))
            .count();
        let notice = WbEvent::Notice {
            text: format!(
                "Forked from \"{}\" after {turn_count} {}. The files are as they were then, in a worktree of its own.",
                s.title,
                if turn_count == 1 { "turn" } else { "turns" }
            ),
        };
        let copied = events.iter().map(|stored| (stored.at, &stored.event));
        let sync_identity =
            sync_identity.filter(|identity| self.session_sync_identity(&source.scope).as_ref() == Some(identity));
        let saved =
            self.store()
                .insert_thread_with_identity(&record, copied.chain([(now, &notice)]), sync_identity.as_ref());
        // Not saved (a full disk): the worktree made for it goes too, so a failed fork leaves nothing behind.
        if let Err(error) = saved {
            if let Some(worktree) = record.summary.worktree.clone() {
                let repo = PathBuf::from(&project.path);
                let _ = tokio::task::spawn_blocking(move || {
                    git::discard_worktree(&repo, Path::new(&worktree.path), &worktree.branch)
                })
                .await;
            }
            return Err(error);
        }
        // The folder the fork starts from, for its first turn's diff.
        if let Some(dir) = self.workdir(&record) {
            let prefix = checkpoint_prefix(&id);
            let _ = tokio::task::spawn_blocking(move || git::checkpoint(&dir, &format!("{prefix}{:010}", 0))).await;
        }
        if self.emits_scope(&record.scope) {
            self.inner.host.emit(Emit::Thread(Box::new(record.summary.clone())));
        }
        Ok(record.summary)
    }

    /// Sends a fork's result back to the session it was forked from: its latest answer, a link to it and where its
    /// changes are ([`send_back::message`]), queued behind the source's running turn as the person's message, so it
    /// never interrupts that turn. Fails when the session is not a fork, its source is gone, or it has no answer yet.
    /// A side chat's "Bring back" is this too ([`send_back::side_chat_message`]).
    pub async fn send_back(&self, fork_id: &str) -> Result<()> {
        let fork = self.record(fork_id)?;
        if !self.in_scope(&fork.scope) {
            return Err("That session is not in this organization.".into());
        }
        let source_id = fork
            .summary
            .source_thread_id
            .clone()
            .ok_or_else(|| "This session is not a fork.".to_string())?;
        let source = self
            .reader()
            .thread(&source_id)?
            .filter(|source| self.in_scope(&source.scope))
            .ok_or_else(|| "The session this was forked from is gone.".to_string())?;
        let events = self.reader().events_after(fork_id, 0)?;
        // An answer from a reverted turn was taken back.
        let reverted = reverted_spans(&events);
        let kept: Vec<StoredEvent> = events
            .into_iter()
            .filter(|stored| {
                !reverted
                    .iter()
                    .any(|(after, end)| *after < stored.seq && stored.seq < *end)
            })
            .collect();
        let side_chat = fork.summary.side_chat;
        let answer = send_back::latest_answer(&kept, fork.summary.created_at).ok_or_else(|| {
            if side_chat {
                "This side chat has no answer to bring back yet.".to_string()
            } else {
                "This fork has no answer to send back yet.".to_string()
            }
        })?;
        let text = if side_chat {
            send_back::side_chat_message(&fork.summary.id, &fork.summary.title, answer)
        } else {
            send_back::message(
                &fork.summary.id,
                &fork.summary.title,
                fork.summary.worktree.as_ref(),
                answer,
            )
        };
        self.queue_message(&source.summary.id, &text, vec![]).await
    }

    /// The worktree the session's agent works in: its own, or a side chat's session's (side_chat.rs). None: its
    /// project's folder.
    fn worktree_of(&self, record: &ThreadRecord) -> Option<Worktree> {
        if !record.summary.side_chat {
            return record.summary.worktree.clone();
        }
        record
            .summary
            .source_thread_id
            .as_deref()
            .and_then(|id| self.store().thread(id).ok().flatten())
            .and_then(|session| session.summary.worktree)
    }

    /// Where the session's agent works: its worktree, else its project's folder. None when neither is there.
    fn workdir(&self, record: &ThreadRecord) -> Option<PathBuf> {
        match &self.worktree_of(record) {
            Some(worktree) if !worktree.removed => Some(PathBuf::from(&worktree.path)),
            Some(_) => None,
            None => self
                .store()
                .project(&record.summary.project_id)
                .ok()
                .flatten()
                .map(|project| PathBuf::from(project.path)),
        }
    }

    fn checkpoints(&self) -> MutexGuard<'_, HashMap<String, watch::Receiver<bool>>> {
        self.inner
            .checkpoints
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The session's last turn checkpoint while it is being written. Not one whose task is gone without writing it.
    fn pending_checkpoint(&self, thread_id: &str) -> Option<watch::Receiver<bool>> {
        self.checkpoints()
            .get(thread_id)
            .filter(|written| !*written.borrow() && written.has_changed().is_ok())
            .cloned()
    }

    /// Waits (up to [`CHECKPOINT_WAIT`]) until the session's last turn checkpoint is written.
    async fn checkpoint_written(&self, thread_id: &str) {
        if let Some(mut written) = self.pending_checkpoint(thread_id) {
            let _ = tokio::time::timeout(CHECKPOINT_WAIT, written.wait_for(|done| *done)).await;
        }
    }

    /// After a turn: what the folder looks like now, named by the turn's end event (off the runtime's workers), then
    /// `written`. The run's next turn waits for it (`send_lined_up`); `Out::Result` armed it (`LiveState::checkpoint`)
    /// when it took that turn's message.
    fn checkpoint_after(&self, thread_id: &str, seq: i64, written: watch::Sender<bool>) {
        // An archived session keeps no checkpoints (see `drop_checkpoints`), nor does a side chat (side_chat.rs).
        let Some(dir) = self
            .record(thread_id)
            .ok()
            .filter(|record| !record.summary.archived && !record.summary.side_chat)
            .and_then(|record| self.workdir(&record))
        else {
            let _ = written.send(true);
            return;
        };
        let refname = format!("{}{seq:010}", checkpoint_prefix(thread_id));
        self.checkpoints().insert(thread_id.to_string(), written.subscribe());
        tokio::task::spawn_blocking(move || {
            if git::in_repo(&dir)
                && let Err(error) = git::checkpoint(&dir, &refname)
            {
                eprintln!("nexus-workbench: checkpoint {refname}: {error}");
            }
            let _ = written.send(true);
        });
    }

    /// Commits what the session's worktree changed (as `title`), pushes its branch and opens its pull request with the
    /// GitHub CLI against the worktree's base, or finds the one already open (the push updated it). `path` finds `gh`.
    pub async fn open_pull_request(
        &self,
        thread_id: &str,
        title: &str,
        body: &str,
        path: &str,
    ) -> Result<ThreadSummary> {
        let record = self.record(thread_id)?;
        let worktree = record
            .summary
            .worktree
            .clone()
            .filter(|worktree| !worktree.removed)
            .ok_or_else(|| "Only a session with its own worktree opens a pull request.".to_string())?;
        if worktree.review.is_some() {
            return Err(REVIEW_NO_PUSH.into());
        }
        if self.is_live(thread_id) && record.summary.status != ThreadStatus::Idle {
            return Err("Wait for the agent to finish its turn.".into());
        }
        let title = title.trim().to_string();
        if title.is_empty() {
            return Err("Give the pull request a title.".into());
        }
        let (body, path) = (body.to_string(), path.to_string());
        let marked = self.inner.host.marks_commits().await;
        let url = tokio::task::spawn_blocking(move || {
            let dir = Path::new(&worktree.path);
            git::commit_all(dir, &title, marked)?;
            git::push(dir, &worktree.branch)?;
            let base = git::merge_base_branch(dir, &worktree.branch).unwrap_or_else(|| {
                worktree
                    .base
                    .split_once('/')
                    .map_or(worktree.base.clone(), |(_, name)| name.to_string())
            });
            git::pull_request(dir, &base, &title, &body, &path)
        })
        .await
        .map_err(|error| error.to_string())??;
        self.change(thread_id, |record| {
            if let Some(worktree) = record.summary.worktree.as_mut() {
                worktree.add_pull(&url);
            }
        })
    }

    /// The folder the session's agent works in: its worktree, else its project's folder.
    pub fn folder(&self, thread_id: &str) -> Result<PathBuf> {
        self.workdir(&self.record(thread_id)?)
            .ok_or_else(|| "This session's folder is gone.".to_string())
    }

    /// The folder the session's git panel works on: the session's folder, or (`repo`, a folder name the panel listed)
    /// a repository directly inside it.
    pub fn git_folder(&self, thread_id: &str, repo: Option<&str>) -> Result<PathBuf> {
        let dir = self.folder(thread_id)?;
        match repo {
            None => Ok(dir),
            Some(name) => child_repo(&dir, name),
        }
    }

    /// The session's folder as git sees it (or, `repo`, a repository directly inside it): its branch, the branches it
    /// can switch to, what isn't committed, and in a folder that isn't a repository, the repositories inside it.
    pub async fn git_state(&self, thread_id: &str, repo: Option<&str>) -> Result<GitState> {
        let record = self.record(thread_id)?;
        let dir = self.git_folder(thread_id, repo)?;
        let own = repo.is_none() && record.summary.worktree.is_some();
        tokio::task::spawn_blocking(move || read_git(&dir, own))
            .await
            .map_err(|error| error.to_string())
    }

    /// Commits everything that changed in the session's folder (or `repo` inside it) as `message` (the person's own git
    /// identity, so their hooks run), then pushes its branch when `push`. An empty message with `push` only pushes.
    /// Fails mid-turn.
    pub async fn commit(&self, thread_id: &str, repo: Option<&str>, message: &str, push: bool) -> Result<GitState> {
        let record = self.record(thread_id)?;
        let dir = self.git_folder(thread_id, repo)?;
        if self.is_live(thread_id) && record.summary.status != ThreadStatus::Idle {
            return Err("Wait for the agent to finish its turn.".into());
        }
        let message = message.trim().to_string();
        if message.is_empty() && !push {
            return Err("Write a commit message.".into());
        }
        let own = repo.is_none() && record.summary.worktree.is_some();
        if push && repo.is_none() && record.summary.worktree.as_ref().is_some_and(|w| w.review.is_some()) {
            return Err(REVIEW_NO_PUSH.into());
        }
        let marked = self.inner.host.marks_commits().await;
        tokio::task::spawn_blocking(move || {
            if !message.is_empty() && !git::commit_all(&dir, &message, marked)? && !push {
                return Err("Nothing to commit.".to_string());
            }
            if push {
                let branch = git::checked_out(&dir).ok_or_else(|| "Check out a branch to push.".to_string())?;
                git::push(&dir, &branch)?;
            }
            Ok(read_git(&dir, own))
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// What a draft of the commit message (`kind` Commit: the session's folder, or `repo` inside it) or of the pull
    /// request's title and description (the session's own worktree) is written from: the change, the repository's
    /// recent subjects and its pull request template. An error when there is nothing to describe.
    pub async fn draft_context(
        &self,
        thread_id: &str,
        repo: Option<&str>,
        kind: draft::Kind,
    ) -> Result<draft::Context> {
        let record = self.record(thread_id)?;
        let dir = match kind {
            draft::Kind::Commit => self.git_folder(thread_id, repo)?,
            draft::Kind::PullRequest => {
                let worktree = record
                    .summary
                    .worktree
                    .as_ref()
                    .filter(|worktree| !worktree.removed)
                    .ok_or_else(|| "Only a session with its own worktree opens a pull request.".to_string())?;
                PathBuf::from(&worktree.path)
            }
        };
        // What the worktree was made from, for a pull request whose base was never recorded (`open_pull_request`).
        let made_from = record.summary.worktree.as_ref().map(|worktree| worktree.base.clone());
        tokio::task::spawn_blocking(move || draft_context_in(&dir, kind, made_from))
            .await
            .map_err(|error| error.to_string())?
    }

    /// Checks out another local branch in the session's project folder (or `repo` inside it). A session with its own
    /// worktree keeps its branch, and no agent in that folder may be mid-turn; git refuses, changing nothing, to
    /// overwrite uncommitted work.
    pub async fn switch_branch(&self, thread_id: &str, repo: Option<&str>, name: &str) -> Result<GitState> {
        let record = self.record(thread_id)?;
        if repo.is_none() && record.summary.worktree.is_some() {
            return Err("A session with its own worktree keeps its branch.".into());
        }
        let dir = self.git_folder(thread_id, repo)?;
        // Mid-turn agents in this project, or in another project whose folder holds this one or sits inside it (a
        // repository that is also its own project), would see their files change under them.
        let busy = self.threads()?.iter().any(|thread| {
            thread.worktree.is_none()
                && self.is_live(&thread.id)
                && thread.status != ThreadStatus::Idle
                && (thread.project_id == record.summary.project_id
                    || self
                        .record(&thread.id)
                        .ok()
                        .and_then(|other| self.workdir(&other))
                        .is_some_and(|folder| overlaps(&folder, &dir)))
        });
        if busy {
            return Err("Wait for the agents in this folder to finish their turns.".into());
        }
        let name = name.to_string();
        let state = tokio::task::spawn_blocking(move || {
            git::switch(&dir, &name)?;
            Ok::<_, String>(read_git(&dir, false))
        })
        .await
        .map_err(|error| error.to_string())??;
        // The sidebar and the top bar show the project's branch.
        self.inner.host.emit(Emit::Projects(self.projects()?));
        Ok(state)
    }

    /// The turns whose changes can be shown: their end events' numbers, oldest first.
    pub async fn changed_turns(&self, thread_id: &str) -> Result<Vec<i64>> {
        let Some(dir) = self.workdir(&self.record(thread_id)?) else {
            return Ok(vec![]);
        };
        let thread_id = thread_id.to_string();
        tokio::task::spawn_blocking(move || changes_source::changed_turns(&dir, &thread_id))
            .await
            .map_err(|error| error.to_string())
    }

    /// What the session changed: in the turn that ended at event `turn`, or (None) everything, against the worktree's
    /// base or else the folder before the first turn.
    pub async fn changes(&self, thread_id: &str, turn: Option<i64>) -> Result<Changes> {
        // A turn's end shows before its checkpoint is written; a read at that moment (the crew's compare card) waits.
        self.checkpoint_written(thread_id).await;
        let record = self.record(thread_id)?;
        let dir = self
            .workdir(&record)
            .ok_or_else(|| "This session's folder is gone.".to_string())?;
        let base = record.summary.worktree.map(|worktree| worktree.base);
        let thread_id = thread_id.to_string();
        tokio::task::spawn_blocking(move || {
            let (diff, truncated) = changes_source::session_diff(&dir, &thread_id, base.as_deref(), turn)?;
            Ok(Changes { diff, truncated })
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// The files the turn that ended at event `turn` changed, with its lines added and removed (`turn_files.rs`).
    pub async fn turn_files(&self, thread_id: &str, turn: i64) -> Result<Vec<turn_files::TurnFile>> {
        self.checkpoint_written(thread_id).await;
        let record = self.record(thread_id)?;
        let dir = self
            .workdir(&record)
            .ok_or_else(|| "This session's folder is gone.".to_string())?;
        let thread_id = thread_id.to_string();
        tokio::task::spawn_blocking(move || turn_files::turn_files(&dir, &thread_id, turn))
            .await
            .map_err(|error| error.to_string())?
    }

    /// How many files [`Workbench::revert_to_turn`] would change, for the person to confirm.
    /// Waits for the turn's checkpoint ref to finish writing before validating it.
    pub async fn revert_preview(&self, thread_id: &str, turn: i64) -> Result<u32> {
        self.checkpoint_written(thread_id).await;
        let (dir, target) = self.revert_target(thread_id, turn)?;
        let plan = tokio::task::spawn_blocking(move || git::revert_plan(&dir, &target))
            .await
            .map_err(|error| error.to_string())??;
        Ok(plan.len() as u32)
    }

    /// Puts the session's files back as they were when the turn ending at event `turn` ended (its checkpoint), and
    /// logs a `revert`: the turns after it are reverted. The agent forgets them too, at its next start: Claude Code
    /// resumes its conversation only up to that turn (`--resume-session-at`), Codex drops the later turns
    /// (`thread/revert`). Where the log holds no such point (turns from before this existed), the next message tells
    /// the agent instead. Refused while a turn runs or the agent has background work. A live agent between turns is
    /// stopped first, so the next message starts it again from the reverted point. Returns how many files changed.
    pub async fn revert_to_turn(&self, thread_id: &str, turn: i64) -> Result<u32> {
        // A turn's end reaches the log before its checkpoint ref exists. Validate after that write, including busy state.
        self.checkpoint_written(thread_id).await;
        let busy = |this: &Workbench| -> Result<bool> {
            let live_busy = this.live(thread_id).is_some_and(|live| {
                let state = live.state();
                !state.turns.is_empty() || !state.background.is_empty()
            });
            Ok(live_busy || this.record(thread_id)?.summary.status.busy())
        };
        if busy(self)? {
            return Err(
                "The agent, its sub-agents or its commands are still working. Stop them or wait for them to finish, then revert."
                    .into(),
            );
        }
        self.refuse_revert_with_crew(thread_id)?;
        let (dir, target) = self.revert_target(thread_id, turn)?;
        // Until the revert is logged, a message to the session is refused: it would start the agent while its files
        // are rewritten, and before the log asks it to rewind.
        let _reverting = Reverting::hold(self, thread_id)?;
        if let Some(live) = self.live(thread_id) {
            self.stop_live(&live, StopReason::Stop, None);
        }
        self.settle(thread_id).await?;
        // The last turn's checkpoint, if still being written, must not snapshot the reverted files (R5).
        self.checkpoint_written(thread_id).await;
        // A message may have started a turn while the agent stopped.
        if busy(self)? {
            return Err("The agent started working again. Wait for the turn to end, then revert.".into());
        }
        let files = tokio::task::spawn_blocking(move || git::revert_files(&dir, &target))
            .await
            .map_err(|error| error.to_string())??;
        let files = files as u32;
        let logged = self.append(thread_id, WbEvent::Revert { turn, files })?;
        // The agent forgets the results it read in the reverted turns: they are pending again.
        if let Err(error) = self.reset_cursors(thread_id) {
            eprintln!("nexus-workbench: {thread_id}: {error}");
        }
        // The folder as the revert left it: the next turn's changes are counted from here, not from the turn before it.
        // Named apart from turn ends (a letter after the number), so it is never listed as a turn of its own.
        if let Some(dir) = self.workdir(&self.record(thread_id)?) {
            let refname = format!("{}{:010}r", checkpoint_prefix(thread_id), logged.seq);
            let _ = tokio::task::spawn_blocking(move || git::checkpoint(&dir, &refname)).await;
        }
        Ok(files)
    }

    /// The session's folder and the checkpoint of the turn ending at event `turn`, if it can be reverted to: a turn
    /// that ended, was not reverted itself, and has its files recorded.
    fn revert_target(&self, thread_id: &str, turn: i64) -> Result<(PathBuf, String)> {
        let record = self.record(thread_id)?;
        if record
            .summary
            .worktree
            .as_ref()
            .is_some_and(|worktree| worktree.removed)
        {
            return Err("This session's worktree was removed, so its files can't be reverted.".into());
        }
        let events = self.reader().events_after(thread_id, 0)?;
        let (kept, _) = conversation(&events, record.summary.agent);
        if !kept.iter().any(|(seq, _)| *seq == turn) {
            return Err(
                "That turn can't be reverted to: it isn't a finished turn of this session, or it was reverted.".into(),
            );
        }
        let dir = self
            .workdir(&record)
            .ok_or_else(|| "This session's folder is gone.".to_string())?;
        let target = format!("{}{turn:010}", checkpoint_prefix(thread_id));
        if !git::refs(&dir, &target).is_ok_and(|names| names.contains(&target)) {
            return Err("That turn's files weren't recorded, so it can't be reverted to.".into());
        }
        Ok((dir, target))
    }

    /// Sends a message. A follow-up reaches the running agent at its next step.
    pub async fn send(&self, thread_id: &str, text: &str) -> Result<()> {
        self.send_message(thread_id, text, vec![]).await
    }

    /// [`Workbench::send`] with images, which go before the text: the person's message.
    pub async fn send_message(&self, thread_id: &str, text: &str, images: Vec<Image>) -> Result<()> {
        self.send_with(
            thread_id,
            text,
            images,
            None,
            true,
            SendContext {
                typed_by_person: true,
                ..Default::default()
            },
        )
        .await
    }

    /// Schedules a separate turn after the current one, retaining queue editing and cancellation.
    pub async fn queue(&self, thread_id: &str, text: &str) -> Result<()> {
        self.queue_message(thread_id, text, vec![]).await
    }

    /// [`Workbench::queue`] with images.
    pub async fn queue_message(&self, thread_id: &str, text: &str, images: Vec<Image>) -> Result<()> {
        self.send_with(
            thread_id,
            text,
            images,
            None,
            true,
            SendContext {
                queued: true,
                ..Default::default()
            },
        )
        .await
    }

    /// [`Workbench::send_message`] with `context` said to the agent after the text, but not logged: what the sessions
    /// and pull requests the message mentions are ([`mentions::block`]).
    pub async fn send_message_with(
        &self,
        thread_id: &str,
        text: &str,
        images: Vec<Image>,
        context: &str,
    ) -> Result<()> {
        self.send_message_as(thread_id, text, images, context, false).await
    }

    async fn send_creation_message(
        &self,
        thread_id: &str,
        text: &str,
        images: Vec<Image>,
        context: &str,
        admitted_execution: Option<&AdmittedExecution>,
    ) -> Result<()> {
        self.send_with(
            thread_id,
            text,
            images,
            None,
            true,
            SendContext {
                mentions: context,
                creating: true,
                typed_by_person: true,
                admitted_execution,
                ..Default::default()
            },
        )
        .await
    }

    /// [`Workbench::send_message_with`], as the person asked: `queued` (Ctrl+Enter) waits for the running turn to end
    /// and stays editable and cancellable ([`Workbench::queue_message`]); otherwise it joins the turn under way at the
    /// agent's next step. While no turn runs, both start one.
    pub async fn send_message_as(
        &self,
        thread_id: &str,
        text: &str,
        images: Vec<Image>,
        context: &str,
        queued: bool,
    ) -> Result<()> {
        self.send_with(
            thread_id,
            text,
            images,
            None,
            true,
            SendContext {
                mentions: context,
                queued,
                typed_by_person: true,
                ..Default::default()
            },
        )
        .await
    }

    /// [`Workbench::send_message`] written by `from`: None for the person, else the linked session that wrote it. Its
    /// `User` row carries `from`, and only the person's message to a tree's root starts the tree's counters again.
    pub async fn send_from(&self, thread_id: &str, text: &str, images: Vec<Image>, from: Option<Sender>) -> Result<()> {
        let person = from.is_none();
        self.send_with(
            thread_id,
            text,
            images,
            from,
            person,
            SendContext {
                typed_by_person: person,
                ..Default::default()
            },
        )
        .await
    }

    /// [`Workbench::send_from`], with `lead` said to the agent before the text and `context` after it, neither logged
    /// with it (`lead`: who wrote a linked session's message; `context`: what the sessions and pull requests the
    /// message mentions are, [`mentions::block`]). The person's message to a session whose children have results it
    /// has not read carries them in front, logged as a row of their own just before it, unless it is a slash command
    /// ([`linked::results`]).
    ///
    /// `person`: the person sends it now, so it clears halts, starts a root's counters again and runs as their turn.
    /// A crew member's first prompt is the person's words (`from` None) sent for them: it runs as a linked session's
    /// turn, which a halt refuses and the crew's spend cap interrupts.
    pub(crate) async fn send_with(
        &self,
        thread_id: &str,
        text: &str,
        images: Vec<Image>,
        from: Option<Sender>,
        person: bool,
        context: SendContext<'_>,
    ) -> Result<()> {
        let SendContext {
            lead,
            mentions: context,
            queued,
            creating,
            typed_by_person,
            admitted_execution,
        } = context;
        let _update_activity = UpdateActivity::begin(self)?;
        let text = text.trim_end();
        if text.trim().is_empty() && images.is_empty() {
            return Ok(());
        }
        check_images(&images)?;
        self.check_saved_provider_launch(thread_id)?;
        self.clear_limit_stop(thread_id)?;
        // A discovered command stays bound to this process. Never launch or rotate a process to run a stale name.
        if from.is_none()
            && let Some(name) = commands::native_name(text)
        {
            Gated::gate(self).check_saved_provider_start(thread_id, creating)?;
            return self.send_native_command(thread_id, name, text, images);
        }
        // Until `start` has handed the message over, a revert waits its turn (it is refused), and the other way round.
        // R2 is covered jointly: `Sending` drops when this returns, which can be before a deferred `send_after`
        // delivers. But `begin_turn` pushes into `LiveState::turns` synchronously, and `revert_to_turn`'s `busy()`
        // check (before and after `settle`) reads that same queue, so a revert can't slip into the gap.
        let _sending = Sending::hold(self, thread_id, creating)?;
        // Its Claude account reached its limits, or agy's launch arguments changed: the agent stops here, between turns.
        // The message then resumes the conversation in a new process (`start`). That process runs on the account with
        // the most left, or is an agy started with the arguments as they are now.
        if let Some(live) = self.live(thread_id) {
            let summary = self.record(thread_id)?.summary;
            self.check_before_move(&summary).await;
            if self.must_move(&summary, &live) || self.antigravity_args_changed(&summary, &live) {
                self.stop_live(&live, StopReason::Move, None);
            }
        }
        self.settle(thread_id).await?;
        let record = self.record(thread_id)?;
        if self
            .worktree_of(&record)
            .as_ref()
            .is_some_and(|worktree| worktree.removed)
        {
            return Err("This session's worktree was removed, so it can't run any more.".into());
        }
        if record.summary.parent_thread_id.is_some() && record.summary.worktree.is_none() {
            return Err(linked::NO_WORKTREE.into());
        }
        // A message that would start an agent in a project no longer open fails before it is logged, so the log never
        // shows a message nothing read. A running agent still takes it (`start`).
        if !self.is_live(thread_id) && self.store().project(&record.summary.project_id)?.is_none() {
            return Err("This session's project is no longer open.".into());
        }
        // A slash command goes alone: Claude Code runs it only as the whole message. The results go after its turn.
        let batch = if person && !text.trim_start().starts_with('/') {
            self.results_for_person(&record)?
        } else {
            None
        };
        let mut lead = lead.unwrap_or_default();
        if let Some(batch) = &batch {
            lead = format!("{}\n\n{lead}", batch.text);
        }
        // A fork from an earlier point: its agent starts a new conversation, told first what was said up to there.
        let said = match &record.seed {
            Some(seed) if !record.native_started && !self.is_live(thread_id) => format!("{seed}{lead}{text}{context}"),
            _ => format!("{lead}{text}{context}"),
        };
        let say = if person && !queued {
            Say::Steer {
                text: said,
                images: images.clone(),
                person: true,
            }
        } else {
            Say::User {
                text: said,
                images: images.clone(),
            }
        };
        if let Some(batch) = &batch {
            self.append(
                thread_id,
                WbEvent::User {
                    text: batch.text.clone(),
                    images: vec![],
                    from: Some(batch.from.clone()),
                },
            )?;
        }
        let user = self.append(
            thread_id,
            WbEvent::User {
                text: text.into(),
                images,
                from,
            },
        )?;
        if let Some(batch) = &batch {
            batch.logged(user.seq);
        }
        let mut record = record;
        if person {
            record.summary.acted_at = self.person_acted(thread_id)?.acted_at;
            self.person_wrote(&record);
            self.title_side_chat(&record, text)?;
        }
        if typed_by_person {
            self.inner.terminals.reset_watch_budget(thread_id);
        }
        if record.summary.archived {
            self.change(thread_id, |record| record.summary.archived = false)?;
        }
        // The worktree's setup script runs first (`setup`); the session may have changed while it ran.
        // Messages that waited leave in the order they came: each keeps its place until `start` has handed it over.
        let (record, _in_line) = match self.wait_for_setup(thread_id).await {
            Some(place) => (self.record(thread_id)?, Some(place)),
            None => (record, None),
        };
        // Before the agent touches anything: what the folder looked like, for the first turn's diff. A session whose
        // agent ran before and has no refs was archived (that drops them): its folder now is a base for the next diff,
        // named apart (a letter after the number), and is never "before the first turn", which a fork would take for
        // the session's start (a private formal model R6). A side chat keeps none (side_chat.rs).
        if !record.summary.side_chat
            && let Some(dir) = self.workdir(&record)
        {
            let prefix = checkpoint_prefix(thread_id);
            let refname = if record.native_started {
                format!("{prefix}{:010}b", user.seq)
            } else {
                format!("{prefix}{:010}", 0)
            };
            let _ = tokio::task::spawn_blocking(move || {
                // In a repository at all: a folder below its top, or a detached HEAD, has turns to show too.
                if git::in_repo(&dir) && git::refs(&dir, &prefix).is_ok_and(|names| names.is_empty()) {
                    let _ = git::checkpoint(&dir, &refname);
                }
            })
            .await;
        }
        let started = self.start(record, say, user.seq, person, admitted_execution).await;
        if started.is_ok()
            && let Some(batch) = batch
        {
            batch.keep();
        }
        started
    }

    /// Waits until the session's agent, if it is stopping or ending, has left: a message must reach a fresh agent,
    /// not the closed input of one on its way out, nor start one while the old run still writes its last events.
    async fn settle(&self, thread_id: &str) -> Result<()> {
        let Some(old) = self.live(thread_id).filter(|live| {
            let state = live.state();
            state.stop.is_some() || state.done
        }) else {
            return Ok(());
        };
        let deadline = Instant::now() + process::GRACE + process::TERM_GRACE + Duration::from_secs(1);
        while self.live(thread_id).is_some_and(|live| Arc::ptr_eq(&live, &old)) {
            if Instant::now() >= deadline {
                return Err("The agent is still stopping. Send the message again in a moment.".into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }

    /// Takes `live` out of the session's live agent, if it is still the one there.
    fn unregister(&self, thread_id: &str, live: &Arc<Live>) -> bool {
        let mut lives = self.inner.live.lock().unwrap_or_else(|p| p.into_inner());
        if lives.get(thread_id).is_some_and(|current| Arc::ptr_eq(current, live)) {
            lives.remove(thread_id);
            true
        } else {
            false
        }
    }

    /// Says `say` to the session's agent, and takes in what that means at once.
    fn say(&self, thread_id: &str, live: &Live, say: Say) -> Result<()> {
        self.outs(thread_id, live, live.tell(say))
    }

    fn outs(&self, thread_id: &str, live: &Live, outs: Vec<Out>) -> Result<()> {
        let handled = outs.into_iter().try_for_each(|out| self.handle(thread_id, live, out));
        self.notice_turn_begun(thread_id, live);
        handled
    }

    /// A notice's reserved turn began (`LiveState::notice_due`). It has no `User` event, so it runs as a turn the agent
    /// starts itself: a halt or a spend trip interrupts it at once (`own_turn_begun`). Called once the agent's lines that
    /// began it are handled, so the interrupt goes after the line that starts the turn.
    fn notice_turn_begun(&self, thread_id: &str, live: &Live) {
        let due = live.state().notice_due.take();
        if let Some(id) = due
            && live.state().turns.front().is_some_and(|turn| turn.id == id)
        {
            self.own_turn_begun(thread_id);
        }
    }

    /// Writes the status this run's state calls for (`live_status`), under the run's status lock. A run whose
    /// clean-up began is left alone: `finish` writes its final status.
    fn refresh_status(&self, thread_id: &str, live: &Live) -> Result<()> {
        let _status = live.status_lock();
        let status = live_status(&live.state());
        if let Some(status) = status {
            self.set_status(thread_id, status)?;
        }
        Ok(())
    }

    /// Records a new turn, for `message`: what to say and its `User` event's number (none: a turn the agent started
    /// itself). The first starts now. A person's follow-up joins the active turn at the next agent step.
    /// Explicitly queued messages wait for the previous turn and support editing, cancellation and "Send now".
    /// False, recording nothing, when the run is
    /// ending, or (for the person's message, `user`) stopping: the message waits for the next run instead.
    /// `person`: the message is the person's (`Turn::person`). A linked session's message or a result to a halted
    /// session is refused, recording nothing; the turn of the person's own message clears the halt when it begins
    /// ([`linked::halts`]).
    fn begin_turn(
        &self,
        thread_id: &str,
        live: &Live,
        user: bool,
        message: Option<(Say, i64)>,
        person: bool,
    ) -> Result<bool> {
        let (say, seq) = message.unzip();
        self.begin_turn_if(thread_id, live, user, say, seq, person, false, false)
    }

    /// [`Workbench::begin_turn`] for a message that never waits behind another (a linked session's results): false,
    /// recording nothing, unless the run is idle (`LiveState::idle`), checked in the same hold that records the turn.
    fn begin_idle_turn(&self, thread_id: &str, live: &Live, say: Say, seq: i64) -> Result<bool> {
        self.begin_turn_if(thread_id, live, true, Some(say), Some(seq), false, true, false)
    }

    /// The turn a refused steer runs as (`Out::TurnQueued`), the sender's: the person's, or Nexus's notice
    /// (`Turn::notice`).
    fn reserve_turn(&self, thread_id: &str, live: &Live, person: bool) -> Result<bool> {
        self.begin_turn_if(thread_id, live, false, None, None, person, false, !person)
    }

    /// [`Workbench::begin_turn`], where `say` may have no `User` event (`seq`: none), as a compaction has none.
    ///
    /// The first turn's `say` goes in line for the agent (`LiveState::line_up`) in the same hold of the lock that
    /// records the turn. A follow-up sent from then on sees the turn, joins it and lines up behind `say`, so it never
    /// reaches the agent before the message that started the turn.
    #[allow(clippy::too_many_arguments)]
    fn begin_turn_if(
        &self,
        thread_id: &str,
        live: &Live,
        user: bool,
        say: Option<Say>,
        seq: Option<i64>,
        person: bool,
        idle: bool,
        notice: bool,
    ) -> Result<bool> {
        let turn_id = format!("turn_{}", short_id());
        // Nexus's notice (`steer_notice`): logged as a `Notice` once a turn took it, before the agent gets it. A refused
        // one leaves no row, and the agent's first events come after it.
        let notice_text = match (&say, person, seq) {
            (
                Some(Say::Steer {
                    text, person: false, ..
                }),
                false,
                None,
            ) => Some(text.clone()),
            _ => None,
        };
        // A failed write costs only the row: the agent still gets the notice, so the caller must not send it again.
        let log_notice = || {
            if let Some(text) = &notice_text
                && let Err(error) = self.append(thread_id, WbEvent::Notice { text: text.clone() })
            {
                eprintln!("nexus-workbench: {thread_id}: {error}");
            }
        };
        let (first, first_in_line, steering) = {
            let mut state = live.state();
            if self.inner.update_restart.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(false);
            }
            if state.done || (user && state.stop.is_some()) || (idle && !state.idle()) {
                return Ok(false);
            }
            // A linked session's message or a result to a halted session: refused under the lock Interrupt writes its
            // halt under ([`linked::halts`]).
            if user
                && !person
                && say.is_some()
                && let Some(refusal) = self.message_refusal(thread_id)
            {
                return Err(refusal.into());
            }
            state.last_activity = Instant::now();
            // The turn Claude Code owes after a background sub-agent ended is one it starts itself; a message's turn
            // takes in whatever it was to answer. A compaction has no `User` event: it is not a message. Nexus's notice
            // (`steer_notice`) has none either, but it is one.
            let message = seq.is_some() || matches!(say, Some(Say::Steer { .. }));
            state.owed = if message { 0 } else { state.owed.saturating_sub(1) };
            let first = state.turns.is_empty();
            if first && person && seq.is_some() {
                self.person_turn_begins(thread_id);
            }
            match (first, person, seq, say) {
                (false, true, Some(seq), Some(Say::Steer { text, images, .. })) => {
                    self.person_turn_begins(thread_id);
                    if let Some(turn) = state.turns.front_mut() {
                        turn.person = true;
                    }
                    state.steering.insert(seq);
                    let first_in_line = state.line_up(
                        Some(seq),
                        Say::Steer {
                            text,
                            images,
                            person: true,
                        },
                    );
                    (false, false, Some((seq, first_in_line)))
                }
                // Nexus's notice joins the turn under way as the person's message does, but leaves the turn and the
                // halt as they are. With no `User` event, `Out::Taken` finds nothing of it in `unread`: a notice that
                // lands after the turn's last step runs as a turn the agent starts itself (`begin_own_turn`), or as
                // the one a refused steer reserves (`Out::TurnQueued`, not the person's).
                (false, false, None, Some(say @ Say::Steer { .. })) => {
                    let first_in_line = state.line_up(None, say);
                    (false, first_in_line, None)
                }
                (_, _, seq, say) => {
                    let say = say.map(|say| match say {
                        Say::Steer { text, images, .. } => Say::User { text, images },
                        say => say,
                    });
                    let (now, held) = if first { (say, None) } else { (None, say) };
                    state.turns.push_back(Turn {
                        id: turn_id.clone(),
                        started: Instant::now(),
                        stopping: false,
                        held,
                        seq,
                        person,
                        notice,
                    });
                    if first && notice {
                        state.notice_due = Some(turn_id.clone());
                    }
                    // In line before the lock is released: a follow-up that sees this turn goes after it.
                    let first_in_line = now.is_some_and(|say| state.line_up(seq, say));
                    (first, first_in_line, None)
                }
            }
        };
        if let Some((seq, first_in_line)) = steering {
            let logged = self.append(thread_id, WbEvent::QueueSteer { seq });
            let sent = if first_in_line {
                self.send_lined_up(thread_id, live)
            } else {
                Ok(())
            };
            logged?;
            sent?;
        }
        if first {
            // Sent even when the log write fails: left at the front of the line, it would hold every later message
            // back.
            let started = self
                .append(thread_id, WbEvent::TurnStart { turn_id })
                .and_then(|_| self.refresh_status(thread_id, live));
            log_notice();
            let sent = if first_in_line {
                self.send_lined_up(thread_id, live)
            } else {
                Ok(())
            };
            started?;
            sent?;
        } else {
            log_notice();
            if first_in_line {
                self.send_lined_up(thread_id, live)?;
            }
        }
        Ok(true)
    }

    /// A lead's first streamed text, usage or turn id starts its spontaneous turn, before its final message arrives.
    fn begin_own_turn(&self, thread_id: &str, live: &Live) -> Result<()> {
        if live.state().turns.is_empty() && self.begin_turn(thread_id, live, false, None, false)? {
            self.own_turn_begun(thread_id);
        }
        Ok(())
    }

    /// Gives the agent `say` (the message numbered `seq`), lined up behind the messages already on their way
    /// (`LiveState::line_up`), so it never reaches the agent before one that left the queue first. A turn's opening
    /// message does not come here: [`Workbench::begin_turn_if`] lines it up under the lock that records the turn.
    // No caller on main yet: a follow-up will send a session's new name through it.
    #[allow(dead_code)]
    fn release(&self, thread_id: &str, live: &Live, seq: Option<i64>, say: Say) -> Result<()> {
        let first = live.state().line_up(seq, say);
        if first {
            self.send_lined_up(thread_id, live)?;
        }
        Ok(())
    }

    /// Sends the messages lined up for the agent (`LiveState::unsent`), in order. Each waits for the last turn's
    /// checkpoint, so a turn's first edits stay out of the turn before's diff: while one is being written, a task
    /// waits for it and sends the rest. A run that ended or began stopping meanwhile gets nothing more: what is still
    /// in line stays there, and `finish` names it as never read (a held turn's message is the turn under way for
    /// `finish`, which ends it as interrupted). An interrupt that came while a message waited goes out right after it.
    fn send_lined_up(&self, thread_id: &str, live: &Live) -> Result<()> {
        let mut result = Ok(());
        loop {
            let say = {
                let mut state = live.state();
                if state.done || state.stop.is_some() {
                    state.releasing = false;
                    state.release_queued = false;
                    return result;
                }
                if let Some(checkpoint) = state.pending_checkpoint() {
                    state.releasing = true;
                    drop(state);
                    self.send_after(thread_id, live, checkpoint);
                    return result;
                }
                state.releasing = false;
                state.release_queued = false;
                let Some((seq, mut say)) = state.unsent.front().cloned() else {
                    return result;
                };
                // Noted before it goes out, so an echo that comes back at once finds it.
                if let (Some(seq), Say::User { text, .. } | Say::Steer { text, .. }) = (seq, &mut say) {
                    if state.spend_notice_pending
                        && !state
                            .interrupt_after_release
                            .as_ref()
                            .is_some_and(|source| source.applies_to(state.turns.front()))
                        && !text.trim_start().starts_with('/')
                    {
                        *text = format!("{}\n\n{text}", linked::spend::STOP_NOTICE);
                        state.spend_notice_pending = false;
                    }
                    state.unread.push_back((seq, text.clone()));
                }
                say
            };
            result = result.and(self.say(thread_id, live, say));
            // The person stopped the turn while it waited: the agent has it now, so the interrupt can land.
            let interrupt = live.state().interrupt_after_release.take();
            if let Some(source) = interrupt {
                source.interrupt(live);
            }
            // Taken off only now, so a message lined up meanwhile goes after it rather than first.
            let mut state = live.state();
            state.unsent.pop_front();
            if state.unsent.is_empty() {
                return result;
            }
        }
    }

    /// [`Workbench::send_lined_up`] from a task, once `checkpoint` is written.
    fn send_after(&self, thread_id: &str, live: &Live, mut checkpoint: watch::Receiver<bool>) {
        let Some(live) = self
            .live(thread_id)
            .filter(|current| std::ptr::eq(Arc::as_ptr(current), live))
        else {
            return;
        };
        let (this, thread_id) = (self.clone(), thread_id.to_string());
        tokio::spawn(async move {
            // A dropped sender (the checkpoint task panicked) also ends the wait.
            let _ = checkpoint.wait_for(|written| *written).await;
            if let Err(error) = this.send_lined_up(&thread_id, &live) {
                eprintln!("nexus-workbench: {thread_id}: {error}");
            }
        });
    }

    /// The queued message numbered `seq` (its `User` event) in `state`, while the agent has not been given it.
    fn waiting(state: &mut LiveState, seq: i64) -> Result<usize> {
        if state.done {
            return Err("The agent has stopped, so that message won't be sent.".into());
        }
        state
            .turns
            .iter()
            .position(|turn| turn.seq == Some(seq) && turn.held.is_some())
            .ok_or_else(|| "That message is no longer waiting: the agent already has it.".into())
    }

    /// Takes the queued message numbered `seq` out of the queue, before the agent gets it.
    pub fn queue_cancel(&self, thread_id: &str, seq: i64) -> Result<()> {
        let live = self.live(thread_id).ok_or("That message is no longer waiting.")?;
        {
            let mut state = live.state();
            let at = Self::waiting(&mut state, seq)?;
            state.turns.remove(at);
        }
        self.release_message(thread_id, seq);
        self.append(thread_id, WbEvent::QueueCancel { seq })?;
        Ok(())
    }

    /// Rewrites the queued message numbered `seq` before the agent gets it. Its images stay.
    pub fn queue_edit(&self, thread_id: &str, seq: i64, text: &str) -> Result<()> {
        let text = text.trim_end();
        if commands::native_name(text).is_some() {
            return Err("Run slash commands from the composer after the current turn finishes.".into());
        }
        let live = self.live(thread_id).ok_or("That message is no longer waiting.")?;
        {
            let mut state = live.state();
            let at = Self::waiting(&mut state, seq)?;
            if let Some(Say::User { text: said, images }) = &mut state.turns[at].held {
                if text.trim().is_empty() && images.is_empty() {
                    return Err("The message is empty. Remove it from the queue instead.".into());
                }
                *said = text.to_string();
            }
        }
        // The rewritten text no longer holds the results said in front of it.
        self.release_message(thread_id, seq);
        self.append(thread_id, WbEvent::QueueEdit { seq, text: text.into() })?;
        Ok(())
    }

    /// Moves the queued message numbered `seq` to just before the queued message `before`, or to the end of the queue
    /// when `before` is None. Both must still be waiting: the agent has neither yet.
    pub fn queue_move(&self, thread_id: &str, seq: i64, before: Option<i64>) -> Result<()> {
        if before == Some(seq) {
            return Ok(());
        }
        let live = self.live(thread_id).ok_or("That message is no longer waiting.")?;
        {
            let mut state = live.state();
            let from = Self::waiting(&mut state, seq)?;
            if let Some(before) = before {
                Self::waiting(&mut state, before)?;
            }
            let Some(turn) = state.turns.remove(from) else {
                return Ok(());
            };
            match before.and_then(|before| state.turns.iter().position(|turn| turn.seq == Some(before))) {
                Some(to) => state.turns.insert(to, turn),
                None => state.turns.push_back(turn),
            }
        }
        self.append(thread_id, WbEvent::QueueMove { seq, before })?;
        Ok(())
    }

    /// Sends the queued message numbered `seq` into the turn under way now, rather than after it ("Send now"): Claude
    /// Code takes it in at its next step, Codex as a steer (`turn/steer`). When the turn has no step left, the agent
    /// runs it as its next turn, ahead of the rest of the queue.
    ///
    /// A steer joins the active turn without another placeholder. Codex reserves a fallback turn after refusal.
    /// Claude Code can start a separate turn when no step remains.
    pub fn queue_steer(&self, thread_id: &str, seq: i64) -> Result<()> {
        let live = self.live(thread_id).ok_or("That message is no longer waiting.")?;
        // Out of the queue and in line in one hold of the lock: a turn end can't slip the next message in between.
        let first = {
            let mut state = live.state();
            if state.stop.is_some() {
                return Err("The agent is stopping, so that message can't go now.".into());
            }
            let at = Self::waiting(&mut state, seq)?;
            let Some(Say::User { text, images }) = state.turns.remove(at).and_then(|turn| turn.held) else {
                return Ok(());
            };
            state.steering.insert(seq);
            state.line_up(
                Some(seq),
                Say::Steer {
                    text,
                    images,
                    person: true,
                },
            )
        };
        // Sent even when the log write fails: left at the front of the line, it would hold every later message back.
        let logged = self.append(thread_id, WbEvent::QueueSteer { seq });
        let sent = if first {
            self.send_lined_up(thread_id, &live)
        } else {
            Ok(())
        };
        logged?;
        sent
    }

    /// Says `text` to the session's running agent as Nexus's own notice, not the person's message. It joins the turn
    /// under way where the agent takes a message mid-turn (Claude Code at its next step, Codex as a steer); the other
    /// agents get it when the turn ends. While no turn runs, it starts one. It is logged as a `Notice`, so the session
    /// shows it as Nexus's. Its turn is never the person's: it clears no halt, and a halted session refuses it, as it
    /// refuses a linked session's message (`message_refusal`). An agent that is not running is not started for it.
    ///
    /// A notice the run ends before the agent reads it is dropped: `finish` counts only messages with a `User` event.
    /// Its `Notice` row stays. Readers find some notices by their text, so `text` must not match them: a
    /// `crew_fold::PROCESS_ENDED` text, the `moving::MOVED` prefix, or "compacted the conversation" (`context_breakdown`).
    pub fn steer_notice(&self, thread_id: &str, text: &str) -> Result<()> {
        let text = text.trim_end();
        if text.trim().is_empty() {
            return Ok(());
        }
        let live = self
            .live(thread_id)
            .ok_or("The agent is not running, so the notice can't reach it.")?;
        let say = Say::Steer {
            text: text.into(),
            images: vec![],
            person: false,
        };
        // `begin_turn_if` logs the `Notice` row once a turn took it.
        if !self.begin_turn_if(thread_id, &live, true, Some(say), None, false, false, false)? {
            return Err("The agent is stopping, so the notice can't reach it.".into());
        }
        Ok(())
    }

    /// Starts the session's agent with `first`, the message numbered `seq` (the person's when `person`), or gives it
    /// to the agent already running.
    async fn start(
        &self,
        record: ThreadRecord,
        first: Say,
        seq: i64,
        person: bool,
        admitted_execution: Option<&AdmittedExecution>,
    ) -> Result<()> {
        self.check_saved_provider_record(&record)?;
        let thread_id = record.summary.id.clone();
        // A running agent takes the message as it is, even when its project was removed meanwhile (that stops no agent).
        // A linked session's message or a result to a halted one is refused there, and its row taken back.
        if let Some(existing) = self.live(&thread_id)
            && self
                .begin_turn(&thread_id, &existing, true, Some((first.clone(), seq)), person)
                .map_err(|error| self.not_taken(&thread_id, seq, error))?
        {
            return Ok(());
        }
        // A linked session runs only in its own worktree, never in the project's folder.
        if record.summary.parent_thread_id.is_some() && record.summary.worktree.is_none() {
            return Err(linked::NO_WORKTREE.into());
        }
        // Never above an ancestor's mode or effort, whatever was stored when it last ran.
        let mut record = self.clamp_to_ancestors(record)?;
        if record.summary.agent.permission_settings().is_some()
            && (record.summary.permission_mode != PermissionMode::Default || record.summary.parent_thread_id.is_some())
        {
            return Err(format!(
                "{} uses its own permission settings. Start a session with this agent directly.",
                driver::label(record.summary.agent),
            ));
        }
        let project = self
            .store()
            .project(&record.summary.project_id)?
            .ok_or_else(|| "This session's project is no longer open.".to_string())?;
        let cwd = PathBuf::from(
            self.worktree_of(&record)
                .as_ref()
                .map_or(&project.path, |worktree| &worktree.path),
        );
        let folder = cwd.to_string_lossy().into_owned();
        let events = if record.native_started {
            self.reader().events_after(&thread_id, 0)?
        } else {
            vec![]
        };
        // A revert since the agent last ran: it forgets the reverted turns, or is told about them.
        let mut rewind = if record.native_started {
            conversation(&events, record.summary.agent).1
        } else {
            None
        };
        // As the person sent it, for a resume the agent can't open ([`resume_recovery`]).
        let sent = (record.native_started || record.fork_from.is_some()).then(|| first.clone());
        let mut first = first;
        if rewind == Some(Rewind::Replay) {
            let seed = self.restart_rewound_conversation(&mut record, &events, seq)?;
            if let (Some(seed), Say::User { text, .. } | Say::Steer { text, .. }) = (seed, &mut first) {
                *text = format!("{seed}{text}");
            }
            rewind = None;
        }
        let resumed = record.native_started;
        // A fork's first start goes on from the source's conversation, as a new one.
        let fork = record.fork_from.as_deref().filter(|_| !resumed);
        if let (Some(Rewind::Tell), Say::User { text, .. } | Say::Steer { text, .. }) = (&rewind, &mut first) {
            *text = format!("{REVERT_NOTE}\n\n{text}");
        }
        // A move into a worktree since the agent last ran: it is told where it works now. A side chat's worktree is its
        // session's, which it follows.
        if let (Some(worktree), Say::User { text, .. } | Say::Steer { text, .. }) =
            (&self.worktree_of(&record), &mut first)
            && moving::unsaid(&events)
        {
            *text = format!("{}\n\n{text}", moving::note(&project.path, worktree));
        }
        // Claude Code loads the project's own settings only once the person trusts the project, and never in a review
        // worktree (claude_trust.rs). A limited launch passes the project's instructions on in the file below.
        let review = self
            .worktree_of(&record)
            .is_some_and(|worktree| worktree.review.is_some());
        let trust = self.claude_trust_launch(record.summary.agent, &project, &cwd, review, &events);
        // Claude Code reads the workbench's instructions from a file: inline, they would put line breaks on its command
        // line, which `claude.cmd` can't be started with. Inline only when the file can't be written (a full disk).
        let agent_harness = harness::harness(record.summary.agent);
        let config = self.config();
        let prompt = match (agent_harness.instructions(), &config.memory) {
            (harness::Instructions::All { .. }, Some(store)) => {
                let guidance = agent_harness.app_tools().offered().then(|| {
                    nexus_agent_memory::tools::launch_guidance(
                        mcp_servers::BUILTIN,
                        record.summary.agent == SessionAgent::Claude,
                    )
                });
                match store.instructions(Path::new(&folder), guidance.as_deref()) {
                    Ok(memory) if !memory.is_empty() => Some(format!("{}\n\n{memory}", driver::instructions())),
                    Ok(_) => None,
                    Err(error) => {
                        eprintln!("nexus-workbench: could not load shared memory: {error}");
                        None
                    }
                }
            }
            _ => None,
        };
        // The project's own instructions, when Claude Code won't load them itself, go after the shared memory too.
        let prompt = match (prompt, trust.instructions.as_deref()) {
            (Some(prompt), Some(project)) => Some(format!("{prompt}\n\n{project}")),
            (prompt, _) => prompt,
        };
        let instructions = if matches!(agent_harness.instructions(), harness::Instructions::All { file: true }) {
            let dir = config
                .instructions
                .unwrap_or_else(|| std::env::temp_dir().join("nexus-workbench"));
            match &prompt {
                Some(text) => config
                    .memory
                    .as_ref()
                    .expect("prompt has a memory store")
                    .prompt_file(text),
                None => driver::instructions_file_with(&dir, trust.instructions.as_deref()),
            }
            .inspect_err(|error| {
                eprintln!(
                    "nexus-workbench: could not write the agent instructions in {}: {error}",
                    dir.display()
                )
            })
            .ok()
        } else {
            None
        };
        let run = driver::Start {
            mode: record.summary.permission_mode,
            model: record.summary.model.as_deref(),
            effort: record.summary.effort,
            fast: record.summary.fast,
            native_id: fork.unwrap_or(&record.native_id),
            resume: resumed || fork.is_some(),
            fork: fork.is_some(),
            cwd: &folder,
            rewind: match &rewind {
                Some(Rewind::At(at)) => Some(at.as_str()),
                _ => None,
            },
            rewind_required: events
                .iter()
                .rev()
                .find_map(|event| match event.event {
                    WbEvent::Rewind { .. } => Some(true),
                    WbEvent::Revert { .. } => Some(false),
                    _ => None,
                })
                .unwrap_or(false),
            instructions: instructions.as_deref(),
            prompt: prompt.as_deref(),
            name: record.summary.name.as_deref(),
        };
        let agent = record.summary.agent;
        let sandbox = agent == SessionAgent::Antigravity && self.inner.host.antigravity_sandbox();
        let mut args = driver::args(agent, &run, sandbox, self.agy_auto());
        args.extend(trust.args);
        let antigravity_args = if agent == SessionAgent::Antigravity {
            args.clone()
        } else {
            Vec::new()
        };
        let protocol = driver::protocol(agent, &run);
        // A new conversation asks its agent for a title, and so does a fork's first start: a side chat forked at its
        // session's end resumes the session's conversation, but is still a new session the agent names.
        let protocol = if !resumed && record.automatic_title {
            title::wrap(protocol, &short_id())
        } else {
            protocol
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let (exited_tx, exited) = watch::channel(false);
        let live = Arc::new(Live {
            partial: Mutex::default(),
            created: Mutex::default(),
            identity: self.inner.host.session_choice_identity(),
            agent,
            saved_provider: record.summary.provider.is_some(),
            antigravity_args,
            recovery_identity: self.inner.host.update_recovery_identity().unwrap_or_default(),
            scope: record.scope.clone(),
            protocol: Mutex::new(protocol),
            status: Mutex::new(()),
            tx,
            exited,
            state: Mutex::new(LiveState {
                pid: None,
                pending: HashMap::new(),
                turns: VecDeque::new(),
                background: HashSet::new(),
                book: TaskBook::default(),
                base_cost: record.summary.cost_usd,
                confirmed: 0.0,
                estimate: 0.0,
                nexus_session: None,
                local_run: None,
                initialized: false,
                name_due: false,
                stop: None,
                last_activity: Instant::now(),
                last_delta: HashMap::new(),
                unwritten: None,
                usage_written: None,
                streamed: HashMap::new(),
                stderr: String::new(),
                explained: false,
                requests: 0,
                cut_short: false,
                reaped: false,
                changes: HashMap::new(),
                said: 0,
                mode_said: 0,
                spawned: false,
                // The last run's last checkpoint, if still being written: this run's first turn waits for it too (R5).
                checkpoint: self.pending_checkpoint(&thread_id),
                releasing: false,
                release_queued: false,
                interrupt_after_release: None,
                spend_notice_pending: linked::spend::notice_pending(&events),
                unsent: VecDeque::new(),
                unread: VecDeque::new(),
                steering: HashSet::new(),
                done: false,
                cursor: None,
                capabilities: None,
                connection: event::SessionConnection {
                    generation: short_id(),
                    via: Some(record.summary.via),
                    ..event::SessionConnection::default()
                },
                last_connection: None,
                account_revision: 0,
                mcp_revision: 0,
                skills_revision: 0,
                task_asks: HashMap::new(),
                owed: 0,
                owed_since: Instant::now(),
                notice_due: None,
                grant: Some(orchestra::Grant::new()),
                refused: None,
                account_dir: None,
                browser: Default::default(),
                resend: resume_recovery::Resend::keep(agent, resumed || fork.is_some(), sent, seq, person),
                transcript: None,
            }),
        });
        // Registered before the launch, so a message sent while the agent starts waits in the channel. Checked and
        // registered under one lock: two quick sends must not start two agents for one session. A running agent takes
        // the message; one that is stopping or ending is waited for, then this starts the next.
        loop {
            let existing = {
                let mut lives = self.inner.live.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if self.inner.update_restart.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err("Nexus is restarting for an update. Send your message after it opens.".into());
                }
                let existing = lives.get(&thread_id).cloned();
                if existing.is_none() {
                    // A halted linked session does not start, whoever sent the message (linked F3). Its message ends
                    // as a stopped turn.
                    if let Some(halt) = self.halted_link(&thread_id)? {
                        drop(lives);
                        return Err(self.refuse_halted(&thread_id, halt));
                    }
                    lives.insert(thread_id.clone(), live.clone());
                }
                existing
            };
            let Some(existing) = existing else {
                break;
            };
            if self
                .begin_turn(&thread_id, &existing, true, Some((first.clone(), seq)), person)
                .map_err(|error| self.not_taken(&thread_id, seq, error))?
            {
                return Ok(());
            }
            self.settle(&thread_id).await?;
        }
        let prompt_counts = match &first {
            Say::User { text, images } | Say::Steer { text, images, .. } => {
                Some(nexus_client::auto_route::Counts::of(text, !images.is_empty()))
            }
            _ => None,
        };
        let mut request = LaunchRequest {
            thread_id: thread_id.clone(),
            agent,
            cwd,
            model: record.summary.model.clone(),
            via: record.summary.via,
            required_payer: self.store().computer_required_payer(&thread_id, &record.scope)?,
            provider: record.summary.provider.clone(),
            account_dir: None,
            args,
            scope: record.scope.clone().into(),
            prompt_counts,
        };
        // The launch failed: the run ends here. Like `finish`, it leaves `live` only after its last write, so a message
        // sent meanwhile waits for it rather than start a run this would then mark failed.
        let fail = |this: &Workbench, text: String, detail: Option<String>| -> Result<()> {
            let (turns, changes, steered) = {
                let mut state = live.state();
                state.done = true;
                let turns: Vec<(String, bool)> = state.turns.drain(..).map(|turn| (turn.id, turn.person)).collect();
                (turns, unanswered_changes(&mut state.changes), state.steering.len())
            };
            // The person's messages queued behind the first: the agent never got them.
            let unread = steered + turns.iter().skip(1).filter(|(_, person)| *person).count();
            let written = (|| {
                if let Some((turn_id, _)) = turns.into_iter().next() {
                    this.append(
                        &thread_id,
                        WbEvent::TurnEnd {
                            turn_id,
                            status: TurnStatus::Error,
                            cost_usd: None,
                            duration_ms: 0,
                            error: Some(text.clone()),
                            cursor: None,
                        },
                    )?;
                }
                this.unread(&thread_id, unread, "The agent could not start, so it did not read")?;
                this.append(&thread_id, WbEvent::Error { text, detail })?;
                let _status = live.status_lock();
                let changes = live.state().newer_changes(changes);
                this.change(&thread_id, |record| {
                    record.summary.status = ThreadStatus::Error;
                    changes.into_iter().for_each(|change| change.apply(record));
                })?;
                Ok(())
            })();
            // Nothing could be saved (a full disk): the screen still learns the session failed.
            if written.is_err()
                && let Ok(mut record) = this.record(&thread_id)
                && this.emits_scope(&record.scope)
            {
                record.summary.status = ThreadStatus::Error;
                this.inner.host.emit(Emit::Thread(Box::new(record.summary)));
            }
            this.unregister(&thread_id, &live);
            this.run_ended(&thread_id);
            let _ = exited_tx.send(true);
            written
        };

        // From here on, a write that fails ends the run as a failed launch does, rather than leave the session
        // registered as running with no agent reading it.
        let opened = (|| {
            self.set_status(&thread_id, ThreadStatus::Starting)?;
            let opening = live.protocol().open();
            for out in opening {
                self.handle(&thread_id, &live, out)?;
            }
            if let Some(text) = trust.notice {
                self.append(&thread_id, WbEvent::Notice { text })?;
            }
            // A stop that lands from here on cancels the launch (below), so this first turn is always recorded.
            self.begin_turn(&thread_id, &live, false, Some((first, seq)), person)
        })();
        if let Err(error) = opened {
            return fail(self, UNSAVED.into(), Some(error));
        }

        // A stop that came in while the launch was being prepared wins over the launch failing: the session reads as
        // stopped either way, not as an error the person did not cause.
        let stopped = |live: &Live| live.state().stop.is_some();
        let account = if request.provider.is_some() {
            None
        } else if let Some(folders) = Self::plan_accounts(&record.summary) {
            match self.rotate(&record, folders).await {
                Ok(account) => account,
                Err(_) if stopped(&live) => return self.cancel_launch(&thread_id, &live, None, &exited_tx).await,
                Err(error) => return fail(self, error, None),
            }
        } else {
            record.summary.account.clone()
        };
        if let Some(account) = &account {
            match self.account_dir(record.summary.agent, account) {
                Ok(dir) => request.account_dir = Some(dir),
                Err(_) if stopped(&live) => return self.cancel_launch(&thread_id, &live, None, &exited_tx).await,
                Err(error) => return fail(self, error, None),
            }
        }
        if live.state().stop.is_some() {
            return self.cancel_launch(&thread_id, &live, None, &exited_tx).await;
        }
        // Rotation or a settings edit may change the choice between admission and launch.
        // Reuse admission on an exact match. A pre-spawn retry without it checks the current choice.
        if !record.native_started
            && request.provider.is_none()
            && request.via == Via::Subscription
            && let Some(payer) = request.required_payer
            && (request.model.is_some() || record.summary.effort.is_some())
        {
            let selection = computer::Selection {
                via: request.via,
                payer,
                model: request.model.clone(),
                effort: record.summary.effort,
            };
            let admitted = admitted_execution
                .is_some_and(|admitted| admitted.matches(agent, request.account_dir.as_deref(), &selection));
            if !admitted {
                let spawned = match self.reader().has_spawned_agent(&thread_id) {
                    Ok(spawned) => spawned,
                    Err(error) => return fail(self, UNSAVED.into(), Some(error)),
                };
                if !spawned {
                    let checked = self
                        .check_execution_selection(agent, request.account_dir.clone(), &selection)
                        .await;
                    if stopped(&live) {
                        return self.cancel_launch(&thread_id, &live, None, &exited_tx).await;
                    }
                    if let Err(error) = checked {
                        return fail(self, error, None);
                    }
                }
            }
        }
        live.state().account_dir = request.account_dir.clone();
        let mut launch = match self.inner.host.launch(request).await {
            Ok(launch) => launch,
            Err(_) if stopped(&live) => return self.cancel_launch(&thread_id, &live, None, &exited_tx).await,
            Err(error) => {
                if !live.saved_provider {
                    faults::agent_failed(agent, "launch_refused");
                }
                return fail(self, error, None);
            }
        };
        // OpenCode's limit on the project's own config (claude_trust.rs), over anything the host set.
        launch.spawn.env.extend(trust.env);
        let mut local_run = launch.local_run.take().map(|id| LocalRun {
            host: self.inner.host.clone(),
            id,
        });
        if live.state().stop.is_some() {
            drop(local_run.take());
            return self
                .cancel_launch(&thread_id, &live, launch.nexus_session, &exited_tx)
                .await;
        }
        // An agent that takes its servers in its session says which it skipped itself; for the others, the host lists
        // the servers it could not start (`mcp_servers::through_launcher_or_builtin`).
        if !mcp_servers::in_session(agent)
            && let Some(text) = mcp_servers::left_out_notice(&launch.session_mcp.skipped)
            && let Err(error) = self.append(&thread_id, WbEvent::Notice { text })
        {
            eprintln!("nexus-workbench: {thread_id}: {error}");
        }
        // The conversation opens only once the agent answers `initialize`, which it cannot do before it spawns below.
        live.protocol().session_mcp(std::mem::take(&mut launch.session_mcp));
        if launch
            .local_provider
            .as_ref()
            .is_some_and(|local| local.kind == nexus_providers::ProviderKind::OpenAiCompatible)
        {
            use nexus_local_agents::agents::{OPENCODE_SERVER_MODEL, opencode_server_provider};
            let provider = opencode_server_provider(&thread_id);
            live.protocol()
                .require_launch_model(&format!("{provider}/{OPENCODE_SERVER_MODEL}"));
        } else if let Some(model) = launch.model.as_deref() {
            live.protocol().launch_model(model);
        }
        if record.summary.provider.is_none() {
            self.arm(&record, &live, &mut launch.spawn).await;
        }
        // Read first: the store's guard ends here, before any await.
        let read = self.store().computer_required_payer(&thread_id, &record.scope);
        let required_payer = match read {
            Ok(required_payer) => required_payer,
            Err(error) => {
                drop(local_run.take());
                self.end_unspawned(launch.nexus_session.clone()).await;
                return Err(error);
            }
        };
        let payer_checked = if let Some(provider) = &record.summary.provider {
            launch
                .local_provider
                .as_ref()
                .filter(|local| &local.id == provider && launch.nexus_session.is_none() && local_run.is_some())
                .ok_or_else(|| "This computer did not prepare the session's saved provider.".to_string())
                .and_then(|local| local.check_payer(required_payer))
        } else if local_run.is_some() {
            agent_launch::check_local_subscription(agent, required_payer, &launch.spawn).await
        } else {
            computer::settings::check_spawn_payer(
                required_payer,
                agent,
                record.summary.via,
                launch.spawn.program.clone(),
                launch.spawn.env.clone(),
                launch.spawn.cwd.clone(),
            )
            .await
        };
        if let Err(error) = payer_checked {
            drop(local_run.take());
            self.end_unspawned(launch.nexus_session.clone()).await;
            return fail(self, error, None);
        }
        if stopped(&live) {
            drop(local_run.take());
            return self
                .cancel_launch(&thread_id, &live, launch.nexus_session, &exited_tx)
                .await;
        }
        // Claude Code's transcript, put back where it will look when it is elsewhere ([`claude_transcripts`]).
        if agent == SessionAgent::Claude {
            let place = claude_transcripts::Place::of(&launch.spawn.env, &launch.spawn.cwd);
            if let Some(place) = place.clone().filter(|_| resumed || fork.is_some()) {
                self.restore_transcript(&thread_id, place, fork.unwrap_or(&record.native_id))
                    .await;
            }
            live.state().transcript = place;
        }
        if let Some(local) = &launch.local_provider {
            if let Err(error) = self.check_saved_provider_record(&record) {
                return fail(self, error, None);
            }
            if local.kind == nexus_providers::ProviderKind::OpenAiCompatible
                && let Some(model) = launch.model.as_deref()
                && record.summary.model.as_deref() != Some(model)
                && let Err(error) = self.change(&thread_id, |row| row.summary.model = Some(model.into()))
            {
                return fail(self, UNSAVED.into(), Some(error));
            }
            if let Some(text) = local.mcp_warning()
                && let Err(error) = self.append(&thread_id, WbEvent::Notice { text })
            {
                return fail(self, UNSAVED.into(), Some(error));
            }
        }
        let started = Instant::now();
        let spawned = if live.saved_provider {
            local_provider::while_allowed(self.inner.host.as_ref(), || process::spawn(&launch.spawn))
                .and_then(|child| child)
        } else {
            process::spawn(&launch.spawn)
        };
        let mut child = match spawned {
            Ok(child) => {
                if live.saved_provider
                    && let Some(spawned) = Gated::gate(self).saved_provider_starts.get_mut(&thread_id)
                {
                    *spawned = true;
                }
                child
            }
            Err(error) => {
                drop(local_run.take());
                if let Some(session) = launch.nexus_session.clone() {
                    self.inner
                        .host
                        .ended(
                            session,
                            ProcessEnd {
                                exit_code: 127,
                                signal: None,
                                duration_ms: 0,
                            },
                        )
                        .await;
                }
                if !live.saved_provider {
                    faults::agent_failed(agent, faults::io_code(&error));
                }
                return fail(
                    self,
                    format!("Could not start {}.", launch.spawn.program.display()),
                    Some(error.to_string()),
                );
            }
        };
        // A stop that came in since the check above found no process to signal: the agent is killed before it reads
        // anything. Otherwise the stop sees `spawned` and signals the pid stored with it.
        let stopped = {
            let mut state = live.state();
            state.pid = child.id();
            state.nexus_session = launch.nexus_session.clone();
            state.connection.attribution = launch.attribution.clone().unwrap_or_default();
            state.connection.selected_account = account.clone();

            state.spawned = state.stop.is_none();
            if state.spawned {
                state.local_run = local_run.take();
            }
            !state.spawned
        };
        if stopped {
            drop(local_run.take());
            process::kill(child.id());
        }
        let saved = (|| {
            if let Some(session) = &launch.nexus_session {
                self.store().open_session(session, &thread_id, now_ms())?;
            }
            self.change(&thread_id, |record| {
                if let Some(session) = &launch.nexus_session {
                    record.summary.nexus_sessions.push(session.clone());
                }
                if nexus_client::agents::is_auto_name(record.summary.model.as_deref())
                    && let Some(model) = &launch.model
                {
                    record.summary.model = Some(model.clone());
                }
            })?;
            self.append(
                &thread_id,
                WbEvent::Session {
                    nexus_session_id: launch.nexus_session.clone(),
                    native_id: Some(record.native_id.clone()),
                    model: launch.model.clone().or(record.summary.model.clone()),
                    via: record.summary.via,
                    resumed,
                },
            )
        })();
        if let Err(error) = saved {
            self.end_unread(&thread_id, &live, child, started).await;
            return fail(self, UNSAVED.into(), Some(error));
        }

        #[cfg(all(test, unix))]
        if self.inner.close_launch_stderr.load(std::sync::atomic::Ordering::SeqCst) {
            drop(child.stderr.take());
        }
        let (Some(mut stdin), Some(stdout), Some(mut stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let run = live.state().local_run.take();
            drop(run);
            let _ = child.start_kill();
            let session = live.state().nexus_session.take();
            if let Some(session) = session {
                self.store().close_session(&session)?;
                let end = ProcessEnd {
                    exit_code: 1,
                    signal: None,
                    duration_ms: 0,
                };
                self.inner.host.ended(session, end).await;
            }
            return fail(self, "Could not talk to the agent.".into(), None);
        };
        let mut rx = rx;
        // A killed agent gets no input: its stdin closes here.
        if !stopped {
            let owner = live.saved_provider.then(|| self.inner.host.clone());
            tokio::spawn(async move {
                while let Some(input) = rx.recv().await {
                    match input {
                        Input::Line(line) => {
                            let line = format!("{line}\n");
                            let written = match owner.as_deref() {
                                Some(host) => local_provider::write_input(host, &mut stdin, line.as_bytes()).await,
                                None => stdin.write_all(line.as_bytes()).await,
                            };
                            if written.is_err() {
                                break;
                            }
                            let _ = stdin.flush().await;
                        }
                        Input::Close => break,
                    }
                }
                // Dropping stdin closes it: the agent ends once it has nothing left to do.
            });
        }
        let stderr_live = live.clone();
        tokio::spawn(async move {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = stderr.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                let mut state = stderr_live.state();
                state.stderr.push_str(&String::from_utf8_lossy(&buffer[..read]));
                // The last few kilobytes are enough to say why it failed.
                if state.stderr.len() > 8192 {
                    let cut = state.stderr.len() - 8192;
                    let cut = (cut..state.stderr.len())
                        .find(|i| state.stderr.is_char_boundary(*i))
                        .unwrap_or(0);
                    state.stderr.drain(..cut);
                }
            }
        });
        let this = self.clone();
        tokio::spawn(async move {
            let pid = child.id();
            let mut exit = Box::pin(async move { child.wait().await.ok() });
            let mut status = None;
            let mut drained_by = None;
            let mut reader = lines::Lines::new(BufReader::new(stdout), lines::MAX_LINE, thread_id.clone());
            let mut buffer = Vec::new();
            loop {
                // Counted from the last line: a line that comes first starts the wait again.
                let quiet = live.protocol().quiet_after();
                tokio::select! {
                    // Raw lines, decoded leniently: one bad byte must not stop the reading, or the agent blocks on a
                    // full pipe. A line cut off by another branch stays in `buffer`, and the next read goes on with it.
                    read = reader.next(&mut buffer) => {
                        if !matches!(read, Ok(read) if read > 0) {
                            break;
                        }
                        let line = String::from_utf8_lossy(&buffer);
                        let outs = live.protocol().read(line.trim_end_matches(['\n', '\r']));
                        for out in outs {
                            if let Err(error) = this.handle(&thread_id, &live, out) {
                                eprintln!("nexus-workbench: {thread_id}: {error}");
                            }
                        }
                        this.notice_turn_begun(&thread_id, &live);
                        buffer.clear();
                    }
                    () = tokio::time::sleep(quiet.unwrap_or_default()), if quiet.is_some() => {
                        let outs = live.protocol().quiet();
                        for out in outs {
                            if let Err(error) = this.handle(&thread_id, &live, out) {
                                eprintln!("nexus-workbench: {thread_id}: {error}");
                            }
                        }
                        this.notice_turn_begun(&thread_id, &live);
                    }
                    // The agent exited, while its output may still be open: a process it left behind holds it.
                    exited = &mut exit, if status.is_none() => {
                        status = Some(exited);
                        // Set before the Gateway call in `finish`, which can take a while: a stop's escalation must
                        // not signal a pid the system may already have given to another process. The grant goes
                        // with it: a process the agent left behind keeps its token, and no call from it is taken.
                        live.mark_reaped();
                        process::end_leftovers(pid);
                        drained_by = Some(tokio::time::Instant::now() + process::DRAIN);
                    }
                    () = tokio::time::sleep_until(drained_by.unwrap_or_else(tokio::time::Instant::now)),
                        if drained_by.is_some() => break,
                }
            }
            let status = match status {
                Some(status) => status,
                None => {
                    let status = exit.await;
                    live.mark_reaped();
                    process::end_leftovers(pid);
                    status
                }
            };
            this.finish(&thread_id, &live, exit_of(status), started).await;
            let _ = exited_tx.send(true);
        });
        Ok(())
    }

    /// An agent spawned that nothing will read: killed and waited for, and the Nexus session its launch opened ended.
    async fn end_unread(&self, thread_id: &str, live: &Live, mut child: tokio::process::Child, started: Instant) {
        let run = live.state().local_run.take();
        drop(run);
        process::kill(child.id());
        let (exit_code, signal) = exit_of(child.wait().await.ok());
        // Before the Gateway call below, as the read loop does: a stop's escalation must not signal a pid the system
        // may already have given to another process.
        live.mark_reaped();
        let session = live.state().nexus_session.take();
        if let Some(session) = session {
            if let Err(error) = self.store().close_session(&session) {
                eprintln!("nexus-workbench: {thread_id}: {error}");
            }
            let end = ProcessEnd {
                exit_code,
                signal,
                duration_ms: started.elapsed().as_millis() as u64,
            };
            self.inner.host.ended(session, end).await;
        }
    }

    /// The host launched, and the agent will not be spawned: its run ends as one that never started, so the
    /// credentials the host granted for it end with it.
    async fn end_unspawned(&self, nexus_session: Option<String>) {
        if let Some(session) = nexus_session {
            let never_started = ProcessEnd {
                exit_code: 127,
                signal: None,
                duration_ms: 0,
            };
            self.inner.host.ended(session, never_started).await;
        }
    }

    /// Stopped (or interrupted) before the agent was spawned: the run ends as a stopped one does, its first message
    /// unread, and the Nexus session the launch opened, if any, is ended.
    async fn cancel_launch(
        &self,
        thread_id: &str,
        live: &Arc<Live>,
        nexus_session: Option<String>,
        exited: &watch::Sender<bool>,
    ) -> Result<()> {
        live.state().nexus_session = nexus_session;
        self.finish(thread_id, live, (0, None), Instant::now()).await;
        let _ = exited.send(true);
        Ok(())
    }

    fn handle(&self, thread_id: &str, live: &Live, out: Out) -> Result<()> {
        if self
            .live(thread_id)
            .is_none_or(|current| !std::ptr::eq(current.as_ref(), live))
        {
            return Ok(());
        }

        // An answer to our own request about a task is not the agent at work: an output view that keeps reading must
        // not keep an idle agent alive.
        if !matches!(out, Out::Answered { .. } | Out::Observe(_)) {
            live.state().last_activity = Instant::now();
        }
        match out {
            Out::Title(text) => {
                if let Some(title) = title::clean(&text) {
                    self.change(thread_id, |record| {
                        if record.automatic_title {
                            record.summary.title = title;
                        }
                    })?;
                }
            }
            Out::Delta { id, text, parent } => {
                if parent.is_none() {
                    // Streamed content can precede the spontaneous turn's completed message.
                    self.begin_own_turn(thread_id, live)?;
                }
                self.partial_delta(thread_id, live, &id, parent.as_deref(), &text);
                // At most one update every 50 ms per stream reaches the screen; the final text replaces it anyway. The main
                // answer and each sub-agent have their own slot, so one streaming beside another never starves it.
                let show = delta_due(&mut live.state().last_delta, &parent, Instant::now());
                if show && self.shown(thread_id) {
                    // Only what the screen does not have yet: the whole text every 50 ms made a long answer cost O(n²).
                    let (text, offset) = delta_suffix(&mut live.state().streamed, &id, &parent, text);
                    self.inner.host.emit(Emit::Delta(Delta {
                        thread_id: thread_id.into(),
                        id,
                        text,
                        parent,
                        offset,
                    }));
                }
            }
            Out::Event(event) => {
                self.observe_native_terminal(thread_id, live, &event)?;
                self.observe_browser_routing(thread_id, live, &event)?;
                if let WbEvent::Commands { commands } = &event {
                    if let Some(native) = harness::harness(live.agent).session_features().native_commands
                        && let Some(capabilities) = &mut live.state().capabilities
                    {
                        native.apply(capabilities, commands);
                    }
                    self.change(thread_id, |_| {})?;
                }
                // Calls' cost and context held back (`usage_due`) go out with the next event a second on.
                let due = {
                    let state = &mut *live.state();
                    usage_due(&mut state.unwritten, &mut state.usage_written, None, Instant::now())
                };
                if let Some(due) = due {
                    self.change(thread_id, |record| due.apply(record))?;
                }
                // A task's report goes through the book, which may hold it or drop it; tool rows tell it whose work
                // shows in the log. Held progress now due goes first.
                let (held, event, agent_ended) = {
                    let book = &mut live.state().book;
                    let now = Instant::now();
                    let held = book.due(Some(now));
                    // A sub-agent's end: a linked session with one running is not finished ([`linked::results`]).
                    let ending = match &event {
                        WbEvent::Task { task_id, status, .. }
                            if *status != TaskStatus::Running
                                && book.running(task_id).is_some_and(|(kind, _)| kind == "agent") =>
                        {
                            Some(task_id.clone())
                        }
                        _ => None,
                    };
                    let event = match event {
                        WbEvent::Task { .. } => match book.take(event, now) {
                            Take::Write(event) => Some(*event),
                            Take::Hold | Take::Drop => None,
                        },
                        event => Some(event),
                    };
                    match &event {
                        Some(WbEvent::Tool { parent: Some(call), .. }) => book.saw_work_of(call),
                        Some(WbEvent::ToolResult { id, .. }) => book.result_of(id),
                        _ => {}
                    }
                    let agent_ended = ending.is_some_and(|task| book.running(&task).is_none());
                    (held, event, agent_ended)
                };
                self.append_held(thread_id, live, held);
                if agent_ended {
                    self.agent_task_ended(thread_id, live);
                }
                let Some(event) = event else {
                    return Ok(());
                };
                if let WbEvent::Text { id, parent, .. } = &event {
                    // The final text replaces the streamed one on screen: nothing more streams under this id.
                    live.state().streamed.remove(&(id.clone(), parent.clone()));
                }
                // Work a sub-agent owns (its events, its reports, its call's result) is that agent's, never the
                // session's: it opens no turn, wherever it lands. Only the session's own agent starts a turn of its own
                // (Claude Code answering a background agent's report). a private formal model I1. A notice needs no turn of
                // its own either: one between turns is a sub-agent's (which names no parent), or the lead compacting
                // before a turn it starts itself, whose next event or result opens that turn.
                let owned = {
                    let state = live.state();
                    event.is_report()
                        || event.parent().is_some()
                        || state.book.owns(&event)
                        || matches!(event, WbEvent::Notice { .. })
                };
                if !owned {
                    // Tool or usage events can arrive before a spontaneous turn's text.
                    self.begin_own_turn(thread_id, live)?;
                }
                if let WbEvent::Approval {
                    request_id,
                    tool,
                    input,
                    parent,
                    ..
                } = &event
                {
                    // Only a permission ask of Nexus's own agent, in Auto or Full access, is allowed at once with no
                    // request shown. A coding agent's ask, a question and a plan wait for the person
                    // ([`driver::answers_ask`]).
                    let unasked = self
                        .record(thread_id)
                        .is_ok_and(|record| driver::answers_ask(live.agent, record.summary.permission_mode, tool));
                    if unasked {
                        let say = Say::Allow {
                            request_id: request_id.clone(),
                            input: input.clone(),
                            answers: None,
                            remember: None,
                        };
                        let outs = live.tell(say);
                        return self.outs(thread_id, live, outs);
                    }
                    let asked = Asked {
                        tool: tool.clone(),
                        input: input.clone(),
                        owner: parent.clone(),
                    };
                    live.state().pending.insert(request_id.clone(), asked);
                    self.append_from(thread_id, live, event)?;
                    self.refresh_status(thread_id, live)?;
                    return Ok(());
                }
                // A task's end ends the approvals it asked for: it no longer waits for an answer. So does the start of a
                // new run: the run before it is over, even when its own end comes late and the book drops it as an
                // older run's (a private formal model A3).
                let ended = match &event {
                    WbEvent::Task { task_id, detail, .. } if detail.as_ref().and_then(|d| d.progress) != Some(true) => {
                        let state = live.state();
                        let call = state.book.call_of(task_id).map(String::from);
                        let expired: Vec<String> = state
                            .pending
                            .iter()
                            .filter(|(_, asked)| {
                                asked
                                    .owner
                                    .as_deref()
                                    .is_some_and(|o| o == task_id || Some(o) == call.as_deref())
                            })
                            .map(|(id, _)| id.clone())
                            .collect();
                        expired
                    }
                    _ => Vec::new(),
                };
                // A wait's answer the agent read whole confirms what it carried ([`linked::results`]).
                let answer = match &event {
                    WbEvent::ToolResult {
                        output,
                        is_error: false,
                        ..
                    } if self.awaits_receipt(thread_id) => Some(output.clone()),
                    _ => None,
                };
                let task_running = matches!(
                    &event,
                    WbEvent::Task {
                        status: TaskStatus::Running,
                        ..
                    }
                );
                self.append_from(thread_id, live, event)?;
                if task_running {
                    self.quiet_if_tripped(thread_id);
                }
                if let Some(output) = answer {
                    self.tool_result_logged(thread_id, &output);
                }
                if !ended.is_empty() {
                    for request_id in ended {
                        live.state().pending.remove(&request_id);
                        self.append_from(
                            thread_id,
                            live,
                            WbEvent::ApprovalResolved {
                                request_id,
                                decision: Decision::Expired,
                                message: None,
                            },
                        )?;
                    }
                    self.refresh_status(thread_id, live)?;
                }
            }
            Out::Init {
                native_id,
                model,
                version,
            } => {
                {
                    let mut state = live.state();
                    state.connection.native_id = Some(native_id.clone());
                    state.connection.model = model.clone();
                    state.connection.version = version.clone();
                }
                let (first, changed) = {
                    let mut state = live.state();
                    let commands = state
                        .capabilities
                        .as_ref()
                        .and_then(|previous| previous.commands.as_deref());
                    let capabilities =
                        Some(harness::harness(live.agent).live_capabilities(version.as_deref(), commands));
                    let changed = state.capabilities != capabilities;
                    state.capabilities = capabilities;
                    (!std::mem::replace(&mut state.initialized, true), changed)
                };
                // The first one shows them with the rest; a later one only when they changed.
                if !first && changed {
                    self.change(thread_id, |_| {})?;
                }
                if first {
                    self.change(thread_id, |record| {
                        record.native_started = true;
                        // A fork now has a conversation of its own, and its agent has what was said before.
                        record.fork_from = None;
                        record.seed = None;
                        // The agent may run the conversation under another id than the one asked for (a Claude Code
                        // fork, any Codex thread): the next start resumes the one it reports, or nothing is found.
                        if !native_id.is_empty() {
                            record.native_id = native_id;
                        }
                    })?;
                    self.refresh_status(thread_id, live)?;
                    self.publish_connection(thread_id, live)?;
                }
                // A name accepted before the agent was ready, with no turn to end, goes out now.
                self.forward_due_name(thread_id, live);
            }
            Out::Observe(observation) => {
                {
                    let mut state = live.state();
                    if observation.get("account").is_some() {
                        state.account_revision += 1;
                    }
                    if observation.get("mcp").is_some() || observation.get("mcpUpdate").is_some() {
                        state.mcp_revision += 1;
                    }
                    if observation.get("skills").is_some() {
                        state.skills_revision += 1;
                    }
                    connection::apply(&mut state.connection, live.agent, &observation);
                    if !live.saved_provider {
                        let folder = state.account_dir.as_deref();
                        match harness::harness(live.agent).observed_plan(&observation) {
                            harness::PlanSeen::Remember(plan) => plan_tier::remember(live.agent, folder, Some(&plan)),
                            harness::PlanSeen::Forget => plan_tier::forget(live.agent, folder),
                            harness::PlanSeen::Keep => {}
                        }
                    }
                }
                self.publish_connection(thread_id, live)?;
            }
            Out::Progress(update) => {
                let row = match live.state().book.take(update, Instant::now()) {
                    Take::Write(row) => Some(*row),
                    Take::Hold | Take::Drop => None,
                };
                if let Some(row) = row {
                    self.append_from(thread_id, live, row)?;
                    self.quiet_if_tripped(thread_id);
                }
            }
            Out::Call { model, usage, parent } => {
                if parent.is_none() && live.state().connection.model.as_ref() != Some(&model) {
                    live.state().connection.model = Some(model.clone());
                    self.publish_connection(thread_id, live)?;
                }
                if parent.is_none() {
                    // Usage alone must trigger halt and spend checks before final text.
                    self.begin_own_turn(thread_id, live)?;
                }
                let held = live.state().book.due(Some(Instant::now()));
                self.append_held(thread_id, live, held);
                let cost = if live.saved_provider && live.agent.coding() == Some(nexus_agents::Agent::Opencode) {
                    0.0
                } else {
                    self.inner.host.price(&model, &usage).unwrap_or(0.0)
                };
                let context = parent.is_none().then_some(
                    usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens + usage.output_tokens,
                );
                let due = {
                    let mut state = live.state();
                    // A call between turns (a Codex sub-agent's) belongs to no turn: it counts toward the session's
                    // total at once, not toward the next turn's.
                    if state.turns.is_empty() {
                        state.confirmed += cost;
                    } else {
                        state.estimate += cost;
                    }
                    let total = state.base_cost + state.confirmed + state.estimate;
                    let state = &mut *state;
                    usage_due(
                        &mut state.unwritten,
                        &mut state.usage_written,
                        Some((total, context)),
                        Instant::now(),
                    )
                };
                self.append_from(
                    thread_id,
                    live,
                    WbEvent::Usage {
                        model: model.clone(),
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cache_read_tokens: usage.cache_read_tokens,
                        cache_write_tokens: usage.cache_write_tokens,
                        cost_usd: cost,
                        parent,
                    },
                )?;
                // A linked tree's spend cap is checked at every call ([`linked::spend`]).
                self.count_call(thread_id, live, &model, &usage);
                // At most once a second: each write is a read and a write of the session and a `wb:thread` that wakes
                // every list on screen, and a long turn makes hundreds of calls. The turn's end writes what is left.
                if let Some(due) = due {
                    self.change(thread_id, |record| due.apply(record))?;
                }
            }
            Out::TurnQueued { person } => {
                self.reserve_turn(thread_id, live, person)?;
            }
            Out::Result {
                total_cost_usd,
                duration_ms: _,
                error,
                interrupted,
                context_window,
            } => {
                // What the tasks did since their last row is in the log before the turn ends (in `recorded`).
                let held = live.state().book.due(None);
                // A result with no turn open (one a sub-agent's work never opened) still ends one: the pair is logged,
                // and the turn's checkpoint taken. a private formal model I2.
                if live.state().turns.is_empty() {
                    self.begin_turn(thread_id, live, false, None, false)?;
                }
                let automatic = live.state().turns.front().is_some_and(|turn| !turn.person);
                // A late success cannot complete a turn forbidden by a halt or spend trip.
                let interrupted = interrupted
                    || (automatic && (self.message_refusal(thread_id).is_some() || self.tripped(thread_id)));
                let (turn, interrupted, turn_cost, total, next, first, written, expired, cursor, unwritten) = {
                    let mut state = live.state();
                    let unwritten = state.unwritten.take();
                    let turn_cost = match total_cost_usd {
                        Some(process_total) => (process_total - state.confirmed).max(0.0),
                        None => state.estimate,
                    };
                    state.confirmed += turn_cost;
                    state.estimate = 0.0;
                    let turn = state.turns.pop_front();
                    let interrupted = interrupted || turn.as_ref().is_some_and(|turn| turn.stopping);
                    if interrupted && state.stop.is_some() {
                        state.cut_short = true;
                    }
                    let cursor = state.cursor.take();
                    let next = state.turns.front_mut().map(|t| (t.id.clone(), t.seq, t.held.take()));
                    // The person's message queued behind it begins: an Interrupt since it was sent holds no longer. Not
                    // when the run is stopping: that message never runs, and a Stop's halt must hold.
                    if state.stop.is_none() && !state.done && state.turns.front().is_some_and(|t| t.person) {
                        self.person_turn_begins(thread_id);
                    }
                    // A notice's reserved turn begins: no `User` event lifts a halt for it, and a halt or a trip
                    // since it was reserved stops it (`notice_turn_begun`).
                    if state.stop.is_none()
                        && !state.done
                        && let Some(turn) = state.turns.front().filter(|t| t.notice)
                    {
                        let id = turn.id.clone();
                        state.notice_due = Some(id);
                    }
                    // In the same hold of the lock: this turn's checkpoint is armed and the next message is in line,
                    // so a message sent, or sent now, from here on waits for the checkpoint and goes after it.
                    let (written, checkpoint) = watch::channel(false);
                    state.checkpoint = Some(checkpoint);
                    let (next, first) = match next {
                        Some((turn_id, seq, Some(say))) => {
                            let first = state.line_up(seq, say);
                            // From here an interrupt waits for the message it would stop (`send_lined_up`). The
                            // person's spares it: it was meant for the turn that just ended (`stop_turn`).
                            state.releasing |= first;
                            state.release_queued |= first;
                            (Some(turn_id), first)
                        }
                        Some((turn_id, _, None)) => (Some(turn_id), false),
                        None => (None, false),
                    };
                    // The session's turn ends the approvals asked inside it: its own agent's, and a foreground
                    // sub-agent's it started. A background sub-agent's, and any Codex child's, outlive it.
                    let lead: Vec<String> = state
                        .pending
                        .iter()
                        .filter(|(_, asked)| state.book.ends_with_lead_turn(asked.owner.as_deref()))
                        .map(|(id, _)| id.clone())
                        .collect();
                    for request_id in &lead {
                        state.pending.remove(request_id);
                    }
                    let expired = lead;
                    (
                        turn,
                        interrupted,
                        turn_cost,
                        state.base_cost + state.confirmed,
                        next,
                        first,
                        written,
                        expired,
                        cursor,
                        unwritten,
                    )
                };
                let status = if interrupted {
                    TurnStatus::Interrupted
                } else if error.is_some() {
                    TurnStatus::Error
                } else {
                    TurnStatus::Completed
                };
                // The plan refused the turn at its usage limit, and no message waits to run next: it stops there. Taken:
                // a refused call says so again in the next turn, so a later failure is not read as this limit.
                let refused = live.state().refused.take();
                let limited = match refused {
                    Some(resets_at) if error.is_some() && !interrupted && next.is_none() => {
                        Some(self.limit_stop(thread_id, resets_at)?)
                    }
                    _ => None,
                };
                // The message whose turn this was: a result message's turn that ended complete confirms it.
                let row = turn.as_ref().and_then(|turn| turn.seq);
                // Recorded apart from the send below: a failed log write must not leave the next message at the front
                // of the line, unsent, holding every later one back.
                let recorded = (|| -> Result<()> {
                    for row in held {
                        self.append_from(thread_id, live, row)?;
                    }
                    for request_id in expired {
                        self.append_from(
                            thread_id,
                            live,
                            WbEvent::ApprovalResolved {
                                request_id,
                                decision: Decision::Expired,
                                message: None,
                            },
                        )?;
                    }
                    let ended = self.append_from(
                        thread_id,
                        live,
                        WbEvent::TurnEnd {
                            turn_id: turn
                                .as_ref()
                                .map(|t| t.id.clone())
                                .unwrap_or_else(|| format!("turn_{}", short_id())),
                            status,
                            cost_usd: Some(turn_cost),
                            duration_ms: turn.map(|t| t.started.elapsed().as_millis() as u64).unwrap_or(0),
                            error,
                            cursor,
                        },
                    )?;
                    self.checkpoint_after(thread_id, ended.seq, written);
                    self.keep_transcript(thread_id, live);
                    if let Some(turn_id) = next {
                        self.append_from(thread_id, live, WbEvent::TurnStart { turn_id })?;
                    }
                    {
                        let _status = live.status_lock();
                        let status = live_status(&live.state());
                        self.change(thread_id, |record| {
                            if let Some(unwritten) = unwritten {
                                unwritten.apply(record);
                            }
                            record.summary.cost_usd = total;
                            if context_window.is_some() {
                                record.summary.context_window = context_window;
                            }
                            if let Some(status) = status {
                                record.summary.status = status;
                            }
                            if limited.is_some() {
                                record.summary.limited = limited;
                            }
                        })?;
                    }
                    Ok(())
                })();
                // After the status lock: telling the agent can take it again.
                let sent = if first {
                    self.send_lined_up(thread_id, live)
                } else {
                    Ok(())
                };
                if recorded.is_ok() {
                    self.turn_ended(thread_id, live, row, status);
                    self.forward_due_name(thread_id, live);
                }
                recorded?;
                sent?;
            }
            Out::Ack { request_id, error } => {
                let change = live.state().changes.remove(&request_id);
                match (error.filter(|_| request_id != "init"), change) {
                    // Refused: nothing to save, but the session is shown again (a newer `updated_at`) without the
                    // change it showed as pending, so it reads the value in force.
                    (Some(error), change) => {
                        let what = match change {
                            Some((_, Change::Mode(_))) => "the permission mode change",
                            Some((_, Change::Model(_))) => "the model change",
                            Some((_, Change::Effort(_))) => "the effort change",
                            Some((_, Change::Fast(_))) => "the fast mode change",
                            None => "that change",
                        };
                        self.append_from(
                            thread_id,
                            live,
                            WbEvent::Notice {
                                text: format!("{} did not apply {what}: {error}", driver::label(live.agent)),
                            },
                        )?;
                        self.change(thread_id, |_| {})?;
                    }
                    (None, Some((n, change))) => self.accepted(thread_id, live, n, change)?,
                    (None, None) => {}
                }
            }
            Out::Answered { request_id, result } => {
                if let Some(asked) = live.state().task_asks.remove(&request_id) {
                    let _ = asked.send(result);
                }
            }
            Out::Cancelled { request_id } => {
                if live.state().pending.remove(&request_id).is_some() {
                    self.append_from(
                        thread_id,
                        live,
                        WbEvent::ApprovalResolved {
                            request_id,
                            decision: Decision::Expired,
                            message: None,
                        },
                    )?;
                    self.refresh_status(thread_id, live)?;
                }
            }
            Out::Background(change) => {
                let between_turns = {
                    let mut state = live.state();
                    let before = state.background.clone();
                    match change {
                        Background::Started(task) => {
                            state.background.insert(task);
                        }
                        Background::Ended(task) => {
                            state.background.remove(&task);
                        }
                        Background::Now(tasks) => state.background = tasks.into_iter().collect(),
                    }
                    state.background != before && state.turns.is_empty()
                };
                // Between turns no turn's end shows the session again: what it left running changed now.
                if between_turns {
                    self.emit_summary(thread_id);
                }
            }
            Out::TaskReady(task) => {
                live.state().book.task_ready(&task);
                self.quiet_if_tripped(thread_id);
            }
            Out::Reply(line) => live.send(line),
            Out::Close => {
                live.state().explained = true;
                let _ = live.tx.send(Input::Close);
            }
            Out::Cursor(at) => {
                if harness::harness(live.agent).session_features().cursor_starts_turn {
                    self.begin_own_turn(thread_id, live)?;
                }
                live.state().cursor = Some(at);
            }
            Out::AutoOffered(offered) => {
                *self.inner.agy_auto.lock().unwrap_or_else(|p| p.into_inner()) = Some(offered);
            }
            Out::Taken { texts } => {
                let read = read_back(&mut live.state().unread, &texts);
                let mut steered = false;
                for seq in read {
                    steered |= live.state().steering.remove(&seq);
                    self.append_from(thread_id, live, WbEvent::Read { seq })?;
                    self.read_logged(thread_id, seq);
                }
                // A message sent into a turn that ended first (the person stopped it, or it had no step left): the
                // agent answers it in a turn of its own. That turn answers the person, so it begins as theirs, and a
                // stop's halt does not end it (`linked::halts`). A spend trip still does (`own_turn_begun`).
                if steered && live.state().turns.is_empty() {
                    self.person_turn_begins(thread_id);
                    self.begin_own_turn(thread_id, live)?;
                    if let Some(turn) = live.state().turns.front_mut() {
                        turn.person = true;
                    }
                }
            }
            Out::RateLimit {
                session,
                weekly,
                session_resets,
                weekly_resets,
                refused,
                resets_at,
            } => {
                let summary = self.record(thread_id)?.summary;
                if summary.provider.is_none()
                    && summary.via == Via::Subscription
                    && let Some(mut rotation) = self.rotation_of(summary.agent)
                {
                    rotation.saw(&summary.account, session, weekly, refused);
                    drop(rotation);
                    let limits: Vec<plans::Limit> =
                        [("Session", session, session_resets), ("Weekly", weekly, weekly_resets)]
                            .into_iter()
                            .filter_map(|(label, used, resets_at)| {
                                Some(plans::Limit {
                                    models: Vec::new(),
                                    label: label.into(),
                                    used_percent: used?,
                                    resets_at,
                                })
                            })
                            .collect();
                    if !limits.is_empty() {
                        self.inner.host.emit(Emit::Plan(plan_book::PlanReport {
                            agent: summary.agent,
                            account: summary.account.clone(),
                            limits,
                            at: now_ms(),
                        }));
                    }
                }
                if summary.provider.is_none() && summary.via == Via::Subscription {
                    live.state().refused = refused.then_some(resets_at);
                }
            }
        }
        Ok(())
    }

    /// Progress rows the task book held, now due. A row that cannot be written is only reported: it must not cost the
    /// event, call or turn end it goes out with.
    fn append_held(&self, thread_id: &str, live: &Live, rows: Vec<WbEvent>) {
        for row in rows {
            if let Err(error) = self.append_from(thread_id, live, row) {
                eprintln!("nexus-workbench: {thread_id}: {error}");
            }
        }
    }

    /// The run ended (its process exited, or its launch was cancelled): its last events and the session's status. The
    /// run stays the session's live one until they are written, and takes no more messages meanwhile (`done`): a
    /// message sent now waits, then starts the next run, whose events and status this can no longer overwrite.
    async fn finish(
        &self,
        thread_id: &str,
        live: &Arc<Live>,
        (exit_code, signal): (i32, Option<String>),
        started: Instant,
    ) {
        let (
            turns,
            lined,
            pending,
            stop,
            initialized,
            stderr,
            nexus_session,
            total,
            cut_short,
            spawned,
            changes,
            background,
            unwritten,
            cursor,
            ended,
            resend,
        ) = {
            let mut state = live.state();
            // First: no call from this run is taken from here on.
            state.grant = None;
            state.done = true;
            // Whoever waits for an answer about a task hears the agent stopped.
            state.task_asks.clear();
            let changes = unanswered_changes(&mut state.changes);
            // Messages still in line that no turn stands for ("Send now" ones): the agent never got them either.
            let LiveState {
                unsent,
                turns,
                steering,
                ..
            } = &mut *state;
            let mut unread = std::mem::take(steering);
            unread.extend(
                unsent
                    .drain(..)
                    .filter(|(seq, _)| !turns.iter().any(|t| seq.is_some() && t.seq == *seq))
                    .filter_map(|(seq, _)| seq),
            );
            let lined = unread.len();
            (
                state.turns.drain(..).collect::<Vec<_>>(),
                lined,
                state.pending.drain().map(|(id, _)| id).collect::<Vec<_>>(),
                state.stop,
                state.initialized,
                state.stderr.trim().to_string(),
                state.nexus_session.take(),
                state.base_cost + state.confirmed + state.estimate,
                state.cut_short,
                state.spawned,
                changes,
                !state.background.is_empty(),
                state.unwritten.take(),
                // Where the agent's conversation stood in the turn it was stopped in: a later revert cuts there
                // (a private formal model R1).
                state.cursor.take(),
                // What it still ran stopped with it: one row per task, so no reader has to guess from a notice.
                state.book.end_all("process_ended"),
                state.resend.take(),
            )
        };
        let crashed = stop.is_none() && (!turns.is_empty() || exit_code != 0);
        let explained = live.state().explained;
        // A resume the agent could not open: the conversation starts again from Nexus's log and the message goes to it
        // once more, instead of failing every message from now on ([`resume_recovery`]).
        let resend = resend.filter(|_| {
            resume_recovery::unresumed(true, crashed, initialized, spawned, explained) && !turns.is_empty()
        });
        let mut reseeded = None;
        // An agent stopped before it was spawned read nothing: no turn was under way, every message is unanswered.
        let mid_turn = spawned && (!turns.is_empty() || cut_short);
        // The person's messages sent behind the turn under way: they show in the session but the agent never answered
        // them. A linked session's are not theirs to send again.
        let unanswered = lined
            + turns
                .iter()
                .skip(usize::from(spawned))
                // A refused steer's reserved turn has no sequence; `steering` already counts its message.
                .filter(|turn| turn.person && turn.seq.is_some())
                .count();
        let tail = (!stderr.is_empty()).then(|| {
            stderr
                .lines()
                .rev()
                .take(20)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        });
        let result: Result<()> = (|| {
            if self.live(thread_id).is_none_or(|current| !Arc::ptr_eq(&current, live)) {
                return Err("The session's original process has retired.".into());
            }
            for row in ended {
                self.append_from(thread_id, live, row)?;
            }
            for request_id in pending {
                self.append_from(
                    thread_id,
                    live,
                    WbEvent::ApprovalResolved {
                        request_id,
                        decision: Decision::Expired,
                        message: None,
                    },
                )?;
            }
            if let Some(turn) = turns.into_iter().next() {
                self.append_from(
                    thread_id,
                    live,
                    WbEvent::TurnEnd {
                        turn_id: turn.id,
                        // A resume that goes on from Nexus's copy did not fail: its message is sent again below.
                        status: if crashed && resend.is_none() {
                            TurnStatus::Error
                        } else {
                            TurnStatus::Interrupted
                        },
                        cost_usd: None,
                        duration_ms: turn.started.elapsed().as_millis() as u64,
                        error: None,
                        cursor,
                    },
                )?;
            }
            self.unread(thread_id, unanswered, "The agent stopped before it read")?;
            // A resume that goes on from Nexus's copy says so instead (`restart_unresumed`).
            if crashed && !explained && resend.is_none() {
                let agent = driver::label(live.agent);
                let text = if initialized {
                    format!("{agent} stopped unexpectedly (exit {exit_code}). Your next message resumes the session.")
                } else {
                    format!("{agent} could not start (exit {exit_code}).")
                };
                self.append_from(thread_id, live, WbEvent::Error { text, detail: tail })?;
            } else if stop == Some(StopReason::Quit) && mid_turn {
                self.append_from(
                    thread_id,
                    live,
                    WbEvent::Notice {
                        text: "Stopped when Nexus quit.".into(),
                    },
                )?;
            } else if stop == Some(StopReason::Restart) {
                self.append_from(thread_id, live, WbEvent::Notice { text: RESTARTED.into() })?;
            } else if background {
                // Nothing else in the log says the agent is gone (it was stopped between turns, say), and the
                // background work it reported would otherwise show as running for good.
                self.append_from(
                    thread_id,
                    live,
                    WbEvent::Notice {
                        text: BACKGROUND_ENDED.into(),
                    },
                )?;
            }
            let status = match (crashed, mid_turn) {
                _ if resend.is_some() => ThreadStatus::Idle,
                (true, _) if !initialized => ThreadStatus::Error,
                (_, true) => ThreadStatus::Interrupted,
                // It died between turns: the error shown above, not an idle session.
                (true, false) => ThreadStatus::Error,
                (false, false) => ThreadStatus::Idle,
            };
            // Changes the agent never answered: kept, so the next start runs with them, unless a newer line's mode
            // was saved already.
            let _status = live.status_lock();
            let changes = live.state().newer_changes(changes);
            self.change(thread_id, |record| {
                if let Some(unwritten) = unwritten {
                    unwritten.apply(record);
                }
                record.summary.status = status;
                record.summary.cost_usd = total;
                changes.into_iter().for_each(|change| change.apply(record));
            })?;
            if let Some(resend) = &resend {
                reseeded = Some(self.restart_unresumed(thread_id, resend.seq())?);
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("nexus-workbench: {thread_id}: {error}");
        }
        self.keep_transcript(thread_id, live);
        if self.unregister(thread_id, live) {
            self.run_ended(thread_id);
        }
        if let Some(session) = nexus_session {
            if let Err(error) = self.store().close_session(&session) {
                eprintln!("nexus-workbench: {thread_id}: {error}");
            }
            self.inner
                .host
                .ended(
                    session,
                    ProcessEnd {
                        exit_code,
                        signal,
                        duration_ms: started.elapsed().as_millis() as u64,
                    },
                )
                .await;
        }
        // Only once the conversation started again: a write that failed leaves the session as it was.
        if let (Some(resend), Some(seed)) = (resend, reseeded) {
            self.resend_unresumed(thread_id.to_string(), resend, seed).await;
        }
    }

    /// Says that the agent never read the person's last `count` messages, if any; `lead` says why ("The agent
    /// stopped before it read").
    fn unread(&self, thread_id: &str, count: usize, lead: &str) -> Result<()> {
        let text = match count {
            0 => return Ok(()),
            1 => format!("{lead} your last message. Send it again to continue."),
            n => format!("{lead} your last {n} messages. Send them again to continue."),
        };
        self.append(thread_id, WbEvent::Notice { text })?;
        Ok(())
    }

    /// Answers an approval. `suggestion` picks one of the agent's "always allow" suggestions (for `allowSession`);
    /// `answers` are the person's choices for an AskUserQuestion.
    pub fn approve(
        &self,
        thread_id: &str,
        request_id: &str,
        decision: Decision,
        message: Option<String>,
        suggestion: Option<usize>,
        answers: Option<Map<String, Value>>,
    ) -> Result<()> {
        let remember = match (decision, suggestion) {
            (Decision::AllowSession, Some(index)) => self.suggestion(thread_id, request_id, index)?,
            _ => None,
        };
        self.answer_approval(thread_id, request_id, decision, message, remember, answers)
    }

    /// [`Self::approve`], from an async caller. An agent whose answer to an ask carries no text (ACP) gets the reason
    /// given with a denial as the person's own queued message ([`Self::queue_denial_reason`]).
    pub async fn answer(
        &self,
        thread_id: &str,
        request_id: &str,
        decision: Decision,
        message: Option<String>,
        suggestion: Option<usize>,
        answers: Option<Map<String, Value>>,
    ) -> Result<()> {
        self.queue_denial_reason(thread_id, request_id, decision, message.as_deref())
            .await?;
        let (this, thread_id, request_id) = (self.clone(), thread_id.to_string(), request_id.to_string());
        tokio::task::spawn_blocking(move || {
            this.approve(&thread_id, &request_id, decision, message, suggestion, answers)
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// Queues `message`, the reason given with denying `request_id`, as the person's own message
    /// ([`Self::queue_message`]), when the session's agent cannot take it with the answer
    /// (`SessionFeatures::deny_reason_as_message`). It gets a `User` row and a queue card, and waits behind the
    /// messages queued before it. Queued before the answer goes out, so its turn is counted while the denied turn still
    /// runs: the session never shows idle between the two.
    pub(crate) async fn queue_denial_reason(
        &self,
        thread_id: &str,
        request_id: &str,
        decision: Decision,
        message: Option<&str>,
    ) -> Result<()> {
        let Some(reason) = message.filter(|m| !m.trim().is_empty()) else {
            return Ok(());
        };
        let Some(live) = self.live(thread_id) else {
            return Ok(());
        };
        if decision != Decision::Deny
            || !harness::harness(live.agent).session_features().deny_reason_as_message
            || !live.state().pending.contains_key(request_id)
        {
            return Ok(());
        }
        self.queue_message(thread_id, reason, vec![]).await
    }

    /// [`Self::approve`] with the raw rule to remember, picked already: a phone's "Allow for this session" picks it on
    /// the computer (`session_sync/control/answer.rs`).
    pub(crate) fn answer_approval(
        &self,
        thread_id: &str,
        request_id: &str,
        decision: Decision,
        message: Option<String>,
        mut remember: Option<Value>,
        answers: Option<Map<String, Value>>,
    ) -> Result<()> {
        let live = self
            .live(thread_id)
            .ok_or("The agent is no longer running; send a message to resume it.")?;
        let Some(Asked { input, .. }) = live.state().pending.remove(request_id) else {
            return Err("That request was already answered, or the agent withdrew it.".into());
        };
        if decision != Decision::AllowSession {
            remember = None;
        }
        // A linked session's "allow for this session" never lifts it above the sessions it was started under: the
        // call is allowed once, and the mode stays. A mode Nexus does not know is never passed on: the agent would
        // switch to it while Nexus shows the old one.
        match remember.as_ref().map(suggested_mode) {
            Some(Suggested::Mode(mode)) if self.check_mode(thread_id, mode).is_err() => remember = None,
            Some(Suggested::UnknownMode) => remember = None,
            _ => {}
        }
        if let Some(rule) = remember.take() {
            // Ancestors that can't be read: allowed once.
            remember = self
                .mode_ceiling_of(thread_id)
                .ok()
                .and_then(|ceiling| rule_under_ancestors(rule, ceiling));
        }
        let say = match decision {
            // An agent whose answer carries no text got the reason as the person's message (`queue_denial_reason`).
            Decision::Deny | Decision::Expired => Say::Deny {
                request_id: request_id.into(),
                message: message
                    .clone()
                    .filter(|m| !m.trim().is_empty())
                    .filter(|_| !harness::harness(live.agent).session_features().deny_reason_as_message),
            },
            Decision::Allow | Decision::AllowSession => Say::Allow {
                request_id: request_id.into(),
                input,
                answers,
                remember: remember.clone(),
            },
        };
        let (n, outs) = live.tell_if(say, |_, _| true).unwrap_or_default();
        self.outs(thread_id, &live, outs)?;
        // "Allow everything for this session" and its kind switch the mode: the session shows the one now in force,
        // unless a line sent after this answer switched it again (a private formal model M1).
        if let Some(Suggested::Mode(mode)) = remember.as_ref().map(suggested_mode) {
            {
                let _status = live.status_lock();
                if live.state().newer(n, &Change::Mode(mode)) {
                    self.change(thread_id, |record| record.summary.permission_mode = mode)?;
                }
            }
            // A switch down takes every descendant down with it.
            self.lower_descendants(thread_id, Some(mode), false);
        }
        self.append(
            thread_id,
            WbEvent::ApprovalResolved {
                request_id: request_id.into(),
                decision,
                message,
            },
        )?;
        self.refresh_status(thread_id, &live)?;
        Ok(())
    }

    /// Saves `change`, from the line numbered `n`, which the agent accepted. Saved only now: the session never shows a
    /// mode the running agent does not have. Not over a mode an answer sent after it saved already: the agent read
    /// that one last.
    fn accepted(&self, thread_id: &str, live: &Live, n: u64, change: Change) -> Result<()> {
        let mode = if let Change::Mode(mode) = change {
            Some(mode)
        } else {
            None
        };
        let status = live.status_lock();
        if live.state().newer(n, &change) {
            // An accepted setting does not report its resolved runtime value. Drop the older
            // observation until a new init or model call reports it; keep the saved choice separate.
            {
                let mut state = live.state();
                match &change {
                    Change::Model(_) => state.connection.model = None,
                    Change::Effort(_) => state.connection.effort = None,
                    // The next model call's `init` reports whether it serves.
                    Change::Fast(_) => state.connection.fast_off = None,
                    Change::Mode(_) => state.connection.permissions = None,
                }
            }
            self.change(thread_id, |record| change.apply(record))?;
            self.publish_connection(thread_id, live)?;
            drop(status);
            if let Some(mode) = mode {
                self.allow_pending(thread_id, live, mode)?;
            }
        }
        Ok(())
    }

    /// Allows each waiting ask that the workbench answers in `mode`, the session's new mode
    /// ([`driver::answers_ask`]). It allows no coding agent's ask, and no question or plan.
    fn allow_pending(&self, thread_id: &str, live: &Live, mode: PermissionMode) -> Result<()> {
        let requests: Vec<String> = live
            .state()
            .pending
            .iter()
            .filter(|(_, asked)| driver::answers_ask(live.agent, mode, &asked.tool))
            .map(|(id, _)| id.clone())
            .collect();
        for request_id in requests {
            self.approve(thread_id, &request_id, Decision::Allow, None, None, None)?;
        }
        Ok(())
    }

    /// The raw suggestion `index` of the approval `request_id`, from the log.
    fn suggestion(&self, thread_id: &str, request_id: &str, index: usize) -> Result<Option<Value>> {
        Ok(self
            .approval_suggestions(thread_id, request_id)?
            .get(index)
            .map(|s| s.raw.clone()))
    }

    /// The "always allow" suggestions of the approval `request_id`, from the log; none when it is not there.
    pub(crate) fn approval_suggestions(
        &self,
        thread_id: &str,
        request_id: &str,
    ) -> Result<Vec<event::PermissionSuggestion>> {
        let events = self.reader().events_after(thread_id, 0)?;
        for stored in events.into_iter().rev() {
            if let WbEvent::Approval {
                request_id: id,
                suggestions,
                ..
            } = stored.event
                && id == request_id
            {
                return Ok(suggestions);
            }
        }
        Ok(Vec::new())
    }

    /// Stops the current turn: the person's Interrupt. The agent keeps the conversation; if it does not end the turn in
    /// time, it is stopped. The session is halted until the person writes to it ([`linked::halts`]): it takes no
    /// result message, and a turn its agent starts on its own ends at once. The sessions below it are untouched.
    ///
    /// Only the turn under way stops. The person's message queued behind it runs as the next turn, and its turn lifts
    /// the halt as it begins; a second Interrupt stops that turn too.
    pub fn interrupt(&self, thread_id: &str) -> Result<()> {
        // The turn is interrupted whether or not the halt could be saved; that error is said after.
        let halted = self.halt_for_interrupt(thread_id);
        self.stop_turn(thread_id, InterruptSource::Person)?;
        self.emit_summary(thread_id);
        halted
    }

    /// Interrupt the current turn. A spend trip targets only the automatic turn that exceeded its cap.
    fn stop_turn(&self, thread_id: &str, source: InterruptSource) -> Result<()> {
        let Some(live) = self.live(thread_id) else {
            return Ok(());
        };
        // The person's message queued behind the last turn is held for that turn's checkpoint: the turn the person meant
        // to stop has ended. Their message runs, its stream untouched, and its turn lifts the halt as it begins, as it
        // does at the turn's end (`Out::Result`). Any other interrupt, and the person's on a message sent after that turn
        // ended, goes out right after the message (below), so it still stops the turn the message starts.
        {
            let state = live.state();
            if !source.applies_to(state.turns.front()) {
                return Ok(());
            }
            // Not when the run is stopping: that message never runs, and a Stop's halt must hold.
            if matches!(source, InterruptSource::Person)
                && state.spawned
                && state.stop.is_none()
                && !state.done
                && state.releasing
                && state.release_queued
                && state.turns.front().is_some_and(|turn| turn.person)
            {
                self.person_turn_begins(thread_id);
                return Ok(());
            }
        }
        live.partial.lock().unwrap_or_else(|p| p.into_inner()).stop();
        // Its wait ends, and what that carried goes back (linked `interrupt`). A result message's turn settles at
        // its end: a queued message may still carry results in front of it.
        self.cut_wait(thread_id);
        let (turn, spawned, held) = {
            let mut state = live.state();
            if !source.applies_to(state.turns.front()) {
                return Ok(());
            }
            let turn = state.turns.front().map(|t| t.id.clone());
            let held = state.spawned && turn.is_some() && state.releasing;
            if held && (state.interrupt_after_release.is_none() || matches!(source, InterruptSource::Person)) {
                state.interrupt_after_release = Some(source.clone());
            }
            state.spend_notice_pending |= matches!(source, InterruptSource::Spend(_));
            (turn, state.spawned, held)
        };
        if matches!(source, InterruptSource::Spend(_))
            && let Err(error) = self.append_from(
                thread_id,
                &live,
                WbEvent::Notice {
                    text: linked::spend::STOP_NOTICE.into(),
                },
            )
        {
            eprintln!("nexus-workbench: {thread_id}: {error}");
        }
        // Still starting: the agent has not read the message (it may not even be recorded yet), and would only get the
        // interrupt after it. Cancel the launch instead.
        if !spawned {
            self.stop_live(&live, StopReason::Stop, None);
            return Ok(());
        }
        let Some(turn) = turn else {
            return Ok(());
        };
        if !held {
            source.interrupt(&live);
        }
        let this = self.clone();
        let grace = self.config().interrupt_grace;
        tokio::spawn(async move {
            tokio::time::sleep(grace).await;
            this.stop_live(&live, StopReason::Stop, Some((&turn, &source)));
        });
        Ok(())
    }

    /// Stops one task of the running agent: a sub-agent or a background command (`wb_task_stop`). Claude Code also stops
    /// its foreground sub-agents and commands, and lets its background sub-agents run on; Codex ends a child's turn, and
    /// its command under way runs on. The task's own `stopped` row confirms the stop.
    pub async fn task_stop(&self, thread_id: &str, task_id: &str) -> Result<()> {
        self.ask_about_task(thread_id, TaskAsk::Stop(task_id)).await.map(|_| ())
    }

    /// Moves a running foreground command (its tool call) to the background (`wb_task_background`): Claude Code's
    /// `background_tasks`. Its start row then reaches the log (agents.rs), and its tool call ends with a receipt. False:
    /// the agent did not move it.
    pub async fn task_background(&self, thread_id: &str, tool_use_id: &str) -> Result<bool> {
        let answer = self.ask_about_task(thread_id, TaskAsk::Background(tool_use_id)).await?;
        Ok(answer.get("backgrounded").and_then(Value::as_bool).unwrap_or(false))
    }

    /// The latest output of a background command, read now and never stored (`wb_task_output`): Claude Code's
    /// `get_task_output`, cut to its last [`TASK_OUTPUT_MAX`] bytes.
    pub async fn task_output(&self, thread_id: &str, task_id: &str) -> Result<TaskOutput> {
        let answer = self.ask_about_task(thread_id, TaskAsk::Output(task_id)).await?;
        let output = answer.get("output").and_then(Value::as_str).unwrap_or_default();
        let total_bytes = answer
            .get("total_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(output.len() as u64);
        let cut = tail(output, TASK_OUTPUT_MAX);
        Ok(TaskOutput {
            truncated: answer.get("truncated").and_then(Value::as_bool).unwrap_or(false) || cut.len() < output.len(),
            output: cut.to_string(),
            total_bytes,
        })
    }

    /// Asks the running agent about one task, and waits for its answer. Refused without asking the agent when none runs
    /// or it is stopping, when its version or kind of task has no such control, or when the task is not in the state the
    /// control needs: Claude Code answers success for an ended or unknown task. A refusal from the agent is the error;
    /// for a control that acts (stop, background) it is also a notice in the log. No answer in time is an error with no
    /// notice, since nothing says the agent refused.
    async fn ask_about_task(&self, thread_id: &str, ask: TaskAsk<'_>) -> Result<Value> {
        let live = self.live(thread_id).ok_or("The agent is not running.")?;
        self.ask_about_task_on(thread_id, &live, ask).await
    }

    /// A spend trip controls this exact run, even if another process starts meanwhile.
    async fn ask_about_task_on(&self, thread_id: &str, live: &Live, ask: TaskAsk<'_>) -> Result<Value> {
        let not_running = || "The agent is not running.".to_string();
        let label = driver::label(live.agent);
        let request = live.next_request();
        let (asked, answer) = oneshot::channel();
        let title = {
            let mut state = live.state();
            if state.done || !state.spawned {
                return Err(not_running());
            }
            // Its input is closing: a line said now would never reach it.
            if state.stop.is_some() {
                return Err("The agent is stopping.".into());
            }
            let Some(capabilities) = state.capabilities.clone().filter(|capabilities| {
                !capabilities.stop.is_empty() || !capabilities.background.is_empty() || !capabilities.output.is_empty()
            }) else {
                return Err(match ask {
                    TaskAsk::Stop(_) => format!("This version of {label} can't stop one task."),
                    TaskAsk::Background(_) => {
                        format!("This version of {label} can't move a command to the background.")
                    }
                    TaskAsk::Output(_) => format!("This version of {label} can't read a command's output."),
                });
            };
            let (kinds, kind, title) = match ask {
                TaskAsk::Stop(task) | TaskAsk::Output(task) => {
                    let Some((kind, title)) = state.book.running(task) else {
                        return Err("That task is not running.".into());
                    };
                    let kinds = if matches!(ask, TaskAsk::Stop(_)) {
                        &capabilities.stop
                    } else {
                        &capabilities.output
                    };
                    (kinds, kind, title)
                }
                TaskAsk::Background(call) => {
                    let Some(title) = state.book.foreground(call) else {
                        return Err("That command is not running in the foreground.".into());
                    };
                    (&capabilities.background, "shell", title)
                }
            };
            if !kinds.iter().any(|accepted| accepted == kind) {
                return Err(match ask {
                    TaskAsk::Stop(_) => format!("{label} can't stop this kind of task."),
                    TaskAsk::Background(_) => format!("{label} can't move a command to the background."),
                    TaskAsk::Output(_) => format!("{label} can't read the output of this kind of task."),
                });
            }
            let title = title.to_string();
            state.task_asks.insert(request.clone(), asked);
            title
        };
        let say = match ask {
            TaskAsk::Stop(task) => Say::TaskStop {
                request: request.clone(),
                task_id: task.into(),
            },
            TaskAsk::Background(call) => Say::TaskBackground {
                request: request.clone(),
                tool_use_id: call.into(),
            },
            TaskAsk::Output(task) => Say::TaskOutput {
                request: request.clone(),
                task_id: task.into(),
            },
        };
        // A refusal the protocol makes itself (a Codex child between its turns) comes back at once.
        for out in live.tell(say) {
            if let Out::Answered {
                request_id,
                result: Err(error),
            } = out
                && request_id == request
            {
                live.state().task_asks.remove(&request);
                return Err(error);
            }
        }
        match tokio::time::timeout(TASK_ASK_WAIT, answer).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(error))) => {
                let what = if title.is_empty() {
                    "the task".to_string()
                } else {
                    format!("“{title}”")
                };
                let text = match ask {
                    TaskAsk::Stop(_) => Some(format!("{label} could not stop {what}: {error}")),
                    TaskAsk::Background(_) => Some(format!("{label} could not move {what} to the background: {error}")),
                    TaskAsk::Output(_) => None,
                };
                if let Some(text) = text {
                    self.append(thread_id, WbEvent::Notice { text })?;
                }
                Err(error)
            }
            Ok(Err(_)) => Err("The agent stopped.".into()),
            Err(_) => {
                live.state().task_asks.remove(&request);
                Err(format!("{label} did not answer."))
            }
        }
    }

    /// Ends the agent process: the person's Stop. The session stays, and resumes on the next message. It and every
    /// session below it are halted, then each running one stops and each still starting ends as stopped, whether or not
    /// this one runs ([`linked::halts`]). A halt or a row below it that can't be written or read still leaves every
    /// process it reaches stopped, its own at least, and is said after. Idle, Move, Restart and Quit stop a process
    /// without this.
    pub fn stop(&self, thread_id: &str) -> Result<()> {
        let stopped = self.stop_tree(thread_id, Some(linked::Halt::Person));
        // Each halt shows on its session's chip: one that was not running changes no status, so nothing else shows it.
        self.emit_tree(thread_id);
        stopped.map(|_| ())
    }

    /// Restarts the agent: ends its process as [`Self::stop`] does, so the next message starts a new one that resumes
    /// the conversation and reads the MCP servers, skills and settings again. A notice in the log says so: after the
    /// turn's end when the agent was running, at once when it was not.
    pub fn restart(&self, thread_id: &str) -> Result<()> {
        let ending = match self.live(thread_id) {
            // Already stopping (a restart, a stop, either while running or still launching): the process is ending
            // anyway, and the first restart's notice, or the stop's own, is enough. Checked and claimed under one lock
            // in `stop_live`, so two Restarts at the same moment still add one notice.
            Some(live) => match self.stop_live(&live, StopReason::Restart, None) {
                None => return Ok(()),
                Some(ending) => ending,
            },
            // Not running: the notice goes in now.
            None => false,
        };
        // Still launching: stopped as usual (that end records nothing), and the notice goes in now.
        if !ending {
            self.append(thread_id, WbEvent::Notice { text: RESTARTED.into() })?;
        }
        Ok(())
    }

    /// `None`: a stop was already under way or the escalation's turn ended. `Some(true)`: this call ends a running
    /// agent, and its end is still to be recorded (with `reason`). `Some(false)`: this call stopped an agent still
    /// launching, which records nothing (a restart then is a plain stop).
    fn stop_live(
        &self,
        live: &Arc<Live>,
        reason: StopReason,
        escalation: Option<(&str, &InterruptSource)>,
    ) -> Option<bool> {
        let stopping = {
            let mut state = live.state();
            if let Some((turn_id, source)) = escalation {
                if state.reaped || state.done {
                    return None;
                }
                // Check under the lock that settles the result. A later turn may already be running.
                state
                    .turns
                    .front()
                    .filter(|turn| turn.id == turn_id && source.applies_to(Some(turn)))?;
            }
            // No call from this run is taken from here on, even if shutdown is already under way.
            state.grant = None;
            if state.spawned
                && !state.reaped
                && !state.done
                && matches!(reason, StopReason::Stop | StopReason::Restart | StopReason::Quit)
                && let Some(turn) = state.turns.front_mut()
            {
                turn.stopping = true;
            }
            if state.stop.is_some() {
                None
            } else {
                state.stop = Some(if reason == StopReason::Restart && !state.spawned {
                    StopReason::Stop
                } else {
                    reason
                });
                Some((state.spawned, !state.turns.is_empty(), state.local_run.take()))
            }
        };
        live.partial.lock().unwrap_or_else(|p| p.into_inner()).stop();
        let (spawned, busy, run) = stopping?;
        drop(run);
        // Still starting: `start` sees the stop before it spawns the agent, or as it stores the pid, and ends the run.
        if !spawned {
            return Some(false);
        }
        if busy {
            let _ = live.tell(Say::Interrupt {
                request: live.next_request(),
            });
        }
        let _ = live.tx.send(Input::Close);
        let mut exited = live.exited.clone();
        let live = live.clone();
        tokio::spawn(async move {
            // `exited` also waits for Gateway to hear the session ended; `reaped` is the process alone, and a reaped
            // pid is never signalled.
            if tokio::time::timeout(process::GRACE, exited.wait_for(|done| *done))
                .await
                .is_ok()
                || live.state().reaped
            {
                return;
            }
            let pid = live.state().pid;
            process::terminate(pid);
            if tokio::time::timeout(process::TERM_GRACE, exited.wait_for(|done| *done))
                .await
                .is_ok()
                || live.state().reaped
            {
                return;
            }
            process::kill(pid);
        });
        Some(true)
    }

    /// Refuses `name` unless it is a valid session name (session_name.rs) that no session but `own` holds.
    pub fn check_name(&self, name: &str, own: Option<&str>) -> Result<()> {
        session_name::check(name)?;
        match self.store().thread_named(name)? {
            Some(holder) if Some(holder.as_str()) != own => Err(session_name::taken(name, &holder)),
            _ => Ok(()),
        }
    }

    pub fn update(&self, thread_id: &str, patch: ThreadPatch) -> Result<ThreadSummary> {
        let gate = self.settings_gate(thread_id);
        let _guard = gate.lock().unwrap_or_else(|p| p.into_inner());
        self.update_inner(thread_id, patch)
    }

    fn settings_gate(&self, id: &str) -> Arc<Mutex<()>> {
        let mut gates = self.inner.settings_gates.lock().unwrap_or_else(|p| p.into_inner());
        gates.retain(|_, gate| gate.strong_count() > 0);
        let gate = gates.get(id).and_then(std::sync::Weak::upgrade).unwrap_or_default();
        gates.insert(id.to_string(), Arc::downgrade(&gate));
        gate
    }

    fn update_inner(&self, thread_id: &str, patch: ThreadPatch) -> Result<ThreadSummary> {
        let mut patch = patch;
        let before = self.record(thread_id)?.summary;
        if let Some(model) = &patch.model
            && let Some(provider) = &before.provider
            && before.agent.coding() == Some(nexus_agents::Agent::Opencode)
        {
            let models = self.inner.host.saved_server_models(provider)?;
            if !model
                .as_ref()
                .is_some_and(|model| !model.is_empty() && models.contains(model))
            {
                return Err("Choose a model saved for this server.".into());
            }
            if model == &before.model {
                patch.model = None;
            } else if self.is_live(thread_id) {
                // OpenCode gets a launch-scoped alias for the chosen server model.
                return Err("Stop this session before changing its server model.".into());
            }
        }
        // Only marks: a pin, Continue at reset, a snooze or "Keep in list" (or their undoing). The session keeps its
        // place among the others, by when it was last active.
        let only_marks = matches!(
            &patch,
            ThreadPatch {
                title: None,
                model: None,
                permission_mode: None,
                effort: None,
                fast: None,
                archived: None,
                orchestration: None,
                pinned: _,
                continue_at_reset: _,
                snoozed_until: _,
                keep_listed: _,
                settled: _,
                name: _,
            }
        ) && (patch.pinned.is_some()
            || patch.continue_at_reset.is_some()
            || patch.snoozed_until.is_some()
            || patch.keep_listed.is_some()
            || patch.settled.is_some()
            || patch.name.is_some());
        if patch.continue_at_reset == Some(true)
            && self
                .record(thread_id)?
                .summary
                .limited
                .is_none_or(|stop| stop.resets_at.is_none())
        {
            return Err("This session is not waiting for a plan's usage limit to reset.".into());
        }
        // A linked session never runs above the sessions it was started under.
        let lowered = patch.permission_mode;
        if let Some(mode) = lowered {
            self.check_mode(thread_id, mode)?;
        }
        let effort = patch.effort;
        if let Some(effort) = effort {
            self.check_effort(thread_id, effort)?;
        }
        if patch.orchestration == Some(true) {
            self.check_orchestration(thread_id)?;
        }
        if patch.fast == Some(true) {
            let summary = self.record(thread_id)?.summary;
            if !driver::fast_allowed(summary.agent, summary.via) {
                return Err("Fast mode is for Claude Code, and for Codex on your ChatGPT plan.".into());
            }
        }
        if let Some(Some(name)) = &patch.name {
            self.check_name(name, Some(thread_id))?;
        }
        // A running agent gets the change first; the session saves it once the agent accepts it (see `Out::Ack`), or
        // when the agent ends without answering (`finish`). An agent on its way out gets nothing: saved now.
        if let Some(live) = self.live(thread_id) {
            if let Some(mode) = patch.permission_mode
                && self.tell_change(thread_id, &live, Change::Mode(mode), |request| Say::Mode {
                    request,
                    mode,
                })?
            {
                patch.permission_mode = None;
            }
            if let Some(model) = patch.model.clone() {
                let model = model.filter(|m| !m.is_empty());
                // The session saves the policy id; the running agent gets its own name for the model, as at launch.
                let say = |request| Say::Model {
                    request,
                    model: model.as_deref().map(|m| live.agent.native_model(m)),
                };
                if self.tell_change(thread_id, &live, Change::Model(model.clone()), say)? {
                    patch.model = None;
                }
            }
            if let Some(effort) = patch.effort
                && self.tell_change(thread_id, &live, Change::Effort(effort), |request| Say::Effort {
                    request,
                    effort,
                })?
            {
                patch.effort = None;
            }
            if let Some(on) = patch.fast
                && self.tell_change(thread_id, &live, Change::Fast(on), |request| Say::Fast { request, on })?
            {
                patch.fast = None;
            }
        }
        let renamed = patch.name.clone().flatten();
        let archiving = patch.archived == Some(true) && !before.archived;
        let snoozing =
            matches!(patch.snoozed_until, Some(Some(until)) if until > now_ms()) && !snooze::snoozed(&before, now_ms());
        let settling = patch.settled == Some(true) && before.aside.settled_at.is_none();
        let summary = self.change_at(thread_id, !only_marks, |record| {
            if let Some(title) = patch.title.filter(|t| !t.trim().is_empty()) {
                record.summary.title = title.trim().to_string();
                record.automatic_title = false;
            }
            if let Some(model) = patch.model {
                record.summary.model = model.filter(|m| !m.is_empty());
            }
            if let Some(mode) = patch.permission_mode {
                record.summary.permission_mode = mode;
            }
            if let Some(effort) = patch.effort {
                record.summary.effort = effort;
            }
            if let Some(on) = patch.fast {
                record.summary.fast = on;
            }
            if let Some(archived) = patch.archived {
                record.summary.archived = archived;
                // Archived, it never continues by itself.
                if archived {
                    record.summary.limited = None;
                }
            }
            if let Some(on) = patch.continue_at_reset
                && let Some(stop) = record.summary.limited.as_mut()
            {
                stop.scheduled = on && stop.resets_at.is_some();
            }
            if let Some(orchestration) = patch.orchestration {
                record.summary.orchestration = orchestration;
            }
            if let Some(pinned) = patch.pinned {
                record.summary.pinned = pinned;
            }
            if let Some(until) = patch.snoozed_until {
                record.summary.aside.snoozed_until = until;
            }
            match patch.settled {
                Some(true) if record.summary.aside.settled_at.is_none() => {
                    record.summary.aside.settled_at = Some(now_ms());
                    record.summary.aside.keep_listed = false;
                }
                Some(false) => record.summary.aside.settled_at = None,
                _ => {}
            }
            if let Some(keep) = patch.keep_listed {
                record.summary.aside.keep_listed = keep;
                if keep {
                    record.summary.aside.settled_at = None;
                }
            }
            if let Some(name) = patch.name {
                record.summary.name = name;
            }
        })?;
        if archiving {
            self.on_archived(&summary);
        }
        // Keyed by when it settled, as a merge's settle (`settle_merged_at`).
        if let Some(at) = summary.aside.settled_at.filter(|_| settling) {
            self.fire_hooks(hooks::Fire::plain(hooks::HookEvent::Settled, thread_id, at.to_string()));
        }
        if let Some(until) = summary.aside.snoozed_until.filter(|_| snoozing) {
            self.fire_hooks(hooks::Fire::plain(
                hooks::HookEvent::Snoozed,
                thread_id,
                until.to_string(),
            ));
        }
        // Every descendant comes down with it: stored at once, a running one's through its agent.
        if lowered.is_some() || effort.is_some() {
            self.lower_descendants(thread_id, lowered, effort.is_some());
        }
        self.forward_name(thread_id, renamed);
        Ok(summary)
    }

    /// A name the store accepted reaches a running Claude Code as its own `/rename` (`Say::Rename`): no turn, no cost.
    /// Only an idle, initialized process gets it, since a turn would take the line in as a message. A busy one gets it
    /// when its turn ends ([`Workbench::forward_due_name`]); a process not running gets it as `--name` at its next
    /// start. A cleared name is never sent: what Claude Code does with an empty `/rename` is unmeasured.
    fn forward_name(&self, thread_id: &str, name: Option<String>) {
        let Some((name, live)) = name.zip(self.live(thread_id)) else {
            return;
        };
        if !harness::harness(live.agent).session_features().live_rename {
            return;
        }
        let ready = {
            let mut state = live.state();
            let ready = state.initialized && state.idle();
            // Ready: this send covers any name still due. Busy: the turn's end sends the name stored then.
            state.name_due = !ready;
            ready
        };
        if ready && let Err(error) = self.release(thread_id, &live, None, Say::Rename { name }) {
            eprintln!("nexus-workbench: {thread_id}: the name did not reach the agent: {error}");
        }
    }

    /// Sends a name accepted while the agent was busy, once it is idle: the name stored now, so of several renames
    /// during the turn only the last goes out, and a name cleared meanwhile sends nothing. Still busy (the next
    /// turn began at once), it waits for that turn's end.
    fn forward_due_name(&self, thread_id: &str, live: &Live) {
        let due = {
            let mut state = live.state();
            let due = state.name_due && state.initialized && state.idle();
            if due {
                state.name_due = false;
            }
            due
        };
        if !due {
            return;
        }
        let Some(name) = self.record(thread_id).ok().and_then(|record| record.summary.name) else {
            return;
        };
        if let Err(error) = self.release(thread_id, live, None, Say::Rename { name }) {
            eprintln!("nexus-workbench: {thread_id}: the name did not reach the agent: {error}");
        }
    }

    fn on_archived(&self, summary: &ThreadSummary) {
        self.drop_checkpoints(summary);
        self.forget_transcripts(&summary.id);
        self.fire_hooks(hooks::Fire::plain(
            hooks::HookEvent::Archived,
            &summary.id,
            summary.updated_at.to_string(),
        ));
    }

    /// Deletes an archived session's checkpoint refs from its project's repository (a worktree shares them), so they
    /// don't pile up in the person's repo. On a thread of its own, not `spawn_blocking`: the app archives from a sync
    /// command on the main thread, which has no async runtime. A failure only leaves refs behind.
    fn drop_checkpoints(&self, summary: &ThreadSummary) {
        let Ok(Some(project)) = self.store().project(&summary.project_id) else {
            return;
        };
        let prefix = checkpoint_prefix(&summary.id);
        let pending = self.pending_checkpoint(&summary.id);
        // Nothing reads the entry once archived: `checkpoint_after` skips archived sessions, and an unarchived one
        // writes a fresh entry with its next turn's checkpoint.
        self.checkpoints().remove(&summary.id);
        std::thread::spawn(move || {
            // The last turn's checkpoint, if still being written, would land after the drop and stay (R5).
            if let Some(written) = pending {
                let deadline = Instant::now() + CHECKPOINT_WAIT;
                while !*written.borrow() && written.has_changed().is_ok() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            // Any repository will do, a detached HEAD too: a worktree session's refs live in the project's repo.
            let repo = Path::new(&project.path);
            if git::in_repo(repo)
                && let Err(error) = git::drop_refs(repo, &prefix)
            {
                eprintln!("nexus-workbench: dropping {prefix}: {error}");
            }
        });
    }

    /// Sends `change` to the agent, to be saved when it answers. False when the agent is stopping or ending: it would
    /// never read it, so the caller saves the change at once.
    fn tell_change(
        &self,
        thread_id: &str,
        live: &Live,
        change: Change,
        say: impl FnOnce(String) -> Say,
    ) -> Result<bool> {
        let request = live.next_request();
        // Stored with its line's number before the line goes out: the agent's answer can come back at once.
        let told = live.tell_if(say(request.clone()), |state, n| {
            let ready = !state.done && state.stop.is_none();
            if ready {
                state.changes.insert(request, (n, change));
            } else if matches!(change, Change::Mode(_)) {
                // The caller saves this mode at once, so it takes a number: an older, unanswered mode (a pick sent
                // before the stop began) is then not saved over it when the agent exits.
                state.said = n;
                state.mode_said = n;
            }
            ready
        });
        match told {
            Some((_, outs)) => {
                self.outs(thread_id, live, outs)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// "Continue where you left off." after an interruption.
    pub async fn resume(&self, thread_id: &str) -> Result<()> {
        self.send(thread_id, CONTINUE).await
    }

    /// Stops agents that have had nothing to do for `idle_after`. Call it once a minute. Work the agent left running
    /// in the background keeps it, unless nothing at all was heard from it for [`BACKGROUND_IDLE_FACTOR`] times as long
    /// (a task whose end was never reported must not hold a process forever). So do linked sessions at work, or results
    /// it has not read. It also offers every running session its children's results again ([`linked::results`]).
    pub fn reap_idle(&self) {
        let idle_after = self.config().idle_after;
        self.sweep_links();
        for (id, live) in self.all_live() {
            let (idle, quiet) = {
                let state = live.state();
                let quiet = state.last_activity.elapsed();
                let idle = state.turns.is_empty() && state.pending.is_empty() && quiet >= idle_after;
                (
                    idle && (state.background.is_empty() || quiet >= idle_after * BACKGROUND_IDLE_FACTOR),
                    quiet,
                )
            };
            // A parent whose children are at work, or whose results it has not read, or a session with a watched
            // terminal command still running, gets the background-work time: results and notices go into its live run.
            if idle
                && (quiet >= idle_after * BACKGROUND_IDLE_FACTOR
                    || (!self.holds_crew(&id) && !self.inner.terminals.has_watched_running(&self.config().scope, &id)))
            {
                self.stop_live(&live, StopReason::Idle, None);
            }
        }
    }

    /// Whether an agent process is running for the session.
    pub fn is_live(&self, thread_id: &str) -> bool {
        self.live(thread_id).is_some()
    }

    /// The scopes (organizations) with an agent running now, the current one or not. An agent keeps the token it
    /// started with, so the app revokes a token only once its scope is gone from here.
    pub fn live_scopes(&self) -> BTreeSet<String> {
        self.all_live()
            .into_iter()
            .map(|(_, live)| live.scope.clone())
            .collect()
    }

    /// Whether agents have unfinished work in any organization, including background tasks and owed turns, or a
    /// shared-terminal command still runs.
    pub fn has_active_work(&self) -> bool {
        self.all_live().iter().any(|(_, live)| {
            let state = live.state();
            !state.done && state.has_active_work()
        }) || self.terminals_running()
            || self.inner.update_activities.load(std::sync::atomic::Ordering::SeqCst) != 0
    }

    /// Reserve an update restart when Code has no active work, across all organizations.
    /// Idle processes may remain: shut them down before installing. Holding `live` and each state closes the race
    /// with agent launches, messages, compaction and automatic turns (`begin_turn_if`).
    pub fn reserve_update_restart(&self) -> Result<()> {
        let lives = self.inner.live.lock().unwrap_or_else(|p| p.into_inner());
        let states: Vec<_> = lives.values().map(|live| live.state()).collect();
        let active = states.iter().any(|state| state.has_active_work());
        if active || self.inner.update_activities.load(std::sync::atomic::Ordering::SeqCst) != 0 {
            return Err("Code sessions are still running. Finish or stop them, then choose Restart now again.".into());
        }
        self.inner
            .update_restart
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    pub fn cancel_update_restart(&self) {
        self.inner
            .update_restart
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// The app is quitting: every agent is asked to stop, and this waits until they have (or were killed).
    pub async fn shutdown(&self) {
        self.inner.terminals.clear();
        self.inner.session_sync_stop.send_replace(true);
        let lives = self.all_live();
        for (_, live) in &lives {
            self.stop_live(live, StopReason::Quit, None);
        }
        let deadline = process::GRACE + process::TERM_GRACE + Duration::from_secs(1);
        let waits = lives.into_iter().map(|(_, live)| {
            let mut exited = live.exited.clone();
            async move {
                let _ = tokio::time::timeout(deadline, exited.wait_for(|done| *done)).await;
            }
        });
        futures::future::join_all(waits).await;
    }
}

/// Whether a stream's latest text should reach the screen at `now`: at most once every 50 ms per stream (`parent`: the
/// sub-agent's tool call, `None` for the main answer). Records the update when it is due.
fn delta_due(last: &mut HashMap<Option<String>, Instant>, parent: &Option<String>, now: Instant) -> bool {
    let due = last
        .get(parent)
        .is_none_or(|at| now.saturating_duration_since(*at) >= Duration::from_millis(50));
    if due {
        last.insert(parent.clone(), now);
    }
    due
}

/// Model calls' cost and context not yet written to the session's summary.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Unwritten {
    /// The session's total cost after the latest call.
    total: f64,
    /// The latest main-agent call's context (a sub-agent's calls leave it as it was).
    context: Option<u64>,
}

impl Unwritten {
    fn apply(self, record: &mut ThreadRecord) {
        record.summary.cost_usd = self.total;
        if let Some(context) = self.context {
            record.summary.context_tokens = Some(context);
        }
    }
}

/// A streamed message as the screen keys its draft: the message id and the sub-agent call it belongs to, if any.
type StreamKey = (String, Option<String>);

/// What the screen has of a streamed message: its text, and that text's length in UTF-16 units (JavaScript's).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Sent {
    text: String,
    units: usize,
}

/// What a delta carries for a message's text so far: the part after what was sent before, with where it goes (in
/// UTF-16 units), or the whole text (offset None) the first time, or when the text does not start with what was sent.
fn delta_suffix(
    streamed: &mut HashMap<StreamKey, Sent>,
    id: &str,
    parent: &Option<String>,
    text: String,
) -> (String, Option<u64>) {
    let key = (id.to_string(), parent.clone());
    match streamed.get_mut(&key) {
        Some(sent) if text.starts_with(&sent.text) => {
            let offset = sent.units as u64;
            let suffix = text[sent.text.len()..].to_string();
            sent.text.push_str(&suffix);
            sent.units += suffix.encode_utf16().count();
            (suffix, Some(offset))
        }
        _ => {
            let units = text.encode_utf16().count();
            streamed.insert(
                key,
                Sent {
                    text: text.clone(),
                    units,
                },
            );
            (text, None)
        }
    }
}

/// How often model calls' cost and context reach the session's summary, at most.
const USAGE_EVERY: Duration = Duration::from_secs(1);

/// Adds a call's `(total, context)` to what is unwritten, and takes it all when a write is due: the first call, then
/// once `USAGE_EVERY` has passed since the last write (`call` None: another event asks whether held-back calls are due).
fn usage_due(
    unwritten: &mut Option<Unwritten>,
    written: &mut Option<Instant>,
    call: Option<(f64, Option<u64>)>,
    now: Instant,
) -> Option<Unwritten> {
    if let Some((total, context)) = call {
        let held = unwritten.get_or_insert(Unwritten { total, context: None });
        held.total = total;
        if context.is_some() {
            held.context = context;
        }
    }
    (*unwritten)?;
    let due = written.is_none_or(|at| now.saturating_duration_since(at) >= USAGE_EVERY);
    if !due {
        return None;
    }
    *written = Some(now);
    unwritten.take()
}

/// The mode a `setMode` permission suggestion switches to, if it is one.
fn suggested_mode(suggestion: &Value) -> Suggested {
    if suggestion.get("type").and_then(Value::as_str) != Some("setMode") {
        return Suggested::Other;
    }
    suggestion
        .get("mode")
        .and_then(Value::as_str)
        .and_then(PermissionMode::parse)
        .map_or(Suggested::UnknownMode, Suggested::Mode)
}

/// What an approval's "always" suggestion asks for, as far as the session's mode goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Suggested {
    /// A switch to this mode.
    Mode(PermissionMode),
    /// A switch to a mode Nexus does not know.
    UnknownMode,
    /// Anything else: a rule, a folder, a decision.
    Other,
}

/// `rule`, Claude Code's `addRules` allow suggestion, as a linked session under ancestors whose highest mode is
/// `ceiling` may keep it; None when it would let the session run unasked what an ancestor would ask about. A session
/// with no ancestors (`ceiling` None) keeps any rule as it is. A linked one keeps it for this session only, never in a
/// settings file, and only when the ancestors run every call unasked (Auto, Full access), or run edits unasked
/// (Auto-accept edits) and the rule names only file tools.
fn rule_under_ancestors(mut rule: Value, ceiling: Option<PermissionMode>) -> Option<Value> {
    let Some(ceiling) = ceiling else {
        return Some(rule);
    };
    if rule.get("type").and_then(Value::as_str) != Some("addRules")
        || rule.get("behavior").and_then(Value::as_str) != Some("allow")
    {
        return Some(rule);
    }
    let only_file_tools = rule.get("rules").and_then(Value::as_array).is_some_and(|rules| {
        !rules.is_empty()
            && rules.iter().all(|one| {
                one.get("toolName")
                    .and_then(Value::as_str)
                    .is_some_and(|tool| changes_source::FILE_TOOLS.contains(&tool))
            })
    });
    let allowed = ceiling.allows_unasked() || (ceiling == PermissionMode::AcceptEdits && only_file_tools);
    if !allowed {
        return None;
    }
    rule["destination"] = serde_json::json!("session");
    Some(rule)
}

#[cfg(unix)]
fn exit_of(status: Option<std::process::ExitStatus>) -> (i32, Option<String>) {
    use std::os::unix::process::ExitStatusExt;
    match status {
        Some(status) => match (status.code(), status.signal()) {
            (Some(code), _) => (code, None),
            (None, Some(signal)) => (128 + signal, Some(signal_name(signal))),
            _ => (1, None),
        },
        None => (1, None),
    }
}

#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    match signal {
        1 => "SIGHUP".into(),
        2 => "SIGINT".into(),
        9 => "SIGKILL".into(),
        15 => "SIGTERM".into(),
        other => format!("signal {other}"),
    }
}

#[cfg(not(unix))]
fn exit_of(status: Option<std::process::ExitStatus>) -> (i32, Option<String>) {
    (status.and_then(|s| s.code()).unwrap_or(1), None)
}

#[cfg(all(test, unix))]
mod local_run_tests;

#[cfg(test)]
mod saved_provider_tests;

#[cfg(test)]
mod tests {
    use super::{PermissionMode, Suggested, rule_under_ancestors, suggested_mode};
    use serde_json::json;

    #[test]
    fn an_unknown_suggested_mode_is_never_read_as_no_mode_switch() {
        assert_eq!(
            suggested_mode(&json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"})),
            Suggested::Mode(PermissionMode::AcceptEdits)
        );
        assert_eq!(
            suggested_mode(&json!({"type": "setMode", "mode": "yoloMode", "destination": "session"})),
            Suggested::UnknownMode
        );
        assert_eq!(suggested_mode(&json!({"type": "addRules"})), Suggested::Other);
    }

    #[test]
    fn a_linked_sessions_allow_rule_stays_under_its_ancestors_and_in_the_session() {
        let rule = |tool: &str| {
            json!({"type": "addRules", "behavior": "allow", "destination": "projectSettings",
                "rules": [{"toolName": tool}]})
        };
        // No ancestors: as it is.
        assert_eq!(rule_under_ancestors(rule("Bash"), None), Some(rule("Bash")));
        // Under Ask first or Plan only: never.
        assert_eq!(rule_under_ancestors(rule("Bash"), Some(PermissionMode::Default)), None);
        assert_eq!(rule_under_ancestors(rule("Edit"), Some(PermissionMode::Plan)), None);
        // Under Auto-accept edits: file tools only.
        assert_eq!(
            rule_under_ancestors(rule("Bash"), Some(PermissionMode::AcceptEdits)),
            None
        );
        let kept = rule_under_ancestors(rule("Edit"), Some(PermissionMode::AcceptEdits));
        assert_eq!(
            kept.as_ref().map(|kept| kept["destination"].clone()),
            Some(json!("session"))
        );
        // Under Auto: any rule, for this session only.
        let kept = rule_under_ancestors(rule("Bash"), Some(PermissionMode::Auto));
        assert_eq!(kept.map(|kept| kept["destination"].clone()), Some(json!("session")));
    }

    /// The output's tail never cuts a character in two: a cut inside one starts at the next.
    #[test]
    fn the_output_tail_keeps_whole_characters() {
        assert_eq!(super::tail("abcdef", 3), "def");
        assert_eq!(super::tail("ab", 8), "ab");
        // "é" is two bytes: four bytes back starts inside the first one.
        assert_eq!(super::tail("éééé", 3), "é");
        assert_eq!(super::tail("xéé", 4), "éé");
    }

    use super::*;

    fn said(seq: i64, event: WbEvent) -> StoredEvent {
        StoredEvent {
            thread_id: "t".into(),
            seq,
            at: 0,
            event,
        }
    }

    #[test]
    fn an_echo_confirms_the_message_it_holds() {
        let texts = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        let mut unread: VecDeque<(i64, String)> = [(3, "fix it".into()), (5, "yes".into()), (7, "yes".into())].into();
        // Each echo confirms one message, oldest first, even when two say the same.
        assert_eq!(read_back(&mut unread, &texts(&["fix it"])), [3]);
        assert_eq!(read_back(&mut unread, &texts(&["yes"])), [5]);
        assert_eq!(read_back(&mut unread, &texts(&["yes"])), [7]);
        assert!(unread.is_empty());
        // An echo of nothing we sent confirms nothing.
        let mut unread: VecDeque<(i64, String)> = [(2, "a".into())].into();
        assert!(read_back(&mut unread, &texts(&["something else"])).is_empty());
        assert_eq!(unread.len(), 1);
        // Codex joins a message into the one before: one echo, both read.
        let mut unread: VecDeque<(i64, String)> = [(2, "one".into()), (4, "two".into()), (6, "three".into())].into();
        assert_eq!(read_back(&mut unread, &texts(&["one", "two"])), [2, 4]);
        // One the agent skipped without an echo leaves unconfirmed.
        let mut unread: VecDeque<(i64, String)> = [(2, "/compact".into()), (4, "go on".into())].into();
        assert_eq!(read_back(&mut unread, &texts(&["go on"])), [4]);
        assert!(unread.is_empty());
        // A message of only images matches an echo with no text.
        let mut unread: VecDeque<(i64, String)> = [(2, String::new()), (4, "and this".into())].into();
        assert_eq!(read_back(&mut unread, &[]), [2]);
    }

    #[test]
    fn calls_reach_the_summary_at_once_then_at_most_once_a_second() {
        let (mut unwritten, mut written) = (None, None);
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let first = usage_due(&mut unwritten, &mut written, Some((1.0, Some(10))), at(0));
        assert_eq!(
            first,
            Some(Unwritten {
                total: 1.0,
                context: Some(10)
            })
        );
        // Held back within the second; a sub-agent's call keeps the main context.
        assert_eq!(
            usage_due(&mut unwritten, &mut written, Some((2.0, Some(20))), at(300)),
            None
        );
        assert_eq!(
            usage_due(&mut unwritten, &mut written, Some((3.0, None)), at(600)),
            None
        );
        assert_eq!(usage_due(&mut unwritten, &mut written, None, at(900)), None);
        // Another event a second on takes what was held, the latest of each.
        let due = usage_due(&mut unwritten, &mut written, None, at(1000));
        assert_eq!(
            due,
            Some(Unwritten {
                total: 3.0,
                context: Some(20)
            })
        );
        // Nothing held: nothing due, and the clock does not move.
        assert_eq!(usage_due(&mut unwritten, &mut written, None, at(2500)), None);
        assert_eq!(written, Some(at(1000)));
    }

    #[test]
    fn a_delta_sends_what_follows_the_text_sent_before_in_utf16_units() {
        let mut streamed = HashMap::new();
        assert_eq!(
            delta_suffix(&mut streamed, "m", &None, "Hé".into()),
            ("Hé".into(), None)
        );
        // "é" is 2 bytes but 1 UTF-16 unit, "😀" 4 bytes and 2 units: offsets count what JavaScript's length counts.
        assert_eq!(
            delta_suffix(&mut streamed, "m", &None, "Hé😀".into()),
            ("😀".into(), Some(2))
        );
        assert_eq!(
            delta_suffix(&mut streamed, "m", &None, "Hé😀 ok".into()),
            (" ok".into(), Some(4))
        );
        // Nothing new: an empty suffix where the text ends.
        assert_eq!(
            delta_suffix(&mut streamed, "m", &None, "Hé😀 ok".into()),
            ("".into(), Some(7))
        );
        // Another message streams on its own.
        assert_eq!(delta_suffix(&mut streamed, "n", &None, "x".into()), ("x".into(), None));
        // A text that does not continue what was sent (shorter) goes whole.
        assert_eq!(delta_suffix(&mut streamed, "m", &None, "H".into()), ("H".into(), None));
        // After a reset (a screen read the log), whole again.
        streamed.clear();
        assert_eq!(
            delta_suffix(&mut streamed, "m", &None, "Hi".into()),
            ("Hi".into(), None)
        );
    }

    #[test]
    fn a_text_that_revises_what_was_sent_goes_whole_not_as_a_suffix() {
        let mut streamed = HashMap::new();
        assert_eq!(
            delta_suffix(&mut streamed, "m", &None, "Hello".into()),
            ("Hello".into(), None)
        );
        // Same length, changed prefix: a length-only check would send "" at offset 5 and leave the screen on "Hello".
        assert_eq!(
            delta_suffix(&mut streamed, "m", &None, "Jello".into()),
            ("Jello".into(), None)
        );
        // Longer, changed prefix.
        assert_eq!(
            delta_suffix(&mut streamed, "m", &None, "Yellow!".into()),
            ("Yellow!".into(), None)
        );
        // The stored text was reset to the revision, so it continues from there.
        assert_eq!(
            delta_suffix(&mut streamed, "m", &None, "Yellow!!".into()),
            ("!".into(), Some(7))
        );
    }

    #[test]
    fn the_same_message_id_under_another_parent_streams_on_its_own() {
        let mut streamed = HashMap::new();
        let sub = Some("toolu_1".to_string());
        assert_eq!(
            delta_suffix(&mut streamed, "m:0", &None, "Main".into()),
            ("Main".into(), None)
        );
        assert_eq!(
            delta_suffix(&mut streamed, "m:0", &sub, "Sub".into()),
            ("Sub".into(), None)
        );
        assert_eq!(
            delta_suffix(&mut streamed, "m:0", &None, "Main text".into()),
            (" text".into(), Some(4))
        );
        assert_eq!(
            delta_suffix(&mut streamed, "m:0", &sub, "Sub text".into()),
            (" text".into(), Some(3))
        );
    }

    #[test]
    fn a_whole_delta_reads_as_before_and_a_suffix_carries_its_offset() {
        let whole = Delta {
            thread_id: "t".into(),
            id: "m".into(),
            text: "Hel".into(),
            parent: None,
            offset: None,
        };
        assert_eq!(
            serde_json::to_value(&whole).unwrap_or_default(),
            serde_json::json!({ "threadId": "t", "id": "m", "text": "Hel", "parent": null })
        );
        let suffix = Delta {
            offset: Some(3),
            text: "lo".into(),
            ..whole
        };
        assert_eq!(
            serde_json::to_value(&suffix).unwrap_or_default(),
            serde_json::json!({ "threadId": "t", "id": "m", "text": "lo", "parent": null, "offset": 3 })
        );
        // An older payload without `offset` still reads (as a whole text).
        let old: Delta =
            serde_json::from_value(serde_json::json!({ "threadId": "t", "id": "m", "text": "Hel" })).expect("reads");
        assert_eq!(old.offset, None);
    }

    /// `cargo test -p nexus-workbench --lib delta_bytes -- --ignored --nocapture`: what a 50,000-character answer costs
    /// on the wire, streamed in 15-character steps (about one 50 ms delta of a fast model).
    #[test]
    #[ignore]
    fn delta_bytes_for_a_long_answer() {
        let answer: String = "lorem ipsum dolor sit amet, ".chars().cycle().take(50_000).collect();
        let steps: Vec<usize> = (15..=answer.len()).step_by(15).collect();
        let measure = |suffixes: bool| {
            let mut streamed = HashMap::new();
            let (mut bytes, start) = (0usize, Instant::now());
            for &end in &steps {
                let so_far = answer[..end].to_string();
                let (text, offset) = if suffixes {
                    delta_suffix(&mut streamed, "m", &None, so_far)
                } else {
                    (so_far, None)
                };
                let delta = Delta {
                    thread_id: "t_0123456789".into(),
                    id: "msg_0123456789:0".into(),
                    text,
                    parent: None,
                    offset,
                };
                bytes += serde_json::to_string(&delta).map(|json| json.len()).unwrap_or_default();
            }
            (bytes, start.elapsed())
        };
        let (before, before_time) = measure(false);
        let (after, after_time) = measure(true);
        println!(
            "[perf] {} deltas: whole text {:.1} MB in {:?}; suffixes {:.1} KB in {:?}",
            steps.len(),
            before as f64 / 1e6,
            before_time,
            after as f64 / 1e3,
            after_time
        );
    }

    #[test]
    fn a_fork_keeps_the_end_of_a_newest_message_that_is_over_the_limit() {
        let long = "é".repeat(MAX_SEED);
        let seed = seed_of(&[
            said(
                1,
                WbEvent::User {
                    text: "first".into(),
                    images: vec![],
                    from: None,
                },
            ),
            said(
                2,
                WbEvent::Text {
                    id: "m".into(),
                    text: format!("{long}END"),
                    parent: None,
                },
            ),
        ]);
        assert!(seed.contains("(Earlier messages left out.)"));
        assert!(
            seed.contains("…") && seed.contains("END"),
            "the newest line's end stays"
        );
        assert!(!seed.contains("User: first"));
        assert!(seed.len() < MAX_SEED + 1024);
    }

    #[test]
    fn reads_what_reverts_ask_of_the_agents_conversation() {
        let end = |seq: i64, cursor: Option<&str>| StoredEvent {
            thread_id: "t".into(),
            seq,
            at: 0,
            event: WbEvent::TurnEnd {
                turn_id: format!("x{seq}"),
                status: TurnStatus::Completed,
                cost_usd: None,
                duration_ms: 0,
                error: None,
                cursor: cursor.map(String::from),
            },
        };
        let revert = |seq: i64, turn: i64| StoredEvent {
            thread_id: "t".into(),
            seq,
            at: 0,
            event: WbEvent::Revert { turn, files: 0 },
        };
        let mut log = vec![end(3, Some("c1")), end(6, Some("c2")), end(9, Some("c3"))];
        // Back to the latest turn: the conversation stays as it is.
        log.push(revert(10, 9));
        assert_eq!(conversation(&log, SessionAgent::Claude).1, None);
        log.push(revert(11, 3));
        let (kept, claude) = conversation(&log, SessionAgent::Claude);
        assert_eq!(kept, [(3, Some("c1".to_string()))]);
        assert_eq!(claude, Some(Rewind::At("c1".into())));
        assert_eq!(conversation(&log, SessionAgent::Codex).1, Some(Rewind::At("c2".into())));
        // A turn since then reached the agent: nothing more to do. A later revert counts only the turns still kept.
        log.extend([end(14, Some("c4")), end(17, Some("c5"))]);
        assert_eq!(conversation(&log, SessionAgent::Claude).1, None);
        log.push(revert(18, 14));
        assert_eq!(
            conversation(&log, SessionAgent::Claude).1,
            Some(Rewind::At("c4".into()))
        );
        assert_eq!(conversation(&log, SessionAgent::Codex).1, Some(Rewind::At("c5".into())));
        // Turns logged before cursors existed: the agent is told instead.
        let old = [end(3, None), end(6, None), revert(7, 3)];
        assert_eq!(conversation(&old, SessionAgent::Claude).1, Some(Rewind::Tell));
        assert_eq!(conversation(&old, SessionAgent::Codex).1, Some(Rewind::Tell));
        // A start that failed before the agent said anything: a rewind point gives way to the note, and the note
        // waits for a turn that ran.
        let failed = |seq: i64| StoredEvent {
            thread_id: "t".into(),
            seq,
            at: 0,
            event: WbEvent::TurnEnd {
                turn_id: format!("x{seq}"),
                status: TurnStatus::Error,
                cost_usd: None,
                duration_ms: 0,
                error: Some("unknown option".into()),
                cursor: None,
            },
        };
        log.push(failed(20));
        assert_eq!(conversation(&log, SessionAgent::Claude).1, Some(Rewind::Tell));
        log.push(failed(21));
        assert_eq!(conversation(&log, SessionAgent::Claude).1, Some(Rewind::Tell));
        log.push(end(24, None));
        assert_eq!(conversation(&log, SessionAgent::Claude).1, None);
    }

    /// a private formal model a rewind point is used only when it is exact, else the agent is told.
    #[test]
    fn a_rewind_point_is_exact_or_the_agent_is_told() {
        let ended = |seq: i64, cursor: Option<&str>, status: TurnStatus| StoredEvent {
            thread_id: "t".into(),
            seq,
            at: 0,
            event: WbEvent::TurnEnd {
                turn_id: format!("x{seq}"),
                status,
                cost_usd: None,
                duration_ms: 0,
                error: None,
                cursor: cursor.map(String::from),
            },
        };
        let end = |seq: i64, cursor: Option<&str>| ended(seq, cursor, TurnStatus::Completed);
        let revert = |seq: i64, turn: i64| StoredEvent {
            thread_id: "t".into(),
            seq,
            at: 0,
            event: WbEvent::Revert { turn, files: 0 },
        };
        // R3: Nexus closed in turn 2 (no cursor). Codex can't cut at it, and cutting at turn 3 would keep it.
        let log = [
            end(3, Some("c1")),
            ended(6, None, TurnStatus::Interrupted),
            end(9, Some("c3")),
            revert(10, 3),
        ];
        assert_eq!(conversation(&log, SessionAgent::Codex).1, Some(Rewind::Tell));
        // R3: told about turn 2 with turn 13's message; a revert that drops turn 13 drops the note, and Codex still
        // remembers turn 2: it is told again rather than cut at turn 13.
        let mut told = log.to_vec();
        told.extend([end(13, Some("c4")), revert(14, 3)]);
        assert_eq!(conversation(&told, SessionAgent::Codex).1, Some(Rewind::Tell));
        // R4: Claude, Nexus closed in the turn after a revert: whether it rewound is unknown, so it is told.
        let log = [
            end(3, Some("c1")),
            end(6, Some("c2")),
            revert(7, 3),
            ended(10, None, TurnStatus::Interrupted),
        ];
        assert_eq!(conversation(&log, SessionAgent::Claude).1, Some(Rewind::Tell));
        assert_eq!(conversation(&log, SessionAgent::Codex).1, Some(Rewind::Tell));
        // R4: back to a Claude turn with no message of its own: turn 1's cursor would drop turn 2, whose cursor was
        // lost.
        let log = [
            end(3, Some("c1")),
            ended(6, None, TurnStatus::Interrupted),
            end(9, None),
            end(12, Some("c4")),
            revert(13, 9),
        ];
        assert_eq!(conversation(&log, SessionAgent::Claude).1, Some(Rewind::Tell));
        // R1: a stopped turn that keeps its cursor clears the rewind and is the cut point like any other.
        let log = [
            end(3, Some("c1")),
            end(6, Some("c2")),
            revert(7, 3),
            ended(10, Some("c3"), TurnStatus::Interrupted),
        ];
        assert_eq!(conversation(&log, SessionAgent::Claude).1, None);
        let mut later = log.to_vec();
        later.extend([end(13, Some("c4")), revert(14, 3)]);
        assert_eq!(
            conversation(&later, SessionAgent::Codex).1,
            Some(Rewind::At("c3".into()))
        );
    }

    /// a private formal model R2: a revert and a message on its way to the agent exclude each other.
    #[test]
    fn a_revert_and_a_message_on_its_way_exclude_each_other() {
        let mut gate = RevertGate::default();
        gate.begin_send("t", false).unwrap_or_else(|e| panic!("{e}"));
        gate.begin_send("t", false).unwrap_or_else(|e| panic!("{e}"));
        assert!(gate.begin_revert("t").is_err_and(|e| e.contains("on its way")));
        assert!(gate.begin_revert("other").is_ok());
        gate.end_send("t");
        assert!(gate.begin_revert("t").is_err(), "one message is still on its way");
        gate.end_send("t");
        assert!(gate.begin_revert("t").is_ok());
        assert!(gate.begin_send("t", false).is_err_and(|e| e.contains("being reverted")));
        assert!(gate.begin_revert("t").is_err_and(|e| e.contains("already")));
    }

    #[test]
    fn places_a_review_beside_earlier_reviews_of_the_same_pull_request() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("work/nexus");
        let reviews = root.join("reviews");
        let (path, branch) = review_place(&reviews, &repo, 42);
        assert_eq!(path, reviews.join("nexus/pr-42"));
        assert_eq!(branch, "review/pr-42");
        std::fs::create_dir_all(&path).unwrap_or_else(|e| panic!("{e}"));
        let (path, branch) = review_place(&reviews, &repo, 42);
        assert_eq!(path, reviews.join("nexus/pr-42-2"));
        assert_eq!(branch, "review/pr-42-2");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn each_stream_gets_its_own_50_ms_refresh_slot() {
        let mut last = HashMap::new();
        let start = Instant::now();
        let sub = Some("toolu_task".to_string());
        assert!(delta_due(&mut last, &None, start));
        // A sub-agent streaming right after the main answer still shows, and does not use up the main answer's slot.
        assert!(delta_due(&mut last, &sub, start + Duration::from_millis(10)));
        assert!(!delta_due(&mut last, &None, start + Duration::from_millis(20)));
        assert!(!delta_due(&mut last, &sub, start + Duration::from_millis(40)));
        assert!(delta_due(&mut last, &None, start + Duration::from_millis(50)));
        assert!(delta_due(&mut last, &sub, start + Duration::from_millis(60)));
    }

    #[test]
    fn folders_overlap_when_one_holds_the_other() {
        let (parent, child) = (Path::new("/code"), Path::new("/code/api"));
        assert!(overlaps(parent, child) && overlaps(child, parent) && overlaps(child, child));
        assert!(!overlaps(child, Path::new("/code/web")));
        assert!(!overlaps(child, Path::new("/code/api-old")));
    }

    #[test]
    fn lists_the_repositories_in_a_folder_that_isnt_one_and_takes_only_their_names() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for name in ["clean", "changed", "plain"] {
            std::fs::create_dir_all(root.join(name)).unwrap_or_else(|e| panic!("{e}"));
        }
        let git = |dir: &str, args: &[&str]| {
            let ok = process::command("git")
                .arg("-C")
                .arg(root.join(dir))
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .status()
                .is_ok_and(|status| status.success());
            assert!(ok, "git {args:?} in {dir}");
        };
        for name in ["clean", "changed"] {
            git(name, &["init", "--quiet", "--initial-branch", "main"]);
            git(name, &["commit", "--quiet", "--allow-empty", "-m", "one"]);
        }
        std::fs::write(root.join("changed/new.txt"), "a\nb\n").unwrap_or_else(|e| panic!("{e}"));

        let state = read_git(&root, false);
        assert_eq!(state.branch, None);
        let repos = state.repos.unwrap_or_default();
        let listed: Vec<_> = repos
            .iter()
            .map(|repo| {
                (
                    repo.name.as_str(),
                    repo.branch.as_deref(),
                    repo.uncommitted.map(|stat| stat.insertions),
                )
            })
            .collect();
        assert_eq!(
            listed,
            [("changed", Some("main"), Some(2)), ("clean", Some("main"), Some(0))]
        );
        assert_eq!(read_git(&root.join("clean"), false).repos, None);

        assert_eq!(child_repo(&root, "changed"), Ok(root.join("changed")));
        for name in ["plain", "missing", "../changed", "changed/.git", "/tmp", ".", ""] {
            assert!(child_repo(&root, name).is_err(), "{name}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn checks_a_messages_images_for_count_type_and_total_size() {
        let image = |media_type: &str, data: String| Image {
            media_type: media_type.into(),
            data,
            name: None,
        };
        let png = |len: usize| image("image/png", "A".repeat(len));
        for media_type in IMAGE_TYPES {
            assert_eq!(check_images(&[image(media_type, "AAAA".into())]), Ok(()));
        }
        assert!(check_images(&[image("image/svg+xml", "AAAA".into())]).is_err());
        assert!(check_images(&vec![png(4); MAX_IMAGES + 1]).is_err());
        assert_eq!(decoded_len("QQ=="), 1);
        let quarter = MAX_IMAGE_BYTES / 4 / 3 * 4;
        assert_eq!(check_images(&vec![png(quarter); 4]), Ok(()));
        assert!(check_images(&vec![png(quarter + 4); 4]).is_err());
    }

    #[test]
    fn titles_a_session_from_its_first_message() {
        assert_eq!(title_of("\n  Fix the login bug\nmore"), "Fix the login bug");
        assert_eq!(title_of(&"x".repeat(80)).chars().count(), 61);
        assert_eq!(title_of("  "), "New session");
    }

    #[test]
    fn names_a_worktree_branch_from_the_first_message() {
        assert_eq!(
            branch_for("nexus/", "Fix the login bug, please!", "t_0123456789ab"),
            "nexus/fix-the-login-bug-please-89ab"
        );
        assert_eq!(branch_for("nexus/", "¿¡ …", "t_0123456789ab"), "nexus/session-89ab");
        assert!(branch_for("nexus/", &"word ".repeat(30), "t_0123456789ab").len() <= "nexus/".len() + 40 + 5);
        // The person's prefix, or none.
        assert_eq!(branch_for("jd/", "Fix it", "t_0123456789ab"), "jd/fix-it-89ab");
        assert_eq!(branch_for("", "Fix it", "t_0123456789ab"), "fix-it-89ab");
    }

    #[test]
    fn a_draft_reads_the_change_the_style_and_for_a_pull_request_the_branch_and_its_template() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-draft-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(root.join(".github")).unwrap_or_else(|e| panic!("{e}"));
        let git = |args: &[&str]| {
            let ok = process::command("git")
                .arg("-C")
                .arg(&root)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .status()
                .is_ok_and(|status| status.success());
            assert!(ok, "git {args:?}");
        };
        let write = |name: &str, text: &str| std::fs::write(root.join(name), text).unwrap_or_else(|e| panic!("{e}"));
        git(&["init", "--quiet", "--initial-branch", "main"]);
        // No commit yet: the new files are the whole change.
        write("a.txt", "one\n");
        let first = draft_context_in(&root, draft::Kind::Commit, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((first.subjects.len(), first.diff.as_str()), (0, ""));
        assert_eq!(first.untracked, vec!["a.txt".to_string()]);

        write(".github/pull_request_template.md", "## Summary\n");
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "feat: one"]);
        assert_eq!(
            draft_context_in(&root, draft::Kind::Commit, None),
            Err("Nothing to commit.".to_string())
        );
        git(&["switch", "--quiet", "-c", "ada/eng-1-two"]);
        git(&["config", "branch.ada/eng-1-two.gh-merge-base", "main"]);
        write("a.txt", "two\n");
        git(&["commit", "--quiet", "-am", "feat: two"]);
        write("a.txt", "three\n");

        let commit = draft_context_in(&root, draft::Kind::Commit, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(commit.branch.as_deref(), Some("ada/eng-1-two"));
        assert_eq!(commit.subjects, vec!["feat: two".to_string(), "feat: one".to_string()]);
        assert!(commit.diff.contains("-two\n+three"), "{}", commit.diff);
        assert!(commit.untracked.is_empty() && commit.commits.is_empty() && commit.template.is_none());

        let pr = draft_context_in(&root, draft::Kind::PullRequest, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(pr.base.as_deref(), Some("main"));
        assert_eq!(pr.commits, vec!["feat: two".to_string()]);
        assert_eq!(pr.subjects, vec!["feat: one".to_string()]);
        assert_eq!(pr.template.as_deref(), Some("## Summary\n"));
        // Since the branch left main, with what isn't committed yet: the pull request commits it first.
        assert!(pr.diff.contains("-one\n+three"), "{}", pr.diff);
        let _ = std::fs::remove_dir_all(&root);
    }
}

pub mod editors;
