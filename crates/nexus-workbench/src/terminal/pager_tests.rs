//! an agent's command runs without a pager, and a read says how long a running command has been silent.
//! it also runs below the app's priority. These start real processes in `/bin/sh`.
use crate::event::{ThreadSummary, Usage};
use crate::process::Shell;
use crate::store::{Store, ThreadRecord};
use crate::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd, Workbench};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::Arc;

use super::process;

const THREAD: &str = "pager";
/// Prints the three pager variables, `-` for an unset one.
const PRINT_PAGERS: &str = r#"printf '%s|%s|%s' "${PAGER:--}" "${GIT_PAGER:--}" "${GH_PAGER:--}""#;

struct NoHost;
impl Host for NoHost {
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, crate::Result<Launch>> {
        Box::pin(async { panic!("pager fixture never launches an agent") })
    }
    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
    fn price(&self, _: &str, _: &Usage) -> Option<f64> {
        None
    }
    fn emit(&self, _: Emit) {}
    fn marks_commits(&self) -> BoxFuture<'static, bool> {
        Box::pin(async { false })
    }
}

/// A workbench whose one session has `mode` as its permission mode.
fn workbench(mode: &str) -> Workbench {
    workbench_with_threads(mode, &[THREAD])
}

fn workbench_with_threads(mode: &str, threads: &[&str]) -> Workbench {
    let store = Store::in_memory().expect("fixture store");
    let project = store.add_project(&std::env::temp_dir(), 100).expect("fixture project");
    for thread in threads {
        let summary: ThreadSummary = serde_json::from_value(json!({
            "id":thread, "projectId":project.id, "title":"Pager fixture", "agent":"claude",
            "model":null, "permissionMode":mode, "via":"nexus", "status":"idle", "costUsd":0.,
            "nexusSessions":[], "createdAt":100, "updatedAt":100, "archived":false
        }))
        .expect("fixture thread summary");
        store
            .insert_thread(&ThreadRecord {
                summary,
                native_id: "native".into(),
                native_started: false,
                scope: "scope".into(),
                fork_from: None,
                seed: None,
                automatic_title: false,
                delivered_seq: 0,
                halt: None,
            })
            .expect("fixture thread");
    }
    Workbench::new(
        store,
        Arc::new(NoHost),
        Config {
            terminal: true,
            scope: "scope".into(),
            shell: Shell::sh(),
            ..Config::default()
        },
    )
    .expect("fixture workbench")
}

/// Reads `id` until it ends, and returns the last read with all of its output.
async fn finish(wb: &Workbench, id: &Value) -> (Value, String) {
    finish_in_thread(wb, THREAD, id).await
}

async fn finish_in_thread(wb: &Workbench, thread: &str, id: &Value) -> (Value, String) {
    let mut output = String::new();
    let mut after = 0;
    for _ in 0..30 {
        let read = wb
            .terminal(thread, "read", json!({"id":id, "after":after, "wait_ms":1000}), false)
            .await
            .expect("read the command");
        output.push_str(read["output"].as_str().unwrap_or_default());
        after = read["next"].as_u64().expect("next cursor");
        if !matches!(read["terminal"]["status"].as_str(), Some("starting" | "running")) {
            return (read, output);
        }
    }
    panic!("the command did not end in 30 s");
}

#[test]
fn an_agents_environment_has_no_pager_and_the_persons_keeps_theirs() {
    let app = || [("PAGER", "less"), ("GIT_PAGER", "less -R")].map(|(k, v)| (k.to_string(), v.to_string()));
    let agent = process::environment(app(), false, true);
    for (key, value) in process::NO_PAGER {
        assert_eq!(agent[key], value, "{key}");
    }
    let person = process::environment(app(), false, false);
    assert_eq!(person["PAGER"], "less");
    assert_eq!(person["GIT_PAGER"], "less -R");
    assert!(!person.contains_key("GH_PAGER"));
}

#[tokio::test]
async fn an_agents_command_runs_without_a_pager_and_its_own_setting_wins() {
    let wb = workbench("auto");
    let run = wb
        .terminal(
            THREAD,
            "run",
            json!({"command":PRINT_PAGERS, "request_id":"one"}),
            false,
        )
        .await
        .expect("run the agent's command");
    assert_eq!(run["agentWritten"], true);
    let (_, output) = finish(&wb, &run["id"]).await;
    assert_eq!(output, "cat|cat|cat");

    // `export`: a prefix assignment (`GIT_PAGER=less printf ..`) reaches the program, not the shell's own expansion.
    let own = format!("export GIT_PAGER=less; {PRINT_PAGERS}");
    let run = wb
        .terminal(THREAD, "run", json!({"command":own, "request_id":"two"}), false)
        .await
        .expect("run the agent's command with its own pager");
    let (_, output) = finish(&wb, &run["id"]).await;
    assert_eq!(output, "cat|less|cat");
}

#[tokio::test]
async fn an_approved_command_stays_the_agents_and_an_edited_one_becomes_the_persons() {
    let wb = workbench("default");
    let staged = wb
        .terminal(
            THREAD,
            "run",
            json!({"command":PRINT_PAGERS, "request_id":"one"}),
            false,
        )
        .await
        .expect("stage the agent's command");
    assert_eq!(staged["status"], "awaitingUser");
    wb.terminal(THREAD, "execute", json!({"id":staged["id"]}), true)
        .await
        .expect("the person approves it");
    let (_, output) = finish(&wb, &staged["id"]).await;
    assert_eq!(output, "cat|cat|cat");

    let staged = wb
        .terminal(THREAD, "run", json!({"command":"true", "request_id":"two"}), false)
        .await
        .expect("stage the agent's command");
    wb.terminal(
        THREAD,
        "execute",
        json!({"id":staged["id"], "command":"true; true"}),
        true,
    )
    .await
    .expect("the person edits and runs it");
    let (read, _) = finish(&wb, &staged["id"]).await;
    assert_eq!(read["terminal"]["agentWritten"], false);
}

/// a command the agent wrote runs below the app, as the agent does. One the person edited keeps their
/// priority.
#[tokio::test]
async fn an_agents_command_runs_below_the_app_and_the_persons_does_not() {
    let wb = workbench("default");
    let staged = wb
        .terminal(THREAD, "run", json!({"command":"nice", "request_id":"one"}), false)
        .await
        .expect("stage the agent's command");
    wb.terminal(THREAD, "execute", json!({"id":staged["id"]}), true)
        .await
        .expect("the person approves it");
    let (_, output) = finish(&wb, &staged["id"]).await;
    let lowered = crate::process::tests::niceness_plus(crate::process::AGENT_NICENESS.parse().expect("a number"));
    assert_eq!(output.trim(), lowered);

    let staged = wb
        .terminal(THREAD, "run", json!({"command":"true", "request_id":"two"}), false)
        .await
        .expect("stage the agent's command");
    wb.terminal(THREAD, "execute", json!({"id":staged["id"], "command":"nice"}), true)
        .await
        .expect("the person edits and runs it");
    let (_, output) = finish(&wb, &staged["id"]).await;
    assert_eq!(output.trim(), crate::process::tests::niceness_plus(0));
}

#[tokio::test]
async fn a_read_says_how_long_a_running_command_has_been_silent() {
    let wb = workbench("auto");
    let run = wb
        .terminal(THREAD, "run", json!({"command":"sleep 1", "request_id":"one"}), false)
        .await
        .expect("run a silent command");
    let read = |after| wb.terminal(THREAD, "read", json!({"id":run["id"], "after":after}), false);
    let first = read(0).await.expect("first read")["idleMs"]
        .as_u64()
        .expect("idleMs while running");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let second = read(0).await.expect("second read");
    if second["terminal"]["status"] == "running" {
        assert!(second["idleMs"].as_u64().expect("idleMs while running") > first);
    }
    let (ended, _) = finish(&wb, &run["id"]).await;
    assert_eq!(ended["terminal"]["status"], "completed");
    assert!(ended["idleMs"].is_null());
}

#[tokio::test]
async fn terminal_commands_expand_their_own_session_id() {
    let wb = workbench_with_threads("auto", &["t_first", "t_second"]);
    for (thread, human, request_id) in [
        ("t_first", false, "agent-first"),
        ("t_second", false, "agent-second"),
        ("t_first", true, "person-first"),
    ] {
        let run = wb
            .terminal(
                thread,
                "run",
                json!({"command":r#"printf '%s' "$NEXUS_SESSION_ID""#, "request_id":request_id}),
                human,
            )
            .await
            .expect("run the command");
        assert_eq!(run["agentWritten"], !human);
        let (ended, output) = finish_in_thread(&wb, thread, &run["id"]).await;
        assert_eq!(ended["terminal"]["exitCode"], 0);
        assert_eq!(output, thread);
    }
}
