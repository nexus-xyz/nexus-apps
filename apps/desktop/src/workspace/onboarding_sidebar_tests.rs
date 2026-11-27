//! Onboarding has no sidebar: Cmd+B does nothing there, and the sidebar comes back with its own open motion after the
//! flow and after a step opened from the checklist. Headless: no credentials.

use super::Step;
use crate::workspace::{Auth, View, Workspace, machine_source::MachineSource};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use std::sync::Arc;

fn fixture(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _gateway_events) = crate::gateway::Gateway::start();
    let (_send, events) = futures::channel::mpsc::unbounded();
    let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = gpui_kit::Task::ready(());
        workspace
    });
    workspace.update_in(cx, |workspace, _, _| {
        workspace.auth = Auth::SignedIn;
        workspace.view = View::Chat;
        workspace.sidebar_open = true;
        workspace.onboarding.leave();
    });
    (workspace, cx)
}

#[gpui_kit::test]
fn cmd_b_during_the_flow_keeps_the_sidebar_that_returns(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.begin_onboarding(Step::Look, false);
        let toggles = workspace.sidebar_toggles;
        workspace.toggle_sidebar(cx);
        assert!(workspace.sidebar_open, "the person's choice stays");
        assert_eq!(workspace.sidebar_toggles, toggles, "nothing moves");
    });
}

#[gpui_kit::test]
fn the_sidebar_opens_with_its_motion_after_a_checklist_step(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        // A first run set up later: its steps go on the checklist, and the sidebar opens with its motion.
        workspace.restart_onboarding(window, cx);
        let toggles = workspace.sidebar_toggles;
        workspace.onboarding_later(window, cx);
        assert_eq!(workspace.sidebar_toggles, toggles + 1);
        // The checklist's row opens one step, which takes the window; leaving it opens the sidebar the same way.
        workspace.revisit_onboarding(window, cx);
        assert!(workspace.onboarding.active());
        let toggles = workspace.sidebar_toggles;
        workspace.onboarding_later(window, cx);
        assert!(!workspace.onboarding.active());
        assert_eq!(
            workspace.sidebar_toggles,
            toggles + 1,
            "the sidebar slides in, not pops"
        );
    });
}

#[gpui_kit::test]
fn a_closed_sidebar_stays_closed_after_the_flow(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.sidebar_open = false;
        let toggles = workspace.sidebar_toggles;
        workspace.restart_onboarding(window, cx);
        workspace.onboarding_later(window, cx);
        assert!(!workspace.sidebar_open);
        assert_eq!(workspace.sidebar_toggles, toggles, "nothing moves");
    });
}
