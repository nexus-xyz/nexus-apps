//! Sessions of Nexus's own agent (`nexus __agent`), run by the stand-in `fake-nexus`: its tools show as Claude Code
//! names them, its approvals go through the workbench's, and the permission mode sets what runs without asking.

mod support;

use nexus_workbench::event::{Decision, PermissionMode, SessionAgent, ThreadStatus, WbEvent};
use support::*;

fn tools(events: &[nexus_workbench::event::StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Tool { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn says(events: &[nexus_workbench::event::StoredEvent], wanted: &str) -> bool {
    events
        .iter()
        .any(|e| matches!(&e.event, WbEvent::Text { text, .. } if text.contains(wanted)))
}

#[tokio::test]
async fn a_nexus_session_asks_runs_and_answers() {
    let s = setup("nexus-asks", Default::default());
    let new = nexus_workbench::NewThread {
        name: None,
        agent: SessionAgent::Nexus,
        permission_mode: PermissionMode::Default,
        ..new_root(&s.project, "please edit and run a tool")
    };
    let created = s.workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));

    let edit = approval(&s.workbench, &created.id).await;
    s.workbench
        .approve(&created.id, &edit, Decision::Allow, None, None, None)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &created.id, |e| {
        e.iter().filter(|e| matches!(e.event, WbEvent::Approval { .. })).count() == 2
    })
    .await;
    let command = events
        .iter()
        .rev()
        .find_map(|e| match &e.event {
            WbEvent::Approval { request_id, tool, .. } if tool == "Bash" => Some(request_id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the command's approval: {events:#?}"));
    s.workbench
        .approve(&created.id, &command, Decision::Allow, None, None, None)
        .unwrap_or_else(|e| panic!("{e}"));

    let events = wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(tools(&events), ["Write", "Bash"], "{events:#?}");
    assert!(says(&events, "You said: please edit and run a tool"));
    let launch = launches_of(&s, &created.id).pop().unwrap_or_else(|| panic!("a launch"));
    assert_eq!(launch.agent, SessionAgent::Nexus);
    assert_eq!(launch.args[..2], ["__agent", "--session"]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn accept_edits_runs_edits_without_asking() {
    let s = setup("nexus-accept-edits", Default::default());
    let new = nexus_workbench::NewThread {
        name: None,
        agent: SessionAgent::Nexus,
        permission_mode: PermissionMode::AcceptEdits,
        ..new_root(&s.project, "please edit")
    };
    let created = s.workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    assert!(
        !events.iter().any(|e| matches!(e.event, WbEvent::Approval { .. })),
        "{events:#?}"
    );
    assert_eq!(tools(&events), ["Write"]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Nexus's own agent is not a coding agent: a switch to Auto or Full access answers the ask it waits on, and its
/// command runs.
#[tokio::test]
async fn switch_to_auto_or_full_access_answers_nexus_agents_open_ask() {
    for mode in [PermissionMode::Auto, PermissionMode::BypassPermissions] {
        let s = setup(&format!("nexus-unasked-{}", mode.as_str()), Default::default());
        let new = nexus_workbench::NewThread {
            name: None,
            agent: SessionAgent::Nexus,
            permission_mode: PermissionMode::Default,
            ..new_root(&s.project, "please run a tool")
        };
        let created = s.workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
        let request = approval(&s.workbench, &created.id).await;
        s.workbench
            .update(&created.id, support::mode(mode))
            .unwrap_or_else(|e| panic!("{e}"));
        let events = wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
        assert!(
            events.iter().any(|e| matches!(&e.event,
                WbEvent::ApprovalResolved { request_id, decision: Decision::Allow, .. } if *request_id == request)),
            "{mode:?}: {events:#?}"
        );
        assert_eq!(tools(&events), ["Bash"], "{mode:?}");
        s.workbench.shutdown().await;
        let _ = std::fs::remove_dir_all(&s.root);
    }
}

/// In Auto and Full access the workbench answers Nexus's own agent's ask at once. No request shows, and the command
/// runs.
#[tokio::test]
async fn auto_and_full_access_answer_nexus_agents_ask_at_once() {
    for mode in [PermissionMode::Auto, PermissionMode::BypassPermissions] {
        let s = setup(&format!("nexus-at-once-{}", mode.as_str()), Default::default());
        s.host
            .extra
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("FAKE_NEXUS_ASK".into(), "1".into());
        let new = nexus_workbench::NewThread {
            name: None,
            agent: SessionAgent::Nexus,
            permission_mode: mode,
            ..new_root(&s.project, "please run a tool")
        };
        let created = s.workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
        let events = wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.event, WbEvent::Approval { .. } | WbEvent::ApprovalResolved { .. })),
            "{mode:?}: {events:#?}"
        );
        assert_eq!(tools(&events), ["Bash"], "{mode:?}");
        s.workbench.shutdown().await;
        let _ = std::fs::remove_dir_all(&s.root);
    }
}

/// The agent's answers in a session, oldest first.
fn answers(events: &[nexus_workbench::event::StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

async fn nexus_session(s: &Setup, text: &str) -> String {
    let new = nexus_workbench::NewThread {
        name: None,
        agent: SessionAgent::Nexus,
        permission_mode: PermissionMode::AcceptEdits,
        ..new_root(&s.project, text)
    };
    let created = s.workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    // A fork goes on from the latest recorded turn: until its checkpoint is written, that is the folder before it.
    for _ in 0..400 {
        if !s
            .workbench
            .changed_turns(&created.id)
            .await
            .unwrap_or_default()
            .is_empty()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    created.id
}

#[tokio::test]
async fn a_fork_and_a_side_chat_go_on_from_the_conversation_and_leave_it_as_it_was() {
    let s = setup("nexus-fork", Default::default());
    let id = nexus_session(&s, "first").await;

    let fork = s
        .workbench
        .fork_thread(&id, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .send(&fork.id, "second")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    // A fork's log starts with its source's turn.
    let events = wait_for(&s.workbench, &fork.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(
        answers(&events).last().map(String::as_str),
        Some("You said: second (remembers: first)")
    );
    let launch = launches_of(&s, &fork.id)
        .pop()
        .unwrap_or_else(|| panic!("the fork's launch"));
    assert!(launch.args.contains(&"--fork".to_string()), "{:?}", launch.args);

    let side = s.workbench.side_chat(&id).await.unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .send(&side.id, "why?")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &side.id, |e| turn_ends(e).len() == 2).await;
    assert!(
        answers(&events)
            .last()
            .is_some_and(|text| text.ends_with("(remembers: first)")),
        "{events:#?}"
    );

    // The source never heard the fork's or the side chat's messages.
    s.workbench.send(&id, "third").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(
        answers(&events).last().map(String::as_str),
        Some("You said: third (remembers: first)")
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_message_edit_rewinds_the_conversation() {
    let s = setup("nexus-rewind", Default::default());
    let id = nexus_session(&s, "one").await;
    s.workbench.send(&id, "two").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    until("idle", || thread(&s.workbench, &id).status == ThreadStatus::Idle).await;
    let second = events
        .iter()
        .rev()
        .find(|e| matches!(e.event, WbEvent::User { .. }))
        .unwrap_or_else(|| panic!("the second message"))
        .seq;
    s.workbench
        .rewind_message(&id, second)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = s.workbench.events(&id, 0).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Rewind { cursor: Some(cursor), .. } if cursor == "2")),
        "the cut is after the first turn's two messages: {events:#?}"
    );

    s.workbench.send(&id, "three").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    assert_eq!(
        answers(&events).last().map(String::as_str),
        Some("You said: three (remembers: one)")
    );
    let launch = launches_of(&s, &id).pop().unwrap_or_else(|| panic!("a launch"));
    assert!(
        launch.args.ends_with(&["--rewind".to_string(), "2".to_string()]),
        "{:?}",
        launch.args
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_subagent_shows_as_an_agent_call_with_its_tools_and_task() {
    let s = setup("nexus-subagent", Default::default());
    let id = nexus_session(&s, "delegate the reading").await;
    let events = s.workbench.events(&id, 0).unwrap_or_else(|e| panic!("{e}"));
    let agent = events
        .iter()
        .find_map(|e| match &e.event {
            WbEvent::Tool {
                id, name, parent: None, ..
            } if name == "Agent" => Some(id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("an Agent call: {events:#?}"));
    assert!(
        events.iter().any(
            |e| matches!(&e.event, WbEvent::Tool { name, parent: Some(parent), .. } if name == "Read" && *parent == agent)
        ),
        "the subagent's read is under its call: {events:#?}"
    );
    assert!(events.iter().any(|e| matches!(
        &e.event,
        WbEvent::Task {
            status: nexus_workbench::event::TaskStatus::Completed,
            tool_count: Some(1),
            ..
        }
    )));
    assert!(events.iter().any(
        |e| matches!(&e.event, WbEvent::ToolResult { id, output, .. } if *id == agent && output == "The notes say hello.")
    ));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
