//! Actual disk lists refreshed only by completion of the retained original scoped session.
use super::*;
use nexus_workbench::event::ThreadStatus;
use std::{sync::atomic::Ordering, time::Duration};

fn status(source: &SourceStore, session: &str, status: ThreadStatus) {
    let store = Store::open(&source.data.join("workbench.sqlite3")).expect("original source writer");
    let mut thread = store
        .thread(session)
        .expect("actual row read")
        .expect("fixture session");
    thread.summary.status = status;
    thread.summary.updated_at += 1;
    store
        .save_thread(&thread)
        .expect("actual original-session status transition");
}
fn tick(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
}
fn paths(files: &Entity<Files>, cx: &mut VisualTestContext) -> Vec<String> {
    files.read_with(cx, |files, _| {
        files.fixture_paths().expect("actual file listing settled").to_vec()
    })
}

#[gpui_kit::test]
fn detached_original_completion_refreshes_real_added_deleted_files_after_source_window_closes(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let foreign = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc);
    let (_, _, files) = detach(&workspace, cx);
    assert!(paths(&files, cx).iter().any(|path| path == "hello.rs"));
    workspace.update_in(cx, |_, window, _| window.remove_window());
    drop(workspace);
    cx.run_until_parked();

    std::fs::remove_file(source.folder.join("hello.rs")).expect("real deleted file");
    std::fs::write(source.folder.join("created.rs"), "actual agent output\n").expect("real added bytes");
    // Neither another original-store session nor the same id in another store may drive this owner's edge.
    status(&source, OTHER, ThreadStatus::Running);
    status(&foreign, THREAD, ThreadStatus::Running);
    tick(cx);
    status(&source, OTHER, ThreadStatus::Idle);
    status(&foreign, THREAD, ThreadStatus::Idle);
    tick(cx);
    assert!(paths(&files, cx).iter().any(|path| path == "hello.rs"));
    assert!(!paths(&files, cx).iter().any(|path| path == "created.rs"));

    status(&source, THREAD, ThreadStatus::Running);
    tick(cx);
    assert!(
        !paths(&files, cx).iter().any(|path| path == "created.rs"),
        "running updates do not relist"
    );
    status(&source, THREAD, ThreadStatus::Idle);
    tick(cx);
    let refreshed = paths(&files, cx);
    assert!(refreshed.iter().any(|path| path == "created.rs"));
    assert!(!refreshed.iter().any(|path| path == "hello.rs"));
    files.update_in(cx, |files, window, cx| {
        files.open_at("created.rs".into(), Some(1), window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        files
            .read_with(cx, |files, cx| files.fixture_preview_text(cx))
            .as_deref(),
        Some("actual agent output\n")
    );

    std::fs::write(source.folder.join("later.rs"), "not another completion\n").expect("real later file");
    tick(cx);
    assert!(
        !paths(&files, cx).iter().any(|path| path == "later.rs"),
        "idle polling does not refresh every tick"
    );
}

#[gpui_kit::test]
fn revoked_original_completion_retires_detached_files_instead_of_relisting(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc);
    let (owner, _, files) = detach(&workspace, cx);
    status(&source, THREAD, ThreadStatus::Running);
    tick(cx);
    std::fs::write(source.folder.join("revoked.rs"), "private\n").expect("real bytes before revocation");
    probe.epoch.fetch_add(1, Ordering::SeqCst);
    status(&source, THREAD, ThreadStatus::Idle);
    tick(cx);
    assert!(files.read_with(cx, |files, _| files.is_retired()));
    assert!(!cx.cx.update(|cx| super::super::current(owner, cx)));
}

#[gpui_kit::test]
fn durable_original_turn_end_between_ticks_refreshes_once_after_source_close(cx: &mut TestAppContext) {
    use nexus_workbench::event::{TurnStatus, WbEvent};
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc);
    let (_, _, files) = detach(&workspace, cx);
    workspace.update_in(cx, |_, window, _| window.remove_window());
    drop(workspace);
    cx.run_until_parked();
    let store = Store::open(&source.data.join("workbench.sqlite3")).expect("actual original writer");
    // The complete real status/event cycle occurs without an executor tick or Code dispatch between edges.
    status(&source, THREAD, ThreadStatus::Running);
    store
        .append(
            THREAD,
            nexus_workbench::now_ms(),
            &WbEvent::TurnStart { turn_id: "fast".into() },
        )
        .expect("durable original turn start");
    std::fs::remove_file(source.folder.join("hello.rs")).expect("real agent deletion");
    std::fs::write(source.folder.join("fast.rs"), "fast completed bytes\n").expect("actual agent write");
    store
        .append(
            THREAD,
            nexus_workbench::now_ms(),
            &WbEvent::TurnEnd {
                turn_id: "fast".into(),
                status: TurnStatus::Completed,
                cost_usd: None,
                duration_ms: 0,
                error: None,
                cursor: None,
            },
        )
        .expect("durable completion before first observation");
    status(&source, THREAD, ThreadStatus::Idle);
    tick(cx);
    assert!(paths(&files, cx).iter().any(|path| path == "fast.rs"));
    assert!(!paths(&files, cx).iter().any(|path| path == "hello.rs"));
    files.update_in(cx, |files, window, cx| {
        files.open_at("fast.rs".into(), Some(1), window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        files
            .read_with(cx, |files, cx| files.fixture_preview_text(cx))
            .as_deref(),
        Some("fast completed bytes\n")
    );

    std::fs::write(source.folder.join("delta.rs"), "not a completion\n").expect("actual later file");
    store
        .append(
            THREAD,
            nexus_workbench::now_ms(),
            &WbEvent::Notice {
                text: "ordinary event".into(),
            },
        )
        .expect("durable non-completion event");
    tick(cx);
    assert!(
        !paths(&files, cx).iter().any(|path| path == "delta.rs"),
        "a consumed checkpoint and later non-completion event do not relist"
    );
}

#[gpui_kit::test]
fn durable_checkpoint_seen_while_still_busy_is_not_consumed_until_refresh(cx: &mut TestAppContext) {
    use nexus_workbench::event::{TurnStatus, WbEvent};
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc);
    let (_, _, files) = detach(&workspace, cx);
    let store = Store::open(&source.data.join("workbench.sqlite3")).expect("actual writer");
    status(&source, THREAD, ThreadStatus::Running);
    std::fs::write(source.folder.join("busy.rs"), "completed agent bytes\n").expect("actual output");
    store
        .append(
            THREAD,
            nexus_workbench::now_ms(),
            &WbEvent::TurnEnd {
                turn_id: "finishing".into(),
                status: TurnStatus::Completed,
                cost_usd: None,
                duration_ms: 0,
                error: None,
                cursor: None,
            },
        )
        .expect("durable end before latest status delivery");
    tick(cx);
    assert!(!paths(&files, cx).iter().any(|path| path == "busy.rs"));
    status(&source, THREAD, ThreadStatus::Idle);
    tick(cx);
    assert!(paths(&files, cx).iter().any(|path| path == "busy.rs"));
}

#[gpui_kit::test]
fn bounded_probe_advances_backlog_across_ticks_and_keeps_terminal_checkpoint(cx: &mut TestAppContext) {
    use nexus_workbench::event::{TurnStatus, WbEvent};
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc);
    let (owner, _, files) = detach(&workspace, cx);
    workspace.update_in(cx, |_, window, _| window.remove_window());
    drop(workspace);
    cx.run_until_parked();
    let store = Store::open(&source.data.join("workbench.sqlite3")).expect("actual original writer");
    status(&source, THREAD, ThreadStatus::Running);
    for index in 0..1030 {
        store
            .append(
                THREAD,
                nexus_workbench::now_ms(),
                &WbEvent::Notice {
                    text: format!("new event {index}"),
                },
            )
            .expect("actual event backlog beyond one bounded worker batch");
    }
    std::fs::write(source.folder.join("backlog.rs"), "after backlog\n").expect("actual agent output");
    store
        .append(
            THREAD,
            nexus_workbench::now_ms(),
            &WbEvent::TurnEnd {
                turn_id: "backlog".into(),
                status: TurnStatus::Completed,
                cost_usd: None,
                duration_ms: 0,
                error: None,
                cursor: None,
            },
        )
        .expect("actual completion after backlog");
    status(&source, THREAD, ThreadStatus::Idle);
    tick(cx);
    assert_eq!(
        cx.cx
            .update(|cx| cx.global::<Registry>().id(owner).expect("current owner").scanned),
        1024,
        "one probe reads at most four pages"
    );
    assert!(!paths(&files, cx).iter().any(|path| path == "backlog.rs"));
    tick(cx);
    assert_eq!(
        cx.cx
            .update(|cx| cx.global::<Registry>().id(owner).expect("current owner").scanned),
        1031
    );
    assert!(paths(&files, cx).iter().any(|path| path == "backlog.rs"));
}
