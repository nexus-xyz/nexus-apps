use std::sync::Mutex;
use std::time::Instant;

use futures::future::BoxFuture;
use serde_json::json;

use super::*;
use crate::event::{Usage, Worktree};
use crate::store::{Store, ThreadRecord};
use crate::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd};

fn hook(event: HookEvent, command: &str) -> Hook {
    Hook {
        event,
        command: command.into(),
        timeout_secs: None,
    }
}

fn summary(id: &str, project_id: &str) -> ThreadSummary {
    serde_json::from_value(json!({
        "id": id, "projectId": project_id, "title": "Fix the login", "agent": "claude", "model": null,
        "permissionMode": "default", "via": "subscription", "status": "idle", "costUsd": 0.0,
        "nexusSessions": [], "contextTokens": null, "contextWindow": null, "createdAt": 1, "updatedAt": 1,
        "archived": false,
    }))
    .expect("summary")
}

#[test]
fn only_the_events_hooks_run_and_none_when_the_policy_is_off() {
    let mut hooks = Hooks {
        enabled: true,
        list: vec![
            hook(HookEvent::TurnFinished, "say done"),
            hook(HookEvent::NeedsYou, "say help"),
            hook(HookEvent::TurnFinished, "  "),
        ],
    };
    let commands: Vec<&str> = hooks
        .matching(HookEvent::TurnFinished)
        .iter()
        .map(|hook| hook.command.as_str())
        .collect();
    assert_eq!(commands, ["say done"]);
    assert!(hooks.matching(HookEvent::Archived).is_empty());
    hooks.enabled = false;
    assert!(hooks.matching(HookEvent::TurnFinished).is_empty());
}

#[test]
fn settings_round_trip_with_camel_case_names_and_a_clamped_timeout() {
    let text = r#"[{"event":"checkFailed","command":"notify","timeoutSecs":9000},{"event":"settled","command":"x"}]"#;
    let hooks: Vec<Hook> = serde_json::from_str(text).expect("hooks");
    assert_eq!(hooks[0].event, HookEvent::CheckFailed);
    assert_eq!(hooks[0].timeout(), Duration::from_secs(MAX_TIMEOUT_SECS));
    assert_eq!(hooks[1].timeout(), Duration::from_secs(DEFAULT_TIMEOUT_SECS));
    assert_eq!(serde_json::to_string(&hooks).expect("json"), text);
    for event in HookEvent::ALL {
        assert_eq!(HookEvent::parse(event.as_str()), Some(event));
    }
    // The id follows the hook, not its place in the list.
    assert_eq!(hooks[0].id(), hooks[0].clone().id());
    assert_ne!(hooks[0].id(), hooks[1].id());
}

#[test]
fn log_events_map_to_turn_finished_and_needs_you() {
    let stored = |event: WbEvent| StoredEvent {
        thread_id: "t".into(),
        seq: 7,
        at: 100,
        event,
    };
    let end = |status| WbEvent::TurnEnd {
        turn_id: "turn".into(),
        status,
        cost_usd: None,
        duration_ms: 0,
        error: None,
        cursor: None,
    };
    let done = fire_for("t", &stored(end(TurnStatus::Completed))).expect("done");
    assert_eq!(
        (done.event, done.turn_result, done.key.as_str()),
        (HookEvent::TurnFinished, Some("completed"), "7@100")
    );
    let error = fire_for("t", &stored(end(TurnStatus::Error))).expect("error");
    assert_eq!(error.turn_result, Some("error"));
    assert_eq!(fire_for("t", &stored(end(TurnStatus::Interrupted))), None);
    let approval = WbEvent::Approval {
        request_id: "r".into(),
        tool_use_id: None,
        tool: "AskUserQuestion".into(),
        input: json!({}),
        description: None,
        suggestions: vec![],
        parent: None,
    };
    assert_eq!(
        fire_for("t", &stored(approval)).map(|f| f.event),
        Some(HookEvent::NeedsYou)
    );
    assert_eq!(fire_for("t", &stored(WbEvent::Notice { text: "x".into() })), None);
}

#[test]
fn env_names_the_session_its_worktree_and_the_check() {
    let mut session = summary("t1", "p1");
    session.worktree = Some(Worktree {
        path: "/w/login".into(),
        branch: "nexus/login".into(),
        base: "origin/main".into(),
        removed: false,
        pull_request: Some("https://github.com/a/b/pull/3".into()),
        pull_requests: Vec::new(),
        review: None,
    });
    let project: Project = serde_json::from_value(json!({
        "id": "p1", "name": "acme", "path": "/src/acme", "branch": "main", "addedAt": 1
    }))
    .expect("project");
    let fire = Fire {
        check: Some(("build".into(), Some("https://ci/1".into()))),
        ..Fire::plain(HookEvent::CheckFailed, "t1", "k".into())
    };
    let env: std::collections::HashMap<String, String> = env(&fire, &session, Some(&project)).into_iter().collect();
    let want = [
        ("NEXUS_HOOK_EVENT", "checkFailed"),
        ("NEXUS_SESSION_ID", "t1"),
        ("NEXUS_SESSION_TITLE", "Fix the login"),
        ("NEXUS_PROJECT", "acme"),
        ("NEXUS_PROJECT_PATH", "/src/acme"),
        ("NEXUS_WORKTREE", "/w/login"),
        ("NEXUS_BRANCH", "nexus/login"),
        ("NEXUS_PULL_REQUEST", "https://github.com/a/b/pull/3"),
        ("NEXUS_TURN_RESULT", ""),
        ("NEXUS_CHECK_NAME", "build"),
        ("NEXUS_CHECK_URL", "https://ci/1"),
    ];
    for (name, value) in want {
        assert_eq!(env.get(name).map(String::as_str), Some(value), "{name}");
    }
    // No worktree: the project's branch, and empty where nothing is known.
    let env: std::collections::HashMap<String, String> = super::env(
        &Fire::plain(HookEvent::Archived, "t2", "k".into()),
        &summary("t2", "p1"),
        Some(&project),
    )
    .into_iter()
    .collect();
    assert_eq!(env["NEXUS_BRANCH"], "main");
    assert_eq!(env["NEXUS_WORKTREE"], "");
    assert_eq!(env["NEXUS_PULL_REQUEST"], "");
}

#[test]
fn a_second_claim_of_the_same_event_skips() {
    let store = Store::in_memory().expect("store");
    assert!(store.claim_hook("h", "t", "turnFinished", "7@1").expect("claim"));
    assert!(!store.claim_hook("h", "t", "turnFinished", "7@1").expect("again"));
    assert!(store.claim_hook("h", "t", "turnFinished", "8@2").expect("next event"));
    assert!(
        store
            .claim_hook("other", "t", "turnFinished", "7@1")
            .expect("other hook")
    );
}

#[test]
fn each_hook_keeps_its_last_runs() {
    let store = Store::in_memory().expect("store");
    let run = |hook: &str, n: i64| HookRun {
        hook: hook.into(),
        event: "settled".into(),
        thread_id: "t".into(),
        started_at: n,
        exit_code: Some(0),
        timed_out: false,
        output: format!("run {n}"),
    };
    for n in 0..(KEEP_RUNS as i64 + 5) {
        store.add_hook_run(&run("a", n)).expect("add");
    }
    store.add_hook_run(&run("b", 0)).expect("add");
    let runs = store.hook_runs().expect("runs");
    let a: Vec<i64> = runs.iter().filter(|r| r.hook == "a").map(|r| r.started_at).collect();
    assert_eq!(a.len(), KEEP_RUNS);
    assert_eq!(a[0], KEEP_RUNS as i64 + 4, "newest first");
    assert_eq!(runs.iter().filter(|r| r.hook == "b").count(), 1);
}

#[test]
fn last_lines_keeps_the_tail() {
    let text: String = (1..=30).map(|n| format!("line {n}\n")).collect();
    let kept = last_lines(&text, KEEP_LINES);
    assert!(kept.starts_with("line 11\n") && kept.ends_with("line 30"), "{kept}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_command_gets_the_variables_and_its_exit() {
    let env = vec![("NEXUS_HOOK_EVENT".to_string(), "settled".to_string())];
    let (code, timed_out, output) = run_command(
        "echo \"$NEXUS_HOOK_EVENT\"; echo oops >&2; exit 3",
        &std::env::temp_dir(),
        &env,
        Duration::from_secs(10),
        &Shell::sh(),
    )
    .await;
    assert_eq!((code, timed_out), (Some(3), false));
    assert!(output.contains("settled") && output.contains("oops"), "{output}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_command_past_its_timeout_is_killed_with_its_group() {
    let started = Instant::now();
    let (code, timed_out, _) = run_command(
        "sleep 30 & sleep 30",
        &std::env::temp_dir(),
        &[],
        Duration::from_secs(1),
        &Shell::sh(),
    )
    .await;
    assert_eq!((code, timed_out), (None, true));
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
}

struct NoHost;

impl Host for NoHost {
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, crate::Result<Launch>> {
        Box::pin(async { Err("no agents here".into()) })
    }
    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
    fn price(&self, _: &str, _: &Usage) -> Option<f64> {
        None
    }
    fn emit(&self, _: Emit) {}
}

struct Fixed {
    hooks: Hooks,
    failed: Mutex<Vec<String>>,
}

impl Source for Fixed {
    fn hooks(&self) -> Hooks {
        self.hooks.clone()
    }
    fn failed(&self, _: &str, _: &str, body: &str) {
        self.failed.lock().expect("failed").push(body.into());
    }
}

fn record(summary: ThreadSummary) -> ThreadRecord {
    ThreadRecord {
        summary,
        native_id: "native".into(),
        native_started: false,
        scope: "scope".into(),
        fork_from: None,
        seed: None,
        automatic_title: false,
        delivered_seq: 0,
        halt: None,
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_workbench_runs_its_hook_once_per_event_and_keeps_the_runs() {
    let dir = std::env::temp_dir().join(format!("nexus-hooks-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let workbench = Workbench::new(
        Store::in_memory().expect("store"),
        Arc::new(NoHost),
        Config {
            scope: "scope".into(),
            shell: Shell::sh(),
            ..Config::default()
        },
    )
    .expect("workbench");
    let project = workbench.store().add_project(&dir, 1).expect("project");
    workbench
        .store()
        .insert_thread(&record(summary("t1", &project.id)))
        .expect("thread");
    // No source yet: nothing runs.
    workbench
        .run_hooks(
            Arc::new(Fixed {
                hooks: Hooks::default(),
                failed: Mutex::default(),
            }),
            Fire::plain(HookEvent::Archived, "t1", "0".into()),
        )
        .await;
    let source = Arc::new(Fixed {
        hooks: Hooks {
            enabled: true,
            list: vec![
                hook(HookEvent::Archived, "echo \"$NEXUS_SESSION_ID $PWD\" >> ran.txt"),
                hook(HookEvent::Archived, "exit 2"),
            ],
        },
        failed: Mutex::default(),
    });
    let fire = Fire::plain(HookEvent::Archived, "t1", "1".into());
    workbench.run_hooks(source.clone(), fire.clone()).await;
    workbench.run_hooks(source.clone(), fire).await;
    let ran = std::fs::read_to_string(dir.join("ran.txt")).expect("ran");
    assert_eq!(ran.lines().count(), 1, "{ran}");
    assert!(ran.starts_with("t1 "), "{ran}");
    let runs = workbench.hook_runs().expect("runs");
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().any(|run| run.exit_code == Some(2) && run.failed()));
    let failed = source.failed.lock().expect("failed").clone();
    assert_eq!(failed.len(), 1);
    assert!(failed[0].contains("exit 2"), "{failed:?}");

    // The policy off: nothing more runs.
    let off = Arc::new(Fixed {
        hooks: Hooks {
            enabled: false,
            ..source.hooks()
        },
        failed: Mutex::default(),
    });
    workbench
        .run_hooks(off, Fire::plain(HookEvent::Archived, "t1", "2".into()))
        .await;
    assert_eq!(workbench.hook_runs().expect("runs").len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
