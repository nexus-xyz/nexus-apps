//! Provider event reconciliation never starts or replays a command.
mod support;
use nexus_workbench::{
    Config,
    event::{Decision, PermissionMode, WbEvent},
};
use serde_json::json;
use support::*;

#[tokio::test]
async fn native_provider_calls_are_recorded_once_with_their_actual_ownership_and_no_invented_exit_code() {
    for terminal in [false, true] {
        let s = setup_with(&format!("native-history-{terminal}"), bounded(), false, |config| {
            Config { terminal, ..config }
        });
        let mut new = new_root(&s.project, "tool");
        new.permission_mode = PermissionMode::Default;
        new.orchestration = false;
        let created = s.workbench.create_thread(new).await.expect("session");
        let request = approval(&s.workbench, &created.id).await;
        let pending = s
            .workbench
            .terminal(&created.id, "list", json!({}), true)
            .await
            .expect("history while pending");
        assert_eq!(pending[0]["status"], "providerPending");
        assert_eq!(pending[0]["owner"], "provider");
        assert_eq!(pending[0]["route"], "externalObserved");
        assert!(
            s.workbench
                .terminal(&created.id, "execute", json!({"id": pending[0]["id"]}), true)
                .await
                .expect_err("no replay")
                .starts_with(if terminal {
                    "terminal_external_process"
                } else {
                    "terminal_unavailable"
                })
        );
        s.workbench
            .approve(&created.id, &request, Decision::Allow, None, None, None)
            .expect("provider approval");
        wait_for(&s.workbench, &created.id, |events| turn_ends(events).len() == 1).await;
        let history = s
            .workbench
            .terminal(&created.id, "list", json!({}), true)
            .await
            .expect("history");
        let rows = history.as_array().expect("entries");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], "completed");
        assert!(
            rows[0]["exitCode"].is_null(),
            "a tool result is not a process exit code"
        );
        assert_eq!(
            rows[0]["bypassCode"],
            if terminal {
                "native_execution_unintercepted"
            } else {
                "host_unsupported"
            }
        );
        let notices = s
            .workbench
            .events(&created.id, 0)
            .expect("events")
            .into_iter()
            .filter(|event| matches!(&event.event, WbEvent::Notice { text } if text.starts_with(nexus_workbench::terminal::BYPASS_NOTICE)))
            .count();
        assert_eq!(notices, 1);
        s.workbench
            .send(&created.id, "another tool")
            .await
            .expect("second provider call");
        let events = wait_for(&s.workbench, &created.id, |events| {
            events
                .iter()
                .filter(|event| matches!(event.event, WbEvent::Approval { .. }))
                .count()
                == 2
        })
        .await;
        let request = events
            .iter()
            .rev()
            .find_map(|event| match &event.event {
                WbEvent::Approval { request_id, .. } => Some(request_id.clone()),
                _ => None,
            })
            .expect("second approval");
        s.workbench
            .approve(&created.id, &request, Decision::Allow, None, None, None)
            .expect("approve second call");
        wait_for(&s.workbench, &created.id, |events| turn_ends(events).len() == 2).await;
        let history = s
            .workbench
            .terminal(&created.id, "list", json!({}), true)
            .await
            .expect("both receipts");
        let rows = history.as_array().expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["command"], rows[1]["command"]);
        assert_ne!(rows[0]["id"], rows[1]["id"]);
        assert_ne!(rows[0]["providerCallId"], rows[1]["providerCallId"]);
        assert_eq!(rows[1]["status"], "completed");
        s.workbench.shutdown().await;
        let _ = std::fs::remove_dir_all(s.root);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn shared_agent_shell_input_is_history_without_persisting_human_input() {
    let s = setup_with("shell-input-history", bounded(), false, |config| Config {
        terminal: true,
        ..config
    });
    let mut new = new_root(&s.project, "hello");
    new.permission_mode = PermissionMode::BypassPermissions;
    new.orchestration = false;
    let thread = root(&s, new).await;
    let shell = s
        .workbench
        .terminal(&thread.id, "open", json!({"request_id":"shell"}), false)
        .await
        .expect("open");
    s.workbench
        .terminal(
            &thread.id,
            "input",
            json!({"id":shell["id"], "data":"echo agent-command\n", "lease":shell["lease"]}),
            false,
        )
        .await
        .expect("agent input");
    s.workbench
        .terminal(&thread.id, "take", json!({"id":shell["id"]}), true)
        .await
        .expect("takeover");
    s.workbench
        .terminal(
            &thread.id,
            "input",
            json!({"id":shell["id"], "data":"echo human-only\n"}),
            true,
        )
        .await
        .expect("human input");
    let history = s
        .workbench
        .terminal(&thread.id, "submissions", json!({"id":shell["id"]}), true)
        .await
        .expect("input history");
    assert_eq!(history["inputs"], json!(["echo agent-command\n"]));
    s.workbench
        .terminal(&thread.id, "private", json!({"id":shell["id"]}), true)
        .await
        .expect("private");
    assert!(
        s.workbench
            .terminal(&thread.id, "submissions", json!({"id":shell["id"]}), false)
            .await
            .expect_err("private history")
            .starts_with("terminal_private")
    );
    s.workbench
        .terminal(
            &thread.id,
            "input",
            json!({"id":shell["id"], "data":"echo private-only\n"}),
            true,
        )
        .await
        .expect("private input");
    let db = rusqlite::Connection::open(s.root.join("workbench.sqlite3")).expect("database");
    let journal: String = db
        .query_row("select group_concat(body) from terminal_command_events", [], |row| {
            row.get(0)
        })
        .expect("events");
    assert!(journal.contains("agent-command"));
    assert!(!journal.contains("human-only"));
    assert!(!journal.contains("private-only"));
    s.workbench.shutdown().await;
}
