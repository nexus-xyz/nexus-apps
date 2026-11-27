//! Installed native close callbacks and constructor rollback around real source-qualified Files views.
use super::source_ingress::{click, draw};
use super::*;
use crate::side::{
    file_view::{FileView, FileViewState},
    files::FilesState,
};
use std::{cell::RefCell, collections::HashSet, rc::Rc};

pub(super) fn isolate_navigation(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    workspace.update(cx, |workspace, _| {
        workspace.navigation_place_sink = Some(Rc::new(RefCell::new(vec![])))
    });
}
/// Set real user control state through the guarded constructor; file lists and previews still read actual bytes.
pub(super) fn selection(
    workspace: &Entity<Workspace>,
    files: &Entity<Files>,
    source: &SourceStore,
    cx: &mut VisualTestContext,
) -> FilesState {
    std::fs::create_dir_all(source.folder.join("src/expanded")).expect("real nested folder");
    std::fs::write(source.folder.join("src/expanded/code.rs"), "first line\nsecond line\n")
        .expect("real selected file");
    let state = FilesState {
        query: "src".into(),
        all: true,
        expanded: HashSet::from(["src".into(), "src/expanded".into()]),
        scroll_top: 1,
        open: Some(FileViewState {
            path: "src/expanded/code.rs".into(),
            line: Some(2),
            source: true,
            zoom: false,
            pdf_page: None,
        }),
    };
    let (_, place, guard) = workspace.read_with(cx, |workspace, _| {
        workspace.files_attached_source(THREAD).expect("actual accepted source")
    });
    files.update_in(cx, |files, window, cx| {
        files.retire(cx);
        *files = Files::restore(place, state.clone(), guard, window, cx);
    });
    cx.run_until_parked();
    files.read_with(cx, |files, cx| {
        assert_eq!(files.snapshot(cx), state);
        assert_eq!(
            files.fixture_preview_text(cx).as_deref(),
            Some("first line\nsecond line\n")
        );
    });
    state
}
#[gpui_kit::test]
fn actual_move_chip_preserves_nondefault_state_and_repeat_reuses_the_native_owner(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, original, cx) = setup(cx, &probe, &source, &rpc);
    isolate_navigation(&workspace, cx);
    let before = selection(&workspace, &original, &source, cx);
    // New sessions start closed; open this tool before testing its native controls.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.pick_tool(crate::workspace::side_pane::Tool::Files, window, cx);
    });
    let resource = key(&workspace, cx);
    click("move-files-window", cx);
    let owner = cx.cx.update(|cx| {
        let registry = cx.global::<Registry>();
        assert_eq!(registry.entries.len(), 1);
        let entry = registry
            .key(&resource)
            .expect("click immediately admitted the exact owner");
        assert!(matches!(entry.presentation, Presentation::Detached { .. }));
        entry.id
    });
    let (window, moved) = detached(owner, cx);
    assert_ne!(moved.entity_id(), original.entity_id());
    original.read_with(cx, |files, _| assert!(files.is_retired()));
    moved.read_with(cx, |files, cx| {
        assert_eq!(files.snapshot(cx), before);
        assert_eq!(
            files.fixture_preview_text(cx).as_deref(),
            Some("first line\nsecond line\n")
        );
    });
    workspace
        .update_in(cx, |workspace, window, cx| workspace.detach_files(window, cx))
        .expect("repeat Move focuses existing owner");
    cx.run_until_parked();
    let (same_window, same) = detached(owner, cx);
    assert_eq!(same_window.window_id(), window.window_id());
    assert_eq!(same.entity_id(), moved.entity_id());
    assert_eq!(cx.cx.update(|cx| cx.global::<Registry>().entries.len()), 1);
}
fn rollback(after_construct: bool, cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, original, cx) = setup(cx, &probe, &source, &rpc);
    isolate_navigation(&workspace, cx);
    let before = selection(&workspace, &original, &source, cx);
    cx.cx.update(|cx| {
        super::super::init(cx);
        if after_construct {
            cx.global_mut::<Registry>().fail_after_construct = true;
        } else {
            cx.global_mut::<Registry>().fail_next_open = true;
        }
    });
    let error = workspace
        .update_in(cx, |workspace, window, cx| workspace.detach_files(window, cx))
        .expect_err("native constructor failure is injected");
    assert!(error.contains("returned"));
    draw(cx);
    let fresh = attached(&workspace, cx).expect("rollback must restore an attached view");
    assert_ne!(fresh.entity_id(), original.entity_id());
    original.read_with(cx, |files, _| assert!(files.is_retired()));
    fresh.read_with(cx, |files, cx| {
        assert!(files.is_current());
        assert_eq!(files.snapshot(cx), before);
        assert_eq!(
            files.fixture_preview_text(cx).as_deref(),
            Some("first line\nsecond line\n")
        );
    });
    cx.cx.update(|cx| { assert_eq!(cx.global::<Registry>().entries.len(), 1); assert!(matches!(&cx.global::<Registry>().entries[0].presentation, Presentation::Attached { files, .. } if files.upgrade().is_some_and(|files| files.entity_id() == fresh.entity_id()))); });
}
#[gpui_kit::test]
fn failed_native_open_before_constructor_rolls_back_selection_to_a_fresh_view(cx: &mut TestAppContext) {
    rollback(false, cx);
}
#[gpui_kit::test]
fn failed_native_open_after_constructor_recovers_the_consumed_snapshot_once(cx: &mut TestAppContext) {
    rollback(true, cx);
}
#[gpui_kit::test]
fn installed_native_close_returns_once_and_stale_close_cannot_move_the_new_view(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, original, cx) = setup(cx, &probe, &source, &rpc);
    isolate_navigation(&workspace, cx);
    let before = selection(&workspace, &original, &source, cx);
    let (owner, window, old) = detach(&workspace, cx);
    window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual detached native close");
    cx.run_until_parked();
    draw(cx);
    let fresh = attached(&workspace, cx).expect("installed close observer returned Files");
    assert_ne!(fresh.entity_id(), old.entity_id());
    fresh.read_with(cx, |files, cx| assert_eq!(files.snapshot(cx), before));
    old.read_with(cx, |files, _| assert!(files.is_retired()));
    cx.cx
        .update(|cx| super::super::returning::return_from(owner, window.window_id(), true, cx));
    cx.run_until_parked();
    draw(cx);
    assert_eq!(
        attached(&workspace, cx).expect("same returned owner").entity_id(),
        fresh.entity_id()
    );
    assert!(window.update(&mut cx.cx, |_, _, _| ()).is_err());
}
#[gpui_kit::test]
fn another_exact_reader_without_fresh_runs_can_focus_the_existing_detached_owner(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc);
    isolate_navigation(&workspace, cx);
    let (owner, window, original) = detach(&workspace, cx);
    let (reader, cx) = fixture(cx, &probe);
    let handle = reader.update_in(cx, |reader, window, cx| {
        install_source(reader, &source.source(), &rpc);
        reader.files_fixture_runs = None;
        select(reader, THREAD, window, cx);
        assert!(reader.runs(THREAD).is_none());
        window.window_handle()
    });
    cx.cx.update(|cx| {
        crate::windows::retention_fixtures::register(Arc::new(probe.clone()), handle, reader.clone(), false, cx)
    });
    isolate_navigation(&reader, cx);
    reader
        .update_in(cx, |reader, window, cx| reader.detach_files(window, cx))
        .expect("exact reader may focus retained original Runs owner");
    cx.run_until_parked();
    let (same_window, same) = detached(owner, cx);
    assert_eq!(same_window.window_id(), window.window_id());
    assert_eq!(same.entity_id(), original.entity_id());
    assert_eq!(cx.cx.update(|cx| cx.global::<Registry>().entries.len()), 1);
}
#[gpui_kit::test]
fn initial_unregistered_reader_only_files_and_held_preview_retire_on_real_source_close(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    cx.update(super::theme);
    let (workspace, cx) = fixture(cx, &probe);
    let (source_window, files, place, guard) = workspace.update_in(cx, |workspace, window, cx| {
        install_source(workspace, &source.source(), &rpc);
        workspace.files_fixture_runs = None;
        select(workspace, THREAD, window, cx);
        assert!(workspace.runs(THREAD).is_none());
        let files = workspace
            .fixture_local_files(THREAD, window, cx)
            .expect("initial reader-only Files");
        let (_, place, guard) = workspace
            .files_attached_source(THREAD)
            .expect("accepted original reader");
        (window.window_handle(), files, place, guard)
    });
    cx.cx.update(|cx| {
        crate::windows::retention_fixtures::register(
            Arc::new(probe.clone()),
            source_window,
            workspace.clone(),
            false,
            cx,
        )
    });
    isolate_navigation(&workspace, cx);
    files.update_in(cx, |files, window, cx| {
        files.open_at("hello.rs".into(), Some(1), window, cx)
    });
    cx.run_until_parked();
    let preview = files.read_with(cx, |files, _| files.fixture_preview().expect("actual opened child"));
    let saved = preview.read_with(cx, |view, _| view.snapshot());
    let (send, pending) = futures::channel::oneshot::channel();
    preview.update_in(cx, |view, window, cx| {
        view.retire(cx);
        *view = FileView::fixture_preview(place, saved, guard, pending, window, cx);
        view.fixture_editor(crate::editor_choice::Fixture {
            load: Arc::new(|| panic!("closed view cannot discover editors")),
            launch: Arc::new(|_, _| panic!("closed view cannot launch an editor")),
        });
    });
    cx.run_until_parked();
    assert!(
        cx.cx.update(|cx| cx.try_global::<Registry>().is_none()),
        "initial attached Files has never registered a retained owner"
    );
    let (fresh, cx) = fixture(cx, &probe);
    let (fresh_window, fresh_files) = fresh.update_in(cx, |workspace, window, cx| {
        install_source(workspace, &source.source(), &rpc);
        workspace.files_fixture_runs = None;
        select(workspace, THREAD, window, cx);
        (
            window.window_handle(),
            workspace
                .fixture_local_files(THREAD, window, cx)
                .expect("fresh authorized reader"),
        )
    });
    cx.cx.update(|cx| {
        crate::windows::retention_fixtures::register(Arc::new(probe.clone()), fresh_window, fresh.clone(), false, cx)
    });
    isolate_navigation(&fresh, cx);
    source_window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("real initial source native close");
    cx.run_until_parked();
    files.read_with(cx, |files, _| assert!(files.is_retired()));
    preview.read_with(cx, |view, _| assert!(view.is_retired()));
    let actual = nexus_workbench::output_files::read(&source.folder, "hello.rs").map(|preview| (preview, true));
    assert!(
        send.send(actual).is_err(),
        "retirement cancels the actual held preview receiver"
    );
    fresh.update_in(cx, |_, window, cx| {
        files.update(cx, |files, cx| {
            files.refresh(cx);
            files.open_at("hello.rs".into(), None, window, cx);
        });
        assert_eq!(
            preview.update(cx, |view, cx| view.fixture_attempt_actions(window, cx)),
            (false, None)
        );
    });
    cx.run_until_parked();
    assert!(
        cx.cx.update(|cx| cx.try_global::<Registry>().is_none()),
        "ordinary native close needs no retained registry entry"
    );
    fresh_files.update_in(cx, |files, window, cx| {
        files.open_at("hello.rs".into(), Some(1), window, cx)
    });
    cx.run_until_parked();
    fresh_files.read_with(cx, |files, cx| {
        assert!(files.is_current());
        assert!(
            files
                .fixture_paths()
                .expect("fresh real list")
                .iter()
                .any(|path| path == "hello.rs")
        );
        assert!(
            files
                .fixture_preview_text(cx)
                .expect("fresh actual preview")
                .contains("fn original()")
        );
    });
}
