//! The harness the linked-session tests share: a workbench over a git repository, driving the stand-in `claude` and
//! `codex`, and the helpers that poll its logs with a deadline.

#![allow(dead_code)]

pub mod watch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use nexus_workbench::event::{
    Effort, PermissionMode, Sender, SessionAgent, StoredEvent, ThreadSummary, TurnStatus, Usage, Via, WbEvent,
};
use nexus_workbench::process::{self, Spawn};
use nexus_workbench::store::Store;
use nexus_workbench::{
    Config, Emit, Host, Launch, LaunchRequest, Limits, NewThread, OrchCode, OrchError, ProcessEnd, StartChild,
    ThreadPatch, Workbench,
};

/// The model the host has no list price for.
pub const UNPRICED: &str = "unpriced-model";

/// A priced call's cost, per output token, and the catalog's highest price, per output token.
pub const PRICE: f64 = 0.0001;
pub const TOP_PRICE: f64 = 0.001;

#[derive(Default)]
pub struct TestHost {
    pub sync: Mutex<Option<Arc<dyn nexus_workbench::session_sync::Backend>>>,
    pub launches: Mutex<Vec<LaunchRequest>>,
    pub choice_identity: Mutex<Option<String>>,
    pub memory: PathBuf,
    /// More switches for the stand-ins, set before a launch.
    pub extra: Mutex<HashMap<String, String>>,
    /// The host has no catalog: no model has a price, and there is no highest one.
    pub no_prices: AtomicBool,
    pub call_price: Mutex<Option<f64>>,
    /// The catalog loads when the workbench asks for it (`ensure_prices`), as the app's does at sign-in.
    pub prices_on_ensure: AtomicBool,
    /// The summaries the workbench sent (`wb:thread`), in order.
    pub summaries: Mutex<Vec<ThreadSummary>>,
    pub turn_end_events: Mutex<Vec<StoredEvent>>,
    pub process_ends: Mutex<Vec<ProcessEnd>>,
    /// The program Nexus's agent starts as, instead of `fake-nexus`: the real CLI (`tests/nexus_runtime.rs`).
    pub nexus: Mutex<Option<PathBuf>>,
    /// Antigravity's Sandbox switch, as Settings would answer it.
    pub antigravity_sandbox: AtomicBool,
}

impl Host for TestHost {
    fn session_sync(&self) -> Option<Arc<dyn nexus_workbench::session_sync::Backend>> {
        self.sync.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    fn session_choice_identity(&self) -> Option<String> {
        self.choice_identity
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .or_else(|| Some("test-person".into()))
    }
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch, String>> {
        let n = {
            let mut launches = self.launches.lock().unwrap_or_else(|p| p.into_inner());
            launches.push(request.clone());
            launches.len()
        };
        let memory = self.memory.to_string_lossy().into_owned();
        let extra = self.extra.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let (program, env) = match request.agent {
            SessionAgent::Claude => (
                env!("CARGO_BIN_EXE_fake-claude"),
                [("FAKE_CLAUDE_DIR".to_string(), memory)],
            ),
            SessionAgent::Codex => (
                env!("CARGO_BIN_EXE_fake-codex"),
                [("FAKE_CODEX_DIR".to_string(), memory)],
            ),
            SessionAgent::Nexus => (
                env!("CARGO_BIN_EXE_fake-nexus"),
                [("FAKE_NEXUS_DIR".to_string(), memory)],
            ),
            SessionAgent::Antigravity => (env!("CARGO_BIN_EXE_fake-agy"), [("FAKE_AGY_DIR".to_string(), memory)]),
            #[cfg(feature = "cursor")]
            SessionAgent::Cursor => (
                env!("CARGO_BIN_EXE_fake-agy"),
                [("FAKE_CURSOR_DIR".to_string(), memory)],
            ),
            #[cfg(feature = "opencode")]
            SessionAgent::Opencode => (env!("CARGO_BIN_EXE_fake-agy"), [("FAKE_AGY_DIR".to_string(), memory)]),
        };
        let program = match (
            request.agent,
            self.nexus.lock().unwrap_or_else(|p| p.into_inner()).clone(),
        ) {
            (SessionAgent::Nexus, Some(nexus)) => nexus,
            _ => PathBuf::from(program),
        };
        Box::pin(async move {
            Ok(Launch {
                session_mcp: Default::default(),
                attribution: None,
                spawn: Spawn {
                    program,
                    args: request.args,
                    env: env.into_iter().chain(extra).collect::<HashMap<_, _>>(),
                    cwd: request.cwd,
                },
                nexus_session: Some(format!("ns_{n}")),
                local_run: None,
                local_provider: None,
                model: None,
            })
        })
    }

    fn ended(&self, _nexus_session: String, end: ProcessEnd) -> BoxFuture<'static, bool> {
        self.process_ends.lock().unwrap_or_else(|p| p.into_inner()).push(end);
        Box::pin(async { true })
    }

    fn price(&self, model: &str, usage: &Usage) -> Option<f64> {
        (model != UNPRICED && !self.no_prices.load(Ordering::SeqCst)).then(|| {
            self.call_price
                .lock()
                .expect("fixture call price")
                .unwrap_or(usage.output_tokens as f64 * PRICE)
        })
    }

    fn top_price(&self, usage: &Usage) -> Option<f64> {
        (!self.no_prices.load(Ordering::SeqCst)).then_some(usage.output_tokens as f64 * TOP_PRICE)
    }

    fn ensure_prices(&self) -> BoxFuture<'static, ()> {
        if self.prices_on_ensure.load(Ordering::SeqCst) {
            self.no_prices.store(false, Ordering::SeqCst);
        }
        Box::pin(async {})
    }

    fn antigravity_sandbox(&self) -> bool {
        self.antigravity_sandbox.load(Ordering::SeqCst)
    }

    fn emit(&self, emit: Emit) {
        match emit {
            Emit::Thread(summary) => {
                self.summaries.lock().unwrap_or_else(|p| p.into_inner()).push(*summary);
            }
            Emit::Event(event) if matches!(event.event, WbEvent::TurnEnd { .. }) => {
                self.turn_end_events
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(event);
            }
            _ => {}
        }
    }
}

pub struct Setup {
    pub host: Arc<TestHost>,
    pub workbench: Workbench,
    pub project: String,
    pub root: PathBuf,
}

pub fn git(dir: &Path, args: &[&str]) {
    let ok = process::command("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .status()
        .is_ok_and(|status| status.success());
    assert!(ok, "git {args:?}");
}

/// A number on each limit, so a test of one limit runs with all of them on, as a person who turns limits on and sets
/// each might. No spend cap. [`Limits::default`] sets no limit at all.
pub fn bounded() -> Limits {
    Limits {
        max_depth: Some(1),
        max_live_children: Some(4),
        max_live_linked: Some(8),
        max_child_worktrees: Some(16),
        max_starts: Some(8),
        max_auto_turns: Some(12),
        max_idle_waits: Some(30),
        crew_spend_usd: None,
    }
}

/// A workbench over a git repository with one commit on `main`, making worktrees, with these limits.
pub fn setup(name: &str, limits: Limits) -> Setup {
    setup_with(name, limits, true, |config| config)
}

pub fn setup_with(name: &str, limits: Limits, repository: bool, config: impl FnOnce(Config) -> Config) -> Setup {
    let root = std::env::temp_dir().join(format!("nexus-workbench-linked-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (repo, memory) = (root.join("repo"), root.join("memory"));
    let _ = std::fs::create_dir_all(&repo);
    let _ = std::fs::create_dir_all(&memory);
    if repository {
        git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        git(&repo, &["commit", "--quiet", "--allow-empty", "-m", "one"]);
    }
    let host = Arc::new(TestHost {
        memory,
        ..TestHost::default()
    });
    let store = Store::open(&root.join("workbench.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let config = config(Config {
        scope: "gw|org".into(),
        worktrees: Some(root.join("worktrees")),
        orchestration: limits,
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    });
    let workbench = Workbench::new(store, host.clone(), config).unwrap_or_else(|e| panic!("{e}"));
    let project = workbench
        .add_project(&repo)
        .map(|p| p.id)
        .unwrap_or_else(|e| panic!("{e}"));
    Setup {
        host,
        workbench,
        project,
        root,
    }
}

pub fn new_root(project: &str, text: &str) -> NewThread {
    NewThread {
        name: None,
        project_id: project.into(),
        agent: SessionAgent::Claude,
        model: None,
        permission_mode: PermissionMode::AcceptEdits,
        effort: None,
        via: Via::Nexus,
        provider: None,
        text: text.into(),
        images: vec![],
        worktree: None,
        parent: None,
        orchestration: true,
    }
}

/// A root with orchestration on whose first turn has ended.
pub async fn root(s: &Setup, new: NewThread) -> ThreadSummary {
    let created = s.workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    thread(&s.workbench, &created.id)
}

pub fn child(agent: SessionAgent, prompt: &str) -> StartChild {
    StartChild {
        agent,
        prompt: prompt.into(),
        title: None,
        model: None,
        effort: None,
        permission_mode: None,
        key: None,
    }
}

/// A child of `parent` started and its first turn ended.
pub async fn finished_child(s: &Setup, parent: &str, start: StartChild) -> ThreadSummary {
    let started = s
        .workbench
        .start_child(parent, start)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &started.id, |e| turn_ends(e).len() == 1).await;
    thread(&s.workbench, &started.id)
}

/// The last summary of session `id` the workbench sent: what the app shows.
pub fn shown(s: &Setup, id: &str) -> Option<ThreadSummary> {
    let summaries = s.host.summaries.lock().unwrap_or_else(|p| p.into_inner());
    summaries.iter().rev().find(|summary| summary.id == id).cloned()
}

pub fn thread(workbench: &Workbench, id: &str) -> ThreadSummary {
    workbench
        .threads()
        .unwrap_or_default()
        .into_iter()
        .find(|t| t.id == id)
        .unwrap_or_else(|| panic!("no thread {id}"))
}

pub async fn wait_for(workbench: &Workbench, id: &str, done: impl Fn(&[StoredEvent]) -> bool) -> Vec<StoredEvent> {
    for _ in 0..400 {
        let events = workbench.events(id, 0).unwrap_or_default();
        if done(&events) {
            return events;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out; events: {:#?}", workbench.events(id, 0).unwrap_or_default());
}

/// The worktrees in `dir`, sorted.
pub fn worktrees_in(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|dir| dir.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect())
        .unwrap_or_default();
    found.sort();
    found
}

/// Waits until `dir` holds exactly the worktrees `expected` (sorted), for up to 20 s.
pub async fn until_worktrees(dir: &Path, expected: &[PathBuf]) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while worktrees_in(dir) != expected {
        assert!(
            Instant::now() < deadline,
            "worktrees {:?}, expected {expected:?}",
            worktrees_in(dir)
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub async fn until_stopped(workbench: &Workbench, id: &str) {
    for _ in 0..400 {
        if !workbench.is_live(id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the agent of {id} is still running");
}

pub fn turn_ends(events: &[StoredEvent]) -> Vec<TurnStatus> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::TurnEnd { status, .. } => Some(*status),
            _ => None,
        })
        .collect()
}

pub fn users(events: &[StoredEvent]) -> Vec<(String, Option<Sender>)> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::User { text, from, .. } => Some((text.clone(), from.clone())),
            _ => None,
        })
        .collect()
}

pub fn launches_of(s: &Setup, id: &str) -> Vec<LaunchRequest> {
    s.host
        .launches
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|l| l.thread_id == id)
        .cloned()
        .collect()
}

pub fn code(result: Result<ThreadSummary, OrchError>) -> OrchCode {
    match result {
        Ok(summary) => panic!("started {}", summary.id),
        Err(error) => error.code,
    }
}

pub fn mode(mode: PermissionMode) -> ThreadPatch {
    ThreadPatch {
        permission_mode: Some(mode),
        ..ThreadPatch::default()
    }
}

pub fn effort(effort: Option<Effort>) -> ThreadPatch {
    ThreadPatch {
        effort: Some(effort),
        ..ThreadPatch::default()
    }
}

/// The approval the session waits on: its request id.
pub async fn approval(workbench: &Workbench, id: &str) -> String {
    let events = wait_for(workbench, id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Approval { .. }))
    })
    .await;
    events
        .iter()
        .find_map(|e| match &e.event {
            WbEvent::Approval { request_id, .. } => Some(request_id.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// A root with orchestration on, titled "Lead", whose first turn is under way (one that runs `relay`, say).
pub async fn root_at_work(s: &Setup, new: NewThread) -> ThreadSummary {
    let created = s.workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
    // A plain title: the stand-ins act on words in what they read, and a child's messages name its parent.
    s.workbench
        .update(
            &created.id,
            ThreadPatch {
                title: Some("Lead".into()),
                ..ThreadPatch::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::TurnStart { .. }))
    })
    .await;
    thread(&s.workbench, &created.id)
}

/// A child of `parent` titled `title`, started on `prompt`.
pub async fn start(s: &Setup, parent: &str, agent: SessionAgent, title: &str, prompt: &str) -> ThreadSummary {
    s.workbench
        .start_child(
            parent,
            StartChild {
                title: Some(title.into()),
                ..child(agent, prompt)
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"))
}

/// Polls `done` until it holds, for up to 20 s.
pub async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// A file the stand-ins look for in their folder (`bgagent-end`, say).
pub fn trigger(s: &Setup, name: &str) {
    trigger_with(s, name, "");
}

/// Publishes a whole message before a stand-in can consume the file.
pub fn trigger_with(s: &Setup, name: &str, text: &str) {
    let staged = s.host.memory.join(format!("{name}.tmp"));
    std::fs::write(&staged, text).unwrap_or_else(|e| panic!("{e}"));
    std::fs::rename(staged, s.host.memory.join(name)).unwrap_or_else(|e| panic!("{e}"));
}

/// The tool results in a log: each output and whether it is an error.
pub fn tool_results(events: &[StoredEvent]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::ToolResult { output, is_error, .. } => Some((output.clone(), *is_error)),
            _ => None,
        })
        .collect()
}

/// Hands the lead's stand-in, in its `relay` turn, `text` as a tool's answer, and waits until it is logged.
pub async fn relay(s: &Setup, lead: &str, text: &str) {
    let before = tool_results(&s.workbench.events(lead, 0).unwrap_or_default()).len();
    trigger_with(s, "relay", text);
    wait_for(&s.workbench, lead, |e| tool_results(e).len() > before).await;
}

/// Ends the lead's `relay` turn, and waits for its end.
pub async fn end_relay(s: &Setup, lead: &str) {
    let before = turn_ends(&s.workbench.events(lead, 0).unwrap_or_default()).len();
    trigger_with(s, "relay", "END");
    wait_for(&s.workbench, lead, |e| turn_ends(e).len() > before).await;
}

/// Whether `child`'s last result is newer than what `parent` read (`list_sessions`).
pub fn has_result(workbench: &Workbench, parent: &str, child: &str) -> bool {
    workbench
        .list_children(parent)
        .unwrap_or_else(|e| panic!("{e}"))
        .sessions
        .into_iter()
        .find(|session| session.session_id == child)
        .unwrap_or_else(|| panic!("{child} is not a child of {parent}"))
        .has_result
}

/// The seqs of a log's turn ends.
pub fn turn_end_seqs(events: &[StoredEvent]) -> Vec<i64> {
    events
        .iter()
        .filter(|e| matches!(e.event, WbEvent::TurnEnd { .. }))
        .map(|e| e.seq)
        .collect()
}

/// A new workbench on `s`'s store, as the app's next launch makes one.
pub fn reopen(s: &Setup, limits: Limits) -> Workbench {
    let store = Store::open(&s.root.join("workbench.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "gw|org".into(),
        worktrees: Some(s.root.join("worktrees")),
        orchestration: limits,
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    Workbench::new(store, s.host.clone(), config).unwrap_or_else(|e| panic!("{e}"))
}

/// Waits until the turn that took the message `text` ended.
pub async fn answered(workbench: &Workbench, id: &str, text: &str) -> Vec<StoredEvent> {
    wait_for(workbench, id, |e| {
        e.iter()
            .position(|e| matches!(&e.event, WbEvent::User { text: said, .. } if said == text))
            .is_some_and(|at| e[at..].iter().any(|e| matches!(e.event, WbEvent::TurnEnd { .. })))
    })
    .await
}
