//! Source pane Return is a real native ingress, not a direct state-machine call.
use super::{Presentation, Probe, Registry, Rpc, SourceStore, THREAD, detach, key, setup};
use gpui_kit::{Modifiers, TestAppContext, px};

#[gpui_kit::test]
fn source_return_browser_click_transfers_the_live_detached_pages_once(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6301);
    let (workspace, original, cx) = setup(cx, &probe, &source, &rpc, page);
    // New sessions start closed; open this tool before testing its native controls.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.pick_tool(crate::workspace::side_pane::Tool::Browser, window, cx);
    });
    let resource = key(&workspace, cx);
    let (owner, detached_window, detached_browser) = detach(&workspace, cx);
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.simulate_next_frame(cx);
    });
    cx.run_until_parked();
    // Finish the real opening timer before hit-testing; three fast frames can still be clipped.
    cx.executor().advance_clock(crate::motion::PANEL);
    cx.run_until_parked();
    workspace.update(cx, |workspace, cx| {
        assert!(!workspace.sync_side_motion(cx).expect("visible source pane").moving);
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.simulate_next_frame(cx);
    });
    cx.run_until_parked();
    let bounds = cx
        .debug_bounds("restore-session-browser")
        .expect("visible source Return Browser control");
    let viewport = cx.update(|window, _| window.viewport_size());
    let point = bounds.center();
    assert!(
        bounds.size.width > px(0.)
            && bounds.size.height > px(0.)
            && point.x >= px(0.)
            && point.x < viewport.width
            && point.y >= px(0.)
            && point.y < viewport.height,
        "Return control must be inside the source viewport: {bounds:?}, {viewport:?}"
    );
    cx.cx.update(|cx| {
        let entry = cx.global::<Registry>().key(&resource).expect("detached source owner");
        assert!(entry.current());
        assert!(matches!(&entry.presentation, Presentation::Detached { .. }));
    });
    cx.simulate_click(point, Modifiers::none());
    cx.run_until_parked();
    cx.cx.update(|cx| {
        let entry = cx
            .global::<Registry>()
            .key(&resource)
            .expect("same owner after source click");
        assert!(entry.current());
        assert!(
            matches!(
                &entry.presentation,
                Presentation::Returning { .. } | Presentation::Attached { .. }
            ),
            "the actual source click must initiate return before later renders"
        );
    });
    // Apply the real deferred return ticket through the production render path.
    for _ in 0..3 {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_next_frame(cx);
        });
        cx.run_until_parked();
    }
    let restored = workspace.read_with(cx, |workspace, _| {
        assert!(workspace.browser_return.is_none());
        assert!(matches!(&workspace.view, crate::workspace::View::Code(view) if view.id == THREAD));
        workspace
            .fixture_session_browser(THREAD)
            .expect("returned source Browser")
    });
    assert_ne!(restored.entity_id(), original.entity_id());
    assert_ne!(restored.entity_id(), detached_browser.entity_id());
    detached_browser.read_with(cx, |browser, _| assert!(browser.is_retired()));
    assert!(page_probe.alive() && !page_probe.parked());
    assert!(
        detached_window.update(&mut cx.cx, |_, _, _| ()).is_err(),
        "old native presentation closed"
    );
    cx.cx.update(|cx| {
        assert!(matches!(
            &cx.global::<Registry>()
                .key(&resource)
                .expect("same canonical owner")
                .presentation,
            Presentation::Attached { .. }
        ));
        assert_eq!(cx.global::<Registry>().key(&resource).expect("one owner").id, owner);
    });
    restored.read_with(cx, |browser, _| assert_eq!(browser.tabs().0.len(), 1));
}

#[gpui_kit::test]
fn authenticated_fixture_survives_real_source_profile_cleanup_before_detach(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, page_probe) = crate::web::fixture_page(6302);
    let (workspace, browser, cx) = setup(cx, &probe, &source, &rpc, page);
    let source_window = workspace.update_in(cx, |_, window, _| window.window_handle());
    for _ in 0..3 {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_next_frame(cx);
        });
        cx.run_until_parked();
    }
    let current = workspace.read_with(cx, |workspace, _| {
        workspace
            .fixture_session_browser(THREAD)
            .expect("profile cleanup keeps the valid source Browser")
    });
    assert_eq!(
        current.entity_id(),
        browser.entity_id(),
        "render must not recreate or discard its authenticated fixture"
    );
    assert!(page_probe.alive() && !page_probe.parked());
    browser.read_with(cx, |browser, _| assert!(!browser.is_retired()));
    let (routed, same, answer) = super::agent_tabs(&source.origin(), cx);
    assert_eq!(same.entity_id(), browser.entity_id());
    assert_eq!(routed.window_id(), source_window.window_id());
    assert_eq!(
        answer["tabs"]
            .as_array()
            .expect("actual source browser_tabs answer")
            .len(),
        1
    );
}
