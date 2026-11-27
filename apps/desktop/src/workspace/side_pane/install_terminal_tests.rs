//! Headless: the model panel's Install for an agent that can't run here opens the side pane's Terminal and runs the
//! agent's install command there, from the keyboard alone.
use super::*;
use crate::workspace::missing_agents::MissingRow;
use crate::workspace::new_code::LaunchMenu;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use core::prelude::v1::test;
use gpui_kit::{Keystroke, TestAppContext};
use nexus_workbench::{api::AgentInfo, event::SessionAgent};

fn info(agent: SessionAgent, path: Option<&str>) -> AgentInfo {
    AgentInfo {
        agent,
        path: path.map(str::to_string),
        version: None,
        models: None,
        problem: None,
    }
}

#[gpui_kit::test]
fn enter_opens_the_fold_and_then_installs_the_agent_in_the_side_terminal(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    cx.update(gpui_kit::init);
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
        workspace.cloud_ui.on = false;
        workspace.code_form.project = None;
        workspace.side.terminal_factory = TerminalSet::for_view_test;
        workspace.code_agents = vec![
            info(SessionAgent::Claude, Some("/fixture/claude")),
            info(SessionAgent::Codex, None),
        ];
        workspace
    });
    let enter = Keystroke::parse("enter").unwrap();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
        assert_eq!(workspace.missing_panel_rows(), vec![MissingRow::Fold(1)]);

        // Enter on the fold opens it.
        workspace.sync_model_rows();
        let models = workspace.launch_rows().len();
        workspace.model_cursor = models;
        workspace.on_keystroke(&enter, window, cx);
        assert!(workspace.code_form.missing_open);
        assert_eq!(
            workspace.missing_panel_rows(),
            vec![MissingRow::Fold(1), MissingRow::Agent(SessionAgent::Codex)]
        );
        assert_eq!(
            workspace.overlay.launch_menu(),
            Some(LaunchMenu::Model),
            "the panel stays open"
        );

        // Enter on the agent closes the panel and runs its install in the side pane's Terminal.
        workspace.sync_model_rows();
        workspace.model_cursor = models + 1;
        workspace.on_keystroke(&enter, window, cx);
        assert_eq!(workspace.overlay.launch_menu(), None);
        assert!(workspace.side.open);
        assert_eq!(workspace.side.tool, Tool::Terminal);
        let set = workspace.side.terminal().expect("a Terminal opened").clone();
        let names: Vec<String> = set.read(cx).listed(cx).into_iter().map(|(name, _)| name).collect();
        assert!(names.iter().any(|name| name == "Install Codex"), "{names:?}");
    });
}
