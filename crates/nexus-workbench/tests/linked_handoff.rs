//! A session hands its work to a new one and archives itself, and a session archives one it started or itself
//! (linked/handoff.rs), with the stand-in `claude` and `codex`: the new session takes the old one's
//! worktree, place, children and name, starts empty with the handoff from the old session, and a chain of handoffs
//! stops at the start limit and at a halt.

mod support;

use std::path::PathBuf;

use nexus_workbench::event::{HandoffFrom, Sender, SessionAgent, TurnStatus, Via};
use nexus_workbench::{HandOff, Limits, NewThread, NewWorktree, OrchCode, ThreadPatch};
use support::*;

fn hand_off(agent: SessionAgent, prompt: &str) -> HandOff {
    HandOff {
        agent,
        prompt: prompt.into(),
        title: None,
        model: None,
        effort: None,
    }
}

#[tokio::test]
async fn a_claude_session_hands_off_to_codex_which_takes_its_worktree_place_and_name_and_starts_empty() {
    let s = setup("handoff-codex", Limits::default());
    let old = root(
        &s,
        NewThread {
            name: Some("fixer".into()),
            via: Via::Subscription,
            worktree: Some(NewWorktree::default()),
            ..new_root(&s.project, "Fix the login loop")
        },
    )
    .await;
    let worktree = old.worktree.clone().unwrap_or_else(|| panic!("a worktree"));
    let next = s
        .workbench
        .hand_off(
            &old.id,
            hand_off(SessionAgent::Codex, "Continue the login fix: the test still fails."),
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((next.agent, next.via), (SessionAgent::Codex, Via::Subscription));
    assert_eq!(next.account, None, "never the old session's Claude account");
    assert_eq!(next.title, old.title);
    assert_eq!(next.name.as_deref(), Some("fixer"), "the name moves with the work");
    assert_eq!(next.worktree.as_ref().map(|w| &w.path), Some(&worktree.path));
    assert_eq!(
        next.handoff_from,
        Some(HandoffFrom {
            thread_id: old.id.clone(),
            title: old.title.clone(),
            agent: SessionAgent::Claude,
        })
    );
    let before = thread(&s.workbench, &old.id);
    assert!(before.archived);
    assert_eq!(
        (before.worktree, before.name),
        (None, None),
        "the old row let go of both"
    );

    let events = wait_for(&s.workbench, &next.id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed]);
    // Empty but for the handoff, from the old session: the conversation is not sent again.
    assert_eq!(
        users(&events),
        [(
            "Continue the login fix: the test still fails.".to_string(),
            Some(Sender::Session {
                thread_id: old.id.clone(),
                title: old.title.clone(),
            })
        )]
    );
    let launch = launches_of(&s, &next.id).pop().unwrap_or_else(|| panic!("a launch"));
    assert_eq!(launch.agent, SessionAgent::Codex);
    assert_eq!(launch.cwd, PathBuf::from(&worktree.path), "it works in the same folder");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn codex_hands_back_to_claude() {
    let s = setup("handoff-claude", Limits::default());
    let old = root(
        &s,
        NewThread {
            agent: SessionAgent::Codex,
            ..new_root(&s.project, "Write the notes")
        },
    )
    .await;
    let next = s
        .workbench
        .hand_off(&old.id, hand_off(SessionAgent::Claude, "Finish the notes."))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(next.agent, SessionAgent::Claude);
    let events = wait_for(&s.workbench, &next.id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed]);
    assert!(thread(&s.workbench, &old.id).archived);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_worker_that_hands_off_keeps_reporting_to_its_lead_and_a_lead_passes_on_its_workers() {
    // At the default depth a worker gets no tools at all; one level more gives it its own.
    let s = setup(
        "handoff-place",
        Limits {
            max_depth: Some(2),
            ..bounded()
        },
    );
    let lead = root(&s, new_root(&s.project, "Lead the release")).await;
    let worker = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    let next = s
        .workbench
        .hand_off(&worker.id, hand_off(SessionAgent::Codex, "Finish the build."))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(next.parent_thread_id.as_deref(), Some(lead.id.as_str()));
    assert_eq!(next.depth, worker.depth);
    wait_for(&s.workbench, &next.id, |e| turn_ends(e).len() == 1).await;
    // The lead consumes an idle worker's result automatically. Its transcript keeps the receipt after delivery.
    let marker = format!("session=\"{}\"", next.id);
    wait_for(&s.workbench, &lead.id, |events| {
        users(events)
            .iter()
            .any(|(text, from)| from.is_some() && text.contains(&marker) && text.contains("status=\"completed\""))
    })
    .await;

    // A lead that hands off passes its workers on.
    let new_lead = s
        .workbench
        .hand_off(&lead.id, hand_off(SessionAgent::Codex, "Lead the rest."))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        thread(&s.workbench, &next.id).parent_thread_id.as_deref(),
        Some(new_lead.id.as_str())
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_chain_of_handoffs_stops_at_the_start_limit_until_the_person_writes() {
    let s = setup(
        "handoff-chain",
        Limits {
            max_starts: Some(2),
            ..bounded()
        },
    );
    let mut current = root(&s, new_root(&s.project, "Ping")).await;
    for agent in [SessionAgent::Codex, SessionAgent::Claude] {
        let next = s
            .workbench
            .hand_off(&current.id, hand_off(agent, "Hand it on."))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        wait_for(&s.workbench, &next.id, |e| turn_ends(e).len() == 1).await;
        current = thread(&s.workbench, &next.id);
    }
    // The tree's counters moved with each handoff: the third one is past the limit.
    assert_eq!(
        code(
            s.workbench
                .hand_off(&current.id, hand_off(SessionAgent::Codex, "Again."))
                .await
        ),
        OrchCode::StartsLimit
    );
    assert!(
        !thread(&s.workbench, &current.id).archived,
        "a refused handoff archives nothing"
    );
    // A person message to the root resets the counters.
    s.workbench
        .send(&current.id, "Keep going")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    answered(&s.workbench, &current.id, "Keep going").await;
    s.workbench
        .hand_off(&current.id, hand_off(SessionAgent::Codex, "Again."))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_stopped_session_hands_off_nothing_and_one_with_orchestration_off_neither() {
    let s = setup("handoff-halt", Limits::default());
    let lead = root(&s, new_root(&s.project, "Lead")).await;
    let worker = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Work")).await;
    // The person's Stop on the lead halts the worker below it: the kill switch holds handoffs too.
    s.workbench.stop(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        code(
            s.workbench
                .hand_off(&worker.id, hand_off(SessionAgent::Codex, "Run away."))
                .await
        ),
        OrchCode::Halted
    );
    assert!(!thread(&s.workbench, &worker.id).archived);
    let plain = root(
        &s,
        NewThread {
            orchestration: false,
            ..new_root(&s.project, "Just me")
        },
    )
    .await;
    assert_eq!(
        code(
            s.workbench
                .hand_off(&plain.id, hand_off(SessionAgent::Codex, "x"))
                .await
        ),
        OrchCode::Off
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_session_archives_itself_or_one_it_started_and_no_other() {
    let s = setup("archive", Limits::default());
    let lead = root(&s, new_root(&s.project, "Lead")).await;
    let worker = finished_child(&s, &lead.id, child(SessionAgent::Codex, "Work")).await;
    let other = root(&s, new_root(&s.project, "Someone else's")).await;
    let refused = s.workbench.archive_session(&lead.id, Some(&other.id));
    assert_eq!(refused.err().map(|e| e.code), Some(OrchCode::NotYourSession));
    s.workbench
        .archive_session(&lead.id, Some(&worker.id))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(thread(&s.workbench, &worker.id).archived);
    s.workbench
        .archive_session(&lead.id, None)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(thread(&s.workbench, &lead.id).archived);
    assert!(!thread(&s.workbench, &other.id).archived);
    // The person restores it as any archived session.
    s.workbench
        .update(
            &lead.id,
            ThreadPatch {
                archived: Some(false),
                ..ThreadPatch::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!thread(&s.workbench, &lead.id).archived);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn concurrent_handoffs_leave_one_successor_and_one_checkout_owner() {
    let s = setup("handoff-race", Limits::default());
    let old = root(
        &s,
        NewThread {
            worktree: Some(NewWorktree::default()),
            ..new_root(&s.project, "Own the checkout")
        },
    )
    .await;
    let (a, b) = tokio::join!(
        s.workbench
            .hand_off(&old.id, hand_off(SessionAgent::Claude, "Continue.")),
        s.workbench
            .hand_off(&old.id, hand_off(SessionAgent::Codex, "Continue."))
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let next = a.or(b).unwrap_or_else(|error| panic!("{error}"));
    wait_for(&s.workbench, &next.id, |events| {
        turn_ends(events) == [TurnStatus::Completed]
    })
    .await;
    let rows = s.workbench.threads().unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(rows.iter().filter(|row| row.worktree.is_some()).count(), 1);
    assert!(
        s.workbench
            .start_child(&old.id, child(SessionAgent::Codex, "Too late"))
            .await
            .is_err()
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
