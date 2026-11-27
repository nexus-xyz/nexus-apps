//! Session-scoped PTYs shared with the person and the active agent. Commands have real process exit statuses.
//! Each command gets its own PTY; terminal input addresses that process, never an unrelated provider-owned shell.
mod api;
mod observe;
#[cfg(all(test, unix))]
mod pager_tests;
pub(crate) mod process;
#[cfg(test)]
mod tests;
pub(crate) mod tools;

/// How the notice of a command a provider ran outside the shared terminal starts (`observe.rs`), followed by its bypass
/// code and `)`. An agent with its own shell bypasses on every command, so a reader may show only the first notice of
/// each code.
pub const BYPASS_NOTICE: &str = "Terminal bypass (";

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::event::PermissionMode;

pub(crate) use tools::{NAMES, definitions};
pub const FEATURE: &str = "shared-terminal";

/// The app's call to a host for the person's terminal actions (`threadId`, `op`, `args`), with the authority of the
/// window that checked them ([`crate::Workbench::terminal`]'s `human`): to its own background service, or to a
/// person's own server over SSH. A host answers it only on a connection that passed the hello's token, and only while
/// its sessions have the shared terminal. Agents' tools never reach it.
pub const SERVICE_CALL: &str = "host_terminal";
const OUTPUT_LIMIT: usize = 512 * 1024;
const MAX_ENTRIES: usize = 256;
const MAX_PER_THREAD: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub id: String,
    pub name: String,
    pub interactive: bool,
    pub command: String,
    pub reason: Option<String>,
    pub cwd: String,
    pub status: String,
    pub owner: String,
    pub lease: u64,
    pub private: bool,
    pub result_shared: bool,
    pub closed: bool,
    pub exit_code: Option<u32>,
    pub agent_cursor: u64,
    #[serde(default)]
    pub input_count: u64,
    #[serde(default)]
    pub restored: bool,
    #[serde(default = "workbench_route")]
    pub route: String,
    #[serde(default)]
    pub provider_call_id: Option<String>,
    #[serde(default)]
    pub parent_agent_id: Option<String>,
    #[serde(default)]
    pub bypass_code: Option<String>,
    #[serde(default)]
    pub bypass_reason: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default)]
    pub decision_source: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub ended_at: Option<i64>,
    #[serde(default)]
    pub stop_requested: bool,
    /// An agent wrote the command, and the person did not edit it. It runs without a pager, also when
    /// the person approves it.
    #[serde(default)]
    pub agent_written: bool,
    /// Hidden history row; its execution receipt remains valid.
    pub deleted: bool,
}

fn workbench_route() -> String {
    "workbench".into()
}

impl Info {
    /// A command that runs now and is not a shell waiting on the person: what a session leaves running.
    fn runs(&self) -> bool {
        !self.interactive && matches!(self.status.as_str(), "starting" | "running")
    }

    pub(crate) fn for_storage(&self) -> Self {
        let mut info = self.for_agent();
        // Reviewed summaries and private launch details are not persisted.
        if info.private {
            info.reason = None;
        }
        info
    }
    fn for_agent(&self) -> Self {
        let mut info = self.clone();
        if info.private {
            info.command = "[Private command]".into();
            info.name = "Private terminal".into();
        }
        info
    }
}

#[derive(Default)]
struct Output {
    bytes: VecDeque<u8>,
    cursor: u64,
}

impl Output {
    fn push(&mut self, bytes: &[u8]) {
        self.cursor += bytes.len() as u64;
        self.bytes.extend(bytes);
        let excess = self.bytes.len().saturating_sub(OUTPUT_LIMIT);
        self.bytes.drain(..excess);
    }

    fn read(&self, after: u64, max: usize) -> (Vec<u8>, u64, bool) {
        let first = self.cursor - self.bytes.len() as u64;
        let start = after.clamp(first, self.cursor);
        let offset = (start - first) as usize;
        let bytes: Vec<_> = self.bytes.iter().skip(offset).take(max).copied().collect();
        let next = start + bytes.len() as u64;
        (bytes, next, after < first)
    }
}

struct State {
    info: Info,
    output: Output,
    reviewed: Output,
    input: Option<mpsc::SyncSender<Vec<u8>>>,
    control: Option<mpsc::Sender<process::Control>>,
    /// When the running process last printed, or started. In memory only: it means nothing after a restart.
    last_output: Option<Instant>,
}

impl State {
    /// How long a running command has printed nothing, so an agent can tell a prompt from a slow build. None once it
    /// ended.
    fn idle_ms(&self) -> Option<u64> {
        if !matches!(self.info.status.as_str(), "starting" | "running") {
            return None;
        }
        self.last_output.map(|at| at.elapsed().as_millis() as u64)
    }
}

/// Told the session's id when one of its commands starts or ends: the workbench shows the session again
/// ([`Terminals::on_change`]).
pub(crate) type Changed = Arc<dyn Fn(&str) + Send + Sync>;

struct Execution {
    scope: String,
    thread: String,
    key: String,
    state: Mutex<State>,
    journal: Option<Arc<Mutex<crate::store::Store>>>,
    changed: Option<Changed>,
}

impl Execution {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn info(&self) -> Info {
        self.state().info.clone()
    }

    /// Its command started or ended: its session shows it.
    fn changed(&self) {
        if let Some(changed) = &self.changed {
            changed(&self.thread);
        }
    }

    fn view(&self, human: bool) -> Info {
        let info = self.info();
        if human { info } else { info.for_agent() }
    }

    fn persist(&self, state: &State, kind: &str) -> Result<(), String> {
        if let Some(store) = &self.journal {
            store
                .lock()
                .map_err(|_| "terminal_history_unavailable")?
                .terminal_save(&state.info, kind)?;
        }
        Ok(())
    }

    fn persist_result(&self, state: &State, kind: &str) {
        if let Err(error) = self.persist(state, kind) {
            // The launch receipt remains uncertain; never make it eligible for automatic replay.
            eprintln!("terminal history update failed: {error}");
        }
    }

    fn stop(&self) {
        let mut state = self.state();
        state.info.stop_requested = true;
        state.info.lease += 1;
        if let Some(control) = state.control.take() {
            // Remain running until the process confirms its exit.
            let _ = control.send(process::Control::Stop);
        } else if state.info.status == "awaitingUser" {
            state.info.status = "cancelled".into();
        }
        state.input = None;
        self.persist_result(&state, "stopRequested");
    }
}

#[derive(Default)]
struct Registry {
    scope: String,
    identity: Option<String>,
    /// This sign-in's commands carry the commit trailer hook. Only a yes is kept: an unavailable Gateway answers no.
    marks_commits: bool,
    routing_failures: HashMap<(String, String), String>,
    entries: Vec<Arc<Execution>>,
    /// Evicting output never makes an old execution request runnable again.
    seen: HashMap<(String, String), (uuid::Uuid, String)>,
    /// Each new execution gets it ([`Terminals::on_change`]).
    changed: Option<Changed>,
}

#[derive(Default)]
pub(crate) struct Terminals(Mutex<Registry>, Option<Arc<Mutex<crate::store::Store>>>);

impl Drop for Terminals {
    fn drop(&mut self) {
        self.clear();
    }
}

impl Terminals {
    pub(crate) fn with_store(store: Arc<Mutex<crate::store::Store>>) -> Result<Self, String> {
        store
            .lock()
            .map_err(|_| "terminal_history_unavailable")?
            .terminal_recover()?;
        Ok(Self(Mutex::new(Registry::default()), Some(store)))
    }

    fn history(&self, scope: &str, thread: &str) -> Result<Vec<Info>, String> {
        let registry = self.0.lock().map_err(|_| "terminal_unavailable")?;
        let mut entries = if let Some(store) = &self.1 {
            store
                .lock()
                .map_err(|_| "terminal_history_unavailable")?
                .terminal_history(scope, registry.identity.as_deref().unwrap_or(""), thread)?
        } else {
            Vec::new()
        };
        for info in &mut entries {
            info.restored = true;
        }
        for entry in registry
            .entries
            .iter()
            .filter(|e| e.scope == scope && e.thread == thread)
        {
            let info = entry.info();
            if let Some(old) = entries.iter_mut().find(|e| e.id == info.id) {
                *old = info;
            } else {
                entries.push(info);
            }
        }
        Ok(entries)
    }

    /// `changed` hears of every command that starts or ends from now on.
    pub(crate) fn on_change(&self, changed: Changed) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).changed = Some(changed);
    }

    /// Session `thread`'s commands that run now, oldest first, as [`crate::event::BackgroundWork`]: what it left running
    /// once its turn ends. An interactive shell waits on the person, so it is not listed. A private command shows by its
    /// agent-facing name only.
    pub(crate) fn running(&self, scope: &str, thread: &str) -> Vec<crate::event::BackgroundWork> {
        let registry = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let mut running: Vec<Info> = registry
            .entries
            .iter()
            .filter(|entry| entry.scope == scope && entry.thread == thread)
            .map(|entry| entry.info().for_agent())
            .filter(Info::runs)
            .collect();
        running.sort_by_key(|info| info.started_at.unwrap_or(info.created_at));
        running
            .into_iter()
            .map(|info| crate::event::BackgroundWork {
                kind: crate::event::BackgroundKind::Shell,
                // The name defaults to the command's text: show its first line either way.
                title: match info.name.trim() {
                    "" => info.command.as_str(),
                    name => name,
                }
                .lines()
                .next()
                .unwrap_or_default()
                .to_string(),
                started_at: info.started_at,
            })
            .collect()
    }

    /// Whether any command runs now, in this sign-in's scope ([`Terminals::running`]'s rule).
    pub(crate) fn any_running(&self) -> bool {
        let registry = self.0.lock().unwrap_or_else(|p| p.into_inner());
        registry.entries.iter().any(|entry| entry.info().runs())
    }

    /// `changed`: the registry's [`Terminals::on_change`], passed in since a caller may hold the registry.
    fn restore(
        &self,
        scope: &str,
        thread: &str,
        key: String,
        mut info: Info,
        changed: Option<Changed>,
    ) -> Arc<Execution> {
        info.restored = true;
        // Buffers, reviewed results and process handles belong to the old host.
        info.result_shared = false;
        info.agent_cursor = 0;
        Arc::new(Execution {
            scope: scope.into(),
            thread: thread.into(),
            key,
            journal: self.1.clone(),
            changed,
            state: Mutex::new(State {
                info,
                output: Output::default(),
                reviewed: Output::default(),
                input: None,
                control: None,
                last_output: None,
            }),
        })
    }

    pub(crate) fn clear(&self) {
        let mut entries = self.0.lock().unwrap_or_else(|p| p.into_inner());
        for entry in entries.entries.drain(..) {
            entry.stop();
        }
        entries.seen.clear();
        entries.routing_failures.clear();
    }

    pub(crate) fn context(&self, scope: &str, identity: Option<String>) {
        let mut registry = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if registry.scope == scope && registry.identity == identity {
            return;
        }
        for entry in registry.entries.drain(..) {
            entry.stop();
        }
        registry.seen.clear();
        registry.routing_failures.clear();
        registry.marks_commits = false;
        registry.scope = scope.into();
        registry.identity = identity;
    }

    fn marks_commits(&self) -> bool {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).marks_commits
    }

    /// Keeps a yes for the sign-in it was asked for, and not for one that replaced it meanwhile.
    fn keep_marks_commits(&self, scope: &str, identity: &Option<String>) {
        let mut registry = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if registry.scope == scope && registry.identity == *identity {
            registry.marks_commits = true;
        }
    }

    pub(crate) fn revoke_agent_input(&self, thread: &str) {
        let registry = self.0.lock().unwrap_or_else(|p| p.into_inner());
        for entry in registry.entries.iter().filter(|entry| entry.thread == thread) {
            let mut state = entry.state();
            state.info.lease += 1;
            state.info.owner = "user".into();
            entry.persist_result(&state, "permissionChanged");
        }
    }

    pub(crate) fn close_thread(&self, thread: &str) {
        let registry = self.0.lock().unwrap_or_else(|p| p.into_inner());
        for entry in &registry.entries {
            if entry.thread == thread {
                entry.stop();
            }
        }
    }

    fn find(&self, scope: &str, thread: &str, id: &str) -> Result<Arc<Execution>, String> {
        {
            let registry = self.0.lock().map_err(|_| "terminal_unavailable")?;
            if registry.scope != scope {
                return Err("terminal_not_found".into());
            }
            if let Some(entry) = registry
                .entries
                .iter()
                .find(|e| e.scope == scope && e.thread == thread && e.info().id == id)
            {
                return Ok(entry.clone());
            }
        }
        let info = self
            .history(scope, thread)?
            .into_iter()
            .find(|i| i.id == id)
            .ok_or("terminal_not_found: This terminal does not belong to this session.")?;
        let changed = self.0.lock().map_err(|_| "terminal_unavailable")?.changed.clone();
        Ok(self.restore(scope, thread, String::new(), info, changed))
    }

    fn stage(&self, scope: &str, thread: &str, cwd: PathBuf, args: &Value) -> Result<Arc<Execution>, String> {
        let command = text(args, "command", 32_000)?;
        let key = text(args, "request_id", 128)?;
        let mut registry = self.0.lock().map_err(|_| "terminal_unavailable")?;
        if registry.scope != scope {
            return Err("terminal_not_found".into());
        }
        let interactive = args["interactive"].as_bool().unwrap_or(false);
        let digest = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_OID,
            format!("{interactive}\0{}\0{}\0{command}", args["require_user"], cwd.display()).as_bytes(),
        );
        // Private receipts bind execution context, not secret command text. Retries still return the original entry.
        let private_digest = format!(
            "private:{}",
            uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_OID,
                format!("{interactive}\0{}\0{}", args["require_user"], cwd.display()).as_bytes(),
            )
        );
        if let Some(store) = &self.1 {
            let receipt = store
                .lock()
                .map_err(|_| "terminal_history_unavailable")?
                .terminal_receipt(scope, registry.identity.as_deref().unwrap_or(""), thread, &key)?;
            if let Some((previous, info)) = receipt {
                let expected = if previous.starts_with("private:") {
                    private_digest.clone()
                } else {
                    digest.to_string()
                };
                if previous != expected {
                    return Err("terminal_request_conflict: This request ID already names another command.".into());
                }
                if let Some(entry) = registry.entries.iter().find(|e| e.info().id == info.id) {
                    return Ok(entry.clone());
                }
                return Ok(self.restore(scope, thread, key, info, registry.changed.clone()));
            }
        }
        if let Some((previous, id)) = registry.seen.get(&(thread.into(), key.clone())) {
            if *previous != digest {
                return Err("terminal_request_conflict: This request ID already names another command.".into());
            }
            return registry
                .entries
                .iter()
                .find(|entry| entry.info().id == *id)
                .cloned()
                .ok_or_else(|| {
                    "terminal_execution_expired: The execution ended and its buffer expired; it was not rerun.".into()
                });
        }
        if registry.seen.len() >= 4096 {
            return Err("terminal_limit: Restart Nexus to clear the execution receipt limit.".into());
        }
        let entries = &mut registry.entries;
        // Keep active commands; discard the oldest ended records first.
        while entries.len() >= MAX_ENTRIES
            || entries
                .iter()
                .filter(|e| e.scope == scope && e.thread == thread)
                .count()
                >= MAX_PER_THREAD
        {
            let full_thread = entries
                .iter()
                .filter(|e| e.scope == scope && e.thread == thread)
                .count()
                >= MAX_PER_THREAD;
            let at = entries
                .iter()
                .position(|entry| {
                    (!full_thread || (entry.scope == scope && entry.thread == thread))
                        && matches!(
                            entry.info().status.as_str(),
                            "completed" | "failed" | "interrupted" | "declined" | "cancelled" | "unknown" | "denied"
                        )
                })
                .ok_or("terminal_limit: Close a terminal before starting another.")?;
            entries.remove(at);
        }
        let entry = Arc::new(Execution {
            scope: scope.into(),
            thread: thread.into(),
            key,
            journal: self.1.clone(),
            changed: registry.changed.clone(),
            state: Mutex::new(State {
                info: Info {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: args
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or(if interactive { "Shell" } else { &command })
                        .chars()
                        .take(48)
                        .collect(),
                    interactive,
                    command,
                    cwd: cwd.to_string_lossy().into_owned(),
                    reason: args
                        .get("reason")
                        .and_then(Value::as_str)
                        .map(|s| s.chars().take(2000).collect()),
                    status: "awaitingUser".into(),
                    owner: "user".into(),
                    lease: 0,
                    private: args["private"].as_bool().unwrap_or(false),
                    result_shared: false,
                    closed: false,
                    exit_code: None,
                    agent_cursor: 0,
                    input_count: 0,
                    restored: false,
                    route: workbench_route(),
                    provider_call_id: None,
                    parent_agent_id: None,
                    bypass_code: None,
                    bypass_reason: None,
                    permission_mode: None,
                    decision_source: None,
                    created_at: crate::now_ms(),
                    started_at: None,
                    ended_at: None,
                    stop_requested: false,
                    agent_written: false,
                    deleted: false,
                },
                output: Output::default(),
                reviewed: Output::default(),
                input: None,
                control: None,
                last_output: None,
            }),
        });
        if let Some(store) = &self.1 {
            store
                .lock()
                .map_err(|_| "terminal_history_unavailable")?
                .terminal_insert(
                    scope,
                    registry.identity.as_deref().unwrap_or(""),
                    thread,
                    &entry.key,
                    &if entry.info().private {
                        private_digest
                    } else {
                        digest.to_string()
                    },
                    &entry.info(),
                )?;
        }
        registry.entries.push(entry.clone());
        registry
            .seen
            .insert((thread.into(), entry.key.clone()), (digest, entry.info().id));
        Ok(entry)
    }
}

fn text(args: &Value, field: &str, max: usize) -> Result<String, String> {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|s| {
            (if field == "data" {
                !s.is_empty()
            } else {
                !s.trim().is_empty() && !s.contains('\0')
            }) && s.len() <= max
        })
        .map(String::from)
        .ok_or_else(|| format!("terminal_invalid_arguments: {field} must contain 1 to {max} bytes."))
}

fn agent_write(state: &State, mode: PermissionMode, args: &Value) -> Result<(), String> {
    if !mode.allows_unasked() || state.info.private || state.info.owner != "agent" {
        return Err(
            "terminal_user_control: The person must run this action or return terminal control in Auto or Full access mode."
                .into(),
        );
    }
    if args.get("lease").and_then(Value::as_u64) != Some(state.info.lease) {
        return Err("terminal_stale_lease: Read the terminal again before sending input.".into());
    }
    Ok(())
}
