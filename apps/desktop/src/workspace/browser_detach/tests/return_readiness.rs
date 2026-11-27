//! A concrete store reader can become ready before a replacement Workspace's runner.
use super::{
    BrowserEvent, Presentation, Probe, Registry, Rpc, SourceStore, THREAD, agent_tabs, emit, fixture, install_source,
    key, owner, select, setup,
};
use gpui_kit::{AppContext, TestAppContext};
use std::sync::Arc;

#[gpui_kit::test]
fn parked_pages_rebind_to_frozen_owner_before_fresh_runner_readiness(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let other_source = SourceStore::new(&probe);
    let original_rpc = Rpc::default();
    let replacement_rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6401);
    let (primary, old_browser, cx) = setup(cx, &probe, &source, &original_rpc, page);
    let resource = key(&primary, cx);
    let original_owner = owner(&resource, cx);
    let (page, _other_page) = crate::web::fixture_page(6402);
    let (_other, _, cx) = setup(cx, &probe, &other_source, &Rpc::default(), page);
    primary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    assert!(page_probe.alive() && page_probe.parked());
    let (fresh, cx) = fixture(cx, &probe);
    let window = fresh.update_in(cx, |workspace, window, cx| {
        install_source(workspace, &source.source(), &replacement_rpc);
        workspace.browser_detach_runs = None;
        assert!(
            workspace.runs(THREAD).is_none(),
            "fresh runner genuinely has not arrived"
        );
        select(workspace, THREAD, window, cx);
        window.window_handle()
    });
    cx.cx.update(|cx| {
        crate::windows::retention_fixtures::register(Arc::new(probe.clone()), window, fresh.clone(), false, cx)
    });
    assert!(fresh.update_in(cx, |workspace, window, cx| {
        workspace.restore_primary_terminals(window, cx)
    }));
    cx.run_until_parked();
    assert!(page_probe.alive() && !page_probe.parked());
    let (routed, browser, answer) = agent_tabs(&source.origin(), cx);
    assert_eq!(routed.window_id(), window.window_id());
    assert_ne!(browser.entity_id(), old_browser.entity_id());
    assert_eq!(answer["tabs"].as_array().expect("real browser_tabs answer").len(), 1);
    cx.cx.update(|cx| {
        let entry = cx.global::<Registry>().key(&resource).expect("same retained owner");
        assert_eq!(entry.id, original_owner);
        assert!(matches!(&entry.presentation, Presentation::Attached { .. }));
        assert_eq!(
            cx.global::<Registry>()
                .entries
                .iter()
                .filter(|entry| entry.origin.key == resource)
                .count(),
            1
        );
    });
    fresh.update_in(cx, |workspace, window, cx| {
        workspace.browser_detach_runs = Some(replacement_rpc.runs(source.source()));
        workspace.sync_side(window, cx);
    });
    cx.run_until_parked();
    let (_, later_browser, _) = agent_tabs(&source.origin(), cx);
    assert_eq!(later_browser.entity_id(), browser.entity_id());
    emit(&browser, BrowserEvent::StopAgent, cx);
    let stops = original_rpc
        .calls()
        .into_iter()
        .filter(|(name, _)| *name == "wb_interrupt")
        .collect::<Vec<_>>();
    assert_eq!(stops.len(), 1, "retained source transport remains the execution owner");
    assert_eq!(stops[0].1["threadId"], THREAD);
    assert!(replacement_rpc.calls().iter().all(|(name, _)| *name != "wb_interrupt"));
    assert!(page_probe.alive() && !page_probe.parked());
}

#[gpui_kit::test]
fn first_account_journal_adoption_preserves_return_until_real_render(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6403);
    let (primary, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (account, proof) = primary.read_with(cx, |workspace, _| {
        (
            workspace.account.clone().expect("verified Account receipt"),
            workspace
                .gateway_account_authority
                .clone()
                .expect("accepted Account epoch"),
        )
    });
    let (id, browser_window, _) = super::detach(&primary, cx);
    primary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    let created = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let record = created.clone();
    let (fresh_probe, source_desc, fresh_rpc) = (probe.clone(), source.source(), rpc.clone());
    cx.cx.update(|cx| {
        cx.global_mut::<Registry>().app_window_factory = Some(std::rc::Rc::new(move |cx| {
            let (window, workspace) = gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut workspace = crate::workspace::gateway_chat::tests::fixture_model(window, cx, &fresh_probe);
                    install_source(&mut workspace, &source_desc, &fresh_rpc);
                    // Publish this fresh presentation before Account readiness, not with pre-seeded authority.
                    workspace.account = None;
                    workspace.terminal_identity = None;
                    workspace.gateway_account_authority = None;
                    workspace.browser_detach_runs = None;
                    workspace
                })
            })
            .ok()?;
            crate::windows::retention_fixtures::register(
                Arc::new(fresh_probe.clone()),
                window,
                workspace.clone(),
                false,
                cx,
            );
            record.borrow_mut().push((window, workspace.clone()));
            Some((window, workspace))
        }));
    });
    cx.cx
        .update(|cx| super::returning::return_from(id, browser_window.window_id(), false, cx));
    cx.run_until_parked();
    assert_eq!(created.borrow().len(), 1);
    let (window, fresh) = created.borrow()[0].clone();
    fresh.read_with(cx, |workspace, _| {
        assert!(workspace.account.is_none());
        assert!(workspace.terminal_identity.is_none());
        assert!(workspace.gateway_account_authority.is_none());
        assert!(
            workspace.browser_return.is_some(),
            "real native Return queued before Account"
        );
    });
    window
        .update(&mut cx.cx, |_, window, cx| {
            fresh.update(cx, |workspace, cx| {
                // The production Account identity/adoption helpers are shared with handle(Account).
                // Resource startup is deliberately excluded: it opens user stores and starts live Code services.
                workspace.accept_navigation_identity(&account, cx);
                workspace.gateway_account_authority = Some(proof);
                workspace.account = Some(account);
                assert!(
                    workspace.restore_primary_terminals(window, cx),
                    "actual saved primary adopted"
                );
                assert!(
                    workspace.browser_return.is_some(),
                    "internal journal visit cannot cancel Return"
                );
                assert!(
                    workspace.runs(THREAD).is_none(),
                    "retained owner does not require fresh Runs"
                );
            })
        })
        .expect("fresh native account-adoption update");
    for _ in 0..3 {
        window
            .update(&mut cx.cx, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.simulate_next_frame(cx);
            })
            .expect("real fresh native render");
        cx.run_until_parked();
    }
    fresh.read_with(cx, |workspace, _| {
        assert!(
            workspace.browser_return.is_none(),
            "render applied the original Return ticket"
        );
        assert!(matches!(&workspace.view, crate::workspace::View::Code(view) if view.id == THREAD));
        assert!(workspace.has_browser(THREAD));
    });
    assert!(page_probe.alive() && !page_probe.parked());
    assert_eq!(agent_tabs(&source.origin(), cx).0.window_id(), window.window_id());
}
