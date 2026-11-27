//! Cross-provider peer messages and subscriptions use two unrelated, explicitly enabled local sessions.
mod support;
use nexus_workbench::event::{PermissionMode, Sender, SessionAgent};
use nexus_workbench::{Limits, NewThread, OrchCode, ThreadPatch};
use support::*;

#[tokio::test]
async fn peers_exchange_messages_and_spend_both_automatic_turn_budgets() {
    let s = setup(
        "peer-messages",
        Limits {
            max_auto_turns: Some(2),
            ..bounded()
        },
    );
    let a = root(&s, new_root(&s.project, "Lead")).await;
    let b = root(
        &s,
        NewThread {
            agent: SessionAgent::Codex,
            ..new_root(&s.project, "Worker")
        },
    )
    .await;
    let off = root(
        &s,
        NewThread {
            orchestration: false,
            ..new_root(&s.project, "Private work")
        },
    )
    .await;
    let listed = s.workbench.list_sessions_all(&a.id).expect("list enabled sessions");
    assert!(listed.iter().any(|row| row.session_id == b.id));
    assert!(!listed.iter().any(|row| row.session_id == off.id));
    assert!(s.workbench.message_session(&a.id, &off.id, "Read this").await.is_err());
    for (sender, target) in [(&a, &b), (&b, &a)] {
        s.workbench
            .message_session(&sender.id, &target.id, "Review the summary.")
            .await
            .expect("send a peer message");
        let events = wait_for(&s.workbench, &target.id, |events| turn_ends(events).len() == 2).await;
        assert!(users(&events).contains(&(
            "Review the summary.".into(),
            Some(Sender::Session {
                thread_id: sender.id.clone(),
                title: sender.title.clone()
            })
        )));
    }
    assert_eq!(
        s.workbench
            .message_session(&a.id, &b.id, "Again")
            .await
            .expect_err("the automatic turn budget is exhausted")
            .code,
        OrchCode::Paused
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

#[tokio::test]
async fn a_subscription_delivers_only_the_next_result_once() {
    let s = setup("peer-subscribe", Limits::default());
    let watcher = root(&s, new_root(&s.project, "Prepare a report")).await;
    let worker = root(
        &s,
        NewThread {
            agent: SessionAgent::Codex,
            ..new_root(&s.project, "Earlier work")
        },
    )
    .await;
    s.workbench
        .subscribe_session(&watcher.id, &worker.id)
        .expect("subscribe to the worker");
    s.workbench
        .subscribe_session(&watcher.id, &worker.id)
        .expect("subscribe to the worker");
    assert_eq!(
        users(&s.workbench.events(&watcher.id, 0).expect("read watcher events")).len(),
        1
    );
    s.workbench
        .send(&worker.id, "First result")
        .await
        .expect("send a test turn");
    let events = wait_for(&s.workbench, &watcher.id, |events| turn_ends(events).len() == 2).await;
    let sent = users(&events);
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].0, "Message 2 of this thread.");
    assert_eq!(
        sent[1].1,
        Some(Sender::Session {
            thread_id: worker.id.clone(),
            title: worker.title.clone()
        })
    );
    s.workbench
        .send(&worker.id, "Later work")
        .await
        .expect("send a test turn");
    wait_for(&s.workbench, &worker.id, |events| turn_ends(events).len() == 3).await;
    assert_eq!(
        users(&s.workbench.events(&watcher.id, 0).expect("read watcher events")).len(),
        2
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

#[tokio::test]
async fn a_peer_cannot_wake_a_stopped_session_or_raise_its_permission_level() {
    let s = setup("peer-permissions", Limits::default());
    let a = root(&s, new_root(&s.project, "Peer")).await;
    let b = root(
        &s,
        NewThread {
            permission_mode: PermissionMode::BypassPermissions,
            ..new_root(&s.project, "Elevated")
        },
    )
    .await;
    assert!(s.workbench.message_session(&a.id, &b.id, "Run this").await.is_err());
    s.workbench
        .update(
            &b.id,
            ThreadPatch {
                permission_mode: Some(PermissionMode::AcceptEdits),
                ..Default::default()
            },
        )
        .expect("lower the worker permission level");
    s.workbench.stop(&a.id).expect("stop the source session");
    assert!(s.workbench.message_session(&b.id, &a.id, "Wake up").await.is_err());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

#[tokio::test]
async fn a_busy_subscriber_receives_the_result_it_requested_even_after_the_peer_moves_on() {
    let s = setup("peer-busy", Limits::default());
    let watcher = root(&s, new_root(&s.project, "Watch")).await;
    let worker = root(
        &s,
        NewThread {
            agent: SessionAgent::Codex,
            ..new_root(&s.project, "Work")
        },
    )
    .await;
    s.workbench.send(&watcher.id, "relay").await.expect("send a test turn");
    s.workbench
        .subscribe_session(&watcher.id, &worker.id)
        .expect("subscribe to the worker");
    s.workbench.send(&worker.id, "Second").await.expect("send a test turn");
    wait_for(&s.workbench, &worker.id, |events| turn_ends(events).len() == 2).await;
    s.workbench.send(&worker.id, "Third").await.expect("send a test turn");
    wait_for(&s.workbench, &worker.id, |events| turn_ends(events).len() == 3).await;
    end_relay(&s, &watcher.id).await;
    let events = wait_for(&s.workbench, &watcher.id, |events| turn_ends(events).len() >= 3).await;
    let peer_messages: Vec<_> = users(&events).into_iter().filter(|(_, from)| from.is_some()).collect();
    assert_eq!(peer_messages.len(), 1);
    assert_eq!(peer_messages[0].0, "Message 2 of this thread.");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}
