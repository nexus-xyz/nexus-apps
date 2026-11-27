//! Headless tests of production GPUI pane paths. Settings use isolated test folders; agents never start.
use super::*;
use crate::workspace::{Auth, code_view::CodeView, machine_source::MachineSource};
use core::prelude::v1::test;
use gpui_kit::{Focusable, TestAppContext, VisualTestContext};
use std::sync::Arc;

fn fixture<'a>(cx: &'a mut TestAppContext, id: &str) -> (Entity<Workspace>, &'a mut VisualTestContext) {
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
        workspace._code_poll = Task::ready(());
        workspace.code = None;
        workspace.runner = None;
        workspace.auth = Auth::SignedIn;
        let mut view = CodeView::new(id.into(), window, cx);
        view.loaded = true;
        workspace.view = View::Code(view);
        workspace.side.scope = Some(Scope::Session(id.into()));
        let mut state = Navigation::new(false);
        state.baseline = Some(0);
        workspace.side.navigation.insert(id.into(), state);
        workspace
    });
    workspace.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    (workspace, cx)
}

#[gpui_kit::test]
fn following_keeps_a_closed_pane_hidden_and_updates_an_open_pane_without_taking_focus(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "navigation-pane-focus");
    workspace.update_in(cx, |this, window, cx| {
        this.composer.update(cx, |input, cx| {
            input.set_value("Unsent draft", window, cx);
            input.focus(window, cx);
        });
        assert!(!this.side.open);
        this.show_workbench_target(Target::Changes("src/a.rs".into()), true, window, cx);
        assert!(!this.side.open);
        this.show_workbench_target(Target::Changes("src/a.rs".into()), false, window, cx);
        assert!(this.side.open);
        assert_eq!(this.side.tool, Tool::Changes);
        assert!(this.composer.read(cx).focus_handle(cx).is_focused(window));
    });
    let folder = tempfile::tempdir().expect("a session folder");
    std::fs::create_dir_all(folder.path().join("src")).expect("make src");
    std::fs::write(folder.path().join("src/b.rs"), "fn b() {}").expect("write the linked file");
    workspace.update_in(cx, |this, window, cx| {
        this.side.dir = folder.path().to_path_buf();
        this.show_workbench_target(Target::File("src/b.rs".into(), Some(17)), true, window, cx);
    });
    cx.run_until_parked();
    workspace.update_in(cx, |this, window, cx| {
        assert_eq!(this.side.tool, Tool::Files);
        assert!(this.composer.read(cx).focus_handle(cx).is_focused(window));
        assert_eq!(this.composer.read(cx).value().as_str(), "Unsent draft");
    });
}

#[gpui_kit::test]
fn following_never_opens_a_linked_file_that_is_not_there(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "navigation-missing-file");
    let folder = tempfile::tempdir().expect("a session folder");
    std::fs::create_dir_all(folder.path().join("src")).expect("make a folder the link names");
    workspace.update_in(cx, |this, window, cx| {
        this.side.dir = folder.path().to_path_buf();
        this.show_workbench_target(Target::Changes("src/a.rs".into()), false, window, cx);
        assert_eq!(this.side.tool, Tool::Changes);
        // Not written yet, a folder, and a path on the agent's sandbox.
        for path in ["demo.gif", "src", "/mnt/data/demo.gif"] {
            this.show_workbench_target(Target::File(path.into(), None), true, window, cx);
        }
    });
    cx.run_until_parked();
    workspace.update_in(cx, |this, window, cx| {
        assert_eq!(this.side.tool, Tool::Changes, "the pane stays where it was");
        // A person's own click still opens it, to show why.
        this.show_workbench_target(Target::File("demo.gif".into(), None), false, window, cx);
        assert_eq!(this.side.tool, Tool::Files);
    });
}

#[gpui_kit::test]
fn following_opens_the_copy_the_agent_read_in_another_worktree(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "navigation-other-worktree");
    // Two worktrees with the same file; the agent read `b`'s, and the session runs in `a`.
    let trees = tempfile::tempdir().expect("a folder for the worktrees");
    let trees = trees.path().canonicalize().expect("the folder");
    for tree in ["a", "b"] {
        std::fs::create_dir_all(trees.join(tree).join("src")).expect("make the worktree");
        std::fs::write(trees.join(tree).join(".git"), "gitdir: /r").expect("mark the worktree");
        std::fs::write(trees.join(tree).join("src/same.rs"), tree).expect("write its copy");
    }
    let read = trees.join("b/src/same.rs").to_string_lossy().into_owned();
    workspace.update_in(cx, |this, window, cx| {
        this.side.dir = trees.join("a");
        this.show_workbench_target(Target::Changes("src/a.rs".into()), false, window, cx);
        let files =
            cx.new(|cx| crate::side::files::Files::new(crate::side::Place::Here(Err("none".into())), window, cx));
        this.side.files = Some(files);
        let call = serde_json::from_value(
            serde_json::json!({"kind":"tool","id":"read-b","name":"Read","input":{"file_path": read}}),
        )
        .expect("a call");
        let result = serde_json::from_value(
            serde_json::json!({"kind":"toolResult","id":"read-b","output":"b","isError":false,"truncated":false}),
        )
        .expect("its result");
        if let View::Code(view) = &mut this.view {
            view.set(crate::code::Transcript::of(vec![(1, 0, call), (2, 0, result)]), cx);
        }
        this.show_workbench_target(Target::File("same.rs".into(), Some(3)), true, window, cx);
    });
    cx.run_until_parked();
    workspace.update_in(cx, |this, _, cx| {
        assert_eq!(this.side.tool, Tool::Files);
        let files = this.side.files.clone().expect("Files");
        assert_eq!(files.read(cx).fixture_open_path(cx), Some(read.clone()));
    });
}

#[gpui_kit::test]
fn a_picked_or_shown_tool_puts_the_keys_in_its_field(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "navigation-tool-focus");
    workspace.update_in(cx, |this, window, cx| {
        this.navigation("navigation-tool-focus").interact();
        this.pick_tool(Tool::Files, window, cx);
        assert_eq!(this.side.tool, Tool::Files);
        // This fixture has no folder, so the tool is given one that reads nothing.
        let files =
            cx.new(|cx| crate::side::files::Files::new(crate::side::Place::Here(Err("none".into())), window, cx));
        this.side.files = Some(files.clone());
        this.focus_side_tool(window, cx);
        assert!(files.read(cx).filter_focused(window, cx), "Files opens in its filter");

        this.toggle_side_pane(window, cx);
        assert!(!this.side.open);
        assert!(this.composer.read(cx).focus_handle(cx).is_focused(window));

        this.toggle_side_pane(window, cx);
        assert!(this.side.open);
        assert!(
            files.read(cx).filter_focused(window, cx),
            "showing the Workbench again focuses its tool"
        );
    });
}

#[gpui_kit::test]
fn following_an_execution_without_the_commands_list_never_opens_a_human_shell(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "navigation-execution");
    workspace.update_in(cx, |this, window, cx| {
        // An agent's command shows only in the session's Commands list, the Terminal tab a person reaches too.
        assert!(this.side.shared_terminal().is_none());
        this.show_workbench_target(Target::Terminal("execution-exact".into()), true, window, cx);
        assert!(!this.side.open);
        assert_ne!(this.side.tool, Tool::Terminal);
        assert!(this.side.terminals.is_empty());
        assert!(this.runner.is_none());
    });
}

#[gpui_kit::test]
fn manual_tool_selection_hiding_and_resizing_preserve_the_navigation_contract(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "navigation-manual");
    workspace.update_in(cx, |this, window, cx| {
        // Start in Manual so this test never persists a preference to the person's settings.
        this.navigation("navigation-manual").interact();
        this.pick_tool(Tool::Files, window, cx);
        assert_eq!(this.side.tool, Tool::Files);
        assert!(this.navigation("navigation-manual").manual);
        let files = this.side.files.clone();
        this.close_side_drawer(window, cx);
        this.offer_workbench(Target::Changes("src/agent.rs".into()), true, cx);
        let target = this
            .navigation("navigation-manual")
            .take(true, true, Instant::now() + Duration::from_secs(1));
        assert!(target.is_none());
        assert!(!this.side.open);
        assert_eq!(this.side.tool, Tool::Files);
        assert_eq!(this.side.files, files);
        this.side.layout.resize(900., false);
        assert!(this.navigation("navigation-manual").manual);
        assert!(!this.side.open);
    });
}

#[gpui_kit::test]
fn a_late_page_cannot_replace_manual_work_or_another_sessions_view(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "navigation-late-page");
    workspace.update_in(cx, |this, _, _| {
        this.navigation("navigation-late-page")
            .offer(Target::Page("pag_new".into()), true, 0);
        this.side
            .pending_pages
            .insert("pag_new".into(), ("navigation-late-page".into(), 0));
        this.navigation("navigation-late-page").interact();
        assert!(!this.accepts_automatic_page("/app/pages/pag_new", true));
        this.side
            .pending_pages
            .insert("pag_old".into(), ("other-session".into(), 0));
        assert!(!this.accepts_automatic_page("/app/pages/pag_old", true));
        this.side
            .pending_pages
            .insert("pag_inactive".into(), ("navigation-late-page".into(), 1));
        assert!(!this.accepts_automatic_page("/app/pages/pag_inactive", false));
        assert!(this.page.is_none());
    });
}

#[gpui_kit::test]
fn browser_permission_is_visible_in_conversation_without_opening_or_selecting_the_pane(cx: &mut TestAppContext) {
    use crate::side::browser::agent::{Answer, REQUESTS};
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "navigation-permission-card");
    workspace.update_in(cx, |this, window, cx| {
        let id = "navigation-permission-card";
        let browser = this.make_browser(id.into(), None, window, cx).expect("fixture browser");
        let tool = this.side.tool;
        let request = browser.read(cx).fixture_request("https://example.invalid/");
        assert!(
            this.render_browser_permission(id, cx.global::<crate::pages::Theme>().0, cx)
                .is_some()
        );
        assert!(!this.side.open);
        assert_eq!(this.side.tool, tool);
        assert!(!this.navigation(id).manual);
        browser.update(cx, |browser, cx| {
            browser.answer_request_id(&request.id, Answer::Allow, cx)
        });
        let tabs = browser.read(cx).tabs().0;
        assert_eq!(tabs.len(), 1);
        assert!(tabs[0].agent_allowed && tabs[0].agent_created);
        assert_eq!(tabs[0].url, "https://example.invalid/");
        let prepared_id = tabs[0].id.clone();
        assert!(
            browser
                .read(cx)
                .prepared_agent_tab(&prepared_id, "https://example.invalid/")
        );
        assert!(
            !browser
                .read(cx)
                .prepared_agent_tab(&prepared_id, "https://different.invalid/")
        );
        assert_eq!(browser.read(cx).live_agent_tab(), Some(prepared_id.clone()));
        let reused = browser.update(cx, |browser, _| {
            browser.take_prepared_agent_tab(&prepared_id, "https://example.invalid/")
        });
        assert_eq!(reused, Some(prepared_id.clone()));
        assert_eq!(browser.read(cx).tabs().0.len(), 1);
        assert!(
            browser
                .update(cx, |browser, _| browser
                    .take_prepared_agent_tab(&prepared_id, "https://example.invalid/"))
                .is_none()
        );
        assert!(!this.side.open);
        assert_eq!(this.side.tool, tool);
        assert!(!this.navigation(id).manual);
        REQUESTS.remove(&request.id);
        let next = browser.read(cx).fixture_request("https://another.invalid/");
        browser.update(cx, |browser, cx| {
            browser.answer_request_id(&request.id, Answer::Allow, cx)
        });
        assert_eq!(
            browser.read(cx).tabs().0.len(),
            1,
            "an old permission card cannot grant a newer request"
        );
        REQUESTS.remove(&next.id);
    });
}

#[gpui_kit::test]
fn primary_events_follow_changes_then_the_presented_file_in_an_open_pane(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "navigation-event-workflow");
    // Following opens a linked file only once it is there.
    let folder = tempfile::tempdir().expect("a session folder");
    std::fs::create_dir_all(folder.path().join("src")).expect("make src");
    std::fs::write(folder.path().join("src/presented.rs"), "fn presented() {}").expect("write the linked file");
    workspace.update_in(cx, |this, window, cx| {
        this.toggle_side_pane(window, cx);
        this.side.dir = folder.path().to_path_buf();
        this.follow_workbench(cx);
        this.composer.update(cx, |input, cx| { input.set_value("Unsent question",window,cx); input.focus(window,cx); });
        let call = serde_json::from_value(serde_json::json!({"kind":"tool","id":"edit-one","name":"Edit","input":{"file_path":"src/edited.rs"}})).unwrap();
        let result = serde_json::from_value(serde_json::json!({"kind":"toolResult","id":"edit-one","output":"Updated src/edited.rs","isError":false,"truncated":false})).unwrap();
        let mut events = vec![(1,0,call),(2,0,result)];
        assert!(window.is_window_active(), "the workflow runs in the active session window");
        if let View::Code(view) = &mut this.view { view.set(crate::code::Transcript::of(events.clone()),cx); }
        this.sync_workbench_navigation_at(Instant::now(),window,cx);
        this.sync_workbench_navigation_at(Instant::now()+Duration::from_secs(1),window,cx);
        assert!(this.side.open);
        assert_eq!(this.side.tool,Tool::Changes);
        assert_eq!(this.navigation("navigation-event-workflow").latest,Some(Target::Changes("src/edited.rs".into())));
        let text = serde_json::from_value(serde_json::json!({"kind":"text","id":"answer","text":"See [the file](src/presented.rs:21)."})).unwrap();
        events.push((3,0,text));
        if let View::Code(view) = &mut this.view { view.set(crate::code::Transcript::of(events),cx); }
        this.sync_workbench_navigation_at(Instant::now(),window,cx);
        this.sync_workbench_navigation_at(Instant::now()+Duration::from_secs(1),window,cx);
    });
    cx.run_until_parked();
    workspace.update_in(cx, |this, window, cx| {
        assert_eq!(this.side.tool, Tool::Files);
        assert_eq!(
            this.navigation("navigation-event-workflow").latest,
            Some(Target::File("src/presented.rs".into(), Some(21)))
        );
        assert_eq!(this.composer.read(cx).value().as_str(), "Unsent question");
        assert!(this.composer.read(cx).focus_handle(cx).is_focused(window));
    });
}

fn switch_session(this: &mut Workspace, id: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    let mut view = CodeView::new(id.into(), window, cx);
    view.loaded = true;
    this.view = View::Code(view);
    this.sync_side(window, cx);
}

#[gpui_kit::test]
fn switching_sessions_restores_only_that_sessions_visibility(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "visibility-one");
    cx.simulate_resize(size(px(1440.), px(900.)));
    cx.run_until_parked();
    workspace.update_in(cx, |this, window, cx| {
        assert!(!this.side.open);
        this.pick_tool(Tool::Files, window, cx);
        assert!(this.side.open);
        switch_session(this, "visibility-two", window, cx);
        assert!(!this.side.open, "another session starts closed");
        this.toggle_side_pane(window, cx);
        assert!(this.side.open, "an explicit toggle opens that session");
        this.close_side_drawer(window, cx);
        switch_session(this, "visibility-one", window, cx);
        assert!(this.side.open, "the first session keeps its open choice");
        this.view = View::NewCode;
        this.sync_side(window, cx);
        assert!(!this.side.open, "New session has its own visibility");
        switch_session(this, "visibility-one", window, cx);
        assert!(this.side.open);
        switch_session(this, "visibility-two", window, cx);
        assert!(!this.side.open, "the second session keeps its closed choice");
    });
}

#[gpui_kit::test]
fn switching_sessions_in_a_drawer_preserves_their_wide_pane_choices(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "drawer-one");
    cx.simulate_resize(size(px(1440.), px(900.)));
    cx.run_until_parked();
    workspace.update_in(cx, |this, window, cx| this.pick_tool(Tool::Files, window, cx));
    cx.simulate_resize(size(px(900.), px(900.)));
    cx.run_until_parked();
    workspace.update_in(cx, |this, window, cx| {
        assert!(!this.side.open);
        switch_session(this, "drawer-two", window, cx);
        this.pick_tool(Tool::Files, window, cx);
        assert!(this.side.open, "the second session opens only its transient drawer");
    });
    cx.simulate_resize(size(px(1440.), px(900.)));
    cx.run_until_parked();
    workspace.update_in(cx, |this, window, cx| {
        assert!(!this.side.open, "opening its drawer does not open its wide pane");
        switch_session(this, "drawer-one", window, cx);
        assert!(this.side.open, "the first session retains its original wide pane");
    });
}

#[gpui_kit::test]
fn fresh_primary_activity_stays_in_the_background_until_explicitly_opened(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "navigation-starts-closed");
    workspace.update_in(cx, |this, window, cx| {
        let call = serde_json::from_value(serde_json::json!({
            "kind":"tool", "id":"edit-closed", "name":"Edit", "input":{"file_path":"src/edited.rs"}
        })).unwrap();
        let result = serde_json::from_value(serde_json::json!({
            "kind":"toolResult", "id":"edit-closed", "output":"Updated src/edited.rs", "isError":false, "truncated":false
        })).unwrap();
        if let View::Code(view) = &mut this.view {
            view.set(crate::code::Transcript::of(vec![(1, 0, call), (2, 0, result)]), cx);
        }
        this.sync_workbench_navigation_at(Instant::now(), window, cx);
        this.sync_workbench_navigation_at(Instant::now() + Duration::from_secs(1), window, cx);
        assert!(!this.side.open);
        assert!(!this.navigation("navigation-starts-closed").manual);
        let target = Target::Changes("src/edited.rs".into());
        assert_eq!(this.navigation("navigation-starts-closed").unread, vec![target.clone()]);
        this.view_workbench_once(target, window, cx);
        assert!(this.side.open);
        assert_eq!(this.side.tool, Tool::Changes);
        assert!(this.navigation("navigation-starts-closed").manual);
    });
}

#[gpui_kit::test]
fn pending_automatic_pages_respect_a_sessions_default_closed_pane(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "navigation-closed-page");
    workspace.update_in(cx, |this, _, _| {
        this.navigation("navigation-closed-page")
            .offer(Target::Page("pag_closed".into()), true, 0);
        this.side
            .pending_pages
            .insert("pag_closed".into(), ("navigation-closed-page".into(), 0));
        assert!(!this.side.open);
        assert!(!this.accepts_automatic_page("/app/pages/pag_closed", true));
        assert!(this.page.is_none());
    });
}

#[gpui_kit::test]
fn navigation_controls_stay_inside_the_workbench_and_hidden_activity_stays_in_conversation(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, "navigation-control-placement");
    workspace.update_in(cx, |this, _, cx| {
        let palette = cx.global::<crate::pages::Theme>().0;
        for sidebar_open in [true, false] {
            this.sidebar_open = sidebar_open;
            this.side.open = false;
            assert!(this.render_navigation(palette, cx).is_none());
            assert!(this.render_background_workbench(palette, cx).is_none());
        }
        this.navigation("navigation-control-placement")
            .offer(Target::Changes("src/agent.rs".into()), true, 0);
        assert!(this.render_navigation(palette, cx).is_none());
        assert!(this.render_background_workbench(palette, cx).is_some());
        this.side.open = true;
        assert!(this.render_navigation(palette, cx).is_some());
        assert!(this.render_background_workbench(palette, cx).is_none());
    });
}
