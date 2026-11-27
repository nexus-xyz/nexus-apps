//! Actual PTYs and the public terminal API, driven with isolated fake agent runtimes.
mod support;
use nexus_workbench::Config;
use nexus_workbench::event::PermissionMode;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use support::*;

async fn fixture(name: &str, mode: PermissionMode) -> (Setup, String) {
    fixture_in(name, mode, |config| config).await
}
async fn fixture_in(name: &str, mode: PermissionMode, shell: impl FnOnce(Config) -> Config) -> (Setup, String) {
    let setup = setup_with(name, bounded(), false, |config| {
        shell(Config {
            terminal: true,
            ..config
        })
    });
    let mut new = new_root(&setup.project, "hello");
    new.permission_mode = mode;
    new.orchestration = false;
    let thread = root(&setup, new).await;
    (setup, thread.id)
}
async fn finished(s: &Setup, thread: &str, id: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let result = s
            .workbench
            .terminal(thread, "read", json!({"id":id,"wait_ms":1000}), false)
            .await
            .expect("terminal read");
        if result["terminal"]["exitCode"].is_u64() {
            return result;
        }
        assert!(Instant::now() < deadline, "command did not finish: {result}");
    }
}
async fn cleanup(s: Setup) {
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

#[cfg(unix)]
#[tokio::test]
async fn the_agent_reads_output_before_exit_and_sends_input_to_the_same_process() {
    let (s, thread) = fixture("terminal-interactive", PermissionMode::BypassPermissions).await;
    let request = json!({"command":"printf 'ready\\n'; read answer; printf 'received:%s\\n' \"$answer\"; exit 7","request_id":"interactive"});
    let started = s
        .workbench
        .terminal(&thread, "run", request.clone(), false)
        .await
        .expect("terminal run");
    let initial = s
        .workbench
        .terminal(&thread, "read", json!({"id":started["id"],"wait_ms":1000}), false)
        .await
        .expect("terminal read");
    assert!(initial["output"].as_str().expect("terminal output").contains("ready"));
    assert!(initial["terminal"]["exitCode"].is_null());
    assert_eq!(initial["terminal"]["status"], "running");
    s.workbench
        .terminal(
            &thread,
            "input",
            json!({"id":started["id"],"data":"hello\n","lease":initial["terminal"]["lease"]}),
            false,
        )
        .await
        .expect("terminal input");
    let done = finished(&s, &thread, &started["id"]).await;
    assert_eq!(done["terminal"]["exitCode"], 7);
    assert!(
        done["output"]
            .as_str()
            .expect("terminal output")
            .contains("received:hello")
    );
    let retry = s
        .workbench
        .terminal(&thread, "run", request, false)
        .await
        .expect("terminal run");
    assert_eq!(retry["id"], started["id"]);
    assert_eq!(retry["exitCode"], 7);
    cleanup(s).await;
}

#[cfg(unix)]
#[tokio::test]
async fn normal_modes_stage_and_the_person_can_edit_before_running() {
    let (s, thread) = fixture("terminal-handoff", PermissionMode::Default).await;
    let staged = s
        .workbench
        .terminal(
            &thread,
            "run",
            json!({"command":"printf original","request_id":"human"}),
            false,
        )
        .await
        .expect("terminal run");
    assert_eq!(staged["status"], "awaitingUser");
    let before = s
        .workbench
        .terminal(&thread, "read", json!({"id":staged["id"]}), false)
        .await
        .expect("terminal read");
    assert_eq!(before["output"], "");
    assert!(before["terminal"]["exitCode"].is_null());
    assert!(
        s.workbench
            .terminal(&thread, "execute", json!({"id":staged["id"]}), false)
            .await
            .is_err()
    );
    s.workbench
        .terminal(
            &thread,
            "execute",
            json!({"id":staged["id"],"command":"printf edited; exit 9"}),
            true,
        )
        .await
        .expect("terminal execute");
    let result = finished(&s, &thread, &staged["id"]).await;
    assert_eq!(result["terminal"]["command"], "printf edited; exit 9");
    assert_eq!(result["terminal"]["exitCode"], 9);
    assert!(result["output"].as_str().expect("terminal output").contains("edited"));
    cleanup(s).await;
}

#[cfg(unix)]
#[tokio::test]
async fn private_output_stays_private_when_the_person_shares_a_reviewed_result() {
    let (s, thread) = fixture("terminal-private", PermissionMode::BypassPermissions).await;
    let staged=s.workbench.terminal(&thread,"request_user",json!({"command":"printf 'private-ready\\n'; read answer; printf 'secret:%s' \"$answer\"","request_id":"private","reason":"Sign in yourself"}),false).await.expect("terminal request_user");
    s.workbench
        .terminal(&thread, "private", json!({"id":staged["id"]}), true)
        .await
        .expect("terminal private");
    s.workbench
        .terminal(&thread, "execute", json!({"id":staged["id"]}), true)
        .await
        .expect("terminal execute");
    let ready = s
        .workbench
        .terminal(&thread, "read", json!({"id":staged["id"],"wait_ms":1000}), true)
        .await
        .expect("terminal read");
    assert!(
        ready["output"]
            .as_str()
            .expect("terminal output")
            .contains("private-ready")
    );
    assert!(
        s.workbench
            .terminal(&thread, "read", json!({"id":staged["id"]}), false)
            .await
            .is_err()
    );
    s.workbench
        .terminal(
            &thread,
            "input",
            json!({"id":staged["id"],"data":"hidden-password\n"}),
            true,
        )
        .await
        .expect("terminal input");
    s.workbench
        .terminal(
            &thread,
            "share",
            json!({"id":staged["id"],"result":"Authentication completed"}),
            true,
        )
        .await
        .expect("terminal share");
    let result = finished(&s, &thread, &staged["id"]).await;
    assert_eq!(result["output"], "Authentication completed");
    assert_eq!(result["outputKind"], "reviewed_summary");
    assert_eq!(result["terminal"]["private"], true);
    assert_eq!(result["terminal"]["command"], "[Private command]");
    assert!(
        s.workbench
            .terminal(&thread, "return", json!({"id":staged["id"]}), true)
            .await
            .is_err()
    );
    cleanup(s).await;
}

#[tokio::test]
async fn plan_mode_cannot_start_commands_and_account_changes_remove_buffers() {
    let (s, thread) = fixture("terminal-scope", PermissionMode::Plan).await;
    assert!(
        s.workbench
            .terminal(&thread, "run", json!({"command":"echo no","request_id":"no"}), false)
            .await
            .expect_err("plan mode refuses commands")
            .starts_with("terminal_plan_mode")
    );
    let staged = s
        .workbench
        .terminal(
            &thread,
            "request_user",
            json!({"command":"echo later","request_id":"later"}),
            true,
        )
        .await
        .expect("terminal request_user");
    s.workbench.set_scope("another-org");
    assert!(
        s.workbench
            .terminal(&thread, "read", json!({"id":staged["id"]}), true)
            .await
            .is_err()
    );
    s.workbench.set_scope("gw|org");
    let restored = s
        .workbench
        .terminal(&thread, "read", json!({"id":staged["id"]}), true)
        .await
        .expect("history remains in its original scope");
    assert_eq!(restored["output"], "");
    assert_eq!(restored["terminal"]["restored"], true);
    assert_eq!(restored["terminal"]["status"], "cancelled");
    assert!(restored["terminal"]["exitCode"].is_null());
    cleanup(s).await;
}

#[cfg(unix)]
#[tokio::test]
async fn named_shells_keep_independent_state_and_handoffs_revoke_agent_input() {
    let (s, thread) = fixture("terminal-shells", PermissionMode::BypassPermissions).await;
    let first = s
        .workbench
        .terminal(&thread, "open", json!({"request_id":"first","name":"Build"}), false)
        .await
        .expect("terminal open");
    let second = s
        .workbench
        .terminal(&thread, "open", json!({"request_id":"second","name":"Web"}), false)
        .await
        .expect("terminal open");
    assert_ne!(first["id"], second["id"]);
    s.workbench
        .terminal(
            &thread,
            "input",
            json!({"id":first["id"],"lease":first["lease"],"data":"export NEXUS_TERMINAL_TEST=one\n"}),
            false,
        )
        .await
        .expect("terminal input");
    let taken = s
        .workbench
        .terminal(&thread, "take", json!({"id":first["id"]}), true)
        .await
        .expect("terminal take");
    assert_ne!(taken["lease"], first["lease"]);
    assert!(
        s.workbench
            .terminal(
                &thread,
                "input",
                json!({"id":first["id"],"lease":first["lease"],"data":"echo stale\n"}),
                false
            )
            .await
            .is_err()
    );
    let returned = s
        .workbench
        .terminal(&thread, "return", json!({"id":first["id"]}), true)
        .await
        .expect("terminal return");
    s.workbench
        .terminal(
            &thread,
            "input",
            json!({"id":first["id"],"lease":returned["lease"],"data":"printf 'state:%s\\n' \"$NEXUS_TERMINAL_TEST\"; exit 4\n"}),
            false,
        )
        .await
        .expect("terminal input");
    s.workbench
        .terminal(
            &thread,
            "input",
            json!({"id":second["id"],"lease":second["lease"],"data":"printf 'state:%s\\n' \"${NEXUS_TERMINAL_TEST:-empty}\"; exit 0\n"}),
            false,
        )
        .await
        .expect("terminal input");
    let first_done = finished(&s, &thread, &first["id"]).await;
    let second_done = finished(&s, &thread, &second["id"]).await;
    assert!(
        first_done["output"]
            .as_str()
            .expect("terminal output")
            .contains("state:one")
    );
    assert!(
        second_done["output"]
            .as_str()
            .expect("terminal output")
            .contains("state:empty")
    );
    assert_eq!(first_done["terminal"]["exitCode"], 4);
    s.workbench
        .terminal(&thread, "rename", json!({"id":second["id"],"name":"Server"}), true)
        .await
        .expect("terminal rename");
    let listed = s
        .workbench
        .terminal(&thread, "list", json!({}), false)
        .await
        .expect("terminal list");
    assert_eq!(listed.as_array().expect("terminal list").len(), 2);
    assert_eq!(listed[1]["name"], "Server");
    cleanup(s).await;
}

#[tokio::test]
async fn dozens_of_terminals_are_listed_and_closed_without_starting_processes() {
    let (s, thread) = fixture("terminal-dozens", PermissionMode::Default).await;
    for index in 0..32 {
        s.workbench
            .terminal(
                &thread,
                "open",
                json!({"request_id":format!("shell-{index}"),"name":format!("Shell {index}")}),
                false,
            )
            .await
            .expect("terminal open");
    }
    let listed = s
        .workbench
        .terminal(&thread, "list", json!({}), true)
        .await
        .expect("terminal list");
    assert_eq!(listed.as_array().expect("terminal list").len(), 32);
    let id = &listed[0]["id"];
    s.workbench
        .terminal(&thread, "close", json!({"id":id}), true)
        .await
        .expect("terminal close");
    assert_eq!(
        s.workbench
            .terminal(&thread, "list", json!({}), false)
            .await
            .expect("terminal list")
            .as_array()
            .expect("terminal list")
            .len(),
        31
    );
    let retry = s
        .workbench
        .terminal(&thread, "open", json!({"request_id":"shell-0"}), false)
        .await
        .expect("terminal open");
    assert_eq!(&retry["id"], id);
    assert_eq!(retry["closed"], true);
    cleanup(s).await;
}

#[cfg(unix)]
#[tokio::test]
async fn changing_sign_in_revokes_a_read_already_waiting_for_output() {
    let (s, thread) = fixture("terminal-identity", PermissionMode::BypassPermissions).await;
    let command = s
        .workbench
        .terminal(
            &thread,
            "run",
            json!({"command":"sleep 5","request_id":"waiting"}),
            false,
        )
        .await
        .expect("terminal run");
    let workbench = s.workbench.clone();
    let caller = thread.clone();
    let id = command["id"].clone();
    let waiting = tokio::spawn(async move {
        workbench
            .terminal(&caller, "read", json!({"id":id,"wait_ms":1000}), false)
            .await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    *s.host.choice_identity.lock().expect("test identity lock") = Some("another-person".into());
    s.workbench.set_scope("gw|org");
    assert!(waiting.await.expect("waiting read task").is_err());
    assert!(
        s.workbench
            .terminal(&thread, "list", json!({}), true)
            .await
            .expect("terminal list")
            .as_array()
            .expect("terminal list")
            .is_empty()
    );
    cleanup(s).await;
}

#[cfg(unix)]
#[tokio::test]
async fn closing_an_interactive_shell_also_ends_its_background_job() {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    // Ending the jobs on a hang-up is bash's and zsh's behaviour, the login shells this guards; a POSIX `sh` such as
    // dash (CI's `/bin/sh`) leaves them running. So this runs bash, through a wrapper that skips its startup files:
    // `Shell::Plain` takes no arguments, and a test never reads the person's own shell setup.
    if !std::path::Path::new("/bin/bash").exists() {
        return;
    }
    let bash = std::env::temp_dir().join(format!("nexus-workbench-norc-bash-{}", std::process::id()));
    std::fs::write(&bash, "#!/bin/sh\nexec /bin/bash --norc --noprofile \"$@\"\n").expect("bash wrapper");
    std::fs::set_permissions(&bash, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("bash wrapper mode");
    let (s, thread) = fixture_in("terminal-close-job", PermissionMode::BypassPermissions, |config| {
        Config {
            shell: nexus_workbench::process::Shell::Plain(bash.clone()),
            ..config
        }
    })
    .await;
    let opened = s
        .workbench
        .terminal(&thread, "open", json!({"request_id":"shell"}), false)
        .await
        .expect("terminal open");
    s.workbench
        .terminal(
            &thread,
            "input",
            json!({"id":opened["id"],"lease":opened["lease"],"data":concat!(r#"sleep 30 & printf '\njob:'; jobs -p; wait"#, "\n")}),
            false,
        )
        .await
        .expect("terminal input");
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid = loop {
        let read = s
            .workbench
            .terminal(&thread, "read", json!({"id":opened["id"],"wait_ms":1000}), false)
            .await
            .expect("terminal read");
        if let Some(pid) = read["output"]
            .as_str()
            .expect("terminal output")
            .split("job:")
            .skip(1)
            .find_map(|tail| {
                tail.split_whitespace()
                    .find_map(|part| part.parse::<i32>().ok().filter(|pid| *pid > 1))
            })
        {
            break Pid::from_raw(pid);
        }
        assert!(Instant::now() < deadline, "shell did not start its job: {read}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    s.workbench
        .terminal(&thread, "close", json!({"id":opened["id"]}), true)
        .await
        .expect("terminal close");
    let deadline = Instant::now() + Duration::from_secs(2);
    while kill(pid, None).is_ok() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let survived = kill(pid, None).is_ok();
    if survived {
        let _ = kill(pid, Signal::SIGKILL);
    }
    cleanup(s).await;
    let _ = std::fs::remove_file(&bash);
    assert!(!survived, "closing the shell left its background job running");
}

#[tokio::test]
async fn deleting_declined_history_and_undo_never_authorizes_or_recreates_the_command() {
    let (s, thread) = fixture("declined-history", PermissionMode::Default).await;
    let request = json!({"command":"printf must-not-run","request_id":"declined"});
    let staged = s
        .workbench
        .terminal(&thread, "run", request.clone(), false)
        .await
        .expect("terminal test operation");
    let id = staged["id"].clone();
    s.workbench
        .terminal(&thread, "decline", json!({"id":id}), true)
        .await
        .expect("terminal test operation");
    s.workbench
        .terminal(&thread, "delete", json!({"id":id}), true)
        .await
        .expect("terminal test operation");
    let list = s
        .workbench
        .terminal(&thread, "list", json!({}), true)
        .await
        .expect("terminal test operation");
    assert!(list.as_array().expect("terminal test operation").is_empty());
    let retry = s
        .workbench
        .terminal(&thread, "run", request, false)
        .await
        .expect("terminal test operation");
    assert_eq!(retry["id"], id);
    assert_eq!(retry["status"], "declined");
    assert_eq!(retry["deleted"], true);
    assert!(
        s.workbench
            .terminal(
                &thread,
                "native_prepare",
                json!({"id":id,"command":staged["command"]}),
                true
            )
            .await
            .is_err()
    );
    s.workbench
        .terminal(&thread, "restore", json!({"id":id}), true)
        .await
        .expect("terminal test operation");
    let list = s
        .workbench
        .terminal(&thread, "list", json!({}), true)
        .await
        .expect("terminal test operation");
    assert_eq!(list[0]["status"], "declined");
    let read = s
        .workbench
        .terminal(&thread, "read", json!({"id":id}), false)
        .await
        .expect("terminal test operation");
    assert_eq!(read["output"], "");
    assert!(read["terminal"]["exitCode"].is_null());
    cleanup(s).await;
}

#[cfg(unix)]
#[tokio::test]
async fn run_executes_the_displayed_command_once_and_the_inline_shell_only_needs_the_answer() {
    let (s, thread) = fixture("inline-run", PermissionMode::Default).await;
    let command = "printf 'pid:%s\\nEnter a word: ' \"$$\"; read -r word; printf 'answer:%s\\n' \"$word\"";
    let staged = s
        .workbench
        .terminal(&thread, "run", json!({"command":command,"request_id":"inline"}), false)
        .await
        .expect("stage");
    let id = staged["id"].clone();
    assert!(
        s.workbench
            .terminal(
                &thread,
                "execute",
                json!({"id":id,"command":command,"expected_command":"stale"}),
                true
            )
            .await
            .is_err()
    );
    let started = s
        .workbench
        .terminal(
            &thread,
            "execute",
            json!({"id":id,"command":command,"expected_command":command}),
            true,
        )
        .await
        .expect("Run starts the displayed command");
    assert_eq!(started["owner"], "user");
    assert_eq!(started["status"], "running");
    let ready = s
        .workbench
        .terminal(&thread, "read", json!({"id":id,"wait_ms":1000}), true)
        .await
        .expect("read prompt");
    assert!(ready["output"].as_str().expect("output").contains("Enter a word:"));
    s.workbench
        .terminal(&thread, "input", json!({"id":id,"data":"Néxus\n"}), true)
        .await
        .expect("type only the answer");
    let done = finished(&s, &thread, &id).await;
    let output = done["output"].as_str().expect("output");
    assert!(output.contains("answer:Néxus"), "{output}");
    assert_eq!(output.matches("pid:").count(), 1);
    assert_eq!(done["terminal"]["exitCode"], 0);
    assert!(
        s.workbench
            .terminal(&thread, "execute", json!({"id":id,"command":command}), true)
            .await
            .is_err()
    );
    cleanup(s).await;
}

#[cfg(unix)]
#[tokio::test]
async fn durable_receipts_return_the_same_result_after_restart_without_replaying() {
    use nexus_workbench::{Workbench, store::Store};
    let (s, thread) = fixture("terminal-durable", PermissionMode::BypassPermissions).await;
    let command = json!({"command":"printf x >> executions; exit 7", "request_id":"durable"});
    let started = s
        .workbench
        .terminal(&thread, "run", command.clone(), false)
        .await
        .expect("start once");
    finished(&s, &thread, &started["id"]).await;
    s.workbench.shutdown().await;
    let reopened = Workbench::new(
        Store::open(&s.root.join("workbench.sqlite3")).expect("reopen store"),
        s.host.clone(),
        Config {
            scope: "gw|org".into(),
            terminal: true,
            shell: nexus_workbench::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("reopen workbench");
    let receipt = reopened
        .terminal(&thread, "run", command, false)
        .await
        .expect("existing receipt");
    assert_eq!(receipt["id"], started["id"]);
    assert_eq!(receipt["exitCode"], 7);
    assert_eq!(receipt["restored"], true);
    assert_eq!(
        std::fs::read_to_string(s.root.join("repo/executions")).expect("side effect"),
        "x"
    );
    let history = reopened
        .terminal(&thread, "list", json!({}), true)
        .await
        .expect("history");
    assert_eq!(history.as_array().expect("entries").len(), 1);
    reopened.shutdown().await;
    cleanup(s).await;
}

#[tokio::test]
async fn plan_denials_are_recorded_and_cannot_be_retried_as_approved_commands() {
    let (s, thread) = fixture("terminal-denied-history", PermissionMode::Plan).await;
    let request = json!({"command":"echo should-not-run", "request_id":"denied"});
    assert!(
        s.workbench
            .terminal(&thread, "run", request, false)
            .await
            .expect_err("plan denial")
            .starts_with("terminal_plan_mode")
    );
    let history = s
        .workbench
        .terminal(&thread, "list", json!({}), true)
        .await
        .expect("history");
    assert_eq!(history[0]["status"], "denied");
    assert!(history[0]["exitCode"].is_null());
    cleanup(s).await;
}

#[tokio::test]
async fn restart_invalidates_pending_approval_and_keeps_private_details_out_of_the_journal() {
    use nexus_workbench::{Workbench, store::Store};
    let (s, thread) = fixture("terminal-private-journal", PermissionMode::Default).await;
    let staged = s
        .workbench
        .terminal(
            &thread,
            "request_user",
            json!({"command":"private-value-in-command", "request_id":"private-journal", "private":true}),
            true,
        )
        .await
        .expect("stage private");
    s.workbench.shutdown().await;
    let db = rusqlite::Connection::open(s.root.join("workbench.sqlite3")).expect("journal");
    let bodies: String = db
        .query_row("select group_concat(body) from terminal_command_events", [], |row| {
            row.get(0)
        })
        .expect("events");
    assert!(!bodies.contains("private-value-in-command"));
    let reopened = Workbench::new(
        Store::open(&s.root.join("workbench.sqlite3")).expect("reopen store"),
        s.host.clone(),
        Config {
            scope: "gw|org".into(),
            terminal: true,
            ..Config::default()
        },
    )
    .expect("reopen workbench");
    let history = reopened
        .terminal(&thread, "list", json!({}), true)
        .await
        .expect("history");
    assert_eq!(history[0]["id"], staged["id"]);
    assert_eq!(history[0]["status"], "cancelled");
    assert_eq!(history[0]["command"], "[Private command]");
    reopened.shutdown().await;
    cleanup(s).await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_lost_launch_is_unknown_and_never_replayed() {
    use nexus_workbench::{Workbench, store::Store};
    let (s, thread) = fixture("terminal-uncertain", PermissionMode::BypassPermissions).await;
    let request = json!({"command":"printf x >> executions", "request_id":"uncertain"});
    let first = s
        .workbench
        .terminal(&thread, "run", request.clone(), false)
        .await
        .expect("start");
    finished(&s, &thread, &first["id"]).await;
    s.workbench.shutdown().await;
    // Model a crash between the launch claim and the recorded result, without a real orphan process.
    let db = rusqlite::Connection::open(s.root.join("workbench.sqlite3")).expect("database");
    db.execute(
        "update terminal_commands set body = json_set(body, '$.status', 'starting', '$.exitCode', null)",
        [],
    )
    .expect("lost result");
    let reopened = Workbench::new(
        Store::open(&s.root.join("workbench.sqlite3")).expect("store"),
        s.host.clone(),
        Config {
            scope: "gw|org".into(),
            terminal: true,
            ..Config::default()
        },
    )
    .expect("workbench");
    let receipt = reopened
        .terminal(&thread, "run", request, false)
        .await
        .expect("receipt");
    assert_eq!(receipt["id"], first["id"]);
    assert_eq!(receipt["status"], "unknown");
    assert!(receipt["exitCode"].is_null());
    assert_eq!(
        std::fs::read_to_string(s.root.join("repo/executions")).expect("side effect"),
        "x"
    );
    reopened
        .delete_thread(&thread)
        .await
        .expect("delete session and history");
    let count: i64 = db
        .query_row("select count(*) from terminal_commands", [], |row| row.get(0))
        .expect("receipt count");
    assert_eq!(count, 0);
    let count: i64 = db
        .query_row("select count(*) from terminal_command_events", [], |row| row.get(0))
        .expect("event count");
    assert_eq!(count, 0);
    reopened.shutdown().await;
    cleanup(s).await;
}

#[cfg(unix)]
#[tokio::test]
async fn concurrent_retries_share_one_launch_and_receipt() {
    let (s, thread) = fixture("terminal-concurrent", PermissionMode::BypassPermissions).await;
    let request = json!({"command":"printf x >> executions", "request_id":"concurrent"});
    let (a, b) = tokio::join!(
        s.workbench.terminal(&thread, "run", request.clone(), false),
        s.workbench.terminal(&thread, "run", request, false)
    );
    let (a, b) = (a.expect("first receipt"), b.expect("retry receipt"));
    assert_eq!(a["id"], b["id"]);
    finished(&s, &thread, &a["id"]).await;
    assert_eq!(
        std::fs::read_to_string(s.root.join("repo/executions")).expect("side effect"),
        "x"
    );
    cleanup(s).await;
}

#[cfg(unix)]
#[tokio::test]
async fn switching_to_plan_never_rewrites_a_running_or_completed_receipt() {
    let (s, id) = fixture("terminal-retry-mode", PermissionMode::BypassPermissions).await;
    let request = json!({"command":"read answer; exit 7", "request_id":"mode-retry"});
    let running = s
        .workbench
        .terminal(&id, "run", request.clone(), false)
        .await
        .expect("run");
    s.workbench
        .update(&id, mode(PermissionMode::Plan))
        .expect("switch mode");
    let deadline = Instant::now() + Duration::from_secs(3);
    while thread(&s.workbench, &id).permission_mode != PermissionMode::Plan {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let retry = s
        .workbench
        .terminal(&id, "run", request.clone(), false)
        .await
        .expect("running receipt");
    assert_eq!(retry["status"], "running");
    assert_eq!(retry["permissionMode"], "bypassPermissions");
    assert_eq!(retry["decisionSource"], "fullAccess");
    assert_eq!(retry["owner"], "user");
    assert_ne!(retry["lease"], running["lease"]);
    s.workbench
        .terminal(&id, "input", json!({"id":running["id"],"data":"done\n"}), true)
        .await
        .expect("person finishes command");
    let done = finished(&s, &id, &running["id"]).await;
    let retry = s
        .workbench
        .terminal(&id, "run", request, false)
        .await
        .expect("completed receipt");
    assert_eq!(retry, done["terminal"]);
    assert_eq!(retry["status"], "failed");
    assert_eq!(retry["exitCode"], 7);
    cleanup(s).await;
}

#[tokio::test]
async fn private_fingerprints_omit_command_text_without_widening_the_handoff() {
    let (s, thread) = fixture("private-fingerprint", PermissionMode::BypassPermissions).await;
    let first = s
        .workbench
        .terminal(
            &thread,
            "request_user",
            json!({"command":"private-one", "request_id":"private-one", "private":true}),
            true,
        )
        .await
        .expect("first private request");
    s.workbench
        .terminal(
            &thread,
            "request_user",
            json!({"command":"private-two", "request_id":"private-two", "private":true}),
            true,
        )
        .await
        .expect("second private request");
    let db = rusqlite::Connection::open(s.root.join("workbench.sqlite3")).expect("journal");
    let count: i64 = db
        .query_row("select count(distinct digest) from terminal_commands", [], |row| {
            row.get(0)
        })
        .expect("fingerprints");
    assert_eq!(
        count, 1,
        "different private command text must not change the persisted context fingerprint"
    );
    let retry = s
        .workbench
        .terminal(
            &thread,
            "request_user",
            json!({"command":"different-private-text", "request_id":"private-one", "private":true}),
            true,
        )
        .await
        .expect("original receipt");
    assert_eq!(retry["id"], first["id"]);
    assert_eq!(
        retry["command"], "private-one",
        "a retry cannot replace the original command"
    );
    assert!(
        s.workbench
            .terminal(
                &thread,
                "run",
                json!({"command":"private-one", "request_id":"private-one"}),
                false
            )
            .await
            .expect_err("handoff remains required")
            .starts_with("terminal_request_conflict")
    );
    cleanup(s).await;
}

/// Backspace erases a whole letter of the answer, not one of its bytes: the command reads what the screen shows, and
/// erasing stops at the prompt.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[tokio::test]
async fn backspace_erases_a_whole_utf8_letter_of_the_answer() {
    let (s, thread) = fixture("utf8-erase", PermissionMode::Default).await;
    let command = "printf 'Enter a word: '; read -r word; printf 'answer:[%s]\\n' \"$word\"";
    let staged = s
        .workbench
        .terminal(&thread, "run", json!({"command":command,"request_id":"utf8"}), false)
        .await
        .expect("stage");
    let id = staged["id"].clone();
    s.workbench
        .terminal(
            &thread,
            "execute",
            json!({"id":id,"command":command,"expected_command":command}),
            true,
        )
        .await
        .expect("run");
    let ready = s
        .workbench
        .terminal(&thread, "read", json!({"id":id,"wait_ms":1000}), true)
        .await
        .expect("read prompt");
    assert!(ready["output"].as_str().expect("output").contains("Enter a word:"));
    // Two letters of two bytes each, then three Backspaces as the Commands view sends them (0x7f): two erase the
    // letters, the third finds nothing left to erase.
    s.workbench
        .terminal(
            &thread,
            "input",
            json!({"id":id,"data":"фы\u{7f}\u{7f}\u{7f}ok\n"}),
            true,
        )
        .await
        .expect("type the answer");
    let done = finished(&s, &thread, &id).await;
    let output = done["output"].as_str().expect("output");
    assert!(output.contains("answer:[ok]"), "{output:?}");
    // One erase echoed per letter, none past them into the prompt.
    assert_eq!(output.matches("\u{8} \u{8}").count(), 2, "{output:?}");
    cleanup(s).await;
}
