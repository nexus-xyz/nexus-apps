//! The WebSocket. One connection per client, on the private Unix socket
//! (or a loopback port), reached through `ssh -L`:
//!
//! 1. The client sends `{ "hello": { "token": "…", "protocol": 1 } }` first, within ten seconds. The host answers
//!    `{ "hello": { "version", "protocol", "machine", "org" } }`, or `{ "error": "…" }` and closes: a wrong token, or a
//!    protocol it does not speak (the hello still carries its own, so the app can offer "Update now").
//! 2. Then requests `{ "id", "call", "args" }` (the `wb_*` commands, [`nexus_workbench::api::COMMANDS`], and
//!    `host_status`), each answered `{ "id", "ok" }` or `{ "id", "error" }`, in the order they finish.
//!    On a server or a cloud machine, the `wb_shell_*` calls also open and drive the person's own shells there
//!    ([`nexus_workbench::shell_pty`]), for the person's own app only ([`Peer::is_the_person`]).
//! 3. Pushed between them: `{ "event": "wb:event", "payload": … }` and the other `wb:*` events ([`Hub`]).
//!
//! Catching up after a disconnect is the client's: it reconnects (the host is subscribed before it answers the hello),
//! reads `wb_thread_events` after the last `seq` it has, then keeps the live events with a greater `seq`. Events are
//! stored before they are pushed, so nothing falls between the two. A client too slow for the push queue is closed
//! (it catches up the same way) rather than skipped over in silence.
//!
//! A client that stops reading (a laptop asleep with its tunnel still open) is dropped once it has taken no bytes for
//! [`WRITE_STALL`]: a write that cannot go on does not hold the connection until the network gives up on it.
//!
//! Upgrades that carry an `Origin` header are refused: a web page on the laptop cannot use the tunnel.
//!
//! On a cloud machine, the app's connections also come through Gateway ([`crate::relay`]). Gateway checked the
//! person, so a relayed hello's token is not checked ([`Peer::Relayed`]); its protocol is. `host_provision` puts the
//! app's setup on the machine ([`crate::provision`]).
//!
//! The desktop app's own service ([`crate::local`]) also takes the app's Gateway sign-in: in the hello
//! (`{ "hello": { "token", "protocol", "signIn": { "gateway", "token", "scope" } } }`), so nothing starts before it,
//! and again with `host_sign_in` when the person signs in to another organization. With the app's session sync on, the
//! sign-in also carries this computer's device key (`deviceKey`), so the service syncs as the app would. A server refuses both: it signs in
//! with its own device token. The service also says where a session's folder
//! is (`host_folder`), for the app's "Open in" on this computer. It passes its agents' calls of the app's tools to the
//! app as `host:app_call` pushes, which the app answers with `host_app_reply` ([`crate::app_relay`]).

use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::{SinkExt, StreamExt};
use nexus_workbench::Emit;
use nexus_workbench::api::{Answer, Api, Push, Reply, Request};
pub use nexus_workbench::api::{HostInfo, PROTOCOL};
use nexus_workbench::event::ThreadStatus;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::Sleep;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request as Upgrade, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// How long a new connection has to say hello.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a client may take no bytes at all before it is dropped. A slow link still takes some bytes in that time;
/// only a client that stopped reading takes none.
pub const WRITE_STALL: Duration = Duration::from_secs(30);

/// Pushes held per client before it counts as too slow (streamed text is most of them).
const PUSH_QUEUE: usize = 4096;

/// What the workbench shows, as JSON for every connected client.
#[derive(Clone)]
pub struct Hub {
    tx: broadcast::Sender<Arc<str>>,
    activity: Arc<Mutex<Activity>>,
}

/// Whether a session's agent is working, or waiting for the person: a turn is in progress.
fn busy(status: ThreadStatus) -> bool {
    matches!(
        status,
        ThreadStatus::Starting | ThreadStatus::Running | ThreadStatus::Approval
    )
}

/// Whether a session's agent is working right now. A turn that waits for the person's answer (a permission prompt) is
/// not: for a cloud machine's sleep, that counts as idle.
fn working(status: ThreadStatus) -> bool {
    matches!(status, ThreadStatus::Starting | ThreadStatus::Running)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// When the host last had a turn running, for `host_status`'s `idleSince`: a cloud machine's app sleeps it after it has
/// been idle for a while, and needs the host (which sees every turn) to say since when, not its own guess from the
/// times it happened to look.
///
/// Every thread update passes through the [`Hub`], so a turn ending is seen the moment its status leaves
/// working/waiting. The host's own count of busy sessions at each `host_status` is the truth and this is only the
/// memory of the moment before: if a busy session is gone from the count without an update (deleted, say), that
/// counts as its turn ending now.
///
/// A cloud machine's heartbeat ([`crate::cloud`]) also needs two more clocks: when an agent last stopped working (a
/// permission prompt counts as stopped, [`working`]), and when the person last used the machine (`interactedAt`: the
/// last call that is not a background read, [`nexus_remote::activity::background`]).
#[derive(Debug)]
struct Activity {
    /// Sessions seen busy and not yet seen otherwise.
    busy: HashSet<String>,
    /// Unix ms: when the host started, or when the last turn was seen to end.
    since: i64,
    /// Sessions seen working (not waiting for the person) and not yet seen otherwise.
    working: HashSet<String>,
    /// Unix ms: when the host started, or when an agent was last seen to stop working.
    agent_since: i64,
    /// Unix ms: the last call from a client that was the person using the machine. None until the first one.
    interacted: Option<i64>,
}

/// Since when, given the sessions remembered as active and how many are active right now: None while any is.
fn quiet_since(seen: &mut HashSet<String>, since: &mut i64, active: usize, now: i64) -> Option<i64> {
    if active > 0 {
        return None;
    }
    // Sessions we saw active that are not any more, with no update that said so.
    if !seen.is_empty() {
        seen.clear();
        *since = now;
    }
    Some(*since)
}

impl Activity {
    fn new(now: i64) -> Activity {
        Activity {
            busy: HashSet::new(),
            since: now,
            working: HashSet::new(),
            agent_since: now,
            interacted: None,
        }
    }

    /// A thread update: its session is busy or it is not, working or not.
    fn seen(&mut self, thread: &str, status: ThreadStatus, now: i64) {
        if busy(status) {
            self.busy.insert(thread.to_string());
        } else if self.busy.remove(thread) {
            self.since = now;
        }
        if working(status) {
            self.working.insert(thread.to_string());
        } else if self.working.remove(thread) {
            self.agent_since = now;
        }
    }

    /// `idleSince` given how many sessions are busy right now: None while any is.
    fn idle_since(&mut self, running: usize, now: i64) -> Option<i64> {
        quiet_since(&mut self.busy, &mut self.since, running, now)
    }

    /// `agentIdleSince` given how many agents are working right now: None while any is.
    fn agent_idle_since(&mut self, working: usize, now: i64) -> Option<i64> {
        quiet_since(&mut self.working, &mut self.agent_since, working, now)
    }
}

/// What a cloud machine's heartbeat says about its sessions ([`crate::cloud`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Heartbeat {
    /// An agent is working now (a turn waiting for a permission answer is not).
    pub busy: bool,
    /// Unix ms: when the last agent stopped working. None while one works.
    pub agent_idle_since: Option<i64>,
    /// Unix ms: the person's last call. None if there was none since the host started.
    pub interacted_at: Option<i64>,
}

impl Default for Hub {
    fn default() -> Hub {
        Hub::new()
    }
}

impl Hub {
    pub fn new() -> Hub {
        Hub {
            tx: broadcast::channel(PUSH_QUEUE).0,
            activity: Arc::new(Mutex::new(Activity::new(now_ms()))),
        }
    }

    /// Sends `emit` to every client connected now; with none, it is dropped (the event log keeps what matters).
    pub fn publish(&self, emit: &Emit) {
        // Before the early return: with no client connected, turns still start and end.
        if let Emit::Thread(thread) = emit
            && let Ok(mut activity) = self.activity.lock()
        {
            activity.seen(&thread.id, thread.status, now_ms());
        }
        if self.tx.receiver_count() == 0 {
            return;
        }
        if let Ok(text) = serde_json::to_string(&Push::new(emit)) {
            let _ = self.tx.send(text.into());
        }
    }

    /// The person used the machine now: a client call that is not a background read.
    pub fn interacted(&self, now: i64) {
        if let Ok(mut activity) = self.activity.lock() {
            activity.interacted = Some(now);
        }
    }

    /// When the person last used the machine (unix ms).
    pub fn interacted_at(&self) -> Option<i64> {
        self.activity.lock().ok().and_then(|activity| activity.interacted)
    }

    /// Sends the host's own event `event` to every client connected now; with none, it is dropped.
    pub fn push(&self, event: &str, payload: &Value) {
        if self.tx.receiver_count() == 0 {
            return;
        }
        let _ = self
            .tx
            .send(json!({ "event": event, "payload": payload }).to_string().into());
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<str>> {
        self.tx.subscribe()
    }

    /// How many clients are connected now (each one is subscribed from its hello until it leaves).
    pub fn clients(&self) -> usize {
        self.tx.receiver_count()
    }
}

#[derive(Debug, Deserialize)]
struct ClientHello {
    hello: HelloBody,
}

#[derive(Debug, Deserialize)]
struct HelloBody {
    token: String,
    protocol: u32,
    #[serde(default, rename = "signIn")]
    sign_in: Option<HelloSession>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum HelloSession {
    SignedIn(SignInBody),
    Local {
        #[serde(rename = "localMcp")]
        mcp: nexus_workbench::mcp_servers::Handed,
        accounts: Vec<nexus_workbench::accounts::Choice>,
    },
    // A distinct key makes older hosts reject this handshake instead of treating it as signed out.
    LocalAccountPending {
        #[serde(rename = "localAccountPendingMcp")]
        mcp: nexus_workbench::mcp_servers::Handed,
        accounts: Vec<nexus_workbench::accounts::Choice>,
    },
}

/// The desktop app's Gateway sign-in, for its own service.
#[derive(Debug, Clone, Deserialize)]
pub struct SignInBody {
    pub gateway: String,
    pub token: String,
    /// The sessions' scope, as the app scopes them (`<gateway>|<org>`).
    pub scope: String,
    /// This computer's device key, while the app's session sync would run: the service syncs as the app's computer
    /// ([`crate::local_sync`]). Absent: it syncs nothing.
    #[serde(default, rename = "deviceKey")]
    pub device_key: Option<String>,
    /// Settings' MCP servers and their keychain values, for the service's agents ([`crate::app_relay::AppTools`]).
    /// Absent: none. Settings' later saves come as `host_mcp_servers` (`nexus_workbench::mcp_servers::SERVICE_CALL`).
    #[serde(default)]
    pub mcp: Option<nexus_workbench::mcp_servers::Handed>,
    /// Settings' plan accounts and rotation, which the service's new sessions start on. Absent (an older app): the
    /// service keeps what it had. Settings' later saves come as `host_accounts` (`nexus_workbench::accounts::SERVICE_CALL`).
    #[serde(default)]
    pub accounts: Option<Vec<nexus_workbench::accounts::Choice>>,
}

/// Everything a connection needs.
pub struct Server {
    pub api: Api,
    pub hub: Hub,
    pub token: String,
    pub info: HostInfo,
    /// The desktop app's own service: the host it signs in with the app's sign-in. None on a server.
    pub local: Option<Arc<crate::gateway::HeadlessHost>>,
    /// Where `host_provision` puts a cloud machine's setup ([`crate::provision`]). None: it is refused (the app's own
    /// service).
    pub provision: Option<crate::provision::Places>,
    /// The person's own shells (`wb_shell_*`, [`nexus_workbench::shell_pty`]) on a server or a cloud machine. None:
    /// refused (the app's own service: the app runs its shells itself).
    pub shells: Option<nexus_workbench::shell_pty::Shells>,
    /// The native sessions this server runs ([`crate::session_runner`]). None: the app's own service.
    pub runners: Option<Arc<crate::session_runner::SessionRunners>>,
    /// The host whose agents may get the app's tools: the app's own service or a person's own server
    /// ([`crate::app_relay`]). It takes Settings' MCP servers and the app's answers only once it has them
    /// (`app_tools()`), which a cloud machine never does. None: none (tests).
    pub tools_host: Option<Arc<crate::gateway::HeadlessHost>>,
}

impl Server {
    /// The app's tools of this host's agents, when they get them.
    fn app_tools(&self) -> Option<Arc<crate::app_relay::AppTools>> {
        self.tools_host.as_ref().and_then(|host| host.app_tools())
    }
}

/// A `host_worktree_roots` call's body: the folders as Settings chose them.
#[derive(Debug, Deserialize)]
struct WorktreeRoots {
    worktrees: PathBuf,
    reviews: PathBuf,
}

impl Server {
    /// `host_worktree_roots`: only from the app whose service this is.
    fn worktree_roots(&self, args: Value) -> Result<(), String> {
        if self.local.is_none() {
            return Err("This Nexus host takes no worktree folders from an app.".into());
        }
        let roots: WorktreeRoots =
            serde_json::from_value(args).map_err(|error| format!("Not the worktree folders: {error}."))?;
        self.api
            .workbench()
            .set_worktree_roots(Some(roots.worktrees), Some(roots.reviews));
        Ok(())
    }
}

impl Server {
    /// A request's answer, from a client at the other end of `peer`: the host's own calls, else the shared `wb_*`
    /// commands.
    pub async fn answer(&self, mut request: Request, peer: Peer) -> Reply {
        if peer == Peer::Relayed && self.local.as_ref().is_some_and(|host| host.session_scope().is_local()) {
            return Reply {
                id: request.id,
                answer: Answer::Error("Only the desktop app can use the signed-out service.".into()),
            };
        }
        if let Some(runners) = &self.runners {
            runners.note(&request.call, &mut request.args);
        }
        if !nexus_remote::activity::background(&request.call) {
            self.hub.interacted(now_ms());
        }
        if nexus_workbench::shell_pty::CALLS.contains(&request.call.as_str()) {
            return Reply {
                id: request.id,
                answer: match self.shell(&request.call, request.args, peer).await {
                    Ok(value) => Answer::Ok(value),
                    Err(error) => Answer::Error(error),
                },
            };
        }
        match request.call.as_str() {
            "host_status" => Reply {
                id: request.id,
                answer: match self.status() {
                    Ok(value) => Answer::Ok(value),
                    Err(error) => Answer::Error(error),
                },
            },
            nexus_workbench::orchestra::RELOAD_LIMITS_CALL => Reply {
                id: request.id,
                answer: match (peer == Peer::Local && self.local.is_some())
                    .then(crate::local::subsessions)
                    .flatten()
                {
                    Some(subsessions) => {
                        self.api.workbench().set_subsessions(subsessions);
                        Answer::Ok(Value::Null)
                    }
                    None => Answer::Error("Only the local desktop service can reload linked-session limits.".into()),
                },
            },
            nexus_workbench::WORKTREE_ROOTS_CALL => Reply {
                id: request.id,
                answer: match self.worktree_roots(request.args) {
                    Ok(()) => Answer::Ok(Value::Null),
                    Err(error) => Answer::Error(error),
                },
            },
            "host_sign_in" => Reply {
                id: request.id,
                answer: match serde_json::from_value::<SignInBody>(request.args) {
                    Ok(body) => match self.sign_in(&body) {
                        Ok(()) => Answer::Ok(Value::Null),
                        Err(error) => Answer::Error(error),
                    },
                    Err(error) => Answer::Error(format!("Not a sign-in: {error}.")),
                },
            },
            nexus_workbench::mcp_servers::SERVICE_CALL => Reply {
                id: request.id,
                answer: match (
                    self.app_tools(),
                    serde_json::from_value::<nexus_workbench::mcp_servers::Handed>(request.args),
                ) {
                    (Some(tools), Ok(servers)) => {
                        tools.set_servers(servers);
                        Answer::Ok(Value::Null)
                    }
                    (None, _) => Answer::Error("This Nexus host takes no MCP servers from an app.".into()),
                    (_, Err(error)) => Answer::Error(format!("Not a list of MCP servers: {error}.")),
                },
            },
            nexus_workbench::accounts::SERVICE_CALL => Reply {
                id: request.id,
                answer: match self.take_accounts(request.args) {
                    Ok(()) => Answer::Ok(Value::Null),
                    Err(error) => Answer::Error(error),
                },
            },
            nexus_workbench::app_socket::REPLY_CALL => Reply {
                id: request.id,
                answer: match self.app_tools().and_then(|tools| tools.relay.clone()) {
                    Some(relay) => match relay.answered(&request.args) {
                        Ok(()) => Answer::Ok(Value::Null),
                        Err(error) => Answer::Error(error),
                    },
                    None => Answer::Error("This Nexus host relays no app tools.".into()),
                },
            },
            nexus_workbench::provision::CALL => Reply {
                id: request.id,
                answer: match self.provision(request.args).await {
                    Ok(report) => Answer::Ok(report),
                    Err(error) => Answer::Error(error),
                },
            },
            "host_folder" => Reply {
                id: request.id,
                answer: match self.folder(&request.args) {
                    Ok(folder) => Answer::Ok(Value::String(folder)),
                    Err(error) => Answer::Error(error),
                },
            },
            nexus_workbench::terminal::SERVICE_CALL => Reply {
                id: request.id,
                answer: match self.terminal(request.args).await {
                    Ok(value) => Answer::Ok(value),
                    Err(error) => Answer::Error(error),
                },
            },
            _ => self.api.answer(request).await,
        }
    }

    /// A `wb_shell_*` call: a shell is the person's own, so only the person's own app opens or reaches one.
    async fn shell(&self, call: &str, args: Value, peer: Peer) -> Result<Value, String> {
        if !peer.is_the_person() {
            return Err("Only the person's own app opens a shell here.".into());
        }
        let shells = self
            .shells
            .as_ref()
            .ok_or("This Nexus host opens no shells: the app runs its own.")?;
        let workbench = self.api.workbench();
        shells
            .answer(call, args, |thread, project| match (thread, project) {
                (Some(thread), _) => workbench.folder(thread),
                (None, Some(project)) => Ok(std::path::PathBuf::from(workbench.project(project)?.path)),
                (None, None) => Err("Name a session or a project.".into()),
            })
            .await
    }

    /// `host_provision`: a cloud machine's setup from the app ([`crate::provision`]), off the async threads.
    async fn provision(&self, args: Value) -> Result<Value, String> {
        let places = self
            .provision
            .clone()
            .ok_or("The desktop app's own service takes no machine setup.")?;
        // A person's own server is not a cloud machine: nothing of a cloud machine's setup goes on it. A success, since
        // the app drops a connection whose setup fails.
        if places.root.join(crate::files::PAIRED).exists() {
            return Ok(json!({ "skipped": "a paired server takes no cloud machine setup" }));
        }
        let provision = serde_json::from_value::<nexus_workbench::provision::HostProvision>(args)
            .map_err(|e| format!("Not a machine setup: {e}."))?;
        let report = tokio::task::spawn_blocking(move || crate::provision::apply(&places, &provision))
            .await
            .map_err(|e| format!("The machine setup stopped: {e}"))??;
        // The person's credentials are in place now: the repository's `.agents/resume` runs (nothing before the setup is ready).
        self.api.resume_cloud();
        Ok(report)
    }

    /// A session's folder (`{ threadId, repo }`, as the git panel names a repository inside it): the desktop app's
    /// service only, whose folders are on the app's own computer.
    fn folder(&self, args: &Value) -> Result<String, String> {
        if self.local.is_none() {
            return Err("Only the desktop app's own service says where its folders are.".into());
        }
        let thread = args
            .get("threadId")
            .and_then(Value::as_str)
            .ok_or("Say which session.")?;
        let repo = args.get("repo").and_then(Value::as_str);
        let folder = self.api.workbench().git_folder(thread, repo)?;
        Ok(folder.to_string_lossy().into_owned())
    }

    /// `host_accounts`: Settings saved the plan accounts or rotation in the app whose service this is.
    fn take_accounts(&self, args: Value) -> Result<(), String> {
        if self.local.is_none() {
            return Err("This Nexus host takes no plan accounts from an app.".into());
        }
        let choices = parse_choices(args)?;
        self.api.workbench().apply_account_choices(&choices);
        Ok(())
    }

    /// The desktop app's sign-in, for its own service: agents start on it from now on, its organization's sessions
    /// are the ones shown, and the Nexus sessions a crash left open are ended with it.
    pub fn sign_in(&self, body: &SignInBody) -> Result<(), String> {
        let Some(host) = &self.local else {
            return Err("This Nexus host signs in with `nexus-host login`.".into());
        };
        if nexus_workbench::session_scope::SessionScope::decode(&body.scope).is_local() {
            return Err("Choose a Nexus organization to sign in to.".into());
        }
        if host.app_tools().is_some_and(|tools| tools.relay.is_none()) {
            return Err(
                "The background service could not open its tools socket. Restart the service before signing in.".into(),
            );
        }
        host.sign_in(&body.gateway, &body.token, &body.scope);
        host.local_sync().set_device(body.device_key.as_deref());
        if let Some(tools) = host.app_tools() {
            tools.set_servers(body.mcp.clone().unwrap_or_default());
        }
        let workbench = self.api.workbench().clone();
        if let Some(choices) = &body.accounts {
            workbench.apply_account_choices(choices);
        }
        workbench.set_scope(&body.scope);
        // Once per run: the list of lost sessions is taken by the first call.
        tokio::spawn(async move { workbench.end_lost_sessions().await });
        Ok(())
    }

    /// `host_terminal`: the person's action on a session's shared terminal, from the app's window. Only a host whose
    /// sessions have the shared terminal takes it: the app's own service and a person's own server. The connection
    /// refuses it from a client Gateway relays ([`RELAYED_TERMINAL`]), so the caller passed the hello's token: the
    /// person's app. A cloud machine refuses it too: its sessions have no shared terminal.
    async fn terminal(&self, args: Value) -> Result<Value, String> {
        if !self
            .api
            .workbench()
            .features()
            .iter()
            .any(|feature| feature == nexus_workbench::terminal::FEATURE)
        {
            return Err("terminal_unavailable: This Nexus host takes no terminal actions from an app.".into());
        }
        let call = parse_terminal_call(args)?;
        self.api
            .workbench()
            .terminal(&call.thread_id, &call.op, call.args, true)
            .await
    }

    /// The hello's fields, the sessions whose agent is working or waiting for the person (`sessions`), how many turns
    /// that is (`runningTurns`), the scopes with any agent running (`liveScopes`), and when the last turn ended
    /// (`idleSince`, unix ms; the host's start if none has; null while a turn runs).
    pub fn status(&self) -> Result<Value, String> {
        let threads = self.api.workbench().threads()?;
        let running: Vec<Value> = threads
            .iter()
            .filter(|t| busy(t.status))
            .map(|t| json!({ "id": t.id, "title": t.title, "status": t.status.as_str() }))
            .collect();
        let running_turns = running.len();
        let mut status = serde_json::to_value(&self.info).map_err(|e| e.to_string())?;
        status["sessions"] = Value::Array(running);
        // All scopes, including sub-agents between parent turns. Older clients ignore this field.
        status["activeWork"] = json!(self.api.workbench().has_active_work());
        // Additive: an older app ignores them. `idleSince` is null while a turn runs.
        status["runningTurns"] = json!(running_turns);
        // Every organization with an agent running, shown or not: the app keeps a token it switched away from until
        // no agent started with it runs.
        status["liveScopes"] = json!(self.api.workbench().live_scopes());
        status["idleSince"] = json!(
            self.hub
                .activity
                .lock()
                .ok()
                .and_then(|mut activity| activity.idle_since(running_turns, now_ms()))
        );
        status["interactedAt"] = json!(self.hub.interacted_at());
        Ok(status)
    }

    /// What a cloud machine's heartbeat sends: whether an agent works now, since when none has, and the person's last
    /// call.
    pub fn heartbeat(&self) -> Result<Heartbeat, String> {
        let threads = self.api.workbench().threads()?;
        let working_now = threads.iter().filter(|t| working(t.status)).count();
        let mut activity = self
            .hub
            .activity
            .lock()
            .map_err(|_| "The host's activity is unavailable.")?;
        Ok(Heartbeat {
            busy: working_now > 0,
            agent_idle_since: activity.agent_idle_since(working_now, now_ms()),
            interacted_at: activity.interacted,
        })
    }
}

/// Until `stop` turns true (or its sender is gone).
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stopped| *stopped).await;
}

/// Accepts connections on `listener` until `stop` turns true.
pub async fn serve_unix(listener: UnixListener, server: Arc<Server>, mut stop: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            _ = stopped(&mut stop) => return,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(connection(stream, server.clone(), stop.clone()));
                }
                Err(error) => {
                    eprintln!("nexus-host: accept: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
        }
    }
}

/// [`serve_unix`] on a loopback port, for a server whose SSH does not forward sockets.
pub async fn serve_tcp(listener: TcpListener, server: Arc<Server>, mut stop: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            _ = stopped(&mut stop) => return,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) if peer.ip().is_loopback() => {
                    tokio::spawn(connection(stream, server.clone(), stop.clone()));
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!("nexus-host: accept: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
        }
    }
}

/// Refuses an upgrade from a browser (it carries `Origin`; the app's own client never does).
#[allow(clippy::result_large_err)]
fn refuse_browsers(request: &Upgrade, response: Response) -> Result<Response, ErrorResponse> {
    if request.headers().contains_key("origin") {
        let mut refused = ErrorResponse::new(Some("Browsers may not connect to the Nexus host.".into()));
        *refused.status_mut() = StatusCode::FORBIDDEN;
        return Err(refused);
    }
    Ok(response)
}

fn text(value: &impl Serialize) -> Message {
    Message::text(serde_json::to_string(value).unwrap_or_default())
}

fn close(code: CloseCode, reason: &str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.to_string().into(),
    }))
}

/// Why a relayed client's `host_terminal` is refused: Gateway relays it, so it never passed the hello's token.
pub const RELAYED_TERMINAL: &str = "terminal_unavailable: A client Gateway relays takes no terminal actions.";

/// A `host_terminal` call's body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalCall {
    thread_id: String,
    op: String,
    #[serde(default)]
    args: Value,
}

fn parse_terminal_call(args: Value) -> Result<TerminalCall, String> {
    serde_json::from_value(args).map_err(|error| format!("Not a terminal action: {error}."))
}

/// Why a client Gateway relays may not make `call`: Settings' MCP servers with their secrets, and the app's answers to
/// its agents' tool calls, come only from the person's own app over SSH, whose hello carried the host's token
///. None: it may.
pub fn relayed_refusal(relayed: bool, call: &str) -> Option<&'static str> {
    if !relayed {
        return None;
    }
    match call {
        nexus_workbench::mcp_servers::SERVICE_CALL => Some(RELAYED_MCP_SERVERS),
        nexus_workbench::app_socket::REPLY_CALL => Some(RELAYED_APP_REPLY),
        _ => None,
    }
}

/// The refusals of [`relayed_refusal`].
pub const RELAYED_MCP_SERVERS: &str = "A client Gateway relays hands this Nexus host no MCP servers.";
pub const RELAYED_APP_REPLY: &str = "A client Gateway relays answers no tool calls of this Nexus host's agents.";

/// Who is at the other end of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Peer {
    /// A client through the socket (or the port): its hello carries the host's token.
    Local,
    /// A client Gateway relays ([`crate::relay`]): Gateway checked who it is, so its hello's token is not. The host
    /// pings it every [`nexus_remote::relay::PING`] and drops it after [`nexus_remote::relay::SILENCE`] with no frame.
    Relayed,
}

impl Peer {
    /// Whether the client is the person's own app: it proved the host's token, or Gateway proved who it is. A new kind
    /// of peer (an agent's, a shared viewer's) must decide here whether it may reach the person's shells.
    pub fn is_the_person(self) -> bool {
        match self {
            Peer::Local | Peer::Relayed => true,
        }
    }
}

/// Reads the client's hello; the error is what the client is told.
async fn hello<S>(source: &mut S, server: &Server, peer: Peer) -> Result<(), String>
where
    S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let first = match tokio::time::timeout(HELLO_TIMEOUT, source.next()).await {
        Ok(Some(Ok(Message::Text(first)))) => first,
        Ok(_) => return Err("Say hello first.".into()),
        Err(_) => return Err("No hello in time.".into()),
    };
    let hello: ClientHello =
        serde_json::from_str(first.as_str()).map_err(|_| "Say hello first, with the token.".to_string())?;
    if peer == Peer::Local && !crate::files::same_token(&hello.hello.token, &server.token) {
        return Err("Wrong token.".into());
    }
    if hello.hello.protocol != PROTOCOL {
        return Err(format!(
            "This Nexus host speaks protocol {PROTOCOL}, not {}: update it.",
            hello.hello.protocol
        ));
    }
    match hello.hello.sign_in {
        Some(HelloSession::SignedIn(sign_in)) => server.sign_in(&sign_in)?,
        Some(session @ (HelloSession::Local { .. } | HelloSession::LocalAccountPending { .. })) => {
            let Some(host) = server.local.as_ref().filter(|host| {
                peer == Peer::Local && nexus_workbench::Host::local_providers_opt_in(host.as_ref()).enabled()
            }) else {
                return Err("This Nexus host cannot start a session without a sign-in.".into());
            };
            let (mcp, accounts, pending) = match session {
                HelloSession::Local { mcp, accounts } => (mcp, accounts, false),
                HelloSession::LocalAccountPending { mcp, accounts } => (mcp, accounts, true),
                HelloSession::SignedIn(_) => unreachable!("only local hello variants reach this branch"),
            };
            if host.set_local_account_pending(pending).is_err() {
                return Err("This Nexus host cannot start a session without a sign-in.".into());
            }
            if let Some(tools) = host.app_tools() {
                tools.set_servers(mcp);
            }
            server.api.workbench().apply_account_choices(&accounts);
        }
        None => {}
    }
    Ok(())
}

/// A stream whose writes fail once they have made no progress for `limit`, so a send to a client that stopped
/// reading ends in an error instead of waiting forever. Reads have no limit: an idle client is a normal one.
pub struct WriteDeadline<S> {
    inner: S,
    limit: Duration,
    /// Started when a write first had to wait; cleared by any write that goes through.
    waiting: Option<Pin<Box<Sleep>>>,
}

impl<S> WriteDeadline<S> {
    pub fn new(inner: S, limit: Duration) -> WriteDeadline<S> {
        WriteDeadline {
            inner,
            limit,
            waiting: None,
        }
    }

    fn went_on<T>(&mut self, poll: Poll<io::Result<T>>, cx: &mut Context<'_>) -> Poll<io::Result<T>> {
        if poll.is_ready() {
            self.waiting = None;
            return poll;
        }
        let limit = self.limit;
        let waiting = self.waiting.get_or_insert_with(|| Box::pin(tokio::time::sleep(limit)));
        match waiting.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the client took no bytes in time",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for WriteDeadline<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for WriteDeadline<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.went_on(poll, cx)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let poll = Pin::new(&mut self.inner).poll_flush(cx);
        self.went_on(poll, cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let poll = Pin::new(&mut self.inner).poll_shutdown(cx);
        self.went_on(poll, cx)
    }
}

/// One client, from the upgrade until it leaves (or the host stops, or it stops reading for [`WRITE_STALL`]).
pub async fn connection<S>(stream: S, server: Arc<Server>, stop: watch::Receiver<bool>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    connection_with(stream, server, stop, WRITE_STALL).await
}

/// [`connection`] with its own write stall limit (the tests' short one).
pub async fn connection_with<S>(stream: S, server: Arc<Server>, stop: watch::Receiver<bool>, stall: Duration)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let stream = WriteDeadline::new(stream, stall);
    // The upgrade has the hello's deadline too: a peer that connects and sends nothing does not hold a task forever
    // (in `--port` mode any local user can connect).
    let Ok(Ok(ws)) = tokio::time::timeout(
        HELLO_TIMEOUT,
        tokio_tungstenite::accept_hdr_async(stream, refuse_browsers),
    )
    .await
    else {
        return;
    };
    serve_socket(ws, server, stop, Peer::Local).await
}

/// One client on a WebSocket already upgraded: from the hello until it leaves (or the host stops). A relayed stream
/// comes here straight from [`crate::relay`], in this process.
pub async fn serve_socket<S>(
    ws: tokio_tungstenite::WebSocketStream<S>,
    server: Arc<Server>,
    mut stop: watch::Receiver<bool>,
    peer: Peer,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut source) = ws.split();
    if let Err(error) = hello(&mut source, &server, peer).await {
        let mut refusal = json!({ "error": error });
        // A protocol mismatch still says which host this is, for "Update now"; a wrong token says nothing more.
        if error.starts_with("This Nexus host speaks") {
            refusal["hello"] = serde_json::to_value(&server.info).unwrap_or_default();
        }
        let _ = sink.send(text(&refusal)).await;
        let _ = sink.send(close(CloseCode::Policy, "refused")).await;
        return;
    }
    // Subscribed before the hello is answered: whatever the client reads from the log after this, it also gets live.
    let mut pushes = server.hub.subscribe();
    if sink.send(text(&json!({ "hello": server.info }))).await.is_err() {
        return;
    }
    let (replies, mut answered) = mpsc::unbounded_channel::<Reply>();
    let relayed = peer == Peer::Relayed;
    let ping = nexus_remote::relay::PING;
    let mut pings = tokio::time::interval_at(tokio::time::Instant::now() + ping, ping);
    let silence = tokio::time::sleep(nexus_remote::relay::SILENCE);
    tokio::pin!(silence);
    loop {
        tokio::select! {
            _ = stopped(&mut stop) => {
                let _ = sink.send(close(CloseCode::Away, "The Nexus host is stopping.")).await;
                return;
            }
            _ = pings.tick(), if relayed => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
            _ = &mut silence, if relayed => return,
            message = source.next() => {
                if relayed {
                    silence.as_mut().reset(tokio::time::Instant::now() + nexus_remote::relay::SILENCE);
                }
                match message {
                Some(Ok(Message::Text(body))) => match serde_json::from_str::<Request>(body.as_str()) {
                    // What only the person's own app may hand the host comes only from a client that passed the
                    // hello's token.
                    Ok(request) if relayed_refusal(relayed, &request.call).is_some() => {
                        let refusal = relayed_refusal(relayed, &request.call).unwrap_or_default();
                        let _ = replies.send(Reply {
                            id: request.id,
                            answer: Answer::Error(refusal.into()),
                        });
                    }
                    // Each call runs on its own: a long one (starting a session) does not hold up the others.
                    // The person's terminal actions come only from a client that passed the hello's token.
                    Ok(request) if relayed && request.call == nexus_workbench::terminal::SERVICE_CALL => {
                        let _ = replies.send(Reply {
                            id: request.id,
                            answer: Answer::Error(RELAYED_TERMINAL.into()),
                        });
                    }
                    Ok(request) => {
                        let (server, replies) = (server.clone(), replies.clone());
                        tokio::spawn(async move {
                            let _ = replies.send(server.answer(request, peer).await);
                        });
                    }
                    Err(error) => {
                        let refusal = text(&json!({ "error": format!("Not a request: {error}.") }));
                        if sink.send(refusal).await.is_err() {
                            return;
                        }
                    }
                },
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => {}
            }},
            Some(reply) = answered.recv() => {
                if sink.send(text(&reply)).await.is_err() {
                    return;
                }
            }
            push = pushes.recv() => match push {
                Ok(push) => {
                    if sink.send(Message::text(push.to_string())).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = sink.send(close(CloseCode::Again, "Too far behind: reconnect and catch up.")).await;
                    return;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}

/// The `host_accounts` body: the app's list of [`nexus_workbench::accounts::Choice`].
fn parse_choices(args: Value) -> Result<Vec<nexus_workbench::accounts::Choice>, String> {
    serde_json::from_value(args).map_err(|error| format!("Not a list of plan accounts: {error}."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn local_service(enabled: bool) -> Server {
        use crate::gateway::{HeadlessHost, HeadlessMachine};
        use nexus_workbench::{Config, Workbench, process::Shell, store::Store};

        let dir = std::env::temp_dir();
        let hub = Hub::new();
        use nexus_workbench::provider::{LocalProviders, LocalProvidersOptIn};
        let mode = if enabled {
            LocalProvidersOptIn::Enabled(LocalProviders {
                keys: Arc::new(nexus_desktop_core::provider_keys::OsProviderKeys),
            })
        } else {
            LocalProvidersOptIn::Disabled
        };
        let host = Arc::new(HeadlessHost::new(
            None,
            dir.clone(),
            hub.clone(),
            Shell::sh(),
            false,
            mode,
        ));
        let workbench =
            Workbench::new(Store::in_memory().expect("store"), host.clone(), Config::default()).expect("workbench");
        Server {
            api: Api::new(
                workbench,
                Arc::new(HeadlessMachine {
                    home: dir,
                    host: host.clone(),
                }),
            ),
            hub,
            token: "service-test-token".into(),
            info: HostInfo {
                version: "test".into(),
                protocol: PROTOCOL,
                machine: "test".into(),
                org: None,
                features: Vec::new(),
            },
            local: Some(host.clone()),
            provision: None,
            shells: None,
            runners: None,
            tools_host: Some(host),
        }
    }

    async fn local_hello(server: &Server, peer: Peer, pending: bool) -> Result<(), String> {
        let mcp_field = if pending { "localAccountPendingMcp" } else { "localMcp" };
        let packet = json!({"hello":{
            "token": server.token,
            "protocol": PROTOCOL,
            "signIn": {(mcp_field): {"servers": [], "values": {}}, "accounts": []}
        }});
        let mut input = futures::stream::iter([Ok(Message::text(packet.to_string()))]);
        hello(&mut input, server, peer).await
    }

    #[tokio::test]
    async fn local_hello_requires_a_local_peer_the_flag_and_a_signed_out_service() {
        nexus_client::install_tls();
        let server = local_service(true);
        let refused = Err("This Nexus host cannot start a session without a sign-in.".into());
        assert_eq!(local_hello(&server, Peer::Local, false).await, Ok(()));
        assert_eq!(local_hello(&server, Peer::Relayed, false).await, refused);
        assert_eq!(local_hello(&local_service(false), Peer::Local, false).await, refused);
        let host = server.local.as_ref().expect("local host");
        host.sign_in("https://gateway.invalid", "signed-in", "gateway|org");
        assert_eq!(local_hello(&server, Peer::Local, false).await, refused);
        assert_eq!(host.scope(), "gateway|org");
        assert!(host.client().is_some());
    }

    #[tokio::test]
    async fn pending_account_hello_requires_a_local_peer_and_keeps_saved_providers_locked() {
        use nexus_workbench::Host as _;

        let server = local_service(true);
        let host = server.local.as_ref().expect("the desktop service");
        assert!(local_hello(&server, Peer::Relayed, true).await.is_err());
        assert!(local_hello(&local_service(false), Peer::Local, true).await.is_err());
        assert_eq!(local_hello(&server, Peer::Local, true).await, Ok(()));
        assert_eq!(host.scope(), "|");
        assert!(host.client().is_none());
        assert_eq!(
            host.while_saved_provider_allowed(&mut || panic!("saved provider is locked"))
                .expect_err("a locked host refuses the saved provider"),
            nexus_workbench::provider::saved_provider_refusal(true)
        );
        assert_eq!(local_hello(&server, Peer::Local, false).await, Ok(()));
        assert!(host.while_saved_provider_allowed(&mut || {}).is_ok());
    }

    #[tokio::test]
    async fn signed_out_service_refuses_requests_from_an_already_relayed_peer() {
        let server = local_service(true);
        let request = Request {
            id: 7,
            call: "host_status".into(),
            args: json!({}),
        };
        assert_eq!(
            server.answer(request.clone(), Peer::Relayed).await,
            Reply {
                id: request.id,
                answer: Answer::Error("Only the desktop app can use the signed-out service.".into()),
            }
        );
        assert!(matches!(
            server.answer(request, Peer::Local).await.answer,
            Answer::Ok(_)
        ));
    }

    #[tokio::test]
    async fn service_sign_in_refuses_the_local_scope_and_a_missing_tools_relay() {
        let server = local_service(true);
        let mut body: SignInBody = serde_json::from_value(json!({
            "gateway": "https://gateway.invalid", "token": "t", "scope": "|"
        }))
        .expect("sign-in");
        assert_eq!(
            server.sign_in(&body),
            Err("Choose a Nexus organization to sign in to.".into())
        );
        let host = server.local.as_ref().expect("local host");
        host.set_app_tools(crate::app_relay::AppTools::with_relay(PathBuf::from("launcher"), None));
        body.scope = "gateway|org".into();
        assert_eq!(
            server.sign_in(&body),
            Err(
                "The background service could not open its tools socket. Restart the service before signing in.".into()
            )
        );
        assert_eq!(host.scope(), "|");
        assert!(host.client().is_none());
    }

    /// The app's wire form of its plan accounts, in the sign-in and in `host_accounts`.
    #[test]
    fn plan_accounts_read_as_the_app_sends_them() {
        use nexus_workbench::accounts::Choice;
        use nexus_workbench::event::SessionAgent;
        let sent = json!([
            { "agent": "claude", "account": "a_1", "rotate": false },
            { "agent": "codex", "account": null, "rotate": true },
        ]);
        assert_eq!(
            parse_choices(sent.clone()),
            Ok(vec![
                Choice {
                    agent: SessionAgent::Claude,
                    account: Some("a_1".into()),
                    rotate: false,
                },
                Choice {
                    agent: SessionAgent::Codex,
                    account: None,
                    rotate: true,
                },
            ])
        );
        let body: SignInBody = serde_json::from_value(json!({
            "gateway": "https://gw", "token": "t", "scope": "s", "accounts": sent,
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(body.accounts.map(|choices| choices.len()), Some(2));
        // An older app sends none: the service keeps what it had.
        let older: SignInBody = serde_json::from_value(json!({ "gateway": "https://gw", "token": "t", "scope": "s" }))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(older.accounts, None);
        assert!(parse_choices(json!({ "agent": "claude" })).is_err());
    }

    /// `idleSince` is the host's start until a turn runs, null while one does, and the moment the last one ended after.
    #[test]
    fn idle_since_follows_the_turns() {
        let mut activity = Activity::new(100);
        assert_eq!(activity.idle_since(0, 150), Some(100));
        activity.seen("a", ThreadStatus::Running, 200);
        activity.seen("b", ThreadStatus::Approval, 210);
        assert_eq!(activity.idle_since(2, 220), None);
        activity.seen("a", ThreadStatus::Idle, 300);
        // One still runs.
        assert_eq!(activity.idle_since(1, 310), None);
        activity.seen("b", ThreadStatus::Idle, 400);
        assert_eq!(activity.idle_since(0, 450), Some(400));
        // Later looks do not move it.
        assert_eq!(activity.idle_since(0, 900), Some(400));
        // A session that went away while busy, with no update: its turn ended when the host noticed.
        activity.seen("c", ThreadStatus::Starting, 1000);
        assert_eq!(activity.idle_since(0, 1200), Some(1200));
        // An update for a session never seen busy is not a turn ending.
        activity.seen("d", ThreadStatus::Error, 1300);
        assert_eq!(activity.idle_since(0, 1400), Some(1200));
    }

    /// For a cloud machine's sleep, a permission prompt is idle: `agentIdleSince` starts when the agent stops to ask,
    /// while `idleSince` waits for the turn to end.
    #[test]
    fn a_permission_prompt_counts_as_an_idle_agent() {
        let mut activity = Activity::new(100);
        assert_eq!(activity.agent_idle_since(0, 150), Some(100));
        activity.seen("a", ThreadStatus::Running, 200);
        assert_eq!(activity.agent_idle_since(1, 250), None);
        activity.seen("a", ThreadStatus::Approval, 300);
        assert_eq!(activity.agent_idle_since(0, 350), Some(300));
        assert_eq!(activity.idle_since(1, 350), None);
        activity.seen("a", ThreadStatus::Running, 400);
        activity.seen("a", ThreadStatus::Idle, 500);
        assert_eq!(activity.agent_idle_since(0, 550), Some(500));
        assert_eq!(activity.idle_since(0, 550), Some(500));
    }

    /// Only the person's calls move `interactedAt`; the app's own background reads do not.
    #[test]
    fn background_reads_are_not_interaction() {
        let hub = Hub::new();
        assert_eq!(hub.interacted_at(), None);
        hub.interacted(500);
        assert_eq!(hub.interacted_at(), Some(500));
    }

    #[tokio::test(start_paused = true)]
    async fn a_write_nobody_reads_fails_after_the_limit() {
        let (near, _far) = tokio::io::duplex(64);
        let mut stream = WriteDeadline::new(near, Duration::from_secs(30));
        let started = tokio::time::Instant::now();
        let written = stream.write_all(&[7; 1024]).await;
        assert_eq!(written.map_err(|e| e.kind()), Err(io::ErrorKind::TimedOut));
        assert_eq!(started.elapsed(), Duration::from_secs(30));
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_reader_that_keeps_taking_bytes_is_kept() {
        let (near, mut far) = tokio::io::duplex(64);
        let mut stream = WriteDeadline::new(near, Duration::from_secs(30));
        // 64 bytes every 20 s: the whole write takes far longer than the limit, but no wait is longer than it.
        let reader = tokio::spawn(async move {
            let mut got = 0;
            let mut chunk = [0; 64];
            while got < 1024 {
                tokio::time::sleep(Duration::from_secs(20)).await;
                got += far.read(&mut chunk).await.unwrap_or(0);
            }
            got
        });
        let started = tokio::time::Instant::now();
        assert!(stream.write_all(&[7; 1024]).await.is_ok());
        assert!(started.elapsed() > Duration::from_secs(30));
        assert_eq!(reader.await.ok(), Some(1024));
    }
}
