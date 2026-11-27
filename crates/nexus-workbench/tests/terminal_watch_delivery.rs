//! How a watched `terminal_run` command's exit notice reaches its session: as a steer mid-turn, as a new turn when
//! idle, or as a transcript row when the agent cannot take it.
#![cfg(unix)]

mod support;

use nexus_workbench::ThreadPatch;
use nexus_workbench::event::{PermissionMode, SessionAgent, TurnStatus, WbEvent};
use serde_json::json;
use support::watch::*;
use support::*;

/// Watched exits steer mid-turn for both Claude Code and Codex and wake an idle session into a new turn. The notice
/// gives the command id and exit code, nothing the command printed, and never logs a `User` row.
#[tokio::test]
async fn watched_exit_steers_mid_turn_and_wakes_idle_for_claude_and_codex() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let name = format!("watch-steer-{}", agent.as_str());
        let s = setup_terminal(&name);
        let prompt = match agent {
            SessionAgent::Claude => "slow",
            SessionAgent::Codex => "queue-probe",
            _ => unreachable!(),
        };
        let mut new = new_root(&s.project, prompt);
        new.agent = agent;
        new.orchestration = false;
        new.permission_mode = PermissionMode::BypassPermissions;
        let thread = s.workbench.create_thread(new).await.expect("create thread");
        let at = first_endpoint(&s).await;

        // Wait until the turn is actively running.
        match agent {
            SessionAgent::Claude => {
                wait_for(&s.workbench, &thread.id, |events| !texts(events).is_empty()).await;
            }
            SessionAgent::Codex => {
                wait_for(&s.workbench, &thread.id, |events| {
                    events.iter().any(|e| matches!(e.event, WbEvent::Tool { .. }))
                })
                .await;
            }
            _ => unreachable!(),
        }

        // Start a watched command that prints, then exits with code 3.
        let cmd = "printf 'mid-turn-output\\n'; exit 3";
        let (started, failed) = mcp_call(
            &at,
            2,
            "terminal_run",
            json!({"command": cmd, "request_id": "mid-turn", "watch": true}),
        )
        .await;
        assert!(!failed, "{started}");
        let cmd_id = started["id"].as_str().expect("cmd id").to_string();

        // Notice reaches the agent inside the running turn as a steer.
        let events = wait_for(&s.workbench, &thread.id, |events| {
            let notice_ok = watch_notices(events).len() == 1;
            let steered_ok = match agent {
                SessionAgent::Claude => texts(events)
                    .iter()
                    .any(|t| t.contains("Heard mid-turn: The watched command")),
                SessionAgent::Codex => texts(events)
                    .iter()
                    .any(|t| t.starts_with("Steered input: ") && t.contains("The watched command")),
                _ => unreachable!(),
            };
            notice_ok && steered_ok
        })
        .await;

        let notices = watch_notices(&events);
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0],
            format!(
                "The watched command \"printf 'mid-turn-output\\n'; exit 3\" ({cmd_id}) exited with code 3. Read its \
                 output with terminal_read."
            )
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(&e.event, WbEvent::User { text, .. } if text.contains("The watched command"))),
            "notice must not create a User row"
        );
        // Only one turn started so far (mid-turn steer, not a second turn).
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.event, WbEvent::TurnStart { .. }))
                .count(),
            1
        );

        // Settle to idle, then verify a watched command wakes the idle session into a new turn.
        if agent == SessionAgent::Codex {
            s.workbench.interrupt(&thread.id).expect("interrupt codex probe");
            wait_for(&s.workbench, &thread.id, |e| turn_ends(e).len() == 1).await;
            // Lift halt with a person message so the idle session can accept notices.
            s.workbench.send(&thread.id, "ready").await.expect("lift halt");
            wait_for(&s.workbench, &thread.id, |e| turn_ends(e).len() == 2).await;
        } else {
            wait_for(&s.workbench, &thread.id, |e| turn_ends(e).len() == 1).await;
        }
        let turns_before = turn_ends(&s.workbench.events(&thread.id, 0).expect("events")).len();
        let (idle_cmd, failed) = mcp_call(
            &at,
            3,
            "terminal_run",
            json!({"command": "printf 'idle-watch-ok'", "request_id": "idle-1", "watch": true}),
        )
        .await;
        assert!(!failed, "{idle_cmd}");
        let idle_id = idle_cmd["id"].as_str().expect("idle cmd id");
        let events = wait_for(&s.workbench, &thread.id, |e| turn_ends(e).len() > turns_before).await;
        let notices = watch_notices(&events);
        assert_eq!(notices.len(), 2, "{notices:?}");
        assert_eq!(
            notices[1],
            format!(
                "The watched command \"printf 'idle-watch-ok'\" ({idle_id}) exited with code 0. Read its output with \
                 terminal_read."
            )
        );
        assert_eq!(turn_ends(&events).last(), Some(&TurnStatus::Completed));

        s.workbench.shutdown().await;
        let _ = std::fs::remove_dir_all(&s.root);
    }
}

/// Runs a gated command that prints `secret`, marks it private before it prints, and opens the gate.
async fn run_private(s: &Setup, thread_id: &str, request_id: &str, secret: &str) -> String {
    let gate = s.root.join(format!("{request_id}-gate"));
    let cmd_id = run_watched(
        &s.workbench,
        thread_id,
        request_id,
        &gated(&gate, &format!("printf '{secret}\\n'; exit 7")),
    )
    .await;
    s.workbench
        .terminal(thread_id, "private", json!({"id": cmd_id}), true)
        .await
        .expect("mark private");
    open(&gate);
    cmd_id
}

/// Neither the real command text, the gate's path nor what the command printed appears; "Private terminal" does.
fn assert_private(text: &str, secret: &str) {
    assert!(text.contains("\"Private terminal\""), "{text}");
    for leak in [secret, "printf", "-gate", "while"] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
}

/// A private watched command's notice and undelivered row name it "Private terminal" and hold nothing it printed.
/// The command prints its secret only after the person marked it private.
#[tokio::test]
async fn private_watched_command_is_named_private_terminal() {
    let s = setup_terminal("watch-private");
    let thread = idle_session(&s).await;
    let wb = &s.workbench;

    let cmd_id = run_private(&s, &thread.id, "priv-1", "SECRET_TOKEN_XYZ").await;
    let events = wait_for(wb, &thread.id, |e| !watch_notices(e).is_empty()).await;
    let notice = &watch_notices(&events)[0];
    assert_eq!(
        notice,
        &format!(
            "The watched command \"Private terminal\" ({cmd_id}) exited with code 7. Read its output with \
             terminal_read."
        )
    );
    assert_private(notice, "SECRET_TOKEN_XYZ");
    wait_for(wb, &thread.id, |e| turn_ends(e).len() == 2).await;

    // With the agent not running, the person's row says the same.
    wb.restart(&thread.id).expect("restart");
    until_stopped(wb, &thread.id).await;
    let row_id = run_private(&s, &thread.id, "priv-2", "SECRET_ROW_XYZ").await;
    let events = wait_for(wb, &thread.id, |e| !undelivered(e).is_empty()).await;
    let row = &undelivered(&events)[0];
    assert_eq!(
        row,
        &format!(
            "The watched command \"Private terminal\" ({row_id}) exited with code 7. The agent was not told: the \
             session was halted or its agent was not running. Send a message to tell it."
        )
    );
    assert_private(row, "SECRET_ROW_XYZ");

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// An agent read waiting when the command exits returns the end itself, so no notice is sent. A person's read
/// changes nothing.
#[tokio::test]
async fn an_agent_read_of_the_end_suppresses_the_notice_but_a_persons_read_does_not() {
    let s = setup_terminal("watch-already-read");
    let thread = idle_session(&s).await;
    let wb = &s.workbench;
    let gate = s.root.join("read-gate");

    // The command prints, then exits silently when the gate opens: the read waits from the cursor after the output.
    let read_id = run_watched(
        wb,
        &thread.id,
        "ar-1",
        &format!("printf 'read-directly'; {}", gated(&gate, "exit 0")),
    )
    .await;
    let after = output_seen(wb, &thread.id, &read_id, "read-directly").await;
    let read = waiting_agent_read(wb, &thread.id, &read_id, after).await;
    open(&gate);
    let end = read.await.expect("agent read");
    assert_eq!(end["terminal"]["exitCode"], 0, "{end}");

    // The marker's notice comes alone: the command the agent read to its end sent none.
    let notices = marker(wb, &thread.id, "after-agent-read").await;
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(!notices[0].contains(&read_id), "{notices:?}");

    // The person's read, waiting while the command exits, leaves the agent's notice due.
    let human_id = run_watched(wb, &thread.id, "ar-2", "sleep 0.25; printf 'human-read-only'").await;
    let human_read = wb
        .terminal(&thread.id, "read", json!({"id": human_id, "wait_ms": 5000}), true)
        .await
        .expect("human read");
    assert_eq!(human_read["terminal"]["exitCode"], 0);
    let events = wait_for(wb, &thread.id, |e| watch_notices(e).len() == 2).await;
    assert!(is_notice_for(&watch_notices(&events)[1], &human_id, 0));

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A waiting agent read that is cancelled never swallows the notice: whether the command exits after the cancel, or
/// exits while the read waits and the read is cancelled before it returns the end.
#[tokio::test]
async fn a_cancelled_agent_read_does_not_swallow_the_notice() {
    let s = setup_terminal("watch-cancelled-read");
    let thread = idle_session(&s).await;
    let wb = &s.workbench;

    // Cancelled, then the command exits.
    let gate = s.root.join("cancel-first-gate");
    let first = run_watched(wb, &thread.id, "cr-1", &gated(&gate, "printf 'cancel-first'")).await;
    drop(waiting_agent_read(wb, &thread.id, &first, 0).await);
    open(&gate);
    let events = wait_for(wb, &thread.id, |e| !watch_notices(e).is_empty()).await;
    let notices = watch_notices(&events);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(is_notice_for(&notices[0], &first, 0), "{notices:?}");
    wait_for(wb, &thread.id, |e| turn_ends(e).len() == 2).await;

    // The command exits while the read waits, then the read is cancelled before it saw the end.
    let gate = s.root.join("exit-first-gate");
    let second = run_watched(wb, &thread.id, "cr-2", &gated(&gate, "exit 5")).await;
    let read = waiting_agent_read(wb, &thread.id, &second, 0).await;
    open(&gate);
    wait_command_ended(wb, &thread.id, &second).await;
    drop(read);
    let events = wait_for(wb, &thread.id, |e| watch_notices(e).len() == 2).await;
    let notices = watch_notices(&events);
    assert!(is_notice_for(&notices[1], &second, 5), "{notices:?}");

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Two watched commands that exit at the same moment each send exactly one notice.
#[tokio::test]
async fn two_watched_commands_exiting_together_each_send_one_notice() {
    let s = setup_terminal("watch-together");
    let thread = idle_session(&s).await;
    let gate = s.root.join("together-gate");
    let first = run_watched(&s.workbench, &thread.id, "tg-1", &gated(&gate, "printf 'first'")).await;
    let second = run_watched(&s.workbench, &thread.id, "tg-2", &gated(&gate, "printf 'second'")).await;
    open(&gate);

    let events = wait_for(&s.workbench, &thread.id, |e| watch_notices(e).len() >= 2).await;
    let notices = watch_notices(&events);
    for id in [&first, &second] {
        assert_eq!(
            notices.iter().filter(|notice| notice.contains(id.as_str())).count(),
            1,
            "{notices:?}"
        );
    }
    // A later marker's notice is the third: neither command sent a second one.
    assert_eq!(marker(&s.workbench, &thread.id, "together-marker").await.len(), 3);

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A halted session refuses the notice without clearing its halt. The transcript says the agent was not told.
#[tokio::test]
async fn halted_session_records_the_undelivered_notice_without_clearing_halt() {
    let s = setup_terminal("watch-halted");
    let mut new = new_root(&s.project, "slow");
    new.orchestration = false;
    new.permission_mode = PermissionMode::BypassPermissions;
    let thread = s.workbench.create_thread(new).await.expect("create");
    wait_for(&s.workbench, &thread.id, |e| !texts(e).is_empty()).await;

    s.workbench.interrupt(&thread.id).expect("interrupt");
    wait_for(&s.workbench, &thread.id, |e| turn_ends(e).len() == 1).await;
    assert!(s.workbench.halt(&thread.id).expect("halt").is_some());

    let cmd_id = run_watched(&s.workbench, &thread.id, "h-1", "printf 'after-halt'; exit 4").await;
    let events = wait_for(&s.workbench, &thread.id, |e| !undelivered(e).is_empty()).await;
    assert_eq!(
        undelivered(&events),
        [format!(
            "The watched command \"printf 'after-halt'; exit 4\" ({cmd_id}) exited with code 4. The agent was not \
             told: the session was halted or its \
             agent was not running. Send a message to tell it."
        )]
    );
    assert!(watch_notices(&events).is_empty(), "{events:#?}");
    assert_eq!(turn_ends(&events).len(), 1);
    assert!(s.workbench.halt(&thread.id).expect("halt").is_some());

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Archiving leaves the agent running, so an archived session still gets the notice, and it stays archived.
#[tokio::test]
async fn an_archived_session_gets_the_notice_and_stays_archived() {
    let s = setup_terminal("watch-archived");
    let thread = idle_session(&s).await;
    s.workbench
        .update(
            &thread.id,
            ThreadPatch {
                archived: Some(true),
                ..Default::default()
            },
        )
        .expect("archive");

    let notices = marker(&s.workbench, &thread.id, "archived-marker").await;
    assert_eq!(notices.len(), 1, "{notices:?}");
    let events = wait_for(&s.workbench, &thread.id, |e| turn_ends(e).len() == 2).await;
    assert!(undelivered(&events).is_empty());
    assert!(shown(&s, &thread.id).expect("shown").archived);

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
