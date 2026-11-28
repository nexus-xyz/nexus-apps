//! `nexus claude` and `nexus codex` sessions in Nexus UI: a numbered session log of the agent's events,
//! synced end-to-end encrypted with the desktop app's own client core.
//!
//! The log is `nexus-workbench`'s store and its session-sync engine (`session_sync.rs`, `GatewayBackend`), opened by
//! this process in the CLI's config folder (`session-log.sqlite3`, `session-sync-keys.sqlite3`). The desktop app keeps
//! its own, so the two never write one SQLite file from two processes. The agent runs in the terminal unchanged, so the
//! events come from its own transcript (`readers.rs`), read while it runs and once more when it exits.
//!
//! Off unless `NEXUS_SESSION_SYNC=1`, the CLI's side of the desktop's `NEXUS_DESKTOP_SESSION_SYNC`, until the release
//! gates turn both on. It also needs this computer's device key (`nexus devices pair`) and the managed
//! `AllowLocalCache` (`allow_local_cache`). Without one of them nothing is written. The organization's content policy,
//! the trust checks and every fail-closed rule are the engine's and the adapter's, unchanged.
//!
//! Phone control (`control.rs`) leases only the run's own thread, and only while its agent can apply a
//! phone's command and the person turned it on (`nexus devices control on`).

mod control;
mod readers;

pub use control::Performed;

use std::collections::HashMap;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use nexus_client::agent_entries::Agent;
use nexus_client::e2e::DeviceKey;
use nexus_client::local_agents::agents::local;
use nexus_client::{Client, ClientOptions};
use nexus_workbench::cli_log::{CliRun, CliThread};
use nexus_workbench::event::{SessionAgent, TurnStatus, Via, WbEvent};
use nexus_workbench::session_sync::{Backend, GatewayBackend, KeyStore};
use nexus_workbench::store::Store;
use nexus_workbench::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd, Workbench};
use serde_json::Value;
use tokio::sync::watch;

pub use readers::{Claude, Codex, Prices, Reader};

type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'static>>;

use crate::Context;

/// The switch. Any other value, or none, is off.
pub const SWITCH: &str = "NEXUS_SESSION_SYNC";
/// How often the transcript is read while the agent runs.
const TAIL: Duration = Duration::from_millis(500);
/// How long a closing run waits for Gateway to confirm what is left. The rest waits in the log for the next run.
const DRAIN: Duration = Duration::from_secs(10);

pub fn enabled(ctx: &Context) -> bool {
    ctx.var(SWITCH).as_deref() == Some("1")
}

/// The CLI's config folder, where the session log and its key store live.
pub fn dir(ctx: &Context) -> PathBuf {
    ctx.config_path.parent().unwrap_or(Path::new(".")).to_path_buf()
}

/// The key store's file: content keys, and the approver `nexus devices pair` confirmed.
pub fn keys_path(ctx: &Context) -> PathBuf {
    dir(ctx).join(KEYS_FILE)
}

const KEYS_FILE: &str = "session-sync-keys.sqlite3";
const LOG_FILE: &str = "session-log.sqlite3";

/// The log's scope for a Gateway and an organization: whose threads and settings these are.
pub fn scope(gateway: &str, tenant: &str) -> String {
    format!("{gateway}|{tenant}")
}

/// The session log's file. It also keeps this computer's Phone control switch (`nexus devices control`).
pub fn log_path(ctx: &Context) -> PathBuf {
    dir(ctx).join(LOG_FILE)
}

/// The workbench's agent for a client agent: whose transcript `readers.rs` reads. `None` for an agent that keeps no
/// transcript (`LocalAgentT::keeps_transcript`): Antigravity, whose conversations are databases no reader here reads
/// yet. Its runs keep no session log.
pub fn workbench_agent(agent: Agent) -> Option<SessionAgent> {
    if !local(agent).keeps_transcript() {
        return None;
    }
    SessionAgent::try_from(agent).ok()
}

/// A switch's value as macOS preferences and the Windows registry print it.
fn switch(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "0x1" => Some(true),
        "0" | "false" | "no" | "0x0" => Some(false),
        _ => None,
    }
}

/// The managed `AllowLocalCache`, from where the desktop app reads it (`src-tauri/src/policy.rs`): macOS preferences
/// in `xyz.nexus.mac` (a configuration profile's forced value included), else Windows Group Policy, HKLM then HKCU.
/// Unset, it is on; Linux has no managed preferences, as for the desktop.
pub fn allow_local_cache() -> bool {
    let read = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
    };
    if cfg!(target_os = "macos") {
        return read("/usr/bin/defaults", &["read", "xyz.nexus.mac", "AllowLocalCache"])
            .and_then(|text| switch(&text))
            .unwrap_or(true);
    }
    if cfg!(windows) {
        for hive in [r"HKLM\Software\Policies\Nexus", r"HKCU\Software\Policies\Nexus"] {
            let value = read("reg", &["query", hive, "/v", "AllowLocalCache"])
                .and_then(|text| text.split_whitespace().last().and_then(switch));
            if let Some(value) = value {
                return value;
            }
        }
    }
    true
}

/// What the agent's run knows before it starts.
pub struct Start {
    pub agent: Agent,
    pub via: Via,
    pub gateway: String,
    pub api_key: String,
    pub nexus_session: String,
    pub model: Option<String>,
    /// The conversation's id when it is known before the run (new with `--session-id`, or resumed by id).
    pub conversation: Option<String>,
    pub cwd: PathBuf,
    /// Claude Code's `CLAUDE_CONFIG_DIR` or Codex's `CODEX_HOME`, as the agent sees it.
    pub agent_home: PathBuf,
    pub dir: PathBuf,
    pub device: DeviceKey,
    pub version: String,
    /// The run's Codex app-server socket (`codex_remote.rs`): the phone can then direct the run. None: read-only.
    pub codex: Option<PathBuf>,
}

/// How the run ended, for a turn the agent left open.
pub struct Finish {
    pub status: TurnStatus,
    /// The conversation's id, when only the run's end told it (Codex's `--last`, Claude Code's `--continue`).
    pub conversation: Option<String>,
}

/// What the log did, for the line after the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Every event is in Gateway.
    Synced { events: usize },
    /// Some wait in the log for the next run.
    Waiting { events: u64, trusted: bool },
    /// Nothing was logged, and why.
    Off(String),
    /// The run started no conversation (`claude --version`, `codex login`): nothing to log.
    Nothing,
}

/// The log of one run, on its own thread with its own runtime, so the agent's terminal is never touched.
pub struct Log {
    finish: watch::Sender<Option<Arc<Finish>>>,
    thread: std::thread::JoinHandle<Outcome>,
}

impl Log {
    pub fn start(start: Start) -> Log {
        let (finish, finished) = watch::channel(None);
        let thread = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return Outcome::Off("the session log could not start".into()),
            };
            runtime.block_on(run(start, finished))
        });
        Log { finish, thread }
    }

    /// Reads the rest, ends an open turn, and waits a little for Gateway.
    pub fn finish(self, finish: Finish) -> Outcome {
        let _ = self.finish.send(Some(Arc::new(finish)));
        self.thread
            .join()
            .unwrap_or_else(|_| Outcome::Off("the session log stopped".into()))
    }
}

/// The workbench's host in the CLI: the person's backend for sync, and nothing else. The CLI starts its agents itself.
struct CliHost(Arc<GatewayBackend>);

impl Host for CliHost {
    fn session_sync(&self) -> Option<Arc<dyn Backend>> {
        Some(self.0.clone())
    }
    fn launch(&self, _: LaunchRequest) -> BoxFuture<Result<Launch, String>> {
        Box::pin(async { Err("The nexus CLI starts its agents itself.".to_string()) })
    }
    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<bool> {
        Box::pin(async { true })
    }
    fn price(&self, _: &str, _: &nexus_workbench::event::Usage) -> Option<f64> {
        None
    }
    fn emit(&self, _: Emit) {}
}

/// The catalog's list prices, for the usage the log records (the desktop's `load_prices`). None on failure: cost 0.
async fn prices(client: &Client) -> Prices {
    let Ok(models) = client.get("/v1/models?status=all").await else {
        return Prices::new();
    };
    let mut prices = HashMap::new();
    for model in models.get("data").and_then(Value::as_array).into_iter().flatten() {
        let (Some(input), Some(output)) = (
            model.pointer("/pricing/input").and_then(Value::as_f64),
            model.pointer("/pricing/output").and_then(Value::as_f64),
        ) else {
            continue;
        };
        for name in ["id", "provider_model"] {
            if let Some(id) = model.get(name).and_then(Value::as_str) {
                prices.insert(readers::model_key(id), (input, output));
            }
        }
    }
    prices
}

/// The workbench for this person, or why there is none. Every error here is the adapter's content-free text.
async fn open(start: &Start) -> Result<(Workbench, bool, Prices), String> {
    let client = Client::new(ClientOptions {
        api_key: Some(start.api_key.clone()),
        client: Some(format!("cli/{}", start.version)),
        ..ClientOptions::new(&start.gateway)
    });
    let whoami = client
        .get("/v1/auth/whoami")
        .await
        .map_err(|_| "Nexus Gateway did not say who you are".to_string())?;
    let tenant = whoami.get("tenantId").and_then(Value::as_str).unwrap_or_default();
    let scope = scope(&start.gateway, tenant);
    // Trusted or not, the log is kept: a pending computer's sessions sync once it is approved.
    let trusted = client.get("/v1/devices").await.ok().is_some_and(|answer| {
        let mine = nexus_client::e2e::b64u(&start.device.encryption_key());
        answer
            .get("devices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|device| {
                device.get("encryptionKey").and_then(Value::as_str) == Some(mine.as_str())
                    && device.get("state").and_then(Value::as_str) == Some("trusted")
            })
    });
    let keys = Arc::new(KeyStore::open(&start.dir.join(KEYS_FILE))?);
    let device = DeviceKey::from_secret_bytes(&start.device.secret_bytes());
    let backend = GatewayBackend::connect(client.clone(), scope.clone(), device, keys, true).await?;
    let store = Store::open(&start.dir.join(LOG_FILE))?;
    let workbench = Workbench::new(
        store,
        Arc::new(CliHost(Arc::new(backend))),
        Config {
            scope,
            ..Config::default()
        },
    )?;
    Ok((workbench, trusted, prices(&client).await))
}

/// The conversation's id and transcript, once the agent has written it.
fn locate(start: &Start, since: SystemTime, told: Option<&str>) -> Option<(String, PathBuf)> {
    let agent = local(start.agent);
    let id = start
        .conversation
        .clone()
        .or_else(|| told.map(String::from))
        .or_else(|| agent.newest_conversation(&start.agent_home, &start.cwd, since))?;
    let path = agent.transcript(&start.agent_home, &start.cwd, &id)?;
    Some((id, path))
}

/// One conversation's transcript, read into its thread.
struct Tail {
    thread: CliThread,
    path: PathBuf,
    reader: Box<dyn Reader>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

impl Tail {
    /// Reads every complete line after the thread's mark and logs its events with the new mark, in one transaction.
    fn read(&mut self, workbench: &Workbench) -> Result<(), String> {
        let mut file = std::fs::File::open(&self.path).map_err(|_| "the transcript could not be read")?;
        file.seek(SeekFrom::Start(self.thread.read_to))
            .map_err(|_| "the transcript could not be read")?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| "the transcript could not be read")?;
        let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n') else {
            return Ok(());
        };
        let mut out = Vec::new();
        let now = now_ms();
        for line in bytes[..end].split(|byte| *byte == b'\n') {
            if let Ok(line) = serde_json::from_slice::<Value>(line) {
                self.reader.line(&line, now, &mut out);
            }
        }
        self.reader.settle(&mut out);
        let read_to = self.thread.read_to + end as u64 + 1;
        workbench.cli_append(&self.thread.id, &out, read_to)?;
        self.thread.read_to = read_to;
        Ok(())
    }
}

async fn run(start: Start, mut finished: watch::Receiver<Option<Arc<Finish>>>) -> Outcome {
    let Some(agent) = workbench_agent(start.agent) else {
        return Outcome::Off(format!("not available for {} yet", start.agent.entry().identity.name));
    };
    let started = SystemTime::now() - Duration::from_secs(2);
    let (workbench, trusted, prices) = match open(&start).await {
        Ok(opened) => opened,
        Err(why) => return Outcome::Off(why),
    };
    // The engine's own loop, beside the reading: each pass sends what the reading logged. Phone control runs beside
    // it, for this run's thread only (`control.rs`).
    let (stop, stopped) = watch::channel(false);
    let (located, thread_id) = watch::channel(None);
    let (conversed, conversation) = watch::channel(None);
    let syncing = {
        let workbench = workbench.clone();
        let stopped = stopped.clone();
        tokio::spawn(async move { workbench.run_session_uploads(stopped).await })
    };
    let performer = control::performer(
        start.codex.as_deref(),
        &workbench,
        thread_id.clone(),
        conversation,
        stopped.clone(),
    )
    .await;
    let controlling = tokio::spawn(control::run(workbench.clone(), thread_id, performer, stopped));
    let mut tail: Option<Tail> = None;
    let mut refused = None;
    let finish = loop {
        let finish = finished.borrow().clone();
        if tail.is_none() && refused.is_none() {
            let told = finish.as_ref().and_then(|finish| finish.conversation.as_deref());
            if let Some((id, path)) = locate(&start, started, told) {
                let run = CliRun {
                    agent,
                    native_id: &id,
                    cwd: &start.cwd,
                    model: start.model.as_deref(),
                    via: start.via,
                };
                match workbench.cli_thread(&run) {
                    Ok(Some(thread)) => {
                        let session = WbEvent::Session {
                            nexus_session_id: Some(start.nexus_session.clone()),
                            native_id: Some(id.clone()),
                            model: start.model.clone(),
                            via: start.via,
                            resumed: thread.read_to > 0,
                        };
                        let logged = workbench.cli_append(&thread.id, &[(now_ms(), session)], thread.read_to);
                        // The readers turn a transcript into workbench events, so they are chosen by the workbench's
                        // agent. `workbench_agent` already stopped an agent that keeps no transcript.
                        let reader: Option<Box<dyn Reader>> = match agent {
                            SessionAgent::Claude => Some(Box::new(Claude::new(prices.clone()))),
                            SessionAgent::Codex => Some(Box::new(Codex::new(prices.clone(), start.model.as_deref()))),
                            SessionAgent::Nexus
                            | SessionAgent::Antigravity
                            | SessionAgent::Cursor
                            | SessionAgent::Grok => None,
                            // nexus-workbench's `opencode` feature adds a variant. This crate leaves it off (turning it
                            // on changes `nexus help` and so commands.json), but a workspace that also builds the
                            // desktop app unifies it on (the open-source repo, whose CI tests the CLI with `-p`).
                            #[allow(unreachable_patterns)]
                            _ => None,
                        };
                        match (logged, reader) {
                            (Ok(()), Some(reader)) => {
                                let _ = located.send(Some(thread.id.clone()));
                                let _ = conversed.send(Some(id.clone()));
                                tail = Some(Tail { thread, path, reader });
                            }
                            (Ok(()), None) => refused = Some("no reader for this agent's conversations".into()),
                            (Err(why), _) => refused = Some(why),
                        }
                    }
                    Ok(None) => refused = Some("this conversation's log belongs to another sign-in".to_string()),
                    Err(why) => refused = Some(why),
                }
            }
        }
        if let Some(tail) = &mut tail
            && let Err(why) = tail.read(&workbench)
        {
            refused = Some(why);
        }
        if let Some(finish) = finish {
            break finish;
        }
        tokio::select! {
            _ = tokio::time::sleep(TAIL) => {}
            _ = finished.changed() => {}
        }
    };
    if let Some(tail) = &mut tail {
        let _ = tail.read(&workbench);
        let mut out = Vec::new();
        tail.reader.finish(now_ms(), finish.status, &mut out);
        if !out.is_empty() {
            let _ = workbench.cli_append(&tail.thread.id, &out, tail.thread.read_to);
        }
    }
    let _ = stop.send(true);
    let _ = syncing.await;
    let _ = controlling.await;
    // Drain: passes until Gateway has every event, or the time is up.
    let deadline = Instant::now() + DRAIN;
    let mut confirmed = 0;
    loop {
        let waiting = workbench.cli_unsynced().unwrap_or(0);
        if waiting == 0 || Instant::now() >= deadline || !trusted {
            if let (None, Some(why)) = (&tail, refused) {
                return Outcome::Off(why);
            }
            if tail.is_none() {
                return Outcome::Nothing;
            }
            return if waiting == 0 {
                Outcome::Synced { events: confirmed }
            } else {
                Outcome::Waiting {
                    events: waiting,
                    trusted,
                }
            };
        }
        let report = workbench.sync_sessions_once().await;
        confirmed += report.confirmed_events;
        if report.confirmed_events == 0 {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_switches_read_as_the_desktop_reads_them() {
        assert_eq!(switch("0\n"), Some(false));
        assert_eq!(switch("1"), Some(true));
        assert_eq!(switch("0x0"), Some(false), "a REG_DWORD");
        assert_eq!(switch("TRUE"), Some(true));
        assert_eq!(switch("maybe"), None);
    }

    #[test]
    fn a_codex_rollout_is_found_by_its_conversation() {
        let home = std::env::temp_dir().join(format!("nexus-rollouts-{}", uuid::Uuid::new_v4()));
        let day = home.join("sessions/2026/11/05");
        std::fs::create_dir_all(&day).expect("dir");
        let id = "0b9c8a1e-6f4d-4b2a-9c3e-1d2f3a4b5c6d";
        let path = day.join(format!("rollout-2026-11-05T10-00-00-{id}.jsonl"));
        std::fs::write(&path, "{}\n").expect("write");
        let codex = local(Agent::Codex);
        assert_eq!(codex.transcript(&home, Path::new("."), id), Some(path));
        assert_eq!(
            codex.transcript(&home, Path::new("."), "11111111-1111-4111-8111-111111111111"),
            None
        );
        let _ = std::fs::remove_dir_all(home);
    }
}
