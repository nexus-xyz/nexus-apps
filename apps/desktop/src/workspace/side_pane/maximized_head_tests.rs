//! Headless: the docked Workbench closes from its own head without hiding chat. The maximized head keeps both its
//! restore and close buttons inside the title bar.
use super::*;
use crate::workspace::{code_view::CodeView, machine_source::MachineSource};
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, VisualTestContext};

const ID: &str = "maximized-head";

fn workbench(cx: &mut TestAppContext, maximized: bool) -> (Entity<Workspace>, &mut VisualTestContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    cx.update(gpui_kit::init);
    let (gateway, _) = crate::gateway::Gateway::start();
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            std::sync::Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = Task::ready(());
        workspace.code = None;
        workspace.runner = None;
        workspace.auth = crate::workspace::Auth::SignedIn;
        workspace.sidebar_open = true;
        let mut view = CodeView::new(ID.into(), window, cx);
        view.loaded = true;
        workspace.view = View::Code(view);
        workspace.side.fixture_session(ID);
        workspace.side.open = true;
        workspace.side.select_tool(Tool::Files);
        workspace.side.maximized = maximized;
        workspace
    });
    workspace.update_in(cx, |_, window, _| window.activate_window());
    draw(cx);
    // The slide runs on the motion's clock: draw again once it has settled.
    cx.executor().advance_clock(crate::motion::PANEL * 2);
    draw(cx);
    (workspace, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui_kit::test]
fn the_maximized_head_keeps_its_restore_and_close_buttons(cx: &mut TestAppContext) {
    let (_, cx) = workbench(cx, true);
    let viewport = cx.update(|window, _| f32::from(window.viewport_size().width));
    let restore = cx
        .debug_bounds("side-maximize")
        .expect("the maximized head draws its restore button");
    assert!(
        f32::from(restore.right()) <= viewport,
        "the restore button sits inside the window: {restore:?}, viewport {viewport}"
    );
    let close = cx
        .debug_bounds("side-close")
        .expect("the maximized head draws its close button");
    assert!(
        f32::from(close.right()) <= viewport,
        "the close button sits inside the window: {close:?}, viewport {viewport}"
    );
    assert!(
        cx.debug_bounds("toggle-side-pane").is_none(),
        "the open button is hidden while the pane is open"
    );
}

#[gpui_kit::test]
fn the_docked_head_closes_the_workbench_instead_of_shrinking_chat(cx: &mut TestAppContext) {
    let (workspace, cx) = workbench(cx, false);
    let close = cx
        .debug_bounds("side-close")
        .expect("the docked head draws its close button");
    assert!(
        cx.debug_bounds("side-maximize").is_some(),
        "the maximize button stays available"
    );
    assert!(
        cx.debug_bounds("toggle-side-pane").is_none(),
        "the open button is hidden while the pane is open"
    );

    cx.simulate_click(close.center(), gpui_kit::Modifiers::none());
    draw(cx);
    workspace.read_with(cx, |workspace, _| {
        assert!(!workspace.side.open, "the Workbench closes");
        assert!(!workspace.side.maximized);
        assert!(matches!(workspace.view, View::Code(_)), "the chat stays selected");
    });
    cx.executor().advance_clock(crate::motion::PANEL * 2);
    draw(cx);
    assert!(cx.debug_bounds("code-title").is_some(), "the chat title stays visible");
    assert!(
        cx.debug_bounds("side-close").is_none(),
        "the Workbench head leaves the screen"
    );
    assert!(
        cx.debug_bounds("toggle-side-pane").is_some(),
        "the session head offers to reopen the Workbench"
    );
}
