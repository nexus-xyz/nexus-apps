//! Outside access: scripts, the `nexus` CLI and other agents list,
//! start, read, message and stop Code sessions through a socket only this person can open. The endpoint, the grants
//! and the tools are `nexus_desktop_mcp::outside`'s; [`OutsideAccess`] runs them in this build, and [`GpuiOutside`] is
//! their `Host` over the runner's workbench.
//!
//! - The socket is the Nexus app's: `$XDG_RUNTIME_DIR/xyz.nexus.desktop/sessions.sock` on Linux, `<app
//!   data>/run/sessions.sock` on macOS, so the `nexus` CLI finds either app there. The grants are the Nexus app's
//!   `outside-access.json`, kept only when the organization allows a local cache.
//! - Only the store's owner listens: the runner makes [`OutsideAccess`] when it takes the store, and closes it when it
//!   lets the store go. The switch is the Nexus app's `outsideAccess` setting (Settings, MCP servers). While the
//!   background service runs the sessions (`service.rs`), this build listens too, and the tools reach the sessions
//!   through the service (`nexus_remote::engine`).
//! - When both apps try, the first one listening keeps the socket. The other is refused with "Another Nexus already
//!   offers outside access on this computer." and shows that under the switch.
//! - A client that asks for access gets the consent prompt in the window (`workspace/outside_prompt.rs`). Nothing runs
//!   for it before the person approves; a prompt left for two minutes denies.
//! - A start goes through New session's choices: the organization's remembered agent, model, mode and folder or
//!   worktree, the client's picks over them, and never a permission mode above the remembered one. The Nexus app hands
//!   its starts to its window because its choices live there; this build keeps them in Rust (`paths.rs`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use nexus_desktop_mcp::outside::grants::{GrantView, Grants, Scope};
use nexus_desktop_mcp::outside::{ConsentRequest, Host, Outside, tools};
use nexus_workbench::api::{CreateArgs, WorktreeArgs};
use nexus_workbench::event::{PermissionMode, SessionAgent, Via};
use serde_json::{Map, Value, json};

use crate::code_run::AgentAccess;
use nexus_remote::engine::Engine;

/// The Nexus app's identifier, which names the socket's folder.
const APP_ID: &str = "xyz.nexus.desktop";

/// Where the Nexus app's outside access listens, for its data folder `data`.
pub fn socket_path(data: &std::path::Path) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        Some(nexus_desktop_mcp::outside::socket::path(APP_ID, data))
    }
    #[cfg(not(unix))]
    {
        let _ = (data, APP_ID);
        None
    }
}

/// What Settings shows.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// Whether this platform has the endpoint (macOS and Linux).
    pub supported: bool,
    pub enabled: bool,
    pub listening: bool,
    pub socket: Option<String>,
    pub problem: Option<String>,
    pub grants: Vec<GrantView>,
}

/// The runner's outside access: the endpoint's state, and the socket while it listens.
pub struct OutsideAccess {
    core: Arc<Outside>,
    host: Arc<dyn Host>,
    socket: Option<PathBuf>,
    /// The switch, as saved in the Nexus app's settings.
    enabled: AtomicBool,
    #[cfg(unix)]
    listening: Mutex<Option<nexus_desktop_mcp::outside::socket::Listening>>,
    problem: Mutex<Option<String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl OutsideAccess {
    /// Loads the grants from `grants` (None: in memory only) and listens on `socket` when `on`.
    pub fn new(grants: Option<PathBuf>, socket: Option<PathBuf>, host: Arc<dyn Host>, on: bool) -> OutsideAccess {
        let grants = grants.map(Grants::load).unwrap_or_default();
        let access = OutsideAccess {
            core: Arc::new(Outside::new(grants)),
            host,
            socket,
            enabled: AtomicBool::new(on),
            #[cfg(unix)]
            listening: Mutex::new(None),
            problem: Mutex::new(None),
        };
        if on {
            access.apply(true);
        }
        access
    }

    /// Starts or stops listening. Stopping denies the prompts that wait.
    pub fn apply(&self, on: bool) {
        #[cfg(unix)]
        {
            let mut listening = lock(&self.listening);
            if !on {
                *listening = None;
                self.core.deny_all();
                *lock(&self.problem) = None;
                return;
            }
            if listening.is_some() {
                return;
            }
            let Some(path) = &self.socket else {
                *lock(&self.problem) = Some("This computer has no app data folder.".into());
                return;
            };
            match nexus_desktop_mcp::outside::socket::listen(path, self.core.clone(), self.host.clone()) {
                Ok(socket) => {
                    *listening = Some(socket);
                    *lock(&self.problem) = None;
                }
                Err(error) => {
                    eprintln!("nexus-desktop-gpui: outside access: {error}");
                    *lock(&self.problem) = Some(error);
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = on;
            self.core.deny_all();
        }
    }

    /// The switch: saved in the Nexus app's settings (only while this build owns the store), then applied.
    pub fn set_enabled(&self, on: bool) -> Result<(), String> {
        crate::desktop_settings::update(|settings| {
            settings.outside_access = Some(on);
            Ok(())
        })?;
        self.enabled.store(on, Ordering::SeqCst);
        self.apply(on);
        Ok(())
    }

    pub fn listening(&self) -> bool {
        #[cfg(unix)]
        {
            lock(&self.listening).is_some()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    pub fn view(&self) -> View {
        View {
            supported: cfg!(unix),
            enabled: self.enabled.load(Ordering::SeqCst),
            listening: self.listening(),
            socket: self.socket.as_ref().map(|path| path.display().to_string()),
            problem: lock(&self.problem).clone(),
            grants: self.core.grants().views(),
        }
    }

    pub fn revoke(&self, id: &str) -> Result<(), String> {
        self.core.grants().revoke(id).map(|_| ())
    }

    /// The prompts waiting for the person, oldest first.
    pub fn pending(&self) -> Vec<ConsentRequest> {
        self.core.pending()
    }

    /// The person's answer to a prompt: a scope approves, None denies. The socket's thread adds the grant.
    pub fn answer(&self, id: &str, scope: Option<Scope>) {
        self.core.answer(id, scope);
    }

    /// The runner let the store go: stop listening.
    pub fn close(&self) {
        self.apply(false);
    }
}

/// What a client asked `start_session` for, checked.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Asked {
    pub agent: Option<SessionAgent>,
    pub model: Option<String>,
    pub mode: Option<PermissionMode>,
    pub worktree: Option<bool>,
    /// The session's unique name, checked by [`tools::name_arg`]; the workbench refuses it when taken.
    pub name: Option<String>,
}

/// What an outside start gets.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub agent: SessionAgent,
    pub model: Option<String>,
    pub mode: PermissionMode,
    pub worktree: bool,
    pub via: Via,
}

/// The choice an outside start gets (the Nexus app's `outsideChoice`): New session's remembered choices for the
/// organization (`remembered`, `paths::new_session_defaults`), with the client's picks over them. A mode asked for is
/// used only when it is lower. A worktree needs a git project. Who pays follows Gateway's route, as New session's
/// default does.
pub fn choose(asked: &Asked, remembered: &Value, git: bool, access: &AgentAccess) -> Choice {
    let base_agent = remembered["agent"]
        .as_str()
        .and_then(SessionAgent::parse)
        .unwrap_or(SessionAgent::Claude);
    let base_mode = remembered["mode"]
        .as_str()
        .and_then(PermissionMode::parse)
        .unwrap_or(PermissionMode::Auto);
    let agent = asked.agent.unwrap_or(base_agent);
    let model = asked
        .model
        .clone()
        .or_else(|| remembered["models"][agent.as_str()]["model"].as_str().map(String::from));
    let mode = match asked.mode {
        Some(mode) if mode.rank() < base_mode.rank() => mode,
        _ => base_mode,
    };
    let worktree = asked.worktree.unwrap_or(remembered["workspace"] == "worktree") && git;
    Choice {
        agent,
        model,
        mode,
        worktree,
        via: access.default_via(agent),
    }
}

/// The tools' `Host` over the runner: its commands, the organization it is scoped to, and who may pay.
pub struct GpuiOutside {
    /// The runner's commands: this build's workbench, or the background service.
    pub api: Engine,
    pub runtime: tokio::runtime::Handle,
    /// The organization the runner is scoped to.
    pub org: Arc<dyn Fn() -> String + Send + Sync>,
    /// Gateway's client under the person's sign-in, when signed in.
    pub client: Arc<dyn Fn() -> Option<nexus_client::Client> + Send + Sync>,
    /// A prompt waits or was answered: the windows draw again.
    pub changed: Arc<dyn Fn() + Send + Sync>,
}

impl Host for GpuiOutside {
    fn ask(&self, _request: &ConsentRequest) {
        (self.changed)();
    }

    fn changed(&self) {
        (self.changed)();
    }

    fn call(&self, name: &str, args: &Map<String, Value>) -> Result<Value, String> {
        // Stopping spawns the escalation timer, which needs the runtime.
        if let Some(done) = self.runtime.block_on(tools::run(&self.api, name, args)) {
            return done;
        }
        match name {
            tools::START => {
                let (_, projects, facts) = self.runtime.block_on(self.api.sessions())?;
                self.start(args, projects, facts)
            }
            _ => Err(format!("Unknown tool {name}.")),
        }
    }
}

impl GpuiOutside {
    /// Starts a session as New session does, with [`choose`]'s choices, and stops it if it came up above the mode
    /// asked for. The answer is the session's summary, with what its agent runs with by the store owner's `facts`.
    fn start(
        &self,
        args: &Map<String, Value>,
        projects: Vec<nexus_workbench::event::Project>,
        facts: Option<nexus_workbench::harness::RunFacts>,
    ) -> Result<Value, String> {
        let prompt = tools::text_arg(args, "prompt")?;
        let query = tools::text_arg(args, "project")?;
        let agent = match args.get("agent").and_then(Value::as_str) {
            Some(name) => Some(SessionAgent::parse(name).ok_or_else(|| format!("Unknown agent {name}."))?),
            None => None,
        };
        let model = args
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty());
        if model.is_some_and(|model| model.len() > 200) {
            return Err("The model's name is too long.".into());
        }
        let mode = tools::asked_mode(args.get("permission_mode").unwrap_or(&Value::Null))?;
        let asked = Asked {
            agent,
            model: model.map(String::from),
            mode,
            worktree: args.get("worktree").and_then(Value::as_bool),
            name: tools::name_arg(args)?,
        };
        let project = tools::in_sight(tools::find_project(&projects, &query)?)?.clone();
        let remembered = crate::paths::new_session_defaults(&(self.org)());
        let access = match (self.client)() {
            Some(client) => self.runtime.block_on(crate::code_run::read_access(&client)),
            None => AgentAccess::default(),
        };
        let choice = choose(&asked, &remembered, project.branch.is_some(), &access);
        // Over the service, a new worktree's branch takes the person's prefix, as New session's does.
        let prefix = self
            .api
            .remote()
            .and_then(|_| crate::desktop_settings::load().branch_prefix);
        let thread = self.runtime.block_on(self.api.thread_create(
            CreateArgs {
                request_id: None,
                project_id: project.id.clone(),
                agent: choice.agent.as_str().to_string(),
                model: choice.model,
                permission_mode: choice.mode.as_str().to_string(),
                effort: None,
                via: choice.via.as_str().to_string(),
                provider: None,
                text: prompt,
                images: Vec::new(),
                files: Vec::new(),
                worktree: choice.worktree.then(WorktreeArgs::default),
                orchestration: None,
                name: asked.name.clone(),
            },
            prefix,
        ))?;
        if let Some(mode) = mode
            && thread.permission_mode.rank() > mode.rank()
        {
            let _ = self.runtime.block_on(self.api.interrupt(&thread.id));
            return Err("The session started above the permission mode asked for, so it was stopped.".into());
        }
        Ok(tools::summary(&thread, &projects, facts.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_takes_the_remembered_choices_and_the_clients_picks_never_above_the_mode() {
        let remembered = json!({"agent": "codex", "mode": "acceptEdits", "workspace": "worktree",
            "models": {"codex": {"model": "gpt-5"}, "claude": {"model": "opus"}}});
        let none = AgentAccess::default();
        let choice = choose(&Asked::default(), &remembered, true, &none);
        assert_eq!(
            choice,
            Choice {
                agent: SessionAgent::Codex,
                model: Some("gpt-5".into()),
                mode: PermissionMode::AcceptEdits,
                worktree: true,
                via: Via::Nexus,
            }
        );
        // The client's agent brings that agent's remembered model; a higher mode is not taken, a lower one is.
        let asked = Asked {
            agent: Some(SessionAgent::Claude),
            mode: Some(PermissionMode::BypassPermissions),
            ..Asked::default()
        };
        let choice = choose(&asked, &remembered, true, &none);
        assert_eq!(
            (choice.agent, choice.model.as_deref(), choice.mode),
            (SessionAgent::Claude, Some("opus"), PermissionMode::AcceptEdits)
        );
        let asked = Asked {
            mode: Some(PermissionMode::Plan),
            worktree: Some(false),
            ..Asked::default()
        };
        let choice = choose(&asked, &remembered, true, &none);
        assert_eq!((choice.mode, choice.worktree), (PermissionMode::Plan, false));
        // A folder that is not a git repository gets no worktree.
        assert!(!choose(&Asked::default(), &remembered, false, &none).worktree);
    }

    #[test]
    fn a_first_start_takes_new_sessions_first_choices() {
        let choice = choose(&Asked::default(), &Value::Null, true, &AgentAccess::default());
        assert_eq!(
            (choice.agent, choice.model, choice.mode, choice.worktree),
            (SessionAgent::Claude, None, PermissionMode::Auto, false)
        );
    }
}
