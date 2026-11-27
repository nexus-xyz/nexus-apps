//! Headless: a fake terminal set, no shell, no settings writes.
use core::prelude::v1::test;
use std::sync::Arc;

use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use nexus_workbench::scripts::ProjectScript;

use super::{Scope, Tool};
use crate::side::terminals::TerminalSet;
use crate::workspace::code_view::CodeView;
use crate::workspace::machine_source::MachineSource;
use crate::workspace::{Auth, View, Workspace};

const ID: &str = "commands-key";

fn fixture(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
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
        workspace.code = None;
        workspace.runner = None;
        workspace.auth = Auth::SignedIn;
        let mut view = CodeView::new(ID.into(), window, cx);
        view.loaded = true;
        workspace.view = View::Code(view);
        workspace.side.terminal_factory = TerminalSet::for_view_test;
        workspace.side.scope = Some(Scope::Session(ID.into()));
        workspace.side.dir = std::env::temp_dir();
        workspace
    });
    cx.update(|_, cx| {
        crate::project_commands::saved(
            "proj",
            vec![ProjectScript {
                id: "c-dev".into(),
                name: "Dev server".into(),
                command: "npm run dev".into(),
                shortcut: Some("Mod+Shift+KeyY".into()),
            }],
            cx,
        )
    });
    workspace.update_in(cx, |workspace, window, cx| {
        window.activate_window();
        window.focus(&workspace.focus, cx);
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        let mut session = crate::workspace::session_keys::tests::session(ID);
        session.project_id = "proj".into();
        workspace.projects = vec![crate::code::CodeProject {
            id: "proj".into(),
            name: "proj".into(),
            branch: None,
            pinned: false,
            remotes: Vec::new(),
            sessions: vec![session],
            machine: None,
            added_at: 0,
        }];
    });
    (workspace, cx)
}

fn terminal_shown(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
    workspace.read_with(cx, |workspace, _| {
        workspace.side.open && workspace.side.tool == Tool::Terminal && !workspace.side.terminals.is_empty()
    })
}

#[gpui_kit::test]
fn a_command_key_runs_its_command_in_a_new_terminal(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    let mod_key = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    // Another key with the same modifiers runs nothing.
    cx.simulate_keystrokes(&format!("{mod_key}-shift-u"));
    assert!(!terminal_shown(&workspace, cx));
    cx.simulate_keystrokes(&format!("{mod_key}-shift-y"));
    assert!(terminal_shown(&workspace, cx), "the command's key opens its terminal");
}

#[gpui_kit::test]
fn a_command_key_does_nothing_away_from_a_code_session(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, _, _| workspace.view = View::Chat);
    let mod_key = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    cx.simulate_keystrokes(&format!("{mod_key}-shift-y"));
    assert!(!terminal_shown(&workspace, cx));
}
