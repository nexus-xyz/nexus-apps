//! Real native return failures, plain retained snapshots and explicit navigation cancellation.
use super::*;
use super::{
    lifecycle::{isolate_navigation, selection},
    source_ingress::draw,
};
use gpui_kit::AppContext;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
type Created = Rc<RefCell<Vec<(AnyWindowHandle, Entity<Workspace>)>>>;
fn factory(probe: &Probe, source: &SourceStore, rpc: &Rpc, ready: bool, cx: &mut VisualTestContext) -> Created {
    let made: Created = Rc::new(RefCell::new(vec![]));
    let record = made.clone();
    let (probe, source, rpc) = (probe.clone(), source.source(), rpc.clone());
    cx.cx.update(|cx| {
        cx.global_mut::<Registry>().app_window_factory = Some(Rc::new(move |cx| {
            let (window, workspace) = gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut workspace = crate::workspace::gateway_chat::tests::fixture_model(window, cx, &probe);
                    workspace.fixture_terminal_views();
                    if ready {
                        install_source(&mut workspace, &source, &rpc);
                    }
                    workspace.navigation_place_sink = Some(Rc::new(RefCell::new(vec![])));
                    workspace
                })
            })
            .ok()?;
            crate::windows::retention_fixtures::register(Arc::new(probe.clone()), window, workspace.clone(), false, cx);
            record.borrow_mut().push((window, workspace.clone()));
            Some((window, workspace))
        }))
    });
    made
}
fn fail(cx: &mut VisualTestContext) -> Rc<Cell<usize>> {
    let calls = Rc::new(Cell::new(0));
    let record = calls.clone();
    cx.cx.update(|cx| {
        cx.global_mut::<Registry>().app_window_factory = Some(Rc::new(move |_| {
            record.set(record.get() + 1);
            None
        }))
    });
    calls
}
fn close_source(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    workspace.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
}
#[gpui_kit::test]
fn failed_explicit_return_keeps_detached_state_then_retry_uses_one_fresh_original_source(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, original, cx) = setup(cx, &probe, &source, &rpc);
    isolate_navigation(&workspace, cx);
    let before = selection(&workspace, &original, &source, cx);
    let (owner, window, moved) = detach(&workspace, cx);
    close_source(&workspace, cx);
    let calls = fail(cx);
    cx.cx
        .update(|cx| super::super::returning::return_from(owner, window.window_id(), false, cx));
    cx.run_until_parked();
    assert_eq!(calls.get(), 1);
    assert_eq!(detached(owner, cx).1.entity_id(), moved.entity_id());
    moved.read_with(cx, |files, cx| {
        assert!(files.is_current());
        assert_eq!(files.snapshot(cx), before);
    });
    let made = factory(&probe, &source, &rpc, true, cx);
    cx.cx
        .update(|cx| super::super::returning::return_from(owner, window.window_id(), false, cx));
    cx.run_until_parked();
    assert_eq!(made.borrow().len(), 1);
    let (fresh_window, fresh) = made.borrow()[0].clone();
    let mut visual = VisualTestContext::from_window(fresh_window, &cx.cx);
    draw(&mut visual);
    let current = attached(&fresh, &mut visual).expect("fresh native original source receives Files");
    current.read_with(&mut visual, |files, cx| assert_eq!(files.snapshot(cx), before));
    moved.read_with(&mut visual, |files, _| assert!(files.is_retired()));
    visual
        .cx
        .update(|cx| super::super::returning::return_from(owner, window.window_id(), false, cx));
    visual.run_until_parked();
    assert_eq!(made.borrow().len(), 1);
    assert_eq!(
        attached(&fresh, &mut visual).expect("same returned view").entity_id(),
        current.entity_id()
    );
}
#[gpui_kit::test]
fn native_close_attempts_one_fresh_detached_rollback_when_source_open_fails(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, original, cx) = setup(cx, &probe, &source, &rpc);
    isolate_navigation(&workspace, cx);
    let before = selection(&workspace, &original, &source, cx);
    let (owner, window, old) = detach(&workspace, cx);
    close_source(&workspace, cx);
    let calls = fail(cx);
    window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual detached close");
    cx.run_until_parked();
    assert_eq!(calls.get(), 1);
    let (fresh_window, fresh) = detached(owner, cx);
    assert_ne!(fresh_window.window_id(), window.window_id());
    assert_ne!(fresh.entity_id(), old.entity_id());
    fresh.read_with(cx, |files, cx| assert_eq!(files.snapshot(cx), before));
    old.read_with(cx, |files, _| assert!(files.is_retired()));
    cx.cx
        .update(|cx| super::super::returning::return_from(owner, window.window_id(), true, cx));
    assert_eq!(calls.get(), 1);
    assert_eq!(detached(owner, cx).0.window_id(), fresh_window.window_id());
}
#[gpui_kit::test]
fn both_failed_opens_keep_only_plain_selection_alive_until_explicit_quit(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, original, cx) = setup(cx, &probe, &source, &rpc);
    isolate_navigation(&workspace, cx);
    let before = selection(&workspace, &original, &source, cx);
    let (owner, window, old) = detach(&workspace, cx);
    close_source(&workspace, cx);
    let calls = fail(cx);
    cx.cx.update(|cx| cx.global_mut::<Registry>().fail_next_open = true);
    window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual native close");
    cx.run_until_parked();
    assert_eq!(calls.get(), 1);
    cx.cx.update(|cx| {
        match &cx
            .global::<Registry>()
            .id(owner)
            .expect("plain owner remains")
            .presentation
        {
            Presentation::Returning { saved } => assert_eq!(saved.borrow().as_ref(), Some(&before)),
            _ => panic!("both failed opens must retain plain state"),
        }
    });
    old.read_with(cx, |files, _| assert!(files.is_retired()));
    assert!(cx.cx.update(|cx| super::super::keeps_app_alive(cx)));
    cx.cx.update(|cx| cx.shutdown());
    cx.run_until_parked();
    assert!(!cx.cx.update(|cx| super::super::keeps_app_alive(cx)));
    assert!(cx.cx.update(|cx| cx.global::<Registry>().entries.is_empty()));
    assert_eq!(calls.get(), 1, "explicit quit cannot reopen any window");
}
#[gpui_kit::test]
fn explicit_navigation_cancels_pending_return_but_exact_session_recovers_plain_state(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, original, cx) = setup(cx, &probe, &source, &rpc);
    isolate_navigation(&workspace, cx);
    let before = selection(&workspace, &original, &source, cx);
    let resource = key(&workspace, cx);
    let (owner, window, _) = detach(&workspace, cx);
    close_source(&workspace, cx);
    let made = factory(&probe, &source, &rpc, false, cx);
    cx.cx
        .update(|cx| super::super::returning::return_from(owner, window.window_id(), false, cx));
    cx.run_until_parked();
    assert_eq!(made.borrow().len(), 1);
    let (fresh_window, fresh) = made.borrow()[0].clone();
    let mut visual = VisualTestContext::from_window(fresh_window, &cx.cx);
    fresh.read_with(&mut visual, |workspace, _| assert!(workspace.files_return.is_some()));
    fresh.update_in(&mut visual, |workspace, window, cx| workspace.new_chat(window, cx));
    visual.run_until_parked();
    fresh.update_in(&mut visual, |workspace, window, cx| {
        assert!(workspace.files_return.is_none());
        install_source(workspace, &source.source(), &rpc);
        workspace.navigation_place_sink = Some(Rc::new(RefCell::new(vec![])));
        workspace.sync_files_return(window, cx);
        assert!(matches!(workspace.view, crate::workspace::View::Chat));
    });
    visual.cx.update(|cx| {
        let entry = cx.global::<Registry>().key(&resource).expect("original plain owner");
        assert!(matches!(entry.presentation, Presentation::Returning { .. }));
    });
    assert!(attached(&fresh, &mut visual).is_none());
    fresh.update_in(&mut visual, |workspace, window, cx| {
        workspace.open_code(THREAD, window, cx);
        select(workspace, THREAD, window, cx);
        workspace.sync_side(window, cx);
    });
    visual.run_until_parked();
    draw(&mut visual);
    let current = attached(&fresh, &mut visual).expect("person explicitly reopened exact original session");
    current.read_with(&mut visual, |files, cx| assert_eq!(files.snapshot(cx), before));
    assert_eq!(made.borrow().len(), 1);
}
