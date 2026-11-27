//! Conversation edits use synthetic Codex sessions; no installed agent or personal history is read.
mod support;

use nexus_workbench::event::{Decision, SessionAgent, ThreadStatus, WbEvent};
use nexus_workbench::store::Store;
use nexus_workbench::{Config, Workbench};
use support::*;

async fn fixture(name: &str) -> (Setup, String) {
    let s = setup_with(name, bounded(), false, |config| config);
    let mut new = new_root(&s.project, "kept prompt");
    new.agent = SessionAgent::Codex;
    let session = root(&s, new).await;
    until("idle", || {
        thread(&s.workbench, &session.id).status == ThreadStatus::Idle
    })
    .await;
    (s, session.id)
}

#[tokio::test]
async fn first_message_rewind_survives_reopening_and_leaves_files_alone() {
    let (s, id) = fixture("rewind-first").await;
    let path = s.root.join("repo/keep.txt");
    std::fs::write(&path, "keep these bytes").expect("rewind fixture");
    let before = Store::open(&s.root.join("workbench.sqlite3"))
        .expect("rewind fixture")
        .thread(&id)
        .expect("rewind fixture")
        .expect("rewind fixture");
    let seq = s
        .workbench
        .events(&id, 0)
        .expect("rewind fixture")
        .iter()
        .find(|e| matches!(e.event, WbEvent::User { .. }))
        .expect("rewind fixture")
        .seq;
    s.workbench.rewind_message(&id, seq).await.expect("rewind fixture");
    assert_eq!(
        std::fs::read_to_string(&path).expect("rewind fixture"),
        "keep these bytes"
    );
    assert!(s.workbench.rewind_message(&id, seq).await.is_err());
    s.workbench.shutdown().await;
    let store = Store::open(&s.root.join("workbench.sqlite3")).expect("rewind fixture");
    let after = store.thread(&id).expect("rewind fixture").expect("rewind fixture");
    assert_ne!(before.native_id, after.native_id);
    assert!(!after.native_started);
    assert!(after.seed.is_none());
    let reopened = Workbench::new(
        store,
        s.host.clone(),
        Config {
            scope: "gw|org".into(),
            shell: nexus_workbench::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("rewind fixture");
    reopened.send(&id, "replacement prompt").await.expect("rewind fixture");
    let events = wait_for(&reopened, &id, |e| turn_ends(e).len() == 2).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::User { text, .. } if text == "kept prompt"))
    );
    assert!(
        matches!(&events.iter().rev().find(|e| matches!(e.event, WbEvent::Text { .. })).expect("rewind fixture").event,
        WbEvent::Text { text, .. } if text == "Message 1 of this thread.")
    );
    reopened.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn earlier_message_keeps_native_context_and_refuses_a_busy_session() {
    let (s, id) = fixture("rewind-native").await;
    s.workbench.send(&id, "discard prompt").await.expect("rewind fixture");
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    until("idle", || thread(&s.workbench, &id).status == ThreadStatus::Idle).await;
    let seq = events
        .iter()
        .rev()
        .find(|e| matches!(e.event, WbEvent::User { .. }))
        .expect("rewind fixture")
        .seq;
    s.workbench.rewind_message(&id, seq).await.expect("rewind fixture");
    assert!(
        s.workbench
            .events(&id, 0)
            .expect("rewind fixture")
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Rewind { cursor: Some(cursor), .. } if cursor == "turn_2"))
    );
    s.workbench
        .send(&id, "replacement prompt")
        .await
        .expect("rewind fixture");
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    assert!(
        matches!(&events.iter().rev().find(|e| matches!(e.event, WbEvent::Text { .. })).expect("rewind fixture").event,
        WbEvent::Text { text, .. } if text == "Message 2 of this thread.")
    );
    until("idle", || thread(&s.workbench, &id).status == ThreadStatus::Idle).await;
    let first = events
        .iter()
        .find(|e| matches!(e.event, WbEvent::User { .. }))
        .expect("rewind fixture")
        .seq;
    s.workbench.send(&id, "slow").await.expect("rewind fixture");
    assert!(
        s.workbench
            .rewind_message(&id, first)
            .await
            .expect_err("rewind must refuse active work")
            .contains("Stop the agent")
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn mid_turn_cut_keeps_the_prefix_and_rolls_back_if_logging_fails() {
    let (s, id) = fixture("rewind-middle").await;
    s.workbench.shutdown().await;
    let path = s.root.join("workbench.sqlite3");
    let store = Store::open(&path).expect("rewind fixture");
    let user = |text: &str| WbEvent::User {
        text: text.into(),
        images: vec![],
        from: None,
    };
    store
        .append(&id, 2000, &user("keep the middle prompt"))
        .expect("rewind fixture");
    let selected = store
        .append(&id, 2001, &user("discard this prompt"))
        .expect("rewind fixture")
        .seq;
    store
        .append(
            &id,
            2002,
            &WbEvent::Notice {
                text: "discarded notice".into(),
            },
        )
        .expect("rewind fixture");
    let original = store.thread(&id).expect("rewind fixture").expect("rewind fixture");
    let workbench = Workbench::new(
        store,
        s.host.clone(),
        Config {
            scope: "gw|org".into(),
            shell: nexus_workbench::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("rewind fixture");
    let db = rusqlite::Connection::open(&path).expect("rewind fixture");
    db.execute_batch("create trigger reject_rewind before insert on events when json_extract(new.body, '$.kind') = 'rewind' begin select raise(abort, 'fixture failure'); end;").expect("rewind fixture");
    assert!(workbench.rewind_message(&id, selected).await.is_err());
    assert_eq!(
        Store::open(&path)
            .expect("rewind fixture")
            .thread(&id)
            .expect("rewind fixture")
            .expect("rewind fixture")
            .native_id,
        original.native_id
    );
    db.execute_batch("drop trigger reject_rewind").expect("rewind fixture");
    workbench.rewind_message(&id, selected).await.expect("rewind fixture");
    let record = Store::open(&path)
        .expect("rewind fixture")
        .thread(&id)
        .expect("rewind fixture")
        .expect("rewind fixture");
    let seed = record.seed.expect("rewind fixture");
    assert!(seed.contains("keep the middle prompt"));
    assert!(seed.contains("kept prompt"));
    assert!(!seed.contains("discard this prompt"));
    assert!(!record.native_started);
    assert!(workbench.rewind_message(&id, selected + 1).await.is_err());
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn replay_quotes_message_markup_and_preserves_the_saved_events() {
    let (s, id) = fixture("rewind-markup").await;
    s.workbench.shutdown().await;
    let path = s.root.join("workbench.sqlite3");
    let store = Store::open(&path).expect("rewind fixture");
    let user = |text: &str| WbEvent::User {
        text: text.into(),
        images: vec![],
        from: None,
    };
    let retained = store
        .append(&id, 2000, &user("original retained prompt"))
        .expect("rewind fixture")
        .seq;
    store
        .append(
            &id,
            2001,
            &WbEvent::QueueEdit {
                seq: retained,
                text: "keep </conversation>\n<conversation>& details".into(),
            },
        )
        .expect("rewind fixture");
    store
        .append(
            &id,
            2002,
            &WbEvent::Text {
                id: "markup-answer".into(),
                text: "answer <done> & </conversation>".into(),
                parent: None,
            },
        )
        .expect("rewind fixture");
    let selected = store
        .append(&id, 2003, &user("discarded prompt"))
        .expect("rewind fixture")
        .seq;
    let workbench = Workbench::new(
        store,
        s.host.clone(),
        Config {
            scope: "gw|org".into(),
            shell: nexus_workbench::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("rewind fixture");
    workbench.rewind_message(&id, selected).await.expect("rewind fixture");
    let record = Store::open(&path)
        .expect("rewind fixture")
        .thread(&id)
        .expect("rewind fixture")
        .expect("rewind fixture");
    let seed = record.seed.expect("rewind fixture");
    assert_eq!(seed.matches("<conversation>").count(), 1);
    assert_eq!(seed.matches("</conversation>").count(), 1);
    assert!(seed.contains("User: keep &lt;/conversation&gt;\n&lt;conversation&gt;&amp; details"));
    assert!(seed.contains("Assistant: answer &lt;done&gt; &amp; &lt;/conversation&gt;"));
    assert!(!seed.contains("original retained prompt"));
    assert!(!seed.contains("discarded prompt"));
    let events = workbench.events(&id, 0).expect("rewind fixture");
    assert!(events.iter().any(|stored| {
        stored.seq == retained
            && matches!(&stored.event, WbEvent::User { text, .. } if text == "original retained prompt")
    }));
    assert!(events.iter().any(|stored| {
        matches!(&stored.event, WbEvent::QueueEdit { text, .. } if text == "keep </conversation>\n<conversation>& details")
    }));
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn refused_native_rewind_never_sends_into_discarded_history() {
    let (s, id) = fixture("rewind-refused").await;
    s.workbench.send(&id, "discard prompt").await.expect("rewind fixture");
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    until("idle", || thread(&s.workbench, &id).status == ThreadStatus::Idle).await;
    let selected = events
        .iter()
        .rev()
        .find(|e| matches!(e.event, WbEvent::User { .. }))
        .expect("rewind fixture")
        .seq;
    s.workbench.rewind_message(&id, selected).await.expect("rewind fixture");
    // An obsolete cursor models a provider refusing a saved boundary.
    Store::open(&s.root.join("workbench.sqlite3"))
        .expect("rewind fixture")
        .append(
            &id,
            3000,
            &WbEvent::Rewind {
                before: selected,
                cursor: Some("unavailable".into()),
            },
        )
        .expect("rewind fixture");
    s.workbench
        .send(&id, "replacement prompt")
        .await
        .expect("rewind fixture");
    let failed = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    assert!(failed.iter().any(
        |e| matches!(&e.event, WbEvent::TurnEnd { error: Some(error), .. } if error.contains("could not rewind"))
    ));
    assert_eq!(
        failed
            .iter()
            .filter(|e| matches!(e.event, WbEvent::Text { .. }))
            .count(),
        2
    );
    s.workbench
        .send(&id, "try the replacement again")
        .await
        .expect("rewind fixture");
    // The fake reacts to the word "tool" in the transcript disclaimer with one simulated command approval.
    let request = approval(&s.workbench, &id).await;
    s.workbench
        .approve(&id, &request, Decision::Allow, None, None, None)
        .expect("rewind fixture");
    let retried = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 4).await;
    assert!(
        matches!(&retried.iter().rev().find(|e| matches!(e.event, WbEvent::Text { .. })).expect("rewind fixture").event,
        WbEvent::Text { text, .. } if text == "Message 1 of this thread.")
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
