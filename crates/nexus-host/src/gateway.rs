//! The host's [`Host`] and [`Machine`]: agents start through Nexus Gateway the way the desktop app starts them
//! (nexus-client's `agents` module), with the host's own device token
//! (a private plan doc, section 4). What the desktop reads from its settings and keychain (MCP servers, extra
//! plan accounts) is not on a host in v1, except what the files in the host's folder add ([`crate::agent_files`]);
//! notifications are the app's, from the pushed events.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use nexus_client::agent_setup::subscriptions_allowed;
use nexus_client::agents::{self, LaunchOptions, Route};
use nexus_client::commit_trailer::{self, Surface};
use nexus_client::{Client, ClientOptions};
use nexus_workbench::agent_launch::{AutoContext, Catalog, LocalLaunch, admit_local_launch, load_catalog, model_key};
use nexus_workbench::api::{AgentInfo, Machine};
use nexus_workbench::driver::label;
use nexus_workbench::event::{SessionAgent, Usage, Via};
use nexus_workbench::nexus_runtime;
use nexus_workbench::process::{self, Shell, Spawn};
use nexus_workbench::provider::LocalProvidersOptIn;
use nexus_workbench::session_scope::SessionScope;
use nexus_workbench::{Emit, Host, Launch, LaunchRequest, ProcessEnd, accounts};
use serde_json::json;

use crate::agent_files::AgentFiles;
use crate::server::Hub;

/// OpenCode reads its Gateway model from `OPENCODE_CONFIG_CONTENT` rather than a CLI flag, so a requested model must
/// still reach `LaunchOptions.model`. Other agents keep `None` when `request.model` is set (so Antigravity and Cursor
/// never get a `--model` flag spliced onto their ACP args).
fn launch_model<'a>(
    agent: nexus_client::agent_entries::Agent,
    requested: Option<&str>,
    resolved: Option<&'a str>,
) -> Option<&'a str> {
    if requested.is_some() && agent != nexus_client::agent_entries::Agent::Opencode {
        None
    } else {
        resolved
    }
}

/// `x-nexus-client` on the host's Gateway calls (the audit log's surface).
pub fn client_options(gateway: &str, token: Option<String>) -> ClientOptions {
    ClientOptions {
        api_key: token,
        client: Some(format!("desktop-host/{}", crate::VERSION)),
        user_agent: Some(format!("nexus-host/{}", crate::VERSION)),
        ..ClientOptions::new(gateway)
    }
}

/// How long a local plan probe stays fresh for Auto routing before re-probing.
const PLANS_CACHE_TTL: Duration = Duration::from_secs(120);

type PlansCache = Option<(Instant, Vec<nexus_workbench::plans::PlanUsage>)>;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The environment agents start with: the host's, with a `PATH` that also has where agents are usually installed
/// (`~/.local/bin`, npm's global folder). The login shell's `PATH` is asked once; a plain `shell` (tests) asks no shell.
fn agent_env(shell: &Shell) -> HashMap<String, String> {
    static PATH: OnceLock<String> = OnceLock::new();
    let mut env: HashMap<String, String> = std::env::vars().collect();
    let home = env.get("HOME").map(PathBuf::from);
    let current = std::env::var("PATH").ok();
    let path = match shell {
        Shell::Login => PATH
            .get_or_init(|| process::agent_path(current.as_deref(), home.as_deref(), shell))
            .clone(),
        Shell::Plain(_) => process::agent_path(current.as_deref(), home.as_deref(), shell),
    };
    env.insert("PATH".into(), path);
    env
}

fn find(agent: SessionAgent, shell: &Shell) -> Option<PathBuf> {
    let env = agent_env(shell);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    nexus_workbench::agent_launch::find(agent, &env, &cwd)
}

/// A server's own plan readings: each installed agent's own login and its limits, as the agent reports them, with
/// the environment it would bill elsewhere with left out. A server keeps no extra accounts. Rotation and Auto read it
/// ([`crate::local::set_plans`]), so Auto on a server knows which of its plan's models have room.
pub fn host_plans(shell: Shell) -> crate::local::PlanProbe {
    Arc::new(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return Vec::new();
        };
        let probes: Vec<_> = SessionAgent::ALL
            .into_iter()
            .filter(|agent| nexus_workbench::harness::harness(*agent).plan_payer().is_some())
            .filter_map(|agent| {
                let program = find(agent, &shell)?;
                let mut env = agent_env(&shell);
                for name in agent
                    .coding()
                    .map_or(&[][..], |coding| coding.entry().billing.plan_overrides)
                {
                    env.remove(*name);
                }
                nexus_workbench::harness::harness(agent).read_plan(
                    nexus_workbench::harness::PlanProbe {
                        program,
                        env,
                        account: None,
                        dir: None,
                        cwd: None,
                    },
                    nexus_workbench::harness::PlanDepth::Limits,
                )
            })
            .collect();
        runtime.block_on(futures::future::join_all(probes))
    })
}

/// `<path> --version`, once per path, giving up after five seconds.
fn version(path: &Path, shell: &Shell) -> Option<String> {
    static VERSIONS: OnceLock<std::sync::Mutex<HashMap<PathBuf, Option<String>>>> = OnceLock::new();
    let versions = VERSIONS.get_or_init(Default::default);
    if let Some(known) = versions.lock().ok().and_then(|v| v.get(path).cloned()) {
        return known;
    }
    let found = version_of(path, shell);
    if let Ok(mut versions) = versions.lock() {
        versions.insert(path.to_path_buf(), found.clone());
    }
    found
}

fn version_of(path: &Path, shell: &Shell) -> Option<String> {
    let mut version = process::command(path);
    version
        .arg("--version")
        .env("PATH", agent_env(shell).get("PATH").cloned().unwrap_or_default());
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

/// One call at these prices per million tokens, cache reads at a tenth of input and writes at 1.25 times.
fn list_price(input: f64, output: f64, usage: &Usage) -> f64 {
    let input_tokens =
        usage.input_tokens as f64 + usage.cache_read_tokens as f64 * 0.1 + usage.cache_write_tokens as f64 * 1.25;
    (input_tokens * input + usage.output_tokens as f64 * output) / 1_000_000.0
}

/// Starts agents through Gateway with the host's own token, and pushes what the workbench shows to the [`Hub`].
pub struct HeadlessHost {
    /// The sign-in agents start on. A server's comes from `nexus-host login`; the desktop app's own service is handed
    /// the app's ([`HeadlessHost::sign_in`]) and can be handed another (a sign-in to another organization).
    sign_in: Arc<RwLock<SignIn>>,
    /// The desktop app's service only: session sync with the app's device key ([`crate::local_sync`]).
    local_sync: crate::local_sync::LocalSync,
    /// Where Codex's catalog is written.
    data: PathBuf,
    hub: Hub,
    prices: Arc<RwLock<Catalog>>,
    plans_cache: Arc<RwLock<PlansCache>>,
    /// `agent.env` and `mcp.json` in the host's folder, read at each start ([`AgentFiles`]).
    files: Arc<AgentFiles>,
    /// Session sync's backend while this server is a trusted device ([`crate::session_sync`]); none, the default,
    /// syncs nothing.
    sync: Arc<crate::session_sync::HostSync>,
    /// The desktop app's service only: the built-in Nexus server its agents get ([`crate::app_relay`]).
    app_tools: OnceLock<Arc<crate::app_relay::AppTools>>,
    /// The shell the agents' `PATH` is asked of: the login shell, or `/bin/sh` in tests ([`Shell`]).
    shell: Shell,
    /// A server only: the native sessions it runs, and its hold on each ([`crate::session_runner`]).
    runners: OnceLock<Arc<crate::session_runner::SessionRunners>>,
    /// A remote server (`Mode::Server`), not this computer's background service (`Mode::Local`). A server refuses an
    /// agent whose sessions never run on a remote server (`SessionFeatures::local_only`).
    on_server: bool,
    local_providers_opt_in: LocalProvidersOptIn,
    program_override: OnceLock<PathBuf>,
}

/// One Gateway sign-in.
#[derive(Clone, Default)]
struct SignIn {
    /// None until signed in: every start then says to sign in.
    client: Option<Client>,
    gateway: String,
    token: Option<String>,
    /// The organization it is for, as sessions are scoped (`<gateway>|<org>`): a launch for another is refused, so a
    /// session never runs on this token in an organization it doesn't belong to.
    scope: SessionScope,
    local_account_pending: bool,
}

impl SignIn {
    fn check_saved_provider(&self, on_server: bool) -> Result<(), String> {
        if self.client.is_some() || self.token.is_some() || self.local_account_pending {
            return Err(nexus_workbench::provider::saved_provider_refusal(true));
        }
        if on_server || !self.scope.is_local() {
            return Err(nexus_workbench::provider::saved_provider_refusal(false));
        }
        Ok(())
    }

    fn local_launch<'a>(
        &self,
        request: &LaunchRequest,
        on_server: bool,
        mode: &'a LocalProvidersOptIn,
    ) -> Option<LocalLaunch<'a>> {
        if on_server || (self.local_account_pending && request.provider.is_some()) {
            return None;
        }
        let signed_in = self.client.is_some() || self.token.is_some();
        let keeps_subscription = self.client.is_some() && self.token.is_some() && request.provider.is_none();
        let scope = if keeps_subscription {
            &request.scope
        } else {
            &self.scope
        };
        if signed_in && !keeps_subscription {
            return None;
        }
        admit_local_launch(request, scope, mode)
    }

    fn new(gateway: &str, token: Option<String>, scope: SessionScope) -> SignIn {
        SignIn {
            client: token
                .as_ref()
                .map(|token| Client::new(client_options(gateway, Some(token.clone())))),
            gateway: gateway.to_string(),
            token,
            scope,
            local_account_pending: false,
        }
    }
}

impl HeadlessHost {
    /// What a repository's `.agents/resume` runs with: what an agent starts with (the host's environment, and the
    /// person's credentials from `agent.env`). None while `agent.env` has none, so the hook waits for the person's
    /// credentials and `.agents/setup` (which runs without them) never races them.
    pub fn resume_env(&self) -> Option<Vec<(String, String)>> {
        let personal = self.files.agent_env();
        if personal.is_empty() {
            return None;
        }
        let mut env = agent_env(&self.shell);
        env.extend(personal);
        Some(env.into_iter().collect())
    }

    /// `on_server`: a remote server, not the desktop app's own service ([`HeadlessHost::on_server`]).
    pub fn new(
        credentials: Option<&crate::files::Credentials>,
        data: PathBuf,
        hub: Hub,
        shell: Shell,
        on_server: bool,
        local_providers_opt_in: LocalProvidersOptIn,
    ) -> HeadlessHost {
        let (gateway, token) = match credentials {
            Some(c) => (c.gateway.clone(), Some(c.token.clone())),
            None => (String::new(), None),
        };
        let org = credentials.and_then(|c| c.org.clone()).unwrap_or_default();
        HeadlessHost {
            sign_in: Arc::new(RwLock::new(SignIn::new(
                &gateway,
                token,
                SessionScope::from(format!("{gateway}|{org}")),
            ))),
            local_sync: Default::default(),
            files: Arc::new(AgentFiles::new(data.clone())),
            data,
            hub,
            prices: Arc::default(),
            plans_cache: Arc::new(RwLock::new(None)),
            sync: Default::default(),
            app_tools: OnceLock::new(),
            shell,
            runners: OnceLock::new(),
            on_server,
            local_providers_opt_in,
            program_override: OnceLock::new(),
        }
    }

    /// Runs native sessions from now on: each agent of one starts with its runner's fence. A server only.
    pub fn set_session_runners(&self, runners: Arc<crate::session_runner::SessionRunners>) {
        let _ = self.runners.set(runners);
    }

    /// Gives the agents started from now on the built-in Nexus server. The desktop app's service only.
    pub fn set_app_tools(&self, tools: crate::app_relay::AppTools) {
        let _ = self.app_tools.set(Arc::new(tools));
    }

    pub fn app_tools(&self) -> Option<Arc<crate::app_relay::AppTools>> {
        self.app_tools.get().cloned()
    }

    /// What the service is signed in to now, for the app's socket: the scope and the sign-in's identity.
    pub fn signed_in(&self) -> (String, String) {
        let current = self.current();
        let identity = current
            .token
            .as_deref()
            .map(nexus_workbench::session_choice::sign_in_identity)
            .unwrap_or_default();
        (current.scope.into_text(), identity)
    }

    /// Session sync in the desktop app's service, with the device key of the app's sign-in.
    pub fn local_sync(&self) -> &crate::local_sync::LocalSync {
        &self.local_sync
    }

    /// Session sync's slot, which `serve` fills once this server's device is trusted.
    pub fn sync(&self) -> Arc<crate::session_sync::HostSync> {
        self.sync.clone()
    }

    fn current(&self) -> SignIn {
        self.sign_in.read().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn client(&self) -> Option<Client> {
        self.current().client
    }

    /// The scope of the sessions this host runs: its Gateway and organization, as the desktop app scopes them.
    pub fn scope(&self) -> String {
        self.session_scope().into_text()
    }

    pub(crate) fn session_scope(&self) -> SessionScope {
        self.current().scope
    }

    /// A local app peer may keep subscription access while its account's organization is still unknown.
    pub(crate) fn set_local_account_pending(&self, pending: bool) -> Result<(), String> {
        let mut sign_in = self.sign_in.write().map_err(|error| error.to_string())?;
        if self.on_server || !sign_in.scope.is_local() || sign_in.client.is_some() || sign_in.token.is_some() {
            return Err("This Nexus host is not in the local workspace.".into());
        }
        sign_in.local_account_pending = pending;
        Ok(())
    }

    /// Signs in with `token` at `gateway`, for the sessions of `scope`; agents started from now on use it. The desktop
    /// app's service only: a server signs in with its own device token (a private plan doc, section 4).
    pub fn sign_in(&self, gateway: &str, token: &str, scope: &str) {
        let gateway = gateway.trim_end_matches('/');
        let next = SignIn::new(gateway, Some(token.to_string()), SessionScope::from(scope));
        if let Ok(mut current) = self.sign_in.write() {
            *current = next;
        }
    }
}

impl Host for HeadlessHost {
    fn local_providers_opt_in(&self) -> &LocalProvidersOptIn {
        &self.local_providers_opt_in
    }

    fn shows_account_free_sessions(&self) -> bool {
        !self.on_server && self.local_providers_opt_in.enabled()
    }

    fn saved_server_models(&self, provider: &nexus_workbench::provider::InstanceId) -> Result<Vec<String>, String> {
        nexus_desktop_core::provider_settings::saved_server_models(provider)
    }

    fn while_saved_provider_allowed(&self, action: &mut dyn FnMut()) -> Result<(), String> {
        let sign_in = self.sign_in.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        sign_in.check_saved_provider(self.on_server)?;
        action();
        Ok(())
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
        let shell = self.shell.clone();
        Box::pin(async move {
            let env = agent_env(&shell);
            let infos = tokio::task::spawn_blocking({
                let env = env.clone();
                move || {
                    agents
                        .into_iter()
                        .map(|agent| {
                            if agent == SessionAgent::Nexus {
                                return nexus_runtime::info(&nexus_runtime::select(&env));
                            }
                            let path = find(agent, &shell);
                            AgentInfo {
                                agent,
                                path: path.map(|p| p.to_string_lossy().into_owned()),
                                version: None,
                                models: None,
                                problem: None,
                            }
                        })
                        .collect()
                }
            })
            .await
            .map_err(|_| "Could not inspect the installed agents.")?;
            nexus_workbench::computer::settings::native_choices_by_account(client, infos, env, profiles).await
        })
    }
    fn session_sync(&self) -> Option<Arc<dyn nexus_workbench::session_sync::Backend>> {
        self.sync.current().or_else(|| {
            let current = self.current();
            self.local_sync
                .current(current.client, current.scope.as_str(), current.token.as_deref()?)
        })
    }
    fn session_choice_identity(&self) -> Option<String> {
        self.current()
            .token
            .as_deref()
            .map(nexus_workbench::session_choice::sign_in_identity)
    }
    /// The app's own probe ([`crate::local::set_plans`]), off the async threads: it starts each agent to ask. None set
    /// (a server): no readings.
    fn plans(&self) -> BoxFuture<'static, Vec<nexus_workbench::plans::PlanUsage>> {
        let probe = crate::local::plans();
        let cache = self.plans_cache.clone();
        Box::pin(async move {
            let Some(probe) = probe else {
                return Vec::new();
            };
            let readings = tokio::task::spawn_blocking(move || probe()).await.unwrap_or_default();
            if !readings.is_empty()
                && let Ok(mut held) = cache.write()
            {
                *held = Some((Instant::now(), readings.clone()));
            }
            readings
        })
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
    /// The app's own switch ([`crate::local::set_antigravity_sandbox`]): on, agy runs with `--sandbox` here too.
    fn antigravity_sandbox(&self) -> bool {
        crate::local::antigravity_sandbox()
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
        let sign_in = self.current();
        let data = self.data.clone();
        let known_prices = self.prices.clone();
        let plans_cache = self.plans_cache.clone();
        let files = self.files.clone();
        let app_tools = self.app_tools();
        let shell = self.shell.clone();
        let runners = self.runners.get().cloned();
        let on_server = self.on_server;
        let program_override = self.program_override.get().cloned();
        let owner_sign_in = self.sign_in.clone();
        let local_providers_opt_in = self.local_providers_opt_in.clone();
        Box::pin(async move {
            let local = sign_in.local_launch(&request, on_server, &local_providers_opt_in);
            let SignIn {
                client,
                gateway,
                token,
                scope,
                local_account_pending,
            } = sign_in;
            let agent = label(request.agent);
            if request.provider.is_some() && local.is_none() {
                return Err(nexus_workbench::provider::saved_provider_refusal(
                    client.is_some() || token.is_some() || local_account_pending,
                ));
            }
            let behavior = nexus_workbench::harness::harness(request.agent);
            let local_only = behavior.session_features().local_only;
            if local_only && on_server {
                return Err(format!(
                    "{agent} sessions must start on this computer in Nexus Desktop."
                ));
            }
            if let Some(local) = local {
                if let Some(refused) = files.launch_refusal(request.agent, request.via) {
                    return Err(refused);
                }
                let entry = request
                    .agent
                    .coding()
                    .expect("the local launch gate checked the agent")
                    .entry();
                let program = program_override
                    .or_else(|| find(request.agent, &shell))
                    .ok_or_else(|| {
                        format!(
                            "{agent} is not installed on this machine. Install it with `{}`, then try again.",
                            entry.install.command,
                        )
                    })?;
                let env = agent_env(&shell);
                return match local {
                    LocalLaunch::SavedProvider(providers) => {
                        let check_owner = || {
                            owner_sign_in
                                .read()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .check_saved_provider(on_server)
                        };
                        crate::app_relay::launch_saved_provider(
                            providers,
                            &request,
                            program,
                            env,
                            app_tools.as_deref(),
                            &files,
                            &check_owner,
                        )
                        .await
                    }
                    LocalLaunch::Subscription(admission) => {
                        nexus_workbench::agent_launch::local_subscription(admission, &request, program, env, |launch| {
                            if let Some(tools) = app_tools {
                                tools.configure_local(launch, request.agent);
                            }
                        })
                        .await
                    }
                };
            }
            if (client.is_some() || local_account_pending) && request.scope.is_local() {
                return Err("This session can only use a local subscription.".into());
            }
            let (Some(client), Some(token)) = (client, token) else {
                return Err(format!(
                    "This machine is not signed in to Nexus, so {agent} can't start here. Sign it in from Remote \
                     servers in the Nexus app's settings, or run `{} login` on it.",
                    host_binary()
                ));
            };
            if request.scope != scope {
                return Err(format!(
                    "This session belongs to another organization than the one this machine is signed in to, so \
                     {agent} won't start on its sign-in."
                ));
            }
            if let Some(refused) = files.launch_refusal(request.agent, request.via) {
                return Err(refused);
            }
            // Inside a native session, the agent starts only while this machine holds the session, with its fence in the
            // environment for the Nexus MCP server's connector changes.
            let fence_env = match &runners {
                Some(runners) => runners
                    .fence_for(&client, &request.thread_id)
                    .await?
                    .map(|fence| fence.env())
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            // Every Nexus organization member qualifies, including people without a staff marker or Nexus email.
            // The hook still limits attribution to nexus-xyz repositories.
            let eligible = commit_trailer::desktop_eligible(&client).await;
            if request.agent == SessionAgent::Nexus {
                // Checked at every start, never from a cache: a `nexus` installed or replaced since counts now.
                let mut env = agent_env(&shell);
                if !on_server
                    && nexus_agent_memory::Store::local()
                        .and_then(|store| store.enabled())
                        .unwrap_or(false)
                {
                    env.insert(
                        nexus_agent_memory::tools::AGENT_ENV.into(),
                        request.agent.as_str().into(),
                    );
                }
                if eligible {
                    commit_trailer::mark(&mut env, Surface::Desktop);
                }
                env.extend(fence_env.iter().cloned());
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
            let Some(program) = program_override.or_else(|| find(request.agent, &shell)) else {
                return Err(format!(
                    "{agent} is not installed on this machine. Install it with `{}`, then try again.",
                    client_agent.entry().install.command
                ));
            };
            // An agent that takes its servers in its session gets them through the launcher, with no app tools.
            let offers_tools =
                behavior.app_tools().offered() || nexus_workbench::mcp_servers::in_session(request.agent);
            let launch_env = agent_env(&shell);
            let launch_home = launch_env.get("HOME").cloned();
            let auto =
                if agents::is_auto_name(request.model.as_deref()) && client_agent.entry().billing.gateway.is_some() {
                    let plan_limits = if request.via == Via::Subscription {
                        let cached = plans_cache.read().ok().and_then(|held| {
                            let (at, readings) = held.as_ref()?;
                            (at.elapsed() < PLANS_CACHE_TTL).then(|| readings.clone())
                        });
                        let readings = match cached {
                            Some(readings) => readings,
                            None => match crate::local::plans() {
                                Some(probe) => {
                                    let fresh = tokio::task::spawn_blocking(move || probe()).await.unwrap_or_default();
                                    if !fresh.is_empty()
                                        && let Ok(mut held) = plans_cache.write()
                                    {
                                        *held = Some((Instant::now(), fresh.clone()));
                                    }
                                    fresh
                                }
                                None => Vec::new(),
                            },
                        };
                        let account = request
                            .account_dir
                            .as_deref()
                            .and_then(Path::file_name)
                            .map(|name| name.to_string_lossy().into_owned());
                        readings
                            .into_iter()
                            .find(|plan| plan.agent == request.agent && plan.account == account && plan.error.is_none())
                            .map(|plan| plan.limits)
                    } else {
                        None
                    };
                    Some(AutoContext {
                        prices: Some(&known_prices),
                        installed: Vec::new(),
                        plan_limits,
                        now_ms: now_ms(),
                    })
                } else {
                    None
                };
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
                    // The host has no usage data setting, so it never sends the plan tier.
                    usage_data: false,
                },
            )
            .await?;
            let mut env = agent_env(&shell);
            if let Some(dir) = &request.account_dir
                && let Some(variable) = accounts::account_variable(request.agent)
            {
                env.insert(variable.into(), dir.to_string_lossy().into_owned());
            }
            // Before the files: with it, Codex hands the fence to the person's own MCP servers too.
            env.extend(fence_env);
            // The person's tokens and tools for this machine, read now: a token replaced since the last session applies.
            // An extra Claude account signs in with its own folder, so it gets no token ([`AgentFiles::apply`]).
            files.apply(
                request.agent,
                request.via,
                request.account_dir.is_none(),
                &mut args,
                &mut env,
            );
            // The desktop app's service: the built-in Nexus server and Settings' servers, as the app's own agents get them.
            let mut app_names = Vec::new();
            let mut session_mcp = nexus_workbench::mcp_servers::SessionMcp::default();
            if let Some(tools) = app_tools.as_ref().filter(|_| offers_tools) {
                let launched = tools.launch(
                    request.agent,
                    &request.thread_id,
                    request.scope.as_str(),
                    &session_id,
                    &gateway,
                    &token,
                );
                args.splice(0..0, launched.args);
                env.extend(launched.env);
                app_names = launched.names;
                session_mcp = launched.session;
            }
            // An agent that takes its servers in its session gets only the launched ones: not `mcp.json`'s, and not
            // the remote servers it skipped.
            let configured_mcp = (request.agent != SessionAgent::Nexus && offers_tools)
                .then(|| {
                    if nexus_workbench::mcp_servers::in_session(request.agent) {
                        return session_mcp.names();
                    }
                    let mut names = app_names;
                    names.extend(files.mcp_names());
                    names
                })
                .filter(|names| !names.is_empty());
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
                    model: launch_model(client_agent, request.model.as_deref(), model.as_deref()),
                    route: if request.via == Via::Subscription {
                        Route::Subscription
                    } else {
                        Route::Gateway
                    },
                    model_catalog: None,
                    web_search: false,
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
                    configured_mcp,
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
        if let Some(relay) = self.app_tools().and_then(|tools| tools.relay.clone()) {
            relay.end_local_run(run);
        }
    }

    fn ended(&self, nexus_session: String, end: ProcessEnd) -> BoxFuture<'static, bool> {
        if let Some(relay) = self.app_tools.get().and_then(|tools| tools.relay.as_ref()) {
            relay.end_run(&nexus_session);
        }
        let client = self.client();
        Box::pin(async move {
            // Not signed in: no session was opened through this host's token, so there is nothing to end.
            let Some(client) = client else {
                return true;
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
                    eprintln!("nexus-host: could not end agent session {nexus_session}: {error}");
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
        self.hub.publish(&emit);
    }
}

/// What the shared commands need from this machine.
pub struct HeadlessMachine {
    pub home: PathBuf,
    /// Asked for its Gateway client at each call: the desktop app's service is signed in after it starts.
    pub host: Arc<HeadlessHost>,
}

impl Machine for HeadlessMachine {
    fn tool_path(&self) -> String {
        agent_env(&self.host.shell).remove("PATH").unwrap_or_default()
    }

    fn home(&self) -> Result<PathBuf, String> {
        Ok(self.home.clone())
    }

    fn agent(&self, agent: SessionAgent) -> AgentInfo {
        if agent == SessionAgent::Nexus {
            return nexus_runtime::info(&nexus_runtime::select(&agent_env(&self.host.shell)));
        }
        let path = find(agent, &self.host.shell);
        let version = path.as_deref().and_then(|path| version(path, &self.host.shell));
        AgentInfo {
            agent,
            path: path.map(|p| p.to_string_lossy().to_string()),
            version,
            models: None,
            problem: None,
        }
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

/// This host's binary as a person types it on the machine. The app installs it in a versioned folder that is not on
/// `PATH` (`~/.nexus/host/<version>/nexus-host`), so a bare `nexus-host` is not found there.
fn host_binary() -> String {
    let Ok(path) = std::env::current_exe() else {
        return "nexus-host".into();
    };
    let path = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&format!("{home}/")) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_launch_keeps_subscriptions_with_a_sign_in_and_keeps_providers_signed_out() {
        nexus_client::install_tls();
        let mut request = LaunchRequest {
            thread_id: "t".into(),
            agent: SessionAgent::Claude,
            cwd: std::env::temp_dir(),
            model: None,
            via: Via::Subscription,
            required_payer: None,
            provider: None,
            account_dir: None,
            args: Vec::new(),
            scope: SessionScope::Local,
            prompt_counts: None,
        };
        let mode = LocalProvidersOptIn::Enabled(nexus_workbench::provider::LocalProviders {
            keys: Arc::new(nexus_desktop_core::provider_keys::OsProviderKeys),
        });
        let mut auth = SignIn::new("", None, SessionScope::Local);
        assert!(auth.local_launch(&request, false, &mode).is_some());
        assert!(
            auth.local_launch(&request, false, &LocalProvidersOptIn::Disabled)
                .is_none()
        );
        assert!(auth.local_launch(&request, true, &mode).is_none());
        auth.client = Some(Client::new(ClientOptions::new("https://gateway.invalid")));
        assert!(auth.local_launch(&request, false, &mode).is_none());
        auth.client = None;
        auth.token = Some("saved-token".into());
        assert!(auth.local_launch(&request, false, &mode).is_none());
        auth.token = None;
        request.scope = "gateway|org".into();
        assert!(auth.local_launch(&request, false, &mode).is_none());
        request.scope = SessionScope::Local;
        request.via = Via::Nexus;
        assert!(auth.local_launch(&request, false, &mode).is_none());
        request.via = Via::Subscription;
        auth.local_account_pending = true;
        assert!(matches!(
            auth.local_launch(&request, false, &mode),
            Some(LocalLaunch::Subscription(_))
        ));
        request.provider = Some(nexus_workbench::provider::InstanceId::new("key:test".into()));
        assert!(auth.local_launch(&request, false, &mode).is_none());
        auth.local_account_pending = false;
        assert!(matches!(
            auth.local_launch(&request, false, &mode),
            Some(LocalLaunch::SavedProvider(_))
        ));
        request.provider = None;
        let auth = SignIn::new("https://gateway.invalid", Some("test-key".into()), "gateway|org".into());
        request.via = Via::Subscription;
        assert!(auth.local_launch(&request, false, &mode).is_some());
        assert!(auth.local_launch(&request, true, &mode).is_none());
        request.provider = Some(nexus_workbench::provider::InstanceId::new("key:test".into()));
        assert!(auth.local_launch(&request, false, &mode).is_none());
    }

    /// The app's service reads the plans with the probe the app's binary set, so its rotation sees signed-out
    /// accounts and limits.
    #[tokio::test]
    async fn the_apps_service_reads_plans_with_the_apps_probe() {
        use nexus_workbench::plans::PlanUsage;
        let reading = PlanUsage::signed_out(SessionAgent::Claude, Some("a_1".into()));
        let handed = reading.clone();
        crate::local::set_plans(Arc::new(move || vec![handed.clone()]));
        let host = HeadlessHost::new(
            None,
            std::env::temp_dir(),
            Hub::new(),
            Shell::sh(),
            false,
            LocalProvidersOptIn::Disabled,
        );
        assert_eq!(host.plans().await, vec![reading]);
    }

    #[test]
    fn prices_by_the_agents_names() {
        let host = HeadlessHost::new(
            None,
            std::env::temp_dir(),
            Hub::new(),
            Shell::sh(),
            true,
            LocalProvidersOptIn::Disabled,
        );
        if let Ok(mut catalog) = host.prices.write() {
            catalog
                .prices
                .insert(model_key("anthropic/claude-haiku-4.5"), (1.0, 5.0));
        }
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Usage::default()
        };
        assert_eq!(host.price("claude-haiku-4-5-20251001", &usage), Some(6.0));
        assert_eq!(host.price("gpt-5", &usage), None);
    }

    /// A call no list price covers counts at the catalog's highest input and output prices; with no catalog, at none.
    #[test]
    fn the_top_price_is_the_catalogs_highest() {
        let host = HeadlessHost::new(
            None,
            std::env::temp_dir(),
            Hub::new(),
            Shell::sh(),
            true,
            LocalProvidersOptIn::Disabled,
        );
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Usage::default()
        };
        assert_eq!(host.top_price(&usage), None);
        if let Ok(mut catalog) = host.prices.write() {
            let prices = &mut catalog.prices;
            prices.insert(model_key("anthropic/claude-haiku-4.5"), (1.0, 5.0));
            prices.insert(model_key("anthropic/claude-opus-4.5"), (5.0, 25.0));
            prices.insert(model_key("openai/odd"), (6.0, 2.0));
        }
        assert_eq!(host.top_price(&usage), Some(31.0));
        assert_eq!(host.price("gpt-5", &usage), None);
    }

    /// A server refuses local-only agents. The app's service goes on to the sign-in check.
    /// The standalone host test covers Antigravity. Cursor requires the desktop's workbench feature.
    #[test]
    fn a_server_refuses_local_only_agents_and_the_apps_service_runs_them() {
        let local_only: Vec<SessionAgent> = SessionAgent::ALL
            .into_iter()
            .filter(|agent| nexus_workbench::harness::harness(*agent).session_features().local_only)
            .collect();
        assert!(local_only.contains(&SessionAgent::Antigravity));
        assert!(local_only.contains(&SessionAgent::Grok));
        for agent in local_only {
            for on_server in [true, false] {
                let host = HeadlessHost::new(
                    None,
                    std::env::temp_dir(),
                    Hub::new(),
                    Shell::sh(),
                    on_server,
                    LocalProvidersOptIn::Disabled,
                );
                let request = LaunchRequest {
                    thread_id: "t".into(),
                    agent,
                    cwd: std::env::temp_dir(),
                    model: None,
                    via: Via::Subscription,
                    required_payer: None,
                    provider: None,
                    account_dir: None,
                    args: Vec::new(),
                    scope: "|".into(),
                    prompt_counts: None,
                };
                let runtime = tokio::runtime::Builder::new_current_thread().build();
                let error = runtime
                    .map(|rt| rt.block_on(host.launch(request)).err())
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                assert_eq!(
                    error.contains("must start on this computer"),
                    on_server,
                    "{agent:?} on_server={on_server}: {error}"
                );
                if !on_server {
                    assert!(error.contains("not signed in to Nexus"), "{agent:?}: {error}");
                }
            }
        }
    }

    #[tokio::test]
    async fn uncredentialed_standalone_host_has_the_local_key_and_requires_sign_in() {
        let host = HeadlessHost::new(
            None,
            std::env::temp_dir(),
            Hub::new(),
            Shell::sh(),
            true,
            LocalProvidersOptIn::Enabled(nexus_workbench::provider::LocalProviders {
                keys: Arc::new(nexus_desktop_core::provider_keys::OsProviderKeys),
            }),
        );
        // The wire key collides with the desktop's Local ledger. The server role still requires credentials.
        assert_eq!(host.session_scope(), SessionScope::Local);
        assert_eq!(host.scope(), "|");
        assert!(!host.shows_account_free_sessions());
        let subscription = LaunchRequest {
            thread_id: "t".into(),
            agent: SessionAgent::Claude,
            cwd: std::env::temp_dir(),
            model: None,
            via: Via::Subscription,
            required_payer: None,
            provider: None,
            account_dir: None,
            args: Vec::new(),
            scope: SessionScope::Local,
            prompt_counts: None,
        };
        let saved = LaunchRequest {
            provider: Some(nexus_workbench::provider::InstanceId::new("key:test".into())),
            ..subscription.clone()
        };

        // A later owner check can also refuse the saved provider. Verify that neither request reaches a local route.
        let sign_in = host.current();
        let admitted = |request| {
            sign_in
                .local_launch(request, host.on_server, &host.local_providers_opt_in)
                .is_some()
        };
        assert_eq!(
            [admitted(&subscription), admitted(&saved)],
            [false, false],
            "local admission for [subscription, saved provider]"
        );

        let error = host.launch(subscription).await.expect_err("subscription refusal");
        assert!(
            error.contains("not signed in to Nexus") && error.contains(" login` on it"),
            "{error}"
        );
        assert_eq!(
            host.launch(saved).await.err().as_deref(),
            Some(nexus_workbench::provider::saved_provider_refusal(false).as_str())
        );
    }

    /// A session of another organization never starts on this host's sign-in: refused before Gateway is asked.
    #[test]
    fn refuses_a_launch_for_another_organization() {
        let credentials = crate::files::Credentials {
            gateway: "https://gateway.invalid".into(),
            token: "nx_test".into(),
            org: Some("acme".into()),
            label: "test".into(),
        };
        let host = HeadlessHost::new(
            Some(&credentials),
            std::env::temp_dir(),
            Hub::new(),
            Shell::sh(),
            true,
            LocalProvidersOptIn::Disabled,
        );
        assert_eq!(host.scope(), "https://gateway.invalid|acme");
        let request = LaunchRequest {
            thread_id: "t".into(),
            agent: SessionAgent::Claude,
            cwd: std::env::temp_dir(),
            model: None,
            via: Via::Nexus,
            required_payer: None,
            provider: None,
            account_dir: None,
            args: Vec::new(),
            scope: "https://gateway.invalid|other".into(),
            prompt_counts: None,
        };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
        let error = runtime.map(|rt| rt.block_on(host.launch(request)).err()).ok().flatten();
        assert!(
            error.as_deref().is_some_and(|e| e.contains("another organization")),
            "{error:?}"
        );
    }

    /// Codex on a ChatGPT plan never starts on a cloud machine: refused before Gateway is asked, with the way out.
    #[test]
    fn a_cloud_machine_refuses_codex_on_a_chatgpt_plan() {
        let dir = std::env::temp_dir().join(format!("nh-cloud-plan-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("cloud.json"), r#"{"machineId":"bx_1"}"#);
        let credentials = crate::files::Credentials {
            gateway: "https://gateway.invalid".into(),
            token: "nx_test".into(),
            org: Some("acme".into()),
            label: "test".into(),
        };
        let host = HeadlessHost::new(
            Some(&credentials),
            dir.clone(),
            Hub::new(),
            Shell::sh(),
            true,
            LocalProvidersOptIn::Disabled,
        );
        let request = |via| LaunchRequest {
            thread_id: "t".into(),
            agent: SessionAgent::Codex,
            cwd: std::env::temp_dir(),
            model: None,
            via,
            required_payer: None,
            provider: None,
            account_dir: None,
            args: Vec::new(),
            scope: "https://gateway.invalid|acme".into(),
            prompt_counts: None,
        };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
        let launch = |via| {
            runtime
                .as_ref()
                .ok()
                .and_then(|rt| rt.block_on(host.launch(request(via))).err())
        };
        let refused = launch(Via::Subscription);
        assert!(
            refused.as_deref().is_some_and(|e| e.contains("Team API")),
            "{refused:?}"
        );
        // Through Nexus it goes on (and fails later here, at the unreachable Gateway or a missing Codex, not on the plan).
        let other = launch(Via::Nexus);
        assert!(other.as_deref().is_none_or(|e| !e.contains("Team API")), "{other:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Agy in the app's service follows Settings' Sandbox switch, as the app's binary reads it.
    #[test]
    fn agy_in_the_apps_service_follows_the_sandbox_switch() {
        let host = HeadlessHost::new(
            None,
            std::env::temp_dir(),
            Hub::new(),
            Shell::sh(),
            false,
            LocalProvidersOptIn::Disabled,
        );
        let on = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let read = on.clone();
        crate::local::set_antigravity_sandbox(Arc::new(move || read.load(std::sync::atomic::Ordering::SeqCst)));
        assert!(host.antigravity_sandbox());
        on.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!host.antigravity_sandbox(), "read each time agy starts");
    }

    #[test]
    fn launch_model_forwards_requested_model_only_for_opencode() {
        use nexus_client::agent_entries::Agent;
        assert_eq!(
            launch_model(Agent::Opencode, Some("zai/glm-5.3"), Some("zai/glm-5.3")),
            Some("zai/glm-5.3")
        );
        for other in [Agent::Claude, Agent::Codex, Agent::Antigravity, Agent::Cursor] {
            assert_eq!(launch_model(other, Some("m1"), Some("m1")), None);
            assert_eq!(launch_model(other, None, Some("m1")), Some("m1"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn auto_launch_routes_through_agents_route() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        nexus_client::install_tls();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let dir = std::env::temp_dir().join(format!("nh-auto-route-{}-{}", std::process::id(), now_ms()));
            std::fs::create_dir_all(&dir).expect("dir");
            let claude = dir.join("claude");
            nexus_workbench::process::write_fake_script(&claude, "#!/bin/sh\nexit 0\n");

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            let routed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let routed_flag = routed.clone();
            let server = tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let routed_flag = routed_flag.clone();
                    tokio::spawn(async move {
                        let mut buf = Vec::new();
                        let mut chunk = [0u8; 4096];
                        let mut header_end = None;
                        let mut content_len = 0usize;
                        while let Ok(n) = stream.read(&mut chunk).await {
                            if n == 0 {
                                break;
                            }
                            buf.extend_from_slice(&chunk[..n]);
                            if header_end.is_none()
                                && let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n")
                            {
                                header_end = Some(pos + 4);
                                let head = String::from_utf8_lossy(&buf[..pos]);
                                for line in head.lines() {
                                    if let Some((k, v)) = line.split_once(':')
                                        && k.eq_ignore_ascii_case("content-length")
                                    {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                            if let Some(end) = header_end
                                && buf.len() >= end + content_len
                            {
                                break;
                            }
                        }
                        let Some(end) = header_end else { return };
                        let head = String::from_utf8_lossy(&buf[..end]);
                        let path = head
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or("/");
                        let (status, payload) = match path {
                            "/v1/tenants/me" => (200, json!({ "agentSubscriptions": true })),
                            "/v1/agents/route" => {
                                routed_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                                (
                                    200,
                                    json!({
                                        "result": "ask_for_paid_fallback",
                                        "model": "anthropic/claude-opus-5.5",
                                        "providerModel": "claude-opus-5-5",
                                        "access": "nexus"
                                    }),
                                )
                            }
                            "/v1/agents/sessions" => (200, json!({ "id": "ses_1" })),
                            "/v1/models?status=all" => (200, json!({ "data": [] })),
                            _ => (404, json!({})),
                        };
                        let body = payload.to_string();
                        let resp = format!(
                            "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                    });
                }
            });

            let gateway = format!("http://{addr}");
            let credentials = crate::files::Credentials {
                gateway: gateway.clone(),
                token: "nx_test".into(),
                org: Some("acme".into()),
                label: "test".into(),
            };
            let host = HeadlessHost::new(
                Some(&credentials),
                dir.clone(),
                Hub::new(),
                Shell::sh(),
                false,
                LocalProvidersOptIn::Disabled,
            );
            let _ = host.program_override.set(claude.clone());
            let request = LaunchRequest {
                thread_id: "t".into(),
                agent: SessionAgent::Claude,
                cwd: dir.clone(),
                model: Some("auto".into()),
                via: Via::Nexus,
                required_payer: None,
                provider: None,
                account_dir: None,
                args: vec!["--model".into(), "auto".into()],
                scope: format!("{gateway}|acme").into(),
                prompt_counts: Some(nexus_client::auto_route::Counts::of("hello", false)),
            };
            let launched = host.launch(request).await.expect("headless auto launch");
            server.abort();
            assert!(routed.load(std::sync::atomic::Ordering::SeqCst));
            assert_eq!(launched.model.as_deref(), Some("claude-opus-5-5"));
            assert!(
                launched
                    .spawn
                    .args
                    .windows(2)
                    .any(|w| w == ["--model", "claude-opus-5-5"]),
                "{:?}",
                launched.spawn.args
            );
            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}
