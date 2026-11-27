//! Real source/native Return chips and source-qualified picker routing.
use super::lifecycle::{isolate_navigation, selection};
use super::*;
use gpui_kit::{Modifiers, px};
pub(super) fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.simulate_next_frame(cx);
    });
    cx.run_until_parked();
    cx.executor().advance_clock(crate::motion::PANEL);
    cx.run_until_parked();
    for _ in 0..3 {
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
    }
}
pub(super) fn click(selector: &'static str, cx: &mut VisualTestContext) {
    draw(cx);
    let bounds = cx.debug_bounds(selector).expect("actual visible native control");
    let viewport = cx.update(|window, _| window.viewport_size());
    let center = bounds.center();
    assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
    assert!(
        center.x >= px(0.) && center.y >= px(0.) && center.x < viewport.width && center.y < viewport.height,
        "control must be inside the native viewport after PANEL completes"
    );
    cx.simulate_click(center, Modifiers::none());
    cx.run_until_parked();
}
fn returned(
    workspace: &Entity<Workspace>,
    owner: u64,
    old: &Entity<Files>,
    cx: &mut VisualTestContext,
) -> Entity<Files> {
    old.read_with(cx, |files, _| {
        assert!(files.is_retired(), "click immediately admitted Return")
    });
    cx.cx.update(|cx| {
        assert!(
            !matches!(
                &cx.global::<Registry>().id(owner).expect("same owner").presentation,
                Presentation::Detached { .. }
            ),
            "click cannot be a no-op"
        )
    });
    draw(cx);
    let fresh = attached(workspace, cx).expect("real render adopted exact returning Files");
    assert_ne!(fresh.entity_id(), old.entity_id());
    fresh
}
#[gpui_kit::test]
fn actual_source_return_chip_consumes_the_exact_snapshot_once(cx: &mut TestAppContext) {
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
    let (owner, window, old) = detach(&workspace, cx);
    click("restore-session-files", cx);
    let fresh = returned(&workspace, owner, &old, cx);
    fresh.read_with(cx, |files, cx| assert_eq!(files.snapshot(cx), before));
    assert!(window.update(&mut cx.cx, |_, _, _| ()).is_err());
    cx.cx
        .update(|cx| super::super::returning::return_from(owner, window.window_id(), false, cx));
    draw(cx);
    assert_eq!(
        attached(&workspace, cx).expect("same fresh owner").entity_id(),
        fresh.entity_id()
    );
}
#[gpui_kit::test]
fn actual_detached_return_button_closes_the_old_native_presentation_once(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (workspace, original, cx) = setup(cx, &probe, &source, &rpc);
    isolate_navigation(&workspace, cx);
    let before = selection(&workspace, &original, &source, cx);
    let (owner, window, old) = detach(&workspace, cx);
    let mut detached_cx = VisualTestContext::from_window(window, &cx.cx);
    click("return-files-to-session", &mut detached_cx);
    let fresh = returned(&workspace, owner, &old, cx);
    fresh.read_with(cx, |files, cx| assert_eq!(files.snapshot(cx), before));
    assert!(window.update(&mut cx.cx, |_, _, _| ()).is_err());
}
#[gpui_kit::test]
fn picker_forwarding_with_identical_session_ids_keeps_original_store_and_other_view_unchanged(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let a = SourceStore::new(&probe);
    let b = SourceStore::new(&probe);
    let rpc = Rpc::default();
    std::fs::write(a.folder.join("shared.rs"), "source A first\nsource A second\n").expect("actual source A file");
    std::fs::write(b.folder.join("shared.rs"), "source B only\n").expect("actual source B file");
    let (source, _, cx) = setup(cx, &probe, &a, &rpc);
    isolate_navigation(&source, cx);
    let source_window = source.update_in(cx, |_, window, _| window.window_handle());
    let (owner, detached_window, moved) = detach(&source, cx);
    let (other, other_files, cx) = setup(cx, &probe, &b, &rpc);
    isolate_navigation(&other, cx);
    other_files.update_in(cx, |files, window, cx| {
        files.open_at("shared.rs".into(), Some(1), window, cx)
    });
    cx.run_until_parked();
    let before = other_files.read_with(cx, |files, cx| files.snapshot(cx));
    source_window
        .update(&mut cx.cx, |_, window, cx| {
            source.update(cx, |source, cx| {
                source.open_in_files("shared.rs".into(), Some(2), window, cx)
            })
        })
        .expect("actual picker route from original source");
    cx.run_until_parked();
    moved.read_with(cx, |files, cx| {
        let state = files.snapshot(cx);
        assert_eq!(state.open.as_ref().expect("forwarded actual file").line, Some(2));
        assert_eq!(
            files.fixture_preview_text(cx).as_deref(),
            Some("source A first\nsource A second\n")
        );
    });
    other_files.read_with(cx, |files, cx| {
        assert_eq!(files.snapshot(cx), before);
        assert_eq!(files.fixture_preview_text(cx).as_deref(), Some("source B only\n"));
    });
    assert_eq!(detached(owner, cx).0.window_id(), detached_window.window_id());
    assert!(
        attached(&source, cx).is_none(),
        "forwarding cannot create a duplicate attached view"
    );
}
