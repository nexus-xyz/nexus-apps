use super::*;
use nexus_workbench::event::ThreadSummary;
use nexus_workbench::store::ThreadRecord;
use serde_json::json;

const SCOPE: &str = "https://synthetic.invalid|audit";

fn record(id: &str, project: &str, scope: &str, updated: i64) -> ThreadRecord {
    let summary: ThreadSummary = serde_json::from_value(json!({
        "id": id, "projectId": project, "title": format!("Synthetic session {id}"),
        "agent": "codex", "model": null, "permissionMode": "default", "via": "nexus", "status": "idle",
        "costUsd": 0.0, "nexusSessions": ["synthetic-run"], "contextTokens": null, "contextWindow": null,
        "createdAt": 1, "updatedAt": updated, "archived": false
    }))
    .unwrap();
    ThreadRecord {
        summary,
        native_id: format!("synthetic-{id}"),
        native_started: false,
        scope: scope.into(),
        fork_from: None,
        seed: None,
        automatic_title: false,
        delivered_seq: 0,
        halt: None,
    }
}

// The pre-audit overview, retained only for output equivalence and the optional benchmark.
fn legacy_overview(reader: &CodeReader) -> (Result<Vec<CodeProject>, String>, Result<Vec<CodeSession>, String>) {
    let projects = (|| {
        let threads = reader
            .store()
            .threads(reader.scope.as_str())
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|record| live(record.summary));
        let projects = reader.store().projects().map_err(|error| error.to_string())?;
        Ok(group(projects, threads, None))
    })();
    let archived = (|| {
        let mut sessions: Vec<_> = reader
            .store()
            .threads(reader.scope.as_str())
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|record| live(record.summary))
            .filter(|thread| thread.archived && !thread.side_chat)
            .map(CodeSession::of)
            .collect();
        sessions.sort_by(list_order);
        Ok(sessions)
    })();
    (projects, archived)
}

#[test]
fn one_overview_preserves_scope_archive_side_chat_and_list_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::in_memory().unwrap();
    let project = store.add_project(dir.path(), 1).unwrap();
    let mut old = record("old", &project.id, SCOPE, 20);
    old.summary.listed_at = Some(10);
    let mut pinned = record("pinned", &project.id, SCOPE, 1);
    pinned.summary.pinned = true;
    let mut archived = record("archived", &project.id, SCOPE, 50);
    archived.summary.archived = true;
    let mut older_archive = record("older-archive", &project.id, SCOPE, 30);
    older_archive.summary.archived = true;
    let mut side_chat = record("side-chat", &project.id, SCOPE, 100);
    side_chat.summary.side_chat = true;
    let mut archived_side_chat = record("archived-side-chat", &project.id, SCOPE, 100);
    archived_side_chat.summary.archived = true;
    archived_side_chat.summary.side_chat = true;
    for thread in [
        old,
        pinned,
        record("new", &project.id, SCOPE, 15),
        archived,
        older_archive,
        side_chat,
        archived_side_chat,
        record("other-org", &project.id, "https://synthetic.invalid|another-org", 200),
    ] {
        store.insert_thread(&thread).unwrap();
    }
    let reader = CodeReader {
        include_local: false,
        reader: store.reader(),
        scope: SCOPE.into(),
    };
    let result = reader.overview();
    assert_eq!(result, legacy_overview(&reader));
    let (projects, archived) = result;
    let projects = projects.unwrap();
    assert_eq!(
        projects[0]
            .sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        ["pinned", "new", "old"]
    );
    assert_eq!(
        archived
            .unwrap()
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        ["archived", "older-archive"]
    );
    store.set_project_state(&project.id, None, Some(true)).unwrap();
    assert_eq!(reader.overview(), legacy_overview(&reader));
    assert!(reader.overview().0.unwrap().is_empty());
}

#[test]
fn an_overview_refresh_moves_a_session_between_active_and_archived() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::in_memory().unwrap();
    let project = store.add_project(dir.path(), 1).unwrap();
    let mut session = record("session", &project.id, SCOPE, 1);
    store.insert_thread(&session).unwrap();
    let reader = CodeReader {
        include_local: false,
        reader: store.reader(),
        scope: SCOPE.into(),
    };
    let (projects, archived) = reader.overview();
    assert_eq!(projects.unwrap()[0].sessions.len(), 1);
    assert!(archived.unwrap().is_empty());
    session.summary.archived = true;
    store.save_thread(&session).unwrap();
    let (projects, archived) = reader.overview();
    assert!(projects.unwrap()[0].sessions.is_empty());
    assert_eq!(archived.unwrap()[0].id, "session");
    session.summary.archived = false;
    store.save_thread(&session).unwrap();
    let (projects, archived) = reader.overview();
    assert_eq!(projects.unwrap()[0].sessions[0].id, "session");
    assert!(archived.unwrap().is_empty());
}

#[test]
fn an_overview_keeps_live_background_work_for_active_and_archived_sessions() {
    use nexus_workbench::event::{BackgroundKind, BackgroundWork};

    let dir = tempfile::tempdir().unwrap();
    let store = Store::in_memory().unwrap();
    let project = store.add_project(dir.path(), 1).unwrap();
    let id = format!("overview-live-{}", uuid::Uuid::new_v4());
    for archived in [false, true] {
        let mut thread = record(&format!("{id}-{archived}"), &project.id, SCOPE, 1);
        thread.summary.archived = archived;
        store.insert_thread(&thread).unwrap();
        thread.summary.background = vec![BackgroundWork {
            kind: BackgroundKind::Shell,
            title: "Synthetic background work".into(),
            started_at: Some(1),
        }];
        crate::live_fields::noted(&thread.summary);
    }
    let reader = CodeReader {
        include_local: false,
        reader: store.reader(),
        scope: SCOPE.into(),
    };
    let (projects, archived) = reader.overview();
    assert_eq!(projects.unwrap()[0].sessions[0].background_work.len(), 1);
    assert_eq!(archived.unwrap()[0].background_work.len(), 1);
}

#[test]
fn an_unreadable_thread_snapshot_fails_both_overview_parts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.sqlite3");
    let store = Store::open(&path).unwrap();
    let reader = CodeReader {
        include_local: false,
        reader: store.reader(),
        scope: SCOPE.into(),
    };
    drop(store);
    std::fs::remove_file(path).unwrap();
    let (projects, archived) = reader.overview();
    assert!(projects.is_err());
    assert_eq!(projects.unwrap_err(), archived.unwrap_err());
}

/// Run serially, without app benchmarks or builds in parallel:
/// cargo test --release --locked code::overview_tests::benchmark_overview -- --ignored --exact --nocapture
#[test]
#[ignore = "manual release benchmark on synthetic data"]
fn benchmark_overview() {
    use std::hint::black_box;
    use std::time::Instant;

    assert!(!cfg!(debug_assertions), "Use a release build to measure performance");
    for count in [100, 1_000, 5_000] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_memory().unwrap();
        let project = store.add_project(dir.path(), 1).unwrap();
        for index in 0..count {
            let mut thread = record(&format!("s-{index}"), &project.id, SCOPE, index as i64);
            thread.summary.archived = index % 4 == 0;
            thread.summary.side_chat = index % 20 == 0;
            store.insert_thread(&thread).unwrap();
        }
        let reader = CodeReader {
            include_local: false,
            reader: store.reader(),
            scope: SCOPE.into(),
        };
        assert_eq!(reader.overview(), legacy_overview(&reader));
        // Warm both paths and alternate their order to reduce cache and order effects.
        for sample in 0..12 {
            for legacy in if sample % 2 == 0 { [true, false] } else { [false, true] } {
                let start = Instant::now();
                let output = if legacy {
                    legacy_overview(&reader)
                } else {
                    reader.overview()
                };
                let elapsed = start.elapsed();
                let _ = black_box(output);
                if sample >= 2 {
                    println!(
                        "{{\"benchmark\":\"code-overview\",\"sessions\":{count},\"legacy\":{legacy},\"sample\":{},\"elapsed_ms\":{:.6}}}",
                        sample - 2,
                        elapsed.as_secs_f64() * 1000.0
                    );
                }
            }
        }
    }
}

#[test]
fn signed_in_overview_keeps_the_local_ledger_beside_its_organization() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::in_memory().unwrap();
    let project = store.add_project(dir.path(), 1).unwrap();
    let local = nexus_workbench::session_scope::SessionScope::local_text();
    for (id, scope) in [
        ("t_local", local),
        ("t_current_org", SCOPE),
        ("t_other_org", "other|org"),
    ] {
        store.insert_thread(&record(id, &project.id, scope, 20)).unwrap();
    }
    let reader = CodeReader {
        reader: store.reader(),
        scope: SCOPE.into(),
        include_local: true,
    };
    let sessions = reader.overview().0.unwrap().remove(0).sessions;
    assert_eq!(sessions.len(), 2);
    assert!(sessions.iter().any(|session| session.id == "t_local"));
    assert!(sessions.iter().any(|session| session.id == "t_current_org"));
    assert_eq!(reader.connection_read("t_local").unwrap().0.unwrap().id, "t_local");
    assert!(reader.connection_read("t_other_org").unwrap().0.is_none());
}
