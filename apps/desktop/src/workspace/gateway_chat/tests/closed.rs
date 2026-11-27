//! Real installed windows::closed callback; stale strong views do not prolong command authority.
use super::{Probe, ask, delta, fixture, named};
use crate::workspace::Workspace;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use std::sync::Arc;
fn register(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, probe: &Probe, panel: bool) {
    let window = workspace.update_in(cx, |_, window, _| window.window_handle());
    let workspace = workspace.clone();
    cx.cx.update(|cx| {
        crate::windows::retention_fixtures::register(Arc::new(probe.clone()), window, workspace, panel, cx)
    });
}
fn stale_commands(old: &Entity<Workspace>, current: &Entity<Workspace>, cx: &mut VisualTestContext) {
    current.update_in(cx, |_, window, cx| {
        old.update(cx, |old, cx| {
            old.ask("stale send".into(), Default::default(), cx);
            old.enqueue_chat("stale queue".into(), Default::default(), true, window, cx);
            old.stop(window, cx);
        })
    });
}
#[gpui_kit::test]
fn real_secondary_close_revokes_execution_even_with_a_strong_old_workspace(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (primary, cx) = fixture(cx, &probe);
    register(&primary, cx, &probe, false);
    let a = ask(&primary, cx, &probe, "primary A");
    let (secondary, cx) = fixture(cx, &probe);
    register(&secondary, cx, &probe, false);
    let b = ask(&secondary, cx, &probe, "secondary B");
    secondary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    assert_eq!(probe.stops(), vec![(b.side, b.turn)]);
    stale_commands(&secondary, &primary, cx);
    assert_eq!(probe.asks().len(), 2);
    assert_eq!(probe.stops(), vec![(b.side, b.turn)]);
    delta(&probe, &a, "primary still active");
    cx.run_until_parked();
    primary.read_with(cx, |workspace, _| {
        assert_eq!(
            workspace.entries.last().expect("primary answer").streamed,
            "primary still active"
        )
    });
}
#[gpui_kit::test]
fn real_primary_park_retires_old_view_without_revoking_moved_execution(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (primary, cx) = fixture(cx, &probe);
    register(&primary, cx, &probe, false);
    let a = ask(&primary, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    let (guard, cx) = fixture(cx, &probe);
    register(&guard, cx, &probe, false);
    primary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    assert!(probe.stops().is_empty());
    stale_commands(&primary, &guard, cx);
    assert_eq!(probe.asks().len(), 1);
    assert!(probe.stops().is_empty());
    delta(&probe, &a, "while closed");
    cx.run_until_parked();
    let (fresh, cx) = fixture(cx, &probe);
    register(&fresh, cx, &probe, false);
    fresh.update_in(cx, |workspace, window, cx| {
        let identity = workspace.terminal_identity.clone().expect("fresh accepted identity");
        let saved = crate::windows::take_primary_terminals(window.window_handle().window_id(), &identity, cx)
            .expect("actual registry parked owner");
        assert!(workspace.apply_primary_retained(saved, window, cx));
        assert_eq!(workspace.asking, Some(a.turn));
        assert_eq!(workspace.entries.last().expect("fresh answer").streamed, "while closed");
        workspace.stop(window, cx);
    });
    assert_eq!(probe.stops(), vec![(a.side, a.turn)]);
}
#[gpui_kit::test]
fn real_panel_close_revokes_execution_even_with_a_strong_old_workspace(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (primary, cx) = fixture(cx, &probe);
    register(&primary, cx, &probe, false);
    let (panel, cx) = fixture(cx, &probe);
    panel.update(cx, |workspace, _| workspace.panel = true);
    register(&panel, cx, &probe, true);
    let a = ask(&panel, cx, &probe, "panel A");
    named(&probe, &a, "panel-a");
    cx.run_until_parked();
    panel.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    assert_eq!(probe.stops(), vec![(a.side, a.turn)]);
    stale_commands(&panel, &primary, cx);
    assert_eq!(probe.asks().len(), 1);
    assert_eq!(probe.stops(), vec![(a.side, a.turn)]);
}
#[gpui_kit::test]
fn real_last_no_tray_close_revokes_captured_binding_before_app_quit(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    register(&workspace, cx, &probe, false);
    let a = ask(&workspace, cx, &probe, "last A");
    let binding = workspace.read_with(cx, |workspace, _| {
        workspace.chat_execution.clone().expect("live binding")
    });
    workspace.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    // Keep both old Workspace and owner references alive across actual no-tray close/quit.
    assert_eq!(probe.stops(), vec![(a.side, a.turn)]);
    let mut state = binding.state.borrow_mut();
    assert!(state.revoked);
    assert!(state.runs.is_empty());
    assert!(!state.permits(binding.incarnation));
}
