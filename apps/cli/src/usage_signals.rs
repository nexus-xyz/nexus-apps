//! The usage signals the CLI sends to Gateway (the collection contract): the activity heartbeat
//! (`POST /v1/diagnostics/heartbeat`), the plan tier on a session open (`plan` on `POST /v1/agents/sessions`) and
//! client errors (`POST /v1/diagnostics/errors`). The same as `usage-signals.ts`.
//!
//! - Each call is sent once, with the person's own key, within [`SEND_TIMEOUT`], on its own thread. No command waits
//!   for one, and its answer is ignored: Gateway keeps nothing while its switches are off.
//! - Nothing here holds content: every value comes from a fixed list or from this binary.
//! - An error is queued in a file beside the config ([`ERRORS_FILE`]) and sent by the next run that has a key: the run
//!   that failed is about to end, and a sign-in failure has no key yet.
//! - The plan tier comes from the agent on this machine, never as a raw string: Claude Code's `subscriptionType`
//!   (`auth status --json`) and Codex's `planType` (its app server's `account/read`, as Nexus for desktop reads it).
//!   Codex's read takes seconds, so a session sends the tier the last read saved ([`PLANS_FILE`]), and a thread
//!   reads it again once a day while the agent runs.
//! - A terminal tells the CLI nothing of focus, and while an agent runs, the agent owns the terminal. So the CLI reads
//!   the terminal's own clock: its access time moves when the person types, and its modification time when output
//!   arrives ([`active`]). Windows has no such clock, so there an agent session sends no heartbeat.
//!
//! Only the binary sends ([`enable`]): the tests of other commands run `run_cli` against a stand-in Gateway, and
//! must see no extra calls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nexus_client::agent_entries::Agent;
use nexus_workbench::event::SessionAgent;
use nexus_workbench::harness::{PlanDepth, PlanProbe, harness};
use serde_json::{Value, json};

use crate::VERSION;
use crate::client::{CLI_VERSION_HEADER, Client, ClientOptions};
use crate::config::{create_private_dir, write_private};

pub const HEARTBEAT_PATH: &str = "/v1/diagnostics/heartbeat";
pub const ERRORS_PATH: &str = "/v1/diagnostics/errors";
/// Each call's limit. A call is never sent again.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(5);
/// At most one heartbeat a minute, and only after the person did something in the last minute.
pub const BEAT_EVERY: Duration = Duration::from_secs(60);
pub const ACTIVE_WINDOW: Duration = Duration::from_secs(60);
/// Output counts as read while the person typed within this long: they watch the agent work.
pub const WATCH_WINDOW: Duration = Duration::from_secs(5 * 60);
/// The most entries one errors request carries, and the most the queue keeps.
pub const MAX_ERRORS: usize = 20;
/// A queued error older than this is dropped: Gateway would date it now.
pub const ERROR_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How old a saved plan tier may be before a session reads it again.
pub const PLAN_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub const ERRORS_FILE: &str = "usage-errors.json";
pub const PLANS_FILE: &str = "agent-plans.json";
pub const BEAT_FILE: &str = "usage-beat.json";
/// The errors one run is sending; see [`flush_errors`].
pub const SENDING_FILE: &str = "usage-errors.sending.json";
/// How long a run may take to send before the next run sends its errors itself.
pub const SENDING_STALE: Duration = Duration::from_secs(30);

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Lets this process send. Only `main` calls it.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// This machine's OS, from the contract's list.
pub fn os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        "linux" => "linux",
        _ => "other",
    }
}

/// The CLI's version as the contract takes it: 1 to 32 characters of `[0-9A-Za-z.+-]`.
pub fn app_version() -> String {
    let clean: String = VERSION
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
        .take(32)
        .collect();
    if clean.is_empty() { "0.0.0".into() } else { clean }
}

/// A heartbeat for `surface` (`code` for an agent session, `other` for any other command).
pub fn heartbeat_body(surface: &str) -> Value {
    let surface = if matches!(surface, "code" | "chat" | "wiki" | "review" | "pages") {
        surface
    } else {
        "other"
    };
    json!({ "surface": surface, "app": "cli", "appVersion": app_version(), "os": os() })
}

/// A name the app chose, as the contract takes it: 1 to 64 characters of `[a-z0-9_.-]`; anything else is `fallback`.
pub fn name_of(name: &str, fallback: &'static str) -> String {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-'));
    if ok { name.to_string() } else { fallback.to_string() }
}

/// The plan tier `agent` reported, mapped to the contract's list. Never the raw string.
pub fn plan_of(agent: Agent, raw: &str) -> &'static str {
    let name = raw.trim().to_ascii_lowercase();
    SessionAgent::try_from(agent)
        .ok()
        .and_then(|agent| harness(agent).plan_tier(&name))
        .unwrap_or("unknown")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

fn beside(config_path: &Path, name: &str) -> PathBuf {
    config_path.parent().unwrap_or(Path::new(".")).join(name)
}

/// Writes through a temporary file and a rename, so a run that reads at the same time never sees half a file. `false`
/// when the file could not be written: a throttle that cannot record must not send.
fn write_beside(config_path: &Path, name: &str, value: &Value) -> bool {
    let path = beside(config_path, name);
    if let Some(dir) = path.parent() {
        let _ = create_private_dir(dir);
    }
    let temporary = beside(config_path, &format!("{name}.{}.tmp", std::process::id()));
    if write_private(&temporary, value.to_string().as_bytes()).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return false;
    }
    if std::fs::rename(&temporary, &path).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return false;
    }
    true
}

/// Runs `work` on its own thread, or not at all when the OS refuses one: a signal never fails a command.
fn spawn(work: impl FnOnce() + Send + 'static) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().spawn(work).ok()
}

/// Whether `argv` runs a command that must stay local and instant: the shell's completion (every Tab), the hidden
/// agent protocol, or flags alone (`--version`, `--help`). Such a run sends nothing. A bare `nexus` (the home screen)
/// is not local.
pub fn local_only(argv: &[String]) -> bool {
    match argv.iter().find(|arg| !arg.starts_with('-')).map(String::as_str) {
        None => !argv.is_empty(),
        Some(word) => matches!(word, "__agent" | "__complete" | "completion"),
    }
}

fn read_beside(config_path: &Path, name: &str) -> Option<Value> {
    let text = std::fs::read_to_string(beside(config_path, name)).ok()?;
    serde_json::from_str(&text).ok()
}

/// A client that sends once, within [`SEND_TIMEOUT`]: each signal carries a fresh `Idempotency-Key`, never reused, and
/// is not retried.
pub fn sender(url: &str, api_key: &str) -> Client {
    Client::new(ClientOptions {
        api_key: Some(api_key.to_string()),
        headers: vec![(CLI_VERSION_HEADER.to_string(), VERSION.to_string())],
        idempotency: true,
        retry: false,
        client: Some(format!("cli/{VERSION}")),
        timeout: SEND_TIMEOUT,
        ..ClientOptions::new(url)
    })
}

/// Sends `body` to `path` once, on its own thread. The caller may wait for it (tests) or not (the CLI). `None` when
/// the OS refused the thread.
pub fn send(url: &str, api_key: &str, path: &'static str, body: Value) -> Option<std::thread::JoinHandle<()>> {
    let (url, api_key) = (url.to_string(), api_key.to_string());
    // The client is built on the signal's thread: building it starts threads too, and a refusal must not panic here.
    spawn(move || {
        let _ = sender(&url, &api_key).post(path, body);
    })
}

/* ------------------------------------------------------------------ a command */

/// One heartbeat with surface `other` for a command the person typed, at most once a minute on this machine
/// ([`BEAT_FILE`] keeps the last one). `None` when none is due.
pub fn command_beat(config_path: &Path, url: &str, api_key: &str) -> Option<std::thread::JoinHandle<()>> {
    let now = now_ms();
    let last = read_beside(config_path, BEAT_FILE)
        .and_then(|saved| saved.get("at").and_then(Value::as_u64))
        .unwrap_or(0);
    if last <= now && now - last < BEAT_EVERY.as_millis() as u64 {
        return None;
    }
    if !write_beside(config_path, BEAT_FILE, &json!({ "at": now })) {
        return None;
    }
    send(url, api_key, HEARTBEAT_PATH, heartbeat_body("other"))
}

/// Whether `argv` is a command the person typed that counts as `other`: not an agent session (it beats on its own),
/// not the shell's completion helper or the hidden agent protocol, and not flags alone (`--version`).
pub fn counts_as_command(argv: &[String]) -> bool {
    match argv.iter().find(|arg| !arg.starts_with('-')).map(String::as_str) {
        None => false,
        Some(word) => {
            Agent::from_command(word).is_none_or(|agent| !crate::agents::launch_available(agent))
                && !matches!(word, "__agent" | "__complete" | "completion")
        }
    }
}

/* ------------------------------------------------------------------ an agent session */

/// Whether the person used the terminal lately: typed in the last minute, or got output in the last minute while
/// they typed in the last five (they read the agent's work).
pub fn active(now: SystemTime, input: Option<SystemTime>, output: Option<SystemTime>) -> bool {
    let within = |at: Option<SystemTime>, window: Duration| {
        at.is_some_and(|at| now.duration_since(at).map_or(true, |age| age <= window))
    };
    within(input, ACTIVE_WINDOW) || (within(output, ACTIVE_WINDOW) && within(input, WATCH_WINDOW))
}

/// The terminal's clock on stdin: when it was last read (input) and written (output). `None` off a terminal and on
/// Windows.
#[cfg(unix)]
pub fn terminal_clock() -> Option<(SystemTime, SystemTime)> {
    use std::io::IsTerminal as _;
    use std::os::fd::AsFd as _;
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return None;
    }
    let file = std::fs::File::from(stdin.as_fd().try_clone_to_owned().ok()?);
    let meta = file.metadata().ok()?;
    Some((meta.accessed().ok()?, meta.modified().ok()?))
}

#[cfg(not(unix))]
pub fn terminal_clock() -> Option<(SystemTime, SystemTime)> {
    None
}

/// The heartbeats of an agent session (surface `code`), on their own thread: each minute, one when [`active`] says
/// the person used the terminal.
pub struct ActivityBeat {
    stop: Option<std::sync::mpsc::Sender<()>>,
}

impl ActivityBeat {
    /// Nothing starts unless the binary enabled sending and stdin is a terminal with a clock.
    pub fn start(url: &str, api_key: &str) -> ActivityBeat {
        if !enabled() || terminal_clock().is_none() {
            return ActivityBeat { stop: None };
        }
        ActivityBeat::start_with(url, api_key, BEAT_EVERY, || {
            terminal_clock().is_some_and(|(input, output)| active(SystemTime::now(), Some(input), Some(output)))
        })
    }

    /// Every `every`, a heartbeat when `is_active` says so.
    pub fn start_with(
        url: &str,
        api_key: &str,
        every: Duration,
        is_active: impl Fn() -> bool + Send + 'static,
    ) -> ActivityBeat {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let (url, api_key) = (url.to_string(), api_key.to_string());
        let _ = spawn(move || {
            let client = sender(&url, &api_key);
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(every) {
                if is_active() {
                    let _ = client.post(HEARTBEAT_PATH, heartbeat_body("code"));
                }
            }
        });
        ActivityBeat { stop: Some(stop) }
    }

    /// Stops the beats. Never waits for one in flight.
    pub fn stop(self) {
        drop(self.stop);
    }
}

/* ------------------------------------------------------------------ errors */

/// What failed, from the contract's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    SignIn,
    Update,
    Agent,
}

impl ErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            ErrorKind::SignIn => "sign_in",
            ErrorKind::Update => "update",
            ErrorKind::Agent => "agent",
        }
    }
}

/// Queues one error for the next run to send. `component` and `code` are this binary's own names; anything outside
/// `[a-z0-9_.-]` becomes `unknown` or `other`.
pub fn record_error(config_path: &Path, kind: ErrorKind, component: &str, code: &str) {
    let entry = json!({
        "app": "cli",
        "appVersion": app_version(),
        "os": os(),
        "kind": kind.as_str(),
        "component": name_of(component, "unknown"),
        "code": name_of(code, "other"),
        "fatal": false,
        "occurredAt": rfc3339_ms(now_ms()),
    });
    let mut queue = queued(config_path);
    queue.push(entry);
    let keep = queue.len().saturating_sub(MAX_ERRORS);
    write_beside(config_path, ERRORS_FILE, &Value::Array(queue.split_off(keep)));
}

fn queued(config_path: &Path) -> Vec<Value> {
    match read_beside(config_path, ERRORS_FILE) {
        Some(Value::Array(entries)) => entries,
        _ => Vec::new(),
    }
}

/// Sends what is queued, once, and drops entries older than [`ERROR_MAX_AGE`]. `None` when nothing waits, or another
/// run is sending.
///
/// The queue moves to [`SENDING_FILE`] first, so a run that starts later leaves it alone, and the file goes once
/// Gateway answered or the call gave up: a failed call is not sent again. A run that ends before that (a fast command
/// ends its process, and its threads with it) leaves the file, and a run [`SENDING_STALE`] later sends it, so an entry
/// can arrive twice. When the files cannot be written or removed, nothing is sent: the same entries would go on every
/// run.
pub fn flush_errors(config_path: &Path, url: &str, api_key: &str) -> Option<std::thread::JoinHandle<()>> {
    let sending = beside(config_path, SENDING_FILE);
    let mut queue = Vec::new();
    if let Ok(meta) = std::fs::metadata(&sending) {
        // A time far in the future (a clock that moved back) counts as stale, or the file would hold the queue forever.
        // A few seconds of skew (a network home) is still a run that is sending.
        let stale = meta.modified().map_or(true, |at| match at.elapsed() {
            Ok(age) => age >= SENDING_STALE,
            Err(ahead) => ahead.duration() >= SENDING_STALE,
        });
        if !stale {
            return None;
        }
        if let Some(Value::Array(left)) = read_beside(config_path, SENDING_FILE) {
            queue = left;
        }
    }
    queue.extend(queued(config_path));
    let cutoff = rfc3339_ms(now_ms().saturating_sub(ERROR_MAX_AGE.as_millis() as u64));
    queue.retain(|entry| {
        entry
            .get("occurredAt")
            .and_then(Value::as_str)
            .is_some_and(|at| at >= cutoff.as_str())
    });
    let keep = queue.len().saturating_sub(MAX_ERRORS);
    let queue = queue.split_off(keep);
    if queue.is_empty() {
        let _ = std::fs::remove_file(&sending);
        let _ = std::fs::remove_file(beside(config_path, ERRORS_FILE));
        return None;
    }
    if !write_beside(config_path, SENDING_FILE, &Value::Array(queue.clone())) {
        return None;
    }
    match std::fs::remove_file(beside(config_path, ERRORS_FILE)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            let _ = std::fs::remove_file(&sending);
            return None;
        }
        _ => {}
    }
    let (url, api_key) = (url.to_string(), api_key.to_string());
    spawn(move || {
        let _ = sender(&url, &api_key).post(ERRORS_PATH, json!({ "errors": queue }));
        let _ = std::fs::remove_file(sending);
    })
}

/// A time as RFC 3339 in UTC, to the millisecond (`2026-11-09T17:31:02.120Z`). The same length always, so it sorts
/// as text.
pub fn rfc3339_ms(ms: u64) -> String {
    let at = time::OffsetDateTime::from_unix_timestamp((ms / 1000) as i64).unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        ms % 1000
    )
}

/* ------------------------------------------------------------------ the plan tier */

/// The tier the last read saved for `agent`, from the contract's list.
pub fn saved_plan(config_path: &Path, agent: Agent) -> Option<String> {
    let saved = read_beside(config_path, PLANS_FILE)?;
    let plan = saved.pointer(&format!("/{}/plan", agent.as_str()))?.as_str()?;
    (plan_of(agent, plan) == plan || plan == "unknown").then(|| plan.to_string())
}

fn saved_at(config_path: &Path, agent: Agent) -> Option<u64> {
    read_beside(config_path, PLANS_FILE)?
        .pointer(&format!("/{}/at", agent.as_str()))?
        .as_u64()
}

/// Saves what `agent` reported: a tier (mapped), or `None` for a login that is not on a plan, which forgets the tier.
/// Both keep the time, so the background read runs once a day and not on every session.
pub fn save_plan(config_path: &Path, agent: Agent, raw: Option<&str>) {
    let mut saved = match read_beside(config_path, PLANS_FILE) {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    match raw {
        Some(raw) => {
            saved.insert(
                agent.as_str().into(),
                json!({ "plan": plan_of(agent, raw), "at": now_ms() }),
            );
        }
        None => {
            saved.insert(agent.as_str().into(), json!({ "at": now_ms() }));
        }
    }
    write_beside(config_path, PLANS_FILE, &Value::Object(saved));
}

/// Keeps the saved tier and records that a read ran now, so an agent that gives no answer is asked once a day.
/// `false` when the time could not be recorded.
fn stamp_plan_read(config_path: &Path, agent: Agent) -> bool {
    let mut saved = match read_beside(config_path, PLANS_FILE) {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    let mut entry = match saved.remove(agent.as_str()) {
        Some(Value::Object(entry)) => entry,
        _ => serde_json::Map::new(),
    };
    entry.insert("at".into(), json!(now_ms()));
    saved.insert(agent.as_str().into(), Value::Object(entry));
    write_beside(config_path, PLANS_FILE, &Value::Object(saved))
}

/// Reads the tier again on its own thread when the saved one is a day old or missing. The agent's environment loses
/// what bills elsewhere first. A read with no answer keeps the saved tier and stamps the time; a time that cannot be
/// recorded means no read, or it would run on every session. The tier is the account's, so the read never runs in the
/// project the person opened: Claude Code's in the config's own private directory, Codex's in the temporary directory
/// (`nexus_workbench::plans::codex`, shared with Nexus for desktop).
pub fn refresh_plan(config_path: &Path, agent: Agent, bin: &Path, env: &HashMap<String, String>) {
    if !enabled() {
        return;
    }
    let now = now_ms();
    // A time in the future (a clock that moved back) is not fresh.
    let fresh = saved_at(config_path, agent).is_some_and(|at| at <= now && now - at < PLAN_MAX_AGE.as_millis() as u64);
    if fresh {
        return;
    }
    let cwd = config_path
        .parent()
        .filter(|dir| dir.is_dir())
        .map_or_else(std::env::temp_dir, Path::to_path_buf);
    // A relative path to the agent named a file in the project, so it is made absolute before the read moves away.
    let bin = if bin.is_relative() && bin.components().count() > 1 {
        std::path::absolute(bin).unwrap_or_else(|_| bin.to_path_buf())
    } else {
        bin.to_path_buf()
    };
    let config_path = config_path.to_path_buf();
    let mut env = env.clone();
    for name in agent.entry().billing.plan_overrides {
        env.remove(*name);
    }
    let probe = PlanProbe {
        program: bin,
        env,
        account: None,
        dir: None,
        cwd: Some(cwd),
    };
    let Some(read) = SessionAgent::try_from(agent)
        .ok()
        .and_then(|agent| harness(agent).read_plan(probe, PlanDepth::TierOnly))
    else {
        return;
    };
    // Stamped before the read: a short session's process may end first, and the next one must not read again.
    if !stamp_plan_read(&config_path, agent) {
        return;
    }
    let _ = spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        let usage = runtime.block_on(read);
        if usage.signed_in {
            save_plan(&config_path, agent, Some(usage.plan.as_deref().unwrap_or("unknown")));
        } else if usage.error.is_none() {
            save_plan(&config_path, agent, None);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<(String, Value, HashMap<String, String>)>>>;

    fn dir() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "nexus-usage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        // A recycled pid must not find another run's files.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    /// A stand-in Gateway that records each request and answers `status` after `delay`.
    fn gateway(status: u16, delay: Duration) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let log = seen.clone();
        let (ready, port) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async move {
                let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
                    let log = log.clone();
                    async move {
                        let path = request.uri().path().to_string();
                        let headers = request
                            .headers()
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
                            .collect();
                        let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                            .await
                            .unwrap_or_default();
                        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                        log.lock().expect("log").push((path, body, headers));
                        tokio::time::sleep(delay).await;
                        (
                            axum::http::StatusCode::from_u16(status).expect("status"),
                            "{}".to_string(),
                        )
                    }
                });
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
                ready.send(listener.local_addr().expect("addr").port()).expect("ready");
                axum::serve(listener, app).await.expect("serve");
            });
        });
        (format!("http://127.0.0.1:{}", port.recv().expect("port")), seen)
    }

    #[test]
    fn maps_each_plan_to_the_contracts_list() {
        for (raw, plan) in [
            ("max", "max"),
            ("Pro", "pro"),
            ("team", "team"),
            ("enterprise", "enterprise"),
        ] {
            assert_eq!(plan_of(Agent::Claude, raw), plan);
        }
        assert_eq!(plan_of(Agent::Claude, "claude_max_20x"), "unknown");
        assert_eq!(plan_of(Agent::Claude, "plus"), "unknown");
        for (raw, plan) in [
            ("free", "free"),
            ("plus", "plus"),
            ("pro", "pro"),
            ("business", "team"),
            ("edu", "enterprise"),
            ("enterprise", "enterprise"),
        ] {
            assert_eq!(plan_of(Agent::Codex, raw), plan);
        }
        assert_eq!(plan_of(Agent::Codex, "ada@example.com"), "unknown");
    }

    /// The rest of each agent's list, read trimmed and in any case.
    #[test]
    fn maps_any_case_and_spacing_to_the_contracts_list() {
        assert_eq!(
            ["free", " PRO ", "Max", "plus"].map(|raw| plan_of(Agent::Claude, raw)),
            ["free", "pro", "max", "unknown"]
        );
        assert_eq!(
            ["Team", " business ", "EDU", "max"].map(|raw| plan_of(Agent::Codex, raw)),
            ["team", "team", "enterprise", "unknown"]
        );
        assert_eq!(plan_of(Agent::Antigravity, "pro"), "unknown");
    }

    #[test]
    fn a_heartbeat_carries_only_the_contracts_fields() {
        let body = heartbeat_body("code");
        let mut keys: Vec<&String> = body.as_object().expect("object").keys().collect();
        keys.sort();
        assert_eq!(keys, ["app", "appVersion", "os", "surface"]);
        assert_eq!(body["app"], "cli");
        assert_eq!(body["surface"], "code");
        assert_eq!(heartbeat_body("/home/ada/acme")["surface"], "other");
        assert!(["macos", "windows", "linux", "other"].contains(&os()));
        let version = app_version();
        assert!((1..=32).contains(&version.len()));
        assert!(version.chars().all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c)));
    }

    #[test]
    fn the_terminal_counts_as_used_after_input_or_watched_output() {
        let now = SystemTime::now();
        let ago = |s: u64| Some(now - Duration::from_secs(s));
        assert!(active(now, ago(10), ago(600)));
        assert!(active(now, ago(200), ago(5)), "output while they watch");
        assert!(!active(now, ago(400), ago(5)), "output with nobody there");
        assert!(!active(now, ago(90), ago(90)));
        assert!(!active(now, None, None));
    }

    #[test]
    fn a_command_beats_once_a_minute_once_with_its_own_key() {
        let (url, seen) = gateway(200, Duration::ZERO);
        let config = dir().join("config.json");
        command_beat(&config, &url, "nx_key")
            .expect("due")
            .join()
            .expect("sent");
        assert!(command_beat(&config, &url, "nx_key").is_none());
        let seen = seen.lock().expect("seen");
        assert_eq!(seen.len(), 1);
        let (path, body, headers) = &seen[0];
        assert_eq!(path, HEARTBEAT_PATH);
        assert_eq!(body["surface"], "other");
        assert_eq!(headers.get("authorization").map(String::as_str), Some("Bearer nx_key"));
        assert!(headers.get("x-nexus-client").is_some_and(|c| c.starts_with("cli/")));
        // A fresh key: the Gateway counts a POST without one. Sent once, so never reused.
        assert!(headers.get("idempotency-key").is_some_and(|key| !key.is_empty()));
    }

    #[test]
    fn each_signal_gets_a_fresh_key_and_one_attempt() {
        let (url, seen) = gateway(503, Duration::ZERO);
        for _ in 0..2 {
            send(&url, "nx_key", HEARTBEAT_PATH, json!({}))
                .expect("spawned")
                .join()
                .expect("sent");
        }
        let seen = seen.lock().expect("seen");
        assert_eq!(seen.len(), 2, "a 503 is not retried");
        let keys: Vec<_> = seen
            .iter()
            .map(|(_, _, headers)| headers.get("idempotency-key").cloned())
            .collect();
        assert!(keys.iter().all(Option::is_some), "{keys:?}");
        assert_ne!(keys[0], keys[1], "a key is never reused across heartbeats");
    }

    #[test]
    fn counts_typed_commands_but_not_agent_sessions_or_completion() {
        let argv = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert!(counts_as_command(&argv(&["models"])));
        assert!(counts_as_command(&argv(&["--json", "spend"])));
        assert!(!counts_as_command(&argv(&["claude", "-p", "hi"])));
        assert!(!counts_as_command(&argv(&["--json", "codex"])));
        assert!(!counts_as_command(&argv(&["cursor-agent"])));
        assert!(!counts_as_command(&argv(&["__complete", "mo"])));
        assert!(!counts_as_command(&argv(&["--version"])));
    }

    #[test]
    fn a_session_beats_only_while_active_and_stops_without_waiting() {
        let (url, seen) = gateway(200, Duration::ZERO);
        let on = Arc::new(AtomicBool::new(false));
        let flag = on.clone();
        let beat = ActivityBeat::start_with(&url, "nx_key", Duration::from_millis(40), move || {
            flag.load(Ordering::SeqCst)
        });
        std::thread::sleep(Duration::from_millis(200));
        assert!(seen.lock().expect("seen").is_empty(), "idle: nothing");
        on.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(200));
        beat.stop();
        let seen = seen.lock().expect("seen");
        assert!(!seen.is_empty());
        assert!(
            seen.iter()
                .all(|(path, body, _)| path == HEARTBEAT_PATH && body["surface"] == "code")
        );
    }

    #[test]
    fn completion_the_agent_protocol_and_flags_alone_stay_local() {
        let argv = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        assert!(local_only(&argv(&["__complete", "mo"])));
        assert!(local_only(&argv(&["completion", "zsh"])));
        assert!(local_only(&argv(&["__agent"])));
        assert!(local_only(&argv(&["--version"])));
        assert!(!local_only(&argv(&["claude"])));
        assert!(!local_only(&argv(&[])), "a bare nexus is the home screen");
        assert!(!local_only(&argv(&["--json", "spend"])));
    }

    #[test]
    fn no_beat_goes_when_it_cannot_be_recorded() {
        let config = dir().join("config.json");
        std::fs::create_dir_all(beside(&config, BEAT_FILE)).expect("dir");
        assert!(command_beat(&config, "http://127.0.0.1:1", "nx_key").is_none());
    }

    #[test]
    fn nothing_goes_and_the_queue_stays_when_it_cannot_be_claimed() {
        let config = dir().join("config.json");
        record_error(&config, ErrorKind::Agent, "agent_spawn", "not_found");
        // A directory where the sending file goes: the claim cannot be written, so the same entries would go each run.
        let sending = beside(&config, SENDING_FILE);
        std::fs::create_dir_all(&sending).expect("dir");
        std::fs::File::open(&sending)
            .expect("open")
            .set_modified(SystemTime::now() - SENDING_STALE - Duration::from_secs(1))
            .expect("age");
        assert!(flush_errors(&config, "http://127.0.0.1:1", "nx_key").is_none());
        assert_eq!(queued(&config).len(), 1);
    }

    #[test]
    fn a_sending_file_dated_in_the_future_is_taken_over() {
        let (url, seen) = gateway(200, Duration::ZERO);
        let config = dir().join("config.json");
        let entry = json!({ "app": "cli", "kind": "update", "component": "updater", "code": "http_404", "occurredAt": rfc3339_ms(now_ms()) });
        write_beside(&config, SENDING_FILE, &json!([entry]));
        std::fs::File::options()
            .write(true)
            .open(beside(&config, SENDING_FILE))
            .expect("file")
            .set_modified(SystemTime::now() + Duration::from_secs(3600))
            .expect("future");
        flush_errors(&config, &url, "nx_key")
            .expect("taken over")
            .join()
            .expect("sent");
        assert_eq!(seen.lock().expect("seen").len(), 1);
    }

    #[test]
    fn forgetting_a_tier_keeps_the_time_of_the_read() {
        let config = dir().join("config.json");
        save_plan(&config, Agent::Codex, Some("business"));
        save_plan(&config, Agent::Codex, None);
        assert_eq!(saved_plan(&config, Agent::Codex), None);
        assert!(saved_at(&config, Agent::Codex).is_some(), "the read waits a day");
    }

    #[test]
    fn a_slow_gateway_is_left_at_five_seconds_and_a_refusal_is_not_retried() {
        let (url, seen) = gateway(200, Duration::from_secs(8));
        let started = std::time::Instant::now();
        send(&url, "nx_key", HEARTBEAT_PATH, heartbeat_body("other"))
            .expect("spawned")
            .join()
            .expect("sent");
        let took = started.elapsed();
        assert!(
            took >= SEND_TIMEOUT && took < SEND_TIMEOUT + Duration::from_secs(2),
            "{took:?}"
        );
        assert_eq!(seen.lock().expect("seen").len(), 1);

        let (url, seen) = gateway(503, Duration::ZERO);
        send(&url, "nx_key", HEARTBEAT_PATH, heartbeat_body("other"))
            .expect("spawned")
            .join()
            .expect("sent");
        assert_eq!(seen.lock().expect("seen").len(), 1, "a 503 is not retried");
    }

    #[test]
    fn queued_errors_go_once_with_no_content_and_at_most_twenty() {
        let (url, seen) = gateway(200, Duration::ZERO);
        let config = dir().join("config.json");
        record_error(&config, ErrorKind::Agent, "agent_spawn", "not_found");
        for _ in 0..MAX_ERRORS {
            record_error(&config, ErrorKind::Update, "updater", "http_404");
        }
        flush_errors(&config, &url, "nx_key")
            .expect("queued")
            .join()
            .expect("sent");
        assert!(flush_errors(&config, &url, "nx_key").is_none(), "never sent twice");
        let seen = seen.lock().expect("seen");
        assert_eq!(seen.len(), 1);
        let (path, body, _) = &seen[0];
        assert_eq!(path, ERRORS_PATH);
        let entries = body["errors"].as_array().expect("errors");
        assert_eq!(entries.len(), MAX_ERRORS);
        assert!(
            entries
                .iter()
                .all(|entry| entry["kind"] == "update" && entry["code"] == "http_404")
        );
        let mut keys: Vec<&String> = entries[0].as_object().expect("entry").keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "app",
                "appVersion",
                "code",
                "component",
                "fatal",
                "kind",
                "occurredAt",
                "os"
            ]
        );
    }

    #[test]
    fn errors_a_run_could_not_finish_sending_go_with_the_next_run_once_it_is_stale() {
        let (url, seen) = gateway(200, Duration::ZERO);
        let config = dir().join("config.json");
        let entry = json!({ "app": "cli", "kind": "update", "component": "updater", "code": "http_404", "occurredAt": rfc3339_ms(now_ms()) });
        write_beside(&config, SENDING_FILE, &json!([entry]));
        assert!(
            flush_errors(&config, &url, "nx_key").is_none(),
            "another run is sending"
        );
        let file = std::fs::File::options()
            .write(true)
            .open(beside(&config, SENDING_FILE))
            .expect("file");
        file.set_modified(SystemTime::now() - SENDING_STALE - Duration::from_secs(1))
            .expect("age");
        record_error(&config, ErrorKind::Agent, "agent_spawn", "not_found");
        flush_errors(&config, &url, "nx_key")
            .expect("stale")
            .join()
            .expect("sent");
        assert!(!beside(&config, SENDING_FILE).exists() && !beside(&config, ERRORS_FILE).exists());
        let seen = seen.lock().expect("seen");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].1["errors"].as_array().expect("errors").len(), 2);
    }

    #[test]
    fn names_outside_the_contract_become_fixed_values() {
        let config = dir().join("config.json");
        record_error(
            &config,
            ErrorKind::SignIn,
            "/Users/zoe/acme",
            "Invalid key for zoe@example.com",
        );
        let queue = queued(&config);
        assert_eq!(queue[0]["kind"], "sign_in");
        assert_eq!(queue[0]["component"], "unknown");
        assert_eq!(queue[0]["code"], "other");
        assert!(!queue[0].to_string().contains("zoe"));
    }

    #[test]
    fn old_errors_are_dropped_unsent() {
        let config = dir().join("config.json");
        write_beside(
            &config,
            ERRORS_FILE,
            &json!([{ "app": "cli", "kind": "agent", "component": "agent_spawn", "code": "x", "occurredAt": "2020-01-01T00:00:00.000Z" }]),
        );
        assert!(flush_errors(&config, "http://127.0.0.1:1", "nx_key").is_none());
        assert!(queued(&config).is_empty());
    }

    #[test]
    fn a_saved_plan_is_read_back_mapped_and_a_login_off_the_plan_forgets_it() {
        let config = dir().join("config.json");
        assert_eq!(saved_plan(&config, Agent::Codex), None);
        save_plan(&config, Agent::Codex, Some("business"));
        save_plan(&config, Agent::Claude, Some("max"));
        assert_eq!(saved_plan(&config, Agent::Codex).as_deref(), Some("team"));
        assert_eq!(saved_plan(&config, Agent::Claude).as_deref(), Some("max"));
        save_plan(&config, Agent::Codex, None);
        assert_eq!(saved_plan(&config, Agent::Codex), None);
        write_beside(
            &config,
            PLANS_FILE,
            &json!({ "claude": { "plan": "ada@example.com", "at": 1 } }),
        );
        assert_eq!(saved_plan(&config, Agent::Claude), None);
    }

    #[test]
    fn writes_times_with_milliseconds() {
        assert_eq!(rfc3339_ms(1_762_709_462_120), "2025-11-09T17:31:02.120Z");
        assert_eq!(rfc3339_ms(1_762_709_462_000), "2025-11-09T17:31:02.000Z");
    }
}
