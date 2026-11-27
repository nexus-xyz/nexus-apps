//! Headless: which terminal a server session gets. No shell starts and no server is called.
use core::prelude::v1::test;
use std::sync::Arc;

use futures::future::BoxFuture;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use serde_json::Value;

use super::super::{Scope, Tool};
use super::*;
use crate::workspace::View;
use crate::workspace::code_view::CodeView;
use crate::workspace::machine_source::MachineSource;

const ID: &str = "server-session";

/// A host that is never reached: the view-test shells open nothing.
struct Unreached;

impl ShellHost for Unreached {
    fn call(&self, _: &'static str, _: Value) -> BoxFuture<'static, Result<Value, String>> {
        Box::pin(async { Err("no server in tests".to_string()) })
    }
}

fn opens(_: &str) -> Option<Arc<dyn ShellHost>> {
    Some(Arc::new(Unreached))
}

fn opens_none(_: &str) -> Option<Arc<dyn ShellHost>> {
    None
}

fn fixture<'a>(
    server: Option<&str>,
    shells: ServerShells,
    cx: &'a mut TestAppContext,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    cx.update(gpui_kit::init);
    let (gateway, _gateway_events) = crate::gateway::Gateway::start();
    let (_send, events) = futures::channel::mpsc::unbounded();
    let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
    let server = server.map(str::to_string);
    cx.add_window_view(move |window, cx| {
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
        workspace.view = View::Code(CodeView::new(ID.into(), window, cx));
        workspace.side.terminal_factory = TerminalSet::for_view_test;
        workspace.side.server_set_factory = TerminalSet::server_for_view_test;
        workspace.side.server_shells = shells;
        workspace.side.scope = Some(Scope::Session(ID.into()));
        workspace.side.dir = std::env::temp_dir();
        workspace.side.server = server;
        workspace.side.open = true;
        workspace.side.tool = Tool::Terminal;
        workspace
    })
}

#[gpui_kit::test]
fn a_server_session_whose_host_opens_shells_gets_a_shell_there(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(Some("box"), opens, cx);
    workspace.update_in(cx, |workspace, window, cx| {
        let set = workspace.ensure_terminal(window, cx).expect("a terminal on the server");
        assert!(set.read(cx).on_a_server());
        let key = PathBuf::from(format!("nexus-server://box/{ID}"));
        assert_eq!(workspace.side.terminal_key(), key);
        assert!(workspace.side.terminals.contains_key(&key));
        // Not under this computer's folder: a local shell there would be another set.
        assert!(!workspace.side.terminals.contains_key(&std::env::temp_dir()));
        assert!(workspace.side.has_terminal());
        // Asked again, the same set.
        let again = workspace.ensure_terminal(window, cx).expect("still there");
        assert_eq!(again.entity_id(), set.entity_id());
    });
}

#[gpui_kit::test]
fn an_older_host_keeps_the_note_and_opens_nothing(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(Some("box"), opens_none, cx);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.ensure_terminal(window, cx).is_none());
        assert!(workspace.side.terminals.is_empty());
        assert!(workspace.side.terminal().is_none());
    });
}

fn project(id: &str, machine: Option<&str>) -> crate::code::CodeProject {
    crate::code::CodeProject {
        id: id.into(),
        name: id.into(),
        branch: None,
        pinned: false,
        remotes: Vec::new(),
        sessions: Vec::new(),
        machine: machine.map(str::to_string),
        added_at: 0,
    }
}

/// The new-session screen of a project, here or on server `box`, with the Workbench open on its terminal.
fn new_session_screen(workspace: &mut Workspace, project_id: &str) {
    workspace.view = View::NewCode;
    workspace.cloud_ui.on = false;
    workspace.projects = vec![project("here", None), project("there", Some("box"))];
    workspace.code_form.project = Some(project_id.into());
    workspace.side.scope = None;
    workspace.side.server = None;
}

#[gpui_kit::test]
fn a_server_projects_new_session_screen_opens_its_shell_on_the_server(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(None, opens, cx);
    workspace.update_in(cx, |workspace, window, cx| {
        new_session_screen(workspace, "there");
        workspace.sync_side(window, cx);
        assert_eq!(workspace.side.scope, Some(Scope::Project("there".into())));
        assert_eq!(workspace.side.server.as_deref(), Some("box"));
        // Its tools read there: Files and Changes ask the server (`ServerPlace` with the project).
        assert!(workspace.side.has_folder());
        assert!(matches!(
            workspace.scope_place(&Scope::Project("there".into())),
            crate::side::Place::Server(ref place) if place.project.as_deref() == Some("there") && place.thread.is_none()
        ));
        // The Terminal tab, as Ctrl+` opens it on this screen.
        workspace.side.open = true;
        workspace.side.tool = Tool::Terminal;
        let set = workspace.ensure_terminal(window, cx).expect("a shell on the server");
        assert!(set.read(cx).on_a_server());
        assert_eq!(workspace.side.terminal_key(), PathBuf::from("nexus-server://box/there"));
        // The browser of a project stays this computer's: no server's page claims it.
        assert!(matches!(
            super::super::browser_place("project:there"),
            Ok(nexus_browser::place::Place::Local)
        ));
    });
}

#[gpui_kit::test]
fn a_local_projects_new_session_screen_keeps_its_shell_here(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(None, opens, cx);
    workspace.update_in(cx, |workspace, window, cx| {
        new_session_screen(workspace, "here");
        workspace.sync_side(window, cx);
        assert_eq!(workspace.side.scope, Some(Scope::Project("here".into())));
        assert_eq!(workspace.side.server, None);
        workspace.side.open = true;
        workspace.side.tool = Tool::Terminal;
        let set = workspace.ensure_terminal(window, cx).expect("a shell here");
        assert!(!set.read(cx).on_a_server());
        assert_eq!(workspace.side.terminal_key(), workspace.side.dir);
        // The cloud screen keeps its own: no project's tools.
        workspace.cloud_ui.on = true;
        assert!(matches!(workspace.side_scope(), Scope::Home));
    });
}

#[gpui_kit::test]
fn a_project_command_on_a_server_session_runs_in_a_shell_there(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(Some("box"), opens, cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.run_in_side_terminal("npm run dev", Some("Dev server"), window, cx);
        assert!(workspace.side.open && workspace.side.tool == Tool::Terminal);
        let set = workspace.side.terminal().cloned().expect("the server's terminals");
        // The open Workbench already had a shell there; the command gets a terminal of its own beside it.
        assert_eq!(
            set.read(cx).listed(cx).last(),
            Some(&("Dev server".to_string(), Some("npm run dev".to_string())))
        );
    });
}

#[gpui_kit::test]
fn a_project_command_on_an_older_host_opens_nothing(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(Some("box"), opens_none, cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.run_in_side_terminal("npm run dev", Some("Dev server"), window, cx);
        assert!(workspace.side.terminals.is_empty());
    });
}

#[gpui_kit::test]
fn a_session_here_keeps_its_shell_here(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(None, opens, cx);
    workspace.update_in(cx, |workspace, window, cx| {
        let set = workspace.ensure_terminal(window, cx).expect("a shell here");
        assert!(!set.read(cx).on_a_server());
        assert_eq!(workspace.side.terminal_key(), std::env::temp_dir());
    });
}
