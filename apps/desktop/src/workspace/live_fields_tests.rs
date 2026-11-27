//! A local session's row, read from the store as the sidebar reads it, shows the work its agent left running: the
//! workbench's `wb:thread` push carries it, and the store never holds it (`live_fields.rs`).

use nexus_workbench::attention;
use nexus_workbench::event::{BackgroundKind, BackgroundWork, ThreadSummary};
use nexus_workbench::store::{Store, ThreadRecord};

use super::seen::status_of;
use super::session_peek_background::lines;
use super::sidebar::state_text;
use crate::code::{CodeSession, CodeStore, Status};

const SCOPE: &str = "gateway|live-fields-org";

/// A store on disk with one idle session, its id and its summary as stored.
fn store_with_a_session(name: &str) -> (std::path::PathBuf, ThreadSummary) {
    let data = std::env::temp_dir().join(format!("nexus-gpui-live-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).expect("a temporary folder");
    let store = Store::open(&data.join("workbench.sqlite3")).expect("the store opens");
    let project = store.add_project(&data, 1).expect("a project");
    let id = format!("live-{name}");
    let summary: ThreadSummary = serde_json::from_value(serde_json::json!({
        "id": id, "projectId": project.id, "title": "Run the tests", "agent": "claude", "model": null,
        "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0, "nexusSessions": [],
        "contextTokens": null, "contextWindow": null, "createdAt": 1, "updatedAt": 1, "archived": false,
    }))
    .expect("a summary");
    store
        .insert_thread(&ThreadRecord {
            summary: summary.clone(),
            native_id: format!("native-{id}"),
            native_started: false,
            scope: SCOPE.into(),
            fork_from: None,
            seed: None,
            automatic_title: false,
            delivered_seq: 0,
            halt: None,
        })
        .expect("saved");
    (data, summary)
}

/// The session's row as the sidebar reads it, and its mark: `Workspace::session_mark` for a row it has seen.
fn row(data: &std::path::Path, id: &str) -> (CodeSession, Status) {
    let store = CodeStore::open_in(data, SCOPE)
        .expect("the store reads")
        .expect("a store");
    let session = store
        .projects()
        .expect("the projects")
        .into_iter()
        .flat_map(|project| project.sessions)
        .find(|session| session.id == id)
        .expect("the session's row");
    let one = CodeSession::of(store.summary(id).expect("one row").expect("in scope"));
    assert_eq!(
        one.background_count(),
        session.background_count(),
        "one row reads as the list does"
    );
    let mark = attention::mark_of(Some(session.activity.status), None);
    let status = status_of(attention::with_background(mark, None, session.background_busy));
    (session, status)
}

#[test]
fn a_pushed_background_command_shows_on_the_stored_row_until_an_empty_push() {
    let (data, stored) = store_with_a_session("command");
    let mut pushed = stored.clone();
    pushed.background = vec![BackgroundWork {
        kind: BackgroundKind::Shell,
        title: "sleep 60".into(),
        started_at: Some(1),
    }];
    crate::code_run::push_thread_for_test(pushed);

    let (session, status) = row(&data, &stored.id);
    assert_eq!(status, Status::Background);
    assert_eq!(
        state_text(status, session.background_count()).as_deref(),
        Some("In background · 1")
    );
    let (card, more) = lines(&session, 2, |_| None);
    assert_eq!(
        card.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(),
        ["sleep 60"]
    );
    assert_eq!(more, 0);

    // The command ended: the workbench pushes the session with nothing left running.
    crate::code_run::push_thread_for_test(stored.clone());
    let (session, status) = row(&data, &stored.id);
    assert_eq!(status, Status::Idle);
    assert_eq!(state_text(status, session.background_count()), None);
    let _ = std::fs::remove_dir_all(data);
}
