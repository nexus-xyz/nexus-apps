//! The project menu's branching: which copy of a repository a pick opens, where the repository picked
//! goes when the session moves to another machine, the missing repository dialog's keys, and the menu's keyboard.
//! Headless: no credentials, settings writes, clones or agent processes.

use super::{LaunchMenu, ProjectPick};
use crate::cloud::CloudRepo;
use crate::code::CodeProject;
use crate::workspace::overlay::OverlaySlot;
use crate::workspace::{Auth, View, Workspace, machine_source::MachineSource};
use gpui_kit::{Entity, Focusable, Keystroke, TestAppContext, VisualTestContext};
use std::sync::Arc;

const NEXUS: &str = "https://github.com/acme/nexus";

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
        workspace.code_form = super::CodeForm::default();
        workspace.code_form.org = None;
        workspace.auth = Auth::SignedIn;
        workspace.view = View::NewCode;
        // One repository with a copy on this computer and one on a server, and a folder on the server.
        workspace.projects = vec![
            project("local", Some(NEXUS), None),
            project("on-srv", Some(NEXUS), Some("srv")),
            project("notes-srv", None, Some("srv")),
        ];
    });
    (workspace, cx)
}

fn project(id: &str, remote: Option<&str>, machine: Option<&str>) -> CodeProject {
    CodeProject {
        id: id.into(),
        name: if remote.is_some() { "nexus".into() } else { id.into() },
        branch: Some("main".into()),
        pinned: false,
        remotes: remote.map(String::from).into_iter().collect(),
        sessions: Vec::new(),
        machine: machine.map(String::from),
        added_at: 0,
    }
}

fn repo() -> CloudRepo {
    CloudRepo {
        name: "acme/nexus".into(),
        url: NEXUS.into(),
        private: false,
        pushed_at: None,
    }
}

fn key(key: &'static str) -> Keystroke {
    Keystroke::parse(key).unwrap()
}

#[gpui_kit::test]
fn a_repository_pick_opens_its_copy_where_the_session_runs(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        let ids = vec!["local".to_string(), "on-srv".to_string()];
        // Nothing picked yet: the copy on this computer.
        workspace.pick_repo_project(ids.clone(), window, cx);
        assert_eq!(workspace.code_form.project.as_deref(), Some("local"));
        // The session runs on the server (another project there is picked): the server's copy.
        workspace.code_form.project = Some("notes-srv".into());
        workspace.pick_repo_project(ids.clone(), window, cx);
        assert_eq!(workspace.code_form.project.as_deref(), Some("on-srv"));
        // Cloud picked: the repository goes to the cloud, and no copy opens.
        workspace.view = View::NewCode;
        workspace.code_form.project = Some("local".into());
        workspace.cloud_ui.on = true;
        workspace.pick_repo_project(ids, window, cx);
        assert!(workspace.cloud_ui.on);
        assert_eq!(workspace.cloud_ui.repo, Some(repo()));
    });
}

#[gpui_kit::test]
fn moving_a_picked_repository_finds_its_copy_or_asks(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        // From the cloud to the server: the server has a copy, so it opens.
        workspace.cloud_ui.on = true;
        workspace.cloud_ui.repo = Some(repo());
        workspace.run_on(Some("srv"), window, cx);
        assert_eq!(workspace.code_form.project.as_deref(), Some("on-srv"));
        assert!(!workspace.cloud_ui.on);
        assert_eq!(workspace.cloud_ui.missing, None);
        // To a server with no copy: nothing opens, and the missing dialog asks about that server.
        workspace.view = View::NewCode;
        workspace.cloud_ui.on = true;
        workspace.cloud_ui.repo = Some(repo());
        workspace.run_on(Some("build"), window, cx);
        assert_eq!(workspace.code_form.project, None);
        assert_eq!(workspace.cloud_ui.missing, Some(repo()));
        assert_eq!(workspace.cloud_ui.missing_on.as_deref(), Some("build"));
        // The repository stays picked through a second move: here, where it has a copy.
        workspace.view = View::NewCode;
        workspace.run_on(None, window, cx);
        assert_eq!(workspace.code_form.project.as_deref(), Some("local"));
        assert_eq!(workspace.cloud_ui.missing, None);
        assert_eq!(workspace.cloud_ui.missing_on, None);
    });
}

#[gpui_kit::test]
fn the_missing_dialog_steps_back_then_cancels_and_enter_only_starts_the_clone(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(
            !workspace.missing_dialog_key(&key("escape"), window, cx),
            "no dialog, not its key"
        );
        workspace.cloud_ui.missing = Some(repo());
        // Step one: Enter opens the clone form (step two), and clones nothing yet.
        assert!(workspace.missing_dialog_key(&key("enter"), window, cx));
        assert!(workspace.clone_form.as_ref().is_some_and(|form| form.in_menu));
        assert!(!workspace.cloning());
        // Step two: Enter belongs to the form's fields.
        assert!(!workspace.missing_dialog_key(&key("enter"), window, cx));
        // Esc steps back to the choices, the repository still waiting.
        assert!(workspace.missing_dialog_key(&key("escape"), window, cx));
        assert!(workspace.clone_form.is_none());
        assert_eq!(workspace.cloud_ui.missing, Some(repo()));
        // Esc again cancels.
        assert!(workspace.missing_dialog_key(&key("escape"), window, cx));
        assert_eq!(workspace.cloud_ui.missing, None);
        assert!(!workspace.missing_dialog_key(&key("escape"), window, cx));
    });
}

#[gpui_kit::test]
fn the_project_menu_moves_and_picks_from_its_search(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.groups.groups.add("Launch", 1);
        workspace.toggle_launch_menu(LaunchMenu::Project, window, cx);
        assert!(workspace.repo_search().read(cx).focus_handle(cx).is_focused(window));
        // Nothing picked yet, so no Current section: the projects (own, then one per repository), No project last.
        let picks = workspace.project_picks();
        assert!(matches!(picks[0], ProjectPick::Own(_)));
        assert_eq!(picks[1], ProjectPick::Repo(vec!["local".into(), "on-srv".into()]));
        assert_eq!(picks[picks.len() - 2], ProjectPick::NoProject);
        assert_eq!(picks.last(), Some(&ProjectPick::NewProject));
        // Up from the first row wraps to New project; Down twice from there lands on the repository.
        workspace.on_keystroke(&key("up"), window, cx);
        assert_eq!(workspace.cloud_ui.cursor, picks.len() - 1);
        workspace.on_keystroke(&key("down"), window, cx);
        workspace.on_keystroke(&key("down"), window, cx);
        assert_eq!(workspace.cloud_ui.cursor, 1);
        workspace.on_keystroke(&key("enter"), window, cx);
        assert_eq!(workspace.overlay.launch_menu(), None);
        assert_eq!(workspace.code_form.project.as_deref(), Some("local"));
    });
    workspace.update_in(cx, |workspace, window, cx| {
        // "local" is picked now: it leads, under Current, and the menu opens on it.
        workspace.toggle_launch_menu(LaunchMenu::Project, window, cx);
        let rows = workspace.project_rows();
        assert_eq!(rows.sections[0].0, crate::workspace::project_menu::Section::Current);
        assert_eq!(
            rows.sections[0].1[0].pick,
            ProjectPick::Repo(vec!["local".into(), "on-srv".into()])
        );
        assert!(rows.sections[0].1[0].picked);
        workspace.on_keystroke(&key("down"), window, cx);
        assert_eq!(workspace.cloud_ui.cursor, 1);
    });
    // Typed into the focused search, as the person does: a change event, not a programmatic set.
    cx.simulate_input("notes");
    workspace.update_in(cx, |workspace, _, _| {
        // A new search lights its first row, and only matching rows are left: the best match first.
        assert_eq!(workspace.cloud_ui.cursor, 0);
        assert_eq!(
            workspace.project_picks(),
            [ProjectPick::Repo(vec!["notes-srv".into()]), ProjectPick::NewProject]
        );
        let rows = workspace.project_rows();
        assert_eq!(rows.sections[0].0, crate::workspace::project_menu::Section::Best);
        assert_eq!(rows.sections[0].1[0].name_marks, [0..5]);
    });
}

#[gpui_kit::test]
fn a_query_ranks_marks_and_narrows_by_scope(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.toggle_launch_menu(LaunchMenu::Project, window, cx);
    });
    // "es" is inside "notes-srv" and scattered in "nexus": both match, the closer one first.
    cx.simulate_input("es");
    workspace.update_in(cx, |workspace, window, cx| {
        let picks = workspace.project_picks();
        assert_eq!(picks[0], ProjectPick::Repo(vec!["notes-srv".into()]));
        assert_eq!(picks[1], ProjectPick::Repo(vec!["local".into(), "on-srv".into()]));
        let all = workspace.project_rows().counts;
        // Tab narrows to Projects, then GitHub (nothing read here), then back to All; Shift+Tab steps back.
        workspace.on_keystroke(&key("tab"), window, cx);
        assert_eq!(
            workspace.cloud_ui.scope,
            crate::workspace::project_menu::Scope::Projects
        );
        assert_eq!(workspace.project_picks().len(), all[1] + 1);
        workspace.on_keystroke(&key("tab"), window, cx);
        assert_eq!(workspace.project_picks(), [ProjectPick::NewProject]);
        workspace.on_keystroke(&key("shift-tab"), window, cx);
        assert_eq!(
            workspace.cloud_ui.scope,
            crate::workspace::project_menu::Scope::Projects
        );
    });
}

#[gpui_kit::test]
fn a_pasted_url_finds_its_repository_and_a_miss_offers_a_named_project(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.toggle_launch_menu(LaunchMenu::Project, window, cx);
    });
    cx.simulate_input("https://github.com/acme/nexus.git");
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(
            workspace.project_picks()[0],
            ProjectPick::Repo(vec!["local".into(), "on-srv".into()])
        );
        // A URL names no project.
        assert_eq!(workspace.new_project_name(), None);
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.on_keystroke(&key("escape"), window, cx);
    });
    cx.simulate_input("zzz");
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(workspace.project_picks(), [ProjectPick::NewProject]);
        assert_eq!(workspace.new_project_name().as_deref(), Some("zzz"));
    });
}

#[gpui_kit::test]
fn reopening_the_project_menu_forgets_the_last_search(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.toggle_launch_menu(LaunchMenu::Project, window, cx);
    });
    cx.simulate_input("notes");
    workspace.update_in(cx, |workspace, window, cx| {
        assert_eq!(workspace.project_picks().len(), 2);
        // Esc clears the search first, and the menu stays open with every row back.
        workspace.on_keystroke(&key("escape"), window, cx);
        assert_eq!(workspace.overlay.launch_menu(), Some(LaunchMenu::Project));
        assert_eq!(workspace.repo_query(), "");
        assert!(workspace.project_picks().contains(&ProjectPick::NoProject));
        // A second Esc closes it.
        workspace.on_keystroke(&key("escape"), window, cx);
        assert_eq!(workspace.overlay.launch_menu(), None);
        workspace.toggle_launch_menu(LaunchMenu::Project, window, cx);
    });
    // Closed by its chip with a search in it: the next opening starts empty.
    cx.simulate_input("notes");
    workspace.update_in(cx, |workspace, window, cx| {
        assert_eq!(workspace.repo_query(), "notes");
        workspace.toggle_launch_menu(LaunchMenu::Project, window, cx);
        assert_eq!(workspace.overlay.launch_menu(), None);
        workspace.toggle_launch_menu(LaunchMenu::Project, window, cx);
        assert_eq!(workspace.repo_query(), "");
        assert!(workspace.project_picks().contains(&ProjectPick::NoProject));
        assert!(
            workspace
                .project_picks()
                .contains(&ProjectPick::Repo(vec!["local".into(), "on-srv".into()]))
        );
    });
}

#[test]
fn a_lit_row_maps_to_its_child_past_the_section_titles() {
    use crate::workspace::project_menu::{MenuRows, Row, Section};
    let row = |pick| Row {
        pick,
        name_marks: Vec::new(),
        detail_marks: Vec::new(),
        picked: false,
    };
    let rows = MenuRows {
        sections: vec![
            (Section::Current, vec![row(ProjectPick::Own("p1".into()))]),
            (
                Section::Projects,
                vec![
                    row(ProjectPick::Repo(vec!["local".into()])),
                    row(ProjectPick::NoProject),
                ],
            ),
            (
                Section::GitHub,
                vec![row(ProjectPick::GitHub(repo())), row(ProjectPick::MoreGitHub)],
            ),
        ],
        new_project: true,
        ..Default::default()
    };
    assert_eq!(rows.picks().len(), 6);
    // "Current", own, "Projects", repo, No project, "GitHub", repository, Show all.
    assert_eq!(rows.child_of(0), Some(1));
    assert_eq!(rows.child_of(1), Some(3));
    assert_eq!(rows.child_of(2), Some(4));
    assert_eq!(rows.child_of(3), Some(6));
    assert_eq!(rows.child_of(4), Some(7));
    assert_eq!(rows.child_of(5), None, "New project sits under the list");
}
