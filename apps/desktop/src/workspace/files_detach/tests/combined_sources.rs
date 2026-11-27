//! Equal session IDs and one account do not authorize sharing a pending return destination across stores.
use super::*;
use super::{combined_tools::fresh_source_factory, lifecycle::isolate_navigation, source_ingress::draw};
use crate::{backend::ExecutionAuthority, gateway::Account};

fn tabs(
    source: &SourceStore,
    cx: &mut VisualTestContext,
) -> (AnyWindowHandle, Entity<crate::side::browser::Browser>, Value) {
    let valid: crate::side::browser::agent::Guard = Arc::new(|| true);
    let (window, browser, guard) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid, cx))
        .expect("actual source-qualified Browser route");
    let task = window
        .update(&mut cx.cx, |_, window, cx| {
            browser.update(cx, |browser, cx| {
                browser.agent(
                    "browser_tabs".into(),
                    json!({}),
                    "different-source-return".into(),
                    guard,
                    window,
                    cx,
                )
            })
        })
        .expect("live native Browser presentation");
    cx.run_until_parked();
    let answer = futures::FutureExt::now_or_never(task)
        .expect("real tabs request completed")
        .expect("authorized Browser answers");
    (window, browser, answer)
}
fn accept_source(
    window: AnyWindowHandle,
    workspace: &Entity<Workspace>,
    source: &SourceStore,
    rpc: &Rpc,
    account: &Account,
    proof: &ExecutionAuthority,
    primary: bool,
    cx: &mut VisualTestContext,
) {
    window
        .update(&mut cx.cx, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.accept_navigation_identity(account, cx);
                workspace.gateway_account_authority = Some(proof.clone());
                workspace.account = Some(account.clone());
                install_source(workspace, &source.source(), rpc);
                workspace.files_fixture_runs = None;
                workspace.browser_detach_runs = None;
                assert!(
                    workspace.runs(THREAD).is_none(),
                    "fresh runtime readiness must not grant ownership"
                );
                assert_eq!(
                    workspace.restore_primary_terminals(window, cx),
                    primary,
                    "only the real primary adopts its own parked journal"
                );
            })
        })
        .expect("accepted Account and actual concrete source arrive in the selected destination");
}

#[gpui_kit::test]
fn equal_ids_in_different_stores_return_files_and_browser_to_two_distinct_pending_destinations(
    cx: &mut TestAppContext,
) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let files_source = SourceStore::new(&probe);
    let browser_source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    std::fs::write(
        files_source.folder.join("hello.rs"),
        "FILES SOURCE A\nsecond original line\n",
    )
    .expect("distinct real source A bytes");
    std::fs::write(browser_source.folder.join("hello.rs"), "ORDINARY FILES SOURCE B\n")
        .expect("distinct real source B bytes");
    let (files_workspace, original_files, cx) = setup(cx, &probe, &files_source, &rpc);
    isolate_navigation(&files_workspace, cx);
    let files_window = files_workspace.update_in(cx, |_, window, _| window.window_handle());
    original_files.update_in(cx, |files, window, cx| {
        files.open_at("hello.rs".into(), Some(2), window, cx)
    });
    cx.run_until_parked();
    let original_selection = original_files.read_with(cx, |files, cx| files.snapshot(cx));
    let files_key = key(&files_workspace, cx);
    let (files_owner, _, old_detached_files) = detach(&files_workspace, cx);
    let (browser_workspace, original_b_files, cx) = setup(cx, &probe, &browser_source, &rpc);
    isolate_navigation(&browser_workspace, cx);
    let (account, proof) = browser_workspace.read_with(cx, |workspace, _| {
        (
            workspace.account.clone().expect("actual accepted same Account"),
            workspace
                .gateway_account_authority
                .clone()
                .expect("actual accepted receipt"),
        )
    });
    assert!(
        files_workspace.read_with(cx, |a, _| a.terminal_identity.clone())
            == browser_workspace.read_with(cx, |b, _| b.terminal_identity.clone()),
        "both sources deliberately use exactly one account"
    );
    let (page, page_probe) = crate::web::fixture_page(82559);
    let (browser_window, old_attached_browser) = browser_workspace.update_in(cx, |workspace, window, cx| {
        let browser = workspace.fixture_local_browser(THREAD, page, window, cx);
        workspace
            .detach_browser(window, cx)
            .expect("detach actual Browser B through production ownership");
        (window.window_handle(), browser)
    });
    cx.run_until_parked();
    let browser_key = browser_workspace.read_with(cx, |workspace, _| {
        workspace
            .browser_detach_key(THREAD)
            .expect("production Browser source key")
    });
    assert_eq!(files_key.session, browser_key.session);
    assert!(
        files_key.source != browser_key.source,
        "only the concrete source differs"
    );
    let (_, old_detached_browser, before_tabs) = tabs(&browser_source, cx);
    files_window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual source A native close");
    browser_window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual source B native close");
    cx.run_until_parked();
    assert!(original_b_files.read_with(cx, |files, _| files.is_retired()));
    assert!(old_attached_browser.read_with(cx, |browser, _| browser.is_retired()));
    let made = fresh_source_factory(&probe, &files_source, &rpc, false, cx);
    cx.cx.update(|cx| super::super::returning::request(&files_key, cx));
    cx.run_until_parked();
    assert_eq!(made.borrow().len(), 1);
    cx.cx
        .update(|cx| crate::workspace::browser_detach::return_to_session(&browser_key, cx));
    cx.run_until_parked();
    assert_eq!(
        made.borrow().len(),
        2,
        "matching account and session ID must not coalesce different source stores"
    );
    let (a_window, a) = made.borrow()[0].clone();
    let (b_window, b) = made.borrow()[1].clone();
    assert_ne!(a_window.window_id(), b_window.window_id());
    a.read_with(cx, |workspace, _| {
        assert!(workspace.account.is_none() && workspace.code.is_none());
        assert!(
            workspace
                .files_return
                .as_ref()
                .is_some_and(|ticket| ticket.key == files_key)
        );
        assert!(workspace.browser_return.is_none());
    });
    b.read_with(cx, |workspace, _| {
        assert!(workspace.account.is_none() && workspace.code.is_none());
        assert!(
            workspace
                .browser_return
                .as_ref()
                .is_some_and(|ticket| ticket.key == browser_key)
        );
        assert!(workspace.files_return.is_none());
    });
    let mut a_visual = VisualTestContext::from_window(a_window, &cx.cx);
    let mut b_visual = VisualTestContext::from_window(b_window, &cx.cx);
    draw(&mut a_visual);
    draw(&mut b_visual);
    assert!(
        attached(&a, &mut a_visual).is_none() && attached(&b, &mut b_visual).is_none(),
        "unverified pending windows receive no private Files"
    );
    assert!(a.read_with(&mut a_visual, |workspace, _| {
        workspace.fixture_session_browser(THREAD).is_none()
    }));
    assert!(b.read_with(&mut b_visual, |workspace, _| {
        workspace.fixture_session_browser(THREAD).is_none()
    }));
    assert!(page_probe.alive() && page_probe.parked());
    accept_source(a_window, &a, &files_source, &rpc, &account, &proof, true, cx);
    draw(&mut a_visual);
    let returned_a = attached(&a, &mut a_visual).expect("source A receives its own retained Files");
    returned_a.read_with(&mut a_visual, |files, cx| {
        assert_eq!(files.snapshot(cx), original_selection);
        assert_eq!(
            files.fixture_preview_text(cx).as_deref(),
            Some("FILES SOURCE A\nsecond original line\n")
        );
    });
    assert!(a.read_with(&mut a_visual, |workspace, _| {
        workspace.fixture_session_browser(THREAD).is_none()
    }));
    assert!(
        b.read_with(&mut b_visual, |workspace, _| workspace.account.is_none()
            && workspace.browser_return.is_some()),
        "source A readiness cannot adopt source B's Browser"
    );
    assert!(page_probe.alive() && page_probe.parked());
    accept_source(b_window, &b, &browser_source, &rpc, &account, &proof, false, cx);
    draw(&mut b_visual);
    let (routed, returned_browser, after_tabs) = tabs(&browser_source, &mut b_visual);
    assert_eq!(routed.window_id(), b_window.window_id());
    assert_ne!(returned_browser.entity_id(), old_detached_browser.entity_id());
    assert_eq!(after_tabs["tabs"], before_tabs["tabs"]);
    assert!(page_probe.alive() && !page_probe.parked());
    assert!(old_detached_browser.read_with(&mut b_visual, |browser, _| browser.is_retired()));
    assert!(old_detached_files.read_with(&mut a_visual, |files, _| files.is_retired()));
    // A Browser session may legitimately construct ordinary attached Files for its own store.
    // Exercise that real path and prove it cannot receive the retained owner or bytes from A.
    b.update_in(&mut b_visual, |workspace, window, cx| {
        workspace.open_in_files("hello.rs".into(), Some(1), window, cx)
    });
    b_visual.run_until_parked();
    draw(&mut b_visual);
    let ordinary_b = attached(&b, &mut b_visual).expect("ordinary source B Files remains usable");
    assert_ne!(ordinary_b.entity_id(), returned_a.entity_id());
    ordinary_b.read_with(&mut b_visual, |files, cx| {
        assert_eq!(
            files.fixture_preview_text(cx).as_deref(),
            Some("ORDINARY FILES SOURCE B\n")
        )
    });
    a_visual.cx.update(|cx| {
        let registry = cx.global::<Registry>();
        assert_eq!(registry.entries.len(), 1, "only Files A has retained ownership");
        let entry = registry.id(files_owner).expect("original retained Files owner");
        assert!(entry.origin.key == files_key);
        assert!(matches!(
            &entry.presentation,
            Presentation::Attached { window, files }
                if window.window_id() == a_window.window_id()
                    && files.upgrade().is_some_and(|files| files.entity_id() == returned_a.entity_id())
        ));
    });
    assert!(a.read_with(&mut a_visual, |workspace, _| {
        workspace.fixture_session_browser(THREAD).is_none()
    }));
    assert_eq!(made.borrow().len(), 2);
}
