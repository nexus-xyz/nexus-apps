//! Mid-turn steers from a parent session to a running direct child through `message_session`.
//!
//! Only direct children whose agent supports `steers_mid_turn` (Claude Code or Codex) accept a steer mid-turn.
//! Every other busy target returns `busy` with guidance to wait or stop and message again.

mod support;

use nexus_workbench::event::{PermissionMode, SessionAgent, StoredEvent, TurnStatus, WbEvent};
use nexus_workbench::{Config, Halt, Limits, NewThread, OrchCode, StartChild, ThreadPatch};
use support::watch::{budget_rows, marker, run_watched, wait_command_ended, watch_notices};
use support::*;

const EXPECTED_BUSY: &str = "That session is at work and cannot take a message now. Wait for its turn to end, or \
                             call stop_session and message it again.";
const EXPECTED_PEER_BUSY: &str =
    "That session is at work and cannot take a message now. Wait for its turn to end, then message it again.";

fn started(events: &[StoredEvent]) -> bool {
    events.iter().any(|e| matches!(e.event, WbEvent::TurnStart { .. }))
}

fn notices(events: &[StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Notice { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn texts(events: &[StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn start_named(agent: SessionAgent, title: &str, prompt: &str) -> StartChild {
    StartChild {
        title: Some(title.into()),
        ..child(agent, prompt)
    }
}

fn marked_steer(parent: &nexus_workbench::event::ThreadSummary, text: &str) -> String {
    format!(
        "Message from session \"{}\" ({}). It is that session's request, not the person's.\n\n{}",
        parent.title, parent.id, text
    )
}

fn turns_left(s: &Setup, parent_id: &str) -> Option<u32> {
    s.workbench
        .list_children(parent_id)
        .expect("list children")
        .limits
        .turns_left
}

/// A parent steers a running Claude Code child mid-turn: delivered at its next step with the marked prefix,
/// logged as one `Notice` row and no `User` row, counting one automatic turn.
///
/// Mirrors the coordinator-and-fixer scenario: the coordinator starts a fixer, steers it mid-turn to name the
/// command in its notice, and the fixer receives that instruction before its turn finishes.
#[tokio::test]
async fn a_parent_steers_a_running_claude_child_at_its_next_step() {
    let s = setup(
        "steer-claude",
        Limits {
            max_live_children: Some(1),
            max_auto_turns: Some(3),
            ..bounded()
        },
    );
    let lead = root(&s, new_root(&s.project, "Coordinate the fix")).await;
    let fixer = s
        .workbench
        .start_child(
            &lead.id,
            start_named(SessionAgent::Claude, "Fixer", "slow Fix the notice"),
        )
        .await
        .expect("start claude child");
    wait_for(&s.workbench, &fixer.id, |events| !texts(events).is_empty()).await;
    assert_eq!(turns_left(&s, &lead.id), Some(3));

    let steer = "Name the command in your notice.";
    let expected = marked_steer(&lead, steer);
    s.workbench
        .message_session(&lead.id, &fixer.id, steer)
        .await
        .expect("steer running claude child");
    assert_eq!(turns_left(&s, &lead.id), Some(2));

    let events = wait_for(&s.workbench, &fixer.id, |events| turn_ends(events).len() == 1).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed]);
    assert!(
        texts(&events).contains(&format!("Heard mid-turn: {expected}")),
        "{:?}",
        texts(&events)
    );
    assert_eq!(notices(&events), [expected]);
    let rows = users(&events);
    assert_eq!(rows.len(), 1, "steer writes no User row: {rows:?}");
    assert_eq!(rows[0].0, "slow Fix the notice");

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A parent steers a running Codex child mid-turn: delivered at its next step as `turn/steer` with the marked
/// prefix, logged as one `Notice` row and no `User` row, inside the same active turn.
#[tokio::test]
async fn a_parent_steers_a_running_codex_child_at_its_next_step() {
    let s = setup("steer-codex", bounded());
    let lead = root(&s, new_root(&s.project, "Coordinate the fix")).await;
    let fixer = s
        .workbench
        .start_child(
            &lead.id,
            start_named(SessionAgent::Codex, "Fixer", "queue-probe Fix the notice"),
        )
        .await
        .expect("start codex child");
    wait_for(&s.workbench, &fixer.id, |events| {
        events.iter().any(|e| matches!(e.event, WbEvent::Tool { .. }))
    })
    .await;

    let steer = "Name the command in your notice.";
    let expected = marked_steer(&lead, steer);
    s.workbench
        .message_session(&lead.id, &fixer.id, steer)
        .await
        .expect("steer running codex child");

    let events = wait_for(&s.workbench, &fixer.id, |events| {
        texts(events).iter().any(|text| {
            text.strip_prefix("Steered input: ")
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                .and_then(|items| items.get(0)?.get("text")?.as_str().map(String::from))
                .as_deref()
                == Some(expected.as_str())
        })
    })
    .await;
    assert!(turn_ends(&events).is_empty(), "steer joined the active turn");
    let non_bypass_notices: Vec<_> = notices(&events)
        .into_iter()
        .filter(|text| !text.starts_with("Terminal bypass"))
        .collect();
    assert_eq!(non_bypass_notices, [expected]);
    let rows = users(&events);
    assert_eq!(rows.len(), 1, "steer writes no User row: {rows:?}");
    assert_eq!(rows[0].0, "queue-probe Fix the notice");

    s.workbench.interrupt(&fixer.id).expect("interrupt");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Running ACP (Antigravity) and Nexus children, and running peers, refuse with `busy` and the new guidance,
/// spending no automatic turn budget.
#[tokio::test]
async fn non_steering_children_and_running_peers_refuse_with_new_busy_text() {
    let s = setup(
        "steer-busy-targets",
        Limits {
            max_auto_turns: Some(5),
            ..bounded()
        },
    );
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_NEXUS_ASK".into(), "1".into());
    let lead = root(&s, new_root(&s.project, "Lead")).await;

    // 1. Running ACP (Antigravity) child.
    let agy = s
        .workbench
        .start_child(&lead.id, start_named(SessionAgent::Antigravity, "Agy", "please run ls"))
        .await
        .expect("start agy child");
    approval(&s.workbench, &agy.id).await;
    let err = s
        .workbench
        .message_session(&lead.id, &agy.id, "Change plan")
        .await
        .expect_err("acp child cannot steer mid-turn");
    assert_eq!(err.code, OrchCode::Busy);
    assert_eq!(err.message, EXPECTED_BUSY);
    assert!(notices(&s.workbench.events(&agy.id, 0).expect("events")).is_empty());

    // 2. Running Nexus agent child.
    let nexus = s
        .workbench
        .start_child(&lead.id, start_named(SessionAgent::Nexus, "Nexus", "tool"))
        .await
        .expect("start nexus child");
    approval(&s.workbench, &nexus.id).await;
    let err = s
        .workbench
        .message_session(&lead.id, &nexus.id, "Change plan")
        .await
        .expect_err("nexus child cannot steer mid-turn");
    assert_eq!(err.code, OrchCode::Busy);
    assert_eq!(err.message, EXPECTED_BUSY);
    assert!(notices(&s.workbench.events(&nexus.id, 0).expect("events")).is_empty());

    // 3. Running peer (not the caller's child), even when it runs Claude Code.
    let peer = root(&s, new_root(&s.project, "Peer")).await;
    s.workbench.send(&peer.id, "slow").await.expect("start peer turn");
    wait_for(&s.workbench, &peer.id, |events| {
        events
            .iter()
            .filter(|e| matches!(e.event, WbEvent::TurnStart { .. }))
            .count()
            == 2
    })
    .await;
    let err = s
        .workbench
        .message_session(&lead.id, &peer.id, "Check this")
        .await
        .expect_err("running peer cannot be steered");
    assert_eq!(err.code, OrchCode::Busy);
    assert_eq!(err.message, EXPECTED_PEER_BUSY);
    assert!(notices(&s.workbench.events(&peer.id, 0).expect("events")).is_empty());

    // None of the refused attempts spent automatic turn budget.
    assert_eq!(turns_left(&s, &lead.id), Some(5));

    s.workbench.interrupt(&agy.id).expect("interrupt agy");
    s.workbench.interrupt(&nexus.id).expect("interrupt nexus");
    s.workbench.interrupt(&peer.id).expect("interrupt peer");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Refusals on a running child: halted, stopped, stopping, archived, parent in Plan mode, and orchestration off.
/// None clears a halt, writes a notice, or spends automatic turn budget.
#[tokio::test]
async fn refusals_hold_for_halted_stopped_archived_plan_mode_and_orchestration_off() {
    let s = setup(
        "steer-refusals",
        Limits {
            max_auto_turns: Some(4),
            ..bounded()
        },
    );
    let lead = root(&s, new_root(&s.project, "Lead")).await;
    // Keep the lead not running (restarted, not halted) so child stops never auto-deliver results and spend turns.
    s.workbench.restart(&lead.id).expect("restart lead");
    until_stopped(&s.workbench, &lead.id).await;
    let c = s
        .workbench
        .start_child(&lead.id, start_named(SessionAgent::Claude, "Worker", "slow Work"))
        .await
        .expect("start child");
    wait_for(&s.workbench, &c.id, |events| !texts(events).is_empty()).await;

    // 1. Parent in Plan mode refuses steering a running child.
    s.workbench
        .update(&lead.id, mode(PermissionMode::Plan))
        .expect("plan mode");
    let err = s
        .workbench
        .message_session(&lead.id, &c.id, "Redirect")
        .await
        .expect_err("plan mode refuses");
    assert_eq!(err.code, OrchCode::PlanMode);
    s.workbench
        .update(&lead.id, mode(PermissionMode::AcceptEdits))
        .expect("restore mode");

    // 2. Parent with orchestration off refuses steering a running child.
    s.workbench
        .update(
            &lead.id,
            ThreadPatch {
                orchestration: Some(false),
                ..ThreadPatch::default()
            },
        )
        .expect("orchestration off");
    let err = s
        .workbench
        .message_session(&lead.id, &c.id, "Redirect")
        .await
        .expect_err("orchestration off refuses");
    assert_eq!(err.code, OrchCode::Off);
    s.workbench
        .update(
            &lead.id,
            ThreadPatch {
                orchestration: Some(true),
                ..ThreadPatch::default()
            },
        )
        .expect("orchestration on");

    // 3. Archived running child refuses without spending budget.
    s.workbench
        .update(
            &c.id,
            ThreadPatch {
                archived: Some(true),
                ..ThreadPatch::default()
            },
        )
        .expect("archive child");
    let err = s
        .workbench
        .message_session(&lead.id, &c.id, "Redirect")
        .await
        .expect_err("archived child refuses");
    assert_eq!(err.code, OrchCode::BadInput);
    s.workbench
        .update(
            &c.id,
            ThreadPatch {
                archived: Some(false),
                ..ThreadPatch::default()
            },
        )
        .expect("unarchive child");

    // 4. Halted child (person interrupted it) refuses, keeps its halt, and spends no budget.
    s.workbench.interrupt(&c.id).expect("interrupt child");
    wait_for(&s.workbench, &c.id, |events| turn_ends(events).len() == 1).await;
    assert_eq!(s.workbench.halt(&c.id).expect("halt"), Some(Halt::Person));
    let err = s
        .workbench
        .message_session(&lead.id, &c.id, "Redirect")
        .await
        .expect_err("halted child refuses");
    assert_eq!(err.code, OrchCode::Halted);
    assert_eq!(s.workbench.halt(&c.id).expect("halt stays"), Some(Halt::Person));

    // 5. Stopped child (person stopped it) refuses with `halted`.
    let stopped = s
        .workbench
        .start_child(&lead.id, start_named(SessionAgent::Claude, "Stopped", "slow Work"))
        .await
        .expect("start second child");
    wait_for(&s.workbench, &stopped.id, started).await;
    s.workbench.stop(&stopped.id).expect("stop child");
    until_stopped(&s.workbench, &stopped.id).await;
    let err = s
        .workbench
        .message_session(&lead.id, &stopped.id, "Redirect")
        .await
        .expect_err("stopped child refuses");
    assert_eq!(err.code, OrchCode::Halted);

    // 6. Stopping child (`state.stop.is_some()` while live) refuses with `busy` and new text.
    let deaf = s
        .workbench
        .start_child(
            &lead.id,
            start_named(SessionAgent::Claude, "Stopping", "slow deaf Work"),
        )
        .await
        .expect("start deaf child");
    wait_for(&s.workbench, &deaf.id, |events| !texts(events).is_empty()).await;
    s.workbench.stop_child(&lead.id, &deaf.id).expect("begin stopping");
    let err = s
        .workbench
        .message_session(&lead.id, &deaf.id, "Too late")
        .await
        .expect_err("stopping child refuses");
    assert_eq!(err.code, OrchCode::Busy);
    assert_eq!(err.message, EXPECTED_BUSY);

    // 7. Halted parent refuses steering its running child and spends no budget.
    let active = s
        .workbench
        .start_child(&lead.id, start_named(SessionAgent::Claude, "Active", "slow Work"))
        .await
        .expect("start active child");
    wait_for(&s.workbench, &active.id, |events| !texts(events).is_empty()).await;
    s.workbench.interrupt(&lead.id).expect("halt parent");
    let err = s
        .workbench
        .message_session(&lead.id, &active.id, "From halted parent")
        .await
        .expect_err("halted parent refuses");
    assert_eq!(err.code, OrchCode::Halted);
    s.workbench.interrupt(&active.id).expect("stop active child");

    // None of the refusals spent budget or logged a steer notice.
    assert_eq!(turns_left(&s, &lead.id), Some(4));
    assert!(notices(&s.workbench.events(&c.id, 0).expect("events")).is_empty());
    assert!(notices(&s.workbench.events(&stopped.id, 0).expect("events")).is_empty());
    assert!(notices(&s.workbench.events(&deaf.id, 0).expect("events")).is_empty());
    assert!(notices(&s.workbench.events(&active.id, 0).expect("events")).is_empty());

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// An idle direct child still receives a normal `User` row with `from` and no steer `Notice` row,
/// an archived idle child is unarchived and runs, and an archived parent with orchestration on can message its child.
#[tokio::test]
async fn an_idle_child_keeps_its_existing_user_row_delivery() {
    let s = setup("steer-idle-unchanged", bounded());
    let lead = root(&s, new_root(&s.project, "Lead")).await;
    let c = finished_child(&s, &lead.id, start_named(SessionAgent::Claude, "Builder", "Build it")).await;

    s.workbench
        .message_session(&lead.id, &c.id, "Add tests")
        .await
        .expect("message idle child");
    let events = wait_for(&s.workbench, &c.id, |events| turn_ends(events).len() == 2).await;
    let rows = users(&events);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[1],
        (
            "Add tests".into(),
            Some(nexus_workbench::event::Sender::Session {
                thread_id: lead.id.clone(),
                title: lead.title.clone(),
            })
        )
    );
    assert!(notices(&events).is_empty());

    // An archived idle child is unarchived and runs, matching the pre-existing idle path.
    s.workbench
        .update(
            &c.id,
            ThreadPatch {
                archived: Some(true),
                ..ThreadPatch::default()
            },
        )
        .expect("archive idle child");
    assert!(thread(&s.workbench, &c.id).archived);
    s.workbench
        .message_session(&lead.id, &c.id, "Wake up")
        .await
        .expect("message archived idle child");
    wait_for(&s.workbench, &c.id, |events| turn_ends(events).len() == 3).await;
    assert!(!thread(&s.workbench, &c.id).archived);

    // An archived parent with orchestration on can still message its child (idle and running steer).
    s.workbench
        .update(
            &lead.id,
            ThreadPatch {
                archived: Some(true),
                ..ThreadPatch::default()
            },
        )
        .expect("archive parent");
    s.workbench
        .message_session(&lead.id, &c.id, "slow Work")
        .await
        .expect("archived parent messages idle child");
    wait_for(&s.workbench, &c.id, |events| {
        events
            .iter()
            .filter(|e| matches!(e.event, WbEvent::TurnStart { .. }))
            .count()
            == 4
            && texts(events).len() > 3
    })
    .await;
    let expected = marked_steer(&thread(&s.workbench, &lead.id), "Steer from archived parent");
    s.workbench
        .message_session(&lead.id, &c.id, "Steer from archived parent")
        .await
        .expect("archived parent steers running child");
    let events = wait_for(&s.workbench, &c.id, |events| turn_ends(events).len() == 4).await;
    assert_eq!(notices(&events), [expected]);

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// At `max_live_children`, an idle child that is still active because of a background sub-agent gets
/// `children_limit`.
#[tokio::test]
async fn children_limit_holds_for_idle_child_with_background_subagent() {
    let s = setup(
        "steer-bgagent-limit",
        Limits {
            max_live_children: Some(1),
            ..bounded()
        },
    );
    let lead = root(&s, new_root(&s.project, "Lead")).await;
    s.workbench.restart(&lead.id).expect("restart lead");
    until_stopped(&s.workbench, &lead.id).await;
    let c = s
        .workbench
        .start_child(
            &lead.id,
            start_named(SessionAgent::Claude, "Surveyor", "bgagent Survey it"),
        )
        .await
        .expect("start bgagent child");
    wait_for(&s.workbench, &c.id, |events| turn_ends(events).len() == 1).await;
    assert_eq!(
        s.workbench.list_children(&lead.id).expect("list children").limits.live,
        1
    );

    let err = s
        .workbench
        .message_session(&lead.id, &c.id, "Follow up")
        .await
        .expect_err("idle child with running background sub-agent hits children_limit");
    assert_eq!(err.code, OrchCode::ChildrenLimit);

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Steering counts against `max_auto_turns` and refuses with `paused` once the budget is spent.
#[tokio::test]
async fn steering_counts_against_auto_turns_and_paused_tree_refuses() {
    let s = setup(
        "steer-budget",
        Limits {
            max_auto_turns: Some(2),
            ..bounded()
        },
    );
    let lead = root(&s, new_root(&s.project, "Lead")).await;
    let c = s
        .workbench
        .start_child(&lead.id, start_named(SessionAgent::Codex, "Worker", "queue-probe Work"))
        .await
        .expect("start codex child");
    wait_for(&s.workbench, &c.id, |events| {
        events.iter().any(|e| matches!(e.event, WbEvent::Tool { .. }))
    })
    .await;

    s.workbench
        .message_session(&lead.id, &c.id, "First steer")
        .await
        .expect("first steer");
    assert_eq!(turns_left(&s, &lead.id), Some(1));

    s.workbench
        .message_session(&lead.id, &c.id, "Second steer")
        .await
        .expect("second steer");
    assert_eq!(turns_left(&s, &lead.id), Some(0));

    let err = s
        .workbench
        .message_session(&lead.id, &c.id, "Third steer")
        .await
        .expect_err("budget spent pauses the tree");
    assert_eq!(err.code, OrchCode::Paused);
    assert_eq!(turns_left(&s, &lead.id), Some(0));

    s.workbench.interrupt(&c.id).expect("interrupt");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A parent's steer never resets the child's person-only watch notice budget.
#[cfg(unix)]
#[tokio::test]
async fn a_parents_steer_does_not_reset_the_watch_notice_budget() {
    let s = setup_with("steer-watch-budget", Limits::default(), true, |config| Config {
        terminal: true,
        shell: nexus_workbench::process::Shell::sh(),
        ..config
    });
    let wb = &s.workbench;
    let lead = root(
        &s,
        NewThread {
            permission_mode: PermissionMode::BypassPermissions,
            ..new_root(&s.project, "Lead")
        },
    )
    .await;
    let other = root(
        &s,
        NewThread {
            orchestration: false,
            permission_mode: PermissionMode::BypassPermissions,
            ..new_root(&s.project, "Other")
        },
    )
    .await;
    let child = finished_child(&s, &lead.id, start_named(SessionAgent::Claude, "Fixer", "Ready")).await;

    // Spend all 10 watch notices on the child, then hit the spent budget once.
    for n in 1..=10 {
        run_watched(wb, &child.id, &format!("w-{n}"), &format!("printf 'w-{n}'")).await;
        wait_for(wb, &child.id, |e| {
            watch_notices(e).len() == n && turn_ends(e).len() == n + 1
        })
        .await;
    }
    run_watched(wb, &child.id, "w-11", "printf 'w-11'").await;
    wait_for(wb, &child.id, |e| budget_rows(e) == 1).await;

    // Start a running turn on the child (non-person) and steer it from its parent.
    wb.message_session(&lead.id, &child.id, "slow Work")
        .await
        .expect("start slow turn");
    wait_for(wb, &child.id, |e| {
        e.iter()
            .filter(|row| matches!(row.event, WbEvent::TurnStart { .. }))
            .count()
            == 12
            && texts(e).len() > 11
    })
    .await;
    wb.message_session(&lead.id, &child.id, "Name the command")
        .await
        .expect("steer running child");
    wait_for(wb, &child.id, |e| turn_ends(e).len() == 12).await;

    // The 12th watched exit is still dropped: the parent's steer did not reset the budget.
    let twelfth = run_watched(wb, &child.id, "w-12", "printf 'w-12'").await;
    wait_command_ended(wb, &child.id, &twelfth).await;
    marker(wb, &other.id, "settled-12").await;
    let events = wb.events(&child.id, 0).expect("events");
    assert_eq!(watch_notices(&events).len(), 10);
    assert_eq!(budget_rows(&events), 1);

    // A person message still resets it and delivers the next notice.
    wb.send_message(&child.id, "Carry on", vec![])
        .await
        .expect("person message");
    wait_for(wb, &child.id, |e| turn_ends(e).len() == 13).await;
    let after = marker(wb, &child.id, "w-13").await;
    assert_eq!(after.len(), 11);

    wb.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
