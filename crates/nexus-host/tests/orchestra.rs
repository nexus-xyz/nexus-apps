//! Linked sessions on a host: its hello offers `orchestration`, and a lead the
//! app starts over the socket calls the host's own `nexus_app` server, which its workbench runs on the host's
//! loopback. The stand-in `claude` (nexus-workbench's src/bin/fake-claude.rs, `orchestrate`) starts a child through
//! it, waits, and reads the child's result whole.

use std::time::{Duration, Instant};

use nexus_host::client::connect;
use nexus_workbench::event::{StoredEvent, TurnStatus, WbEvent};
use nexus_workbench::linked::results::{RESULTS_HEADER, receipt_of};
use nexus_workbench::process;
use serde_json::{Value, json};
use tokio::net::UnixStream;

mod common;

use common::{TOKEN, start};

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = process::command("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .status()
        .is_ok_and(|status| status.success());
    assert!(ok, "git {args:?}");
}

fn turn_ends(events: &[StoredEvent]) -> Vec<TurnStatus> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::TurnEnd { status, .. } => Some(*status),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lead_on_the_host_starts_a_child_through_its_server_and_reads_its_result() {
    let host = start("orch");
    git(&host.repo, &["init", "--quiet", "--initial-branch", "main"]);
    git(&host.repo, &["commit", "--quiet", "--allow-empty", "-m", "one"]);
    let stream = UnixStream::connect(&host.socket)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let mut app = connect(stream, TOKEN).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        app.info.features,
        [
            "orchestration",
            "session-commands",
            "session-connection",
            "orchestration-controls",
            "session-start-choice",
            "project-clone",
            "project-dirs",
            "diff-sources",
            "project-files",
            "file-previews",
            "file-preview-pieces",
            "inline-media",
            "project-pin-archive",
            "side-chats",
            "move-to-worktree",
            "server-shells"
        ]
    );

    let project = app
        .call("wb_project_add", json!({ "path": host.repo }))
        .await
        .0
        .unwrap_or_else(|e| panic!("{e}"));
    app.call(
        "wb_project_orchestration_set",
        json!({"projectId":project["id"],"on":true}),
    )
    .await
    .0
    .expect("project setting");
    // A project on the host is pinned and archived through it, as on this computer.
    let organized = app
        .call(
            "wb_project_update",
            json!({"projectId":project["id"],"pinned":true,"archived":true}),
        )
        .await
        .0
        .expect("project pinned and archived");
    assert_eq!(
        (&organized["pinned"], &organized["archived"]),
        (&json!(true), &json!(true))
    );
    app.call("wb_project_update", json!({"projectId":project["id"],"archived":false}))
        .await
        .0
        .expect("project back from the archive");
    let create = json!({
        "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "acceptEdits",
        "effort": null, "via": "nexus", "text": "orchestrate", "images": [], "worktree": null,
    });
    let lead = app
        .call("wb_thread_create", create)
        .await
        .0
        .unwrap_or_else(|e| panic!("{e}"));
    let lead = lead["id"].as_str().unwrap_or_default().to_string();

    let workbench = host.api.workbench().clone();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut answered = false;
    let events = loop {
        if let Some(choice) = workbench
            .threads()
            .expect("valid test fixture")
            .into_iter()
            .find(|t| t.id == lead)
            .and_then(|t| t.start_choices.into_iter().next())
        {
            assert!(!answered, "only one choice for this start");
            assert!(
                workbench
                    .list_children(&lead)
                    .expect("valid test fixture")
                    .sessions
                    .is_empty()
            );
            app.call(
                "wb_session_start_answer",
                json!({"threadId": lead, "requestId": choice.id, "choice": "new", "remember": false}),
            )
            .await
            .0
            .expect("valid test fixture");
            answered = true;
        }
        let events = workbench.events(&lead, 0).unwrap_or_default();
        if turn_ends(&events).len() == 1 {
            break events;
        }
        assert!(Instant::now() < deadline, "the lead's turn never ended: {events:#?}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(answered, "the host must ask before creating a child");
    let results: Vec<(String, bool)> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::ToolResult { output, is_error, .. } => Some((output.clone(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2, "{events:#?}");
    assert!(results.iter().all(|(_, error)| !error), "{results:?}");
    let started: Value = serde_json::from_str(&results[0].0).unwrap_or_else(|e| panic!("{e}"));
    let child = started["session_id"].as_str().unwrap_or_default().to_string();
    let answer = &results[1].0;
    assert!(
        receipt_of(answer).is_some() && answer.contains(RESULTS_HEADER),
        "{answer}"
    );
    assert!(answer.contains(&format!("session=\"{child}\"")), "{answer}");
    assert!(answer.contains("Message 1 of this session."), "{answer}");
    // Read whole: nothing is left for the lead.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let children = workbench.list_children(&lead).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(children.sessions.len(), 1);
        if !children.sessions[0].has_result {
            break;
        }
        assert!(Instant::now() < deadline, "the result was never read");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let _ = host.stop.send(true);
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&host.root);
}

#[tokio::test]
async fn a_server_cannot_reload_limits_from_desktop_settings() {
    use nexus_host::server::Peer;
    use nexus_workbench::api::{Answer, Request};
    use nexus_workbench::orchestra::{LIMITS_FEATURE, RELOAD_LIMITS_CALL};

    let host = start("limits-server");
    assert!(
        !host
            .server
            .info
            .features
            .iter()
            .any(|feature| feature == LIMITS_FEATURE)
    );
    for peer in [Peer::Local, Peer::Relayed] {
        let request = Request {
            id: 1,
            call: RELOAD_LIMITS_CALL.into(),
            args: json!({}),
        };
        let reply = host.server.answer(request, peer).await;
        assert!(matches!(reply.answer, Answer::Error(ref why) if why.contains("local desktop service")));
    }
}
