//! Cloud machines in a desktop app: a Linux VM Nexus Gateway starts
//! for the person at a provider (boat.dev in v1), running the same `nexus-host` a server runs. Gateway holds the
//! provider account ([`gateway`]), so the person pastes no provider key, and the same machine shows on every device
//! they sign in on. To the Code screen it is one more machine: the app lists it with `kind: "cloud"` and its
//! [`CloudInfo`] ([`MachineView`]), and reaches its host through [`Cloud::call`]. Nexus GPUI (desktop-gpui/src/cloud.rs)
//! runs this module and gives it a [`CloudEnv`]. This module adds what a server does not have:
//!
//! - **The person's machines come from Gateway** ([`Cloud::sync`]): on start, every 30 seconds, after a sign-in or an
//!   organization switch, and when the window gets focus. A machine Gateway lists that this device does not know is
//!   added (`cloud:machines`), one it no longer lists is forgotten here. Each device has its own SSH key per machine,
//!   and registers it with Gateway (`POST …/access`) before every connect: after a wake, the machine is new.
//! - **The machines are the sign-in's**. Gateway lists them for one sign-in: its Gateway and token, so
//!   another organization is another sign-in. A sign-out, another sign-in, or Gateway refusing this one (401, or 403
//!   on the list) ends them here at once ([`Inner::end`]): the relays close, the previews and tasks stop, the machines
//!   and this device's keys for them are forgotten, and the app is told. Every call checks the sign-in first, and
//!   an answer, an event or a Gateway call for an ended one goes nowhere. A network failure ends nothing.
//! - **Secrets in the keychain** (`nexus_desktop_core::secrets`): the Claude setup token (`cloud-claude`) and the
//!   person's GitHub token (`cloud-github`, from this computer's `gh` or read back after a sign-in on a machine).
//!   Settings keep only when each was saved, a hint or the GitHub login, and the sign-in they belong to. A
//!   sign-out or another sign-in forgets them, hints included ([`Inner::claim`]), so its machines, Settings and
//!   repository list never get them. Each is saved, read and written onto a machine for one sign-in, under its check.
//! - **One machine per session**. [`Cloud::create`] asks Gateway for the machine of a new session
//!   (`t_<request id>`, its title) and saves it at once, then in the background waits for it (through Gateway's queue
//!   when Nexus has no room yet) and starts the relay (`nexus_remote::machine::Remote`) over [`MachineLink`]. Each
//!   connect wakes it only when someone acted ([`Cloud::wake`], a message sent there; never a read): a machine
//!   Gateway put to sleep is found asleep, and its relay is held (`Remote::hold`), not retried. When Gateway brings
//!   one back by itself (a restart on a bigger size), its relay resumes. `cloud:progress` reports each step, `cloud:state` the
//!   provider's state (asked every 30 seconds and after every action).
//! - **Setup on every connect** ([`crate::link::provision`]): the Claude token in `agent.env` (a device with
//!   no token leaves the machine's as it is, unless the person cleared it here), a Playwright
//!   browser in `mcp.json`, [`AGENT_INSTRUCTIONS`] in `~/.claude/CLAUDE.md` and Codex's `~/.codex/AGENTS.md`, git's
//!   name and email from this computer, and the GitHub token for `gh` and git. A token changed in Settings reaches the
//!   machine on the next connect; setting or clearing it reconnects the connected machines at once.
//! - **Codex runs through Nexus**. No ChatGPT sign-in goes to a machine: OpenAI permits a ChatGPT plan on
//!   a hosted machine only through its Sign in with ChatGPT partner program. The machine's host starts Codex on Team
//!   API with its own Nexus key, and refuses Codex on a plan. [`agent_env`] is where that program's token would join.
//! - **Sleep is Gateway's.** The machine's host tells Gateway what its sessions do every minute, and Gateway decides
//!   when the machine sleeps. Archiving a cloud session deletes its machine ([`Cloud::delete`]).
//! - **GitHub, repos, ports, previews** over SSH to the machine (the target the last connect reached).
//! - **Through Gateway** (the relay). A machine whose host is online at Gateway (`hostOnline`) is
//!   reached through it: one WebSocket to `…/connect` with the person's sign-in, and no SSH, no key, no install. The
//!   setup goes as the `host_provision` call ([`Provision::host_provision`]). The steps are Create, Start, Connect and
//!   Ready. A machine whose host is not online [`HOST_ONLINE_WAIT`] after it is ready, or a Gateway that does not say,
//!   is reached over SSH as before. SSH is still reached later for what needs it (previews, ports, the GitHub sign-in).
//! - **Errors that do not heal** ([`CloudError::heals`]: a refused sign-in, a role or an organization without cloud
//!   machines, no host build) stop a connect's tries; the error shows with "Try again".
//!
//! Nothing here takes a UI toolkit ([`CloudEnv`]), so the tests drive the same code against the fake Gateway (over the
//! fake boat server) and a local sshd (desktop-gpui's src/cloud/tests.rs).

pub mod gateway;
pub mod history;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use nexus_desktop_core::secrets::Vault;
use nexus_desktop_core::settings::{MachineSetting, Settings};
use nexus_remote::activity;
use nexus_remote::client::Pushed;
use nexus_remote::forward::{self, Forwards};
use nexus_remote::install::{Fetch, Installer, Source, parse};
use nexus_remote::machine::{Backoff, Link, OpenError, Opened, Reached, Remote, Sink, SocketOpened, SshLink, Status};
use nexus_remote::ssh::{Target, ssh_error};
use nexus_workbench::cloud_agent::cloud_agent;
use nexus_workbench::event::SessionAgent;
use nexus_workbench::process::async_command;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use crate::keys::{self, KeyPair};
use crate::link::{CloudLink, PLAYWRIGHT_MCP, Progress, Provision, WAKE_DEADLINE, provision, wake_up};
use crate::{Error as CloudError, Machine, Provider, State as ProviderState, wait_ready};
use gateway::{GatewayCloud, GatewayMachine, Unavailable};

/// The two sign-ins' keychain accounts, their hints and the token check, shared with every desktop shell.
pub use nexus_desktop_core::cloud::{CLAUDE_ACCOUNT, GITHUB_ACCOUNT, hint, looks_like_setup_token};

/// The keychain account an older app kept machine `id`'s stop-only key in; removed when the machine is forgotten.
fn old_stop_account(id: &str) -> String {
    format!("cloud-stop-{id}")
}

/// Where the agent's browser saves its screenshots and videos: the session's own project folder, so a file saved
/// without a name shows in the reply. Relative on purpose: @playwright/mcp resolves `--output-dir` against
/// its own working folder, and Claude Code starts the browser in the session's folder. One `mcp.json` serves every
/// session on the machine, so it cannot name a session's absolute path.
pub const PROOF_DIR: &str = ".nexus/artifacts";

/// Ports a preview never offers: SSH, and the ones boat keeps for its own desktop and services.
fn reserved_port(port: u16) -> bool {
    matches!(port, 22 | 6080 | 8090 | 8911 | 47984..=48010)
}

/// What a read gets from a sleeping machine: the screen's quiet word for it, nothing to explain. Reading never wakes
/// one; sending a message does.
pub const ASLEEP: &str = "Asleep";

/// What [`Cloud::repos`] answers without a GitHub sign-in to list them with: the app shows the GitHub step for it,
/// not an error.
pub const GITHUB_NOT_CONNECTED: &str = "Connect GitHub in Settings › Cloud to pick a repository.";

/// How long after a machine is ready its host has to come online at Gateway before the app reaches it over SSH.
pub const HOST_ONLINE_WAIT: Duration = Duration::from_secs(30);
const SIGN_IN: &str = "Sign in to Nexus to use your cloud machine.";
/// An answer or a step for a sign-in the person left.
const CHANGED: &str = "The Nexus sign-in changed, so this cloud machine is closed here.";

/// The sign-in the cloud machines here belong to: the one Gateway listed them for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Owner {
    /// The sign-in's [`fingerprint`].
    key: u64,
    /// Its turn: a new one at every change of sign-in, and when Gateway refuses this one ([`Inner::revoke`]).
    generation: u64,
}

/// The Claude and GitHub tokens one sign-in holds here, and what settings keep of them ([`Inner::secrets`]).
#[derive(Default)]
struct Secrets {
    claude: Option<String>,
    github: Option<String>,
    account: CloudAccountSetting,
}

/// What the machines here are bound to: a sign-in (None: signed out) and its generation.
struct Bound {
    key: Option<u64>,
    generation: u64,
}

/// A sign-in's fingerprint: its Gateway and token, hashed (no token kept in memory twice). Another organization is
/// another token.
fn fingerprint(client: &nexus_client::Client) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    client.base().hash(&mut hasher);
    client.options().api_key.hash(&mut hasher);
    hasher.finish()
}

/// A sign-in's tag in settings.json (`cloud.owner`): its Gateway and token, through SHA-256, so it outlives a Rust
/// upgrade (a [`fingerprint`] may not) and gives nothing of the token back.
fn owner_tag(client: &nexus_client::Client) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"nexus-desktop cloud owner\n");
    hash.update(client.base().as_bytes());
    hash.update(b"\n");
    hash.update(client.options().api_key.as_deref().unwrap_or_default().as_bytes());
    hash.finalize()[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// A cloud machine's state and setting, and the cloud setup, as settings.json keeps them; in
/// `nexus_desktop_core::settings` with the rest of the file.
pub use nexus_desktop_core::settings::{CloudAccountSetting, CloudSetting, CloudState};

/// Gateway's state as ours, with its error: [`from_provider`], and `queued`.
fn gateway_state(machine: &GatewayMachine) -> (CloudState, Option<String>) {
    match machine.state.as_str() {
        "queued" => (CloudState::Queued, None),
        _ => from_provider(&machine.machine().state),
    }
}

/// How long after this app put a machine to sleep a `ready` from Gateway is taken as the sleep not yet seen, not as
/// Gateway bringing it back.
const SLEEP_SETTLE: Duration = Duration::from_secs(120);

/// The provider's state as ours, with its error.
fn from_provider(state: &ProviderState) -> (CloudState, Option<String>) {
    match state {
        ProviderState::Creating => (CloudState::Creating, None),
        ProviderState::Starting => (CloudState::Starting, None),
        ProviderState::Ready => (CloudState::Ready, None),
        ProviderState::Sleeping => (CloudState::Sleeping, None),
        ProviderState::Stopping => (CloudState::Stopping, None),
        ProviderState::Failed { message } => (CloudState::Failed, Some(message.clone())),
        ProviderState::Gone => (CloudState::Gone, None),
    }
}

/// `CloudInfo`: what the screens see of a cloud machine.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudInfo {
    pub provider: String,
    pub size: String,
    pub state: CloudState,
    pub error: Option<String>,
    pub sleeping_since: Option<i64>,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub queued_until: Option<i64>,
    pub stop_reason: Option<String>,
    /// The repository its session clones, when this device started it ([`CloudSetting::repo`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

impl From<&CloudSetting> for CloudInfo {
    fn from(setting: &CloudSetting) -> CloudInfo {
        CloudInfo {
            provider: setting.provider.clone(),
            size: setting.size.clone(),
            state: setting.state,
            error: setting.error.clone(),
            sleeping_since: setting.sleeping_since,
            session_id: setting.session_id.clone(),
            title: setting.title.clone(),
            queued_until: setting.queued_until,
            stop_reason: setting.stop_reason.clone(),
            repo: setting.repo.clone(),
        }
    }
}

/// A machine for the app: its setting and where its connection is. A cloud machine's [`CloudInfo`] stands in for its
/// saved setting.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineView {
    #[serde(flatten)]
    pub machine: MachineSetting,
    pub status: Status,
    /// A cloud machine's state ([`CloudInfo`]), in place of its saved setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud: Option<CloudInfo>,
}

/// The cloud account as the screens see it (`CloudAccount`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAccount {
    pub availability: CloudAvailability,
    pub claude: ClaudeAccount,
    pub github: GithubAccount,
}

/// `CloudGithubAccount`: the GitHub token every new machine gets.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubAccount {
    pub connected: bool,
    pub user: Option<String>,
    pub saved_at: Option<i64>,
}

/// `CloudAvailability`: whether this Nexus offers cloud machines to the person, as Gateway last answered.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAvailability {
    pub available: bool,
    /// `unconfigured`, `not_allowed` or `signed_out`; null when available.
    pub reason: Option<String>,
}

impl CloudAvailability {
    fn of(unavailable: Option<Unavailable>) -> CloudAvailability {
        CloudAvailability {
            available: unavailable.is_none(),
            reason: unavailable.map(|u| u.reason().to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAccount {
    pub connected: bool,
    pub hint: Option<String>,
    pub saved_at: Option<i64>,
}

/// A step of creating or waking a machine (`CloudStep`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CloudStep {
    Create,
    Start,
    Ssh,
    Setup,
    Install,
    Connect,
    Ready,
}

/// `CloudProgress`: one step of creating or waking a machine.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudProgress {
    pub machine: String,
    pub step: CloudStep,
    pub detail: Option<String>,
    pub error: Option<String>,
}

/// A repository a session can clone (`CloudRepo`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudRepo {
    pub name: String,
    pub url: String,
    pub private: bool,
    pub pushed_at: Option<i64>,
}

/// A port something listens on, on the machine (`CloudPort`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPort {
    pub port: u16,
    pub process: Option<String>,
}

/// What the cloud machines need from the app: its settings, keychain, Gateway sign-in, events and relay sink. Nexus
/// GPUI's is its `GpuiEnv`; the tests have their own.
pub trait CloudEnv: Send + Sync + 'static {
    fn settings(&self) -> Settings;
    fn update_settings(&self, change: &mut dyn FnMut(&mut Settings)) -> Result<(), String>;
    fn vault(&self) -> Vault;
    /// The client for the Gateway the app is signed in to, with the person's token and organization; None when
    /// signed out.
    fn gateway(&self) -> Option<nexus_client::Client>;
    /// Sends an event to the app's screens: `cloud:progress`, `cloud:state`,
    /// `cloud:machines` and `cloud:github`, with the payloads above.
    fn emit(&self, event: &str, payload: Value);
    /// Where machine `machine`'s relay events and statuses go (`machine:push`, `machine:status`).
    fn sink(&self, machine: &str) -> Arc<dyn Sink>;
    /// The host to install.
    fn source(&self) -> Source;
    fn fetch(&self) -> Option<Fetch>;
    /// The app's data folder; the machines' keys go under `cloud/`.
    fn data_dir(&self) -> PathBuf;
    /// A connect's setup of the awake machine at `target` ([`provision`], over SSH): what it reported (`github`,
    /// `playwright`). A test's machine in memory answers its own.
    fn provision<'a>(
        &'a self,
        target: &'a Target,
        setup: &'a Provision,
    ) -> BoxFuture<'a, Result<HashMap<String, String>, CloudError>> {
        Box::pin(provision(target, setup))
    }
    /// A connect's last step: nexus-host installed, started and reached through `installer` (a server's
    /// [`SshLink`]). A test answers a host in memory.
    fn open_host(&self, installer: Installer) -> BoxFuture<'static, Result<Opened, String>> {
        Box::pin(async move { SshLink { installer }.open().await })
    }
}

/// What can change between the app and the tests.
#[derive(Clone)]
pub struct Options {
    /// This computer's `gh`, for "Use this computer's GitHub".
    pub local_gh: PathBuf,
    pub backoff: Backoff,
    /// How often Gateway is asked for the person's machines.
    pub poll: Duration,
    /// How long a list from Gateway counts as fresh when the window gets focus or the account is read.
    pub sync_fresh: Duration,
    /// GitHub's API, for the repositories the saved token sees.
    pub github_api: String,
    /// How long a wake or a create may take before a call waiting for it fails.
    pub wake_wait: Duration,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            local_gh: PathBuf::from("gh"),
            backoff: Backoff::default(),
            poll: Duration::from_secs(30),
            sync_fresh: Duration::from_secs(15),
            github_api: "https://api.github.com".into(),
            wake_wait: WAKE_DEADLINE + Duration::from_secs(120),
        }
    }
}

/// How a machine's host is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Way {
    /// Not yet decided: through Gateway when its host comes online in time.
    #[default]
    Undecided,
    Relay,
    Ssh,
}

/// What the app holds for a cloud machine while it runs.
#[derive(Default)]
struct Live {
    remote: Mutex<Option<Remote>>,
    /// Held while the link opens, so a sleep waits for a connect in progress to finish before stopping the machine.
    gate: tokio::sync::Mutex<()>,
    /// Put to sleep (by the person or Gateway): the link refuses to open until a wake.
    asleep: AtomicBool,
    /// A wake was asked for: the next open may wake the machine.
    wake: AtomicBool,
    /// When this app last put it to sleep (or deleted it).
    slept: Mutex<Option<Instant>>,
    /// How the last connect reached it, for the SSH commands.
    target: Mutex<Option<Target>>,
    /// The step the last connect reached.
    step: Mutex<Option<CloudStep>>,
    /// The last `cloud:progress` sent, for `cloud_progress` (a screen opened midway).
    progress: Mutex<Option<CloudProgress>>,
    /// Why the last create, wake or connect failed, for a call waiting on it; cleared by the next try.
    failed: Mutex<Option<String>>,
    /// Its previews' port forwards (`nexus_remote::forward`).
    forwards: Forwards,
    /// The create in the background.
    task: Mutex<Option<tokio::task::AbortHandle>>,
    /// The GitHub sign-in on its way.
    github: Mutex<Option<tokio::task::AbortHandle>>,
    /// Through Gateway or over SSH; decided again after a wake.
    way: Mutex<Way>,
}

impl Live {
    fn remote(&self) -> Option<Remote> {
        self.remote.lock().ok().and_then(|r| r.clone())
    }

    fn target(&self) -> Option<Target> {
        self.target.lock().ok().and_then(|t| t.clone())
    }

    fn slept_lately(&self) -> bool {
        self.slept
            .lock()
            .ok()
            .and_then(|s| *s)
            .is_some_and(|at| at.elapsed() < SLEEP_SETTLE)
    }

    fn set_slept(&self, at: Option<Instant>) {
        if let Ok(mut slept) = self.slept.lock() {
            *slept = at;
        }
    }

    fn failed(&self) -> Option<String> {
        self.failed.lock().ok().and_then(|f| f.clone())
    }

    fn set_failed(&self, error: Option<String>) {
        if let Ok(mut failed) = self.failed.lock() {
            *failed = error;
        }
    }

    fn connected(&self) -> bool {
        self.remote()
            .is_some_and(|remote| matches!(remote.status(), Status::Connected { .. }))
    }

    fn way(&self) -> Way {
        self.way.lock().map(|way| *way).unwrap_or_default()
    }

    fn set_way(&self, way: Way) {
        if let Ok(mut slot) = self.way.lock() {
            *slot = way;
        }
    }

    /// Connected, and reached as the last connect said: over SSH with its address, or through Gateway.
    fn reached(&self) -> bool {
        self.connected() && (self.target().is_some() || self.way() == Way::Relay)
    }

    /// A create still running in the background.
    fn creating(&self) -> bool {
        self.task
            .lock()
            .ok()
            .and_then(|t| t.as_ref().map(|t| !t.is_finished()))
            .unwrap_or(false)
    }

    /// Stops what runs against the machine now: the relay is held, the previews close, a GitHub sign-in stops.
    fn park(&self) {
        if let Some(remote) = self.remote() {
            remote.hold();
        }
        self.forwards.close();
        if let Some(github) = self.github.lock().ok().and_then(|mut g| g.take()) {
            github.abort();
        }
    }

    /// Its sign-in ended: the relay closes for good, the previews close, the create and the GitHub sign-in stop, and
    /// the address it was reached at is forgotten.
    fn end(&self) {
        self.asleep.store(true, Ordering::SeqCst);
        self.wake.store(false, Ordering::SeqCst);
        self.park();
        if let Some(task) = self.task.lock().ok().and_then(|mut t| t.take()) {
            task.abort();
        }
        let remote = self.remote.lock().ok().and_then(|mut r| r.take());
        if let Some(remote) = remote {
            remote.close();
        }
        if let Ok(mut target) = self.target.lock() {
            *target = None;
        }
    }
}

struct Inner {
    env: Arc<dyn CloudEnv>,
    options: Options,
    runtime: tokio::runtime::Handle,
    live: Mutex<HashMap<String, Arc<Live>>>,
    /// Held by a sync and by a create, so a sync never forgets a machine a create is saving.
    sync_lock: tokio::sync::Mutex<()>,
    /// When the last sync started.
    synced: Mutex<Option<Instant>>,
    /// The sign-in the machines here belong to; None until the first look ([`Inner::owner`]). Another one is a
    /// sign-in, a sign-out, an organization or a Gateway switch.
    bound: Mutex<Option<Bound>>,
    /// `bound`'s generation, read without its lock: by the relays' sinks, which a change calls while it holds it.
    generation: AtomicU64,
    /// Why cloud machines are unavailable, as Gateway last answered; None: available.
    unavailable: Mutex<Option<Unavailable>>,
    loops: Mutex<Vec<tokio::task::AbortHandle>>,
    /// [`HOST_ONLINE_WAIT`], in milliseconds; shorter in tests ([`Cloud::set_host_online_wait`]).
    host_wait: AtomicU64,
    me: Weak<Inner>,
}

/// The app's cloud machines.
#[derive(Clone)]
pub struct Cloud {
    inner: Arc<Inner>,
}

fn millis(wait: Duration) -> u64 {
    u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// The Playwright browser for the agents on a cloud machine (`mcp.json`): Chrome, which boat preinstalls (Playwright's
/// own download would land in `~/.cache`, wiped on every wake), headless (one shared screen, many sessions), its own
/// profile per session (`--isolated`: a saved profile can be open in one browser only), screenshots and recordings in
/// the session's [`PROOF_DIR`], and the devtools tools, which record video and traces on request.
pub fn playwright_config() -> String {
    json!({
        "mcpServers": {
            "browser": {
                "command": "npx",
                "args": [
                    "-y", PLAYWRIGHT_MCP, "--headless", "--browser", "chrome", "--isolated",
                    "--output-dir", PROOF_DIR, "--caps", "devtools",
                ],
            }
        }
    })
    .to_string()
}

/// The machine's `agent.env` from the plan sign-ins kept on this device, or None with none. One line per agent plan,
/// named by the agent's `CloudAgent::token_var` (nexus-host's `agent_files::plan_credential` reads the same names).
/// Only Claude's today: a ChatGPT plan never goes to a cloud machine until Nexus can use Sign in with ChatGPT, and then
/// its token is one more line here.
pub fn agent_env(claude: Option<String>) -> Option<String> {
    let lines: String = [(SessionAgent::Claude, claude)]
        .into_iter()
        .filter_map(|(agent, value)| Some((cloud_agent(agent).token_var()?, value?)))
        .map(|(key, value)| format!("{key}={}\n", value.trim()))
        .collect();
    (!lines.is_empty()).then_some(lines)
}

/// What the agents are told on a cloud machine, in their own block of `~/.claude/CLAUDE.md` and `~/.codex/AGENTS.md`
/// (the person's text there stays): where they run, the browser it has, how the person sees a dev server, and how the work leaves the machine.
pub const AGENT_INSTRUCTIONS: &str = "\
# You are on a Nexus cloud machine

- This is a Linux VM in the cloud, not the person's laptop. The laptop may be closed: finish the work and report it.
- You have a browser tool (Playwright MCP, server `browser`). Use it to check UI work, and take screenshots of the
  result: they show in your reply.
- Start dev servers on 0.0.0.0 or 127.0.0.1 with a fixed port, and say the port. The person opens it with Preview in
  the Nexus app.
- When the work is done, push the branch and open a pull request with `gh pr create`. `gh` and git are signed in as
  the person.
- Keep secrets out of commits, logs and replies: tokens, keys, `.env` files, and anything in `~/.nexus`.
";

/// `2026-11-01T12:34:56Z` (or with a fraction or an offset) as unix ms. Enough for GitHub's, boat's and Gateway's
/// times. Digits alone are unix ms already.
pub fn parse_time(text: &str) -> Option<i64> {
    let text = text.trim();
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
        return text.parse().ok();
    }
    let (date, time) = text.split_once(['T', ' '])?;
    let mut date = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let (clock, offset_secs) = match time.find(['Z', 'z', '+', '-']) {
        Some(at) => {
            let (clock, zone) = time.split_at(at);
            let offset = if zone.eq_ignore_ascii_case("z") {
                0
            } else {
                let sign = if zone.starts_with('-') { -1 } else { 1 };
                let mut parts = zone[1..].split(':').map(|p| p.parse::<i64>().ok());
                let hours = parts.next()??;
                let minutes = parts.next().flatten().unwrap_or(0);
                sign * (hours * 3600 + minutes * 60)
            };
            (clock, offset)
        }
        None => (time, 0),
    };
    let mut clock = clock.splitn(3, ':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let seconds: f64 = clock.next().unwrap_or("0").parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3600 + minute * 60 - offset_secs;
    Some(secs * 1000 + (seconds * 1000.0).round() as i64)
}

/// `ss -ltnpH`'s lines as ports, without SSH, boat's own, `skip` (the host's), and duplicates (IPv4 and IPv6).
pub fn parse_ports(output: &str, skip: Option<u16>) -> Vec<CloudPort> {
    let mut ports: Vec<CloudPort> = Vec::new();
    for line in output.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(local) = fields.get(3) else {
            continue;
        };
        let Some(port) = local.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) else {
            continue;
        };
        if port == 0 || reserved_port(port) || Some(port) == skip {
            continue;
        }
        let process = line
            .split_once("((\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(name, _)| name.to_string())
            .filter(|name| !name.is_empty());
        match ports.iter_mut().find(|p| p.port == port) {
            Some(known) => {
                if known.process.is_none() {
                    known.process = process;
                }
            }
            None => ports.push(CloudPort { port, process }),
        }
    }
    ports.sort_by_key(|p| p.port);
    ports
}

/// The repositories script's lines (`name\turl\tprivate\tpushedAt`), newest push first, each once.
pub fn parse_repos(output: &str) -> Vec<CloudRepo> {
    let mut repos: Vec<CloudRepo> = Vec::new();
    for line in output.lines() {
        let mut fields = line.split('\t');
        let (Some(name), Some(url)) = (fields.next(), fields.next()) else {
            continue;
        };
        if name.is_empty() || !name.contains('/') || repos.iter().any(|r| r.name == name) {
            continue;
        }
        let private = fields.next() == Some("true");
        let pushed_at = fields.next().and_then(parse_time);
        repos.push(CloudRepo {
            name: name.to_string(),
            url: url.to_string(),
            private,
            pushed_at,
        });
    }
    repos.sort_by_key(|repo| std::cmp::Reverse(repo.pushed_at));
    repos
}

/// From `gh auth login --web`'s output: the one-time code (`! First copy your one-time code: ABCD-1234`) and the
/// page to type it on.
pub fn parse_device_code(line: &str) -> (Option<String>, Option<String>) {
    let code = line
        .split_once("one-time code:")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .map(|code| code.trim_end_matches(['.', ',']).to_string())
        .filter(|code| code.len() >= 4);
    let link = line
        .split_whitespace()
        .find(|word| word.starts_with("https://"))
        .map(|link| link.trim_end_matches(['.', ',', ')']).to_string());
    (code, link)
}

/// GitHub's device sign-in on the machine, its code printed rather than a browser opened.
const GH_LOGIN: &str = r#"export GH_NO_UPDATE_NOTIFIER=1 BROWSER=true
exec gh auth login --hostname github.com --git-protocol https --web 2>&1
"#;

/// After a sign-in: git uses `gh` for github.com, who it is, and the token, for the app's keychain.
const GH_SETUP_GIT: &str = r#"export GH_NO_UPDATE_NOTIFIER=1 GH_PROMPT_DISABLED=1
gh auth setup-git --hostname github.com >/dev/null 2>&1 || echo "setupGit=failed"
echo "user=$(gh api user -q .login 2>/dev/null)"
echo "token=$(gh auth token --hostname github.com 2>/dev/null)"
"#;

/// The repositories the machine's `gh` can see, the person's and their organizations', two pages of the most
/// recently pushed.
const GH_REPOS: &str = r#"export GH_NO_UPDATE_NOTIFIER=1 GH_PROMPT_DISABLED=1
for page in 1 2; do
  gh api "user/repos?affiliation=owner,collaborator,organization_member&sort=pushed&per_page=100&page=$page" \
    --jq '.[] | [.full_name, .html_url, (.private | tostring), (.pushed_at // "")] | @tsv' || exit 1
done
"#;

/// What listens on the machine, and the host's own port.
const PORTS: &str = r#"root="${NEXUS_HOST_DIR:-$HOME/.nexus/host}"
echo "hostport $(cat "$root/run/host.port" 2>/dev/null)"
ss -ltnpH 2>/dev/null
"#;

/// The worktrees (under `~/nexus-worktrees`, and the projects given as arguments) holding commits on no remote.
const UNPUSHED: &str = r#"{
  for d in "$@"; do echo "$d"; done
  find "$HOME/nexus-worktrees" -maxdepth 4 -name .git 2>/dev/null | while IFS= read -r g; do dirname "$g"; done
} | sort -u | while IFS= read -r d; do
  [ -d "$d" ] || continue
  if [ -n "$(git -C "$d" log --branches --not --remotes --oneline 2>/dev/null | head -n 1)" ]; then echo "unpushed=$d"; fi
done
echo "checked=yes"
"#;

impl Cloud {
    /// The cloud machines, on the tokio runtime this is called in.
    pub fn new(env: Arc<dyn CloudEnv>, options: Options) -> Cloud {
        let runtime = tokio::runtime::Handle::current();
        Cloud {
            inner: Arc::new_cyclic(|me| Inner {
                env,
                options,
                runtime,
                live: Mutex::new(HashMap::new()),
                sync_lock: tokio::sync::Mutex::new(()),
                synced: Mutex::new(None),
                bound: Mutex::new(None),
                generation: AtomicU64::new(0),
                unavailable: Mutex::new(None),
                loops: Mutex::new(Vec::new()),
                host_wait: AtomicU64::new(millis(HOST_ONLINE_WAIT)),
                me: me.clone(),
            }),
        }
    }

    /// How long after a machine is ready its host has to come online at Gateway ([`HOST_ONLINE_WAIT`]). Tests shorten
    /// it.
    pub fn set_host_online_wait(&self, wait: Duration) {
        self.inner.host_wait.store(millis(wait), Ordering::SeqCst);
    }

    /// Picks up the saved cloud machines (their relays start, a sleeping one stays asleep), asks Gateway for the
    /// person's machines, and starts asking again every [`Options::poll`]. Signed out, the saved machines are a
    /// previous sign-in's, and are forgotten.
    pub fn start(&self) {
        let inner = &self.inner;
        if let Ok(owner) = inner.owner() {
            inner.while_owner(owner, || {
                for machine in inner.cloud_machines() {
                    let Some(cloud) = &machine.cloud else { continue };
                    inner.resume(owner, &machine.id, cloud.state);
                }
            });
        }
        let poll = {
            let weak = inner.me.clone();
            let every = inner.options.poll;
            inner.runtime.spawn(async move {
                loop {
                    if let Some(inner) = weak.upgrade() {
                        let _ = inner.sync().await;
                    } else {
                        return;
                    }
                    tokio::time::sleep(every).await;
                }
            })
        };
        // A sign-in, a sign-out or an organization switch: a local look every 2 seconds, no call.
        let session = {
            let weak = inner.me.clone();
            inner.runtime.spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    let Some(inner) = weak.upgrade() else { return };
                    if inner.session_moved() {
                        let _ = inner.sync().await;
                    }
                }
            })
        };
        if let Ok(mut loops) = inner.loops.lock() {
            loops.push(poll.abort_handle());
            loops.push(session.abort_handle());
        }
    }

    /// The app quits: the relays close (the machines keep running, and Gateway puts them to sleep once idle), the
    /// previews close.
    pub fn shutdown(&self) {
        if let Ok(mut loops) = self.inner.loops.lock() {
            for task in loops.drain(..) {
                task.abort();
            }
        }
        let all: Vec<Arc<Live>> = self
            .inner
            .live
            .lock()
            .map(|live| live.values().cloned().collect())
            .unwrap_or_default();
        for live in all {
            if let Some(remote) = live.remote() {
                remote.close();
            }
            live.forwards.close();
            for task in [&live.task, &live.github] {
                if let Some(task) = task.lock().ok().and_then(|mut t| t.take()) {
                    task.abort();
                }
            }
        }
    }

    /// Whether `id` is a cloud machine of the sign-in now: a changed sign-in ends the previous one's first.
    pub fn is_cloud(&self, id: &str) -> bool {
        let _ = self.inner.owner();
        self.inner.setting(id).is_some_and(|m| m.kind == "cloud")
    }

    /// Machine `id`'s relay, once it has one.
    /// Whether session `thread` runs on cloud machine `id`, as far as its relay knows without asking the machine: the
    /// webview read the session's events through it (it has to, to show the session) or they arrived live. False for a
    /// server or an unknown machine.
    pub fn holds_session(&self, id: &str, thread: &str) -> bool {
        self.is_cloud(id) && self.remote(id).is_some_and(|remote| remote.holds(thread))
    }

    pub fn remote(&self, id: &str) -> Option<Remote> {
        self.inner.owner().ok()?;
        self.inner.live_get(id)?.remote()
    }

    /// Where its connection is: [`Status::Closed`] while it has no relay, sleeps or is going to sleep.
    pub fn status(&self, id: &str) -> Status {
        match self.inner.owner() {
            Ok(_) => self.inner.status(id),
            Err(_) => Status::Closed,
        }
    }

    /// The last step sent on `cloud:progress` for machine `id`, None once it is ready (or never started).
    pub fn progress(&self, id: &str) -> Option<CloudProgress> {
        self.inner.owner().ok()?;
        let last = self.inner.live_get(id)?.progress.lock().ok()?.clone()?;
        (last.step != CloudStep::Ready || last.error.is_some()).then_some(last)
    }

    /// How the last connect reached it.
    pub fn target(&self, id: &str) -> Option<Target> {
        self.inner.owner().ok()?;
        self.inner.live_get(id)?.target()
    }

    /// The installer for the scripts outside the connection (`machine_probe`, `machine_login`): the address the last
    /// connect reached, which changes on every wake.
    pub fn installer(&self, id: &str) -> Result<Installer, String> {
        self.inner.owner()?;
        let target = self.target(id).ok_or_else(|| {
            if self.inner.asleep(id) {
                ASLEEP.to_string()
            } else {
                "This cloud machine is not reachable yet.".to_string()
            }
        })?;
        Ok(Installer {
            target,
            source: self.inner.env.source(),
            fetch: self.inner.env.fetch(),
        })
    }

    /// Machine `machine` for the webview: its saved setting gives way to [`CloudInfo`] (no key paths).
    pub fn view(&self, machine: MachineSetting) -> MachineView {
        let _ = self.inner.owner();
        self.inner.view(machine)
    }

    // ---- The person's machines, from Gateway ----

    /// Asks Gateway for the person's machines now, and makes this device's list match: see [`Inner::sync`].
    pub async fn sync(&self) -> Result<(), String> {
        self.inner.sync().await
    }

    /// The window got focus: the list is asked for again when the last one is older than [`Options::sync_fresh`].
    pub fn focused(&self) {
        if self.inner.stale() {
            let inner = self.inner.clone();
            self.inner.runtime.spawn(async move {
                let _ = inner.sync().await;
            });
        }
    }

    /// The person signed in, out, or switched organization or Gateway: the previous sign-in's machines end before this
    /// returns ([`Inner::owner`]), and the new one's are asked for.
    pub fn session_changed(&self) {
        let _ = self.inner.owner();
        let inner = self.inner.clone();
        self.inner.runtime.spawn(async move {
            let _ = inner.sync().await;
        });
    }

    // ---- The account ----

    /// Whether cloud machines are available (asked again when the last answer is stale), and the Claude token.
    pub async fn account(&self) -> CloudAccount {
        if self.inner.stale() {
            let _ = self.inner.sync().await;
        }
        self.account_now()
    }

    /// The account as the sign-in now sees it: signed out, or for another sign-in's secrets, nothing is connected.
    fn account_now(&self) -> CloudAccount {
        let inner = &self.inner;
        let secrets = inner.owner().and_then(|owner| inner.secrets(owner)).unwrap_or_default();
        let saved = secrets.account;
        let claude = secrets.claude.is_some();
        let github = secrets.github.is_some();
        CloudAccount {
            availability: CloudAvailability::of(self.inner.unavailable()),
            claude: ClaudeAccount {
                connected: claude,
                hint: saved.claude_hint.filter(|_| claude),
                saved_at: saved.claude_saved_at.filter(|_| claude),
            },
            github: GithubAccount {
                connected: github,
                user: saved.github_user.filter(|_| github),
                saved_at: saved.github_saved_at.filter(|_| github),
            },
        }
    }

    /// Saves the Claude setup token for the sign-in now; the connected machines get it now (they reconnect), the
    /// others on their next connect.
    pub fn claude_set(&self, token: &str) -> Result<CloudAccount, String> {
        let token = token.trim();
        if !looks_like_setup_token(token) {
            return Err(nexus_desktop_core::cloud::NOT_A_SETUP_TOKEN.into());
        }
        let owner = self.inner.owner()?;
        self.inner.change_secret(
            owner,
            |vault| vault.set(CLAUDE_ACCOUNT, token),
            |account| nexus_desktop_core::cloud::claude_saved(account, token, now_ms()),
        )?;
        // Gateway keeps it too, so a session started from the web, the phone or the assistant has it.
        let inner = self.inner.clone();
        self.inner.runtime.spawn(async move {
            inner.claude_to_gateway(owner).await;
        });
        self.inner.reconnect_connected();
        Ok(self.account_now())
    }

    /// Removes the Claude setup token here and from Gateway: no new session of the person's uses it.
    pub fn claude_clear(&self) -> Result<CloudAccount, String> {
        let owner = self.inner.owner()?;
        self.inner.change_secret(
            owner,
            |vault| vault.delete(CLAUDE_ACCOUNT),
            |account| {
                nexus_desktop_core::cloud::claude_cleared(account);
                // Asked even when this device never put it there: another device may have.
                account.claude_on_gateway = true;
            },
        )?;
        let inner = self.inner.clone();
        self.inner.runtime.spawn(async move {
            inner.claude_off_gateway(owner).await;
        });
        self.inner.reconnect_connected();
        Ok(self.account_now())
    }

    // ---- The lifecycle ----

    /// Asks Gateway for the machine of new session `session_id` (`t_<request id>`), saves it and returns it at once; it
    /// starts in the background (`cloud:progress`). Asked again for the same session, Gateway answers the same machine.
    pub async fn create(&self, session_id: &str, title: &str) -> Result<MachineView, String> {
        self.create_for(session_id, title, None).await
    }

    /// The same, for a session that clones `repo`: Gateway may start its machine from a saved copy of it.
    pub async fn create_for(&self, session_id: &str, title: &str, repo: Option<&str>) -> Result<MachineView, String> {
        let inner = &self.inner;
        let owner = inner.owner()?;
        let session_id = session_id.trim();
        if !session_id.starts_with("t_") || !gateway::valid_id(session_id) {
            return Err(format!("{session_id:?} is not a session id."));
        }
        // A sync waits, so it never forgets the machine between Gateway's answer and the save.
        let _sync = inner.sync_lock.lock().await;
        // Asked after the wait: a sign-in that changed meanwhile asks Gateway for nothing.
        let gateway = inner.provider(owner)?;
        // The person's own GitHub sign-in for the machines: Gateway gives the saved copy only to a person GitHub lets
        // read the repository, and asks GitHub that with it. Without one, the machine clones as before.
        let github = repo
            .and_then(|_| inner.secrets(owner).ok())
            .and_then(|secrets| secrets.github);
        let made = gateway
            .create_machine(session_id, title, repo, github.as_deref())
            .await
            .map_err(|error| {
                inner.refused(owner, &error);
                error.to_string()
            })?;
        // An answer for a sign-in the person left saves nothing.
        inner
            .while_owner(owner, || {
                inner.set_unavailable(None);
                let machine = inner.add(&made, repo)?;
                let live = inner.live_of(&machine.id);
                if !live.creating() && live.remote().is_none() {
                    inner.spawn_bring_up(owner, &machine.id, &live);
                }
                Ok(inner.view(machine))
            })
            .ok_or_else(|| CHANGED.to_string())?
    }

    /// Picks up a failed create or wake where it stopped.
    pub fn retry(&self, id: &str) -> Result<(), String> {
        let inner = &self.inner;
        let owner = inner.owner()?;
        inner
            .while_owner(owner, || {
                inner.cloud_setting(id)?;
                let live = inner.live_of(id);
                live.set_failed(None);
                if live.remote().is_none() {
                    if !live.creating() {
                        inner.spawn_bring_up(owner, id, &live);
                    }
                    return Ok(());
                }
                inner.wake(owner, id)
            })
            .ok_or_else(|| CHANGED.to_string())?
    }

    /// Wakes it (when it sleeps) and connects; returns at once (`cloud:progress`, `machine:status`).
    pub fn wake(&self, id: &str) -> Result<(), String> {
        let owner = self.inner.owner()?;
        self.inner
            .while_owner(owner, || self.inner.wake(owner, id))
            .ok_or_else(|| CHANGED.to_string())?
    }

    /// Puts it to sleep: the relay is held (not retried), the previews close, then the provider saves its disk.
    pub async fn sleep(&self, id: &str) -> Result<(), String> {
        let inner = &self.inner;
        let owner = inner.owner()?;
        let (live, was_asleep) = inner
            .while_owner(owner, || {
                inner.cloud_setting(id)?;
                let live = inner.live_of(id);
                let was_asleep = live.asleep.swap(true, Ordering::SeqCst);
                live.wake.store(false, Ordering::SeqCst);
                live.set_slept(Some(Instant::now()));
                live.park();
                Ok::<_, String>((live, was_asleep))
            })
            .ok_or_else(|| CHANGED.to_string())??;
        // A connect in progress finishes (or fails) before the machine is stopped under it.
        let _gate = live.gate.lock().await;
        let slept = async { inner.provider(owner)?.sleep(id).await.map_err(String::from) }.await;
        if let Err(error) = slept {
            inner.while_owner(owner, || {
                if !was_asleep {
                    live.asleep.store(false, Ordering::SeqCst);
                    if let Some(remote) = live.remote() {
                        remote.resume();
                    }
                }
            });
            return Err(error);
        }
        inner
            .while_owner(owner, || {
                if let Ok(mut target) = live.target.lock() {
                    *target = None;
                }
                inner.set_state(id, CloudState::Stopping, None, None);
            })
            .ok_or_else(|| CHANGED.to_string())?;
        // Follow it until it is asleep, so `cloud:state` says so without waiting for the next poll.
        let weak = inner.me.clone();
        let id = id.to_string();
        inner.runtime.spawn(async move {
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(180) {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(inner) = weak.upgrade() else { return };
                match inner.refresh(owner, &id).await {
                    Some(CloudState::Stopping) | None if inner.owns(owner) => {}
                    _ => return,
                }
            }
        });
        Ok(())
    }

    /// Deletes it through Gateway and forgets it here (the other devices forget it on their next sync). Unless `force`, first lists the worktrees on it holding commits
    /// that are on no remote, and deletes nothing when there are any.
    pub async fn delete(&self, id: &str, force: bool) -> Result<Vec<String>, String> {
        let inner = &self.inner;
        let owner = inner.owner()?;
        let machine = inner.cloud_setting(id)?;
        if !force && !matches!(machine.state, CloudState::Gone | CloudState::Failed) {
            let unpushed = self
                .unpushed(owner, id)
                .await
                .map_err(|error| format!("Could not check the machine for unpushed work: {error}"))?;
            if !unpushed.is_empty() {
                return Ok(unpushed);
            }
        }
        let (live, was_asleep) = inner
            .while_owner(owner, || {
                let live = inner.live_of(id);
                let was_asleep = live.asleep.swap(true, Ordering::SeqCst);
                live.set_slept(Some(Instant::now()));
                live.park();
                if let Some(task) = live.task.lock().ok().and_then(|mut t| t.take()) {
                    task.abort();
                }
                (live, was_asleep)
            })
            .ok_or_else(|| CHANGED.to_string())?;
        let deleted = async { inner.provider(owner)?.delete(id).await.map_err(String::from) }.await;
        if let Err(error) = deleted {
            inner.while_owner(owner, || {
                if !was_asleep {
                    live.asleep.store(false, Ordering::SeqCst);
                    if let Some(remote) = live.remote() {
                        remote.resume();
                    }
                }
            });
            return Err(error);
        }
        inner
            .while_owner(owner, || {
                inner.forget(id, &machine)?;
                let mut gone = machine;
                gone.state = CloudState::Gone;
                inner
                    .env
                    .emit("cloud:state", json!({ "machine": id, "cloud": CloudInfo::from(&gone) }));
                Ok(Vec::new())
            })
            .ok_or_else(|| CHANGED.to_string())?
    }

    /// The worktrees and projects on the machine holding commits that are on no remote.
    async fn unpushed(&self, owner: Owner, id: &str) -> Result<Vec<String>, String> {
        let target = self.awake_as(owner, id).await?;
        let projects: Vec<String> = match self.inner.live_get(id).and_then(|live| live.remote()) {
            Some(remote) => remote
                .call("wb_overview", json!({}))
                .await
                .ok()
                .and_then(|o| o.get("projects").and_then(Value::as_array).cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|p| p.get("path").and_then(Value::as_str).map(String::from))
                .collect(),
            None => Vec::new(),
        };
        self.inner.still(owner)?;
        let args: Vec<&str> = projects.iter().map(String::as_str).collect();
        let output = target.run(UNPUSHED, &args, None, Duration::from_secs(60)).await?;
        self.inner.still(owner)?;
        if !output.stdout.contains("checked=yes") {
            return Err(ssh_error(&output.stderr).unwrap_or_else(|| "the machine did not answer.".into()));
        }
        Ok(output
            .stdout
            .lines()
            .filter_map(|line| line.strip_prefix("unpushed="))
            .map(String::from)
            .collect())
    }

    /// The machine awake and connected, waking it first when it sleeps: how it is reached.
    pub async fn awake(&self, id: &str) -> Result<Target, String> {
        let owner = self.inner.owner()?;
        self.awake_as(owner, id).await
    }

    /// [`Cloud::awake`] for sign-in `owner`: it stops waiting once another sign-in replaced it. A machine reached
    /// through Gateway has no SSH address yet: it is reached over SSH now (its key registered), for what needs SSH.
    async fn awake_as(&self, owner: Owner, id: &str) -> Result<Target, String> {
        let live = self.connected_as(owner, id).await?;
        if let Some(target) = live.target() {
            return Ok(target);
        }
        self.inner.ssh_target(owner, id, &live).await
    }

    /// The machine awake and its relay connected, waking it first when it sleeps.
    async fn connected_as(&self, owner: Owner, id: &str) -> Result<Arc<Live>, String> {
        let inner = &self.inner;
        let live = inner
            .while_owner(owner, || {
                inner.cloud_setting(id)?;
                Ok::<_, String>(inner.live_of(id))
            })
            .ok_or_else(|| CHANGED.to_string())??;
        if live.reached() {
            return Ok(live);
        }
        let parked = live.asleep.load(Ordering::SeqCst) || live.remote().is_none_or(|r| r.is_held());
        if parked || matches!(inner.status(id), Status::Unreachable { .. }) {
            inner
                .while_owner(owner, || inner.wake(owner, id))
                .ok_or_else(|| CHANGED.to_string())??;
        }
        let started = Instant::now();
        loop {
            inner.still(owner)?;
            if live.reached() {
                return Ok(live);
            }
            if let Some(error) = live.failed() {
                return Err(error);
            }
            if live.asleep.load(Ordering::SeqCst) && !live.wake.load(Ordering::SeqCst) {
                return Err(ASLEEP.into());
            }
            if started.elapsed() > inner.options.wake_wait {
                return Err("The cloud machine did not wake in time.".into());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// A `wb_*` call on the machine (`machine_call`): a sleeping machine is woken first, except for reads, the screen's
    /// own and the person's opening a session to look at it ([`activity::wakes`]).
    pub async fn call(&self, id: &str, call: &str, args: Value) -> Result<Value, String> {
        // A changed sign-in ends the previous one's machines here first: their relays are gone.
        let owner = self.inner.owner()?;
        let live = self.inner.live_get(id).ok_or_else(|| "No such machine.".to_string())?;
        if !live.connected() {
            let parked = live.asleep.load(Ordering::SeqCst) || live.remote().is_none_or(|r| r.is_held());
            if parked && !activity::wakes(call) {
                return Err(ASLEEP.into());
            }
            self.connected_as(owner, id).await?;
        }
        let remote = live
            .remote()
            .ok_or_else(|| "Not connected to this machine yet.".to_string())?;
        let answer = remote.call(call, args).await;
        // An answer that arrives after the sign-in changed goes nowhere.
        self.inner.still(owner)?;
        answer
    }

    /// Asks Gateway for every machine's state now (what the 30-second poll does).
    pub async fn refresh(&self) {
        let _ = self.inner.sync().await;
    }

    // ---- GitHub, repos, ports, previews ----

    /// This computer's `gh` token (`gh auth token`), when it is signed in to GitHub.
    async fn local_github_token(&self) -> Result<String, String> {
        let mut local = async_command(&self.inner.options.local_gh);
        local
            .args(["auth", "token", "--hostname", "github.com"])
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(20), local.output())
            .await
            .map_err(|_| "This computer's gh did not answer.".to_string())?
            .map_err(|_| "gh is not installed on this computer.".to_string())?;
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() || token.is_empty() || token.contains(char::is_whitespace) {
            return Err(
                "gh on this computer is not signed in to GitHub. Run `gh auth login` here first, or sign in on \
                        a cloud machine instead."
                    .into(),
            );
        }
        Ok(token)
    }

    /// Saves this computer's GitHub sign-in for the machines: every new machine's `gh` and git use it.
    pub async fn github_use_local(&self) -> Result<CloudAccount, String> {
        let owner = self.inner.owner()?;
        let token = self.local_github_token().await?;
        self.save_github(owner, &token).await
    }

    /// Checks `token` with GitHub, then keeps it in the keychain with its login, for sign-in `owner` (refused once
    /// another replaced it). The connected machines get it now (they reconnect), the others on their next connect.
    async fn save_github(&self, owner: Owner, token: &str) -> Result<CloudAccount, String> {
        let user = github_api(&self.inner.options.github_api, token, "/user")
            .await?
            .get("login")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| "GitHub did not say who you are.".to_string())?;
        self.inner.change_secret(
            owner,
            |vault| vault.set(GITHUB_ACCOUNT, token),
            |account| nexus_desktop_core::cloud::github_saved(account, user, now_ms()),
        )?;
        self.inner.reconnect_connected();
        Ok(self.account_now())
    }

    /// Forgets the GitHub token here. Machines that have it keep their `gh` sign-in.
    pub fn github_clear(&self) -> Result<CloudAccount, String> {
        let owner = self.inner.owner()?;
        self.inner.change_secret(
            owner,
            |vault| vault.delete(GITHUB_ACCOUNT),
            nexus_desktop_core::cloud::github_cleared,
        )?;
        Ok(self.account_now())
    }

    /// Starts GitHub's device sign-in on the machine: the code arrives on `cloud:github`, then `done` or `failed`.
    pub async fn github_login(&self, id: &str) -> Result<(), String> {
        let owner = self.inner.owner()?;
        let target = self.awake_as(owner, id).await?;
        let live = self.inner.live_get(id).ok_or_else(|| CHANGED.to_string())?;
        let mut command = target.command(GH_LOGIN, &[])?;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| format!("Could not run ssh: {e}. Is OpenSSH installed?"))?;
        let env = self.inner.env.clone();
        let weak = self.inner.me.clone();
        let machine = id.to_string();
        let task = self.inner.runtime.spawn(async move {
            // Nothing of it reaches the webview or the keychain once the person left the sign-in.
            let owned = || weak.upgrade().is_some_and(|inner| inner.owns(owner));
            let emit = |payload: Value| {
                if owned() {
                    env.emit("cloud:github", payload);
                }
            };
            let mut stdin = child.stdin.take();
            // Older gh asks for Enter before it opens the page; ours opens nothing (BROWSER=true).
            if let Some(stdin) = stdin.as_mut() {
                let _ = stdin.write_all(b"\n").await;
            }
            let (mut code, mut link, mut last) = (None, None, String::new());
            if let Some(stdout) = child.stdout.take() {
                let mut lines = tokio::io::BufReader::new(stdout).lines();
                let read = tokio::time::timeout(Duration::from_secs(15 * 60), async {
                    while let Ok(Some(line)) = lines.next_line().await {
                        let (found_code, found_link) = parse_device_code(&line);
                        let fresh = found_code.is_some() || found_link.is_some();
                        if code.is_none() && found_code.is_some() {
                            code = found_code;
                        }
                        if link.is_none() && found_link.is_some() {
                            link = found_link;
                        }
                        if let Some(code) = &code
                            && fresh
                        {
                            emit(json!({
                                "machine": machine, "state": "code", "code": code,
                                "link": link.clone().unwrap_or_else(|| "https://github.com/login/device".into()),
                            }));
                        }
                        if !line.trim().is_empty() {
                            last = line.trim().trim_start_matches(['!', 'X', '✓', ' ']).to_string();
                        }
                    }
                })
                .await;
                if read.is_err() {
                    emit(json!({ "machine": machine, "state": "failed", "error": "The GitHub code expired." }));
                    return;
                }
            }
            drop(stdin);
            let ok = child.wait().await.is_ok_and(|status| status.success());
            if !ok {
                let error = if last.is_empty() {
                    "GitHub sign-in did not finish.".to_string()
                } else {
                    last
                };
                emit(json!({ "machine": machine, "state": "failed", "error": error }));
                return;
            }
            let setup = target.run(GH_SETUP_GIT, &[], None, Duration::from_secs(60)).await;
            let fields = setup.ok().map(|o| parse(&o.stdout)).unwrap_or_default();
            let user = fields.get("user").cloned().filter(|u| !u.is_empty());
            // The token comes back to this computer's keychain, so every new machine gets it.
            if let (Some(token), Some(cloud)) = (fields.get("token").filter(|t| !t.is_empty()), weak.upgrade())
                && owned()
                && let Err(error) = (Cloud { inner: cloud }).save_github(owner, token).await
            {
                eprintln!("nexus-desktop: cloud machine {machine}: GitHub token not kept: {error}");
            }
            match user {
                Some(user) => emit(json!({ "machine": machine, "state": "done", "user": user })),
                None => emit(json!({ "machine": machine, "state": "failed",
                    "error": "Signed in, but GitHub did not say who you are." })),
            }
        });
        // Kept with the machine, so an end of the sign-in stops it; stopped now if that already happened.
        let kept = self.inner.while_owner(owner, || {
            if let Some(old) = live.github.lock().ok().and_then(|mut g| g.replace(task.abort_handle())) {
                old.abort();
            }
        });
        if kept.is_none() {
            task.abort();
            return Err(CHANGED.into());
        }
        Ok(())
    }

    /// The repositories the person's GitHub sees, newest push first: with the token the sign-in saved, from GitHub's
    /// API (no machine needed, for a new session); else through `gh` on machine `id`. Signed out, none.
    pub async fn repos(&self, id: Option<&str>) -> Result<Vec<CloudRepo>, String> {
        let owner = self.inner.owner()?;
        if let Some(token) = self.inner.secrets(owner)?.github {
            let mut repos = Vec::new();
            for page in 1..=2 {
                let path = format!(
                    "/user/repos?affiliation=owner,collaborator,organization_member&sort=pushed&per_page=100&page={page}"
                );
                let answer = github_api(&self.inner.options.github_api, token.trim(), &path).await?;
                let items = answer.as_array().cloned().unwrap_or_default();
                repos.extend(items.iter().filter_map(github_repo));
                if items.len() < 100 {
                    break;
                }
            }
            // A list that arrives after the sign-in changed goes nowhere.
            self.inner.still(owner)?;
            return Ok(repos);
        }
        let Some(id) = id else {
            return Err(GITHUB_NOT_CONNECTED.into());
        };
        let target = self.awake_as(owner, id).await?;
        let output = target.run(GH_REPOS, &[], None, Duration::from_secs(60)).await?;
        self.inner.still(owner)?;
        if !output.ok {
            return Err(ssh_error(&output.stderr)
                .map(|e| format!("Could not list your GitHub repositories: {e}"))
                .unwrap_or_else(|| {
                    "Could not list your GitHub repositories. Is GitHub connected on the machine?".into()
                }));
        }
        Ok(parse_repos(&output.stdout))
    }

    /// The ports something listens on, on the machine.
    pub async fn ports(&self, id: &str) -> Result<Vec<CloudPort>, String> {
        let owner = self.inner.owner()?;
        let target = self.awake_as(owner, id).await?;
        let output = target.run(PORTS, &[], None, Duration::from_secs(30)).await?;
        self.inner.still(owner)?;
        let host_port = output
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix("hostport "))
            .and_then(|value| value.trim().rsplit(':').next().and_then(|p| p.parse().ok()));
        if !output.stdout.starts_with("hostport") {
            return Err(ssh_error(&output.stderr).unwrap_or_else(|| "The machine did not answer.".into()));
        }
        Ok(parse_ports(&output.stdout, host_port))
    }

    /// Forwards `port` on the machine to a free port here (`ssh -N -L`), kept while the machine is awake, and returns
    /// its address for the preview tab.
    pub async fn preview(&self, id: &str, port: u16) -> Result<String, String> {
        if port == 0 {
            return Err("Port 0 cannot be previewed.".into());
        }
        let owner = self.inner.owner()?;
        let target = self.awake_as(owner, id).await?;
        let live = self.inner.live_get(id).ok_or_else(|| CHANGED.to_string())?;
        if let Some(local) = live.forwards.live(port) {
            return Ok(format!("http://127.0.0.1:{local}"));
        }
        // The sign-in changed while it started: the forward closes now (dropped), not after the wait.
        let forward = forward::open(&target, port, || {
            if self.inner.owns(owner) {
                Ok(())
            } else {
                Err(CHANGED.into())
            }
        })
        .await?;
        // Kept with the machine, so an end of the sign-in closes it; closed now (dropped) if that already happened.
        let local = self
            .inner
            .while_owner(owner, || live.forwards.keep(port, forward))
            .ok_or_else(|| CHANGED.to_string())?;
        Ok(format!("http://127.0.0.1:{local}"))
    }

    /// One page of session thread `thread`'s history on machine `id`, from Gateway. It reads Gateway only:
    /// a sleeping machine stays asleep.
    pub async fn history(
        &self,
        id: &str,
        thread: &str,
        page: history::Page,
        limit: usize,
    ) -> Result<history::HistoryPage, String> {
        let owner = self.inner.owner()?;
        let gateway = self.inner.provider(owner)?;
        let page = gateway.gateway()?.history(id, thread, page, limit).await?;
        self.inner.still(owner)?;
        Ok(page)
    }

    /// The provider's link to `port` (it carries a token: anyone holding it gets in).
    pub async fn share(&self, id: &str, port: u16) -> Result<String, String> {
        let owner = self.inner.owner()?;
        self.awake_as(owner, id).await?;
        let link = self.inner.provider(owner)?.preview(id, port, false).await?;
        self.inner.still(owner)?;
        Ok(link)
    }

    /// The provider's live desktop stream (must be opened within minutes).
    pub async fn desktop(&self, id: &str) -> Result<String, String> {
        let owner = self.inner.owner()?;
        self.awake_as(owner, id).await?;
        let link = self.inner.provider(owner)?.desktop(id).await?;
        self.inner.still(owner)?;
        Ok(link)
    }
}

/// A GET on GitHub's API with the person's token.
async fn github_api(base: &str, token: &str, path: &str) -> Result<Value, String> {
    nexus_client::install_tls();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(format!("{}{path}", base.trim_end_matches('/')))
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "nexus-desktop")
        .send()
        .await
        .map_err(|e| format!("Could not reach GitHub: {e}"))?;
    match response.status().as_u16() {
        200 => response
            .json()
            .await
            .map_err(|e| format!("GitHub answered something unexpected: {e}")),
        401 => Err("GitHub refused the saved sign-in. Connect GitHub again in Settings › Cloud.".into()),
        status => Err(format!("GitHub answered {status}.")),
    }
}

/// One repository from GitHub's API.
fn github_repo(item: &Value) -> Option<CloudRepo> {
    Some(CloudRepo {
        name: item.get("full_name")?.as_str()?.to_string(),
        url: item.get("html_url")?.as_str()?.to_string(),
        private: item.get("private").and_then(Value::as_bool).unwrap_or(false),
        pushed_at: item.get("pushed_at").and_then(Value::as_str).and_then(parse_time),
    })
}

impl Inner {
    fn cloud_machines(&self) -> Vec<MachineSetting> {
        self.env
            .settings()
            .machines
            .into_iter()
            .filter(|m| m.kind == "cloud")
            .collect()
    }

    fn setting(&self, id: &str) -> Option<MachineSetting> {
        self.env.settings().machines.into_iter().find(|m| m.id == id)
    }

    fn cloud_setting(&self, id: &str) -> Result<CloudSetting, String> {
        self.setting(id)
            .and_then(|m| m.cloud)
            .ok_or_else(|| "No such cloud machine.".to_string())
    }

    fn update_cloud(&self, id: &str, change: impl FnOnce(&mut CloudSetting)) -> Option<CloudSetting> {
        let mut change = Some(change);
        let mut after = None;
        let _ = self.env.update_settings(&mut |settings| {
            if let Some(cloud) = settings
                .machines
                .iter_mut()
                .find(|m| m.id == id)
                .and_then(|m| m.cloud.as_mut())
                && let Some(change) = change.take()
            {
                change(cloud);
                after = Some(cloud.clone());
            }
        });
        after
    }

    fn live_get(&self, id: &str) -> Option<Arc<Live>> {
        self.live.lock().ok()?.get(id).cloned()
    }

    fn live_of(&self, id: &str) -> Arc<Live> {
        match self.live.lock() {
            Ok(mut live) => live.entry(id.to_string()).or_default().clone(),
            Err(_) => Arc::new(Live::default()),
        }
    }

    fn asleep(&self, id: &str) -> bool {
        self.live_get(id).is_some_and(|l| l.asleep.load(Ordering::SeqCst))
    }

    /// Gateway's cloud machines as sign-in `owner`: each call is refused once another sign-in replaced it, so a wake or
    /// a key registration on its way stops calling Gateway for a sign-in the person left.
    fn provider(&self, owner: Owner) -> Result<Arc<Owned>, String> {
        let client = self.env.gateway().ok_or_else(|| SIGN_IN.to_string())?;
        if fingerprint(&client) != owner.key || self.generation.load(Ordering::SeqCst) != owner.generation {
            return Err(CHANGED.into());
        }
        let gateway = GatewayCloud::new(client).ok_or_else(|| SIGN_IN.to_string())?;
        Ok(Arc::new(Owned {
            inner: self.me.clone(),
            owner,
            gateway,
        }))
    }

    // ---- Whose machines ----

    /// The sign-in now, as the machines' owner. When it is not the one they belong to (signed out, another account,
    /// organization or Gateway), the previous one's machines end first ([`Inner::end`]), before this returns. The
    /// first look binds to it, and ends the saved machines only when signed out: they are a previous sign-in's.
    fn owner(&self) -> Result<Owner, String> {
        let (now, generation) = {
            let mut bound = self.bound.lock().unwrap_or_else(PoisonError::into_inner);
            self.rebind(&mut bound)
        };
        now.map(|key| Owner { key, generation })
            .ok_or_else(|| SIGN_IN.to_string())
    }

    /// Binds to the sign-in now, read under `bound`'s lock (so two callers never bind back to an older one), ending
    /// the previous one's machines and secrets when it changed; returns it and its generation.
    fn rebind(&self, bound: &mut Option<Bound>) -> (Option<u64>, u64) {
        let client = self.env.gateway();
        let now = client.as_ref().map(fingerprint);
        if let Some(bound) = bound.as_ref()
            && bound.key == now
        {
            return (now, bound.generation);
        }
        let first = bound.is_none();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *bound = Some(Bound { key: now, generation });
        if !first || now.is_none() {
            self.end(now.is_none().then_some(Unavailable::SignedOut));
        }
        self.claim(client.as_ref().map(owner_tag), first);
        // Another sign-in's account is asked for again.
        if let Ok(mut synced) = self.synced.lock() {
            *synced = None;
        }
        (now, generation)
    }

    /// Runs `apply` while `owner` is the app's sign-in, with no change of sign-in in between; None when it is not.
    /// `apply` holds the lock a change takes: it must not call [`Inner::owner`], this, or anything that does.
    fn while_owner<T>(&self, owner: Owner, apply: impl FnOnce() -> T) -> Option<T> {
        let mut bound = self.bound.lock().unwrap_or_else(PoisonError::into_inner);
        let (now, generation) = self.rebind(&mut bound);
        (now == Some(owner.key) && generation == owner.generation).then(apply)
    }

    /// Err once `owner` is no longer the app's sign-in: for an answer that arrived after it changed.
    fn still(&self, owner: Owner) -> Result<(), String> {
        self.while_owner(owner, || ()).ok_or_else(|| CHANGED.to_string())
    }

    /// Whether `owner` is the app's sign-in, without the lock: for the relays' sinks, which a change calls while it
    /// holds it. A change not seen yet counts too (the fingerprint differs).
    fn owns(&self, owner: Owner) -> bool {
        self.generation.load(Ordering::SeqCst) == owner.generation && self.session_now() == Some(owner.key)
    }

    /// Gateway refused sign-in `owner` (a 401, or a 403 on the list): its machines end as at a sign-out. The same
    /// sign-in lists them again once Gateway lets it.
    fn revoke(&self, owner: Owner, unavailable: Unavailable) {
        let mut bound = self.bound.lock().unwrap_or_else(PoisonError::into_inner);
        let (now, generation) = self.rebind(&mut bound);
        if now != Some(owner.key) || generation != owner.generation {
            return;
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *bound = Some(Bound { key: now, generation });
        self.end(Some(unavailable));
    }

    /// A refused Gateway call as `owner`: a refused token (401) ends its machines; another refusal is kept for the
    /// account while `owner` is the sign-in.
    fn refused(&self, owner: Owner, error: &CloudError) {
        match error {
            CloudError::Unauthorized(_) => self.revoke(owner, Unavailable::SignedOut),
            _ => {
                self.while_owner(owner, || self.note_unavailable(error));
            }
        }
    }

    /// Ends what this device holds of the machines, under `bound`'s lock once the generation moved: each relay closes
    /// (a late event is dropped by [`CloudSink`]), its previews, create and GitHub sign-in stop, then the machines and
    /// this device's keys for them are forgotten, and the webview is told (`cloud:machines`). Gateway lists them again
    /// for a sign-in that has them.
    fn end(&self, unavailable: Option<Unavailable>) {
        let lives: Vec<Arc<Live>> = self
            .live
            .lock()
            .map(|mut live| live.drain().map(|(_, live)| live).collect())
            .unwrap_or_default();
        for live in lives {
            live.end();
        }
        let mut removed = Vec::new();
        for machine in self.cloud_machines() {
            let forgotten = match &machine.cloud {
                Some(cloud) => self.forget(&machine.id, cloud),
                None => self
                    .env
                    .update_settings(&mut |settings| settings.machines.retain(|m| m.id != machine.id)),
            };
            if forgotten.is_ok() {
                removed.push(machine.id);
            }
        }
        self.set_unavailable(unavailable);
        if !removed.is_empty() {
            self.env
                .emit("cloud:machines", json!({ "added": [], "removed": removed }));
        }
    }

    // ---- Whose secrets ----

    /// Keeps the Claude and GitHub tokens only for the sign-in they belong to (`cloud.owner`), under `bound`'s lock at
    /// each change of sign-in: `tag` is the new one's ([`owner_tag`]), None when signed out. Another sign-in's, and
    /// every one at a sign-out, are forgotten ([`Inner::forget_secrets`]). The first look gives secrets with no owner
    /// (saved before any tag) to the sign-in it finds.
    fn claim(&self, tag: Option<String>, first: bool) {
        let saved = self.env.settings().cloud.unwrap_or_default();
        match (tag, saved.owner.as_deref()) {
            (Some(tag), Some(owner)) if tag == owner => {}
            (Some(tag), None) if first => {
                if saved != CloudAccountSetting::default()
                    && let Err(error) = self.env.update_settings(&mut |settings| {
                        if let Some(account) = settings.cloud.as_mut() {
                            account.owner = Some(tag.clone());
                        }
                    })
                {
                    eprintln!("nexus-desktop: cloud secrets: owner not saved: {error}");
                }
            }
            _ => {
                if let Err(error) = self.forget_secrets() {
                    eprintln!("nexus-desktop: cloud secrets not forgotten: {error}");
                }
            }
        }
    }

    /// Deletes both tokens from the keychain, then what settings keep of them. While an entry is still there, the
    /// settings stay, so its owner stays known: no other sign-in reads it, and the next change tries again.
    fn forget_secrets(&self) -> Result<(), String> {
        let vault = self.env.vault();
        for account in [CLAUDE_ACCOUNT, GITHUB_ACCOUNT] {
            vault.delete(account)?;
        }
        if self.env.settings().cloud.is_some() {
            self.env.update_settings(&mut |settings| settings.cloud = None)?;
        }
        Ok(())
    }

    /// The Gateway client of sign-in `owner`, under [`Inner::while_owner`]: the one its tag is made from.
    fn client_of(&self, owner: Owner) -> Option<nexus_client::Client> {
        self.env.gateway().filter(|client| fingerprint(client) == owner.key)
    }

    /// Sign-in `owner`'s tokens and what settings keep of them: none when they are another sign-in's (one the keychain
    /// did not let go of). Err once another sign-in replaced `owner`.
    fn secrets(&self, owner: Owner) -> Result<Secrets, String> {
        self.while_owner(owner, || {
            let tag = owner_tag(&self.client_of(owner)?);
            let account = self.env.settings().cloud.unwrap_or_default();
            if account.owner.as_deref() != Some(tag.as_str()) {
                return Some(Secrets::default());
            }
            let vault = self.env.vault();
            Some(Secrets {
                claude: vault.get(CLAUDE_ACCOUNT),
                github: vault.get(GITHUB_ACCOUNT),
                account,
            })
        })
        .flatten()
        .ok_or_else(|| CHANGED.to_string())
    }

    /// Saves or deletes a token for sign-in `owner`: `apply` changes the keychain, `change` the settings, which then
    /// name `owner`. Secrets that are not `owner`'s go first, so the other token never joins it. Refused once another
    /// sign-in replaced `owner`, so nothing of it reaches the next one.
    fn change_secret(
        &self,
        owner: Owner,
        apply: impl FnOnce(&Vault) -> Result<(), String>,
        change: impl FnOnce(&mut CloudAccountSetting),
    ) -> Result<(), String> {
        self.while_owner(owner, || {
            let tag = owner_tag(&self.client_of(owner).ok_or_else(|| CHANGED.to_string())?);
            if self.env.settings().cloud.and_then(|c| c.owner).as_deref() != Some(tag.as_str()) {
                self.forget_secrets()?;
            }
            apply(&self.env.vault())?;
            let mut change = Some(change);
            self.env.update_settings(&mut |settings| {
                if let Some(change) = change.take() {
                    nexus_desktop_core::cloud::change_account(settings, |account| {
                        change(account);
                        account.owner = Some(tag.clone());
                    });
                }
            })
        })
        .ok_or_else(|| CHANGED.to_string())?
    }

    fn unavailable(&self) -> Option<Unavailable> {
        self.unavailable.lock().ok().and_then(|u| *u)
    }

    fn set_unavailable(&self, unavailable: Option<Unavailable>) {
        if let Ok(mut slot) = self.unavailable.lock() {
            *slot = unavailable;
        }
    }

    /// A refusal that says cloud machines are unavailable is kept for the account.
    fn note_unavailable(&self, error: &CloudError) {
        if let Some(unavailable) = Unavailable::of(error) {
            self.set_unavailable(Some(unavailable));
        }
    }

    /// The sign-in now, as its [`fingerprint`]; None when signed out.
    fn session_now(&self) -> Option<u64> {
        self.env.gateway().map(|client| fingerprint(&client))
    }

    /// Whether the sign-in changed since the machines were bound to one.
    fn session_moved(&self) -> bool {
        let now = self.session_now();
        self.bound
            .lock()
            .map(|bound| bound.as_ref().is_none_or(|bound| bound.key != now))
            .unwrap_or(false)
    }

    /// [`Cloud::status`] without the sign-in check, for the code that already made it.
    fn status(&self, id: &str) -> Status {
        let sleeping = self.asleep(id)
            || self
                .cloud_setting(id)
                .is_ok_and(|c| matches!(c.state, CloudState::Sleeping | CloudState::Stopping));
        if sleeping {
            return Status::Closed;
        }
        self.live_get(id)
            .and_then(|live| live.remote())
            .map(|r| r.status())
            .unwrap_or(Status::Closed)
    }

    /// [`Cloud::view`] without the sign-in check.
    fn view(&self, mut machine: MachineSetting) -> MachineView {
        let status = self.status(&machine.id);
        let cloud = machine.cloud.take().as_ref().map(CloudInfo::from);
        MachineView { machine, status, cloud }
    }

    /// Whether the last sync is older than [`Options::sync_fresh`] (or there was none).
    fn stale(&self) -> bool {
        self.synced
            .lock()
            .ok()
            .and_then(|s| *s)
            .is_none_or(|at| at.elapsed() >= self.options.sync_fresh)
    }

    /// A known machine's relay, as its saved state says: a sleeping one is held until a wake, a gone one has none.
    fn resume(&self, owner: Owner, id: &str, state: CloudState) {
        let live = self.live_of(id);
        match state {
            CloudState::Gone => {}
            CloudState::Sleeping | CloudState::Stopping => {
                live.asleep.store(true, Ordering::SeqCst);
                self.start_remote(owner, id, &live);
            }
            _ => self.start_remote(owner, id, &live),
        }
    }

    /// [`Cloud::wake`] under [`Inner::while_owner`].
    fn wake(&self, owner: Owner, id: &str) -> Result<(), String> {
        self.cloud_setting(id)?;
        let live = self.live_of(id);
        live.set_failed(None);
        live.set_slept(None);
        live.wake.store(true, Ordering::SeqCst);
        live.asleep.store(false, Ordering::SeqCst);
        // A woken machine boots again: its host may come online at Gateway this time.
        live.set_way(Way::Undecided);
        match live.remote() {
            Some(remote) => {
                remote.resume();
                remote.reconnect();
            }
            None => self.start_remote(owner, id, &live),
        }
        Ok(())
    }

    /// Saves Gateway's machine on this device, with this device's key paths; the saved one when it is known.
    fn add(&self, machine: &GatewayMachine, repo: Option<&str>) -> Result<MachineSetting, String> {
        let repo = repo.map(str::trim).filter(|repo| !repo.is_empty()).map(String::from);
        let dir = self.env.data_dir().join("cloud").join(&machine.id);
        let (state, error) = gateway_state(machine);
        let setting = MachineSetting {
            id: machine.id.clone(),
            name: machine.title.clone().unwrap_or_else(|| machine.name.clone()),
            kind: "cloud".into(),
            ssh: String::new(),
            cloud: Some(CloudSetting {
                provider: machine.provider.clone(),
                size: machine.size.clone(),
                key: dir.join("id_ed25519"),
                known_hosts: dir.join("known_hosts"),
                state,
                error: machine.error.clone().or(error),
                session_id: machine.session_id.clone(),
                title: machine.title.clone(),
                repo: repo.clone(),
                queued_until: machine.queued_until.filter(|_| state == CloudState::Queued),
                stop_reason: machine.stop_reason.clone(),
                sleeping_since: machine.sleeping_since.filter(|_| state == CloudState::Sleeping),
                created_at: machine.created_at.unwrap_or_else(now_ms),
            }),
        };
        let mut saved = None;
        self.env.update_settings(&mut |settings| {
            if saved.is_some() {
                return;
            }
            match settings.machines.iter_mut().find(|m| m.id == setting.id) {
                Some(known) => {
                    // A sync may have saved it first: the repository picked here still files it.
                    if let Some(cloud) = known.cloud.as_mut()
                        && cloud.repo.is_none()
                    {
                        cloud.repo = repo.clone();
                    }
                    saved = Some(known.clone());
                }
                None => {
                    settings.machines.push(setting.clone());
                    saved = Some(setting.clone());
                }
            }
        })?;
        saved.ok_or_else(|| "Could not save the machine.".to_string())
    }

    /// Forgets machine `id` on this device: its relay, previews, setting and keys. Nothing at Gateway.
    fn forget(&self, id: &str, cloud: &CloudSetting) -> Result<(), String> {
        if let Some(live) = self.live_get(id) {
            live.asleep.store(true, Ordering::SeqCst);
            live.park();
            for task in [&live.task, &live.github] {
                if let Some(task) = task.lock().ok().and_then(|mut t| t.take()) {
                    task.abort();
                }
            }
            if let Some(remote) = live.remote() {
                remote.close();
            }
        }
        self.env
            .update_settings(&mut |settings| settings.machines.retain(|m| m.id != id))?;
        let _ = self.env.vault().delete(&old_stop_account(id));
        if let Some(dir) = cloud.key.parent()
            && dir.file_name().is_some_and(|name| name == std::ffi::OsStr::new(id))
        {
            let _ = std::fs::remove_dir_all(dir);
        }
        if let Ok(mut all) = self.live.lock() {
            all.remove(id);
        }
        Ok(())
    }

    /// Asks Gateway for the person's machines in the signed-in organization, and makes this device's list match: a
    /// machine it lists that is not known here is added (its relay starts, held when it sleeps), a known one gets
    /// its state, one it no longer lists is forgotten here (deleted on another device, or another organization's).
    /// `cloud:machines` says what was added and removed. Gateway refusing the sign-in (401, 403) ends its machines
    /// ([`Inner::revoke`]); another refusal (no provider account, Gateway struggling) is kept for the account and
    /// changes no machine, and so does a failed call. A list that arrives after the sign-in changed changes nothing.
    async fn sync(self: &Arc<Self>) -> Result<(), String> {
        let _sync = self.sync_lock.lock().await;
        if let Ok(mut synced) = self.synced.lock() {
            *synced = Some(Instant::now());
        }
        // A changed sign-in ends the previous one's machines here, before anything is asked.
        let Ok(owner) = self.owner() else {
            self.set_unavailable(Some(Unavailable::SignedOut));
            return Ok(());
        };
        let gateway = self.provider(owner)?;
        let listed = match gateway.list().await {
            Ok(listed) => listed,
            Err(error) => {
                match &error {
                    CloudError::Api { status: 403, .. } => self.revoke(owner, Unavailable::NotAllowed),
                    _ => self.refused(owner, &error),
                }
                return Err(error.to_string());
            }
        };
        self.while_owner(owner, || self.apply_list(owner, &listed))
            .ok_or_else(|| CHANGED.to_string())
    }

    /// The machines Gateway listed for `owner`, under [`Inner::while_owner`]: see [`Inner::sync`].
    fn apply_list(&self, owner: Owner, listed: &[GatewayMachine]) {
        self.set_unavailable(None);
        let ids: HashSet<&str> = listed.iter().map(|m| m.id.as_str()).collect();
        let mut removed = Vec::new();
        for machine in self.cloud_machines() {
            if ids.contains(machine.id.as_str()) {
                continue;
            }
            let Some(cloud) = &machine.cloud else { continue };
            eprintln!(
                "nexus-desktop: cloud machine {} is no longer on Gateway: forgetting it here",
                machine.id
            );
            if self.forget(&machine.id, cloud).is_ok() {
                removed.push(machine.id.clone());
            }
        }
        let mut added = Vec::new();
        for machine in listed {
            if self.setting(&machine.id).is_some() {
                let creating = self.live_get(&machine.id).is_some_and(|live| live.creating());
                if !creating {
                    self.apply_gateway(owner, machine);
                }
                continue;
            }
            let Ok(setting) = self.add(machine, None) else { continue };
            if let Some(cloud) = &setting.cloud {
                self.resume(owner, &setting.id, cloud.state);
            }
            added.push(self.view(setting));
        }
        if !added.is_empty() || !removed.is_empty() {
            self.env
                .emit("cloud:machines", json!({ "added": added, "removed": removed }));
        }
    }

    /// Records the provider's state and tells the webview when it changed.
    fn set_state(&self, id: &str, state: CloudState, error: Option<String>, sleeping_since: Option<i64>) {
        let mut changed = false;
        let after = self.update_cloud(id, |cloud| {
            let since = match state {
                CloudState::Sleeping => sleeping_since.or(cloud.sleeping_since).or(Some(now_ms())),
                _ => None,
            };
            let queued_until = cloud.queued_until.filter(|_| state == CloudState::Queued);
            changed = cloud.state != state
                || cloud.error != error
                || cloud.sleeping_since != since
                || cloud.queued_until != queued_until;
            cloud.queued_until = queued_until;
            cloud.state = state;
            cloud.error = error;
            cloud.sleeping_since = since;
        });
        if changed && let Some(cloud) = after {
            self.env.emit(
                "cloud:state",
                json!({ "machine": id, "cloud": CloudInfo::from(&cloud) }),
            );
        }
    }

    /// Says a step of machine `id` on `cloud:progress`. A machine this device does not have (forgotten, or a sign-in
    /// the person left) says nothing.
    fn progress(&self, id: &str, step: CloudStep, detail: Option<&str>, error: Option<&str>) {
        if self.setting(id).is_none() {
            return;
        }
        let progress = CloudProgress {
            machine: id.to_string(),
            step,
            detail: detail.map(String::from),
            error: error.map(String::from),
        };
        if let Some(live) = self.live_get(id) {
            if let Ok(mut last) = live.step.lock() {
                *last = Some(step);
            }
            if let Ok(mut last) = live.progress.lock() {
                *last = Some(progress.clone());
            }
        }
        self.env
            .emit("cloud:progress", serde_json::to_value(&progress).unwrap_or_default());
    }

    /// A step failed: said on `cloud:progress`, kept for a call waiting on it.
    fn fail(&self, id: &str, step: CloudStep, error: impl Into<String>) -> String {
        let error = error.into();
        self.progress(id, step, None, Some(&error));
        if let Some(live) = self.live_get(id) {
            live.set_failed(Some(error.clone()));
        }
        error
    }

    /// [`Inner::fail`] for a step of sign-in `owner`: once another sign-in replaced it, nothing is said or kept (the
    /// same machine may be listed again for the new one), and the step only ends.
    fn fail_as(&self, owner: Owner, id: &str, step: CloudStep, error: impl Into<String>) -> String {
        if self.owns(owner) {
            self.fail(id, step, error)
        } else {
            CHANGED.to_string()
        }
    }

    /// Starts machine `id`'s relay for sign-in `owner`, under [`Inner::while_owner`]: its connects and events stop
    /// counting once another sign-in replaced it.
    fn start_remote(&self, owner: Owner, id: &str, live: &Arc<Live>) {
        let link = Arc::new(MachineLink {
            inner: self.me.clone(),
            id: id.to_string(),
            owner,
        });
        let sink = Arc::new(CloudSink {
            inner: self.me.clone(),
            id: id.to_string(),
            owner,
            app: self.env.sink(id),
        });
        let _inside = self.runtime.enter();
        let remote = Remote::start(link, sink, self.options.backoff);
        if let Ok(mut slot) = live.remote.lock()
            && let Some(old) = slot.replace(remote)
        {
            old.close();
        }
    }

    fn reconnect_connected(&self) {
        let all: Vec<Arc<Live>> = self
            .live
            .lock()
            .map(|live| live.values().cloned().collect())
            .unwrap_or_default();
        for live in all {
            if live.connected()
                && let Some(remote) = live.remote()
            {
                remote.reconnect();
            }
        }
    }

    fn spawn_bring_up(&self, owner: Owner, id: &str, live: &Arc<Live>) {
        let weak = self.me.clone();
        let machine = id.to_string();
        let task = self.runtime.spawn(async move {
            if let Some(inner) = weak.upgrade() {
                inner.bring_up(owner, &machine).await;
            }
        });
        if let Ok(mut slot) = live.task.lock()
            && let Some(old) = slot.replace(task.abort_handle())
        {
            old.abort();
        }
    }

    /// A new machine (Gateway made it already, or queued it): this device's key, the wait through the queue (as long
    /// as it takes), the wait until it runs; then its relay.
    async fn bring_up(self: &Arc<Self>, owner: Owner, id: &str) {
        let Ok(cloud) = self.cloud_setting(id) else { return };
        let Some(live) = self.live_get(id) else { return };
        live.set_failed(None);
        let provider = match self.provider(owner) {
            Ok(provider) => provider,
            Err(error) => {
                self.fail_as(owner, id, CloudStep::Create, error);
                return;
            }
        };
        self.progress(
            id,
            CloudStep::Create,
            Some("Making this computer's key for the machine"),
            None,
        );
        if let Some(dir) = cloud.key.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(error) = keys::load_or_generate(&cloud.key).await {
            self.fail_as(owner, id, CloudStep::Create, error.to_string());
            return;
        }
        // Nexus has no room yet: Gateway makes it when there is. No deadline here; the session shows the wait.
        loop {
            let machine = match provider.machine(id).await {
                Ok(machine) => machine,
                Err(error) if error.is_retryable() => {
                    tokio::time::sleep(self.options.poll.min(Duration::from_secs(5))).await;
                    continue;
                }
                Err(error) => {
                    self.refused(owner, &error);
                    self.fail_as(owner, id, CloudStep::Create, error.to_string());
                    return;
                }
            };
            if self
                .while_owner(owner, || self.apply_gateway(owner, &machine))
                .is_none()
            {
                return;
            }
            if machine.state != "queued" {
                break;
            }
            self.progress(id, CloudStep::Create, Some("Waiting for a machine"), None);
            tokio::time::sleep(self.options.poll.min(Duration::from_secs(5))).await;
        }
        self.progress(id, CloudStep::Start, Some("Starting the machine"), None);
        let ready = wait_ready(&*provider, id, WAKE_DEADLINE, |state| {
            let (state, error) = from_provider(state);
            if self.owns(owner) {
                self.set_state(id, state, error, None);
            }
        })
        .await;
        if let Err(error) = ready {
            self.refused(owner, &error);
            self.fail_as(owner, id, CloudStep::Start, error.to_string());
            return;
        }
        self.while_owner(owner, || {
            live.asleep.store(false, Ordering::SeqCst);
            live.wake.store(true, Ordering::SeqCst);
            match live.remote() {
                Some(remote) => {
                    remote.resume();
                    remote.reconnect();
                }
                None => self.start_remote(owner, id, &live),
            }
        });
    }

    /// Puts the Claude token saved here on Gateway, once per save: a token saved before Gateway kept one moves there
    /// on the next connect. True when Gateway keeps it. False with no token here, or when this Gateway keeps none.
    async fn claude_to_gateway(&self, owner: Owner) -> bool {
        let Ok(secrets) = self.secrets(owner) else {
            return false;
        };
        let Some(token) = secrets.claude else {
            return false;
        };
        if secrets.account.claude_on_gateway {
            return true;
        }
        let Ok(gateway) = self.provider(owner) else {
            return false;
        };
        if !matches!(gateway.put_claude_token(&token).await, Ok(true)) {
            return false;
        }
        let _ = self.change_secret(
            owner,
            |_| Ok(()),
            |account| account.claude_on_gateway = !account.claude_cleared,
        );
        true
    }

    /// Removes the Claude token from Gateway after it was removed here. Until Gateway confirms, each connect asks
    /// again.
    async fn claude_off_gateway(&self, owner: Owner) {
        let pending = self
            .secrets(owner)
            .is_ok_and(|secrets| secrets.account.claude_cleared && secrets.account.claude_on_gateway);
        if !pending {
            return;
        }
        let Ok(gateway) = self.provider(owner) else {
            return;
        };
        if gateway.delete_claude_token().await.is_ok() {
            let _ = self.change_secret(owner, |_| Ok(()), |account| account.claude_on_gateway = false);
        }
    }

    /// What goes on sign-in `owner`'s machine at this connect: the Claude token (none when Gateway keeps it, and the
    /// machine's host writes it; else when it saved one here; else the machine's stays, unless it cleared it here),
    /// the browser, git's identity, its GitHub token, and what the agents
    /// are told about the machine. Another sign-in's tokens never go.
    async fn provision_for(&self, owner: Owner) -> Result<Provision, String> {
        let secrets = self.secrets(owner)?;
        let git = |key: &'static str| async move {
            let mut git = async_command("git");
            git.args(["config", "--global", key])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let output = tokio::time::timeout(Duration::from_secs(5), git.output())
                .await
                .ok()?
                .ok()?;
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (output.status.success() && !value.is_empty()).then_some(value)
        };
        // Gateway keeps the Claude token: the machine's host writes it into agent.env itself, so this
        // connect leaves that file alone. A Gateway that keeps none gets this device's, as before.
        let cleared = secrets.account.claude_cleared;
        let on_gateway = if cleared {
            self.claude_off_gateway(owner).await;
            false
        } else {
            self.claude_to_gateway(owner).await
        };
        Ok(Provision {
            agent_env: if on_gateway { None } else { agent_env(secrets.claude) },
            keep_agent_env: on_gateway || !cleared,
            mcp_json: Some(playwright_config()),
            git_name: git("user.name").await,
            git_email: git("user.email").await,
            github_token: secrets.github,
            claude_md: Some(AGENT_INSTRUCTIONS.into()),
        })
    }

    /// A connect (the relay's [`Link::reach_host`]) for sign-in `owner`: through Gateway when the machine's host is
    /// online there ([`Inner::relay`]), else over SSH ([`Inner::open`]).
    async fn reach(self: &Arc<Self>, owner: Owner, id: &str) -> Result<Reached, OpenError> {
        let live = self.live_get(id).ok_or_else(|| "No such cloud machine.".to_string())?;
        if live.way() != Way::Ssh {
            match self.relay(owner, id, &live).await? {
                Some(opened) => return Ok(Reached::Socket(opened)),
                None => live.set_way(Way::Ssh),
            }
        }
        self.open(owner, id).await.map(Reached::Stream)
    }

    /// A failed step with Gateway's error: said and kept as [`Inner::fail_as`] does, with whether waiting may fix it.
    fn stop(&self, owner: Owner, id: &str, step: CloudStep, error: CloudError) -> OpenError {
        OpenError {
            heals: error.heals(),
            message: self.fail_as(owner, id, step, error.to_string()),
        }
    }

    /// Whether a connect may go on with the machine in `machine`'s state. Gone, its disk expired or failed: said, and
    /// its relay held. Asleep with no wake asked: the relay waits for one instead of waking it.
    fn check_state(&self, owner: Owner, id: &str, live: &Live, machine: &Machine) -> Result<(), OpenError> {
        let waking = live.wake.load(Ordering::SeqCst);
        match &machine.state {
            ProviderState::Gone => {
                live.park();
                self.set_state(id, CloudState::Gone, None, None);
                Err(self.stop(owner, id, CloudStep::Start, CloudError::NotFound))
            }
            ProviderState::Sleeping if machine.data_expired_at.is_some() => {
                live.park();
                let error = CloudError::DataExpired.to_string();
                self.set_state(id, CloudState::Failed, Some(error), None);
                Err(self.stop(owner, id, CloudStep::Start, CloudError::DataExpired))
            }
            ProviderState::Failed { message } => {
                live.park();
                self.set_state(id, CloudState::Failed, Some(message.clone()), None);
                Err(OpenError {
                    heals: false,
                    message: self.fail_as(
                        owner,
                        id,
                        CloudStep::Start,
                        format!("The cloud machine failed: {message}"),
                    ),
                })
            }
            ProviderState::Sleeping | ProviderState::Stopping if !waking => {
                live.asleep.store(true, Ordering::SeqCst);
                live.park();
                let (state, _) = from_provider(&machine.state);
                let since = machine.stopped_at.as_deref().and_then(parse_time);
                self.set_state(id, state, None, since);
                Err(ASLEEP.to_string().into())
            }
            _ => Ok(()),
        }
    }

    /// Says a wake's [`Progress`] for machine `id` while `owner` is the sign-in.
    fn progress_of(&self, owner: Owner, id: &str) -> Arc<dyn Fn(Progress) + Send + Sync> {
        let weak = self.me.clone();
        let machine_id = id.to_string();
        Arc::new(move |progress| {
            let Some(inner) = weak.upgrade().filter(|inner| inner.owns(owner)) else {
                return;
            };
            match progress {
                Progress::Waking => {
                    inner.progress(&machine_id, CloudStep::Start, Some("Waking the machine"), None);
                    inner.set_state(&machine_id, CloudState::Starting, None, None);
                }
                Progress::Starting => {
                    inner.progress(&machine_id, CloudStep::Start, Some("Starting the machine"), None);
                    inner.set_state(&machine_id, CloudState::Starting, None, None);
                }
                Progress::Preparing => inner.progress(
                    &machine_id,
                    CloudStep::Ssh,
                    Some("Letting this computer in over SSH"),
                    None,
                ),
            }
        })
    }

    /// The connect through Gateway (the relay): the machine woken when asked, its host waited for
    /// at Gateway (`hostOnline`, [`HOST_ONLINE_WAIT`]), then one WebSocket to `…/connect`, with `host_provision` as the
    /// relay's first call. None: reach it over SSH instead (a Gateway that does not say, or a host that did not come
    /// online in time on the first connect after a wake).
    async fn relay(
        self: &Arc<Self>,
        owner: Owner,
        id: &str,
        live: &Arc<Live>,
    ) -> Result<Option<SocketOpened>, OpenError> {
        let _gate = live.gate.lock().await;
        self.still(owner)?;
        if live.asleep.load(Ordering::SeqCst) {
            live.park();
            return Err(ASLEEP.to_string().into());
        }
        let provider = self
            .provider(owner)
            .map_err(|e| self.fail_as(owner, id, CloudStep::Start, e))?;
        let machine = provider.machine(id).await.map_err(|e| {
            self.refused(owner, &e);
            self.stop(owner, id, CloudStep::Start, e)
        })?;
        if machine.host_online.is_none() {
            return Ok(None);
        }
        self.still(owner)?;
        self.check_state(owner, id, live, &machine.machine())?;
        let mut online = machine.host_online == Some(true) && machine.state == "ready";
        if machine.state != "ready" {
            let progress = self.progress_of(owner, id);
            wake_up(&*provider, id, WAKE_DEADLINE, &*progress).await.map_err(|e| {
                self.refused(owner, &e);
                self.stop(owner, id, CloudStep::Start, e)
            })?;
        }
        if !online {
            self.progress(id, CloudStep::Start, Some("Waiting for Nexus on the machine"), None);
            let wait = Duration::from_millis(self.host_wait.load(Ordering::SeqCst));
            let started = Instant::now();
            while !online && started.elapsed() < wait {
                tokio::time::sleep(crate::POLL).await;
                self.still(owner)?;
                match provider.machine(id).await {
                    Ok(machine) => online = machine.host_online == Some(true),
                    Err(error) if error.heals() => {}
                    Err(error) => {
                        self.refused(owner, &error);
                        return Err(self.stop(owner, id, CloudStep::Start, error));
                    }
                }
            }
        }
        if !online {
            if live.way() == Way::Relay {
                return Err(OpenError {
                    message: "The machine's Nexus host is not online yet.".into(),
                    heals: true,
                });
            }
            return Ok(None);
        }
        live.set_way(Way::Relay);
        self.while_owner(owner, || {
            live.wake.store(false, Ordering::SeqCst);
            self.set_state(id, CloudState::Ready, None, None);
        })
        .ok_or_else(|| CHANGED.to_string())?;
        self.progress(id, CloudStep::Connect, Some("Connecting through Nexus"), None);
        // A person's own server is not a cloud machine: the person's credentials and the cloud
        // instructions stay off it.
        let setup = match machine.provider.as_str() {
            "own" => None,
            _ => Some(self.provision_for(owner).await?.host_provision()),
        };
        self.still(owner)?;
        let link = provider
            .gateway()
            .and_then(|gateway| gateway.connect(id))
            .map_err(|e| self.stop(owner, id, CloudStep::Connect, e))?;
        let socket = nexus_remote::relay::dial(&link.url, &link.headers)
            .await
            .map_err(|error| match error.heals() {
                true => OpenError::from(error),
                false => OpenError {
                    heals: false,
                    message: self.fail_as(owner, id, CloudStep::Connect, error.message),
                },
            })?;
        // A connection opened for a sign-in the person left is dropped here, unused.
        self.still(owner)?;
        Ok(Some(SocketOpened {
            socket,
            token: String::new(),
            setup: setup
                .map(|setup| {
                    (
                        nexus_workbench::provision::CALL.to_string(),
                        serde_json::to_value(setup).unwrap_or_default(),
                    )
                })
                .into_iter()
                .collect(),
        }))
    }

    /// Machine `id`'s SSH address when it was reached through Gateway: its key registered (`POST …/access`) and its
    /// host key pinned, for what needs SSH.
    async fn ssh_target(&self, owner: Owner, id: &str, live: &Live) -> Result<Target, String> {
        let cloud = self.cloud_setting(id)?;
        let provider = self.provider(owner)?;
        let key = keys::load_or_generate(&cloud.key).await.map_err(String::from)?;
        self.still(owner)?;
        let target = self.cloud_link(provider, id, key, &cloud).reach().await.map_err(|e| {
            self.refused(owner, &e);
            e.to_string()
        })?;
        self.while_owner(owner, || {
            if let Ok(mut slot) = live.target.lock() {
                *slot = Some(target.clone());
            }
        })
        .ok_or_else(|| CHANGED.to_string())?;
        Ok(target)
    }

    /// A connect over SSH for sign-in `owner`: the machine woken only when asked, reached, prepared, its host
    /// installed and started, then its stream. It stops at the next step once another sign-in replaced `owner`.
    async fn open(self: &Arc<Self>, owner: Owner, id: &str) -> Result<Opened, OpenError> {
        let live = self.live_get(id).ok_or_else(|| "No such cloud machine.".to_string())?;
        let _gate = live.gate.lock().await;
        self.still(owner)?;
        if live.asleep.load(Ordering::SeqCst) {
            live.park();
            return Err(ASLEEP.to_string().into());
        }
        let cloud = self.cloud_setting(id)?;
        let provider = self
            .provider(owner)
            .map_err(|e| self.fail_as(owner, id, CloudStep::Start, e))?;
        let machine = provider.get(id).await.map_err(|e| {
            self.refused(owner, &e);
            self.stop(owner, id, CloudStep::Start, e)
        })?;
        self.still(owner)?;
        self.check_state(owner, id, &live, &machine)?;
        let key = keys::load_or_generate(&cloud.key)
            .await
            .map_err(|e| self.stop(owner, id, CloudStep::Ssh, e))?;
        self.still(owner)?;
        let mut link = self.cloud_link(provider.clone(), id, key, &cloud);
        link.progress = self.progress_of(owner, id);
        let target = match link.reach().await {
            Ok(target) => target,
            Err(error) => {
                if matches!(error, CloudError::DataExpired | CloudError::NotFound) && self.owns(owner) {
                    live.park();
                    let (state, message) = match error {
                        CloudError::NotFound => (CloudState::Gone, None),
                        _ => (CloudState::Failed, Some(error.to_string())),
                    };
                    self.set_state(id, state, message, None);
                }
                let step = live.step.lock().ok().and_then(|s| *s).unwrap_or(CloudStep::Start);
                let step = if matches!(step, CloudStep::Start | CloudStep::Ssh) {
                    step
                } else {
                    CloudStep::Start
                };
                return Err(self.stop(owner, id, step, error));
            }
        };
        // Kept only while `owner` is the sign-in: an end of it forgets the address.
        self.while_owner(owner, || {
            live.wake.store(false, Ordering::SeqCst);
            self.set_state(id, CloudState::Ready, None, None);
            if let Ok(mut slot) = live.target.lock() {
                *slot = Some(target.clone());
            }
        })
        .ok_or_else(|| CHANGED.to_string())?;
        self.progress(id, CloudStep::Setup, Some("Preparing the machine"), None);
        let prepare = self.provision_for(owner).await?;
        self.still(owner)?;
        let fields = self
            .env
            .provision(&target, &prepare)
            .await
            .map_err(|e| self.stop(owner, id, CloudStep::Setup, e))?;
        self.still(owner)?;
        if fields.get("github").map(String::as_str) == Some("failed") {
            self.progress(
                id,
                CloudStep::Setup,
                Some("GitHub did not take the saved sign-in on the machine; connect GitHub again in Settings"),
                None,
            );
        }
        if fields.get("playwright").map(String::as_str) == Some("failed") {
            self.progress(
                id,
                CloudStep::Setup,
                Some("The browser for the agents did not install; sessions start without it"),
                None,
            );
        }
        self.progress(id, CloudStep::Install, Some("Installing and starting nexus-host"), None);
        let opened = self
            .env
            .open_host(Installer {
                target,
                source: self.env.source(),
                fetch: self.env.fetch(),
            })
            .await
            .map_err(|e| self.fail_as(owner, id, CloudStep::Install, e))?;
        // A stream opened for a sign-in the person left is dropped here, unused.
        self.still(owner)?;
        self.progress(id, CloudStep::Connect, Some("Connecting"), None);
        Ok(opened)
    }

    /// The link to machine `id` through Gateway: every reach registers this device's key (`POST …/access`).
    fn cloud_link(&self, provider: Arc<Owned>, id: &str, key: KeyPair, cloud: &CloudSetting) -> CloudLink {
        let provider: Arc<dyn Provider> = provider;
        let mut link = CloudLink::new(provider, id, key, cloud.known_hosts.clone(), self.env.source());
        link.fetch = self.env.fetch();
        link
    }

    /// "Update now" over the link for sign-in `owner`: refused while asleep (the update would wake it).
    async fn update(self: &Arc<Self>, owner: Owner, id: &str) -> Result<(), String> {
        let live = self.live_get(id).ok_or_else(|| "No such cloud machine.".to_string())?;
        let _gate = live.gate.lock().await;
        self.still(owner)?;
        if live.asleep.load(Ordering::SeqCst) {
            return Err(ASLEEP.into());
        }
        let cloud = self.cloud_setting(id)?;
        let provider = self.provider(owner)?;
        let key = keys::load_or_generate(&cloud.key).await.map_err(String::from)?;
        self.still(owner)?;
        self.cloud_link(provider, id, key, &cloud).update().await
    }

    /// Asks Gateway for machine `id` now as sign-in `owner`; returns its state. None when it did not answer, or the
    /// sign-in changed.
    async fn refresh(&self, owner: Owner, id: &str) -> Option<CloudState> {
        self.cloud_setting(id).ok()?;
        match self.provider(owner).ok()?.machine(id).await {
            Ok(machine) => self.while_owner(owner, || self.apply_gateway(owner, &machine)),
            Err(CloudError::NotFound) => {
                let gone = Machine {
                    id: id.to_string(),
                    name: String::new(),
                    state: ProviderState::Gone,
                    provider_state: "gone".into(),
                    size: None,
                    labels: Default::default(),
                    stopped_at: None,
                    data_expired_at: None,
                };
                self.while_owner(owner, || self.apply(owner, id, &gone))
            }
            Err(error) => {
                self.refused(owner, &error);
                None
            }
        }
    }

    /// [`Inner::apply`] with what only Gateway says: the session's title, the queue, why it stopped. Under
    /// [`Inner::while_owner`].
    fn apply_gateway(&self, owner: Owner, machine: &GatewayMachine) -> CloudState {
        let queued = machine.state == "queued";
        let mut changed = false;
        let after = self.update_cloud(&machine.id, |cloud| {
            let queued_until = machine.queued_until.filter(|_| queued);
            let title = machine.title.clone().or(cloud.title.take());
            changed = cloud.queued_until != queued_until
                || cloud.stop_reason != machine.stop_reason
                || cloud.title != title
                || (queued && cloud.state != CloudState::Queued);
            cloud.queued_until = queued_until;
            cloud.stop_reason = machine.stop_reason.clone();
            cloud.title = title;
            if machine.session_id.is_some() {
                cloud.session_id = machine.session_id.clone();
            }
            if queued {
                cloud.state = CloudState::Queued;
            }
        });
        if changed && let Some(cloud) = after {
            self.env.emit(
                "cloud:state",
                json!({ "machine": machine.id, "cloud": CloudInfo::from(&cloud) }),
            );
        }
        if queued {
            return CloudState::Queued;
        }
        self.apply(owner, &machine.id, &machine.machine())
    }

    /// Records the machine's state as Gateway says it. A machine that fell asleep by itself has its relay held, a
    /// gone one too. Under [`Inner::while_owner`].
    fn apply(&self, owner: Owner, id: &str, machine: &Machine) -> CloudState {
        let live = self.live_of(id);
        let (state, error) = from_provider(&machine.state);
        let waking = live.wake.load(Ordering::SeqCst);
        match state {
            CloudState::Sleeping | CloudState::Stopping if !waking => {
                if !live.asleep.swap(true, Ordering::SeqCst) {
                    live.park();
                    if let Ok(mut target) = live.target.lock() {
                        *target = None;
                    }
                }
            }
            CloudState::Gone | CloudState::Failed => live.park(),
            // Gateway brought it back by itself (a restart on a bigger size, or another device woke it): the relay
            // connects again, so the session can go on.
            CloudState::Ready if live.asleep.load(Ordering::SeqCst) && !live.slept_lately() => {
                live.asleep.store(false, Ordering::SeqCst);
                match live.remote() {
                    Some(remote) => {
                        remote.resume();
                        remote.reconnect();
                    }
                    None => self.start_remote(owner, id, &live),
                }
            }
            _ => {}
        }
        // A wake or a create in progress reports its own states.
        if !(waking && matches!(state, CloudState::Sleeping | CloudState::Stopping)) {
            let error = match (state, machine.data_expired_at.is_some()) {
                (CloudState::Sleeping, true) => Some(CloudError::DataExpired.to_string()),
                _ => error,
            };
            let since = machine.stopped_at.as_deref().and_then(parse_time);
            self.set_state(id, state, error, since);
        }
        state
    }
}

/// Gateway's cloud machines as one sign-in ([`Inner::provider`]): every call is refused once another sign-in replaced
/// it. A call already on its way finishes.
struct Owned {
    inner: Weak<Inner>,
    owner: Owner,
    gateway: GatewayCloud,
}

impl Owned {
    fn gateway(&self) -> Result<&GatewayCloud, CloudError> {
        match self.inner.upgrade() {
            Some(inner) if inner.owns(self.owner) => Ok(&self.gateway),
            _ => Err(CloudError::Other(CHANGED.into())),
        }
    }

    async fn list(&self) -> Result<Vec<GatewayMachine>, CloudError> {
        self.gateway()?.list().await
    }

    async fn create_machine(
        &self,
        session_id: &str,
        title: &str,
        repo: Option<&str>,
        github: Option<&str>,
    ) -> Result<GatewayMachine, CloudError> {
        self.gateway()?.create_machine(session_id, title, repo, github).await
    }

    async fn machine(&self, id: &str) -> Result<GatewayMachine, CloudError> {
        self.gateway()?.machine(id).await
    }

    async fn put_claude_token(&self, token: &str) -> Result<bool, CloudError> {
        self.gateway()?.put_claude_token(token).await
    }

    async fn delete_claude_token(&self) -> Result<(), CloudError> {
        self.gateway()?.delete_claude_token().await
    }
}

impl Provider for Owned {
    fn create(&self, spec: crate::Spec) -> BoxFuture<'_, crate::Result<String>> {
        Box::pin(async move { self.gateway()?.create(spec).await })
    }

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, crate::Result<Machine>> {
        Box::pin(async move { self.gateway()?.get(id).await })
    }

    fn sleep<'a>(&'a self, id: &'a str) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move { self.gateway()?.sleep(id).await })
    }

    fn wake<'a>(&'a self, id: &'a str, size: Option<crate::Size>) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move { self.gateway()?.wake(id, size).await })
    }

    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move { self.gateway()?.delete(id).await })
    }

    fn authorize_ssh<'a>(&'a self, id: &'a str, public_key: &'a str) -> BoxFuture<'a, crate::Result<crate::SshAccess>> {
        Box::pin(async move { self.gateway()?.authorize_ssh(id, public_key).await })
    }

    fn preview<'a>(&'a self, id: &'a str, port: u16, public: bool) -> BoxFuture<'a, crate::Result<String>> {
        Box::pin(async move { self.gateway()?.preview(id, port, public).await })
    }

    fn desktop<'a>(&'a self, id: &'a str) -> BoxFuture<'a, crate::Result<String>> {
        Box::pin(async move { self.gateway()?.desktop(id).await })
    }

    fn exec<'a>(
        &'a self,
        id: &'a str,
        command: &'a str,
        timeout: Duration,
    ) -> BoxFuture<'a, crate::Result<crate::ExecOutput>> {
        Box::pin(async move { self.gateway()?.exec(id, command, timeout).await })
    }

    fn mint_stop_key<'a>(&'a self, id: &'a str) -> BoxFuture<'a, crate::Result<Option<String>>> {
        Box::pin(async move { self.gateway()?.mint_stop_key(id).await })
    }
}

/// A cloud machine's link for its relay: [`Inner::reach`] on every connect, for the sign-in that started it.
struct MachineLink {
    inner: Weak<Inner>,
    id: String,
    owner: Owner,
}

impl Link for MachineLink {
    fn open(&self) -> BoxFuture<'_, nexus_remote::Result<Opened>> {
        Box::pin(async move {
            let inner = self.inner.upgrade().ok_or_else(|| "The app is quitting.".to_string())?;
            inner.open(self.owner, &self.id).await.map_err(|error| error.message)
        })
    }

    fn reach_host(&self) -> BoxFuture<'_, Result<Reached, OpenError>> {
        Box::pin(async move {
            let inner = self.inner.upgrade().ok_or_else(|| "The app is quitting.".to_string())?;
            inner.reach(self.owner, &self.id).await
        })
    }

    fn update(&self) -> BoxFuture<'_, nexus_remote::Result<()>> {
        Box::pin(async move {
            let inner = self.inner.upgrade().ok_or_else(|| "The app is quitting.".to_string())?;
            inner.update(self.owner, &self.id).await
        })
    }
}

/// The app's sink, plus the last steps of `cloud:progress`: connected is `ready`, a failed hello is `connect`'s error.
/// It passes nothing on once the sign-in that started the relay is not the app's: an event already on its
/// way when the person signed out goes nowhere.
struct CloudSink {
    inner: Weak<Inner>,
    id: String,
    owner: Owner,
    app: Arc<dyn Sink>,
}

impl CloudSink {
    fn owned(&self) -> Option<Arc<Inner>> {
        self.inner.upgrade().filter(|inner| inner.owns(self.owner))
    }
}

impl Sink for CloudSink {
    fn push(&self, push: &Pushed) {
        if self.owned().is_some() {
            self.app.push(push);
        }
    }

    fn status(&self, status: &Status) {
        let Some(inner) = self.owned() else { return };
        self.app.status(status);
        match status {
            Status::Connected { host, .. } => {
                if let Some(live) = inner.live_get(&self.id) {
                    live.set_failed(None);
                }
                let detail = format!("nexus-host {} on {}", host.version, host.machine);
                inner.progress(&self.id, CloudStep::Ready, Some(&detail), None);
            }
            Status::Unreachable { error, .. } => {
                let last = inner
                    .live_get(&self.id)
                    .and_then(|l| l.step.lock().ok().and_then(|s| *s));
                if last == Some(CloudStep::Connect) {
                    inner.fail(&self.id, CloudStep::Connect, error.clone());
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
