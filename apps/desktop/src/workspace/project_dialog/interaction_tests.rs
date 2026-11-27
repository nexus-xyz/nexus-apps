//! Exercise navigation against an isolated workspace without remote calls.
use super::*;
use crate::machines::ServerView;
use crate::workspace::machine_source::MachineSource;
use core::prelude::v1::test;
use nexus_workbench::api::HostInfo;

/// A connected server that clones; it does not list folders, so the dialog makes no remote call.
fn server(id: &str) -> ServerView {
    ServerView {
        id: id.into(),
        name: id.into(),
        ssh: "fixture.invalid".into(),
        busy: 0,
        status: Status::Connected {
            host: HostInfo {
                version: "0.1.0".into(),
                protocol: 1,
                machine: id.into(),
                org: None,
                features: vec![nexus_workbench::clone::FEATURE.into()],
            },
            update: None,
        },
    }
}

#[gpui_kit::test]
fn the_dialog_opens_on_the_folder_field_switches_modes_and_locks_a_running_clone(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    cx.update(gpui_kit::init);
    let (gateway, _gateway_events) = crate::gateway::Gateway::start();
    let (_send, events) = futures::channel::mpsc::unbounded();
    let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        Workspace::with_machine_source(
            std::sync::Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        )
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.servers = vec![server("dialog-fixture-a"), server("dialog-fixture-b")];
        // The server's folder button opens straight on its folder field, focused, ending in a slash.
        workspace.open_project_dialog(Some("dialog-fixture-a".into()), window, cx);
        let dialog = workspace.project_dialog.as_ref().unwrap();
        assert_eq!(dialog.mode, Mode::Folder);
        let browser = dialog.browser.clone().expect("the folder field is open");
        let input = browser.read(cx).input.clone();
        assert!(input.read(cx).value().ends_with('/'));
        assert!(input.read(cx).focus_handle(cx).is_focused(window));

        workspace.project_mode(Mode::Clone, window, cx);
        let form = workspace.server_folder.as_ref().unwrap().clone.as_ref().unwrap();
        form.source
            .update(cx, |input, cx| input.set_value("owner/repo", window, cx));
        form.parent
            .update(cx, |input, cx| input.set_value("/srv/private-a", window, cx));
        // Another computer keeps the mode and the repository, and resets the path.
        workspace.choose_project_computer(Some("dialog-fixture-b".into()), window, cx);
        assert_eq!(workspace.project_dialog.as_ref().unwrap().mode, Mode::Clone);
        let form = workspace.server_folder.as_ref().unwrap();
        assert_eq!(form.machine, "dialog-fixture-b");
        let clone = form.clone.as_ref().unwrap();
        assert_eq!(clone.source.read(cx).value().as_ref(), "owner/repo");
        assert_eq!(clone.parent.read(cx).value().as_ref(), "~");
        assert!(
            workspace.clone_form.is_none(),
            "remote selection never opens a local clone"
        );
        workspace
            .server_folder
            .as_mut()
            .unwrap()
            .clone
            .as_mut()
            .unwrap()
            .running = Some("fixture-request".into());
        // A running clone holds the dialog: no other computer, no other mode, no close.
        workspace.choose_project_computer(None, window, cx);
        workspace.project_mode(Mode::Folder, window, cx);
        workspace.close_project_dialog(window, cx);
        let dialog = workspace
            .project_dialog
            .as_ref()
            .expect("a running clone keeps the dialog");
        assert_eq!(dialog.machine.as_deref(), Some("dialog-fixture-b"));
        assert_eq!(dialog.mode, Mode::Clone);
        workspace
            .server_folder
            .as_mut()
            .unwrap()
            .clone
            .as_mut()
            .unwrap()
            .running = None;
    });
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    let bounds = cx.debug_bounds("project-dialog").expect("the project dialog is drawn");
    assert!(bounds.size.width >= px(440.), "the form has room for repository paths");
    for selector in [
        "project-mode-Folder",
        "project-mode-Clone",
        "project-computer",
        "project-field-Repository",
        "project-field-Clone into",
        "project-submit",
    ] {
        let field = cx.debug_bounds(selector).expect("each control is drawn");
        assert!(field.size.height > px(24.), "{selector} keeps a usable height");
        assert!(
            field.top() >= bounds.top() && field.bottom() <= bounds.bottom(),
            "{selector} stays inside the dialog"
        );
    }
    assert!(
        cx.debug_bounds("project-choose-folder").is_none(),
        "remote paths have no native picker"
    );
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.project_mode(Mode::Folder, window, cx);
    });
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    let folder = cx
        .debug_bounds("project-field-Folder")
        .expect("the folder field is drawn");
    assert!(folder.top() >= bounds.top() && folder.bottom() <= bounds.bottom());
    assert!(
        cx.debug_bounds("project-browse").is_none(),
        "Browse… is for this computer only"
    );
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.close_project_dialog(window, cx);
        assert!(workspace.project_dialog.is_none());
        assert!(workspace.server_folder.is_none());
    });
}

#[gpui_kit::test]
fn signed_out_project_add_lists_folders_from_the_picker_and_dialog(cx: &mut TestAppContext) {
    use crate::code_run::CodeEvent;
    use crate::workspace::{Auth, local_mode::Entry};
    const LOCAL_SCOPE: &str = "|";
    use std::time::{Duration, Instant};

    let _agents = crate::code_run::fixture_agents(Vec::new());
    let profile = tempfile::tempdir().unwrap();
    let data = profile.path().join("data");
    let picker = profile.path().join("picked-repo");
    let typed = profile.path().join("typed-repo");
    for repo in [&picker, &typed] {
        std::fs::create_dir(repo).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(repo)
                .status()
                .unwrap()
                .success()
        );
    }
    cx.executor().allow_parking();
    cx.update(gpui_kit::init);
    let (gateway, _gateway_events) = crate::gateway::Gateway::start();
    let (_, code_events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            std::sync::Arc::new(gateway),
            code_events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = Task::ready(());
        workspace.auth = Auth::SignedOut { error: None };
        workspace.local_mode.opt_in = crate::local_providers_opt_in::enabled_for_test();
        workspace.local_mode.remembered = true;
        workspace
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.enter_local(Entry::Launch, window, cx);
        assert!(workspace.is_local());
        // Model the first scope open when the background service hasn't created its database yet.
        workspace.code = crate::code::CodeStore::open_in(&data, LOCAL_SCOPE).unwrap();
        assert!(workspace.code.is_none());
        workspace.code_refresh = None;
        workspace.refresh_code(cx);
    });
    cx.run_until_parked();
    let (runner, mut events) = crate::code_run::test_runner(data, LOCAL_SCOPE);
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.runner = Some(Rc::new(runner));
        workspace.add_project(cx);
    });
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.multiple);
        Some(vec![picker])
    });
    cx.run_until_parked();

    let mut wait_for_project = |name: &str, cx: &mut VisualTestContext| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut changed = Vec::new();
            while let Ok(event) = events.try_recv() {
                if let CodeEvent::ProjectAddFailed { error, .. } = &event {
                    panic!("the picker refused {name}: {error}");
                }
                changed.push(event);
            }
            if !changed.is_empty() {
                workspace.update_in(cx, |workspace, window, cx| {
                    workspace.on_code_events(changed, window, cx)
                });
            }
            cx.run_until_parked();
            if workspace.read_with(cx, |workspace, _| {
                workspace
                    .projects
                    .iter()
                    .any(|project| project.name == name && workspace.code_form.project.as_ref() == Some(&project.id))
            }) {
                return;
            }
            assert!(Instant::now() < deadline, "{name} was not listed and selected");
            std::thread::sleep(Duration::from_millis(1));
        }
    };

    wait_for_project("picked-repo", cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_project_dialog(None, window, cx);
        let field = workspace.project_dialog.as_ref().unwrap().browser.clone().unwrap();
        field.update(cx, |field, cx| {
            field.set_path(typed.to_string_lossy().into_owned(), window, cx)
        });
        workspace.submit_project(window, cx);
    });
    wait_for_project("typed-repo", cx);
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.project_dialog.is_none());
        let mut names: Vec<_> = workspace.projects.iter().map(|project| project.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["picked-repo", "typed-repo"]);
    });
    workspace.update_in(cx, |workspace, _, cx| workspace.add_project(cx));
    cx.simulate_path_prompt_response(|_| Some(Vec::new()));
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(
            workspace.notices.current().unwrap().text,
            "Add a project: The picker returned no local folder."
        );
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.auth = Auth::Restoring;
        workspace.clear_session(cx);
        // A late change from the previous runner must not reopen the forgotten scope.
        workspace.on_code_events(vec![CodeEvent::Changed], window, cx);
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.code_scope.is_none());
        assert!(workspace.code.is_none());
        assert!(workspace.projects.is_empty());
    });
}
