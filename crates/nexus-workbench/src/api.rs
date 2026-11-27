//! The `wb_*` commands of the crate's contract.ts, with no transport: [`Api::call`] takes a command's name and its
//! arguments as the webview sends them (camelCase JSON) and answers with the JSON the webview reads. The desktop app
//! calls the typed methods here (through `nexus-remote`'s engine), and a headless host (`nexus-host`)
//! answers the same calls over a WebSocket ([`Request`], [`Reply`], [`Push`]). a private plan doc, section 1.
//!
//! What a command needs from the machine it runs on, beyond the workbench (the agents' `PATH`, the home folder, which
//! agents are installed, the organization's setting), comes from a [`Machine`]. Commands that only make sense with a
//! window on the same computer (the folder picker, opening an editor, notifications) and those kept in the app's own
//! settings (plan accounts, rotation, worktree folders) stay in the app: [`COMMANDS`] lists what this module answers.
//! The app's background service still takes the plan accounts and rotation from the app ([`crate::accounts::Choice`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::attachments::{self, FileInput};
use crate::draft;
use crate::event::{Decision, Effort, Image, PermissionMode, Project, SessionAgent, StoredEvent, ThreadSummary, Via};
use crate::git::{self, PullRequestInfo};
use crate::schedules::{ScheduleInput, ScheduleView, WebhookSecret};
use crate::scripts::ProjectScript;
use crate::store::{EventsPage, SearchHit};
use crate::sync::Pull;
use crate::{
    Changes, DiffSource, DiffTargets, Emit, GitState, NewReview, NewThread, NewWorktree, Result, ThreadPatch,
    Workbench, cleanup, clone, cloud_setup, files, mentions, project_files,
};

/// What the commands ask of the machine they run on.
pub trait Machine: Send + Sync + 'static {
    /// The `PATH` that git and the GitHub CLI run with (the agents' own).
    fn tool_path(&self) -> String;
    /// The person's home folder, where the default project (`~/nexus`) is.
    fn home(&self) -> Result<PathBuf>;
    /// Where the agent is installed, and its `--version`; for Nexus's agent, the `nexus` that answers its handshake
    /// (`nexus_runtime`). Called off the async workers: it runs the agent.
    fn agent(&self, agent: SessionAgent) -> AgentInfo;
    /// Whether the organization allows sessions on a person's own subscription (`agentSubscriptions`). False when
    /// it cannot be read.
    fn subscriptions_allowed(&self) -> BoxFuture<'static, bool>;
    /// The installed agent's own models ([`crate::agent_models::discover`]); None when discovery fails.
    fn discover_models(
        &self,
        agent: &AgentInfo,
        path: &str,
    ) -> BoxFuture<'static, Option<Vec<crate::agent_models::Model>>> {
        let (agent, path) = (agent.clone(), path.to_string());
        Box::pin(async move { crate::agent_models::discover(&agent, &path).await })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub agent: SessionAgent,
    pub path: Option<String>,
    pub version: Option<String>,
    /// The installed agent's own model capabilities, when discovery succeeds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<crate::agent_models::Model>>,
    /// Why the program found cannot run the agent, for a person (`path` is None then): a `nexus` that is the
    /// JavaScript CLI or too old (`nexus_runtime::problem`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub projects: Vec<Project>,
    pub threads: Vec<ThreadSummary>,
    pub agents: Vec<AgentInfo>,
    pub persistent: bool,
    pub agent_subscriptions: bool,
    /// What this machine offers beyond the protocol, as a host's hello says it (`HostInfo::features`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    pub execution_identities: Vec<crate::computer::ExecutionIdentity>,
}

/// How long `wb_overview` waits for the organization's agent-subscriptions setting.
pub const OVERVIEW_ORG_TIMEOUT: Duration = Duration::from_secs(3);

/// `wb_thread_create`'s `worktree`: work in a new git worktree, on this branch, from this base (each optional); or,
/// with `review`, check out that pull request of the project's repository to review it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorktreeArgs {
    pub branch: Option<String>,
    pub base: Option<String>,
    #[serde(default)]
    pub review: Option<u64>,
    /// The prefix of a branch named from the first message: the desktop app's setting, which a host does not keep.
    #[serde(default, rename = "branchPrefix")]
    pub branch_prefix: Option<String>,
}

/// `wb_thread_create`'s arguments, as the webview names them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateArgs {
    #[serde(default)]
    pub request_id: Option<String>,
    pub project_id: String,
    pub agent: String,
    pub model: Option<String>,
    pub permission_mode: String,
    pub effort: Option<String>,
    pub via: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<nexus_providers::InstanceId>,
    pub text: String,
    pub images: Vec<Image>,
    #[serde(default)]
    pub files: Vec<FileInput>,
    pub worktree: Option<WorktreeArgs>,
    /// Whether its agent may start subsessions; absent: this computer's switch (`Config::subsessions`).
    #[serde(default)]
    pub orchestration: Option<bool>,
    /// The session's unique name (session_name.rs); absent: none. Refused when invalid or taken.
    #[serde(default)]
    pub name: Option<String>,
}

/// Authority captured when the desktop composer submits a new thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartAuthority {
    pub scope: String,
    pub identity: Option<String>,
}

/// `wb_thread_update`'s arguments. `model: ""` and `effort: ""` go back to the default; a missing one is left.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateArgs {
    pub thread_id: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub effort: Option<String>,
    /// Claude Code's fast mode on or off; a missing one is left.
    #[serde(default)]
    pub fast: Option<bool>,
    pub archived: Option<bool>,
    pub orchestration: Option<bool>,
    pub pinned: Option<bool>,
    /// "Continue at reset" on a session its plan's limit stopped, or off again.
    #[serde(default)]
    pub continue_at_reset: Option<bool>,
    /// Snoozed until this time (ms since the epoch); 0 wakes it (snooze.rs). A JSON null would read as "leave it".
    pub snoozed_until: Option<i64>,
    pub keep_listed: Option<bool>,
    /// Settled by hand (true), or back in the list (false); a missing one is left.
    #[serde(default)]
    pub settled: Option<bool>,
    /// The session's unique name (session_name.rs); `""` clears it, a missing one leaves it.
    #[serde(default)]
    pub name: Option<String>,
}

/// The commands [`Api::call`] answers, as contract.ts names them.
pub const COMMANDS: &[&str] = &[
    "wb_computer_list",
    "wb_computer_request",
    "wb_computer_request_get",
    "wb_synced_thread_page",
    "wb_synced_command",
    "wb_synced_command_get",
    "wb_computer_info",
    "wb_computer_name_set",
    "wb_computer_join_begin",
    "wb_computer_join_poll",
    "wb_computer_join_allow",
    "wb_phone_invitation_begin",
    "wb_phone_invitation_poll",
    "wb_phone_invitation_approve",
    "wb_phone_invitation_cancel",
    "wb_local_history",
    "wb_overview",
    "wb_threads",
    "wb_code_control",
    "wb_code_control_set",
    "wb_agent_models",
    "wb_project_add",
    "wb_project_clone",
    "wb_project_clone_progress",
    "wb_dirs",
    "wb_cloud_setup",
    "wb_cloud_setup_retry",
    "wb_project_default",
    "wb_project_remove",
    "wb_project_update",
    "wb_project_setup",
    "wb_session_start_preference",
    "wb_session_start_overrides",
    "wb_project_orchestration_set",
    "wb_session_start_preference_set",
    "wb_session_start_answer",
    "wb_project_setup_set",
    "wb_project_scripts",
    "wb_project_scripts_set",
    "wb_project_defaults",
    "wb_project_defaults_set",
    "wb_project_claude_trust",
    "wb_project_claude_trust_set",
    "wb_schedules",
    "wb_schedule_save",
    "wb_schedule_delete",
    "wb_schedule_run",
    "wb_schedule_webhook",
    "wb_setup_stop",
    "wb_thread_create",
    "wb_pull_requests",
    "wb_worktree_bases",
    "wb_files",
    "wb_file_read",
    "wb_file_preview",
    "wb_file_media",
    "wb_file_search",
    "wb_mention_prs",
    "wb_pull_request",
    "wb_git_state",
    "wb_commit",
    "wb_draft_context",
    "wb_switch_branch",
    "wb_git_fetch",
    "wb_git_pull",
    "wb_branch_create",
    "wb_changed_turns",
    "wb_changes",
    "wb_turn_files",
    "wb_diff",
    "wb_diff_targets",
    "wb_revert_preview",
    "wb_revert_to_turn",
    "wb_rewind_message",
    "wb_worktree_remove",
    "wb_worktrees",
    "wb_worktree_remove_orphan",
    "wb_worktree_cleanup",
    "wb_move_to_worktree",
    "wb_thread_delete",
    "wb_thread_fork",
    "wb_send_back",
    "wb_side_chat",
    "wb_side_chat_close",
    "wb_thread_events",
    "wb_thread_stream",
    "wb_thread_page",
    "wb_search",
    "wb_send",
    "wb_attachment_chunk",
    "wb_attachment_cancel",
    "wb_approve",
    "wb_queue_cancel",
    "wb_queue_edit",
    "wb_queue_move",
    "wb_queue_steer",
    "wb_interrupt",
    "wb_task_stop",
    "wb_task_background",
    "wb_task_output",
    "wb_stop",
    "wb_compact",
    "wb_connection",
    "wb_thread_update",
    "wb_thread_switch_agent",
    "wb_settle_merged",
    "wb_check_failed",
    "wb_link_pull_request",
];

/// The remote protocol's number (a private plan doc, sections 1 and 2): a client and a host with different
/// numbers do not talk (the app offers to update the host). Raised when a change would break a client of the previous
/// number; new commands, events and fields do not raise it.
pub const PROTOCOL: u32 = 1;

/// What a host's hello says about it (`{ "hello": { "version", "protocol", "machine", "org", "features" } }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostInfo {
    pub version: String,
    pub protocol: u32,
    /// The machine's name (its hostname).
    pub machine: String,
    /// The organization the host is signed in to, if any.
    pub org: Option<String>,
    /// What it offers beyond the protocol: `orchestration` when its agents get the linked-session tools
    /// ([`crate::orchestra::FEATURE`]), and `orchestration-controls` when it sends the crew's
    /// controls ([`crate::orchestra::CONTROLS_FEATURE`]). Absent from an older host's hello.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

/// One call from a client over a transport, such as nexus-host's WebSocket: `{ "id": 7, "call": "wb_send", "args": {…} }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub call: String,
    #[serde(default)]
    pub args: Value,
}

/// Its answer: `{ "id": 7, "ok": … }` or `{ "id": 7, "error": "…" }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub id: u64,
    #[serde(flatten)]
    pub answer: Answer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    Ok(Value),
    Error(String),
}

/// What the workbench shows, as pushed to a client: `{ "event": "wb:event", "payload": {…} }`. The payload is the
/// [`Emit`] itself.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Push<'a> {
    pub event: &'static str,
    pub payload: &'a Emit,
}

impl<'a> Push<'a> {
    pub fn new(emit: &'a Emit) -> Push<'a> {
        Push {
            event: emit.name(),
            payload: emit,
        }
    }
}

/// The workbench and its machine, answering the `wb_*` commands.
type CachedModels = Option<(Instant, Vec<AgentInfo>)>;

#[derive(Clone)]
pub struct Api {
    wb: Workbench,
    machine: Arc<dyn Machine>,
    models: Arc<tokio::sync::Mutex<CachedModels>>,
    /// Where each running clone stands, by its request id ([`Api::project_clone`]).
    clones: Arc<Mutex<HashMap<String, clone::Progress>>>,
    /// A cloud machine's own setup at boot, and where it stands ([`cloud_setup`]).
    setup: cloud_setup::Setup,
}

fn parse<T>(value: &str, parse: fn(&str) -> Option<T>, what: &str) -> Result<T> {
    parse(value).ok_or_else(|| format!("Unknown {what} {value}."))
}

/// Store work (SQLite) and other blocking work off the async runtime's workers.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}

/// A command's arguments; none given reads as `{}`.
fn args_of<T: DeserializeOwned>(name: &str, args: Value) -> Result<T> {
    let args = if args.is_null() {
        Value::Object(Map::new())
    } else {
        args
    };
    serde_json::from_value(args).map_err(|error| format!("Invalid arguments for {name}: {error}."))
}

fn json<T: Serialize>(answer: Result<T>) -> Result<Value> {
    serde_json::to_value(answer?).map_err(|error| error.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PathArg {
    path: String,
}

/// `wb_project_clone`'s arguments.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloneArgs {
    source: String,
    parent: String,
    request_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestArg {
    request_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectArg {
    project_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupSetArgs {
    project_id: String,
    script: Option<String>,
}

/// `wb_project_update`'s arguments: a field left out stays as it is.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectUpdateArgs {
    project_id: String,
    pinned: Option<bool>,
    archived: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScriptsSetArgs {
    project_id: String,
    scripts: Vec<ProjectScript>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefaultsSetArgs {
    project_id: String,
    defaults: crate::project_defaults::ProjectDefaults,
}

/// `wb_schedule_save`'s arguments: no `scheduleId` adds one.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleSaveArgs {
    #[serde(default)]
    schedule_id: Option<String>,
    schedule: ScheduleInput,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleArg {
    schedule_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleWebhookArgs {
    schedule_id: String,
    on: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadArg {
    thread_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteThreadArgs {
    thread_id: String,
    #[serde(default)]
    discard_uncommitted: bool,
    #[serde(default)]
    offer_discard_uncommitted: bool,
}

/// Only a workbench that accepts confirmed deletion returns this for a client that asks.
pub const WORKTREE_DELETE_CONFIRMATION_REQUIRED: &str = "Discard uncommitted changes to delete this worktree.";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSwitchArgs {
    thread_id: String,
    #[serde(flatten)]
    target: crate::agent_switch::AgentSwitch,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskArgs {
    thread_id: String,
    task_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallArgs {
    thread_id: String,
    tool_use_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StopArgs {
    thread_id: String,
    #[serde(default)]
    restart: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesArgs {
    thread_id: Option<String>,
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileReadArgs {
    thread_id: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilePreviewArgs {
    thread_id: String,
    path: String,
    /// Set: an image or PDF comes in pieces from here ([`crate::output_files::read_piece`]).
    #[serde(default)]
    offset: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileMediaArgs {
    thread_id: String,
    path: String,
    #[serde(default)]
    offset: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileSearchArgs {
    thread_id: String,
    query: String,
    #[serde(default)]
    case_sensitive: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestArgs {
    thread_id: String,
    title: String,
    body: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoArgs {
    thread_id: String,
    repo: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitArgs {
    thread_id: String,
    repo: Option<String>,
    message: String,
    push: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DraftArgs {
    thread_id: String,
    repo: Option<String>,
    kind: draft::Kind,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwitchArgs {
    thread_id: String,
    repo: Option<String>,
    branch: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullArgs {
    thread_id: String,
    repo: Option<String>,
    rebase: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnArgs {
    thread_id: String,
    turn: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MaybeTurnArgs {
    thread_id: String,
    turn: Option<i64>,
    subscription: Option<crate::ForkSubscription>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffArgs {
    thread_id: String,
    source: DiffSource,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsArgs {
    thread_id: String,
    after_seq: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageArgs {
    thread_id: String,
    #[serde(default)]
    before_seq: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchArgs {
    query: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendArgs {
    thread_id: String,
    text: String,
    images: Vec<Image>,
    #[serde(default)]
    files: Vec<FileInput>,
    /// The person queued it (Ctrl+Enter): it waits for the running turn to end instead of joining it. An older host
    /// ignores it, and the message joins the turn.
    #[serde(default)]
    queued: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApproveArgs {
    thread_id: String,
    request_id: String,
    decision: Decision,
    message: Option<String>,
    suggestion: Option<usize>,
    answers: Option<Map<String, Value>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeqArgs {
    thread_id: String,
    seq: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueEditArgs {
    thread_id: String,
    seq: i64,
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueMoveArgs {
    thread_id: String,
    seq: i64,
    #[serde(default)]
    before: Option<i64>,
}

impl Api {
    pub fn new(wb: Workbench, machine: Arc<dyn Machine>) -> Api {
        Api {
            wb,
            machine,
            models: Default::default(),
            clones: Default::default(),
            setup: Default::default(),
        }
    }

    pub fn workbench(&self) -> &Workbench {
        &self.wb
    }

    /// Answers the command `name` (one of [`COMMANDS`]) with its arguments as the webview sends them.
    ///
    /// The caller owns the scope: `call` never sets one, so sessions are listed and emitted under whatever scope the
    /// [`Workbench`] holds. Call [`Workbench::set_scope`] before each command (the app does it in `bench()`), or a
    /// host would see no sessions or another organization's.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value> {
        match name {
            "wb_computer_list" => json(self.computer_list().await),
            "wb_computer_request" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct A {
                    computer_id: String,
                    request_id: String,
                    operation: crate::computer::Operation,
                }
                let a: A = args_of(name, args)?;
                json(self.computer_request(a.computer_id, a.request_id, a.operation).await)
            }
            "wb_computer_request_get" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct A {
                    computer_id: String,
                    request_id: String,
                }
                let a: A = args_of(name, args)?;
                json(self.computer_request_get(a.computer_id, a.request_id).await)
            }
            "wb_synced_thread_page" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct A {
                    thread_id: String,
                    before: Option<i64>,
                    after: Option<i64>,
                    limit: Option<usize>,
                }
                let a: A = args_of(name, args)?;
                json(self.synced_thread_page(a.thread_id, a.before, a.after, a.limit).await)
            }
            "wb_synced_command" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct A {
                    thread_id: String,
                    request_id: String,
                    action: crate::session_sync::control::Action,
                }
                let a: A = args_of(name, args)?;
                json(self.synced_command(a.thread_id, a.request_id, a.action).await)
            }
            "wb_synced_command_get" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct A {
                    thread_id: String,
                    request_id: String,
                }
                let a: A = args_of(name, args)?;
                json(self.synced_command_get(a.thread_id, a.request_id).await)
            }
            "wb_computer_info" => json(self.computer_info().await),
            "wb_computer_name_set" => {
                #[derive(Deserialize)]
                struct A {
                    name: String,
                }
                let a: A = args_of(name, args)?;
                json(self.computer_name_set(a.name).await)
            }
            "wb_computer_join_begin" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct A {
                    link: String,
                }
                let a: A = args_of(name, args)?;
                json(self.computer_join_begin(a.link).await)
            }
            "wb_computer_join_poll" | "wb_computer_join_allow" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct A {
                    invite_id: String,
                }
                let a: A = args_of(name, args)?;
                if name == "wb_computer_join_poll" {
                    json(self.computer_join_poll(a.invite_id).await)
                } else {
                    json(self.computer_join_allow(a.invite_id).await)
                }
            }
            "wb_phone_invitation_begin" => json(self.phone_invitation_begin().await),
            "wb_phone_invitation_approve" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct A {
                    invite_id: String,
                    device_id: String,
                    code: String,
                }
                let a: A = args_of(name, args)?;
                json(self.phone_invitation_approve(a.invite_id, a.device_id, a.code).await)
            }
            "wb_phone_invitation_poll" | "wb_phone_invitation_cancel" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct A {
                    invite_id: String,
                }
                let a: A = args_of(name, args)?;
                if name == "wb_phone_invitation_poll" {
                    json(self.phone_invitation_poll(a.invite_id).await)
                } else {
                    json(self.phone_invitation_cancel(a.invite_id).await)
                }
            }
            "wb_local_history" => {
                #[derive(Deserialize)]
                struct A {
                    cursor: Option<String>,
                    limit: Option<usize>,
                }
                let a: A = args_of(name, args)?;
                json(self.local_history(a.cursor, a.limit).await)
            }
            "wb_code_control" => json(self.code_control().await),
            "wb_code_control_set" => {
                #[derive(Deserialize)]
                struct Choice {
                    enabled: bool,
                }
                let choice: Choice = args_of(name, args)?;
                json(self.set_code_control(choice.enabled).await)
            }
            "wb_agent_models" => json(self.agent_models().await),
            "wb_overview" => json(self.overview().await),
            // The sessions with their live fields, and nothing else: no agent check and no Gateway call, so a window
            // may read it on each reconnect (GPUI's `live_fields.rs`).
            "wb_threads" => json(
                blocking({
                    let wb = self.wb.clone();
                    move || wb.threads()
                })
                .await,
            ),
            "wb_project_add" => {
                let a: PathArg = args_of(name, args)?;
                json(self.project_add(a.path).await)
            }
            "wb_project_clone" => {
                let a: CloneArgs = args_of(name, args)?;
                json(self.project_clone(a.source, a.parent, a.request_id).await)
            }
            "wb_project_clone_progress" => {
                let a: RequestArg = args_of(name, args)?;
                json(Ok(self.clone_progress(&a.request_id)))
            }
            "wb_dirs" => {
                let a: PathArg = args_of(name, args)?;
                json(self.dirs(a.path).await)
            }
            "wb_cloud_setup" => json(Ok(self.cloud_setup())),
            "wb_cloud_setup_retry" => json(Ok(self.cloud_setup_retry())),
            "wb_project_default" => json(self.project_default().await),
            "wb_project_remove" => {
                let a: ProjectArg = args_of(name, args)?;
                json(self.project_remove(a.project_id).await)
            }
            "wb_project_update" => {
                let a: ProjectUpdateArgs = args_of(name, args)?;
                json(self.project_update(a.project_id, a.pinned, a.archived).await)
            }
            "wb_project_setup" => {
                let a: ProjectArg = args_of(name, args)?;
                json(self.project_setup(a.project_id).await)
            }
            "wb_project_setup_set" => {
                let a: SetupSetArgs = args_of(name, args)?;
                json(self.project_setup_set(a.project_id, a.script).await)
            }
            "wb_project_scripts" => {
                let a: ProjectArg = args_of(name, args)?;
                json(self.project_scripts(a.project_id).await)
            }
            "wb_project_scripts_set" => {
                let a: ScriptsSetArgs = args_of(name, args)?;
                json(self.project_scripts_set(a.project_id, a.scripts).await)
            }
            "wb_project_defaults" => {
                let a: ProjectArg = args_of(name, args)?;
                json(self.project_defaults(a.project_id).await)
            }
            "wb_project_defaults_set" => {
                let a: DefaultsSetArgs = args_of(name, args)?;
                json(self.project_defaults_set(a.project_id, a.defaults).await)
            }
            "wb_project_claude_trust" => {
                let a: ProjectArg = args_of(name, args)?;
                let wb = self.wb.clone();
                json(blocking(move || wb.claude_trust(&a.project_id)).await)
            }
            "wb_project_claude_trust_set" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Args {
                    project_id: String,
                    trusted: bool,
                }
                let a: Args = args_of(name, args)?;
                let wb = self.wb.clone();
                json(blocking(move || wb.set_claude_trust(&a.project_id, a.trusted)).await)
            }
            "wb_schedules" => json(self.schedules().await),
            "wb_schedule_save" => {
                let a: ScheduleSaveArgs = args_of(name, args)?;
                json(self.schedule_save(a.schedule_id, a.schedule).await)
            }
            "wb_schedule_delete" => {
                let a: ScheduleArg = args_of(name, args)?;
                json(self.schedule_delete(a.schedule_id).await)
            }
            "wb_schedule_run" => {
                let a: ScheduleArg = args_of(name, args)?;
                json(self.schedule_run(&a.schedule_id).await)
            }
            "wb_schedule_webhook" => {
                let a: ScheduleWebhookArgs = args_of(name, args)?;
                json(self.schedule_webhook(&a.schedule_id, a.on).await)
            }
            "wb_thread_create" | "wb_thread_create_files" => json(self.thread_create(args_of(name, args)?).await),
            // Native app to its service. The distinct command makes older hosts fail closed.
            "wb_thread_create_bound" => {
                let source: StartAuthority = args_of(name, args["source"].clone())?;
                json(self.thread_create_at_scope(args_of(name, args)?, Some(&source)).await)
            }
            "wb_thread_create_once" => {
                let args: CreateArgs = args_of(name, args)?;
                if args.request_id.is_none() {
                    return Err("A session creation ID is required.".into());
                }
                json(self.thread_create(args).await)
            }
            "wb_pull_requests" => {
                let a: ProjectArg = args_of(name, args)?;
                json(self.pull_requests(&a.project_id).await)
            }
            "wb_worktree_bases" => {
                let a: ProjectArg = args_of(name, args)?;
                json(self.worktree_bases(&a.project_id).await)
            }
            "wb_files" => {
                let a: FilesArgs = args_of(name, args)?;
                json(self.files(a.thread_id, a.project_id).await)
            }
            "wb_file_preview" => {
                let a: FilePreviewArgs = args_of(name, args)?;
                json(self.file_preview(&a.thread_id, a.path, a.offset).await)
            }
            "wb_file_media" => {
                let a: FileMediaArgs = args_of(name, args)?;
                json(self.file_media(&a.thread_id, a.path, a.offset).await)
            }
            "wb_file_read" => {
                let a: FileReadArgs = args_of(name, args)?;
                json(self.file_read(&a.thread_id, a.path).await)
            }
            "wb_file_search" => {
                let a: FileSearchArgs = args_of(name, args)?;
                json(self.file_search(&a.thread_id, a.query, a.case_sensitive).await)
            }
            "wb_mention_prs" => {
                let a: FilesArgs = args_of(name, args)?;
                json(self.mention_prs(a.thread_id, a.project_id).await)
            }
            "wb_pull_request" => {
                let a: PullRequestArgs = args_of(name, args)?;
                json(self.pull_request(&a.thread_id, &a.title, &a.body).await)
            }
            "wb_git_state" => {
                let a: RepoArgs = args_of(name, args)?;
                json(self.git_state(&a.thread_id, a.repo.as_deref()).await)
            }
            "wb_commit" => {
                let a: CommitArgs = args_of(name, args)?;
                json(self.commit(&a.thread_id, a.repo.as_deref(), &a.message, a.push).await)
            }
            "wb_draft_context" => {
                let a: DraftArgs = args_of(name, args)?;
                json(self.draft_context(&a.thread_id, a.repo.as_deref(), a.kind).await)
            }
            "wb_switch_branch" => {
                let a: SwitchArgs = args_of(name, args)?;
                json(self.switch_branch(&a.thread_id, a.repo.as_deref(), &a.branch).await)
            }
            "wb_git_fetch" => {
                let a: RepoArgs = args_of(name, args)?;
                json(self.git_fetch(&a.thread_id, a.repo.as_deref()).await)
            }
            "wb_git_pull" => {
                let a: PullArgs = args_of(name, args)?;
                json(self.git_pull(&a.thread_id, a.repo.as_deref(), a.rebase).await)
            }
            "wb_branch_create" => {
                let a: SwitchArgs = args_of(name, args)?;
                json(self.branch_create(&a.thread_id, a.repo.as_deref(), &a.branch).await)
            }
            "wb_changed_turns" => {
                let a: ThreadArg = args_of(name, args)?;
                json(self.changed_turns(&a.thread_id).await)
            }
            "wb_changes" => {
                let a: MaybeTurnArgs = args_of(name, args)?;
                json(self.changes(&a.thread_id, a.turn).await)
            }
            "wb_turn_files" => {
                let a: TurnArgs = args_of(name, args)?;
                json(self.turn_files(&a.thread_id, a.turn).await)
            }
            "wb_diff" => {
                let a: DiffArgs = args_of(name, args)?;
                json(self.diff(&a.thread_id, a.source).await)
            }
            "wb_diff_targets" => {
                let a: ThreadArg = args_of(name, args)?;
                json(self.diff_targets(&a.thread_id).await)
            }
            "wb_revert_preview" => {
                let a: TurnArgs = args_of(name, args)?;
                json(self.revert_preview(&a.thread_id, a.turn).await)
            }
            "wb_revert_to_turn" => {
                let a: TurnArgs = args_of(name, args)?;
                json(self.revert_to_turn(&a.thread_id, a.turn).await)
            }
            "wb_rewind_message" => {
                let a: SeqArgs = args_of(name, args)?;
                json(self.rewind_message(&a.thread_id, a.seq).await)
            }
            "wb_worktree_remove" => {
                let a: ThreadArg = args_of(name, args)?;
                json(self.worktree_remove(&a.thread_id).await)
            }
            "wb_worktrees" => json(self.worktrees().await),
            "wb_worktree_remove_orphan" => {
                let a: PathArg = args_of(name, args)?;
                json(self.worktree_remove_orphan(&a.path).await)
            }
            "wb_move_to_worktree" => {
                let a: ThreadArg = args_of(name, args)?;
                json(self.move_to_worktree(&a.thread_id).await)
            }
            "wb_thread_delete" => {
                let a: DeleteThreadArgs = args_of(name, args)?;
                let answer = self
                    .thread_delete_with_options(&a.thread_id, a.discard_uncommitted)
                    .await;
                if a.offer_discard_uncommitted
                    && answer.as_ref().is_err_and(|error| {
                        matches!(
                            error.as_str(),
                            crate::git::WORKTREE_UNCOMMITTED | crate::git::WORKTREE_UNPUSHED
                        )
                    })
                {
                    return Err(WORKTREE_DELETE_CONFIRMATION_REQUIRED.into());
                }
                json(answer)
            }
            "wb_thread_fork" => {
                let a: MaybeTurnArgs = args_of(name, args)?;
                json(
                    self.thread_fork_with_subscription(&a.thread_id, a.turn, a.subscription)
                        .await,
                )
            }
            "wb_send_back" => {
                let a: ThreadArg = args_of(name, args)?;
                json(self.send_back(&a.thread_id).await)
            }
            "wb_side_chat" => {
                let a: ThreadArg = args_of(name, args)?;
                json(self.side_chat(&a.thread_id).await)
            }
            "wb_side_chat_close" => {
                let a: ThreadArg = args_of(name, args)?;
                json(self.side_chat_close(&a.thread_id).await)
            }
            "wb_thread_stream" => {
                let a: EventsArgs = args_of(name, args)?;
                let wb = self.wb.clone();
                json(blocking(move || wb.thread_stream(&a.thread_id, a.after_seq)).await)
            }
            "wb_thread_events" => {
                let a: EventsArgs = args_of(name, args)?;
                json(self.thread_events(a.thread_id, a.after_seq).await)
            }
            "wb_thread_page" => {
                let a: PageArgs = args_of(name, args)?;
                json(self.thread_page(a.thread_id, a.before_seq, a.limit).await)
            }
            "wb_search" => {
                let a: SearchArgs = args_of(name, args)?;
                json(self.search(a.query).await)
            }
            "wb_send" | "wb_send_files" => {
                let a: SendArgs = args_of(name, args)?;
                json(
                    self.send_files_as(&a.thread_id, &a.text, a.images, a.files, a.queued)
                        .await,
                )
            }
            "wb_attachment_chunk" => json(self.attachment_chunk(args_of(name, args)?).await),
            "wb_attachment_cancel" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Args {
                    upload_id: String,
                    #[serde(default)]
                    thread_id: Option<String>,
                }
                let a: Args = args_of(name, args)?;
                json(self.attachment_cancel(a.upload_id, a.thread_id).await)
            }
            "wb_project_orchestration_set" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Args {
                    project_id: String,
                    on: bool,
                }
                let a: Args = args_of(name, args)?;
                json(self.wb.set_project_orchestration(&a.project_id, a.on))
            }
            "wb_session_start_preference" => {
                let a: ProjectArg = args_of(name, args)?;
                json(self.wb.session_start_preference(&a.project_id))
            }
            "wb_session_start_overrides" => {
                #[derive(Deserialize)]
                struct Args {
                    clear: bool,
                }
                let a: Args = args_of(name, args)?;
                json(self.wb.session_start_overrides(a.clear))
            }
            "wb_session_start_preference_set" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Args {
                    project_id: String,
                    preference: crate::session_choice::Preference,
                }
                let a: Args = args_of(name, args)?;
                json(self.wb.set_session_start_preference(&a.project_id, a.preference))
            }
            "wb_session_start_answer" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Args {
                    thread_id: String,
                    request_id: String,
                    choice: crate::session_choice::Preference,
                    #[serde(default)]
                    remember: bool,
                }
                let a: Args = args_of(name, args)?;
                json(
                    self.wb
                        .answer_session_start(&a.thread_id, &a.request_id, a.choice, a.remember),
                )
            }
            "wb_approve" => {
                let a: ApproveArgs = args_of(name, args)?;
                json(
                    self.approve(
                        a.thread_id,
                        a.request_id,
                        a.decision,
                        a.message,
                        a.suggestion,
                        a.answers,
                    )
                    .await,
                )
            }
            "wb_queue_cancel" => {
                let a: SeqArgs = args_of(name, args)?;
                json(self.queue_cancel(a.thread_id, a.seq).await)
            }
            "wb_queue_edit" => {
                let a: QueueEditArgs = args_of(name, args)?;
                json(self.queue_edit(a.thread_id, a.seq, a.text).await)
            }
            "wb_queue_move" => {
                let a: QueueMoveArgs = args_of(name, args)?;
                json(self.queue_move(a.thread_id, a.seq, a.before).await)
            }
            "wb_queue_steer" => {
                let a: SeqArgs = args_of(name, args)?;
                json(self.queue_steer(a.thread_id, a.seq).await)
            }
            "wb_interrupt" => {
                let a: ThreadArg = args_of(name, args)?;
                json(self.interrupt(&a.thread_id))
            }
            "wb_task_stop" => {
                let a: TaskArgs = args_of(name, args)?;
                json(self.task_stop(&a.thread_id, &a.task_id).await)
            }
            "wb_task_background" => {
                let a: CallArgs = args_of(name, args)?;
                json(
                    self.task_background(&a.thread_id, &a.tool_use_id)
                        .await
                        .map(|backgrounded| serde_json::json!({ "backgrounded": backgrounded })),
                )
            }
            "wb_task_output" => {
                let a: TaskArgs = args_of(name, args)?;
                json(self.task_output(&a.thread_id, &a.task_id).await)
            }
            "wb_connection" => {
                let a: ThreadArg = args_of(name, args)?;
                json(self.wb.connection(&a.thread_id).await)
            }
            "wb_compact" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct CompactArgs {
                    thread_id: String,
                    instructions: Option<String>,
                }
                let a: CompactArgs = args_of(name, args)?;
                json(self.wb.compact(&a.thread_id, a.instructions).await)
            }
            "wb_stop" => {
                let a: StopArgs = args_of(name, args)?;
                json(if a.restart {
                    self.restart(&a.thread_id)
                } else {
                    self.stop(&a.thread_id)
                })
            }
            "wb_setup_stop" => {
                let a: ThreadArg = args_of(name, args)?;
                self.setup_stop(&a.thread_id);
                Ok(Value::Null)
            }
            "wb_thread_switch_agent" => {
                let args: AgentSwitchArgs = args_of(name, args)?;
                if self.machine.agent(args.target.agent).path.is_none() {
                    return Err(format!(
                        "Install {} on this machine before switching.",
                        crate::driver::label(args.target.agent)
                    ));
                }
                if args.target.via == Via::Subscription && !self.machine.subscriptions_allowed().await {
                    return Err("Your organization has subscription plans turned off.".into());
                }
                json(self.wb.switch_agent(&args.thread_id, args.target).await)
            }
            "wb_thread_update" => json(self.thread_update(args_of(name, args)?).await),
            "wb_settle_merged" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Args {
                    thread_id: String,
                    number: u64,
                    /// "Archive merged sessions" is on: the session archives instead of staying in Settled.
                    #[serde(default)]
                    archive: bool,
                }
                let a: Args = args_of(name, args)?;
                if a.archive {
                    json(self.wb.archive_merged(&a.thread_id, a.number))
                } else {
                    json(self.wb.settle_merged(&a.thread_id, a.number))
                }
            }
            // The pull request watch saw a check fail: the hooks of the store's owner run (`hooks`). It adds no hook.
            "wb_check_failed" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Args {
                    thread_id: String,
                    name: String,
                    url: Option<String>,
                }
                let a: Args = args_of(name, args)?;
                self.wb.check_failed(&a.thread_id, &a.name, a.url.as_deref());
                Ok(Value::Null)
            }
            // The pull request watch found the branch's pull request, opened outside the app's form: the session saves it.
            "wb_link_pull_request" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Args {
                    thread_id: String,
                    url: String,
                }
                let a: Args = args_of(name, args)?;
                json(self.wb.link_pull_request(&a.thread_id, &a.url))
            }
            "wb_worktree_cleanup" => {
                #[derive(Deserialize)]
                struct CleanupArgs {
                    rules: cleanup::Rules,
                }
                let a: CleanupArgs = args_of(name, args)?;
                json(self.worktree_cleanup(&a.rules, None).await)
            }
            _ => Err(format!("Unknown command {name}.")),
        }
    }

    /// [`Api::call`] for one [`Request`].
    pub async fn answer(&self, request: Request) -> Reply {
        let answer = match self.call(&request.call, request.args).await {
            Ok(value) => Answer::Ok(value),
            Err(error) => Answer::Error(error),
        };
        Reply { id: request.id, answer }
    }

    /// Cached for five minutes across windows. Concurrent refreshes serialize and share the cached result. Nexus's
    /// agent is looked at again each time, from the cache too: a handshake takes milliseconds, and a `nexus` installed
    /// or replaced since must count at once (`nexus_runtime`).
    pub async fn agent_models(&self) -> Result<Vec<AgentInfo>> {
        self.agent_models_each(|_| {}).await
    }

    /// [`Api::agent_models`], with the whole list handed to `landed` each time one agent's discovery ends, so a fast
    /// agent's models do not wait for a slow one (`agy models` takes up to its deadline). An agent still discovering
    /// carries its previous models, or none. A cached list is handed over once.
    pub async fn agent_models_each(&self, mut landed: impl FnMut(Vec<AgentInfo>) + Send) -> Result<Vec<AgentInfo>> {
        let mut cache = self.models.lock().await;
        if let Some((at, models)) = cache.as_mut()
            && at.elapsed() < Duration::from_secs(5 * 60)
        {
            let machine = self.machine.clone();
            let nexus = tokio::task::spawn_blocking(move || machine.agent(SessionAgent::Nexus))
                .await
                .map_err(|e| e.to_string())?;
            for agent in models.iter_mut().filter(|one| one.agent == SessionAgent::Nexus) {
                *agent = nexus.clone();
            }
            landed(models.clone());
            return Ok(models.clone());
        }
        let machine = self.machine.clone();
        let found = tokio::task::spawn_blocking(move || SessionAgent::ALL.map(|agent| machine.agent(agent)))
            .await
            .map_err(|e| e.to_string())?;
        // A failed refresh keeps the last successful metadata, rather than dropping people's current choices; an agent
        // still discovering shows the same meanwhile.
        let previous = |agent: SessionAgent| {
            cache
                .as_ref()
                .and_then(|(_, previous)| previous.iter().find(|one| one.agent == agent))
                .and_then(|one| one.models.clone())
        };
        let mut agents: Vec<AgentInfo> = found
            .iter()
            .map(|agent| AgentInfo {
                models: previous(agent.agent),
                ..agent.clone()
            })
            .collect();
        let path = self.machine.tool_path();
        let mut discoveries: futures::stream::FuturesUnordered<_> = found
            .iter()
            .enumerate()
            .map(|(at, agent)| {
                let discovery = self.machine.discover_models(agent, &path);
                async move { (at, discovery.await) }
            })
            .collect();
        while let Some((at, models)) = futures::StreamExt::next(&mut discoveries).await {
            if models.is_some() {
                agents[at].models = models;
            }
            landed(agents.clone());
        }
        *cache = Some((Instant::now(), agents.clone()));
        Ok(agents)
    }

    pub async fn overview(&self) -> Result<Overview> {
        // The organization's setting is a network call: a slow Gateway must not leave the Code page loading. Not heard
        // from in time reads as "not allowed" (the same as a failed call); the next load asks again.
        let allowed = tokio::time::timeout(OVERVIEW_ORG_TIMEOUT, self.machine.subscriptions_allowed());
        let stored = blocking({
            let wb = self.wb.clone();
            move || Ok((wb.projects()?, wb.threads()?, wb.persistent()))
        });
        let (stored, allowed, agents) = tokio::join!(stored, allowed, self.agent_models());
        let agents = agents?;
        let (projects, threads, persistent) = stored?;
        let execution_identities = self
            .execution_identities(&threads.iter().map(|t| t.id.clone()).collect::<Vec<_>>())
            .await?;
        Ok(Overview {
            projects,
            threads,
            agents,
            persistent,
            agent_subscriptions: allowed.unwrap_or(false),
            execution_identities,
            features: self.wb.features(),
        })
    }

    pub async fn project_add(&self, path: String) -> Result<Project> {
        let wb = self.wb.clone();
        let tool_path = self.machine.tool_path();
        // `~/code` is the machine's home folder, as Add a project's field says.
        let path = if path.trim_start().starts_with('~') {
            crate::dirs::resolve(&path, &self.machine.home()?)?
                .to_string_lossy()
                .into_owned()
        } else {
            path
        };
        blocking(move || {
            let project = wb.add_project(Path::new(&path))?;
            // A cloud project whose history never finished arriving (the host closed first) resumes now.
            clone::resume_deepen_if_blobless(PathBuf::from(&path), tool_path);
            Ok(project)
        })
        .await
    }

    /// The folders in `path` on this machine, for Add a project's folder field ([`crate::dirs`]).
    pub async fn dirs(&self, path: String) -> Result<crate::dirs::Dirs> {
        let wb = self.wb.clone();
        let home = self.machine.home();
        blocking(move || crate::dirs::list(&path, &home?, &wb.projects()?)).await
    }

    /// Clones `source` (a repository URL, or GitHub's `owner/repo`) into `<parent>/<its name>` on this machine, and
    /// opens the clone as a project. Where it stands is [`Api::clone_progress`] of `request_id` until it ends.
    pub async fn project_clone(&self, source: String, parent: String, request_id: String) -> Result<Project> {
        // The machine sets this repository up itself (an older app asks for its clone): wait for that one instead.
        if let Some(project) = self.join_cloud_setup(&source, &request_id).await? {
            return Ok(project);
        }
        {
            let mut clones = self
                .clones
                .lock()
                .map_err(|_| "The clone list is unavailable.".to_string())?;
            if clones.contains_key(&request_id) {
                return Err("That clone is already running.".into());
            }
            clones.insert(request_id.clone(), clone::Progress::stage("Finding the repository"));
        }
        let home = self.machine.home();
        let path = self.machine.tool_path();
        let (clones, id) = (self.clones.clone(), request_id.clone());
        let cloned = blocking(move || {
            let home = home?;
            let progress = |progress: clone::Progress| {
                if let Ok(mut clones) = clones.lock() {
                    clones.insert(id.clone(), progress);
                }
            };
            let source = clone::parse(&source)?;
            // A cloud machine that started from a saved copy of this repository only fetches its new commits.
            if let Some(dest) = clone::warm(&source, &parent, &home, &path, None, &|_| {}, &progress) {
                return Ok(dest);
            }
            let plan = clone::plan(&source, &path)?;
            let dest = clone::destination(&parent, &plan.name, &home)?;
            clone::clone(&plan, &dest, &path, &progress)?;
            // No repository code runs here: a clone only clones (the boot setup is a person's or organization's opt-in).
            Ok(dest)
        })
        .await;
        if let Ok(mut clones) = self.clones.lock() {
            clones.remove(&request_id);
        }
        let dest = cloned?;
        let wb = self.wb.clone();
        blocking(move || wb.add_project(&dest)).await
    }

    /// Where the clone `request_id` stands; None once it ended, or for an id no clone has.
    pub fn clone_progress(&self, request_id: &str) -> Option<clone::Progress> {
        self.clones.lock().ok()?.get(request_id).cloned()
    }

    /// Starts the cloud machine's own setup of GitHub's `repo` (`owner/name`), once for the host's life, with its state
    /// saved in `file` and its GitHub token from `token` ([`cloud_setup`]). nexus-host calls it at boot when
    /// Gateway's `cloud.json` names a setup. Nothing runs again when the file says it is ready or failed.
    pub fn begin_cloud_setup(&self, repo: String, file: PathBuf, token: Arc<dyn cloud_setup::TokenSource>) {
        let Ok(home) = self.machine.home() else {
            return;
        };
        let Some(start) = self.setup.claim(&repo, file, token, &home) else {
            // Ready already (a wake, a restart): the repository is there, so its `.agents/resume` runs.
            self.resume_cloud();
            return;
        };
        let api = self.clone();
        tokio::spawn(async move { api.run_cloud_setup(start).await });
    }

    /// Gives the repository's `.agents/resume` the agent's environment (`env` answers None while the person's
    /// credentials are not on the machine). nexus-host sets it before [`Api::begin_cloud_setup`].
    pub fn set_cloud_resume_env(&self, env: cloud_setup::ResumeEnv) {
        self.setup.set_resume_env(env);
    }

    /// Runs the repository's `.agents/resume` in the background, when the machine's setup is ready and the person's
    /// credentials are in place; otherwise nothing runs yet. It stops at 10 seconds, and a failure only shows in the
    /// status line (`wb_cloud_setup`'s `resume`). Called at each host start, after the setup, and when the app
    /// provisions the machine.
    pub fn resume_cloud(&self) {
        let (setup, home) = (self.setup.clone(), self.machine.home());
        tokio::spawn(async move {
            let Ok(home) = home else {
                return;
            };
            let _ = blocking(move || Ok(cloud_setup::resume(&setup, &home, clone::AGENTS_RESUME_LIMIT))).await;
        });
    }

    /// Where the machine's own setup stands (`wb_cloud_setup`): `idle` when it has none, and the apps clone.
    pub fn cloud_setup(&self) -> cloud_setup::View {
        self.setup.view(|id| self.wb.project(id).ok())
    }

    /// Try again after a failed setup (`wb_cloud_setup_retry`): it runs again with a new token, from the clone, or
    /// only `.agents/setup` when that failed and the clone stands. Answers where it stands; a setup that had not
    /// failed goes on as it was.
    pub fn cloud_setup_retry(&self) -> cloud_setup::View {
        if let Some(start) = self.setup.restart() {
            let api = self.clone();
            tokio::spawn(async move { api.run_cloud_setup(start).await });
        }
        self.cloud_setup()
    }

    /// One run of the machine's own setup: Gateway's token, then the clone (or the saved copy) and `.agents/setup`
    /// ([`cloud_setup::run`]), then the project. A refused token leaves the clone to the app ([`cloud_setup::Setup::withdraw`]).
    /// A run of `.agents/setup` alone (`SetupAt`) asks for no token: the clone is there, and the grant may have ended.
    async fn run_cloud_setup(&self, start: cloud_setup::Start) {
        let (Some(repo), Some(source)) = (self.setup.repo(), self.setup.token_source()) else {
            return;
        };
        let auth = match start {
            cloud_setup::Start::SetupAt(_) => None,
            cloud_setup::Start::Clone => {
                let token = match source.token().await {
                    Ok(token) => token,
                    Err(cloud_setup::TokenError::Withdrawn(why)) => return self.setup.withdraw(&why),
                    Err(cloud_setup::TokenError::Failed(why)) => {
                        return self.setup.fail(format!("Could not get access to {repo}: {why}"));
                    }
                };
                let Some(auth) = clone::GithubAuth::new(&token) else {
                    return self
                        .setup
                        .fail("Gateway's token for the repository has an unexpected shape.".into());
                };
                Some(auth)
            }
        };
        let (setup, wb) = (self.setup.clone(), self.wb.clone());
        let home = self.machine.home();
        let path = self.machine.tool_path();
        let done = blocking(move || {
            let home = home?;
            let known = |theirs: &Path| {
                wb.projects()
                    .is_ok_and(|projects| projects.iter().any(|p| Path::new(&p.path) == theirs))
            };
            let dest = cloud_setup::run(&setup, &repo, start, &home, &path, auth.as_ref(), &known)?;
            wb.add_project(&dest)
        })
        .await;
        match done {
            Ok(project) => {
                self.setup.ready(project.id);
                source.done().await;
                // The person's credentials may be on the machine already (the app connected during the setup).
                self.resume_cloud();
            }
            Err(error) => self.setup.fail(error),
        }
    }

    /// When the machine sets `source` up itself: waits for that setup, with its progress as clone `request_id`'s,
    /// and answers its project, or why it failed. None when it does not (another repository, no setup of its own,
    /// or a failed one: the app's clone then runs as usual).
    async fn join_cloud_setup(&self, source: &str, request_id: &str) -> Result<Option<Project>> {
        let Some(repo) = self.setup.repo() else {
            return Ok(None);
        };
        let same = clone::parse(source)
            .ok()
            .and_then(|source| clone::github_key(&source))
            .is_some_and(|key| key == repo.to_ascii_lowercase());
        if !same {
            return Ok(None);
        }
        let report = |progress: Option<clone::Progress>| {
            if let Ok(mut clones) = self.clones.lock() {
                match progress {
                    Some(progress) => clones.insert(request_id.to_string(), progress),
                    None => clones.remove(request_id),
                };
            }
        };
        while self.setup.running() {
            report(self.setup.stage());
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        report(None);
        if let Some(reason) = self.setup.failure() {
            if self.setup.kept_clone() {
                // Its `.agents/setup` failed and the clone stands: a clone of the app's own would find the folder taken.
                return Err(format!("Setting up {repo} failed: {reason}"));
            }
            // Failed: the app's own clone runs instead, with the person's own sign-in.
            eprintln!("nexus-workbench: the machine's setup failed ({reason}); the app clones instead");
            return Ok(None);
        }
        Ok(self.setup.ready_project().and_then(|id| self.wb.project(&id).ok()))
    }

    /// `~/nexus`, created if missing, as a project: where a session starts when no folder was picked.
    pub async fn project_default(&self) -> Result<Project> {
        let path = self.machine.home()?.join("nexus");
        let wb = self.wb.clone();
        blocking(move || {
            std::fs::create_dir_all(&path).map_err(|e| format!("Could not create {}: {e}", path.display()))?;
            wb.add_project(&path)
        })
        .await
    }

    pub async fn project_remove(&self, project_id: String) -> Result<()> {
        let wb = self.wb.clone();
        blocking(move || wb.remove_project(&project_id)).await
    }

    /// Pins or unpins the project, and archives it or brings it back; None leaves that one as it is.
    pub async fn project_update(
        &self,
        project_id: String,
        pinned: Option<bool>,
        archived: Option<bool>,
    ) -> Result<Project> {
        let wb = self.wb.clone();
        blocking(move || wb.update_project(&project_id, pinned, archived)).await
    }

    /// The command run in each new worktree of the project before its first turn ([`crate::setup`]), if any.
    pub async fn project_setup(&self, project_id: String) -> Result<Option<String>> {
        let wb = self.wb.clone();
        blocking(move || wb.project_setup(&project_id)).await
    }

    /// Sets it (blank or null takes it away) and answers with what was saved.
    pub async fn project_setup_set(&self, project_id: String, script: Option<String>) -> Result<Option<String>> {
        let wb = self.wb.clone();
        blocking(move || wb.set_project_setup(&project_id, script.as_deref())).await
    }

    /// The project's commands ([`crate::scripts`]), run on request in a session's terminal.
    pub async fn project_scripts(&self, project_id: String) -> Result<Vec<ProjectScript>> {
        let wb = self.wb.clone();
        blocking(move || wb.project_scripts(&project_id)).await
    }

    /// Replaces them (an empty list takes them away) and answers with what was saved.
    pub async fn project_scripts_set(
        &self,
        project_id: String,
        scripts: Vec<ProjectScript>,
    ) -> Result<Vec<ProjectScript>> {
        let wb = self.wb.clone();
        blocking(move || wb.set_project_scripts(&project_id, scripts)).await
    }

    /// The project's new-session defaults ([`crate::project_defaults`]).
    pub async fn project_defaults(&self, project_id: String) -> Result<crate::project_defaults::ProjectDefaults> {
        let wb = self.wb.clone();
        blocking(move || wb.project_defaults(&project_id)).await
    }

    /// Sets them (all unset takes them away) and answers with what was saved.
    pub async fn project_defaults_set(
        &self,
        project_id: String,
        defaults: crate::project_defaults::ProjectDefaults,
    ) -> Result<crate::project_defaults::ProjectDefaults> {
        let wb = self.wb.clone();
        blocking(move || wb.set_project_defaults(&project_id, defaults)).await
    }

    /// The organization's scheduled sessions on this machine ([`crate::schedules`]).
    pub async fn schedules(&self) -> Result<Vec<ScheduleView>> {
        let wb = self.wb.clone();
        blocking(move || wb.schedules()).await
    }

    /// Adds a schedule (no `schedule_id`) or replaces one, and answers with what was saved.
    pub async fn schedule_save(&self, schedule_id: Option<String>, schedule: ScheduleInput) -> Result<ScheduleView> {
        let wb = self.wb.clone();
        blocking(move || wb.save_schedule(schedule_id.as_deref(), schedule, None)).await
    }

    pub async fn schedule_delete(&self, schedule_id: String) -> Result<()> {
        let wb = self.wb.clone();
        blocking(move || wb.delete_schedule(&schedule_id)).await
    }

    /// Run now: the schedule's session starts at once, counted against its daily cap.
    pub async fn schedule_run(&self, schedule_id: &str) -> Result<ThreadSummary> {
        self.wb.run_schedule_now(schedule_id).await
    }

    /// Turns the schedule's webhook on with a new token, shown only in this answer, or off (null).
    pub async fn schedule_webhook(&self, schedule_id: &str, on: bool) -> Result<Option<WebhookSecret>> {
        self.wb.set_schedule_webhook(schedule_id, on).await
    }

    pub async fn thread_create(&self, args: CreateArgs) -> Result<ThreadSummary> {
        self.thread_create_at_scope(args, None).await
    }

    pub async fn thread_create_at_scope(
        &self,
        args: CreateArgs,
        scope: Option<&StartAuthority>,
    ) -> Result<ThreadSummary> {
        self.thread_create_guarded(args, None, scope).await
    }

    pub(crate) async fn thread_create_scoped(
        &self,
        args: CreateArgs,
        identity: &crate::session_sync::Identity,
        expires: i64,
        generation: i64,
        sender: String,
        selection: Option<(SessionAgent, crate::computer::Selection)>,
    ) -> Result<ThreadSummary> {
        self.thread_create_guarded(
            args,
            Some(crate::CreationGuard {
                identity: identity.clone(),
                expires_at: expires,
                generation,
                sender,
                selection,
            }),
            None,
        )
        .await
    }
    async fn thread_create_guarded(
        &self,
        mut args: CreateArgs,
        guard: Option<crate::CreationGuard>,
        expected_scope: Option<&StartAuthority>,
    ) -> Result<ThreadSummary> {
        if let Some(scope) = expected_scope {
            self.wb.check_start_scope(scope)?;
        }
        if args.provider.is_some() {
            self.wb.check_saved_provider_creation(guard.as_ref())?;
            if !args.files.is_empty() {
                return Err("This build cannot save shared files for a local provider session.".into());
            }
            let text = args.text.clone();
            return self.thread_create_with(args, text, guard, expected_scope).await;
        }
        self.wb.project(&args.project_id)?;
        // Attached files go in the new session's own folder, so its id is fixed first (as a creation request's).
        if !args.files.is_empty() && args.request_id.is_none() {
            args.request_id = Some(uuid::Uuid::new_v4().to_string());
        }
        let id = args.request_id.as_deref().map(crate::creation_thread_id).transpose()?;
        let files = std::mem::take(&mut args.files);
        let (text, folder) = self
            .prepare_files(id.as_deref().unwrap_or_default(), &args.text, files)
            .await?;
        let created = self.thread_create_with(args, text, guard, expected_scope).await;
        // A session that never started has no use for them; one that did may be retried with the same id, which
        // saves its files again.
        if let (Err(_), Some(folder), Some(id)) = (&created, folder, id.as_deref())
            && !self.wb.has_thread(id)
        {
            let _ = std::fs::remove_dir_all(&folder);
            // The session's own folder too, which nothing else deletes. Only when empty: a retry with the same id may
            // be saving its files there.
            if let Some(session) = folder.parent() {
                let _ = std::fs::remove_dir(session);
            }
        }
        created
    }

    async fn thread_create_with(
        &self,
        args: CreateArgs,
        text: String,
        guard: Option<crate::CreationGuard>,
        expected_scope: Option<&StartAuthority>,
    ) -> Result<ThreadSummary> {
        let project = self.wb.project(&args.project_id)?;
        let orchestration = args.orchestration.unwrap_or(self.wb.config().subsessions);
        // Mentions come from the text as typed, not the file references prepare_files adds to `text`.
        let local_provider = args.provider.is_some();
        if local_provider {
            self.wb.check_saved_provider_creation(guard.as_ref())?;
        }
        let context = if local_provider {
            String::new()
        } else {
            self.mention_context(PathBuf::from(project.path), &args.text).await
        };
        // A review names only the pull request's number: its URL and the branch it targets come from the GitHub CLI.
        let review = match args
            .worktree
            .as_ref()
            .and_then(|w| w.review)
            .filter(|_| !local_provider)
        {
            Some(number) => {
                let project = self.wb.project(&args.project_id)?;
                let path = self.machine.tool_path();
                let tool_path = path.clone();
                let info = tokio::task::spawn_blocking(move || {
                    git::pull_request_info(Path::new(&project.path), number, &path)
                })
                .await
                .map_err(|e| e.to_string())??;
                Some(NewReview {
                    number: info.number,
                    url: info.url,
                    base: info.base,
                    tool_path,
                })
            }
            None => None,
        };
        let new = NewThread {
            project_id: args.project_id,
            agent: parse(&args.agent, SessionAgent::parse, "agent")?,
            model: args.model,
            permission_mode: parse(&args.permission_mode, PermissionMode::parse, "permission mode")?,
            effort: args
                .effort
                .filter(|e| !e.is_empty())
                .map(|e| parse(&e, Effort::parse, "effort"))
                .transpose()?,
            via: parse(&args.via, Via::parse, "billing")?,
            provider: args.provider,
            text,
            images: args.images,
            worktree: args.worktree.map(|w| NewWorktree {
                branch: w.branch,
                base: w.base,
                review,
                branch_prefix: w.branch_prefix,
            }),
            parent: None,
            orchestration,
            name: args.name.filter(|name| !name.is_empty()),
        };
        self.wb.check_creation_guard(guard.as_ref())?;
        match args.request_id {
            Some(id) => {
                self.wb
                    .create_thread_once_in_scope(new, &id, context, guard, expected_scope)
                    .await
            }
            None => self.wb.create_thread_in_scope(new, context, expected_scope).await,
        }
    }

    /// The open pull requests of the project's repository, from the GitHub CLI, for a review session.
    pub async fn pull_requests(&self, project_id: &str) -> Result<Vec<PullRequestInfo>> {
        let project = self.wb.project(project_id)?;
        let path = self.machine.tool_path();
        // The same list, and cache, as the composer's "#" picker.
        blocking(move || mentions::pull_requests(Path::new(&project.path), &path)).await
    }

    /// What a new session's worktree in the project can start from, the default base first ([`git::worktree_bases`]).
    pub async fn worktree_bases(&self, project_id: &str) -> Result<Vec<String>> {
        let project = self.wb.project(project_id)?;
        blocking(move || crate::git::worktree_bases(Path::new(&project.path))).await
    }

    /// The files of a session's folder (`thread_id`), or of a project's before its first session starts
    /// (`project_id`), for the composer's @-mention picker ([`files::list`]).
    pub async fn files(&self, thread_id: Option<String>, project_id: Option<String>) -> Result<files::FileList> {
        let dir = self.folder_of(thread_id, project_id)?;
        blocking(move || files::list(&dir)).await
    }

    /// One file of a session's folder, for the file viewer ([`project_files::read`]: refused outside the folder).
    pub async fn file_read(&self, thread_id: &str, path: String) -> Result<project_files::FileText> {
        let dir = self.wb.folder(thread_id)?;
        blocking(move || project_files::read(&dir, &path)).await
    }

    /// A selected output, including outside the project, on this session's machine.
    pub async fn file_preview(
        &self,
        thread_id: &str,
        path: String,
        offset: Option<u64>,
    ) -> Result<crate::output_files::Preview> {
        let dir = self.wb.folder(thread_id)?;
        blocking(move || match offset {
            Some(offset) => crate::output_files::read_piece(&dir, &path, offset),
            None => crate::output_files::read(&dir, &path),
        })
        .await
    }

    /// A piece of an image or video that a reply names, inside this session's folder on its machine
    /// ([`crate::output_media::read`]).
    pub async fn file_media(
        &self,
        thread_id: &str,
        path: String,
        offset: u64,
    ) -> Result<crate::output_media::MediaChunk> {
        let dir = self.wb.folder(thread_id)?;
        blocking(move || crate::output_media::read(&dir, &path, offset)).await
    }

    /// The lines of a session's files that hold `query`, for project search ([`project_files::search`]).
    pub async fn file_search(
        &self,
        thread_id: &str,
        query: String,
        case_sensitive: bool,
    ) -> Result<project_files::SearchResult> {
        let dir = self.wb.folder(thread_id)?;
        blocking(move || project_files::search(&dir, &query, case_sensitive)).await
    }

    /// A session's folder, or a project's before its first session starts.
    fn folder_of(&self, thread_id: Option<String>, project_id: Option<String>) -> Result<PathBuf> {
        match (thread_id, project_id) {
            (Some(thread_id), _) => self.wb.folder(&thread_id),
            (None, Some(project_id)) => Ok(PathBuf::from(self.wb.project(&project_id)?.path)),
            (None, None) => Err("Name a session or a project.".into()),
        }
    }

    /// The open pull requests of that folder's repository, for the composer's "#" picker ([`mentions::pull_requests`]:
    /// kept a few seconds, so typing after "#" doesn't run the GitHub CLI each time).
    pub async fn mention_prs(
        &self,
        thread_id: Option<String>,
        project_id: Option<String>,
    ) -> Result<Vec<PullRequestInfo>> {
        let dir = self.folder_of(thread_id, project_id)?;
        let path = self.machine.tool_path();
        blocking(move || mentions::pull_requests(&dir, &path)).await
    }

    /// What the agent gets after a message from the folder `dir`, about the sessions and pull requests it mentions
    /// ([`mentions`]). A session that can't be read says so; a number the GitHub CLI doesn't know as a pull request
    /// (an issue, or `gh` signed out), or doesn't describe within [`mentions::LOOKUP_FOR`], is left as typed.
    async fn mention_context(&self, dir: PathBuf, text: &str) -> String {
        let found = mentions::parse(text);
        if found.is_empty() {
            return String::new();
        }
        let wb = self.wb.clone();
        let sessions = found.sessions;
        let mut entries = blocking(move || {
            let threads = wb.threads().unwrap_or_default();
            Ok(sessions
                .iter()
                .map(|id| {
                    threads
                        .iter()
                        .find(|thread| &thread.id == id)
                        .and_then(|thread| Some(mentions::session_entry(thread, &wb.events(id, 0).ok()?)))
                        .unwrap_or_else(|| mentions::missing_session(id))
                })
                .collect::<Vec<_>>())
        })
        .await
        .unwrap_or_default();
        let path = self.machine.tool_path();
        entries.extend(
            mentions::pull_request_entries(&found.pull_requests, mentions::LOOKUP_FOR, move |number| {
                mentions::pull_request(&dir, number, &path)
            })
            .await,
        );
        mentions::block(&entries)
    }

    /// Commits, pushes and opens (or updates) the session's pull request with the GitHub CLI.
    pub async fn pull_request(&self, thread_id: &str, title: &str, body: &str) -> Result<ThreadSummary> {
        let path = self.machine.tool_path();
        self.wb.open_pull_request(thread_id, title, body, &path).await
    }

    /// The session's folder as git sees it (its git panel).
    pub async fn git_state(&self, thread_id: &str, repo: Option<&str>) -> Result<GitState> {
        self.wb.git_state(thread_id, repo).await
    }

    /// Commits what changed in the session's folder (or `repo` inside it), then pushes when `push`.
    pub async fn commit(&self, thread_id: &str, repo: Option<&str>, message: &str, push: bool) -> Result<GitState> {
        self.wb.commit(thread_id, repo, message, push).await
    }

    /// What "Write it" drafts a commit message or a pull request's text from: the change and the repository's style.
    pub async fn draft_context(
        &self,
        thread_id: &str,
        repo: Option<&str>,
        kind: draft::Kind,
    ) -> Result<draft::Context> {
        self.wb.draft_context(thread_id, repo, kind).await
    }

    /// Checks out another local branch in the session's project folder, or `repo` inside it.
    pub async fn switch_branch(&self, thread_id: &str, repo: Option<&str>, branch: &str) -> Result<GitState> {
        self.wb.switch_branch(thread_id, repo, branch).await
    }

    /// Fetches what the branch is compared against (not mid-turn, at most once a minute), then reads the git state.
    pub async fn git_fetch(&self, thread_id: &str, repo: Option<&str>) -> Result<GitState> {
        self.wb.fetch(thread_id, repo).await
    }

    /// Pulls the branch's upstream: a fast-forward, or with `rebase`, a rebase that is undone if it conflicts.
    pub async fn git_pull(&self, thread_id: &str, repo: Option<&str>, rebase: bool) -> Result<Pull> {
        self.wb.pull(thread_id, repo, rebase).await
    }

    /// Makes a new branch at the current commit in the session's project folder, or `repo` inside it, and checks it out.
    pub async fn branch_create(&self, thread_id: &str, repo: Option<&str>, branch: &str) -> Result<GitState> {
        self.wb.create_branch(thread_id, repo, branch).await
    }

    /// The turns whose changes can be shown, by their end events' numbers.
    pub async fn changed_turns(&self, thread_id: &str) -> Result<Vec<i64>> {
        self.wb.changed_turns(thread_id).await
    }

    /// What one turn changed, or (`turn` None) the whole session, as a unified diff.
    pub async fn changes(&self, thread_id: &str, turn: Option<i64>) -> Result<Changes> {
        self.wb.changes(thread_id, turn).await
    }

    /// The files the turn ending at event `turn` changed, with its lines added and removed.
    pub async fn turn_files(&self, thread_id: &str, turn: i64) -> Result<Vec<crate::turn_files::TurnFile>> {
        self.wb.turn_files(thread_id, turn).await
    }

    /// The session's folder against its base branch, one commit, or HEAD (`source`), as a unified diff.
    pub async fn diff(&self, thread_id: &str, source: DiffSource) -> Result<Changes> {
        self.wb.diff(thread_id, source).await
    }

    /// The base branch and the newest commits the Changes view can compare against.
    pub async fn diff_targets(&self, thread_id: &str) -> Result<DiffTargets> {
        self.wb.diff_targets(thread_id).await
    }

    /// How many files reverting to the turn ending at event `turn` would change.
    pub async fn revert_preview(&self, thread_id: &str, turn: i64) -> Result<u32> {
        self.wb.revert_preview(thread_id, turn).await
    }

    /// Puts the session's files back as they were after that turn; the agent forgets the later turns.
    pub async fn revert_to_turn(&self, thread_id: &str, turn: i64) -> Result<u32> {
        self.wb.revert_to_turn(thread_id, turn).await
    }

    /// Every worktree Settings › Worktrees lists ([`crate::cleanup::Listed`]).
    pub async fn worktrees(&self) -> Result<Vec<crate::cleanup::Listed>> {
        let wb = self.wb.clone();
        blocking(move || wb.worktrees()).await
    }

    /// Removes a worktree no session names, unless that would lose work.
    pub async fn worktree_remove_orphan(&self, path: &str) -> Result<()> {
        self.wb.remove_orphan_worktree(path).await
    }

    pub async fn rewind_message(&self, thread_id: &str, seq: i64) -> Result<()> {
        self.wb.rewind_message(thread_id, seq).await
    }

    /// Takes a session's worktree off the disk, unless that would lose work.
    pub async fn worktree_remove(&self, thread_id: &str) -> Result<ThreadSummary> {
        self.wb.remove_worktree(thread_id).await
    }

    /// Removes the worktrees the rules match, where nothing would be lost ([`cleanup`]). `merged`: how to tell a merged
    /// pull request; None: ask the GitHub CLI on this machine.
    pub async fn worktree_cleanup(
        &self,
        rules: &cleanup::Rules,
        merged: Option<cleanup::MergedHead>,
    ) -> Result<cleanup::Report> {
        let merged = merged.unwrap_or_else(|| cleanup::merged_by_gh(self.machine.tool_path()));
        self.wb.clean_worktrees(rules, merged).await
    }

    /// Moves a session from its project's folder into a new worktree, with what the folder had not committed
    /// ([`Workbench::move_to_worktree`]).
    pub async fn move_to_worktree(&self, thread_id: &str) -> Result<ThreadSummary> {
        self.wb.move_to_worktree(thread_id).await
    }

    /// Deletes a session for good ([`Workbench::delete_thread`]).
    /// Its attached files go with it. The session is gone by then, so files that won't go are not an error: the delete
    /// worked, and a second one would fail on the missing session. They are what it returns, for the person to see.
    pub async fn thread_delete(&self, thread_id: &str) -> Result<Option<String>> {
        self.thread_delete_with_options(thread_id, false).await
    }

    async fn thread_delete_with_options(&self, thread_id: &str, discard_uncommitted: bool) -> Result<Option<String>> {
        let mut left = self
            .wb
            .delete_thread_with_options(thread_id, discard_uncommitted)
            .await?;
        let id = thread_id.to_string();
        let attached = match self.attachments_root() {
            Ok(root) => blocking(move || attachments::remove(&root, &id)).await,
            Err(error) => Err(error),
        };
        if let Err(error) = attached {
            left.push(format!("Its attached files are still there ({error})."));
        }
        if left.is_empty() {
            return Ok(None);
        }
        eprintln!("nexus-workbench: {thread_id}: {}", left.join(" "));
        Ok(Some(format!("The session is deleted. {}", left.join(" "))))
    }

    /// Forks a session from the turn that ended at event `turn` (None: the latest).
    pub async fn thread_fork(&self, thread_id: &str, turn: Option<i64>) -> Result<ThreadSummary> {
        self.wb.fork_thread(thread_id, turn).await
    }

    pub async fn thread_fork_with_subscription(
        &self,
        thread_id: &str,
        turn: Option<i64>,
        subscription: Option<crate::ForkSubscription>,
    ) -> Result<ThreadSummary> {
        self.wb
            .fork_thread_with_subscription(thread_id, turn, subscription)
            .await
    }

    /// Queues a fork's result in the session it was forked from (`Workbench::send_back`).
    pub async fn send_back(&self, thread_id: &str) -> Result<()> {
        self.wb.send_back(thread_id).await
    }

    /// Opens a side chat on a session (`Workbench::side_chat`).
    pub async fn side_chat(&self, thread_id: &str) -> Result<ThreadSummary> {
        self.wb.side_chat(thread_id).await
    }

    /// Closes a side chat, deleting it when the person never wrote in it (`Workbench::close_side_chat`).
    pub async fn side_chat_close(&self, thread_id: &str) -> Result<bool> {
        self.wb.close_side_chat(thread_id).await
    }

    // The calls below run their store work off the async workers: reading a long session's log or rescanning it for an
    // approval's suggestion must not hold up a worker.

    pub async fn thread_events(&self, thread_id: String, after_seq: i64) -> Result<Vec<StoredEvent>> {
        let wb = self.wb.clone();
        blocking(move || wb.events(&thread_id, after_seq)).await
    }

    /// A page of a session's log: the latest when `before_seq` is None (page.rs).
    pub async fn thread_page(
        &self,
        thread_id: String,
        before_seq: Option<i64>,
        limit: Option<usize>,
    ) -> Result<EventsPage> {
        let wb = self.wb.clone();
        blocking(move || wb.events_page(&thread_id, before_seq, limit)).await
    }

    /// The scope's sessions whose prompts or answers match `query` (3+ letters a word), newest first, at most 20.
    pub async fn search(&self, query: String) -> Result<Vec<SearchHit>> {
        let wb = self.wb.clone();
        blocking(move || wb.search(&query, 20)).await
    }

    /// Where attached files are kept: the workbench's folder for them, else `~/.nexus/attachments` on this machine.
    fn attachments_root(&self) -> Result<PathBuf> {
        match self.wb.attachments_root() {
            Some(root) => Ok(root),
            None => Ok(self.machine.home()?.join(".nexus/attachments")),
        }
    }

    /// Saves the files in session `thread_id`'s attachments folder and adds their paths to `text`.
    async fn prepare_files(
        &self,
        thread_id: &str,
        text: &str,
        files: Vec<FileInput>,
    ) -> Result<(String, Option<PathBuf>)> {
        if files.is_empty() {
            return Ok((text.into(), None));
        }
        let root = self.attachments_root()?;
        let (thread_id, text) = (thread_id.to_string(), text.to_string());
        blocking(move || attachments::prepare(&root, &thread_id, &text, &files)).await
    }

    /// Adds a piece to an attachment being uploaded in pieces (attachments.rs); a message then names it by `upload`.
    pub async fn attachment_chunk(&self, chunk: attachments::Chunk) -> Result<attachments::Progress> {
        let root = self.attachments_root()?;
        blocking(move || attachments::write_chunk(&root, &chunk)).await
    }

    /// Deletes an upload for session `thread_id` (None: one from an older app), finished or not.
    pub async fn attachment_cancel(&self, upload_id: String, thread_id: Option<String>) -> Result<()> {
        let root = self.attachments_root()?;
        blocking(move || attachments::cancel(&root, thread_id.as_deref(), &upload_id)).await
    }

    pub async fn send_files(
        &self,
        thread_id: &str,
        text: &str,
        images: Vec<Image>,
        files: Vec<FileInput>,
    ) -> Result<()> {
        self.send_files_as(thread_id, text, images, files, false).await
    }

    /// [`Api::send_files`], `queued`: the message waits for the running turn to end, editable and cancellable, rather
    /// than joining it ([`Workbench::send_message_as`]).
    pub async fn send_files_as(
        &self,
        thread_id: &str,
        text: &str,
        images: Vec<Image>,
        files: Vec<FileInput>,
        queued: bool,
    ) -> Result<()> {
        self.wb.check_saved_provider_launch(thread_id)?;
        if !files.is_empty() && self.wb.record(thread_id)?.summary.provider.is_some() {
            return Err("This computer cannot save shared files for a local provider session.".into());
        }
        let dir = self.wb.folder(thread_id)?;
        let context = self.mention_context(dir, text).await;
        let (text, _) = self.prepare_files(thread_id, text, files).await?;
        self.wb
            .send_message_as(thread_id, &text, images, &context, queued)
            .await
    }

    pub async fn send(&self, thread_id: &str, text: &str, images: Vec<Image>) -> Result<()> {
        self.wb.send_message(thread_id, text, images).await
    }

    pub async fn code_control(&self) -> Result<crate::session_sync::control::Settings> {
        let wb = self.wb.clone();
        blocking(move || wb.code_control()).await
    }

    pub async fn set_code_control(&self, enabled: bool) -> Result<crate::session_sync::control::Settings> {
        let wb = self.wb.clone();
        blocking(move || wb.set_code_control(enabled)).await
    }

    pub async fn approve(
        &self,
        thread_id: String,
        request_id: String,
        decision: Decision,
        message: Option<String>,
        suggestion: Option<usize>,
        answers: Option<Map<String, Value>>,
    ) -> Result<()> {
        self.wb
            .answer(&thread_id, &request_id, decision, message, suggestion, answers)
            .await
    }

    /// Takes back a message queued behind the running turn (`seq`: its event's number) before the agent reads it.
    pub async fn queue_cancel(&self, thread_id: String, seq: i64) -> Result<()> {
        let wb = self.wb.clone();
        blocking(move || wb.queue_cancel(&thread_id, seq)).await
    }

    /// Rewrites a queued message before the agent reads it.
    pub async fn queue_edit(&self, thread_id: String, seq: i64, text: String) -> Result<()> {
        let wb = self.wb.clone();
        blocking(move || wb.queue_edit(&thread_id, seq, &text)).await
    }

    /// Moves a queued message just before the queued message `before`, or last when `before` is None.
    pub async fn queue_move(&self, thread_id: String, seq: i64, before: Option<i64>) -> Result<()> {
        let wb = self.wb.clone();
        blocking(move || wb.queue_move(&thread_id, seq, before)).await
    }

    /// Sends a queued message into the running turn now ("Send now").
    pub async fn queue_steer(&self, thread_id: String, seq: i64) -> Result<()> {
        let wb = self.wb.clone();
        blocking(move || wb.queue_steer(&thread_id, seq)).await
    }

    /// Stops the current turn. Call it on the async runtime: stopping spawns the escalation timer.
    pub fn interrupt(&self, thread_id: &str) -> Result<()> {
        self.wb.interrupt(thread_id)
    }

    /// Stops one sub-agent or background command ([`Workbench::task_stop`]).
    pub async fn connection(&self, thread_id: &str) -> Result<crate::event::SessionConnection> {
        self.wb.connection(thread_id).await
    }

    pub async fn compact(&self, thread_id: &str, instructions: Option<String>) -> Result<()> {
        self.wb.compact(thread_id, instructions).await
    }

    pub async fn task_stop(&self, thread_id: &str, task_id: &str) -> Result<()> {
        self.wb.task_stop(thread_id, task_id).await
    }

    /// Moves a foreground command to the background ([`Workbench::task_background`]).
    pub async fn task_background(&self, thread_id: &str, tool_use_id: &str) -> Result<bool> {
        self.wb.task_background(thread_id, tool_use_id).await
    }

    /// A background command's latest output ([`Workbench::task_output`]).
    pub async fn task_output(&self, thread_id: &str, task_id: &str) -> Result<crate::event::TaskOutput> {
        self.wb.task_output(thread_id, task_id).await
    }

    /// Ends the agent process. On the async runtime, as [`Api::interrupt`].
    pub fn stop(&self, thread_id: &str) -> Result<()> {
        self.wb.stop(thread_id)
    }

    /// Ends the agent process and says so in the log ([`Workbench::restart`]). On the async runtime, as [`Api::stop`].
    pub fn restart(&self, thread_id: &str) -> Result<()> {
        self.wb.restart(thread_id)
    }

    /// Stops the session's running setup script; its waiting messages then go on to the agent. Nothing when none
    /// runs.
    pub fn setup_stop(&self, thread_id: &str) {
        self.wb.stop_setup(thread_id);
    }

    pub async fn thread_update(&self, args: UpdateArgs) -> Result<ThreadSummary> {
        let UpdateArgs {
            thread_id,
            title,
            model,
            permission_mode,
            effort,
            fast,
            archived,
            orchestration,
            pinned,
            continue_at_reset,
            snoozed_until,
            keep_listed,
            settled,
            name,
        } = args;
        let permission_mode = permission_mode
            .map(|mode| parse(&mode, PermissionMode::parse, "permission mode"))
            .transpose()?;
        // `effort: ""` is the model's default again; a missing `effort` leaves it.
        let effort = effort
            .map(|e| {
                if e.is_empty() {
                    Ok(None)
                } else {
                    parse(&e, Effort::parse, "effort").map(Some)
                }
            })
            .transpose()?;
        // `model: ""` clears it (the organization's default); a missing `model` leaves it.
        let model = model.map(|m| Some(m).filter(|m| !m.is_empty()));
        let wb = self.wb.clone();
        blocking(move || {
            wb.update(
                &thread_id,
                ThreadPatch {
                    title,
                    model,
                    permission_mode,
                    effort,
                    fast,
                    archived,
                    orchestration,
                    pinned,
                    continue_at_reset,
                    snoozed_until: snoozed_until.map(|at| Some(at).filter(|at| *at > 0)),
                    keep_listed,
                    settled,
                    name: name.map(|name| Some(name).filter(|name| !name.is_empty())),
                },
            )
        })
        .await
    }
}
