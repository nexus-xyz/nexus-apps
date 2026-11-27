//! These fixtures use the installed close observer, including callbacks holding stale strong entities.
use super::*;

#[gpui_kit::test]
fn detach_rebinds_live_page_and_repeat_action_focuses_the_same_presentation(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6101);
    let (workspace, old, cx) = setup(cx, &probe, &source, &rpc, page);
    let (_, _, before) = agent_tabs(&source.origin(), cx);
    let (id, detached_window, browser) = detach(&workspace, cx);
    assert!(page_probe.alive() && !page_probe.parked());
    assert_ne!(old.entity_id(), browser.entity_id());
    old.read_with(cx, |browser, _| assert!(browser.is_retired()));
    workspace.read_with(cx, |workspace, _| assert!(!workspace.has_browser(THREAD)));
    let (routed, current, after) = agent_tabs(&source.origin(), cx);
    assert_eq!(routed.window_id(), detached_window.window_id());
    assert_eq!(current.entity_id(), browser.entity_id());
    assert_eq!(
        after["tabs"], before["tabs"],
        "moving must preserve tab IDs, selection and page state"
    );
    workspace
        .update_in(cx, |workspace, window, cx| workspace.detach_browser(window, cx))
        .expect("repeat action focuses existing detached window");
    cx.run_until_parked();
    let (again, same) = detached(id, cx);
    assert_eq!(again.window_id(), detached_window.window_id());
    assert_eq!(same.entity_id(), browser.entity_id());
    assert_eq!(cx.cx.update(|cx| cx.global::<Registry>().entries.len()), 1);
}

fn rollback(after_construct: bool, cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(if after_construct { 6103 } else { 6102 });
    let (workspace, old, cx) = setup(cx, &probe, &source, &rpc, page);
    let resource = key(&workspace, cx);
    let (_, _, before) = agent_tabs(&source.origin(), cx);
    cx.cx.update(|cx| {
        if after_construct {
            cx.global_mut::<Registry>().fail_after_construct = true;
        } else {
            cx.global_mut::<Registry>().fail_next_open = true;
        }
    });
    let error = workspace
        .update_in(cx, |workspace, window, cx| workspace.detach_browser(window, cx))
        .expect_err("native open is deliberately rejected");
    assert!(error.contains("returned"));
    cx.run_until_parked();
    assert!(
        page_probe.alive() && !page_probe.parked(),
        "failed move cannot lose its page"
    );
    old.read_with(cx, |browser, _| assert!(browser.is_retired()));
    let fresh = workspace.read_with(cx, |workspace, _| {
        workspace.fixture_session_browser(THREAD).expect("rolled-back browser")
    });
    assert_ne!(old.entity_id(), fresh.entity_id());
    cx.cx.update(|cx| {
        match &cx
            .global::<Registry>()
            .key(&resource)
            .expect("same resource owner")
            .presentation
        {
            Presentation::Attached { browser, .. } => {
                assert_eq!(browser.upgrade().expect("fresh view").entity_id(), fresh.entity_id())
            }
            _ => panic!("rollback must restore exactly one attached presentation"),
        }
    });
    let (_, routed, after) = agent_tabs(&source.origin(), cx);
    assert_eq!(routed.entity_id(), fresh.entity_id());
    assert_eq!(after["tabs"], before["tabs"]);
}
#[gpui_kit::test]
fn failed_open_before_constructor_returns_page_to_a_fresh_source_view(cx: &mut TestAppContext) {
    rollback(false, cx);
}
#[gpui_kit::test]
fn failed_open_after_constructor_recovers_the_consumed_page_once(cx: &mut TestAppContext) {
    rollback(true, cx);
}

#[gpui_kit::test]
fn installed_detached_close_callback_returns_once_and_stale_close_cannot_move_it_again(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6104);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, window, detached_browser) = detach(&workspace, cx);
    window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual native close");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| workspace.sync_browser_return(window, cx));
    cx.run_until_parked();
    let fresh = workspace.read_with(cx, |workspace, _| {
        workspace.fixture_session_browser(THREAD).expect("returned browser")
    });
    assert_ne!(fresh.entity_id(), detached_browser.entity_id());
    detached_browser.read_with(cx, |browser, _| assert!(browser.is_retired()));
    assert!(page_probe.alive() && !page_probe.parked());
    cx.cx
        .update(|cx| returning::return_from(id, window.window_id(), true, cx));
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(
            workspace
                .fixture_session_browser(THREAD)
                .expect("unchanged returned view")
                .entity_id(),
            fresh.entity_id()
        )
    });
    assert_eq!(cx.cx.update(|cx| cx.global::<Registry>().entries.len()), 1);
}

#[gpui_kit::test]
fn primary_native_close_keeps_detached_page_and_rejects_a_strong_stale_workspace(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6105);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, detached_window, browser) = detach(&workspace, cx);
    workspace.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    assert!(page_probe.alive() && !page_probe.parked());
    assert!(cx.cx.update(|cx| crate::workspace::browser_detach::keeps_app_alive(cx)));
    let (routed, current, _) = agent_tabs(&source.origin(), cx);
    assert_eq!(routed.window_id(), detached_window.window_id());
    assert_eq!(current.entity_id(), browser.entity_id());
    let error = detached_window
        .update(&mut cx.cx, |_, window, cx| {
            workspace.update(cx, |workspace, cx| workspace.detach_browser(window, cx))
        })
        .expect("live detached presentation")
        .expect_err("closed source view is permanently retired");
    assert!(error.contains("closed"));
    assert_eq!(detached(id, cx).1.entity_id(), browser.entity_id());
}

#[gpui_kit::test]
fn closing_the_source_releases_its_workspace_and_composer_while_page_stays_alive(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6106);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let weak_workspace = workspace.downgrade();
    let weak_composer = workspace.read_with(cx, |workspace, _| workspace.composer.downgrade());
    let (_, window, _) = detach(&workspace, cx);
    workspace.update_in(cx, |_, window, _| window.remove_window());
    cx.cx.update(|_| drop(workspace));
    cx.run_until_parked();
    assert!(
        weak_workspace.upgrade().is_none(),
        "owner must not keep the old Workspace"
    );
    assert!(
        weak_composer.upgrade().is_none(),
        "owner must not keep the old composer"
    );
    assert!(page_probe.alive());
    assert_eq!(agent_tabs(&source.origin(), cx).0.window_id(), window.window_id());
}

#[gpui_kit::test]
fn account_aba_revokes_before_native_close_and_cannot_return_stale_pages(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6107);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, window, browser) = detach(&workspace, cx);
    let identity = origin(id, cx).key.identity;
    probe.epoch.fetch_add(2, Ordering::SeqCst);
    let valid: crate::side::browser::agent::Guard = Arc::new(|| true);
    assert!(
        cx.cx
            .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid, cx))
            .is_err()
    );
    cx.cx
        .update(|cx| crate::workspace::browser_detach::identity_changed(Some(&identity), cx));
    cx.run_until_parked();
    assert!(!page_probe.alive());
    browser.read_with(cx, |browser, _| assert!(browser.is_retired()));
    assert!(cx.cx.update(|cx| cx.global::<Registry>().entries.is_empty()));
    cx.cx
        .update(|cx| returning::return_from(id, window.window_id(), true, cx));
    workspace.read_with(cx, |workspace, _| assert!(!workspace.has_browser(THREAD)));
}

#[gpui_kit::test]
fn installed_app_quit_hook_retires_detached_owners_before_callbacks_can_reopen_windows(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6108);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, window, browser) = detach(&workspace, cx);
    let creates = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = creates.clone();
    cx.cx.update(|cx| {
        cx.global_mut::<Registry>().app_window_factory = Some(std::rc::Rc::new(move |_| {
            count.set(count.get() + 1);
            None
        }))
    });
    cx.cx.update(|app| app.shutdown());
    cx.run_until_parked();
    assert!(!page_probe.alive());
    browser.read_with(cx, |browser, _| assert!(browser.is_retired()));
    assert_eq!(creates.get(), 0);
    assert!(!cx.cx.update(|cx| crate::workspace::browser_detach::keeps_app_alive(cx)));
    cx.cx
        .update(|cx| returning::return_from(id, window.window_id(), true, cx));
    assert_eq!(creates.get(), 0);
    assert!(cx.cx.update(|cx| cx.global::<Registry>().entries.is_empty()));
}

#[gpui_kit::test]
fn changed_identity_releases_detached_pages_even_while_the_original_proof_is_current(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6109);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, window, browser) = detach(&workspace, cx);
    let previous = origin(id, cx);
    assert!(previous.current());
    let epoch = probe.epoch.load(Ordering::SeqCst);
    let account = crate::gateway::Account::from_json(&json!({
        "tenant": {"id": "org-b", "slug": "org-b", "name": "Another organization", "kind": "company"},
        "user": {"id": "person-b", "personId": "person-b", "name": "B", "email": "b@example.test"},
        "role": "member", "permissions": []
    }))
    .expect("accepted different account shape");
    let next = crate::windows::retention::Identity::current(&probe, &account).expect("different accepted identity");
    assert!(next != previous.key.identity);
    cx.cx
        .update(|cx| crate::workspace::browser_detach::identity_changed(Some(&next), cx));
    cx.run_until_parked();
    assert_eq!(probe.epoch.load(Ordering::SeqCst), epoch);
    assert!(
        previous.current(),
        "this regression isolates the identity comparison from execution epoch revocation"
    );
    assert!(!page_probe.alive());
    browser.read_with(cx, |browser, _| assert!(browser.is_retired()));
    assert!(cx.cx.update(|cx| cx.global::<Registry>().entries.is_empty()));
    cx.cx
        .update(|cx| returning::return_from(id, window.window_id(), true, cx));
    workspace.read_with(cx, |workspace, _| assert!(!workspace.has_browser(THREAD)));
}
