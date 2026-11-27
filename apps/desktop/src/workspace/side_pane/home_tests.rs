//! Headless: with no session, the Workbench's tabs are what is open. Terminal and Browser were tabs whether or not
//! they were open, so a × dropped the shell and left its tab, and the toggle started a new shell. "+" then had nothing
//! to reopen. Now a closed tool takes its tab, the toggle opens an empty Workbench that lists what it can open, and
//! "+" offers what is not a tab.
use super::*;
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, VisualTestContext};

const TERMINAL_TAB: &str = "side-tab-2";
const BROWSER_TAB: &str = "side-tab-3";

fn new_session_screen(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    // The Workbench is in place at once, not sliding in, so a click lands on its rows.
    cx.update(|cx| cx.set_reduce_motion(true));
    let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            std::sync::Arc::new(gateway),
            events,
            arrivals,
            false,
            crate::workspace::machine_source::MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = Task::ready(());
        workspace.code = None;
        workspace.runner = None;
        workspace.auth = crate::workspace::Auth::SignedIn;
        workspace.view = View::NewCode;
        workspace.code_form.project = None;
        workspace.side.terminal_factory = TerminalSet::for_view_test;
        workspace
    });
    workspace.update_in(cx, |_, window, _| window.activate_window());
    (workspace, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui_kit::test]
fn with_no_session_a_closed_terminal_takes_its_tab_and_the_toggle_lists_the_tools(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = new_session_screen(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.pick_tool(Tool::Terminal, window, cx);
        assert!(workspace.side.showing(Tool::Terminal));
        assert!(workspace.side.terminal().is_some());
    });
    draw(cx);
    assert!(cx.debug_bounds(TERMINAL_TAB).is_some());
    assert!(
        cx.debug_bounds(BROWSER_TAB).is_none(),
        "no browser is open, so it has no tab"
    );

    workspace.update_in(cx, |workspace, _, cx| {
        workspace.side.plus_menu = true;
        cx.notify();
    });
    draw(cx);
    assert!(
        cx.debug_bounds("side-plus-browser").is_some(),
        "\"+\" reopens the browser"
    );
    assert!(cx.debug_bounds("side-plus-new-terminal").is_some());
    assert!(
        cx.debug_bounds("side-plus-terminal").is_none(),
        "Terminal is a tab already"
    );

    workspace.update_in(cx, |workspace, window, cx| {
        workspace.side.plus_menu = false;
        workspace.close_tool(Tool::Terminal, window, cx);
        assert!(workspace.side.terminal().is_none());
        assert!(!workspace.side.open, "closing the last tab hides the Workbench");
        workspace.toggle_side_pane(window, cx);
        assert!(workspace.side.open);
        assert!(workspace.side.terminal().is_none(), "the toggle starts no shell");
    });
    draw(cx);
    assert!(cx.debug_bounds("workbench-empty").is_some());
    assert!(
        cx.debug_bounds(TERMINAL_TAB).is_none(),
        "the closed terminal took its tab"
    );
    assert!(cx.debug_bounds("workbench-empty-browser").is_some());
    assert!(
        cx.debug_bounds("workbench-empty-new-terminal").is_none(),
        "with no shell, Terminal starts the first one"
    );

    let terminal = cx
        .debug_bounds("workbench-empty-terminal")
        .expect("the empty Workbench lists Terminal");
    cx.simulate_click(terminal.center(), gpui_kit::Modifiers::none());
    draw(cx);
    workspace.update_in(cx, |workspace, _, _| {
        assert!(workspace.side.terminal().is_some(), "its row starts a shell");
        assert!(workspace.side.showing(Tool::Terminal));
    });
    assert!(cx.debug_bounds(TERMINAL_TAB).is_some());
    assert!(cx.debug_bounds("workbench-empty").is_none());
    workspace.update_in(cx, |workspace, _, _| workspace.side.clear_private_terminals());
}
