//! Browser and Files share app lifetime and source routing, but retain separate presentations and return tickets.
use super::*;
use crate::side::browser::Browser;
use gpui_kit::AppContext;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::atomic::Ordering,
};

type Created = Rc<RefCell<Vec<(AnyWindowHandle, Entity<Workspace>)>>>;
type Factory = Rc<dyn Fn(&mut App) -> Option<(AnyWindowHandle, Entity<Workspace>)>>;

fn browser(source: &SourceStore, cx: &mut VisualTestContext) -> (AnyWindowHandle, Entity<Browser>) {
    let valid: crate::side::browser::agent::Guard = Arc::new(|| true);
    let (window, browser, _) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid, cx))
        .expect("Browser remains available through its original source");
    browser.read_with(cx, |browser, _| {
        assert!(!browser.is_retired());
        assert_eq!(browser.tabs().0.len(), 1);
    });
    (window, browser)
}

fn detach_both(
    workspace: &Entity<Workspace>,
    source: &SourceStore,
    page: crate::web::RetainedPage,
    cx: &mut VisualTestContext,
) -> (AnyWindowHandle, Entity<Browser>, u64, AnyWindowHandle, Entity<Files>) {
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.fixture_local_browser(THREAD, page, window, cx);
        workspace.detach_browser(window, cx).expect("move actual Browser owner");
    });
    cx.run_until_parked();
    let (browser_window, browser) = browser(source, cx);
    workspace.update_in(cx, |workspace, window, cx| select(workspace, THREAD, window, cx));
    let (owner, files_window, files) = detach(workspace, cx);
    assert_ne!(browser_window.window_id(), files_window.window_id());
    (browser_window, browser, owner, files_window, files)
}

fn draw(window: AnyWindowHandle, cx: &mut VisualTestContext) {
    for _ in 0..3 {
        window
            .update(&mut cx.cx, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.simulate_next_frame(cx);
            })
            .expect("real source window draws both return paths");
        cx.run_until_parked();
    }
}

pub(super) fn fresh_source_factory(
    probe: &Probe,
    source: &SourceStore,
    rpc: &Rpc,
    ready: bool,
    cx: &mut VisualTestContext,
) -> Created {
    let created: Created = Rc::new(RefCell::new(vec![]));
    let record = created.clone();
    let (probe, source, rpc) = (probe.clone(), source.source(), rpc.clone());
    cx.cx.update(|cx| {
        let factory: Factory = Rc::new(move |cx| {
            let (window, workspace) = gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut workspace = crate::workspace::gateway_chat::tests::fixture_model(window, cx, &probe);
                    workspace.fixture_terminal_views();
                    if ready {
                        install_source(&mut workspace, &source, &rpc);
                    } else {
                        workspace.account = None;
                        workspace.terminal_identity = None;
                        workspace.gateway_account_authority = None;
                        workspace.code = None;
                        workspace.files_fixture_runs = None;
                        workspace.browser_detach_runs = None;
                    }
                    assert!(
                        workspace.navigation_place_sink.is_some(),
                        "fresh test navigation stays in memory"
                    );
                    workspace
                })
            })
            .ok()?;
            crate::windows::retention_fixtures::register(Arc::new(probe.clone()), window, workspace.clone(), false, cx);
            record.borrow_mut().push((window, workspace.clone()));
            Some((window, workspace))
        });
        cx.global_mut::<Registry>().app_window_factory = Some(factory.clone());
        crate::workspace::browser_detach::fixture_return_factory(factory, cx);
    });
    created
}

fn forbid_reopen(cx: &mut VisualTestContext) -> Rc<Cell<usize>> {
    let attempts = Rc::new(Cell::new(0));
    let record = attempts.clone();
    cx.cx.update(|cx| {
        let factory: Factory = Rc::new(move |_| {
            record.set(record.get() + 1);
            None
        });
        cx.global_mut::<Registry>().app_window_factory = Some(factory.clone());
        crate::workspace::browser_detach::fixture_return_factory(factory, cx);
    });
    attempts
}

#[gpui_kit::test]
fn files_native_return_after_source_close_keeps_browser_alive_then_both_share_the_fresh_source(
    cx: &mut TestAppContext,
) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, original_files, cx) = setup(cx, &probe, &source, &rpc);
    original_files.update_in(cx, |files, window, cx| {
        files.open_at("hello.rs".into(), Some(1), window, cx)
    });
    let before = original_files.read_with(cx, |files, cx| files.snapshot(cx));
    let (page, page_probe) = crate::web::fixture_page(8101);
    let (browser_window, detached_browser, _, files_window, detached_files) =
        detach_both(&workspace, &source, page, cx);
    let weak_workspace = workspace.downgrade();
    let weak_composer = workspace.read_with(cx, |workspace, _| workspace.composer.downgrade());
    workspace.update_in(cx, |_, window, _| window.remove_window());
    cx.cx.update(|_| drop(workspace));
    cx.run_until_parked();
    assert!(
        weak_workspace.upgrade().is_none(),
        "neither owner retains the old Workspace"
    );
    assert!(
        weak_composer.upgrade().is_none(),
        "neither owner retains the old composer"
    );
    assert!(cx.cx.update(|cx| super::super::keeps_app_alive(cx)));
    assert!(cx.cx.update(|cx| crate::workspace::browser_detach::keeps_app_alive(cx)));
    assert_eq!(browser(&source, cx).0.window_id(), browser_window.window_id());
    let created = fresh_source_factory(&probe, &source, &rpc, true, cx);
    files_window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual Files native close");
    cx.run_until_parked();
    assert_eq!(created.borrow().len(), 1);
    let (fresh_window, fresh) = created.borrow()[0].clone();
    draw(fresh_window, cx);
    let returned_files = attached(&fresh, cx).expect("Files returned to the new source");
    assert_ne!(returned_files.entity_id(), original_files.entity_id());
    assert_ne!(returned_files.entity_id(), detached_files.entity_id());
    returned_files.read_with(cx, |files, cx| assert_eq!(files.snapshot(cx), before));
    detached_files.read_with(cx, |files, _| assert!(files.is_retired()));
    assert!(page_probe.alive() && !page_probe.parked());
    assert_eq!(browser(&source, cx).1.entity_id(), detached_browser.entity_id());
    browser_window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual Browser native close");
    cx.run_until_parked();
    draw(fresh_window, cx);
    let (routed, returned_browser) = browser(&source, cx);
    assert_eq!(routed.window_id(), fresh_window.window_id());
    assert_ne!(returned_browser.entity_id(), detached_browser.entity_id());
    assert_eq!(
        attached(&fresh, cx).expect("Files remains attached").entity_id(),
        returned_files.entity_id()
    );
    assert_eq!(
        created.borrow().len(),
        1,
        "Browser reuses the verified Files return destination"
    );
    assert!(page_probe.alive() && !page_probe.parked());
}

#[gpui_kit::test]
fn both_return_tickets_survive_internal_navigation_and_restore_once_in_real_render(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc);
    let (page, page_probe) = crate::web::fixture_page(8102);
    let (browser_window, old_browser, _, files_window, old_files) = detach_both(&workspace, &source, page, cx);
    let files_key = key(&workspace, cx);
    let browser_key = workspace.read_with(cx, |workspace, _| {
        workspace.browser_detach_key(THREAD).expect("original Browser key")
    });
    let source_window = workspace.update_in(cx, |workspace, window, cx| {
        // Leaving the last attached tool may open the real Shell fallback. This fixture
        // exercises its normal view construction without an OS PTY waking the test scheduler.
        workspace.fixture_terminal_views();
        assert_eq!(workspace.fixture_private_terminal_count(), 0);
        workspace.new_chat(window, cx);
        window.window_handle()
    });
    draw(source_window, cx);
    workspace.read_with(cx, |workspace, _| {
        assert!(
            workspace.fixture_private_terminal_count() > 0,
            "the actual Chat-route fallback constructed an isolated terminal view"
        );
    });
    let pending_seen = Rc::new(Cell::new(false));
    let observed = pending_seen.clone();
    let target = workspace.clone();
    let expected_browser = browser_key.clone();
    let expected_files = files_key.clone();
    cx.cx.update(|cx| {
        crate::workspace::browser_detach::return_to_session(&browser_key, cx);
        super::super::returning::request(&files_key, cx);
        // Both requests defer their transfer. Observe after those FIFO callbacks but before
        // GPUI's test effect flush draws the ready source and consumes its return tickets.
        cx.defer(move |cx| {
            let workspace = target.read(cx);
            assert!(
                workspace
                    .browser_return
                    .as_ref()
                    .is_some_and(|ticket| ticket.key == expected_browser),
                "Browser has its own exact route ticket"
            );
            assert!(
                workspace
                    .files_return
                    .as_ref()
                    .is_some_and(|ticket| ticket.key == expected_files),
                "Files has its own exact route ticket"
            );
            observed.set(true);
        });
    });
    cx.run_until_parked();
    assert!(
        pending_seen.get(),
        "both real transfers ran before the ticket observation"
    );
    draw(source_window, cx);
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.browser_return.is_none() && workspace.files_return.is_none());
        assert!(matches!(&workspace.view, crate::workspace::View::Code(view) if view.id == THREAD));
        assert!(workspace.fixture_session_browser(THREAD).is_some());
    });
    let current_files = attached(&workspace, cx).expect("Files survives Browser internal route adoption");
    assert_ne!(current_files.entity_id(), old_files.entity_id());
    let (current_window, current_browser) = browser(&source, cx);
    assert_eq!(current_window.window_id(), source_window.window_id());
    assert_ne!(current_browser.entity_id(), old_browser.entity_id());
    old_files.read_with(cx, |files, _| assert!(files.is_retired()));
    old_browser.read_with(cx, |browser, _| assert!(browser.is_retired()));
    assert!(browser_window.update(&mut cx.cx, |_, _, _| ()).is_err());
    assert!(files_window.update(&mut cx.cx, |_, _, _| ()).is_err());
    cx.cx.update(|cx| {
        crate::workspace::browser_detach::return_to_session(&browser_key, cx);
        super::super::returning::request(&files_key, cx);
    });
    draw(source_window, cx);
    assert_eq!(
        attached(&workspace, cx).expect("same returned Files").entity_id(),
        current_files.entity_id()
    );
    assert_eq!(browser(&source, cx).1.entity_id(), current_browser.entity_id());
    assert!(page_probe.alive() && !page_probe.parked());
}

#[gpui_kit::test]
fn installed_sign_out_reader_retires_both_detached_tools_before_their_close_callbacks(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc);
    let (page, page_probe) = crate::web::fixture_page(8103);
    let (_, old_browser, _, _, old_files) = detach_both(&workspace, &source, page, cx);
    workspace.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    let attempts = forbid_reopen(cx);
    let events = cx
        .cx
        .update(|cx| crate::windows::retention_fixtures::watch_identity(Arc::new(probe.clone()), cx));
    probe.epoch.fetch_add(2, Ordering::SeqCst);
    events
        .unbounded_send(crate::gateway::Event::SignedOut)
        .expect("installed app account reader");
    cx.run_until_parked();
    old_files.read_with(cx, |files, _| assert!(files.is_retired()));
    old_browser.read_with(cx, |browser, _| assert!(browser.is_retired()));
    assert!(!page_probe.alive());
    assert!(!cx.cx.update(|cx| super::super::keeps_app_alive(cx)));
    assert!(!cx.cx.update(|cx| crate::workspace::browser_detach::keeps_app_alive(cx)));
    assert!(cx.cx.update(|cx| cx.global::<Registry>().entries.is_empty()));
    assert_eq!(
        attempts.get(),
        0,
        "revocation cannot reopen the source through a close callback"
    );
}

#[gpui_kit::test]
fn installed_quit_hook_retires_both_tools_and_stale_return_requests_cannot_reopen(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc);
    let (page, page_probe) = crate::web::fixture_page(8104);
    let (_, old_browser, _, _, old_files) = detach_both(&workspace, &source, page, cx);
    let files_key = key(&workspace, cx);
    let browser_key = workspace.read_with(cx, |workspace, _| {
        workspace.browser_detach_key(THREAD).expect("original Browser key")
    });
    workspace.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    let attempts = forbid_reopen(cx);
    cx.cx.update(|cx| cx.shutdown());
    cx.run_until_parked();
    old_files.read_with(cx, |files, _| assert!(files.is_retired()));
    old_browser.read_with(cx, |browser, _| assert!(browser.is_retired()));
    assert!(!page_probe.alive());
    assert!(!cx.cx.update(|cx| super::super::keeps_app_alive(cx)));
    assert!(!cx.cx.update(|cx| crate::workspace::browser_detach::keeps_app_alive(cx)));
    let windows = cx.cx.update(|cx| cx.windows().len());
    cx.cx.update(|cx| {
        crate::workspace::browser_detach::return_to_session(&browser_key, cx);
        super::super::returning::request(&files_key, cx);
    });
    cx.run_until_parked();
    assert_eq!(attempts.get(), 0);
    assert_eq!(cx.cx.update(|cx| cx.windows().len()), windows);
    assert!(cx.cx.update(|cx| cx.global::<Registry>().entries.is_empty()));
}

fn pending_account_returns_coalesce(browser_first: bool, cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc);
    let (account, proof) = workspace.read_with(cx, |workspace, _| {
        (
            workspace.account.clone().expect("original accepted Account"),
            workspace
                .gateway_account_authority
                .clone()
                .expect("accepted epoch receipt"),
        )
    });
    let (page, page_probe) = crate::web::fixture_page(if browser_first { 8105 } else { 8106 });
    let (_, old_browser, _, _, old_files) = detach_both(&workspace, &source, page, cx);
    let files_key = key(&workspace, cx);
    let browser_key = workspace.read_with(cx, |workspace, _| {
        workspace.browser_detach_key(THREAD).expect("Browser source key")
    });
    workspace.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    let created = fresh_source_factory(&probe, &source, &rpc, false, cx);
    cx.cx.update(|cx| {
        if browser_first {
            crate::workspace::browser_detach::return_to_session(&browser_key, cx);
        } else {
            super::super::returning::request(&files_key, cx);
        }
    });
    cx.run_until_parked();
    assert_eq!(created.borrow().len(), 1);
    let (window, fresh) = created.borrow()[0].clone();
    fresh.read_with(cx, |workspace, _| {
        assert!(workspace.account.is_none() && workspace.code.is_none());
        assert_eq!(workspace.browser_return.is_some(), browser_first);
        assert_eq!(workspace.files_return.is_some(), !browser_first);
    });
    cx.cx.update(|cx| {
        if browser_first {
            super::super::returning::request(&files_key, cx);
        } else {
            crate::workspace::browser_detach::return_to_session(&browser_key, cx);
        }
    });
    cx.run_until_parked();
    assert_eq!(
        created.borrow().len(),
        1,
        "the second tool must reuse the original pending source destination"
    );
    draw(window, cx);
    fresh.read_with(cx, |workspace, _| {
        assert!(workspace.account.is_none() && workspace.code.is_none());
        assert!(workspace.browser_return.is_some() && workspace.files_return.is_some());
        assert!(workspace.fixture_session_browser(THREAD).is_none());
    });
    assert!(
        attached(&fresh, cx).is_none(),
        "unready destination cannot receive Files"
    );
    assert!(page_probe.alive() && page_probe.parked());
    old_browser.read_with(cx, |browser, _| assert!(browser.is_retired()));
    old_files.read_with(cx, |files, _| assert!(files.is_retired()));
    window
        .update(&mut cx.cx, |_, window, cx| {
            fresh.update(cx, |workspace, cx| {
                // Exercise Account identity and primary journal adoption without starting live services.
                workspace.accept_navigation_identity(&account, cx);
                workspace.gateway_account_authority = Some(proof);
                workspace.account = Some(account);
                install_source(workspace, &source.source(), &rpc);
                workspace.files_fixture_runs = None;
                workspace.browser_detach_runs = None;
                assert!(
                    workspace.restore_primary_terminals(window, cx),
                    "actual retained primary is adopted"
                );
                assert!(
                    workspace.browser_return.is_some() && workspace.files_return.is_some(),
                    "internal first-Account adoption must preserve both queued tools"
                );
            });
        })
        .expect("accepted source metadata arrives in the real fresh window");
    draw(window, cx);
    fresh.read_with(cx, |workspace, _| {
        assert!(workspace.browser_return.is_none() && workspace.files_return.is_none());
        assert!(matches!(&workspace.view, crate::workspace::View::Code(view) if view.id == THREAD));
    });
    assert!(attached(&fresh, cx).is_some());
    assert_eq!(browser(&source, cx).0.window_id(), window.window_id());
    assert!(page_probe.alive() && !page_probe.parked());
    assert_eq!(created.borrow().len(), 1);
}

#[gpui_kit::test]
fn files_then_browser_return_share_one_account_and_reader_pending_source(cx: &mut TestAppContext) {
    pending_account_returns_coalesce(false, cx);
}

#[gpui_kit::test]
fn browser_then_files_return_share_one_account_and_reader_pending_source(cx: &mut TestAppContext) {
    pending_account_returns_coalesce(true, cx);
}
