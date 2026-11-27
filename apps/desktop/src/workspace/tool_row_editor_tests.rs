//! Cmd-click (Ctrl off a Mac) on a file's tool row opens the file in the person's editor, as the Nexus app's
//! `SessionCallRow`. A plain click still opens the row. Headless: there is no store reader, so no editor starts, and
//! the reason shows under the row.

use super::*;
use crate::workspace::machine_source::MachineSource;
use crate::workspace::tool_body::editor_target;
use core::prelude::v1::test;
use nexus_workbench::event::WbEvent;
use serde_json::json;
use std::sync::Arc;

const SESSION: &str = "tool-row-editor";

fn fixture(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _) = crate::gateway::Gateway::start();
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
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
        workspace.auth = crate::workspace::Auth::SignedIn;
        workspace._code_poll = Task::ready(());
        workspace.runner = None;
        workspace.code = None;
        let mut view = CodeView::new(SESSION.into(), window, cx);
        view.loaded = true;
        workspace.view = View::Code(view);
        workspace.side.fixture_session(SESSION);
        workspace
    });
    workspace.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    let transcript = Transcript::of(
        [
            WbEvent::User {
                text: "Edit the file".into(),
                images: Vec::new(),
                from: None,
            },
            WbEvent::TurnStart { turn_id: "turn".into() },
            WbEvent::Tool {
                id: "edit".into(),
                name: "Edit".into(),
                input: json!({"file_path": "src/main.rs", "old_string": "fn old() {}", "new_string": "fn new() {}"}),
                parent: None,
            },
            WbEvent::ToolResult {
                id: "edit".into(),
                output: "Edited".into(),
                images: Vec::new(),
                is_error: false,
                truncated: false,
                parent: None,
            },
        ]
        .into_iter()
        .enumerate()
        .map(|(ix, event)| (ix as i64 + 1, ix as i64 + 1, event))
        .collect(),
    );
    workspace.update(cx, |workspace, cx| {
        let mut session = crate::workspace::session_keys::tests::session(SESSION);
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
        if let View::Code(view) = &mut workspace.view {
            view.set(transcript, cx);
        }
        cx.notify();
    });
    draw(cx);
    (workspace, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
}

fn row_center(cx: &mut VisualTestContext) -> Point<Pixels> {
    draw(cx);
    ["tool-row-0", "tool-row-1", "tool-row-2", "tool-row-3"]
        .into_iter()
        .find_map(|selector| cx.debug_bounds(selector))
        .expect("the Edit row is drawn")
        .center()
}

fn state(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> (bool, Option<String>) {
    workspace.read_with(cx, |workspace, _| match &workspace.view {
        View::Code(view) => (
            view.open_tools.contains("edit"),
            view.row_error.as_ref().map(|(id, error)| format!("{id}: {error}")),
        ),
        _ => unreachable!("the fixture opens a Code session"),
    })
}

fn secondary() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            platform: true,
            ..Modifiers::default()
        }
    } else {
        Modifiers {
            control: true,
            ..Modifiers::default()
        }
    }
}

#[gpui_kit::test]
fn a_secondary_click_on_a_file_row_opens_the_editor_and_leaves_the_row_shut(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    let center = row_center(cx);
    cx.simulate_click(center, secondary());
    cx.run_until_parked();
    let (open, error) = state(&workspace, cx);
    assert!(!open, "a secondary click does not open the row");
    assert_eq!(
        error.as_deref(),
        Some("edit: Code is not ready."),
        "the open was tried, and why it failed shows"
    );

    cx.simulate_click(center, Modifiers::default());
    cx.run_until_parked();
    assert!(state(&workspace, cx).0, "a plain click still opens the row");
}

#[test]
fn only_file_calls_name_a_file_to_open() {
    assert_eq!(
        editor_target("Read", &json!({"file_path": "/a.rs", "offset": 12})),
        Some(("/a.rs".into(), Some(12)))
    );
    assert_eq!(
        editor_target("Read", &json!({"file_path": "/a.rs", "offset": 0})),
        Some(("/a.rs".into(), None))
    );
    assert_eq!(
        editor_target("Edit", &json!({"file_path": "/a.rs", "offset": 9})),
        Some(("/a.rs".into(), None))
    );
    assert_eq!(
        editor_target("NotebookEdit", &json!({"notebook_path": "/n.ipynb"})),
        Some(("/n.ipynb".into(), None))
    );
    assert_eq!(editor_target("Bash", &json!({"file_path": "/a.rs"})), None);
    assert_eq!(editor_target("Write", &json!({})), None);
}
