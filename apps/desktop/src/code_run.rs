//! Code sessions run here: this build owns the Nexus app's session store (`workbench.sqlite3`) and starts the agents
//! itself, through `nexus-workbench`.
//!
//! Only one process may own the store. [`CodeRunner::start`] takes its lock (`run/workbench.lock`). When the Nexus app
//! or its background service holds it, the lock is refused and this build stays read-only (`code.rs` still reads the
//! store) until the person takes over (`owner.rs`). The lock goes with the runner, and the operating system releases
//! it after a crash too.
//!
//! The [`Host`] here is our own, not nexus-host's `HeadlessHost`: nexus-host needs Unix sockets and `nix`, so it does
//! not build on Windows. The launch is the same as nexus-host's: Gateway picks the model and opens
//! the Nexus agent session, and nexus-client's `agent_launch` gives the agent the environment that routes it through
//! Gateway. The agent gets the MCP servers of the Nexus app's settings and the built-in Nexus server (`mcp.rs`), and
//! starts on the plan account the Nexus app's settings name.
//!
//! Commands run on a tokio runtime of our own, as Gateway calls do (`gateway.rs`). What the workbench reports comes back
//! to the UI as a [`CodeEvent`] on a channel.
//!
//! The runner also drives the once-a-minute upkeep: idle agents stop, sessions continue at their plan's reset, and
//! scheduled sessions start (`nexus_workbench::schedules`). It also runs the worktree sweep: a minute after the store
//! is taken, then every hour, by the cleanup rules saved at that moment (`nexus_workbench::cleanup`), here and on each
//! connected server (`nexus_remote::cleanup`). Both loops belong to the runner, so they run only while this build
//! holds the store's lock, and they stop when the runner goes.
//!
//! Session sync runs with the store too. It is on by default, and runs only with the managed `AllowLocalCache`, a
//! sign-in and this computer's device key. The backend comes from `nexus_desktop_core::session_sync`.
//!
//! With "Keep agents running when Nexus quits" on, the runner does not take the lock: the background service holds it
//! and runs the sessions (`service.rs`). The commands go there (`nexus_remote::engine`), the service runs its own upkeep,
//! and the sweep asks it to sweep. Outside access and the pull request watch reach its sessions through it too.
//! Quitting closes the connection; the service and its agents go on.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::future::BoxFuture;
use nexus_client::Client;
use nexus_client::agent_setup::{codex_web_search, subscriptions_allowed};
use nexus_client::agents::{self, LaunchOptions, Route};
use nexus_client::commit_trailer::{self, Surface};
use nexus_desktop_core::session_sync;
use nexus_desktop_core::settings::Settings;
use nexus_workbench::api::{AgentInfo, Api, CreateArgs, Machine, UpdateArgs, WorktreeArgs};
use nexus_workbench::cleanup;
use nexus_workbench::driver::label;
use nexus_workbench::event::{Decision, Effort, PermissionMode, SessionAgent, Usage, Via, WbEvent};
use nexus_workbench::owner::StoreOwner;
use nexus_workbench::process::{self, Shell, Spawn};
use nexus_workbench::schedules::{ScheduleInput, ScheduleView, WebhookSecret};
use nexus_workbench::store::Store;
use nexus_workbench::{
    Config, Emit, GitState, Host, Launch, LaunchRequest, ProcessEnd, Workbench, accounts, nexus_runtime,
};
use serde_json::{Map, Value, json};

use crate::app_socket::AppSocket;
use crate::notify::Notifier as _;
use crate::outside::OutsideAccess;
use crate::owner::Refused;
use crate::secret_request::GpuiSecrets;
use nexus_remote::engine::Engine;
use nexus_workbench::agent_launch::{LocalLaunch, admit_local_launch};
use nexus_workbench::provider::LocalProvidersOptIn;
use nexus_workbench::session_scope::SessionScope;

/// How often idle agents are stopped, sessions waiting for a plan's reset are continued and due schedules start.
const TICK: Duration = Duration::from_secs(60);

/// When the runner's loops run, where they read the Nexus app's settings, and where its sockets keep their files.
/// Tests shorten the times and hand in their own settings and folders.
#[derive(Clone)]
struct Loops {
    local_providers_opt_in: LocalProvidersOptIn,
    /// The app's active sign-in. Tests provide an isolated source without reading the keychain.
    client: crate::backend::SessionClient,
    /// The upkeep's minute ([`TICK`]).
    tick: Duration,
    /// The first worktree sweep waits this long after the store is taken.
    sweep_first: Duration,
    /// Then a sweep runs this often.
    sweep_every: Duration,
    /// The settings saved now: read at start, and again before each sweep for its rules.
    settings: Arc<dyn Fn() -> Settings + Send + Sync>,
    /// Where `request_secret` writes its files (`secret_request.rs`).
    secret_files: PathBuf,
    /// Where outside access listens (`outside.rs`).
    outside_socket: Option<PathBuf>,
    /// Whether sessions should run in the background service now: the switch and the policy (`service::wanted`).
    keep_running: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The managed `AllowLocalCache`: off keeps the store and outside access's grants in memory.
    local_cache: bool,
    /// Where the agents' browser calls go: the windows (`side::browser::agent::post`), on this build's socket or from
    /// the background service.
    browser: crate::app_socket::BrowserPost,
    /// Whether this runner starts the app's pull request watch (`pr_watch.rs`), which is process-wide. Tests leave it
    /// off: a running watch reaches every window test from the `nexus-code` thread.
    pull_watch: bool,
}

impl Loops {
    /// The app's times, its `settings.json` and its outside access socket.
    fn app(client: crate::backend::SessionClient, local_providers_opt_in: LocalProvidersOptIn) -> Loops {
        Loops {
            local_providers_opt_in,
            client,
            tick: TICK,
            sweep_first: cleanup::FIRST_SWEEP_AFTER,
            sweep_every: cleanup::SWEEP_EVERY,
            settings: Arc::new(crate::desktop_settings::load),
            secret_files: crate::paths::cache_dir().join(nexus_desktop_mcp::secret_request::FOLDER),
            outside_socket: crate::paths::data_dir().and_then(|data| crate::outside::socket_path(&data)),
            keep_running: Arc::new(crate::service::wanted_now),
            local_cache: crate::managed::local_cache(crate::managed::policy()),
            browser: Arc::new(crate::side::browser::agent::post),
            pull_watch: true,
        }
    }
}

/// What the runner tells the UI.
#[derive(Debug, Clone)]
pub enum CodeEvent {
    /// A session, its events or the projects changed: read the store again.
    Changed,
    /// The session that draft view `draft` (`draft-N`) asked for was made: that view, if open, takes `id`.
    Created { draft: String, id: String },
    /// A project was added (its id); the new-session form selects it.
    ProjectAdded {
        id: String,
        scope: String,
        generation: u64,
        origin: Option<gpui_kit::EntityId>,
    },
    /// Add project was refused; the window that asked shows the reason.
    ProjectAddFailed {
        error: String,
        origin: Option<gpui_kit::EntityId>,
    },
    /// A clone finished: the project it opened, or why it failed (the clone form shows it).
    Cloned(Result<String, String>),
    /// The agents on this computer, with their models.
    Agents(Vec<AgentInfo>),
    /// Who may pay for new sessions: the organization's policy and Gateway's route for each agent.
    Access {
        /// The fingerprint of the sign-in it was read with (`secrets::marker`): another one's answer is not this one's.
        credential: Option<String>,
        access: AgentAccess,
    },
    /// A command other than a start failed, for a person. It changes no start.
    Failed(String),
    /// The session that draft view `draft` asked for could not be made, for a person.
    StartFailed { draft: String, error: String },
    /// A message did not reach session `thread_id`: its text and files go back to the composer, with `error`.
    Unsent {
        thread_id: String,
        text: String,
        attachments: crate::attachments::Draft,
        error: String,
    },
    /// A session's answer streamed further (`stream.rs`): only the open view draws again.
    Streamed,
    /// Session `thread_id`'s turn ended: a Keep updated share of it is shared again (`session_share.rs`).
    TurnEnded(String),
}

/// A new session, as the form under the Code view fills it.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub project_id: String,
    pub agent: SessionAgent,
    /// None: the agent's default, by Gateway's rules.
    pub model: Option<String>,
    /// `default`, `auto`, `acceptEdits`, `plan` or `bypassPermissions` (`PermissionMode::as_str`).
    pub permission_mode: String,
    /// None: the model's default effort.
    pub effort: Option<Effort>,
    /// Who pays: the organization through Gateway, or the person's own plan.
    pub via: Via,
    pub provider: Option<nexus_workbench::provider::InstanceId>,
    /// Work in a new git worktree of the project, on a branch named from the first message.
    pub worktree: bool,
    /// Check out this pull request of the project's repository in the reviews folder, to review it (with `worktree`).
    pub review: Option<u64>,
    /// What the new worktree starts from (with `worktree`); None: main, else the repository's default base.
    pub base: Option<String>,
    pub text: String,
    /// The images and files attached to the first message.
    pub attachments: crate::attachments::Draft,
}

/// Prefer main even when the repository's default or checked-out branch differs. Repositories without main keep
/// their own default (the first base returned by `wb_worktree_bases`).
pub(crate) fn default_worktree_base(bases: &[String]) -> Option<&String> {
    ["origin/main", "main"]
        .into_iter()
        .find_map(|name| bases.iter().find(|base| base.as_str() == name))
        .or_else(|| bases.first())
}

/// Who pays, by kind: the payer chip's icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payer {
    Checking,
    Plan,
    Key,
    Credits,
    None,
}

/// Who may pay for new sessions, as the Nexus app reads it (`route.ts`): the organization's `agentSubscriptions`, and
/// Gateway's answer to `GET /v1/agents/access`.
#[derive(Debug, Clone, Default)]
pub struct AgentAccess {
    /// The organization allows sessions on a person's own plan. False when Gateway did not answer.
    pub subscriptions: bool,
    /// Gateway's answer; None when it gave none (an old Gateway, an error).
    pub answer: Option<Value>,
}

impl AgentAccess {
    /// The route a new session for `agent` takes before the person picks one (`chooseRoute`): Gateway when plans are
    /// off; the person's saved preference; the plan when Gateway's route cannot run (no provider key) and the plan
    /// can; else Gateway's default.
    pub fn default_via(&self, agent: SessionAgent) -> Via {
        let Some(client) = agent.coding().filter(|_| self.subscriptions) else {
            return Via::Nexus;
        };
        let Some(answer) = &self.answer else {
            return Via::Nexus;
        };
        let entry = nexus_client::agent_setup::agent_entry(Some(answer), client);
        let route = |name: &str| {
            if name == "subscription" {
                Via::Subscription
            } else {
                Via::Nexus
            }
        };
        let chosen = route(entry["route"].as_str().unwrap_or("gateway"));
        if entry["from"] == "preference" {
            return chosen;
        }
        let available = |r: Route| {
            nexus_client::agent_setup::route_of(&entry, r).map(|view| view["available"] != Value::Bool(false))
        };
        if chosen == Via::Nexus
            && available(Route::Gateway) == Some(false)
            && available(Route::Subscription) != Some(false)
        {
            return Via::Subscription;
        }
        chosen
    }

    pub fn plan_available(&self, agent: SessionAgent) -> bool {
        let Some(client) = agent.coding().filter(|_| self.subscriptions) else {
            return false;
        };
        let entry = nexus_client::agent_setup::agent_entry(self.answer.as_ref(), client);
        nexus_client::agent_setup::route_of(&entry, Route::Subscription).is_none_or(|route| route["available"] != false)
    }

    /// The plan's name for `agent` ("Claude", "ChatGPT"), as Gateway names it.
    /// What pays on `via`, in the payer chip's words (`payerOf` in `harness-menu.tsx`): the plan ("Claude plan"),
    /// the organization's key, Nexus credits, or none yet.
    pub fn payer(&self, agent: SessionAgent, via: Via, org: &str) -> (Payer, String) {
        if self.answer.is_none() {
            return (Payer::Checking, "Checking…".into());
        }
        let Some(client) = agent.coding() else {
            return (Payer::Credits, "Credits".into());
        };
        let entry = nexus_client::agent_setup::agent_entry(self.answer.as_ref(), client);
        if via == Via::Subscription {
            if let Some(provider) = agent.configured_provider() {
                return (Payer::Plan, provider.into());
            }
            let plan = self.plan(agent).unwrap_or_else(|| "Your".into());
            let plan = plan
                .strip_prefix("Claude ")
                .filter(|rest| !rest.is_empty())
                .unwrap_or(&plan)
                .to_string();
            return (Payer::Plan, format!("{plan} plan"));
        }
        let gateway = nexus_client::agent_setup::route_of(&entry, Route::Gateway);
        match gateway.and_then(|view| view["access"].as_str()) {
            Some("byok") => (
                Payer::Key,
                format!("{} key", if org.is_empty() { "Your organization's" } else { org }),
            ),
            Some("nexus") => (Payer::Credits, "Credits".into()),
            _ if gateway.is_none_or(|view| view["available"] != false) => (Payer::Credits, "Credits".into()),
            _ => (Payer::None, "No key yet".into()),
        }
    }

    pub fn plan(&self, agent: SessionAgent) -> Option<String> {
        let client = agent.coding()?;
        let entry = nexus_client::agent_setup::agent_entry(self.answer.as_ref(), client);
        nexus_client::agent_setup::route_of(&entry, Route::Subscription)?["plan"]
            .as_str()
            .map(String::from)
    }
}

/// The scheduled sessions' commands (`nexus_workbench::api`'s `schedule_*`), in the
/// organization the runner is scoped to. Only the runner hands one out, so only the store's owner changes schedules.
#[derive(Clone)]
pub struct Schedules {
    api: Engine,
    runtime: tokio::runtime::Handle,
}

/// A command's answer, when it comes.
pub type Answer<T> = futures::channel::oneshot::Receiver<Result<T, String>>;

impl Schedules {
    pub fn list(&self) -> Answer<Vec<ScheduleView>> {
        self.ask(|api| async move { api.schedules().await })
    }

    /// Add a schedule (`id` None) or change one.
    pub fn save(&self, id: Option<String>, input: ScheduleInput) -> Answer<ScheduleView> {
        self.ask(move |api| async move { api.schedule_save(id, input).await })
    }

    /// Save schedule `id` as `change` makes it from what the store holds now, not from a list read earlier: an
    /// agent may have edited it since.
    pub fn update(
        &self,
        id: String,
        change: fn(&nexus_workbench::schedules::Schedule) -> ScheduleInput,
    ) -> Answer<ScheduleView> {
        self.ask(move |api| async move {
            let now = api
                .schedules()
                .await?
                .into_iter()
                .find(|view| view.schedule.id == id)
                .ok_or_else(|| "That schedule is gone.".to_string())?;
            api.schedule_save(Some(id), change(&now.schedule)).await
        })
    }

    pub fn delete(&self, id: String) -> Answer<()> {
        self.ask(move |api| async move { api.schedule_delete(id).await })
    }

    /// Run now: a new session at once, counted against the day's cap. Answers with the session's id.
    pub fn run_now(&self, id: String) -> Answer<String> {
        self.ask(move |api| async move { api.schedule_run(&id).await.map(|thread| thread.id) })
    }

    /// Turn the webhook on with a new token (shown only in this answer), or off.
    pub fn webhook(&self, id: String, on: bool) -> Answer<Option<WebhookSecret>> {
        self.ask(move |api| async move { api.schedule_webhook(&id, on).await })
    }

    fn ask<T, F>(&self, command: impl FnOnce(Engine) -> F + Send + 'static) -> Answer<T>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, String>> + Send + 'static,
    {
        ask(&self.api, &self.runtime, command)
    }
}

/// Run `command` on the runner's runtime; its answer comes on the receiver.
fn ask<T, F>(
    api: &Engine,
    runtime: &tokio::runtime::Handle,
    command: impl FnOnce(Engine) -> F + Send + 'static,
) -> Answer<T>
where
    T: Send + 'static,
    F: Future<Output = Result<T, String>> + Send + 'static,
{
    let (answer, answered) = futures::channel::oneshot::channel();
    let api = api.clone();
    runtime.spawn(async move {
        let _ = answer.send(command(api).await);
    });
    answered
}

mod projects;
mod service_handoff;
mod team_sessions;
pub use projects::{Projects, project_defaults};
pub(crate) use service_handoff::State as ServiceHandoff;

/// Outside access over `api`: the Nexus app's socket and grants, listening when its switch is on.
fn outside_access(
    host: &Arc<GpuiHost>,
    api: Engine,
    runtime: &tokio::runtime::Handle,
    data: &Path,
    loops: &Loops,
    changed: Arc<dyn Fn() + Send + Sync>,
) -> Arc<OutsideAccess> {
    let outside_host = Arc::new(crate::outside::GpuiOutside {
        api,
        runtime: runtime.clone(),
        org: {
            let host = Arc::downgrade(host);
            Arc::new(move || {
                host.upgrade()
                    .and_then(|host| host.scope().split_once('|').map(|(_, org)| org.to_string()))
                    .unwrap_or_default()
            })
        },
        client: {
            let host = Arc::downgrade(host);
            Arc::new(move || host.upgrade().and_then(|host| host.client()))
        },
        changed,
    });
    // `AllowLocalCache` off: nothing on disk, so the grants last until the runner goes.
    let grants = loops.local_cache.then(|| data.join("outside-access.json"));
    Arc::new(OutsideAccess::new(
        grants,
        loops.outside_socket.clone(),
        outside_host,
        (loops.settings)().outside_access == Some(true),
    ))
}

/// The store's lock in `data`, or `None` when another app holds it. `StoreOwner::take` says so only in its message.
fn take_store(data: &Path) -> Result<Option<StoreOwner>, String> {
    match StoreOwner::take(data) {
        Ok(owner) => Ok(Some(owner)),
        Err(error) if error.starts_with("Another Nexus app") => Ok(None),
        Err(error) => Err(error),
    }
}

/// This build's hold on the Code sessions: the store's lock, the workbench and the commands over it.
#[cfg(test)]
type TerminalFixture =
    Arc<dyn Fn(String, String, Value, crate::side::shared_terminals::Guard) -> Answer<Value> + Send + Sync>;

pub struct CodeRunner {
    #[cfg(test)]
    terminal_fixture: Mutex<Option<TerminalFixture>>,
    /// Where the commands go: this build's workbench, or the background service.
    api: Engine,
    host: Arc<GpuiHost>,
    runtime: tokio::runtime::Handle,
    /// The once-a-minute upkeep ([`upkeep`]), stopped when the runner, and with it the lock, goes. None while the
    /// service runs the sessions: it runs its own.
    upkeep: Option<tokio::task::AbortHandle>,
    /// The hourly worktree sweep ([`sweep_worktrees`]), stopped with the upkeep.
    sweeps: tokio::task::AbortHandle,
    /// The sweeps' state, shared with Settings' Clean up now.
    swept: Arc<Sweeps>,
    /// The open `request_secret` cards (`secret_request.rs`).
    secrets: GpuiSecrets,
    /// The built-in server's socket (`app_socket.rs`); None when it could not bind.
    app_socket: Option<Arc<AppSocket>>,
    /// Outside access (`outside.rs`).
    outside: Arc<OutsideAccess>,
    /// Session sync's loop (`Workbench::run_session_sync_until_shutdown`), stopped with the upkeep. None while the
    /// service runs the sessions: it syncs them itself.
    syncs: Option<tokio::task::AbortHandle>,
    /// The settings now, for a new session's branch prefix over the service.
    settings: Arc<dyn Fn() -> Settings + Send + Sync>,
    /// Whether the service should keep running when this build quits (`Loops::keep_running`).
    keep_running: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The background service, when it runs the sessions: stopped at quit if the switch was turned off.
    #[cfg(unix)]
    service: Option<nexus_remote::local::LocalService>,
    #[cfg(unix)]
    service_handoff: Arc<Mutex<service_handoff::State>>,
    #[cfg(unix)]
    service_account_generation: Arc<std::sync::atomic::AtomicU64>,
    service_status: Option<Arc<Mutex<nexus_remote::machine::Status>>>,
    /// The store's lock, while this build holds it. None while the service holds it.
    _owner: Option<StoreOwner>,
    /// The readers' way to the store when it is in memory (`local_store.rs`), dropped with the runner.
    _kept: Option<crate::local_store::Kept>,
    /// Whether this runner started the pull request watch (`Loops::pull_watch`): only then does its drop stop it.
    pull_watch: bool,
}

impl Drop for CodeRunner {
    fn drop(&mut self) {
        if let Some(upkeep) = &self.upkeep {
            upkeep.abort();
        }
        if self.pull_watch {
            crate::pr_watch::stop();
        }
        self.sweeps.abort();
        // Both sockets close with the store's lock: only its owner serves them.
        if let Some(socket) = &self.app_socket {
            socket.close();
        }
        self.outside.close();
        if let Some(syncs) = &self.syncs {
            syncs.abort();
        }
        // The service and its agents go on: only this build's connection closes.
        if let Some(remote) = self.api.remote() {
            remote.close();
        }
    }
}

/// What the runner runs once the store is its own: lost sessions ended, the schedules'
/// webhooks listening, then each `tick` the idle agents reaped, the plan resets continued and the due schedules
/// started.
async fn upkeep(workbench: Workbench, tick: Duration) {
    // Sessions the last owner left running when it died are ended in Gateway first.
    workbench.end_lost_sessions().await;
    // A schedule's webhook listens from the start; its runs wait for the first tick.
    workbench.serve_triggers().await;
    loop {
        tokio::time::sleep(tick).await;
        workbench.reap_idle();
        workbench.continue_at_resets(nexus_workbench::now_ms()).await;
        // Read the clock again: the resets can wait on Gateway for a while.
        workbench.run_schedules(nexus_workbench::now_ms()).await;
    }
}

/// The worktree sweep on this computer: after `sweep_first`, then every
/// `sweep_every`, with the rules saved at that moment. With every rule off it does nothing. The workbench removes only
/// what loses no work, and never a running agent's worktree (`nexus_workbench::cleanup`). This build has no pull request
/// watch, so the merged rule asks the GitHub CLI for each session, as a server's host does.
async fn sweep_worktrees(api: Engine, host: Arc<GpuiHost>, loops: Loops, sweeps: Arc<Sweeps>) {
    tokio::time::sleep(loops.sweep_first).await;
    loop {
        let read = loops.settings.clone();
        let rules = tokio::task::spawn_blocking(move || read().worktree_cleanup.unwrap_or_default())
            .await
            .unwrap_or_default();
        if rules.any() {
            sweep_once(&api, &host, &sweeps, &rules).await;
        }
        tokio::time::sleep(loops.sweep_every).await;
    }
}

/// The sweeps' shared state: one sweep at a time, and what the last one did (Settings' "Last cleanup").
#[derive(Default)]
pub struct Sweeps {
    running: tokio::sync::Mutex<()>,
    last: Mutex<Option<nexus_remote::cleanup::LastCleanup>>,
}

impl Sweeps {
    pub fn last(&self) -> Option<nexus_remote::cleanup::LastCleanup> {
        self.last
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// One sweep by `rules`, after any sweep still running, kept as the last one. The windows read the sessions again when
/// it removed a worktree.
async fn sweep_once(
    api: &Engine,
    host: &GpuiHost,
    sweeps: &Sweeps,
    rules: &nexus_workbench::cleanup::Rules,
) -> nexus_remote::cleanup::LastCleanup {
    let _one = sweeps.running.lock().await;
    let last = sweep_everywhere(api, rules).await;
    for problem in &last.problems {
        eprintln!("nexus-desktop-gpui: worktree cleanup: {problem}");
    }
    if !last.removed.is_empty() {
        // The app's reader, which hands it to every window (`windows.rs`).
        let _ = host.events().unbounded_send(CodeEvent::Changed);
    }
    *sweeps.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(last.clone());
    last
}

/// One sweep by `rules`: this computer's sessions (or the background
/// service's), then each connected server's and awake cloud machine's, whose host sweeps the sessions it owns
/// (`nexus_remote::cleanup`). A machine it could not sweep is named in the report's problems.
async fn sweep_everywhere(api: &Engine, rules: &nexus_workbench::cleanup::Rules) -> nexus_remote::cleanup::LastCleanup {
    let mut last = nexus_remote::cleanup::LastCleanup {
        at: nexus_workbench::now_ms(),
        ..Default::default()
    };
    last.add_answer(
        None,
        api.worktree_cleanup(rules)
            .await
            .map_err(|error| format!("This computer: {error}")),
    );
    for (name, remote) in crate::machines::get().connected_now() {
        last.add_answer(Some(&name), nexus_remote::cleanup::sweep(&name, &remote, rules).await);
    }
    last
}

fn reload_linked_limits(runtime: &tokio::runtime::Handle, remote: nexus_remote::machine::Remote) {
    runtime.spawn(async move {
        if let Err(error) = remote
            .call(nexus_workbench::orchestra::RELOAD_LIMITS_CALL, json!({}))
            .await
        {
            eprintln!("nexus-desktop-gpui: reloading linked-session limits: {error}");
        }
    });
}

/// A process-local continuation guard for `host`'s current scope, with no UI handles.
fn source_guard(host: &Arc<GpuiHost>) -> crate::side::browser::agent::Guard {
    let data = host.data.clone();
    let captured = host.scope.read().ok().map(|scope| {
        (
            scope.clone(),
            host.source_generation.load(std::sync::atomic::Ordering::SeqCst),
        )
    });
    let host = Arc::downgrade(host);
    Arc::new(move || {
        let (Some(host), Some((scope, generation))) = (host.upgrade(), captured.as_ref()) else {
            return false;
        };
        let Ok(current) = host.scope.read() else { return false };
        host.data == data
            && *current == *scope
            && host.source_generation.load(std::sync::atomic::Ordering::SeqCst) == *generation
    })
}

/// The background service, while it runs the sessions, takes `host`'s sign-in now, with this computer's device key
/// as the keychain holds it. Signed out, nothing is sent: the service keeps the last one until it stops.
fn hand_sign_in(api: &Engine, host: &Arc<GpuiHost>, runtime: &tokio::runtime::Handle) {
    let (Some(remote), Some(sign_in)) = (api.remote().cloned(), host.sign_in()) else {
        return;
    };
    let authority = source_guard(host);
    let host = host.clone();
    let marker = sign_in["token"].as_str().map(nexus_desktop_core::secrets::marker);
    runtime.spawn(async move {
        let current = || authority() && host.token().as_deref().map(nexus_desktop_core::secrets::marker) == marker;
        if remote.call_if_current("host_sign_in", sign_in, &current).await.is_err() && current() {
            remote.reconnect();
        }
    });
}

impl CodeRunner {
    #[cfg(test)]
    pub(crate) fn local_providers_opt_in(&self) -> &LocalProvidersOptIn {
        &self.host.local_providers_opt_in
    }

    /// A process-local continuation guard for a retained local browser, with no UI handles.
    pub(crate) fn source_guard(&self) -> crate::side::browser::agent::Guard {
        source_guard(&self.host)
    }
    /// Stable locator for presentation; no runner address or credential.
    pub(crate) fn store_source(&self) -> (PathBuf, String) {
        (self.host.data.clone(), self.host.scope())
    }

    /// Take the store for `scope` (`<gateway>|<org>`) and start the workbench. Refused when another app owns the
    /// store, or it could not be opened; the UI stays read-only then.
    pub fn start(
        gateway: &str,
        scope: &str,
        client: crate::backend::SessionClient,
        local_providers_opt_in: LocalProvidersOptIn,
    ) -> Result<(CodeRunner, UnboundedReceiver<CodeEvent>), Refused> {
        Self::start_owned(gateway, scope, client, local_providers_opt_in)
            .map_err(Refused::Failed)?
            .ok_or(Refused::Busy)
    }

    /// [`CodeRunner::start`], with `None` when another app holds the lock.
    fn start_owned(
        gateway: &str,
        scope: &str,
        client: crate::backend::SessionClient,
        local_providers_opt_in: LocalProvidersOptIn,
    ) -> Result<Option<(CodeRunner, UnboundedReceiver<CodeEvent>)>, String> {
        let data = crate::paths::data_dir().ok_or("This computer has no app data folder.")?;
        let loops = Loops::app(client, local_providers_opt_in);
        #[cfg(unix)]
        {
            let (worktrees, reviews) = worktree_roots(&home(), &(loops.settings)());
            let service = data.clone();
            Self::start_with(data, gateway, scope, loops, move |host| {
                let host = host.clone();
                crate::service::local_service(service, worktrees, reviews, Arc::new(move || host.service_session()))
            })
        }
        #[cfg(not(unix))]
        Self::start_in(data, gateway, scope, loops)
    }

    /// Over the background service `local` makes when the switch is on now (`Loops::keep_running`), else on the
    /// store's lock in `data`.
    #[cfg(unix)]
    fn start_with(
        data: PathBuf,
        gateway: &str,
        scope: &str,
        loops: Loops,
        local: impl FnOnce(&Arc<GpuiHost>) -> Result<nexus_remote::local::LocalService, String>,
    ) -> Result<Option<(CodeRunner, UnboundedReceiver<CodeEvent>)>, String> {
        if (loops.keep_running)() {
            Self::start_service_in(data, gateway, scope, loops, local)
        } else {
            Self::start_in(data, gateway, scope, loops)
        }
    }

    /// [`CodeRunner::start_owned`] on the store in `data`, with its loops on `loops`.
    fn start_in(
        data: PathBuf,
        gateway: &str,
        scope: &str,
        loops: Loops,
    ) -> Result<Option<(CodeRunner, UnboundedReceiver<CodeEvent>)>, String> {
        // The Nexus app (or its background service) holds the lock while it runs: it keeps the sessions and runs the
        // schedules, and this build reads them.
        let Some(owner) = take_store(&data)? else {
            return Ok(None);
        };
        std::fs::create_dir_all(&data).map_err(|error| format!("Could not open {}: {error}", data.display()))?;
        // On disk, or in memory when the organization keeps nothing here.
        let store = nexus_workbench::app_store::open(&data, loops.local_cache)?;
        let kept = crate::local_store::keep(&data, &store)?;
        let runtime = runtime();
        let (events, receiver) = unbounded();
        let host = Arc::new(GpuiHost {
            local_providers_opt_in: loops.local_providers_opt_in.clone(),
            sign_in: Arc::new(RwLock::new(SignIn {
                gateway: gateway.to_string(),
                client: loops.client.clone(),
            })),
            scope: Arc::new(RwLock::new(SessionScope::from(scope))),
            source_generation: std::sync::atomic::AtomicU64::new(0),
            data: data.clone(),
            prices: Arc::default(),
            agent_models: Arc::default(),
            events: Mutex::new(events),
            titles: Mutex::default(),
            agents: crate::notify::AgentNames::default(),
            app: OnceLock::new(),
            sync: Arc::default(),
        });
        let home = home();
        // The Nexus app's choices for worktrees and plan accounts; Settings changes the folders and prefix later
        // (`apply_settings`).
        let settings = (loops.settings)();
        let (worktrees, reviews) = worktree_roots(&home, &settings);
        let config = Config {
            scope: scope.to_string(),
            terminal: true,
            memory: nexus_agent_memory::Store::local().ok(),
            orchestration: settings.linked_limits(),
            subsessions: settings.subsessions_on(),
            subsessions_first_start: settings.subsessions().first_start,
            cross_agent_off: settings.subsessions().cross_agent_off,
            worktrees: Some(worktrees),
            reviews: Some(reviews),
            accounts: Some(data.join("accounts")),
            attachments: Some(data.join("attachments")),
            instructions: Some(data.join("instructions")),
            transcripts: Some(data.join("transcripts")),
            trigger_port: Some(nexus_workbench::schedules::trigger::DEFAULT_PORT),
            shell: shell(),
            ..Config::default()
        };
        // `new` recovers the sessions the last owner left: it may start tasks, so it runs inside the runtime.
        let workbench = {
            let _entered = runtime.enter();
            Workbench::new(store, host.clone(), config)?
        };
        // This app owns the store: its hooks run here, and a failed one shows a notification. Tests never read the
        // person's own hooks.
        #[cfg(not(test))]
        if let Some(path) = crate::desktop_settings::file() {
            workbench.set_hooks(Arc::new(nexus_desktop_core::hooks::FromSettings {
                path,
                notifier: Some(Arc::new(crate::notify::GpuiNotifier)),
            }));
        }
        if let Some(prefix) = &settings.branch_prefix
            && let Err(error) = workbench.set_branch_prefix(prefix)
        {
            eprintln!("nexus-desktop-gpui: branch prefix {prefix}: {error}");
        }
        apply_accounts(&workbench, &settings);
        let machine = Arc::new(GpuiMachine {
            host: host.clone(),
            home,
        });
        let api = Arc::new(Api::new(workbench.clone(), machine));
        // The pull request watch runs with the store, as the sessions do.
        if loops.pull_watch {
            crate::pr_watch::start(workbench.clone(), &runtime);
        }
        // The built-in server's socket and outside access, served only by the store's owner. Each tells the windows
        // to draw again when a card or a prompt opens or closes.
        let changed: Arc<dyn Fn() + Send + Sync> = {
            let host = Arc::downgrade(&host);
            Arc::new(move || {
                if let Some(host) = host.upgrade() {
                    let _ = host.events().unbounded_send(CodeEvent::Changed);
                }
            })
        };
        let secrets = GpuiSecrets::new(
            workbench.clone(),
            runtime.clone(),
            loops.secret_files.clone(),
            changed.clone(),
        );
        let scope: Arc<dyn Fn() -> String + Send + Sync> = {
            let host = Arc::downgrade(&host);
            Arc::new(move || host.upgrade().map(|host| host.scope()).unwrap_or_default())
        };
        serve_server_tools(&secrets, &runtime, &loops, host.data.clone(), scope);
        let current: crate::app_socket::Current = {
            let host = Arc::downgrade(&host);
            Arc::new(move || match host.upgrade() {
                Some(host) => (host.scope(), host.session_choice_identity().unwrap_or_default()),
                None => (String::new(), String::new()),
            })
        };
        let app_socket = match AppSocket::bind(
            host.data.clone(),
            runtime.clone(),
            secrets.clone(),
            current,
            loops.browser.clone(),
        ) {
            Ok(socket) => {
                let socket = Arc::new(socket);
                let _ = host.app.set(socket.clone());
                Some(socket)
            }
            Err(error) => {
                eprintln!("nexus-desktop-gpui: the built-in server's socket: {error}");
                None
            }
        };
        let outside = outside_access(&host, Engine::here(api.clone()), &runtime, &data, &loops, changed);
        let upkeep = runtime.spawn(upkeep(workbench, loops.tick)).abort_handle();
        let swept = Arc::new(Sweeps::default());
        let sweeps = runtime
            .spawn(sweep_worktrees(
                Engine::here(api.clone()),
                host.clone(),
                loops.clone(),
                swept.clone(),
            ))
            .abort_handle();
        // It asks the host for a backend on each pass: none while the switch is off.
        let computer = api.clone();
        let sync = api.workbench().clone();
        let syncs = runtime
            .spawn(async move {
                tokio::join!(
                    sync.run_session_sync_until_shutdown(),
                    computer.run_computer_control_until_shutdown()
                );
            })
            .abort_handle();
        let runner = CodeRunner {
            #[cfg(test)]
            terminal_fixture: Mutex::new(None),
            api: Engine::here(api),
            host,
            runtime,
            upkeep: Some(upkeep),
            sweeps,
            swept,
            secrets,
            app_socket,
            outside,
            syncs: Some(syncs),
            settings: loops.settings.clone(),
            keep_running: loops.keep_running.clone(),
            #[cfg(unix)]
            service: None,
            #[cfg(unix)]
            service_handoff: Default::default(),
            #[cfg(unix)]
            service_account_generation: Default::default(),
            service_status: None,
            _owner: Some(owner),
            _kept: kept,
            pull_watch: loops.pull_watch,
        };
        runner.load_agents();
        // Sessions the workbench recovered may already run work in the background.
        runner.seed_live_fields();
        Ok(Some((runner, receiver)))
    }

    /// [`CodeRunner::start_owned`] over the background service, which holds the store in `data`: the service `local`
    /// makes for this build's host. `None` when the Nexus app holds the store in its own process: no second owner is
    /// started. A running service is reached; else one is started, from `local`'s binary.
    #[cfg(unix)]
    fn start_service_in(
        data: PathBuf,
        gateway: &str,
        scope: &str,
        loops: Loops,
        local: impl FnOnce(&Arc<GpuiHost>) -> Result<nexus_remote::local::LocalService, String>,
    ) -> Result<Option<(CodeRunner, UnboundedReceiver<CodeEvent>)>, String> {
        let runtime = runtime();
        let (events, receiver) = unbounded();
        let host = Arc::new(GpuiHost {
            local_providers_opt_in: loops.local_providers_opt_in.clone(),
            sign_in: Arc::new(RwLock::new(SignIn {
                gateway: gateway.to_string(),
                client: loops.client.clone(),
            })),
            scope: Arc::new(RwLock::new(SessionScope::from(scope))),
            source_generation: std::sync::atomic::AtomicU64::new(0),
            data: data.clone(),
            prices: Arc::default(),
            agent_models: Arc::default(),
            events: Mutex::new(events),
            titles: Mutex::default(),
            agents: crate::notify::AgentNames::default(),
            app: OnceLock::new(),
            sync: Arc::default(),
        });
        let local = local(&host)?;
        if crate::service::held_in_process(&local) {
            return Ok(None);
        }
        // This build's own workbench keeps no sessions: it reads the
        // agents installed on this computer and their models.
        let workbench = {
            let _entered = runtime.enter();
            let config = Config {
                scope: scope.to_string(),
                shell: shell(),
                ..Config::default()
            };
            Workbench::new(Store::in_memory()?, host.clone(), config)?
        };
        let machine = Arc::new(GpuiMachine {
            host: host.clone(),
            home: home(),
        });
        let api = Arc::new(Api::new(workbench.clone(), machine));
        let changed: Arc<dyn Fn() + Send + Sync> = {
            let host = Arc::downgrade(&host);
            Arc::new(move || {
                if let Some(host) = host.upgrade() {
                    let _ = host.events().unbounded_send(CodeEvent::Changed);
                }
            })
        };
        // The scope the sessions are read under, from the store the service holds.
        let scope_now: Arc<dyn Fn() -> String + Send + Sync> = {
            let host = Arc::downgrade(&host);
            Arc::new(move || host.upgrade().map(|host| host.scope()).unwrap_or_default())
        };
        // The service passes its agents' `request_secret` calls here (`ServiceSink`): their cards open in this build.
        let sessions: crate::secret_request::Threads = {
            let (data, scope) = (data.clone(), scope_now.clone());
            Arc::new(move || nexus_workbench::reader::summaries_in(&data, &scope()))
        };
        let secrets = GpuiSecrets::reading(
            workbench,
            runtime.clone(),
            loops.secret_files.clone(),
            changed.clone(),
            Some(sessions),
        );
        serve_server_tools(&secrets, &runtime, &loops, data.clone(), scope_now.clone());
        let service_status = Arc::new(Mutex::new(nexus_remote::machine::Status::Connecting { attempt: 1 }));
        let sink = Arc::new(ServiceSink {
            host: host.clone(),
            status: service_status.clone(),
            secrets: secrets.clone(),
            runtime: runtime.clone(),
            remote: OnceLock::new(),
            browser: loops.browser.clone(),
            ended: Arc::default(),
        });
        let remote = {
            let _entered = runtime.enter();
            let backoff = nexus_remote::machine::Backoff {
                first: Duration::from_millis(200),
                max: Duration::from_secs(5),
                healthy: Duration::from_secs(60),
            };
            let remote = nexus_remote::machine::Remote::start(Arc::new(local.clone()), sink.clone(), backoff);
            let _ = sink.remote.set(remote.clone());
            runtime.spawn(nexus_remote::local::update_when_idle(remote.clone()));
            remote
        };
        // The pull request watch reads the sessions from the store and changes them through the service.
        if loops.pull_watch {
            crate::pr_watch::start_over_service(
                crate::pr_watch::Service {
                    remote: remote.clone(),
                    data: data.clone(),
                    scope: scope_now,
                },
                &runtime,
            );
        }
        let engine = Engine::service(api, remote);
        // Outside access as when this build holds the store: its tools reach the sessions through the service.
        let outside = outside_access(&host, engine.clone(), &runtime, &data, &loops, changed);
        let swept = Arc::new(Sweeps::default());
        let sweeps = runtime
            .spawn(sweep_worktrees(
                engine.clone(),
                host.clone(),
                loops.clone(),
                swept.clone(),
            ))
            .abort_handle();
        let runner = CodeRunner {
            #[cfg(test)]
            terminal_fixture: Mutex::new(None),
            api: engine,
            host,
            runtime,
            upkeep: None,
            sweeps,
            swept,
            secrets,
            app_socket: None,
            outside,
            syncs: None,
            settings: loops.settings.clone(),
            keep_running: loops.keep_running.clone(),
            service: Some(local),
            #[cfg(unix)]
            service_handoff: Default::default(),
            #[cfg(unix)]
            service_account_generation: Default::default(),
            service_status: Some(service_status),
            _owner: None,
            _kept: None,
            pull_watch: loops.pull_watch,
        };
        runner.load_agents();
        Ok(Some((runner, receiver)))
    }

    /// Only sessions on this computer may open a folder in an application here: this build's, or its background
    /// service's, which says where the folder is (`host_folder`).
    pub fn editor_folder(&self, id: &str, repo: Option<&str>) -> Answer<std::path::PathBuf> {
        if crate::machines::get().machine_of(id).is_some() {
            let (sender, receiver) = futures::channel::oneshot::channel();
            let _ = sender.send(Err("Open in editor is available for sessions on this computer.".into()));
            return receiver;
        }
        let (id, repo) = (id.to_string(), repo.map(String::from));
        self.ask(move |api| async move {
            if api.remote().is_none() {
                return api.workbench().git_folder(&id, repo.as_deref());
            }
            let folder = api.call("host_folder", json!({ "threadId": id, "repo": repo })).await?;
            folder
                .as_str()
                .map(std::path::PathBuf::from)
                .ok_or_else(|| "The background service did not say where the session's folder is.".into())
        })
    }

    /// The connection to the background service, while it runs the sessions.
    pub fn service_status(&self) -> Option<nexus_remote::machine::Status> {
        self.service_status
            .as_ref()
            .map(|status| status.lock().unwrap_or_else(|error| error.into_inner()).clone())
    }

    /// The person signed out: the service stops with its agents, which ran on the sign-in that is gone. The next
    /// sign-in starts it again.
    pub fn signed_out(&self) {
        if self.host.local_providers_opt_in.enabled() {
            if self.host.launch_client().1 {
                self.retire_service_after_sign_out();
            }
            return;
        }
        #[cfg(unix)]
        if let (Some(remote), Some(local)) = (self.api.remote().cloned(), self.service.clone()) {
            remote.drop_connection();
            self.runtime.spawn(async move {
                if let Err(error) = local.stop().await {
                    eprintln!("nexus-desktop-gpui: background service: {error}");
                }
            });
        }
    }

    pub(crate) fn retire_service_after_sign_out(&self) {
        if !self.host.session_scope().is_local() {
            self.apply_scope("");
            self.stop_signed_in_service();
        }
    }

    /// Quitting over the service: the connection closes, and the service and its agents keep running. Unless the
    /// switch was turned off since this build started: then the service stops now, with its agents.
    fn hand_over(&self, remote: &nexus_remote::machine::Remote) {
        remote.close();
        #[cfg(unix)]
        if !(self.keep_running)()
            && let Some(local) = self.service.clone()
            && let Err(error) = self.runtime.block_on(async move { local.stop().await })
        {
            eprintln!("nexus-desktop-gpui: background service: {error}");
        }
    }

    /// What Settings › Devices calls after it made or replaced this computer's device key. This build's own sync tries
    /// again, and the background service gets the sign-in again with the new key: it took the old one at sign-in.
    pub fn device_key_changed(&self) -> std::rc::Rc<dyn Fn()> {
        let (api, host, runtime) = (self.api.clone(), self.host.clone(), self.runtime.clone());
        std::rc::Rc::new(move || {
            host.sync.retry_unavailable();
            hand_sign_in(&api, &host, &runtime);
        })
    }

    /// The schedules' commands, while this build owns the store.
    pub fn schedules(&self) -> Schedules {
        Schedules {
            api: self.api.clone(),
            runtime: self.runtime.clone(),
        }
    }

    /// The projects' commands for Settings, while this build owns the store.
    pub fn projects(&self) -> Projects {
        Projects {
            api: self.api.clone(),
            runtime: self.runtime.clone(),
            host: self.host.clone(),
            sweeps: self.swept.clone(),
            settings: self.settings.clone(),
        }
    }

    /// Settings' Gateway switch: agents use the new Gateway's active sign-in from now on.
    pub fn set_gateway(&self, gateway: &str, client: crate::backend::SessionClient) {
        *self
            .host
            .sign_in
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = SignIn {
            gateway: gateway.to_string(),
            client,
        };
        // The account's scope is handed over by set_scope, after this Gateway restores its sign-in.
    }

    /// Settings' worktree and review folders to the background service while it runs the sessions: its new sessions
    /// use them, as this build's own do. It took the first ones on its command line.
    fn hand_worktree_roots(&self, worktrees: PathBuf, reviews: PathBuf) {
        let Some(remote) = self.api.remote().cloned() else {
            return;
        };
        self.runtime.spawn(async move {
            let roots = json!({ "worktrees": worktrees, "reviews": reviews });
            if let Err(error) = remote.call(nexus_workbench::WORKTREE_ROOTS_CALL, roots).await {
                eprintln!("nexus-desktop-gpui: could not hand the background service the worktree folders: {error}");
            }
        });
    }

    /// The service takes the sign-in now. Signed out,
    /// nothing is sent: the service keeps the last one until it stops.
    fn hand_sign_in(&self) {
        hand_sign_in(&self.api, &self.host, &self.runtime);
    }

    /// Settings changed the worktree folders or the branch prefix: new sessions use them. Sessions already made keep
    /// theirs.
    pub fn apply_settings(&self, settings: &nexus_desktop_core::settings::Settings) {
        let workbench = self.api.workbench();
        let (worktrees, reviews) = worktree_roots(&home(), settings);
        workbench.set_worktree_roots(Some(worktrees.clone()), Some(reviews.clone()));
        self.hand_worktree_roots(worktrees, reviews);
        {
            // On the runtime: a tree the new limits release has its waiting results offered there.
            let _entered = self.runtime.enter();
            workbench.set_subsessions(settings.subsessions());
        }
        if let Some(remote) = self.api.remote().cloned()
            && matches!(remote.status(), nexus_remote::machine::Status::Connected { host, .. } if host.features.iter().any(|f| f == nexus_workbench::orchestra::LIMITS_FEATURE))
        {
            reload_linked_limits(&self.runtime, remote);
        }
        apply_accounts(workbench, settings);
        if let Err(error) = workbench.set_branch_prefix(&settings.branch_prefix_or_default()) {
            eprintln!("nexus-desktop-gpui: branch prefix: {error}");
        }
        let _ = self.hand_mcp_servers(settings.mcp_servers.clone());
        self.hand_server_mcp_servers(settings.mcp_servers.clone());
        self.hand_accounts(settings);
    }

    /// Settings' MCP servers, with their keychain values, to every server reached over SSH: its agents get them the
    /// next time they start, as this build's own do. Never to a cloud machine (`machines::server_tools`).
    fn hand_server_mcp_servers(&self, servers: Vec<nexus_desktop_core::settings::McpServer>) {
        self.runtime.spawn(async move {
            // The keychain is read off the runtime's threads.
            if let Ok(handed) = tokio::task::spawn_blocking(move || crate::mcp::for_service(&servers)).await {
                crate::machines::get().hand_mcp_servers(handed);
            }
        });
    }

    /// Settings' plan accounts and rotation to the background service while it runs the sessions: its new sessions
    /// start on them, as this build's own do. Without it, the service starts every session on the agent's own login.
    /// Nothing to do when this build runs the sessions itself.
    fn hand_accounts(&self, settings: &nexus_desktop_core::settings::Settings) {
        let Some(remote) = self.api.remote().cloned() else {
            return;
        };
        let choices = account_choices(settings);
        self.runtime.spawn(async move {
            if let Err(error) = remote
                .call(nexus_workbench::accounts::SERVICE_CALL, json!(choices))
                .await
            {
                eprintln!("nexus-desktop-gpui: could not hand the background service the plan accounts: {error}");
            }
        });
    }

    /// Settings' MCP servers, with their keychain values, to the background service while it runs the sessions: its
    /// agents get them the next time they start, as this build's own do. The
    /// task that hands them; None when this build runs the sessions itself.
    fn hand_mcp_servers(
        &self,
        servers: Vec<nexus_desktop_core::settings::McpServer>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let remote = self.api.remote().cloned()?;
        Some(self.runtime.spawn(async move {
            // The keychain is read off the runtime's threads.
            let handed = tokio::task::spawn_blocking(move || crate::mcp::for_service(&servers)).await;
            let Ok(handed) = handed else {
                return;
            };
            if let Err(error) = remote.call(nexus_desktop_mcp::mcp::SERVICE_CALL, handed).await {
                eprintln!("nexus-desktop-gpui: could not hand the background service the MCP servers: {error}");
            }
        }))
    }

    /// The sessions shown and started are now this organization's.
    pub fn set_scope(&self, scope: &str) {
        // The local view can open before the shared auth event arrives, including after a Gateway switch.
        if SessionScope::decode(scope).is_local() && self.host.local_providers_opt_in.enabled() {
            self.signed_out();
        }
        self.apply_scope(scope);
        #[cfg(unix)]
        self.resume_service_after_sign_out();
        self.hand_sign_in();
        // `leave_org` dropped the live fields, also on a sign-in again to the same organization: read them again.
        self.seed_live_fields();
    }

    fn apply_scope(&self, scope: &str) {
        if let Ok(mut current) = self.host.scope.write() {
            if current.as_str() != scope {
                self.host
                    .source_generation
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                *current = SessionScope::from(scope);
            }
        }
        self.api.workbench().set_scope(scope);
    }

    /// Reads every session from its owner, this build's workbench or the background service, and keeps their live
    /// fields (`live_fields.rs`).
    fn seed_live_fields(&self) {
        match self.api.remote().cloned() {
            Some(remote) => seed_live_fields(service_threads(remote), &self.host, &self.runtime),
            None => {
                let workbench = self.api.workbench().clone();
                let read = async move { workbench.threads().map_err(|error| error.to_string()) };
                seed_live_fields(read, &self.host, &self.runtime);
            }
        }
    }

    /// Read the agents and their models again (cached for five minutes by the workbench). Each agent's models are sent
    /// as its discovery ends, in a whole list each time: a slow `agy models` does not hold back the others.
    pub fn load_agents(&self) {
        let (api, events, kept) = (self.api.clone(), self.host.events(), self.host.agent_models.clone());
        self.runtime.spawn(async move {
            let landed = events.clone();
            let read = api
                .agent_models_each(move |agents| {
                    if let Ok(mut held) = kept.write() {
                        held.clone_from(&agents);
                    }
                    let _ = landed.unbounded_send(CodeEvent::Agents(agents));
                })
                .await;
            if let Err(error) = read {
                let _ = events.unbounded_send(CodeEvent::Failed(error));
            }
        });
    }

    /// What Auto would run for a new session of `agent` paid by `via`, on this computer: Gateway's dry run, with the
    /// plan's last reading of the agent's own login. It opens no session. `None`: Auto cannot route here.
    pub(crate) fn auto_preview(
        &self,
        client: Client,
        agent: SessionAgent,
        via: Via,
    ) -> Answer<Option<nexus_client::auto_route::Answer>> {
        let (sender, receiver) = futures::channel::oneshot::channel();
        let (prices, installed) = (self.host.prices.clone(), self.host.agent_models.clone());
        self.runtime.spawn(async move {
            let answer = async {
                let coding = agent.coding()?;
                let program = find(agent)?;
                let path_env = agent_env().remove("PATH").unwrap_or_default();
                let plan_limits = crate::plan_book::current(agent, &None)
                    .filter(|plan| plan.error.is_none())
                    .map(|plan| plan.limits);
                let models = installed
                    .read()
                    .ok()
                    .and_then(|agents| agents.iter().find(|info| info.agent == agent)?.models.clone())
                    .unwrap_or_default();
                let auto = nexus_workbench::agent_launch::AutoContext {
                    prices: Some(&prices),
                    installed: models,
                    plan_limits,
                    now_ms: crate::code::now_ms(),
                };
                nexus_workbench::agent_launch::auto_answer(&client, coding, via, None, &program, &path_env, auto, None)
                    .await
            }
            .await;
            let _ = sender.send(Ok(answer));
        });
        receiver
    }

    /// What Auto would run when it picks the agent too: every agent Auto routes that is installed here, each with its
    /// plan's last reading when `via` is the plan. A dry run. `None`: fewer than one such agent, or no answer.
    pub(crate) fn auto_preview_across(
        &self,
        client: Client,
        via: Via,
    ) -> Answer<Option<(SessionAgent, nexus_client::auto_route::Answer)>> {
        let (sender, receiver) = futures::channel::oneshot::channel();
        let (prices, installed) = (self.host.prices.clone(), self.host.agent_models.clone());
        self.runtime.spawn(async move {
            let answer = async {
                let path_env = agent_env().remove("PATH").unwrap_or_default();
                let known = installed.read().map(|agents| agents.clone()).unwrap_or_default();
                let agents: Vec<nexus_workbench::agent_launch::AcrossAgent<'_>> = SessionAgent::ALL
                    .into_iter()
                    .filter(|agent| crate::workspace::takes_auto(*agent))
                    .filter_map(|agent| {
                        let coding = agent.coding()?;
                        let program = find(agent)?;
                        let plan_limits = crate::plan_book::current(agent, &None)
                            .filter(|plan| plan.error.is_none())
                            .map(|plan| plan.limits);
                        let models = known
                            .iter()
                            .find(|info| info.agent == agent)
                            .and_then(|info| info.models.clone())
                            .unwrap_or_default();
                        Some(nexus_workbench::agent_launch::AcrossAgent {
                            coding,
                            program,
                            auto: nexus_workbench::agent_launch::AutoContext {
                                prices: Some(&prices),
                                installed: models,
                                plan_limits,
                                now_ms: crate::code::now_ms(),
                            },
                        })
                    })
                    .collect();
                nexus_workbench::agent_launch::auto_answer_across(&client, agents, via, &path_env, None).await
            }
            .await;
            let _ = sender.send(Ok(answer));
        });
        receiver
    }

    /// Read who may pay for new sessions: the organization's policy and Gateway's routes.
    pub fn load_access(&self) {
        let (client, events) = (self.host.client(), self.host.events());
        self.runtime.spawn(async move {
            let Some(client) = client else { return };
            let credential = client
                .options()
                .api_key
                .as_deref()
                .map(nexus_desktop_core::secrets::marker);
            let access = read_access(&client).await;
            let _ = events.unbounded_send(CodeEvent::Access { credential, access });
        });
    }

    /// The open `request_secret` card of the session `thread`, if any.
    pub fn secret_request(&self, thread: &str) -> Option<nexus_desktop_mcp::secret_request::SecretRequest> {
        self.secrets.of(thread)
    }

    /// The person's answer to a `request_secret` card: the value to provide it, None to decline. The file is
    /// written off the UI thread.
    pub fn answer_secret(
        &self,
        id: String,
        value: Option<String>,
    ) -> futures::channel::oneshot::Receiver<Result<(), String>> {
        let (answer, answered) = futures::channel::oneshot::channel();
        let secrets = self.secrets.clone();
        self.runtime.spawn_blocking(move || {
            let _ = answer.send(secrets.settle(&id, value.as_deref()));
        });
        answered
    }

    /// Outside access, while this build owns the store.
    pub fn outside(&self) -> Arc<OutsideAccess> {
        self.outside.clone()
    }

    /// The built-in server's socket, while it listens.
    #[cfg(test)]
    fn app_socket(&self) -> Option<&AppSocket> {
        self.app_socket.as_deref()
    }

    /// Send a message with its attachments, as the Nexus app's `wb_send`: it starts the agent if it is not running.
    /// While a turn runs, it joins that turn at the agent's next step, or with `queued` waits for the turn to end.
    /// Files over one piece go up in pieces first (`attachments::sendable_files`). Messages to one session leave in
    /// the order they were sent (`outbox`); one that fails comes back as [`CodeEvent::Unsent`].
    pub fn send(&self, thread_id: String, text: String, attachments: crate::attachments::Draft, queued: bool) {
        let mut place = crate::outbox::join(&thread_id);
        let (api, events) = (self.api.clone(), self.host.events());
        self.runtime.spawn(async move {
            place.wait().await;
            let sent = async {
                let files = upload(&api, &thread_id, &attachments).await?;
                let images = crate::attachments::session_images(&attachments);
                api.send_files_as(&thread_id, &text, images, files, queued).await
            };
            let event = match sent.await {
                Ok(()) => CodeEvent::Changed,
                Err(error) => CodeEvent::Unsent {
                    thread_id,
                    text,
                    attachments,
                    error,
                },
            };
            let _ = events.unbounded_send(event);
        });
    }

    /// Commit everything that changed in the session's folder as `message`, with the person's own git identity.
    pub fn commit(
        &self,
        thread_id: String,
        message: String,
    ) -> futures::channel::oneshot::Receiver<Result<GitState, String>> {
        self.ask(move |api| async move { api.commit(&thread_id, None, &message, false).await })
    }

    /// A git question or action for the top bar's git panel and pull request form (`workspace/git_panel.rs`).
    pub fn git<T: Send + 'static, F: Future<Output = Result<T, String>> + Send + 'static>(
        &self,
        command: impl FnOnce(Engine) -> F + Send + 'static,
    ) -> futures::channel::oneshot::Receiver<Result<T, String>> {
        self.ask(command)
    }

    /// Whether this build runs the sessions' commands itself. Over the background service, Commands work in this window
    /// ([`CodeRunner::terminal`]), but cannot move to another one.
    pub fn commands_in_process(&self) -> bool {
        self.api.remote().is_none()
    }

    /// Trusted app-window access to the session's command PTYs. Human authority goes only to this build's workbench, or
    /// to the app's own service through its dedicated call (`host_terminal`), never through generic host calls.
    pub fn terminal(&self, thread: String, op: String, args: Value) -> Answer<Value> {
        self.terminal_guarded(thread, op, args, self.source_guard())
    }

    #[cfg(test)]
    pub(crate) fn fixture_terminal(
        &self,
        handler: impl Fn(String, String, Value, crate::side::shared_terminals::Guard) -> Answer<Value>
        + Send
        + Sync
        + 'static,
    ) {
        *self.terminal_fixture.lock().unwrap() = Some(Arc::new(handler));
    }

    /// Local presentation authority is checked when queued work actually dispatches,
    /// and inside the backend after asynchronous commit-hook lookup.
    pub(crate) fn terminal_guarded(
        &self,
        thread: String,
        op: String,
        args: Value,
        valid: crate::side::shared_terminals::Guard,
    ) -> Answer<Value> {
        #[cfg(test)]
        let handler = self.terminal_fixture.lock().unwrap().clone();
        #[cfg(test)]
        if let Some(handler) = handler {
            if !valid() {
                let (sender, receiver) = futures::channel::oneshot::channel();
                let _ = sender.send(Err("The Commands owner is no longer current.".into()));
                return receiver;
            }
            return handler(thread, op, args, valid);
        }
        self.ask(move |api| async move {
            if !valid() {
                return Err("The Commands owner is no longer current.".into());
            }
            // Over the background service too: it runs the commands, and this window shows them.
            api.terminal_guarded(&thread, &op, args, valid.as_ref()).await
        })
    }

    /// Stop the current turn.
    pub fn interrupt(&self, thread_id: String) {
        self.run(move |api| async move { api.interrupt(&thread_id).await.map(|()| None) });
    }

    /// A retained browser's Stop must still own its source when its queued command actually runs.
    pub(crate) fn interrupt_guarded(&self, thread_id: String, valid: crate::side::browser::agent::Guard) {
        drop(self.interrupt_after(thread_id, valid, std::future::ready(())));
    }

    /// One per-call wait also lets a fixture hold the real dispatch boundary without changing the runtime.
    fn interrupt_after<F>(
        &self,
        thread_id: String,
        valid: crate::side::browser::agent::Guard,
        ready: F,
    ) -> tokio::task::JoinHandle<bool>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let (api, events) = (self.api.clone(), self.host.events());
        self.runtime.spawn(async move {
            ready.await;
            if !valid() {
                return false;
            }
            // Engine carries this same proof across service readiness and checks before its wire enqueue.
            let result = api.interrupt_guarded(&thread_id, valid.as_ref()).await;
            if valid() {
                let event = match result {
                    Ok(()) => CodeEvent::Changed,
                    Err(error) => CodeEvent::Failed(error),
                };
                let _ = events.unbounded_send(event);
            }
            true
        })
    }

    /// Answer the agent's question to use a tool. `message` goes to the agent with the answer (a reason to deny, what
    /// to change in a plan, a question's receipt). `suggestion` is the index of the rule "Always allow" saves, for
    /// [`Decision::AllowSession`]. `answers` are a question's answers (`questions.ts`'s `questionAnswers`).
    pub fn approve(
        &self,
        thread_id: String,
        request_id: String,
        decision: Decision,
        message: Option<String>,
        suggestion: Option<usize>,
        answers: Option<Map<String, Value>>,
    ) {
        self.run(move |api| async move {
            api.approve(thread_id, request_id, decision, message, suggestion, answers)
                .await
                .map(|()| None)
        });
    }

    /// Implement a plan (`plan-card.tsx`): approve it, then switch the session to Auto-accept edits, Claude Code's own
    /// first answer to a plan. The plan stays approved when the switch fails; the error says so.
    pub fn implement_plan(&self, thread_id: String, request_id: String) {
        self.run(move |api| async move {
            api.approve(thread_id.clone(), request_id, Decision::Allow, None, None, None)
                .await?;
            api.thread_update(UpdateArgs {
                thread_id,
                permission_mode: Some(PermissionMode::AcceptEdits.as_str().to_string()),
                ..UpdateArgs::default()
            })
            .await
            .map_err(|_| {
                "Plan approved, but the session could not switch to Accept edits. Pick it from the mode menu."
                    .to_string()
            })?;
            Ok(None)
        });
    }

    /// Rewind the conversation to before the person's message `seq` (`wb_rewind_message`); files stay as they are.
    pub fn rewind(&self, thread_id: String, seq: i64) -> futures::channel::oneshot::Receiver<Result<(), String>> {
        self.ask(move |api| async move { api.rewind_message(&thread_id, seq).await })
    }

    /// How many files [`CodeRunner::revert_to_turn`] would change (`wb_revert_preview`), for the person to confirm.
    pub fn revert_preview(
        &self,
        thread_id: String,
        turn: i64,
    ) -> futures::channel::oneshot::Receiver<Result<u32, String>> {
        self.ask(move |api| async move { api.revert_preview(&thread_id, turn).await })
    }

    /// Put the session's files back as they were when the turn ending at event `turn` ended (`wb_revert_to_turn`).
    pub fn revert_to_turn(
        &self,
        thread_id: String,
        turn: i64,
    ) -> futures::channel::oneshot::Receiver<Result<u32, String>> {
        self.ask(move |api| async move { api.revert_to_turn(&thread_id, turn).await })
    }

    /// Take back a message queued behind the running turn (`seq`: its event) before the agent reads it.
    pub fn queue_cancel(&self, thread_id: String, seq: i64) {
        self.run(move |api| async move { api.queue_cancel(thread_id, seq).await.map(|()| None) });
    }

    /// Rewrite a queued message before the agent reads it.
    pub fn queue_edit(&self, thread_id: String, seq: i64, text: String) {
        self.run(move |api| async move { api.queue_edit(thread_id, seq, text).await.map(|()| None) });
    }

    /// Move a queued message just before the queued message `before`, or last when `before` is None.
    pub fn queue_move(&self, thread_id: String, seq: i64, before: Option<i64>) {
        self.run(move |api| async move { api.queue_move(thread_id, seq, before).await.map(|()| None) });
    }

    /// Send a queued message into the running turn now ("Send now"): the agent reads it at its next step.
    pub fn queue_steer(&self, thread_id: String, seq: i64) {
        self.run(move |api| async move { api.queue_steer(thread_id, seq).await.map(|()| None) });
    }

    /// Change the session's model, effort or permission mode (`wb_thread_update`). None leaves one as it is;
    /// `Some(None)` sets the model or effort back to the default. A running agent takes the change at its next step.
    pub fn update(
        &self,
        thread_id: String,
        model: Option<Option<String>>,
        effort: Option<Option<Effort>>,
        mode: Option<PermissionMode>,
    ) {
        self.run(move |api| async move {
            api.thread_update(UpdateArgs {
                thread_id,
                model: model.map(Option::unwrap_or_default),
                effort: effort.map(|effort| effort.map(|effort| effort.as_str().to_string()).unwrap_or_default()),
                permission_mode: mode.map(|mode| mode.as_str().to_string()),
                ..UpdateArgs::default()
            })
            .await
            .map(|_| None)
        });
    }

    /// Change what the session's menu changes (`wb_thread_update`): its title, pin, archive, snooze or "Keep in
    /// list". Fill in only the fields that change.
    pub fn update_session(&self, args: UpdateArgs) {
        self.run(move |api| async move { api.thread_update(args).await.map(|_| None) });
    }

    /// Take the session's worktree off the disk, unless that would lose work (`wb_worktree_remove`).
    pub fn remove_worktree(&self, thread_id: String) {
        self.run(move |api| async move { api.worktree_remove(&thread_id).await.map(|_| None) });
    }

    /// A new session that goes on from turn `turn` (the latest when None), in a worktree of its own: its id.
    pub fn fork(
        &self,
        thread_id: String,
        turn: Option<i64>,
        subscription: Option<nexus_workbench::ForkSubscription>,
    ) -> futures::channel::oneshot::Receiver<Result<String, String>> {
        self.ask(move |api| async move {
            api.thread_fork_with_subscription(&thread_id, turn, subscription)
                .await
                .map(|thread| thread.id)
        })
    }

    pub fn session_events(
        &self,
        thread_id: String,
    ) -> futures::channel::oneshot::Receiver<Result<Vec<nexus_workbench::event::StoredEvent>, String>> {
        self.ask(move |api| async move { api.thread_events(thread_id, 0).await })
    }

    /// End the agent process; the next message starts it again.
    pub fn stop(&self, thread_id: String) {
        self.run(move |api| async move { api.stop(&thread_id).await.map(|()| None) });
    }

    /// Add a folder as a project.
    pub fn add_project(&self, path: PathBuf, origin: Option<gpui_kit::EntityId>) {
        let scope = self.host.scope();
        let generation = self.host.source_generation.load(std::sync::atomic::Ordering::SeqCst);
        let authority = self.source_guard();
        self.run(move |api| async move {
            let result = api
                .project_add_guarded(path.to_string_lossy().into_owned(), Some(&*authority))
                .await;
            let event = match result {
                Ok(project) => CodeEvent::ProjectAdded {
                    id: project.id,
                    scope,
                    generation,
                    origin,
                },
                Err(error) => {
                    eprintln!("nexus-desktop-gpui: Add a project: {error}");
                    CodeEvent::ProjectAddFailed { error, origin }
                }
            };
            Ok(Some(event))
        });
    }

    pub fn owns_project_result(&self, scope: &str, generation: u64) -> bool {
        self.host.scope() == scope
            && self.host.source_generation.load(std::sync::atomic::Ordering::SeqCst) == generation
    }

    /// Add the folder at `path` (`~` is the home folder) as a project, and answer with its id or why it was refused:
    /// Add a project shows the refusal under its field.
    pub fn add_project_at(&self, path: String) -> futures::channel::oneshot::Receiver<Result<String, String>> {
        let authority = self.source_guard();
        self.ask(move |api| async move {
            api.project_add_guarded(path, Some(&*authority))
                .await
                .map(|project| project.id)
        })
    }

    /// The folders in `path` on this computer, for Add a project's folder field (`wb_dirs`).
    pub fn dirs(
        &self,
        path: String,
    ) -> futures::channel::oneshot::Receiver<Result<nexus_workbench::dirs::Dirs, String>> {
        self.ask(move |api| async move { api.dirs(path).await })
    }

    /// Clone `source` (a URL, or GitHub's `owner/repo`) into `parent` and open it as a project. Where it stands is
    /// [`CodeRunner::clone_progress`] of `request_id` until [`CodeEvent::Cloned`] arrives.
    pub fn clone_project(&self, source: String, parent: String, request_id: String) {
        self.run(move |api| async move {
            let cloned = api.project_clone(source, parent, request_id).await;
            Ok(Some(CodeEvent::Cloned(cloned.map(|project| project.id))))
        });
    }

    /// Git's progress on the clone `request_id`: its stage, and how far as a percentage when git says.
    pub fn clone_progress(&self, request_id: &str) -> Option<(String, Option<u8>)> {
        self.api.clone_progress(request_id, &self.runtime)
    }

    /// Pin or unpin a project, and archive it or bring it back; None leaves that one as it is.
    pub fn update_project(&self, project_id: String, pinned: Option<bool>, archived: Option<bool>) {
        self.run(move |api| async move {
            api.project_update(project_id, pinned, archived).await?;
            Ok(None)
        });
    }

    /// Make a session and send its first message, for draft view `draft`: its answer names that view.
    pub fn create(&self, draft: String, mut new: NewSession) {
        // Over the service, a new worktree's branch takes the person's prefix.
        let prefix = self.api.remote().and_then(|_| (self.settings)().branch_prefix);
        let guarded = self.host.local_providers_opt_in.enabled();
        let source = nexus_workbench::api::StartAuthority {
            scope: self.host.scope(),
            identity: self.host.session_choice_identity(),
        };
        let original = self.source_guard();
        let host = self.host.clone();
        let marker = host.token().as_deref().map(nexus_desktop_core::secrets::marker);
        self.run(move |api| async move {
            let made = async move {
                let current =
                    || original() && host.token().as_deref().map(nexus_desktop_core::secrets::marker) == marker;
                if guarded && !current() {
                    return Err("The account changed before the session could start.".into());
                }
                if new.provider.is_none() && new.worktree && new.review.is_none() && new.base.is_none() {
                    let value = api
                        .call("wb_worktree_bases", json!({ "projectId": new.project_id }))
                        .await?;
                    let bases: Vec<String> = serde_json::from_value(value).map_err(|error| error.to_string())?;
                    new.base = default_worktree_base(&bases).cloned();
                }
                if new.provider.is_some() && !new.attachments.files.is_empty() {
                    return Err("This build cannot save shared files for a local provider session.".into());
                }
                // Larger files go up first, in pieces, for the session this request makes; a retry sends them again.
                let request = uuid::Uuid::new_v4().to_string();
                let files = if new.provider.is_none() {
                    upload(&api, &nexus_workbench::creation_thread_id(&request)?, &new.attachments).await?
                } else {
                    vec![]
                };
                let thread = api
                    .thread_create_at_scope(
                        CreateArgs {
                            request_id: (!files.is_empty()).then_some(request),
                            project_id: new.project_id,
                            agent: new.agent.as_str().to_string(),
                            model: new.model,
                            permission_mode: new.permission_mode,
                            effort: new.effort.map(|effort| effort.as_str().to_string()),
                            via: new.via.as_str().to_string(),
                            provider: new.provider.clone(),
                            images: crate::attachments::session_images(&new.attachments),
                            text: new.text,
                            files,
                            worktree: (new.provider.is_none() && new.worktree).then(|| WorktreeArgs {
                                review: new.review,
                                base: new.base,
                                ..WorktreeArgs::default()
                            }),
                            orchestration: None,
                            name: None,
                        },
                        prefix,
                        guarded.then_some(&source),
                        guarded.then_some(&current as &(dyn Fn() -> bool + Send + Sync)),
                    )
                    .await?;
                Ok::<_, String>(thread.id)
            }
            .await;
            Ok(Some(match made {
                Ok(id) => CodeEvent::Created { draft, id },
                Err(error) => CodeEvent::StartFailed { draft, error },
            }))
        });
    }

    /// Work that the UI's restart would interrupt. The background service and remote hosts survive it.
    pub fn update_restart_sessions(&self) -> Result<Vec<nexus_workbench::update_restart::Session>, String> {
        // The background service survives the UI's restart. SSH and cloud agents do too.
        if self.api.remote().is_some() {
            return Ok(Vec::new());
        }
        self.api.workbench().update_restart_sessions()
    }

    pub fn reserve_confirmed_update_restart(
        &self,
        confirmed: &[nexus_workbench::update_restart::Session],
    ) -> Result<Vec<nexus_workbench::update_restart::Session>, String> {
        if self.api.remote().is_some() {
            if !(self.keep_running)() {
                self.reserve_update_restart()?;
            }
            return Ok(Vec::new());
        }
        self.api.workbench().reserve_confirmed_update_restart(confirmed)
    }

    pub fn continue_after_update(&self, entry: crate::update_recovery::Entry) -> Answer<()> {
        let host = self.host.clone();
        self.ask(move |api| async move {
            if host
                .token()
                .as_deref()
                .map(nexus_desktop_core::secrets::marker)
                .as_deref()
                != Some(entry.session.identity.as_str())
                || host.scope() != entry.session.scope
            {
                return Err("The signed-in account changed. Switch back before continuing this agent.".into());
            }
            if api.remote().is_some() {
                return Err(
                    "These agents stopped in the app. Turn off the background service before continuing them here."
                        .into(),
                );
            }
            api.workbench().resume_after_update(&entry.session).await
        })
    }

    /// The existing idle-only reservation, also used when switching to the background service.
    pub fn reserve_update_restart(&self) -> Result<(), String> {
        if let Some(remote) = self.api.remote().cloned() {
            // The rule for the service: refused while it runs sessions, or when it does not say.
            let running = self
                .runtime
                .block_on(async move { tokio::time::timeout(Duration::from_secs(3), remote.sessions_running()).await });
            return match running {
                Ok(Some(false)) => Ok(()),
                _ => {
                    Err("Code sessions are still running in the background. Finish or stop them, then restart.".into())
                }
            };
        }
        self.api.workbench().reserve_update_restart()
    }

    pub(crate) fn awake_engine(&self) -> Engine {
        self.api.clone()
    }

    pub fn cancel_update_restart(&self) {
        self.api.workbench().cancel_update_restart();
    }

    /// Stop every agent (each gets a few seconds to end its turn) and end its Nexus session. Blocks: called as the
    /// app quits, so the agents are not cut off.
    pub fn shutdown(&self) {
        // Revoke attached as well as detached continuations before any shutdown wait.
        // Admitted Commands may still hold this runner while their receipts finish.
        self.host
            .source_generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // No schedule starts a session, and no sweep removes a worktree, while the others stop.
        if let Some(upkeep) = &self.upkeep {
            upkeep.abort();
        }
        self.sweeps.abort();
        if let Some(remote) = self.api.remote() {
            self.hand_over(remote);
            return;
        }
        let workbench = self.api.workbench().clone();
        let _ = self
            .runtime
            .block_on(async move { tokio::time::timeout(Duration::from_secs(15), workbench.shutdown()).await });
    }

    /// Run a command on the runtime and hand its answer back to whoever asked, not to the UI's events.
    fn ask<T, F>(
        &self,
        command: impl FnOnce(Engine) -> F + Send + 'static,
    ) -> futures::channel::oneshot::Receiver<Result<T, String>>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, String>> + Send + 'static,
    {
        let (answer, answered) = futures::channel::oneshot::channel();
        let api = self.api.clone();
        self.runtime.spawn(async move {
            let _ = answer.send(command(api).await);
        });
        answered
    }

    /// Run a command on the runtime; its error, or what it reports, goes to the UI.
    fn run<F>(&self, command: impl FnOnce(Engine) -> F + Send + 'static)
    where
        F: Future<Output = Result<Option<CodeEvent>, String>> + Send + 'static,
    {
        let (api, events) = (self.api.clone(), self.host.events());
        self.runtime.spawn(async move {
            let event = match command(api).await {
                Ok(Some(event)) => event,
                Ok(None) => CodeEvent::Changed,
                Err(error) => CodeEvent::Failed(error),
            };
            let _ = events.unbounded_send(event);
        });
    }
}

/// Who may pay for new sessions: the organization's policy and Gateway's routes (`GET /v1/agents/access`).
pub async fn read_access(client: &Client) -> AgentAccess {
    let (allowed, answer) = futures::join!(subscriptions_allowed(client), client.get("/v1/agents/access"));
    AgentAccess {
        subscriptions: allowed.unwrap_or(false),
        answer: answer.ok().filter(|answer| answer.get("agents").is_some()),
    }
}

/// A message's files as the core takes them: small ones whole, larger ones uploaded in pieces to this computer, for
/// session `thread_id` only.
async fn upload(
    api: &Engine,
    thread_id: &str,
    attachments: &crate::attachments::Draft,
) -> Result<Vec<nexus_workbench::attachments::FileInput>, String> {
    crate::attachments::sendable_files(
        thread_id,
        attachments,
        |chunk| api.attachment_chunk(chunk),
        |id| api.attachment_cancel(id, thread_id.to_string()),
    )
    .await
}

/// The runtime the workbench and its agents run on, for as long as the process: tasks hold the workbench, and a
/// runtime dropped on its own worker thread would panic.
pub(crate) fn runtime() -> tokio::runtime::Handle {
    static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("nexus-code")
                .enable_all()
                .build()
                .expect("the Code sessions runtime starts");
            Box::leak(Box::new(runtime)).handle().clone()
        })
        .clone()
}

#[cfg(not(test))]
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// In a test, a home folder under the test root (`test_agents.rs`): no test writes under the person's.
#[cfg(test)]
fn home() -> PathBuf {
    test_agents::home()
}

/// New work's and reviews' worktree folders: the person's, else `~/nexus-worktrees` and `~/nexus-reviews` (the Nexus
/// app's `workbench::worktree_roots`).
/// The plan accounts new sessions start on, and whether they switch near a limit, for Claude and for Codex, as
/// Settings saved them ([`nexus_workbench::accounts::Choice`]).
fn account_choices(settings: &nexus_desktop_core::settings::Settings) -> Vec<nexus_workbench::accounts::Choice> {
    [
        (SessionAgent::Claude, &settings.claude_account, settings.rotate_accounts),
        (
            SessionAgent::Codex,
            &settings.codex_account,
            settings.rotate_codex_accounts,
        ),
    ]
    .into_iter()
    .map(|(agent, account, rotation)| nexus_workbench::accounts::Choice {
        agent,
        account: account.clone(),
        rotate: rotation.unwrap_or_else(|| nexus_workbench::rotate::auto_rotate_by_default(agent)),
    })
    .collect()
}

/// [`account_choices`] on this build's workbench. An account removed since falls back to the agent's own login.
fn apply_accounts(workbench: &Workbench, settings: &nexus_desktop_core::settings::Settings) {
    workbench.apply_account_choices(&account_choices(settings));
}

fn worktree_roots(home: &Path, settings: &nexus_desktop_core::settings::Settings) -> (PathBuf, PathBuf) {
    let chosen = |dir: &Option<String>, default: &str| {
        dir.as_deref()
            .filter(|dir| !dir.trim().is_empty())
            .map_or_else(|| home.join(default), PathBuf::from)
    };
    (
        chosen(&settings.worktrees_dir, "nexus-worktrees"),
        chosen(&settings.reviews_dir, "nexus-reviews"),
    )
}

/// The Gateway and its live client source change together when the app switches Gateway.
struct SignIn {
    gateway: String,
    client: crate::backend::SessionClient,
}

/// Starts agents through Gateway with the app's active sign-in, and tells the UI what changed.
struct GpuiHost {
    local_providers_opt_in: LocalProvidersOptIn,
    /// The Gateway agents start through; Settings' Gateway switch moves it (`CodeRunner::set_gateway`).
    sign_in: Arc<RwLock<SignIn>>,
    /// The organization signed in to, as sessions are scoped: a launch for another is refused.
    scope: Arc<RwLock<SessionScope>>,
    /// Advances on every actual source change, so A → B → A cannot revive old local owners.
    source_generation: std::sync::atomic::AtomicU64,
    /// Where Codex's catalog is written.
    data: PathBuf,
    prices: Arc<RwLock<Catalog>>,
    /// The installed agents' models as discovery last found them ([`CodeRunner::load_agents`]): what Auto offers on the
    /// person's plan.
    agent_models: Arc<RwLock<Vec<AgentInfo>>>,
    /// Where events go: the app's reader, which hands them to every window (`windows.rs`).
    events: Mutex<UnboundedSender<CodeEvent>>,
    /// Each session's title, as its summary last said, for its notifications.
    titles: Mutex<HashMap<String, String>>,
    /// Sub-agents' names, so an approval says which one asks.
    agents: crate::notify::AgentNames,
    /// The built-in server's socket: each run gets a credential for it at launch, which ends with the run.
    app: OnceLock<Arc<AppSocket>>,
    /// Session sync's backend for the current sign-in.
    sync: Arc<session_sync::Slot>,
}

impl GpuiHost {
    /// A moment worth a notification (`notify.rs`), and the pull request watch's nudge when a turn ends.
    fn notify(&self, emit: &Emit) {
        match emit {
            Emit::Event(event) => {
                if matches!(event.event, WbEvent::TurnEnd { .. }) {
                    crate::pr_watch::turn_ended(&event.thread_id);
                    let _ = self
                        .events()
                        .unbounded_send(CodeEvent::TurnEnded(event.thread_id.clone()));
                }
                let asker = self.agents.read(&event.thread_id, &event.event);
                let Some(body) = crate::notify::message(&event.event, asker.as_deref()) else {
                    return;
                };
                let title = self
                    .titles
                    .lock()
                    .ok()
                    .and_then(|titles| titles.get(&event.thread_id).cloned())
                    .unwrap_or_else(|| "Code session".into());
                crate::notify::GpuiNotifier.show(None, &event.thread_id, &title, &body);
            }
            Emit::Thread(thread) => {
                if let Ok(mut titles) = self.titles.lock() {
                    titles.insert(thread.id.clone(), thread.title.clone());
                }
                // The windows read the store, which never holds these: keep them for the reads.
                crate::live_fields::noted(thread);
                // A pull request its agent just opened: the watch reads it now.
                crate::pr_watch::saw_links(thread);
            }
            Emit::Plan(report) => crate::plan_book::reported(report),
            Emit::Delta(_) | Emit::Projects(_) => {}
        }
    }
}

impl GpuiHost {
    fn events(&self) -> UnboundedSender<CodeEvent> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn scope(&self) -> String {
        self.session_scope().into_text()
    }

    fn session_scope(&self) -> SessionScope {
        self.scope.read().map(|scope| scope.clone()).unwrap_or_default()
    }

    fn token(&self) -> Option<String> {
        self.client()?.options().api_key.clone()
    }

    /// The sign-in the background service runs agents on (`{ gateway, token, scope }`), with this computer's device
    /// key while session sync is on; None when signed out.
    fn sign_in(&self) -> Option<Value> {
        let client = self.client()?;
        let token = client.options().api_key.as_ref()?;
        let gateway = &client.options().base_url;
        let scope = self.session_scope();
        let scope = match &scope {
            SessionScope::Local | SessionScope::Unscoped => return None,
            SessionScope::Other(scope) => scope.as_str(),
        };
        let mut sign_in = json!({ "gateway": gateway, "token": token, "scope": scope });
        // The service syncs as this computer while this build's own sync would run (nexus-host's `local_sync`).
        if let Some(key) = session_sync::service_device_key(
            &crate::keychain::vault(),
            gateway,
            crate::managed::local_cache(crate::managed::policy()),
        ) {
            sign_in["deviceKey"] = json!(key.as_str());
        }
        // Settings' MCP servers with their keychain values: the service's agents start with them, as this build's own.
        let settings = crate::desktop_settings::load();
        sign_in["mcp"] = crate::mcp::for_service(&settings.mcp_servers);
        // Settings' plan accounts: the service's new sessions start on them, as this build's own.
        sign_in["accounts"] = json!(account_choices(&settings));
        Some(sign_in)
    }

    #[cfg(unix)]
    fn service_session(&self) -> Option<Value> {
        self.sign_in().or_else(|| {
            if !self.local_providers_opt_in.enabled() || !self.session_scope().is_local() {
                return None;
            }
            let (client, signed_out) = self.launch_client();
            let mcp_field = if signed_out {
                "localMcp"
            } else if client.is_some() {
                "localAccountPendingMcp"
            } else {
                return None;
            };
            let settings = crate::desktop_settings::load();
            Some(json!({
                (mcp_field): crate::mcp::for_service(&settings.mcp_servers),
                "accounts": account_choices(&settings),
            }))
        })
    }

    fn client(&self) -> Option<Client> {
        self.launch_client().0
    }

    /// The scoped client and whether the credential source is signed out.
    fn launch_client(&self) -> (Option<Client>, bool) {
        let sign_in = self.sign_in.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(client) = (sign_in.client)().filter(|client| client.options().api_key.is_some()) else {
            return (None, true);
        };
        // A source for another Gateway must never send its credential here or admit a local run.
        let matches = client.options().base_url.trim_end_matches('/') == sign_in.gateway.trim_end_matches('/');
        (matches.then_some(client), false)
    }
}

async fn launch_local(
    route: LocalLaunch<'_>,
    request: LaunchRequest,
    app: Option<&AppSocket>,
    check_owner: &(dyn Fn() -> Result<(), String> + Sync),
) -> Result<Launch, String> {
    let agent = request.agent;
    let entry = agent.coding().expect("the local launch gate checked the agent").entry();
    let program = find(agent).ok_or_else(|| {
        format!(
            "{} is not installed on this computer. Install it with `{}`, then try again.",
            label(agent),
            entry.install.command,
        )
    })?;
    let env = agent_env();
    match route {
        LocalLaunch::SavedProvider(local) => {
            let settings = nexus_desktop_core::provider_settings::read_for_launch().await?;
            let servers = crate::desktop_settings::load().mcp_servers;
            let values = crate::mcp::values(&servers);
            let exe = std::env::current_exe().ok();
            nexus_workbench::local_provider::prepare(
                &request,
                program,
                env,
                nexus_workbench::local_provider::Source {
                    settings: &settings,
                    local,
                    check_owner,
                },
                nexus_workbench::local_provider::Mcp {
                    program: exe.as_deref(),
                    servers: &servers,
                    values: &values,
                    native: Default::default(),
                },
                |run, servers| app?.grant_local_launch(run, servers),
            )
            .await
        }
        LocalLaunch::Subscription(admission) => {
            if let Some(dir) = &request.account_dir {
                accounts::share(dir, agent, &agent_home(agent, &env));
            }
            nexus_workbench::agent_launch::local_subscription(admission, &request, program, env, |launch| {
                let servers = crate::desktop_settings::load().mcp_servers;
                let values = crate::mcp::values(&servers);
                let exe = std::env::current_exe().ok();
                nexus_workbench::mcp_servers::configure_local(
                    launch,
                    agent,
                    exe.as_deref(),
                    &servers,
                    &values,
                    |run, servers| app?.grant_local_launch(run, servers),
                );
            })
            .await
        }
    }
}

fn check_saved_provider(sign_in: &SignIn, scope: &SessionScope) -> Result<(), String> {
    if !scope.is_local() || (sign_in.client)().is_some_and(|client| client.options().api_key.is_some()) {
        return Err(nexus_workbench::provider::saved_provider_refusal(true));
    }
    Ok(())
}

fn with_saved_owner(
    sign_in: &RwLock<SignIn>,
    scope: &RwLock<SessionScope>,
    action: &mut dyn FnMut(),
) -> Result<(), String> {
    let _transition = crate::backend::SESSION_AUTH_TRANSITION
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let sign_in = sign_in.read().unwrap_or_else(|poisoned| poisoned.into_inner());
    let scope = scope.read().unwrap_or_else(|poisoned| poisoned.into_inner());
    check_saved_provider(&sign_in, &scope)?;
    action();
    Ok(())
}

impl Host for GpuiHost {
    fn local_providers_opt_in(&self) -> &LocalProvidersOptIn {
        &self.local_providers_opt_in
    }

    fn shows_account_free_sessions(&self) -> bool {
        self.local_providers_opt_in.enabled()
    }

    fn saved_server_models(&self, provider: &nexus_workbench::provider::InstanceId) -> Result<Vec<String>, String> {
        nexus_desktop_core::provider_settings::saved_server_models(provider)
    }

    fn while_saved_provider_allowed(&self, action: &mut dyn FnMut()) -> Result<(), String> {
        with_saved_owner(&self.sign_in, &self.scope, action)
    }

    /// Settings › Agents › Antigravity's Sandbox switch, read from the saved settings each time agy starts.
    fn antigravity_sandbox(&self) -> bool {
        crate::desktop_settings::load().antigravity_sandboxed()
    }
    fn update_recovery_identity(&self) -> Option<String> {
        self.token().as_deref().map(nexus_desktop_core::secrets::marker)
    }
    fn computer_choices(
        &self,
        agents: Vec<SessionAgent>,
        claude_account: Option<PathBuf>,
    ) -> BoxFuture<'static, Result<nexus_workbench::computer::settings::NativeChoices, String>> {
        let profiles = claude_account
            .into_iter()
            .map(|dir| (SessionAgent::Claude, dir))
            .collect();
        self.computer_choices_by_account(agents, profiles)
    }

    fn computer_choices_by_account(
        &self,
        agents: Vec<SessionAgent>,
        profiles: nexus_workbench::computer::settings::AccountProfiles,
    ) -> BoxFuture<'static, Result<nexus_workbench::computer::settings::NativeChoices, String>> {
        let client = self.client();
        Box::pin(async move {
            let infos = tokio::task::spawn_blocking(move || agents.into_iter().map(agent_info).collect())
                .await
                .map_err(|_| "Could not inspect the installed agents.")?;
            nexus_workbench::computer::settings::native_choices_by_account(client, infos, agent_env(), profiles).await
        })
    }
    fn refresh_session_sync(&self) {
        self.sync.retry_unavailable();
    }
    /// The backend for the sign-in now, or None: off, not allowed, signed out, not yet resolved or refused.
    fn session_sync(&self) -> Option<Arc<dyn nexus_workbench::session_sync::Backend>> {
        if !session_sync::wanted(crate::managed::local_cache(crate::managed::policy())) {
            return None;
        }
        let client = self.client()?;
        let marker = nexus_desktop_core::secrets::marker(client.options().api_key.as_deref()?);
        let gateway = client.options().base_url.clone();
        let scope = self.scope();
        let (slot, data) = (self.sync.clone(), self.data.clone());
        self.sync.current(
            &session_sync::sign_in(&scope, &marker),
            |task| {
                runtime().spawn(task);
            },
            move || {
                Box::pin(async move {
                    let vault = crate::keychain::vault();
                    let device = nexus_desktop_core::device_key::stored_key(&vault, &gateway).ok()??;
                    let keys = slot.keys(|| Some(data.join(session_sync::KEYS_FILE)))?;
                    let allowed = crate::managed::local_cache(crate::managed::policy());
                    session_sync::connect(client, scope, device, keys, allowed).await
                })
            },
        )
    }

    fn session_choice_identity(&self) -> Option<String> {
        self.token()
            .as_deref()
            .map(nexus_workbench::session_choice::sign_in_identity)
    }

    fn marks_commits(&self) -> BoxFuture<'static, bool> {
        let client = self.client();
        Box::pin(async move {
            match client {
                Some(client) => commit_trailer::desktop_eligible(&client).await,
                None => false,
            }
        })
    }

    /// Every account's limits, for the workbench's `rotate`. Without them it knows only the session's own account, so
    /// auto-rotation has nowhere to move.
    fn plans(&self) -> BoxFuture<'static, Vec<nexus_workbench::plans::PlanUsage>> {
        Box::pin(async {
            tokio::task::spawn_blocking(crate::pages::subscriptions::probe_all)
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default()
        })
    }

    fn ensure_prices(&self) -> BoxFuture<'static, ()> {
        let (client, known) = (self.client(), self.prices.clone());
        Box::pin(async move {
            let Some(client) = client else { return };
            let catalog = load_catalog(&client).await;
            if !catalog.is_empty()
                && let Ok(mut held) = known.write()
            {
                *held = catalog;
            }
        })
    }

    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch, String>> {
        let (client, signed_out) = self.launch_client();
        let scope = self.session_scope();
        let data = self.data.clone();
        let known_prices = self.prices.clone();
        let installed = self.agent_models.clone();
        let app = self.app.get().cloned();
        let owner_sign_in = self.sign_in.clone();
        let owner_scope = self.scope.clone();
        let local_providers_opt_in = self.local_providers_opt_in.clone();
        Box::pin(async move {
            if let Some(reason) = request.agent.local_launch_refusal() {
                return Err(reason);
            }
            if request.via == Via::Subscription {
                nexus_desktop_core::daily_active::note(nexus_desktop_core::daily_active::Mode::OwnSubscription);
            }
            let agent = label(request.agent);
            if request.provider.is_some() && (!signed_out || !scope.is_local()) {
                return Err(nexus_workbench::provider::saved_provider_refusal(true));
            }
            let local_scope = if client.is_some() && request.provider.is_none() {
                &request.scope
            } else {
                &scope
            };
            if (signed_out || client.is_some())
                && let Some(route) = admit_local_launch(&request, local_scope, &local_providers_opt_in)
            {
                let check_owner = || with_saved_owner(&owner_sign_in, &owner_scope, &mut || {});
                return launch_local(route, request, app.as_deref(), &check_owner).await;
            }
            let Some(client) = client else {
                if request.provider.is_some() {
                    return Err(nexus_workbench::provider::saved_provider_refusal(false));
                }
                return Err(format!("Sign in to Nexus to start {agent}."));
            };
            let gateway = client.options().base_url.clone();
            let token = client
                .options()
                .api_key
                .clone()
                .ok_or_else(|| format!("Sign in to Nexus to start {agent}."))?;
            if request.scope.is_local() {
                return Err("This session can only use a local subscription.".into());
            }
            if request.scope != scope {
                return Err(format!(
                    "This session belongs to another organization than the one you are signed in to, so {agent} \
                     won't start on this sign-in."
                ));
            }
            // Follow the other desktop hosts: verify eligibility at each start, before preparing any agent.
            let eligible = commit_trailer::desktop_eligible(&client).await;
            // Nexus's own agent is no coding agent: it runs the nexus CLI.
            if request.agent.coding().is_none() {
                // Checked at every start, never from a cache: a `nexus` installed or replaced since counts now.
                let mut env = commit_environment(agent_env(), eligible);
                if nexus_agent_memory::Store::local()
                    .and_then(|store| store.enabled())
                    .unwrap_or(false)
                {
                    env.insert(
                        nexus_agent_memory::tools::AGENT_ENV.into(),
                        request.agent.as_str().into(),
                    );
                }
                let runtime = tokio::task::spawn_blocking({
                    let env = env.clone();
                    move || nexus_runtime::select(&env)
                })
                .await
                .map_err(|e| e.to_string())?;
                let Some(program) = runtime.ready() else {
                    return Err(nexus_runtime::refusal(
                        &runtime,
                        &nexus_runtime::install_there(&gateway),
                    ));
                };
                return nexus_workbench::nexus::launch(&request, program.to_path_buf(), env, &gateway, &token);
            }
            let Some(client_agent) = request.agent.coding() else {
                return Err(format!("{agent} cannot start here."));
            };
            let Some(program) = find(request.agent) else {
                return Err(format!(
                    "{agent} is not installed on this computer. Install it with `{}`, then try again.",
                    client_agent.entry().install.command
                ));
            };
            // An agent that takes its servers in its session gets them through the launcher, with no app tools.
            let in_session = nexus_workbench::mcp_servers::in_session(request.agent);
            let offers_tools = nexus_workbench::harness::harness(request.agent).app_tools().offered() || in_session;
            let launch_env = agent_env();
            let launch_home = Some(home().to_string_lossy().into_owned());
            let auto = (agents::is_auto_name(request.model.as_deref()) && crate::workspace::takes_auto(request.agent))
                .then(|| {
                    let account = request
                        .account_dir
                        .as_deref()
                        .and_then(Path::file_name)
                        .map(|name| name.to_string_lossy().into_owned());
                    let plan_limits = crate::plan_book::current(request.agent, &account)
                        .filter(|plan| plan.error.is_none())
                        .map(|plan| plan.limits);
                    let models = installed
                        .read()
                        .ok()
                        .and_then(|agents| {
                            agents
                                .iter()
                                .find(|info| info.agent == request.agent)
                                .and_then(|info| info.models.clone())
                        })
                        .unwrap_or_default();
                    nexus_workbench::agent_launch::AutoContext {
                        prices: Some(&known_prices),
                        installed: models,
                        plan_limits,
                        now_ms: crate::code::now_ms(),
                    }
                });
            let nexus_workbench::agent_launch::OpenedSession {
                session_id,
                model,
                team,
                access,
                mut args,
            } = nexus_workbench::agent_launch::open_session(
                &client,
                &request,
                &program,
                launch_env,
                launch_home.as_deref(),
                auto,
                &nexus_workbench::provider::SessionFacts {
                    usage_data: nexus_desktop_core::usage::allowed(),
                },
            )
            .await?;
            let mut env = agent_env();
            // Read Settings only for agents with an app-tool transport.
            let mut servers = if offers_tools {
                crate::desktop_settings::load().mcp_servers
            } else {
                Default::default()
            };
            let mut values = crate::mcp::values(&servers);
            // Nexus's own tools, under this app's sign-in, unless Settings has a server named nexus.
            if offers_tools
                && let Ok(exe) = std::env::current_exe()
                && let Some((mut server, mut own)) =
                    crate::mcp::builtin(&servers, &exe.to_string_lossy(), &gateway, &token)
            {
                // Its pull request tools and `request_secret` reach this run over the app's socket (`app_socket.rs`).
                let sign_in = nexus_workbench::session_choice::sign_in_identity(&token);
                if let Some((address, app_token)) = app
                    .as_ref()
                    .and_then(|app| app.grant(&request.thread_id, request.scope.as_str(), &session_id, &sign_in))
                {
                    crate::mcp::with_app_tools(&mut server, &mut own, &address.to_string(), &app_token);
                }
                nexus_workbench::mcp_servers::with_shared_memory(
                    &mut server,
                    &mut own,
                    request.agent,
                    &request.thread_id,
                );
                servers.insert(0, server);
                values.extend(own);
            }
            // The session's browser, for the agent's `nexus_browser` tools, over the same socket with a credential of
            // its own, as the Nexus app gives it.
            let sign_in = nexus_workbench::session_choice::sign_in_identity(&token);
            if offers_tools
                && let Ok(exe) = std::env::current_exe()
                && let Some((address, browser_token)) = app
                    .as_ref()
                    .and_then(|app| app.grant(&request.thread_id, request.scope.as_str(), &session_id, &sign_in))
                && let Some((server, own)) = nexus_desktop_mcp::browser::configuration(
                    &servers,
                    &exe.to_string_lossy(),
                    &address.to_string(),
                    &browser_token,
                )
            {
                servers.push(server);
                values.extend(own);
            }
            // Claude Code starts each local server through the launcher, which keeps the token out of it (`mcp_launch`).
            let launcher = std::env::current_exe()
                .map(|exe| exe.to_string_lossy().into_owned())
                .unwrap_or_default();
            let launched = nexus_workbench::mcp_servers::through_launcher_or_builtin(
                request.agent,
                &launcher,
                &servers,
                &values,
                |launchable| {
                    let app = app.as_ref().filter(|_| !launcher.is_empty())?;
                    let (address, launch_token) = app.grant_launch(
                        &request.thread_id,
                        request.scope.as_str(),
                        &session_id,
                        &sign_in,
                        launchable,
                    )?;
                    Some((address.to_string(), launch_token))
                },
            );
            let left_out = launched
                .as_ref()
                .map(|launched| launched.left_out.clone())
                .unwrap_or_default();
            let (mcp_args, mcp_env) = match launched {
                Some(launched) => (launched.args, launched.env),
                None => crate::mcp::launch(request.agent, &servers, &values),
            };
            args.splice(0..0, mcp_args);
            env.extend(mcp_env);
            // agy writes its servers' environment to disk: it gets the launcher and a launch credential, and the
            // values stay here (`app_socket.rs` answers the launcher).
            let session_mcp = match nexus_workbench::mcp_servers::session(request.agent, &servers, &values) {
                Some(session) if in_session => std::env::current_exe()
                    .ok()
                    .zip(app.as_ref().and_then(|app| {
                        app.grant_launch(
                            &request.thread_id,
                            request.scope.as_str(),
                            &session_id,
                            &sign_in,
                            session.launchable.clone(),
                        )
                    }))
                    .map(|(exe, (address, launch_token))| {
                        nexus_workbench::mcp_servers::session_entries(
                            &exe.to_string_lossy(),
                            &address.to_string(),
                            &launch_token,
                            &session,
                        )
                    })
                    .unwrap_or_default(),
                // The servers that did not start, for the session's notice.
                _ => nexus_workbench::mcp_servers::SessionMcp {
                    servers: Vec::new(),
                    skipped: left_out.clone(),
                },
            };
            if let Some(dir) = &request.account_dir {
                accounts::share(dir, request.agent, &agent_home(request.agent, &env));
                if let Some(variable) = accounts::account_variable(request.agent) {
                    env.insert(variable.into(), dir.to_string_lossy().into_owned());
                }
            }
            let route = if request.via == Via::Subscription {
                Route::Subscription
            } else {
                Route::Gateway
            };
            let web_search = codex_web_search(&client, client_agent, route).await;
            let (args, env) = nexus_workbench::agent_launch::launch(
                client_agent,
                &program,
                &data,
                eligible,
                LaunchOptions {
                    core_url: &gateway,
                    api_key: &token,
                    session_id: &session_id,
                    team: team.as_deref(),
                    model: if request.model.is_some() {
                        None
                    } else {
                        model.as_deref()
                    },
                    route,
                    model_catalog: None,
                    web_search,
                    args,
                    env,
                },
            )
            .await;
            let catalog = load_catalog(&client).await;
            if !catalog.is_empty()
                && let Ok(mut known) = known_prices.write()
            {
                *known = catalog;
            }
            Ok(Launch {
                attribution: Some(nexus_workbench::event::SessionAttribution {
                    gateway: nexus_workbench::connection::gateway_origin(&gateway),
                    organization: request
                        .scope
                        .as_str()
                        .split_once('|')
                        .map(|(_, org)| org.to_string())
                        .filter(|org| !org.is_empty()),
                    team,
                    access,
                    // agy gets only the launched servers, not the remote ones it skipped.
                    configured_mcp: Some(if in_session {
                        session_mcp.names()
                    } else {
                        servers
                            .iter()
                            .filter(|server| server.enabled && !left_out.contains(&server.name))
                            .map(|server| server.name.clone())
                            .collect()
                    }),
                }),
                spawn: Spawn {
                    program,
                    args,
                    env,
                    cwd: request.cwd,
                },
                nexus_session: Some(session_id),
                local_run: None,
                local_provider: None,
                model,
                session_mcp,
            })
        })
    }

    fn end_local_run(&self, run: &str) {
        if let Some(app) = self.app.get() {
            app.end_run(run);
        }
    }

    fn ended(&self, nexus_session: String, end: ProcessEnd) -> BoxFuture<'static, bool> {
        // The run's credential for the app's socket goes, with its open secret card and its files.
        if let Some(app) = self.app.get() {
            app.end_run(&nexus_session);
        }
        let client = self.client();
        Box::pin(async move {
            // Not signed in: nothing can end it now; asking again later may.
            let Some(client) = client else {
                return false;
            };
            let path = format!("/v1/agents/sessions/{}/end", nexus_client::url::enc(&nexus_session));
            let exit_code = if (0..=255).contains(&end.exit_code) {
                end.exit_code
            } else {
                1
            };
            let body = json!({ "exitCode": exit_code, "durationMs": end.duration_ms, "signal": end.signal });
            match client.post(&path, body).await {
                Ok(_) => true,
                Err(error) => {
                    eprintln!("nexus-desktop-gpui: could not end agent session {nexus_session}: {error}");
                    end_refused(&error)
                }
            }
        })
    }

    fn price(&self, model: &str, usage: &Usage) -> Option<f64> {
        let catalog = self.prices.read().ok()?;
        let prices = &catalog.prices;
        let key = model_key(model);
        let (input, output) = prices.get(&key).copied().or_else(|| {
            prices
                .iter()
                .filter(|(id, _)| key.starts_with(id.as_str()))
                .max_by_key(|(id, _)| id.len())
                .map(|(_, price)| *price)
        })?;
        Some(list_price(input, output, usage))
    }

    fn top_price(&self, usage: &Usage) -> Option<f64> {
        let catalog = self.prices.read().ok()?;
        let prices = &catalog.prices;
        // The highest input and the highest output price, each of any model: never below what the call cost.
        let input = prices.values().map(|(input, _)| *input).reduce(f64::max)?;
        let output = prices.values().map(|(_, output)| *output).reduce(f64::max)?;
        Some(list_price(input, output, usage))
    }

    fn emit(&self, emit: Emit) {
        self.notify(&emit);
        match &emit {
            // Streamed text is not stored: the open view draws it until the turn's text event lands (`stream.rs`).
            Emit::Delta(delta) => {
                if crate::stream::delta(delta) {
                    let _ = self.events().unbounded_send(CodeEvent::Streamed);
                }
                return;
            }
            Emit::Event(stored) => {
                crate::stream::event(&stored.thread_id, &stored.event);
            }
            // The store did not change; the open views show the plan book's news on their next tick.
            Emit::Plan(_) => return,
            _ => {}
        }
        let _ = self.events().unbounded_send(CodeEvent::Changed);
    }
}

/// A `wb:thread` push of `thread`, as the background service sends it, through a host of its own: the path a session's
/// live fields take to the windows' reads.
#[cfg(test)]
pub(crate) fn push_thread_for_test(thread: nexus_workbench::event::ThreadSummary) {
    let (events, _) = unbounded();
    let host = GpuiHost {
        local_providers_opt_in: LocalProvidersOptIn::Disabled,
        sign_in: Arc::new(RwLock::new(SignIn {
            gateway: String::new(),
            client: Arc::new(|| None),
        })),
        scope: Arc::default(),
        source_generation: std::sync::atomic::AtomicU64::new(0),
        data: PathBuf::new(),
        prices: Arc::default(),
        agent_models: Arc::default(),
        events: Mutex::new(events),
        titles: Mutex::default(),
        agents: Default::default(),
        app: OnceLock::new(),
        sync: Arc::default(),
    };
    let payload = serde_json::to_value(&thread).expect("a summary serializes");
    let pushed = serde_json::from_value(payload).expect("the push reads back");
    host.emit(Emit::Thread(Box::new(pushed)));
}

/// The background service's sessions with their live fields (`wb_threads`). Not `wb_overview`: that one also checks
/// every agent's version and asks Gateway, which a reconnect must not set off. A service older than this answers an
/// error, which is logged, and its pushes fill the fields in.
fn service_threads(
    remote: nexus_remote::machine::Remote,
) -> impl std::future::Future<Output = Result<Vec<nexus_workbench::event::ThreadSummary>, String>> + Send + 'static {
    async move {
        let threads = remote
            .call("wb_threads", serde_json::json!({}))
            .await
            .map_err(|error| error.to_string())?;
        serde_json::from_value(threads).map_err(|error| format!("The background service's threads: {error}"))
    }
}

/// Keeps the live fields of every session `read` returns (`live_fields::seed`), then the windows read again. A push
/// that lands while it reads wins over what it read.
fn seed_live_fields(
    read: impl std::future::Future<Output = Result<Vec<nexus_workbench::event::ThreadSummary>, String>> + Send + 'static,
    host: &Arc<GpuiHost>,
    runtime: &tokio::runtime::Handle,
) {
    let mark = crate::live_fields::mark();
    let host = Arc::downgrade(host);
    runtime.spawn(async move {
        match read.await {
            Ok(threads) => {
                crate::live_fields::seed(mark, threads);
                if let Some(host) = host.upgrade() {
                    let _ = host.events().unbounded_send(CodeEvent::Changed);
                }
            }
            Err(error) => eprintln!("nexus-desktop-gpui: the sessions' live fields: {error}"),
        }
    });
}

/// What the background service pushes, as this build's own workbench emits it: notifications, streamed text, and the
/// windows read the store again. Its agents' calls of the built-in server's tools, answered as on this build's socket.
#[cfg_attr(not(unix), allow(dead_code))]
struct ServiceSink {
    host: Arc<GpuiHost>,
    status: Arc<Mutex<nexus_remote::machine::Status>>,
    /// The open `request_secret` cards.
    secrets: GpuiSecrets,
    runtime: tokio::runtime::Handle,
    /// The connection the answers go back on, set once it starts.
    remote: OnceLock<nexus_remote::machine::Remote>,
    /// Hands a browser call to the windows (`side::browser::agent::post`, or a test's fake).
    browser: crate::app_socket::BrowserPost,
    /// The service's runs that ended: a browser call of one still waiting stops.
    ended: Arc<Mutex<std::collections::HashSet<String>>>,
}

/// The servers' agents' calls of the app's tools are answered with this runner's cards and browser from now on
/// (`machines::server_tools`).
fn serve_server_tools(
    secrets: &GpuiSecrets,
    runtime: &tokio::runtime::Handle,
    loops: &Loops,
    data: PathBuf,
    scope: Arc<dyn Fn() -> String + Send + Sync>,
) {
    crate::machines::get().set_server_tools(crate::machines::ServerTools {
        secrets: secrets.clone(),
        runtime: runtime.clone(),
        browser: loops.browser.clone(),
        data,
        scope,
    });
}

/// Whether a browser call the background service passed on still holds, as a credential on this build's socket does:
/// this build is signed in to the run's organization (`scope`) as the run's person (`sign_in`), and the run goes on.
fn relayed_call_holds(
    grant: &nexus_desktop_mcp::app_socket::Grant,
    scope: &str,
    sign_in: Option<&str>,
    ended: &std::collections::HashSet<String>,
) -> bool {
    grant.scope == scope && sign_in == Some(grant.sign_in.as_str()) && !ended.contains(&grant.run)
}

impl ServiceSink {
    /// A call of the built-in server's or the browser's tools that the service passes on
    /// (`nexus_desktop_mcp::service_relay`): answered on a thread of its own, as on this build's socket
    /// (`app_socket.rs`). A browser call holds while this build is signed in as the run's person, to its organization,
    /// and the run goes on. A run that ended closes its card, its files and its requests for the browser.
    fn builtin_tools(&self, pushed: nexus_desktop_mcp::service_relay::Pushed) {
        use nexus_desktop_mcp::service_relay::{Pushed, reply};
        match pushed {
            Pushed::Call(call) => {
                let (Some(remote), host) = (self.remote.get().cloned(), self.host.clone()) else {
                    return;
                };
                let (secrets, runtime) = (self.secrets.clone(), self.runtime.clone());
                let (browser, ended) = (self.browser.clone(), self.ended.clone());
                std::thread::spawn(move || {
                    let args = reply(&call, &host.scope(), |grant, input| {
                        if !input["tools"].is_null() {
                            return crate::app_socket::call(grant, input, &runtime, &secrets);
                        }
                        let valid: Arc<dyn Fn() -> bool + Send + Sync> = {
                            let (host, grant) = (Arc::downgrade(&host), grant.clone());
                            Arc::new(move || {
                                let (Some(host), Ok(ended)) = (host.upgrade(), ended.lock()) else {
                                    return false;
                                };
                                relayed_call_holds(
                                    &grant,
                                    &host.scope(),
                                    host.session_choice_identity().as_deref(),
                                    &ended,
                                )
                            })
                        };
                        crate::app_socket::browser(&browser, &host.data, None, grant, &input["message"], valid)
                    });
                    let answered = runtime.block_on(remote.call(nexus_desktop_mcp::app_socket::REPLY_CALL, args));
                    if let Err(error) = answered {
                        eprintln!("nexus-desktop-gpui: could not answer the background service: {error}");
                    }
                });
            }
            Pushed::RunEnded(run) => {
                self.secrets.end_run(&run);
                if let Ok(mut ended) = self.ended.lock() {
                    ended.insert(run);
                }
                crate::side::browser::agent::REQUESTS.prune_invalid();
            }
        }
    }
}

impl ServiceSink {
    fn notify_changed(&self) {
        if let Err(error) = self.host.events().unbounded_send(CodeEvent::Changed) {
            eprintln!("nexus-desktop-gpui: could not report a background service change: {error}");
        }
    }
}

/// Keep the visible refusal while the service silently retries. A retry countdown is not shown in the app.
fn update_presented_service_status(
    shown: &mut nexus_remote::machine::Status,
    incoming: &nexus_remote::machine::Status,
) -> bool {
    use nexus_remote::machine::Status;
    let same = match (&*shown, incoming) {
        (Status::Unreachable { .. }, Status::Connecting { attempt }) if *attempt > 1 => true,
        (Status::Unreachable { error: before, .. }, Status::Unreachable { error, .. }) => before == error,
        (before, after) => before == after,
    };
    if !same {
        *shown = incoming.clone();
    }
    !same
}

impl nexus_remote::machine::Sink for ServiceSink {
    fn push(&self, push: &nexus_remote::client::Pushed) {
        if let Some(pushed) = nexus_desktop_mcp::service_relay::read(&push.event, &push.payload) {
            return self.builtin_tools(pushed);
        }
        let emit = match push.event.as_str() {
            "wb:event" => serde_json::from_value(push.payload.clone()).ok().map(Emit::Event),
            "wb:thread" => serde_json::from_value(push.payload.clone())
                .ok()
                .map(|thread| Emit::Thread(Box::new(thread))),
            "wb:delta" => serde_json::from_value(push.payload.clone()).ok().map(Emit::Delta),
            "wb:plan" => serde_json::from_value(push.payload.clone()).ok().map(Emit::Plan),
            _ => None,
        };
        match emit {
            Some(emit) => self.host.emit(emit),
            None => self.notify_changed(),
        }
    }

    fn status(&self, status: &nexus_remote::machine::Status) {
        use nexus_remote::machine::Status;
        let changed = {
            let mut shown = self.status.lock().unwrap_or_else(|error| error.into_inner());
            update_presented_service_status(&mut shown, status)
        };
        if !changed {
            return;
        }
        match status {
            // Connected (again): what the service did meanwhile is in the store, but the live fields only in the
            // service: read them.
            Status::Connected { host, .. } => {
                if host
                    .features
                    .iter()
                    .any(|f| f == nexus_workbench::orchestra::LIMITS_FEATURE)
                    && let Some(remote) = self.remote.get().cloned()
                {
                    reload_linked_limits(&self.runtime, remote);
                }
                if let Some(remote) = self.remote.get().cloned() {
                    seed_live_fields(service_threads(remote), &self.host, &self.runtime);
                }
            }
            Status::NeedsUpdate { error, .. } | Status::Unreachable { error, .. } => {
                eprintln!("nexus-desktop-gpui: background service: {error}");
            }
            _ => {}
        }
        self.notify_changed();
    }
}

/// What the shared commands need from this computer.
struct GpuiMachine {
    host: Arc<GpuiHost>,
    home: PathBuf,
}

impl Machine for GpuiMachine {
    fn tool_path(&self) -> String {
        agent_env().remove("PATH").unwrap_or_default()
    }

    fn home(&self) -> Result<PathBuf, String> {
        Ok(self.home.clone())
    }

    fn agent(&self, agent: SessionAgent) -> AgentInfo {
        agent_info(agent)
    }

    fn subscriptions_allowed(&self) -> BoxFuture<'static, bool> {
        let client = self.host.client();
        Box::pin(async move {
            match client {
                Some(client) => subscriptions_allowed(&client).await.unwrap_or(false),
                None => false,
            }
        })
    }
}

/// An agent on this computer: where it is and its version, its models not read yet.
fn agent_info(agent: SessionAgent) -> AgentInfo {
    // Nexus's own agent is no coding agent: it is the nexus CLI.
    if agent.coding().is_none() {
        return nexus_runtime::info(&nexus_runtime::select(&agent_env()));
    }
    let path = find(agent);
    let version = path.as_deref().and_then(version);
    AgentInfo {
        agent,
        path: path.map(|p| p.to_string_lossy().to_string()),
        version,
        models: None,
        problem: None,
    }
}

/// The vendor login checks used by start readiness; each command has the shared five-second deadline. With them, the
/// account of a Claude login that reads signed out but still names one: it was lost, not never made.
pub fn read_local_logins() -> Answer<(Vec<nexus_client::local_agents::LocalAgent>, Option<String>)> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    runtime().spawn(async move {
        let result = tokio::task::spawn_blocking(|| {
            let vars = agent_env();
            let env = |name: &str| vars.get(name).cloned();
            let agents = nexus_client::local_agents::detect_all(&env);
            let signed_out = agents.iter().any(|local| {
                local.agent == SessionAgent::Claude.as_str()
                    && local.login == Some(nexus_client::local_agents::Login::SignedOut)
            });
            let lost = signed_out
                .then(|| nexus_workbench::plans::claude_state_file(&env))
                .flatten()
                .and_then(|state| std::fs::read_to_string(state).ok())
                .and_then(|text| serde_json::from_str(&text).ok())
                .and_then(|state| nexus_workbench::plans::claude_last_email(&state));
            (agents, lost)
        })
        .await
        .map_err(|error| error.to_string());
        let _ = sender.send(result);
    });
    receiver
}

/// Read one server's agents without borrowing the local computer's installation state.
pub fn read_remote_agents(
    machine: String,
    machines: Arc<crate::machines::Machines>,
) -> futures::channel::oneshot::Receiver<Result<Vec<AgentInfo>, String>> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    runtime().spawn(async move {
        let result = async {
            let result = tokio::time::timeout(
                Duration::from_secs(25),
                machines.call(&machine, "wb_agent_models", serde_json::json!({})),
            )
            .await
            .map_err(|_| "The server's agent check timed out.".to_string())?
            .map_err(|_| "The server's agent check was lost.".to_string())??;
            serde_json::from_value(result)
                .map_err(|error| format!("The server's agent list could not be read: {error}"))
        }
        .await;
        let _ = sender.send(result);
    });
    receiver
}

/// Install the native Nexus agent through the shared checksum and handshake installer.
/// The caller starts this only after the person chooses Install.
pub fn install_nexus(gateway: String) -> futures::channel::oneshot::Receiver<Result<AgentInfo, String>> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    runtime().spawn(async move {
        let result = async {
            nexus_client::install_tls();
            let http = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::limited(5))
                .timeout(Duration::from_secs(10 * 60))
                .build()
                .map_err(|error| error.to_string())?;
            let fetch: nexus_runtime::Fetch = Arc::new(move |url| {
                let http = http.clone();
                Box::pin(async move {
                    let response = http.get(url).send().await.map_err(|error| error.to_string())?;
                    let status = response.status().as_u16();
                    let body = response.bytes().await.map_err(|error| error.to_string())?;
                    Ok((status, body.to_vec()))
                })
            });
            let installed = nexus_runtime::install_from(&gateway, agent_env(), fetch).await?;
            Ok(nexus_runtime::info(&installed))
        }
        .await;
        let _ = sender.send(result);
    });
    receiver
}

/// Update an installed agent with its vendor's own command (`Install.update`), in the agents' environment. The caller
/// starts this only after the person presses Update, and reads the version again after it ends.
pub fn update_agent(
    agent: nexus_client::agent_entries::Agent,
) -> futures::channel::oneshot::Receiver<Result<(), String>> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    let command = agent.entry().install.update;
    let env = agent_env();
    runtime().spawn(async move {
        let _ = sender.send(run_update(command, &env).await);
    });
    receiver
}

/// Run `command` with `sh -c` and only `env`. Stops it after ten minutes, as the Nexus installer does.
async fn run_update(command: &str, env: &HashMap<String, String>) -> Result<(), String> {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(Duration::from_secs(10 * 60), output).await {
        Err(_) => return Err(format!("`{command}` did not finish in ten minutes.")),
        Ok(Err(error)) => return Err(format!("`{command}` could not start: {error}")),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(());
    }
    // The last line the command printed usually says why: npm and the installers end with their error.
    let said = [&output.stderr, &output.stdout]
        .into_iter()
        .filter_map(|bytes| {
            String::from_utf8_lossy(bytes)
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .map(str::to_owned)
        })
        .next();
    Err(match said {
        Some(line) => format!("`{command}` failed: {}", line.trim()),
        None => format!("`{command}` failed ({}).", output.status),
    })
}

#[cfg(test)]
thread_local! {
    static TEST_AGENT_CATALOG: std::cell::RefCell<Option<Vec<AgentInfo>>> = const { std::cell::RefCell::new(None) };
}

/// A thread-local catalog for deterministic UI fixtures; dropping it restores the previous catalog.
#[cfg(test)]
pub(crate) struct AgentCatalogFixture(Option<Vec<AgentInfo>>);

#[cfg(test)]
impl Drop for AgentCatalogFixture {
    fn drop(&mut self) {
        TEST_AGENT_CATALOG.with(|catalog| *catalog.borrow_mut() = self.0.take());
    }
}

#[cfg(test)]
pub(crate) fn fixture_agents(agents: Vec<AgentInfo>) -> AgentCatalogFixture {
    AgentCatalogFixture(TEST_AGENT_CATALOG.with(|catalog| catalog.replace(Some(agents))))
}

#[cfg(test)]
pub(crate) fn agent_catalog_is_fixture() -> bool {
    TEST_AGENT_CATALOG.with(|catalog| catalog.borrow().is_some())
}

/// A runner over the store in `data` for a workspace test: long ticks, default settings, no outside socket, and a
/// Gateway on port 9, where nothing listens.
#[cfg(test)]
pub(crate) fn test_runner(data: PathBuf, scope: &str) -> (CodeRunner, UnboundedReceiver<CodeEvent>) {
    let hour = Duration::from_secs(3600);
    let loops = Loops {
        local_providers_opt_in: LocalProvidersOptIn::Disabled,
        // No sign-in: a workspace test never reads the keychain.
        client: Arc::new(|| None),
        tick: hour,
        sweep_first: hour,
        sweep_every: hour,
        settings: Arc::new(Settings::default),
        secret_files: data.join("secret-files"),
        outside_socket: None,
        keep_running: Arc::new(|| false),
        local_cache: true,
        browser: Arc::new(crate::side::browser::agent::post),
        pull_watch: false,
    };
    CodeRunner::start_in(data, "http://127.0.0.1:9", scope, loops)
        .expect("the store opens")
        .expect("free, so taken")
}

/// The agents and their models, read without the store: for the model chips while the Nexus app runs the sessions
/// and this build only reads them. Discovery asks each agent for its catalog and sends no prompt.
pub fn read_agents() -> futures::channel::oneshot::Receiver<Vec<AgentInfo>> {
    let (answer, receiver) = futures::channel::oneshot::channel();
    #[cfg(test)]
    if let Some(agents) = TEST_AGENT_CATALOG.with(|catalog| catalog.borrow().clone()) {
        let _ = answer.send(agents);
        return receiver;
    }
    runtime().spawn(async move {
        let mut agents = match tokio::task::spawn_blocking(|| SessionAgent::ALL.map(agent_info)).await {
            Ok(agents) => agents,
            Err(_) => return,
        };
        let path = agent_env().remove("PATH").unwrap_or_default();
        let models = futures::future::join_all(
            agents
                .iter()
                .map(|agent| nexus_workbench::agent_models::discover(agent, &path)),
        )
        .await;
        for (agent, models) in agents.iter_mut().zip(models) {
            agent.models = models;
        }
        let _ = answer.send(agents.to_vec());
    });
    receiver
}

// What follows is nexus-host's `gateway.rs`, as it is: the same agent lookup, model choice and prices.

/// The shared hook stays scoped to eligible sessions and reaches Codex before its shell policy is prepared.
fn commit_environment(mut env: HashMap<String, String>, eligible: bool) -> HashMap<String, String> {
    if eligible {
        commit_trailer::mark(&mut env, Surface::Desktop);
    }
    env
}

/// The environment agents start with: this app's, with a `PATH` that also has where agents are usually installed
/// (`~/.local/bin`, npm's global folder; computed once).
#[cfg(not(test))]
pub fn agent_env() -> HashMap<String, String> {
    static PATH: OnceLock<String> = OnceLock::new();
    let mut env: HashMap<String, String> = std::env::vars().collect();
    let home = Some(home()).filter(|home| !home.as_os_str().is_empty());
    let path =
        PATH.get_or_init(|| process::agent_path(std::env::var("PATH").ok().as_deref(), home.as_deref(), &shell()));
    env.insert("PATH".into(), path.clone());
    env
}

/// In a test, the stand-in agents first on a `PATH` of the system's folders, and a home folder under the test root
/// (`test_agents.rs`): no test starts the person's own `claude` or `codex`, or writes under their home folder.
#[cfg(test)]
pub fn agent_env() -> HashMap<String, String> {
    test_agents::env()
}

/// The shell for the agents' `PATH` and the workbench: the person's login shell, and `/bin/sh` in this crate's tests,
/// so a test never reads `$SHELL` or starts the person's shell and its startup files (a zsh profile can start tmux).
pub(crate) fn shell() -> Shell {
    if cfg!(test) { Shell::sh() } else { Shell::Login }
}

/// The agent's profile, whose settings extra accounts share, resolved with the launch environment.
fn agent_home(agent: SessionAgent, env: &HashMap<String, String>) -> PathBuf {
    accounts::profile_dir(agent, &home(), &|name| env.get(name).cloned())
}

pub(crate) fn find(agent: SessionAgent) -> Option<PathBuf> {
    let env = agent_env();
    let cwd = std::env::current_dir().unwrap_or_else(|_| home());
    nexus_workbench::agent_launch::find(agent, &env, &cwd)
}

/// `<path> --version`, once per path, giving up after five seconds.
fn version(path: &Path) -> Option<String> {
    static VERSIONS: OnceLock<std::sync::Mutex<HashMap<PathBuf, Option<String>>>> = OnceLock::new();
    let versions = VERSIONS.get_or_init(Default::default);
    if let Some(known) = versions.lock().ok().and_then(|v| v.get(path).cloned()) {
        return known;
    }
    let found = version_of(path);
    if let Ok(mut versions) = versions.lock() {
        versions.insert(path.to_path_buf(), found.clone());
    }
    found
}

fn version_of(path: &Path) -> Option<String> {
    let mut version = process::command(path);
    version
        .arg("--version")
        .env("PATH", agent_env().get("PATH").cloned().unwrap_or_default());
    let (status, stdout) = process::stdout_within(version, Duration::from_secs(5))?;
    // Only the first line: a leftover process may print into the output after the command exited.
    let line = stdout.lines().map(str::trim).find(|line| !line.is_empty())?;
    status.success().then(|| line.to_string())
}

/// Gateway refused an end for good (an unknown session, say); anything else is worth asking again.
fn end_refused(error: &nexus_client::Error) -> bool {
    matches!(error, nexus_client::Error::Core(core)
        if (400..500).contains(&core.status) && !matches!(core.status, 401 | 403 | 408 | 429))
}

use nexus_workbench::agent_launch::{Catalog, load_catalog, model_key};

/// One call at these prices per million tokens, cache reads at a tenth of input and writes at 1.25 times.
fn list_price(input: f64, output: f64, usage: &Usage) -> f64 {
    let input_tokens =
        usage.input_tokens as f64 + usage.cache_read_tokens as f64 * 0.1 + usage.cache_write_tokens as f64 * 1.25;
    (input_tokens * input + usage.output_tokens as f64 * output) / 1_000_000.0
}

#[cfg(test)]
pub(crate) mod test_agents;

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_client::agent_entries::Agent as ClientAgent;

    mod sign_in;

    /// What the background service takes as Settings' plan accounts: each agent's chosen account, and its
    /// rotation, with the agent's default when Settings never set it.
    #[test]
    fn account_choices_carry_each_agents_account_and_rotation() {
        use nexus_workbench::accounts::Choice;
        let settings = nexus_desktop_core::settings::Settings {
            claude_account: Some("a_1".into()),
            rotate_codex_accounts: Some(true),
            ..Default::default()
        };
        assert_eq!(
            account_choices(&settings),
            vec![
                Choice {
                    agent: SessionAgent::Claude,
                    account: Some("a_1".into()),
                    rotate: nexus_workbench::rotate::auto_rotate_by_default(SessionAgent::Claude),
                },
                Choice {
                    agent: SessionAgent::Codex,
                    account: None,
                    rotate: true,
                },
            ]
        );
    }

    #[test]
    fn shutdown_revokes_a_held_attached_command_before_any_move() {
        let data = std::env::temp_dir().join(format!("nexus-gpui-command-shutdown-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&data).expect("fixture data directory");
        let (runner, _events) = test_runner(data.clone(), "scope");
        let held = Arc::new(Mutex::new(None));
        let pending = held.clone();
        runner.fixture_terminal(move |_, _, _, valid| {
            let (answer, receive) = futures::channel::oneshot::channel();
            *pending.lock().unwrap() = Some((valid, answer));
            receive
        });
        let captured = runner.source_guard();
        let answer = runner.terminal_guarded(
            "session".into(),
            "request_user".into(),
            json!({"command":"echo once"}),
            captured.clone(),
        );
        let (valid, reply) = held.lock().unwrap().take().expect("held command admission");
        assert!(valid());
        // No detached owner or presentation has ever existed. The original runner
        // itself must revoke the queued request while it remains strongly held.
        runner.shutdown();
        assert!(!valid());
        assert!(!captured());
        let mut mutations = 0;
        let result = if valid() {
            mutations += 1;
            Ok(json!({"id":"must-not-start"}))
        } else {
            Err("owner retired at final dispatch".into())
        };
        reply.send(result).unwrap();
        assert!(futures::executor::block_on(answer).unwrap().is_err());
        assert_eq!(mutations, 0);
        let continuation = runner.terminal_guarded("session".into(), "execute".into(), json!({"id":"held"}), captured);
        assert!(futures::executor::block_on(continuation).unwrap().is_err());
        assert!(held.lock().unwrap().is_none(), "no second-step dispatch after quit");
        drop(runner);
        let _ = std::fs::remove_dir_all(data);
    }

    /// No test can reach the person's own agents: the runner's lookup (`find` on `agent_env`, then `--version`) finds
    /// the stand-ins under the test root, and no other folder on the test `PATH` holds an agent to fall back to.
    #[cfg(unix)]
    #[test]
    fn a_test_finds_only_the_stand_in_agents_and_a_home_under_the_test_root() {
        let root = crate::paths::test_root();
        for agent in [Agent::Claude, Agent::Codex] {
            let info = agent_info(agent);
            let path = PathBuf::from(info.path.unwrap_or_else(|| panic!("{agent:?} is found")));
            assert!(
                path.starts_with(&root),
                "{agent:?} resolves outside the test root {}: {}",
                root.display(),
                path.display()
            );
            assert_eq!(
                info.version.as_deref(),
                Some(test_agents::VERSION),
                "{agent:?} at {} is not the stand-in",
                path.display()
            );
        }
        let env = agent_env();
        for dir in env["PATH"].split(':') {
            for name in test_agents::STAND_INS.into_iter().chain(["nexus"]) {
                let program = Path::new(dir).join(name);
                assert!(
                    !program.exists() || program.starts_with(&root),
                    "the test PATH reaches an agent outside the test root: {}",
                    program.display()
                );
            }
        }
        for (name, folder) in [("HOME", PathBuf::from(&env["HOME"])), ("home()", home())] {
            assert!(
                folder.starts_with(&root),
                "{name} is outside the test root {}: {}",
                root.display(),
                folder.display()
            );
        }
        assert!(Path::new(&env["HISTFILE"]).starts_with(&root), "{}", env["HISTFILE"]);
    }

    #[test]
    fn worktree_bases_prefer_remote_main_then_local_main_then_the_repository_default() {
        for (names, expected) in [
            (vec!["feature", "main", "origin/main"], Some("origin/main")),
            (vec!["feature", "main"], Some("main")),
            (vec!["origin/trunk", "feature"], Some("origin/trunk")),
            (vec![], None),
        ] {
            let bases: Vec<String> = names.into_iter().map(String::from).collect();
            assert_eq!(default_worktree_base(&bases).map(String::as_str), expected);
        }
    }

    #[test]
    fn only_eligible_sessions_add_the_desktop_hook_before_codex_shell_configuration() {
        let original = HashMap::from([("GIT_CONFIG_PARAMETERS".into(), "'user.name=Existing Person'".into())]);
        assert_eq!(commit_environment(original.clone(), false), original);
        for route in [Route::Gateway, Route::Subscription] {
            let env = commit_environment(original.clone(), true);
            let hook = env[commit_trailer::VARIABLE].clone();
            assert!(hook.starts_with("'user.name=Existing Person'"));
            assert!(hook.contains(Surface::Desktop.trailer()));
            let (args, launched) = agents::agent_launch(
                ClientAgent::Codex,
                LaunchOptions {
                    route,
                    core_url: "https://gateway.example",
                    api_key: "nx_test",
                    session_id: "session_test",
                    team: None,
                    model: None,
                    model_catalog: None,
                    web_search: false,
                    args: vec!["app-server".into()],
                    env,
                },
            );
            let setting = args
                .iter()
                .find_map(|arg| arg.strip_prefix("shell_environment_policy.set.GIT_CONFIG_PARAMETERS="));
            let forwarded: String =
                serde_json::from_str(setting.expect("Codex shell override")).expect("quoted Git hook");
            assert_eq!(forwarded, hook);
            assert_eq!(launched[commit_trailer::VARIABLE], hook);
        }
    }

    /// "Keep agents running when Nexus quits": hand-over and take-back with a stand-in service.
    #[cfg(unix)]
    mod service;

    #[test]
    fn a_held_store_reads_as_busy_and_a_released_one_is_taken() {
        let data = std::env::temp_dir().join(format!("nexus-gpui-owner-{}", std::process::id()));
        // Another app's hold, as it takes it at launch.
        let other = StoreOwner::take(&data).expect("the other app owns the store");
        assert!(matches!(take_store(&data), Ok(None)));
        drop(other);
        let ours = take_store(&data).expect("the lock opens").expect("released, so taken");
        drop(ours);
        let _ = std::fs::remove_dir_all(data);
    }

    /// A store in a fresh folder with one project and a schedule that missed its time an hour ago and skips missed
    /// runs: the first tick of whoever runs the schedules records the skip, and starts no agent.
    fn store_with_a_missed_schedule(name: &str, scope: &str) -> PathBuf {
        use nexus_workbench::schedules::{Schedule, ScheduleInput, When};
        let data = std::env::temp_dir().join(format!("nexus-gpui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&data).expect("a temporary folder");
        let store = Store::open(&data.join("workbench.sqlite3")).expect("the store opens");
        let now = nexus_workbench::now_ms();
        let project = store.add_project(&data, now).expect("a project");
        let input = ScheduleInput {
            project_id: project.id,
            title: None,
            prompt: "Check the build.".into(),
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: PermissionMode::Plan,
            effort: None,
            via: Via::Nexus,
            worktree: false,
            when: When::Interval { minutes: 60 },
            enabled: true,
            max_runs_per_day: None,
            catch_up: false,
        };
        // Saved two hours ago: its next run was an hour ago.
        let schedule = Schedule::build(input, "s_missed".into(), None, None, now - 2 * 3_600_000, &chrono::Utc)
            .expect("a valid schedule");
        store.save_schedule(scope, &schedule).expect("saved");
        data
    }

    /// The schedule's runs, read from the store's file: only while no runner in this process holds the store.
    fn runs(data: &Path, scope: &str) -> usize {
        let store = Store::open(&data.join("workbench.sqlite3")).expect("the store opens");
        store.schedules(scope).expect("schedules")[0].runs.len()
    }

    /// The schedule's runs, asked of the runner, as Settings asks while this build owns the store. A second
    /// `Store::open` here would compete with the runner's writes for the lock, and under load fail as "database is
    /// locked".
    fn runs_held(runner: &CodeRunner) -> usize {
        let schedules = futures::executor::block_on(runner.schedules().list())
            .expect("the runner answers")
            .expect("schedules");
        schedules[0].schedule.runs.len()
    }

    /// Wait up to `limit` for `done`.
    fn wait_for(limit: Duration, done: impl Fn() -> bool) -> bool {
        let until = std::time::Instant::now() + limit;
        while std::time::Instant::now() < until {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        done()
    }

    // Nothing listens on port 9: no Gateway call leaves this computer.
    const GATEWAY: &str = "http://127.0.0.1:9";

    /// The loops every `tick`, a sweep at the first tick and then each tick, on `settings`. The sockets' files go in
    /// a temporary folder of their own.
    fn loops(tick: Duration, settings: Settings) -> Loops {
        let dir = std::env::temp_dir().join(format!("ngs-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        Loops {
            local_providers_opt_in: LocalProvidersOptIn::Disabled,
            client: Arc::new(|| None),
            tick,
            sweep_first: tick,
            sweep_every: tick,
            settings: Arc::new(move || settings.clone()),
            secret_files: dir.join("secret-files"),
            outside_socket: Some(dir.join("run").join("sessions.sock")),
            keep_running: Arc::new(|| false),
            local_cache: true,
            browser: Arc::new(crate::side::browser::agent::post),
            pull_watch: false,
        }
    }

    /// The loops every `tick` on the default settings: no sweep rule on.
    fn quick(tick: Duration) -> Loops {
        loops(tick, Settings::default())
    }

    #[test]
    fn the_schedules_run_only_while_this_build_owns_the_store() {
        let scope = format!("{GATEWAY}|org-gated");
        let data = store_with_a_missed_schedule("gated", &scope);
        let tick = Duration::from_millis(50);
        // Another app holds the store: no runner, so no loop.
        let other = StoreOwner::take(&data).expect("the other app owns the store");
        assert!(matches!(
            CodeRunner::start_in(data.clone(), GATEWAY, &scope, quick(tick)),
            Ok(None)
        ));
        std::thread::sleep(tick * 6);
        assert_eq!(
            runs(&data, &scope),
            0,
            "the schedule did not run here while the other app owns the store"
        );
        // It quits: this build takes the store, and its loop runs the schedule at the first tick.
        drop(other);
        let (runner, _events) = CodeRunner::start_in(data.clone(), GATEWAY, &scope, quick(tick))
            .expect("the store opens")
            .expect("released, so taken");
        assert!(
            wait_for(Duration::from_secs(10), || runs_held(&runner) == 1),
            "the loop ran the schedule"
        );
        drop(runner);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn session_sync_runs_with_the_store_and_stops_with_the_runner() {
        let scope = format!("{GATEWAY}|org-sync");
        let data = store_with_a_missed_schedule("sync", &scope);
        let (runner, _events) = CodeRunner::start_in(data.clone(), GATEWAY, &scope, quick(Duration::from_secs(60)))
            .expect("the store opens")
            .expect("free, so taken");
        let syncs = runner.syncs.clone().expect("this build runs session sync");
        assert!(
            !syncs.is_finished(),
            "the sync loop runs while this build owns the store"
        );
        // Sync has no switch, but the test's sign-in source gives no client (`loops`): no backend, so no Gateway call
        // and no device key read. Tests keep the keychain in memory too (`keychain::vault`).
        assert!(runner.host.session_sync().is_none());
        assert!(
            runner.host.sign_in().is_none(),
            "signed out: no device key goes to the service"
        );
        drop(runner);
        assert!(
            wait_for(Duration::from_secs(5), || syncs.is_finished()),
            "the sync loop ended with the lock"
        );
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn the_loop_stops_with_the_runner() {
        let scope = format!("{GATEWAY}|org-stops");
        let data = store_with_a_missed_schedule("stops", &scope);
        let tick = Duration::from_millis(50);
        let (runner, _events) = CodeRunner::start_in(data.clone(), GATEWAY, &scope, quick(tick))
            .expect("the store opens")
            .expect("free, so taken");
        let upkeep = runner.upkeep.clone().expect("this build runs the upkeep");
        drop(runner);
        assert!(
            wait_for(Duration::from_secs(5), || upkeep.is_finished()),
            "the loop ended with the lock"
        );
        let _ = std::fs::remove_dir_all(data);
    }

    /// Every file under `dir`, as paths inside it.
    fn files_under(dir: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut folders = vec![dir.to_path_buf()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    folders.push(path);
                } else if let Ok(inside) = path.strip_prefix(dir) {
                    found.push(inside.to_string_lossy().into_owned());
                }
            }
        }
        found.sort();
        found
    }

    #[test]
    fn with_the_local_cache_off_the_sessions_stay_in_memory() {
        let scope = format!("{GATEWAY}|org-no-cache");
        let root = std::env::temp_dir().join(format!("nexus-gpui-no-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (data, repo) = (root.join("data"), root.join("repo"));
        std::fs::create_dir_all(&repo).expect("a temporary folder");
        git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        let loops = Loops {
            local_cache: false,
            ..quick(Duration::from_secs(600))
        };
        let (runner, _events) = CodeRunner::start_in(data.clone(), GATEWAY, &scope, loops)
            .expect("the store opens")
            .expect("free, so taken");
        runner.add_project(repo.clone(), None);
        // The sidebar's reader finds the project in the runner's memory.
        let projects = || {
            crate::code::CodeStore::open_in(&data, scope.clone())
                .ok()
                .flatten()
                .and_then(|store| store.projects().ok())
                .map_or(0, |projects| projects.len())
        };
        assert!(
            wait_for(Duration::from_secs(10), || projects() == 1),
            "the reader sees the project"
        );
        let written = files_under(&data);
        assert!(
            written
                .iter()
                .all(|file| !file.starts_with("workbench.sqlite3") && file != "outside-access.json"),
            "no event log and no grants on disk: {written:?}"
        );
        drop(runner);
        // The project left with the runner, and no reader opens a store on disk.
        assert!(
            crate::local_store::open_in(&data).expect("the folder reads").is_none(),
            "nothing to read once the runner is gone"
        );
        let written = files_under(&data);
        assert!(
            written.iter().all(|file| !file.starts_with("workbench.sqlite3")),
            "{written:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@nexus.test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A store in a fresh folder with a project: a repository whose `main` is pushed to a bare remote, so a worktree
    /// on it loses nothing. Answers the folder and the project's id.
    fn store_with_a_repository(name: &str) -> (PathBuf, String) {
        let data = std::env::temp_dir().join(format!("nexus-gpui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let (repo, remote) = (data.join("repo"), data.join("remote.git"));
        std::fs::create_dir_all(&repo).expect("a temporary folder");
        git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        git(&repo, &["commit", "--quiet", "--allow-empty", "-m", "one"]);
        git(&data, &["init", "--quiet", "--bare", &remote.to_string_lossy()]);
        git(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
        git(&repo, &["push", "--quiet", "origin", "main"]);
        let store = Store::open(&data.join("workbench.sqlite3")).expect("the store opens");
        let project = store.add_project(&repo, nexus_workbench::now_ms()).expect("a project");
        (data, project.id)
    }

    /// A stopped session in `scope` with its own clean worktree of `main`, last active at `updated_at`. Answers the
    /// worktree's folder.
    fn session_with_worktree(
        data: &Path,
        scope: &str,
        project: &str,
        id: &str,
        archived: bool,
        updated_at: i64,
    ) -> PathBuf {
        let dir = data.join("worktrees").join(id);
        let branch = format!("nexus/{id}");
        git(
            &data.join("repo"),
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                &branch,
                &dir.to_string_lossy(),
                "main",
            ],
        );
        let summary = serde_json::from_value(json!({
            "id": id, "projectId": project, "title": id, "agent": "claude", "model": null,
            "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0, "nexusSessions": [],
            "contextTokens": null, "contextWindow": null, "createdAt": updated_at, "updatedAt": updated_at,
            "archived": archived,
            "worktree": { "path": dir.to_string_lossy(), "branch": branch, "base": "main" },
        }))
        .expect("a session");
        let record = nexus_workbench::store::ThreadRecord {
            summary,
            native_id: format!("native-{id}"),
            native_started: false,
            scope: scope.to_string(),
            fork_from: None,
            seed: None,
            automatic_title: false,
            delivered_seq: 0,
            halt: None,
        };
        let store = Store::open(&data.join("workbench.sqlite3")).expect("the store opens");
        store.insert_thread(&record).expect("saved");
        dir
    }

    /// The runner's sessions in its scope, as outside access asks for them (`Engine::sessions`) while this build owns
    /// the store. A second `Store::open` here would compete with the runner's writes for the lock, and under load fail
    /// as "database is locked".
    fn sessions_held(runner: &CodeRunner) -> Vec<nexus_workbench::event::ThreadSummary> {
        answered(runner.ask(|api| async move { api.sessions().await }))
            .expect("sessions")
            .0
    }

    /// Whether the runner marks the session's worktree as removed.
    fn removed_held(runner: &CodeRunner, id: &str) -> bool {
        sessions_held(runner)
            .into_iter()
            .find(|summary| summary.id == id)
            .and_then(|summary| summary.worktree)
            .is_some_and(|worktree| worktree.removed)
    }

    /// Whether the store's file marks the session's worktree as removed: only while no runner in this process holds
    /// the store.
    fn marked_removed(data: &Path, id: &str) -> bool {
        let store = Store::open(&data.join("workbench.sqlite3")).expect("the store opens");
        store
            .thread(id)
            .ok()
            .flatten()
            .and_then(|record| record.summary.worktree)
            .is_some_and(|worktree| worktree.removed)
    }

    /// Settings with these cleanup rules and new worktrees in `data`, never in the home folder.
    fn settings_with(data: &Path, rules: cleanup::Rules) -> Settings {
        let mut settings = Settings::default();
        settings.set_cleanup_rules(rules).expect("valid rules");
        settings
            .set_worktree_root(
                nexus_desktop_core::settings::WorktreeRoot::Worktrees,
                Some(data.join("worktrees").to_string_lossy().into_owned()),
            )
            .expect("a full path");
        settings
    }

    #[test]
    fn the_worktree_sweep_runs_only_while_this_build_owns_the_store() {
        let scope = format!("{GATEWAY}|org-sweep-gated");
        let (data, project) = store_with_a_repository("sweep-gated");
        let now = nexus_workbench::now_ms();
        let archived = session_with_worktree(&data, &scope, &project, "t_archived", true, now);
        let in_use = session_with_worktree(&data, &scope, &project, "t_in_use", false, now);
        let rules = cleanup::Rules {
            archived: true,
            ..cleanup::Rules::default()
        };
        let tick = Duration::from_millis(50);
        // Another app holds the store: it sweeps, not this build.
        let other = StoreOwner::take(&data).expect("the other app owns the store");
        assert!(matches!(
            CodeRunner::start_in(
                data.clone(),
                GATEWAY,
                &scope,
                loops(tick, settings_with(&data, rules.clone()))
            ),
            Ok(None)
        ));
        std::thread::sleep(tick * 6);
        assert!(archived.exists(), "no sweep here while the other app owns the store");
        // It quits: this build takes the store, and its sweep removes the archived session's worktree.
        drop(other);
        let (runner, _events) =
            CodeRunner::start_in(data.clone(), GATEWAY, &scope, loops(tick, settings_with(&data, rules)))
                .expect("the store opens")
                .expect("released, so taken");
        assert!(
            wait_for(Duration::from_secs(10), || !archived.exists()
                && removed_held(&runner, "t_archived")),
            "the sweep removed the archived session's worktree"
        );
        assert!(in_use.exists() && !removed_held(&runner, "t_in_use"));
        drop(runner);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn the_worktree_sweep_follows_the_saved_rules() {
        let scope = format!("{GATEWAY}|org-sweep-rules");
        let (data, project) = store_with_a_repository("sweep-rules");
        let now = nexus_workbench::now_ms();
        let idle = session_with_worktree(&data, &scope, &project, "t_idle", false, now - 30 * DAY_MS);
        let idle_dirty = session_with_worktree(&data, &scope, &project, "t_dirty", false, now - 30 * DAY_MS);
        let recent = session_with_worktree(&data, &scope, &project, "t_recent", false, now - 6 * DAY_MS);
        // The archived rule is off: an archived session in use this week stays.
        let archived = session_with_worktree(&data, &scope, &project, "t_archived", true, now);
        std::fs::write(idle_dirty.join("wip.txt"), "wip\n").expect("a change");
        let rules = cleanup::Rules {
            idle_days: Some(7),
            ..cleanup::Rules::default()
        };
        let tick = Duration::from_millis(50);
        let (runner, _events) =
            CodeRunner::start_in(data.clone(), GATEWAY, &scope, loops(tick, settings_with(&data, rules)))
                .expect("the store opens")
                .expect("free, so taken");
        assert!(
            wait_for(Duration::from_secs(10), || !idle.exists()
                && removed_held(&runner, "t_idle")),
            "idle for 30 days with a 7-day rule: removed"
        );
        // A few more sweeps: what stays, stays.
        std::thread::sleep(tick * 6);
        assert!(idle_dirty.exists(), "uncommitted changes keep the worktree");
        assert!(recent.exists(), "idle for 6 days with a 7-day rule: kept");
        assert!(archived.exists(), "the archived rule is off");
        drop(runner);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn no_rule_on_removes_nothing() {
        let scope = format!("{GATEWAY}|org-sweep-off");
        let (data, project) = store_with_a_repository("sweep-off");
        let old = nexus_workbench::now_ms() - 365 * DAY_MS;
        let archived = session_with_worktree(&data, &scope, &project, "t_old", true, old);
        let tick = Duration::from_millis(50);
        let (runner, _events) = CodeRunner::start_in(
            data.clone(),
            GATEWAY,
            &scope,
            loops(tick, settings_with(&data, cleanup::Rules::default())),
        )
        .expect("the store opens")
        .expect("free, so taken");
        std::thread::sleep(tick * 6);
        assert!(archived.exists() && !removed_held(&runner, "t_old"));
        drop(runner);
        let _ = std::fs::remove_dir_all(data);
    }

    /// A runner command's answer, waited for.
    fn answered<T>(answer: Answer<T>) -> Result<T, String> {
        futures::executor::block_on(answer).expect("the runner answered")
    }

    #[test]
    fn settings_change_a_project_and_clean_up_now() {
        let scope = format!("{GATEWAY}|org-projects");
        let (data, project) = store_with_a_repository("projects");
        let archived = session_with_worktree(&data, &scope, &project, "t_archived", true, nexus_workbench::now_ms());
        // An hour's tick: no sweep of its own while the test runs.
        let (rules, off) = (
            cleanup::Rules {
                archived: true,
                ..cleanup::Rules::default()
            },
            Duration::from_secs(3600),
        );
        let (runner, _events) =
            CodeRunner::start_in(data.clone(), GATEWAY, &scope, loops(off, settings_with(&data, rules)))
                .expect("the store opens")
                .expect("free, so taken");
        let projects = runner.projects();

        assert_eq!(
            answered(projects.set_setup(project.clone(), "  npm ci \n".into())),
            Ok(Some("npm ci".into()))
        );
        assert_eq!(answered(projects.set_setup(project.clone(), " ".into())), Ok(None));
        let script = |id: &str, name: &str| nexus_workbench::scripts::ProjectScript {
            id: id.into(),
            name: name.into(),
            command: "npm test".into(),
            shortcut: Some("Mod+Alt+KeyT".into()),
        };
        let saved = answered(projects.set_scripts(project.clone(), vec![script("c-1", " Test ")])).expect("saved");
        assert_eq!(saved[0].name, "Test");
        // The workbench's rules refuse two commands with one name, and keep what was saved.
        assert!(answered(projects.set_scripts(project.clone(), vec![script("c-1", "A"), script("c-2", "a")])).is_err());
        answered(projects.update(project.clone(), Some(true), None)).expect("pinned");

        let defaults = nexus_workbench::project_defaults::ProjectDefaults {
            agent: Some(SessionAgent::Codex),
            model: Some("gpt-5".into()),
            mode: Some(PermissionMode::Plan),
            workspace: None,
        };
        assert_eq!(
            answered(projects.set_defaults(project.clone(), defaults.clone())),
            Ok(defaults.clone())
        );
        assert_eq!(answered(projects.defaults(project.clone())), Ok(defaults.clone()));

        assert_eq!(projects.last_cleanup(), None);
        let last = answered(projects.clean_up_now()).expect("swept");
        assert_eq!(last.removed.len(), 1);
        assert!(!archived.exists() && removed_held(&runner, "t_archived"));
        assert_eq!(projects.last_cleanup(), Some(last));
        drop(runner);

        // What the store's file kept, read once the runner is gone: a second `Store::open` while it holds the store
        // would compete with its writes for the lock.
        let store = Store::open(&data.join("workbench.sqlite3")).expect("the store opens");
        let stored = store
            .projects()
            .expect("projects")
            .into_iter()
            .find(|row| row.id == project)
            .expect("the project");
        assert!(stored.pinned);
        assert_eq!(
            store
                .project_scripts(&project)
                .expect("read")
                .map(|json| json.contains("Test")),
            Some(true)
        );
        let kept = store.project_defaults(&project).expect("read");
        assert_eq!(nexus_workbench::project_defaults::read(kept), defaults);
        assert!(marked_removed(&data, "t_archived"));
        drop(store);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn clean_up_now_needs_a_rule() {
        let scope = format!("{GATEWAY}|org-projects-off");
        let (data, _) = store_with_a_repository("projects-off");
        let (runner, _events) = CodeRunner::start_in(
            data.clone(),
            GATEWAY,
            &scope,
            loops(
                Duration::from_secs(3600),
                settings_with(&data, cleanup::Rules::default()),
            ),
        )
        .expect("the store opens")
        .expect("free, so taken");
        assert!(answered(runner.projects().clean_up_now()).is_err());
        assert_eq!(runner.projects().last_cleanup(), None);
        drop(runner);
        let _ = std::fs::remove_dir_all(data);
    }

    /// The branches of the runner's sessions that have a worktree, asked of the runner while it holds the store.
    fn branches(runner: &CodeRunner) -> Vec<String> {
        sessions_held(runner)
            .into_iter()
            .filter_map(|summary| summary.worktree.map(|worktree| worktree.branch))
            .collect()
    }

    fn new_session(project: &str, text: &str) -> NewSession {
        NewSession {
            project_id: project.to_string(),
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: "default".into(),
            effort: None,
            via: Via::Nexus,
            provider: None,
            worktree: true,
            review: None,
            base: None,
            text: text.into(),
            attachments: crate::attachments::Draft::default(),
        }
    }

    #[test]
    fn new_sessions_branches_use_the_saved_prefix() {
        let scope = format!("{GATEWAY}|org-prefix");
        let (data, project) = store_with_a_repository("prefix");
        let mut settings = settings_with(&data, cleanup::Rules::default());
        settings.set_branch_prefix(Some("jd/".into())).expect("a valid prefix");
        // The prefix saved when the store is taken.
        let (runner, _events) = CodeRunner::start_in(
            data.clone(),
            GATEWAY,
            &scope,
            loops(Duration::from_secs(60), settings.clone()),
        )
        .expect("the store opens")
        .expect("free, so taken");
        // The agent does not start (no sign-in for this Gateway): the session and its worktree are made first.
        runner.create("draft-0".into(), new_session(&project, "Fix the login"));
        assert!(
            wait_for(Duration::from_secs(10), || branches(&runner)
                .iter()
                .any(|branch| branch.starts_with("jd/"))),
            "the new session's branch starts with the saved prefix: {:?}",
            branches(&runner)
        );
        // Settings saves another prefix here (`Workspace::settings_saved`): the next session uses it.
        settings
            .set_branch_prefix(Some("team/".into()))
            .expect("a valid prefix");
        runner.apply_settings(&settings);
        runner.create("draft-0".into(), new_session(&project, "Fix the logout"));
        assert!(
            wait_for(Duration::from_secs(10), || branches(&runner)
                .iter()
                .any(|branch| branch.starts_with("team/"))),
            "the next session's branch starts with the new prefix: {:?}",
            branches(&runner)
        );
        drop(runner);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn new_sessions_start_from_main_unless_a_base_is_picked() {
        let scope = format!("{GATEWAY}|org-main-base");
        let (data, project) = store_with_a_repository("main-base");
        let repo = data.join("repo");
        // No origin/HEAD: the workbench's default would be the checked-out feature branch.
        git(&repo, &["switch", "--quiet", "-c", "feature"]);
        std::fs::write(repo.join("feature.txt"), "feature only\n").expect("a feature file");
        git(&repo, &["add", "feature.txt"]);
        git(&repo, &["commit", "--quiet", "-m", "feature"]);
        let (runner, _events) = CodeRunner::start_in(
            data.clone(),
            GATEWAY,
            &scope,
            loops(
                Duration::from_secs(3600),
                settings_with(&data, cleanup::Rules::default()),
            ),
        )
        .expect("the store opens")
        .expect("free, so taken");
        runner.create("draft-0".into(), new_session(&project, "Start from main"));
        assert!(wait_for(Duration::from_secs(10), || branches(&runner).len() == 1));
        let first = sessions_held(&runner).pop().unwrap().worktree.unwrap();
        assert_eq!(first.base, "origin/main");
        assert!(!Path::new(&first.path).join("feature.txt").exists());
        assert!(
            repo.join("feature.txt").exists(),
            "the project folder keeps its checkout"
        );

        let mut picked = new_session(&project, "Start from feature");
        picked.base = Some("feature".into());
        runner.create("draft-0".into(), picked);
        assert!(wait_for(Duration::from_secs(10), || branches(&runner).len() == 2));
        let picked = sessions_held(&runner)
            .into_iter()
            .filter_map(|session| session.worktree)
            .find(|worktree| worktree.base == "feature")
            .expect("the picked base is kept");
        assert!(Path::new(&picked.path).join("feature.txt").exists());
        assert_ne!(picked.path, first.path);
        drop(runner);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn a_relayed_browser_call_holds_for_its_person_organization_and_run_only() {
        let grant = nexus_desktop_mcp::app_socket::Grant {
            thread: "t1".into(),
            scope: "gw|acme".into(),
            run: "ns_1".into(),
            sign_in: "me".into(),
        };
        let none = std::collections::HashSet::new();
        assert!(relayed_call_holds(&grant, "gw|acme", Some("me"), &none));
        assert!(
            !relayed_call_holds(&grant, "gw|other", Some("me"), &none),
            "another organization"
        );
        assert!(
            !relayed_call_holds(&grant, "gw|acme", Some("you"), &none),
            "another person"
        );
        assert!(!relayed_call_holds(&grant, "gw|acme", None, &none), "signed out");
        let ended = std::collections::HashSet::from(["ns_1".to_string()]);
        assert!(
            !relayed_call_holds(&grant, "gw|acme", Some("me"), &ended),
            "the run ended"
        );
    }

    #[test]
    fn prices_by_the_agents_names() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Usage::default()
        };
        assert_eq!(list_price(1.0, 5.0, &usage), 6.0);
        assert_eq!(model_key("anthropic/claude-haiku-4.5"), "claude-haiku-4-5");
    }

    /// Update runs the catalog's own command against the agent on `PATH`, and a failure says the line that explains it.
    /// A fake `claude` stands in for the real one: no network.
    #[cfg(unix)]
    #[test]
    fn update_runs_the_vendors_command_and_says_why_it_failed() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().expect("a fake bin folder");
        let ran = bin.path().join("ran");
        let fake = bin.path().join("claude");
        let write_fake = |script: &str| {
            std::fs::write(&fake, script).expect("the fake agent");
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("it runs");
        };
        let env = HashMap::from([("PATH".to_string(), format!("{}:/usr/bin:/bin", bin.path().display()))]);
        let command = nexus_client::agent_entries::Agent::Claude.entry().install.update;

        write_fake(&format!("#!/bin/sh\necho \"$@\" > '{}'\n", ran.display()));
        runtime()
            .block_on(run_update(command, &env))
            .expect("the update succeeds");
        assert_eq!(std::fs::read_to_string(&ran).expect("the fake ran").trim(), "update");

        write_fake("#!/bin/sh\necho 'Checking for updates'\necho 'npm ERR! code EACCES' >&2\nexit 1\n");
        assert_eq!(
            runtime().block_on(run_update(command, &env)),
            Err("`claude update` failed: npm ERR! code EACCES".into())
        );

        write_fake("#!/bin/sh\nexit 3\n");
        let silent = runtime().block_on(run_update(command, &env)).unwrap_err();
        assert!(silent.starts_with("`claude update` failed (exit status: 3"), "{silent}");
    }
}

/// The sockets, driven by fake clients (`socket_tests.rs`).
#[cfg(all(test, unix))]
#[path = "socket_tests.rs"]
mod socket_tests;
