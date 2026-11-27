//! Starts sessions against the Cursor ACP stand-in.
#![cfg(feature = "cursor")]

mod support;

use nexus_workbench::NewThread;
use nexus_workbench::event::{Decision, PermissionMode, SessionAgent, Via, WbEvent};
use support::*;

fn cursor(s: &Setup, permission_mode: PermissionMode, text: &str) -> NewThread {
    NewThread {
        agent: SessionAgent::Cursor,
        via: Via::Subscription,
        provider: None,
        permission_mode,
        orchestration: false,
        ..new_root(&s.project, text)
    }
}

#[tokio::test]
async fn cursor_keeps_command_approval_with_the_person() {
    let s = setup("cursor-approval", Default::default());
    let session = s
        .workbench
        .create_thread(cursor(&s, PermissionMode::AcceptEdits, "run ls"))
        .await
        .expect("Cursor session");
    let ask = approval(&s.workbench, &session.id).await;
    s.workbench
        .approve(&session.id, &ask, Decision::Allow, None, None, None)
        .expect("the person answers Cursor's ask");
    wait_for(&s.workbench, &session.id, |events| turn_ends(events).len() == 1).await;
    assert!(s.host.memory.join("prompts").is_file());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn refused_cursor_plan_never_reaches_the_process() {
    let s = setup("cursor-plan-refused", Default::default());
    s.host
        .extra
        .lock()
        .expect("stand-in environment")
        .insert("FAKE_CURSOR_REFUSE_PLAN".into(), "1".into());
    let session = s
        .workbench
        .create_thread(cursor(&s, PermissionMode::Plan, "plan this"))
        .await
        .expect("the session can be created");
    wait_for(&s.workbench, &session.id, |events| {
        events.iter().any(
            |event| matches!(&event.event, WbEvent::Error { text, .. } if text.contains("Plan refused by stand-in")),
        )
    })
    .await;
    assert!(!s.host.memory.join("prompts").exists());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
