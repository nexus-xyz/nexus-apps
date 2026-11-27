//! A project's setup script (`nexus_workbench::setup`): it runs in a new session's worktree before the agent's first
//! turn, its output lands in the session's log, and a failure is shown there without keeping the agent from starting.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use nexus_workbench::event::{PermissionMode, SessionAgent, SetupStatus, StoredEvent, TurnStatus, Usage, Via, WbEvent};
use nexus_workbench::process::{self, Spawn};
use nexus_workbench::store::Store;
use nexus_workbench::{Config, Emit, Host, Launch, LaunchRequest, NewThread, NewWorktree, ProcessEnd, Workbench};

#[derive(Default)]
struct TestHost {
    memory: PathBuf,
    launches: Mutex<Vec<LaunchRequest>>,
}

impl Host for TestHost {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch, String>> {
        self.launches
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(request.clone());
        let env: HashMap<String, String> = [
            (
                "ANTHROPIC_BASE_URL".to_string(),
                "https://gateway.test/v1/anthropic".to_string(),
            ),
            ("FAKE_CLAUDE_DIR".to_string(), self.memory.to_string_lossy().to_string()),
        ]
        .into_iter()
        .collect();
        Box::pin(async move {
            Ok(Launch {
                session_mcp: Default::default(),
                attribution: None,
                spawn: Spawn {
                    program: PathBuf::from(env!("CARGO_BIN_EXE_fake-claude")),
                    args: request.args,
                    env,
                    cwd: request.cwd,
                },
                nexus_session: None,
                local_run: None,
                local_provider: None,
                model: None,
            })
        })
    }

    fn ended(&self, _nexus_session: String, _end: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }

    fn price(&self, _model: &str, _usage: &Usage) -> Option<f64> {
        None
    }

    fn emit(&self, _emit: Emit) {}
}

fn git(dir: &Path, args: &[&str]) {
    let ok = process::command("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(dir)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "git {args:?} failed in {}", dir.display());
}

struct Bench {
    workbench: Workbench,
    project: String,
    root: PathBuf,
}

/// A workbench with one git project and a worktrees folder.
fn bench(name: &str) -> Bench {
    let root = std::env::temp_dir().join(format!("nexus-workbench-setup-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("repo");
    let memory = root.join("memory");
    let _ = std::fs::create_dir_all(&repo);
    let _ = std::fs::create_dir_all(&memory);
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    git(&repo, &["commit", "--quiet", "--allow-empty", "-m", "one"]);
    let host = Arc::new(TestHost {
        memory,
        ..TestHost::default()
    });
    let store = Store::open(&root.join("workbench.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "gw|org".into(),
        worktrees: Some(root.join("worktrees")),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(store, host, config).unwrap_or_else(|e| panic!("{e}"));
    let project = workbench
        .add_project(&repo)
        .map(|p| p.id)
        .unwrap_or_else(|e| panic!("{e}"));
    Bench {
        workbench,
        project,
        root,
    }
}

fn new_thread(project: &str, text: &str, worktree: bool) -> NewThread {
    NewThread {
        name: None,
        project_id: project.into(),
        agent: SessionAgent::Claude,
        model: None,
        permission_mode: PermissionMode::Default,
        effort: None,
        via: Via::Nexus,
        provider: None,
        text: text.into(),
        images: vec![],
        worktree: worktree.then(NewWorktree::default),
        parent: None,
        orchestration: false,
    }
}

async fn wait_for(workbench: &Workbench, id: &str, done: impl Fn(&[StoredEvent]) -> bool) -> Vec<StoredEvent> {
    for _ in 0..400 {
        let events = workbench.events(id, 0).unwrap_or_default();
        if done(&events) {
            return events;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out; events: {:#?}", workbench.events(id, 0).unwrap_or_default());
}

fn turn_ended(events: &[StoredEvent]) -> bool {
    events.iter().any(|e| matches!(e.event, WbEvent::TurnEnd { .. }))
}

/// The events that matter here, in order; the setup's updates while it runs count as one.
fn kinds(events: &[StoredEvent]) -> Vec<&'static str> {
    let mut kinds: Vec<&'static str> = events
        .iter()
        .map(|e| match &e.event {
            WbEvent::User { .. } => "user",
            WbEvent::Setup { status, .. } => match status {
                SetupStatus::Running => "setup",
                _ => "setupEnd",
            },
            WbEvent::TurnStart { .. } => "turnStart",
            WbEvent::TurnEnd { .. } => "turnEnd",
            _ => "other",
        })
        .filter(|kind| *kind != "other")
        .collect();
    kinds.dedup();
    kinds
}

/// Everything the setup said, in order, from all of its events.
fn setup_output(events: &[StoredEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Setup { output, .. } => Some(output.as_str()),
            _ => None,
        })
        .collect()
}

fn setup_end(events: &[StoredEvent]) -> Option<(SetupStatus, Option<i32>)> {
    events.iter().rev().find_map(|e| match &e.event {
        WbEvent::Setup { status, exit_code, .. } if *status != SetupStatus::Running => Some((*status, *exit_code)),
        _ => None,
    })
}

#[tokio::test]
async fn runs_the_setup_script_in_the_new_worktree_before_the_first_turn() {
    let b = bench("ok");
    let saved = b
        .workbench
        .set_project_setup(
            &b.project,
            Some("  echo installing; echo \"$NEXUS_PROJECT_ROOT\" > from-setup.txt\n"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        saved.as_deref(),
        Some("echo installing; echo \"$NEXUS_PROJECT_ROOT\" > from-setup.txt")
    );
    assert_eq!(b.workbench.project_setup(&b.project).ok().flatten(), saved);

    let created = b
        .workbench
        .create_thread(new_thread(&b.project, "Fix the login bug", true))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let worktree = created.worktree.clone().unwrap_or_else(|| panic!("a worktree"));
    let events = wait_for(&b.workbench, &created.id, turn_ended).await;

    // The setup shows first, the message right after it, and the agent starts only once the setup ended.
    assert_eq!(kinds(&events), ["setup", "user", "setupEnd", "turnStart", "turnEnd"]);
    assert_eq!(setup_end(&events), Some((SetupStatus::Done, Some(0))));
    assert!(setup_output(&events).contains("installing"), "{events:#?}");
    let WbEvent::Setup { command, .. } = &events[0].event else {
        panic!("not the setup")
    };
    assert_eq!(command, saved.as_deref().unwrap_or_default());
    // It ran in the worktree, and knew where the project's own folder is.
    let written = std::fs::read_to_string(Path::new(&worktree.path).join("from-setup.txt")).unwrap_or_default();
    assert_eq!(written.trim(), b.root.join("repo").to_string_lossy());

    // A session in the project's own folder has nothing to set up.
    let plain = b
        .workbench
        .create_thread(new_thread(&b.project, "elsewhere", false))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&b.workbench, &plain.id, turn_ended).await;
    assert_eq!(kinds(&events), ["user", "turnStart", "turnEnd"]);

    // Cleared: new worktrees start without it.
    assert_eq!(b.workbench.set_project_setup(&b.project, Some("  ")).ok(), Some(None));
    assert_eq!(b.workbench.project_setup(&b.project).ok(), Some(None));
    let _ = std::fs::remove_dir_all(&b.root);
}

#[tokio::test]
async fn a_failed_setup_is_shown_and_the_agent_starts_anyway() {
    let b = bench("fail");
    b.workbench
        .set_project_setup(&b.project, Some("echo missing-tool >&2; exit 3"))
        .unwrap_or_else(|e| panic!("{e}"));
    let created = b
        .workbench
        .create_thread(new_thread(&b.project, "Go", true))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&b.workbench, &created.id, turn_ended).await;
    assert_eq!(setup_end(&events), Some((SetupStatus::Failed, Some(3))));
    assert!(setup_output(&events).contains("missing-tool"), "{events:#?}");
    assert_eq!(kinds(&events), ["setup", "user", "setupEnd", "turnStart", "turnEnd"]);
    let completed = events.iter().any(|e| {
        matches!(
            e.event,
            WbEvent::TurnEnd {
                status: TurnStatus::Completed,
                ..
            }
        )
    });
    assert!(completed, "{events:#?}");

    // A message sent while the setup runs waits for it too, behind the first.
    b.workbench
        .set_project_setup(&b.project, Some("sleep 1; echo slow"))
        .unwrap_or_else(|e| panic!("{e}"));
    let created = b
        .workbench
        .create_thread(new_thread(&b.project, "First", true))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    // Setup is registered before the queued first message runs, leaving no restart gap.
    assert!(b.workbench.reserve_update_restart().is_err());
    wait_for(&b.workbench, &created.id, |e| kinds(e).contains(&"user")).await;
    // Several messages wait together: all are released when the setup ends, and must reach the agent as sent.
    for text in ["Second", "Third"] {
        b.workbench
            .send(&created.id, text)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let events = wait_for(&b.workbench, &created.id, |e| {
        e.iter().filter(|e| matches!(e.event, WbEvent::TurnEnd { .. })).count() == 3
    })
    .await;
    // The order the agent read them in (the log's order is the order they were sent, which proves nothing here).
    // Each session writes its own file, and the directory lists them in no fixed order, so pick this session's
    // file by its content (the one that starts with "First") instead of by position.
    let sessions: Vec<Vec<String>> = std::fs::read_dir(b.root.join("memory"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "prompts"))
        .map(|entry| {
            std::fs::read_to_string(entry.path())
                .unwrap_or_default()
                .lines()
                .map(String::from)
                .collect()
        })
        .collect();
    let read = sessions
        .iter()
        .find(|lines| {
            lines
                .first()
                .is_some_and(|l| l.starts_with("First  [Nexus desktop navigation metadata]"))
        })
        .unwrap_or_else(|| panic!("no session read \"First\" first: {sessions:#?}"));
    assert_eq!(read[1..], ["Second", "Third"]);
    let users: Vec<&str> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::User { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(users, ["First", "Second", "Third"]);
    let order = kinds(&events);
    let ended = order.iter().position(|k| *k == "setupEnd");
    let started = order.iter().position(|k| *k == "turnStart");
    assert!(ended < started, "{order:?}");
    let _ = std::fs::remove_dir_all(&b.root);
}

#[tokio::test]
async fn a_stopped_setup_lets_the_waiting_messages_through() {
    let b = bench("stop");
    b.workbench
        .set_project_setup(&b.project, Some("echo begun; sleep 60"))
        .unwrap_or_else(|e| panic!("{e}"));
    let created = b
        .workbench
        .create_thread(new_thread(&b.project, "First", true))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&b.workbench, &created.id, |e| setup_output(e).contains("begun")).await;
    assert!(b.workbench.stop_setup(&created.id));
    let events = wait_for(&b.workbench, &created.id, turn_ended).await;
    assert_eq!(setup_end(&events), Some((SetupStatus::Stopped, None)));
    // The waiting message reached the agent once the setup ended, and the turn ran.
    let order = kinds(&events);
    assert!(order.ends_with(&["setupEnd", "turnStart", "turnEnd"]), "{order:?}");
    // Ended: nothing left to stop.
    assert!(!b.workbench.stop_setup(&created.id));
    let _ = std::fs::remove_dir_all(&b.root);
}

#[tokio::test]
async fn a_setup_cut_short_by_quitting_ends_as_failed_on_the_next_start() {
    let root = std::env::temp_dir().join(format!("nexus-workbench-setup-quit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::create_dir_all(&root);
    let store = Store::open(&root.join("workbench.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let setup = |id: &str, status| WbEvent::Setup {
        id: id.into(),
        command: "npm ci".into(),
        status,
        output: String::new(),
        exit_code: None,
        duration_ms: None,
    };
    // One setup quit during, one that ended.
    for (thread, id, status) in [
        ("t1", "setup_t1", SetupStatus::Running),
        ("t2", "setup_t2", SetupStatus::Running),
        ("t2", "setup_t2", SetupStatus::Done),
    ] {
        store
            .append(thread, 1, &setup(id, status))
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let host = Arc::new(TestHost::default());
    let workbench = Workbench::new(
        store,
        host,
        Config {
            shell: nexus_workbench::process::Shell::sh(),
            ..Config::default()
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let ends = |thread: &str| setup_end(&workbench.events(thread, 0).unwrap_or_default());
    assert_eq!(ends("t1"), Some((SetupStatus::Failed, None)));
    assert_eq!(ends("t2"), Some((SetupStatus::Done, None)));
    let _ = std::fs::remove_dir_all(&root);
}
