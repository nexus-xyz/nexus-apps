//! Return through real fresh native views, with delayed readiness and bounded native-open failure.
use super::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
type Created = Rc<RefCell<Vec<(AnyWindowHandle, Entity<Workspace>)>>>;

fn fresh_factory(probe: &Probe, source: &SourceStore, rpc: &Rpc, ready: bool, cx: &mut VisualTestContext) -> Created {
    let created: Created = Rc::new(RefCell::new(vec![]));
    let record = created.clone();
    let (probe, source, rpc) = (probe.clone(), source.source(), rpc.clone());
    cx.cx.update(|cx| {
        cx.global_mut::<Registry>().app_window_factory = Some(Rc::new(move |cx| {
            let (window, workspace) = gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut workspace = crate::workspace::gateway_chat::tests::fixture_model(window, cx, &probe);
                    assert!(
                        workspace.navigation_place_sink.is_some(),
                        "fixture navigation must stay in memory"
                    );
                    if ready {
                        install_source(&mut workspace, &source, &rpc);
                    }
                    workspace
                })
            })
            .ok()?;
            crate::windows::retention_fixtures::register(Arc::new(probe.clone()), window, workspace.clone(), false, cx);
            record.borrow_mut().push((window, workspace.clone()));
            Some((window, workspace))
        }))
    });
    created
}
fn fail_factory(cx: &mut VisualTestContext) -> Rc<Cell<usize>> {
    let called = Rc::new(Cell::new(0));
    let record = called.clone();
    cx.cx.update(|cx| {
        cx.global_mut::<Registry>().app_window_factory = Some(Rc::new(move |_| {
            record.set(record.get() + 1);
            None
        }))
    });
    called
}
fn close_source(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    workspace.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
}

#[gpui_kit::test]
fn failed_return_keeps_detached_page_then_retry_creates_a_fresh_original_source_view(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6201);
    let (old_workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, window, browser) = detach(&old_workspace, cx);
    close_source(&old_workspace, cx);
    let called = fail_factory(cx);
    cx.cx
        .update(|cx| returning::return_from(id, window.window_id(), false, cx));
    cx.run_until_parked();
    assert_eq!(called.get(), 1);
    assert_eq!(detached(id, cx).1.entity_id(), browser.entity_id());
    assert!(page_probe.alive() && !page_probe.parked());
    let created = fresh_factory(&probe, &source, &rpc, true, cx);
    cx.cx
        .update(|cx| returning::return_from(id, window.window_id(), false, cx));
    cx.run_until_parked();
    assert_eq!(created.borrow().len(), 1);
    let (fresh_window, fresh_workspace) = created.borrow()[0].clone();
    assert_ne!(old_workspace.entity_id(), fresh_workspace.entity_id());
    fresh_window
        .update(&mut cx.cx, |_, window, cx| {
            fresh_workspace.update(cx, |workspace, cx| workspace.sync_browser_return(window, cx))
        })
        .expect("new real app window");
    cx.run_until_parked();
    fresh_workspace.read_with(cx, |workspace, _| {
        assert!(matches!(&workspace.view, View::Code(view) if view.id == THREAD));
        assert!(workspace.has_browser(THREAD));
    });
    browser.read_with(cx, |browser, _| assert!(browser.is_retired()));
    assert!(page_probe.alive() && !page_probe.parked());
    let (routed, current, _) = agent_tabs(&source.origin(), cx);
    assert_eq!(routed.window_id(), fresh_window.window_id());
    assert_ne!(current.entity_id(), browser.entity_id());
    cx.cx
        .update(|cx| returning::return_from(id, window.window_id(), false, cx));
    assert_eq!(
        created.borrow().len(),
        1,
        "a stale Return cannot create another app window"
    );
}

#[gpui_kit::test]
fn native_close_rebuilds_detached_presentation_if_the_session_window_cannot_open(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6202);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, old_window, old_browser) = detach(&workspace, cx);
    close_source(&workspace, cx);
    let calls = fail_factory(cx);
    old_window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual detached native close");
    cx.run_until_parked();
    assert_eq!(calls.get(), 1);
    let (fresh_window, fresh_browser) = detached(id, cx);
    assert_ne!(old_window.window_id(), fresh_window.window_id());
    assert_ne!(old_browser.entity_id(), fresh_browser.entity_id());
    old_browser.read_with(cx, |browser, _| assert!(browser.is_retired()));
    assert!(page_probe.alive() && !page_probe.parked());
    assert_eq!(agent_tabs(&source.origin(), cx).0.window_id(), fresh_window.window_id());
    cx.cx
        .update(|cx| returning::return_from(id, old_window.window_id(), true, cx));
    assert_eq!(detached(id, cx).0.window_id(), fresh_window.window_id());
    assert_eq!(calls.get(), 1);
}

#[gpui_kit::test]
fn failed_session_and_detached_opens_keep_plain_pages_until_explicit_quit(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6203);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, window, browser) = detach(&workspace, cx);
    close_source(&workspace, cx);
    let calls = fail_factory(cx);
    cx.cx.update(|cx| cx.global_mut::<Registry>().fail_next_open = true);
    window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual native close");
    cx.run_until_parked();
    assert_eq!(calls.get(), 1);
    cx.cx.update(|cx| {
        match &cx
            .global::<Registry>()
            .id(id)
            .expect("plain owner still held")
            .presentation
        {
            Presentation::Returning { saved } => assert!(saved.borrow().is_some()),
            _ => panic!("both failed opens must retain plain pages"),
        }
    });
    browser.read_with(cx, |browser, _| assert!(browser.is_retired()));
    assert!(page_probe.alive() && page_probe.parked());
    assert!(cx.cx.update(|cx| crate::workspace::browser_detach::keeps_app_alive(cx)));
    cx.cx.update(crate::workspace::browser_detach::quit);
    cx.run_until_parked();
    assert!(!page_probe.alive());
    assert_eq!(calls.get(), 1, "quit must not create a return window");
    assert!(!cx.cx.update(|cx| crate::workspace::browser_detach::keeps_app_alive(cx)));
}

#[gpui_kit::test]
fn explicit_new_chat_cancels_deferred_return_route_without_discarding_the_page(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6204);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let resource = key(&workspace, cx);
    let (id, window, _) = detach(&workspace, cx);
    close_source(&workspace, cx);
    let created = fresh_factory(&probe, &source, &rpc, false, cx);
    cx.cx
        .update(|cx| returning::return_from(id, window.window_id(), false, cx));
    cx.run_until_parked();
    assert_eq!(created.borrow().len(), 1);
    let (fresh_window, fresh) = created.borrow()[0].clone();
    fresh.read_with(cx, |workspace, _| assert!(workspace.browser_return.is_some()));
    fresh_window
        .update(&mut cx.cx, |_, window, cx| {
            fresh.update(cx, |workspace, cx| workspace.new_chat(window, cx))
        })
        .expect("explicit New Chat in fresh native view");
    cx.run_until_parked();
    fresh_window
        .update(&mut cx.cx, |_, window, cx| {
            fresh.update(cx, |workspace, cx| {
                assert!(workspace.browser_return.is_none());
                install_source(workspace, &source.source(), &rpc);
                workspace.sync_browser_return(window, cx);
                assert!(matches!(workspace.view, View::Chat));
            })
        })
        .expect("source becomes ready after explicit navigation");
    cx.cx.update(|cx| {
        assert!(matches!(
            &cx.global::<Registry>()
                .key(&resource)
                .expect("return owner retained")
                .presentation,
            Presentation::Returning { .. }
        ))
    });
    assert!(page_probe.alive() && page_probe.parked());
    fresh_window
        .update(&mut cx.cx, |_, window, cx| {
            fresh.update(cx, |workspace, cx| {
                workspace.open_code(THREAD, window, cx);
                workspace.sync_side(window, cx);
            })
        })
        .expect("person reopens the exact original session");
    cx.run_until_parked();
    fresh.read_with(cx, |workspace, _| assert!(workspace.has_browser(THREAD)));
    assert!(page_probe.alive() && !page_probe.parked());
    assert_eq!(agent_tabs(&source.origin(), cx).0.window_id(), fresh_window.window_id());
}

#[gpui_kit::test]
fn two_preexisting_same_source_browsers_refuse_detach_without_discarding_either_page(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page_a, alive_a) = crate::web::fixture_page(6205);
    let (first, browser_a, cx) = setup(cx, &probe, &source, &rpc, page_a);
    let (page_b, alive_b) = crate::web::fixture_page(6206);
    let (_, browser_b, cx) = setup(cx, &probe, &source, &rpc, page_b);
    let error = first
        .update_in(cx, |workspace, window, cx| workspace.detach_browser(window, cx))
        .expect_err("existing duplicate native owners require an explicit choice");
    assert!(error.contains("other Browser"));
    assert!(alive_a.alive() && alive_b.alive());
    browser_a.read_with(cx, |browser, _| assert!(!browser.is_retired()));
    browser_b.read_with(cx, |browser, _| assert!(!browser.is_retired()));
}
