//! Headless checks for archived projects in the sidebar's "Archived" group.

use super::super::Workspace;
use super::super::folder_motion;
use crate::code::ArchivedProject;
use gpui_kit::{Entity, Task, TestAppContext, VisualTestContext};

fn fixture(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _gateway_events) = crate::gateway::Gateway::start();
    let (_send, events) = futures::channel::mpsc::unbounded();
    let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
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
        workspace
    });
    workspace.update_in(cx, |workspace, _, _| {
        workspace.auth = crate::workspace::Auth::SignedIn;
        workspace.view = crate::workspace::View::Chat;
        workspace.archived_open = true;
        workspace.archived_fold_motion = folder_motion::Transition::settled(1.);
        workspace.archived_chats = Some(Ok(Vec::new()));
    });
    (workspace, cx)
}

#[gpui_kit::test]
fn an_archived_project_waits_in_the_archive_with_unarchive(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("archived-project").is_none(),
        "no archived project, no row"
    );

    workspace.update_in(cx, |workspace, _, cx| {
        workspace.archived_projects = vec![ArchivedProject {
            id: "p-web".into(),
            name: "web".into(),
        }];
        cx.notify();
    });
    cx.run_until_parked();
    let row = cx
        .debug_bounds("archived-project")
        .expect("the archived project has a row");
    let button = cx
        .debug_bounds("archived-project-unarchive")
        .expect("the row offers Unarchive project");
    assert!(row.contains(&button.center()), "Unarchive sits on the project's row");

    // With no runner here, the click reaches `update_project`, which says the sessions are read-only.
    cx.simulate_click(button.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert!(
            workspace.composer_note.is_some(),
            "the click asked to unarchive the project"
        );
    });
}
