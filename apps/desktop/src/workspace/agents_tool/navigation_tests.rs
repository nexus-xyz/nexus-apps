//! Exercise panel navigation against the real workspace without starting an agent or reading a store.
use super::*;
use crate::workspace::{code_view::CodeView, machine_source::MachineSource};
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, VisualTestContext, point, px};

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
        workspace._code_poll = Task::ready(());
        let mut view = CodeView::new("navigation-test".into(), window, cx);
        view.loaded = true;
        workspace.view = View::Code(view);
        workspace.side.fixture_session("navigation-test");
        let mut state = AgentsTool::new("navigation-test".into(), SessionAgent::Claude, cx);
        let events = vec![StoredEvent {
            thread_id: "navigation-test".into(),
            seq: 1,
            at: 1,
            event: WbEvent::Tool {
                id: "child".into(),
                name: "Agent".into(),
                input: json!({"description": "Inspect layout"}),
                parent: None,
            },
        }];
        let mut crew = crew_fold::fold_crew(&events, SessionAgent::Claude);
        // Ended nodes do not schedule the live activity timer.
        for node in &mut crew.nodes {
            node.status = AgentStatus::Done;
        }
        state.crew = Some(crew);
        state.events = Arc::new(events);
        state.next = Instant::now() + Duration::from_secs(3600);
        workspace.side.agents = Some(state);
        workspace
    });
    workspace.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    (workspace, cx)
}

#[gpui_kit::test]
fn enter_and_back_preserve_the_conversation_overview_scroll_and_cursor(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        let state = workspace.side.agents.as_mut().unwrap();
        state.cursor = Some("child".into());
        state.overview_scroll.set_offset(point(px(0.), px(-120.)));
        assert!(workspace.agent_key("enter", cx));
        let state = workspace.side.agents.as_ref().unwrap();
        assert!(state.detail_in_pane);
        assert_eq!(state.selected.as_deref(), Some("child"));
        assert!(
            !workspace.agent_view_open(),
            "the lead conversation stays in the main area"
        );
        assert!(!workspace.side.maximized);
        workspace.side.open = true;
        workspace.side.select_tool(crate::workspace::side_pane::Tool::Agents);
        workspace.side.layout.drawer = true;
        workspace.side.agents.as_ref().unwrap().focus.focus(window, cx);
        workspace.on_keystroke(&Keystroke::parse("escape").unwrap(), window, cx);
        assert!(workspace.side.open, "Escape returns inside the drawer before hiding it");
        let state = workspace.side.agents.as_ref().unwrap();
        assert!(!state.detail_in_pane);
        assert_eq!(state.cursor.as_deref(), Some("child"));
        assert_eq!(state.overview_scroll.offset(), point(px(0.), px(-120.)));
        assert!(state.focus.is_focused(window));
        assert!(matches!(&workspace.view, View::Code(view) if view.id == "navigation-test"));
    });
}

#[gpui_kit::test]
fn lead_details_and_missing_agents_keep_a_working_return_path(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_agent_detail(crew_fold::LEAD_ID.into(), cx);
        let state = workspace.side.agents.as_ref().unwrap();
        assert!(state.detail_in_pane);
        assert_eq!(state.selected.as_deref(), Some(crew_fold::LEAD_ID));
        assert!(!workspace.agent_view_open());
        workspace.open_agent_detail("missing".into(), cx);
        workspace.render_agent_panel(
            crate::palette::Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark),
            cx,
        );
        workspace.back_to_agents(window, cx);
        assert!(workspace.side.agents.as_ref().unwrap().selected.is_none());
    });
}

#[gpui_kit::test]
fn agents_ignore_the_workbench_maximize_shortcut(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.side.open = true;
        workspace.side.select_tool(crate::workspace::side_pane::Tool::Agents);
        workspace.toggle_maximize_side(window, cx);
        assert!(!workspace.side.maximized);
        workspace.side.select_tool(crate::workspace::side_pane::Tool::Files);
        workspace.toggle_maximize_side(window, cx);
        assert!(
            workspace.side.maximized,
            "other tools keep their existing maximize behavior"
        );
    });
}

#[gpui_kit::test]
fn linked_agent_inspection_switches_sessions_only_after_open_conversation(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        let summary = |id: &str, parent: Option<&str>| -> ThreadSummary {
            serde_json::from_value(json!({
                "id": id, "projectId": "p", "title": id, "agent": "claude", "model": null,
                "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.,
                "nexusSessions": [], "contextTokens": null, "contextWindow": null,
                "createdAt": 1, "updatedAt": 2, "archived": false, "parentThreadId": parent,
            }))
            .unwrap()
        };
        let root = summary("navigation-test", None);
        let child = summary("linked-session", Some("navigation-test"));
        let state = workspace.side.agents.as_mut().unwrap();
        state.crew = Some(state.crew.as_ref().unwrap().with_linked_sessions(Some(&root), &[child]));
        assert!(state.crew.as_ref().unwrap().resolve("session:linked-session").is_some());
        workspace.open_agent_detail("session:linked-session".into(), cx);
        let state = workspace.side.agents.as_ref().unwrap();
        assert!(state.detail_in_pane);
        assert!(
            state.pending_session.is_none(),
            "inspection must not navigate to another session"
        );
        assert!(!workspace.agent_view_open());
        workspace.open_agent_conversation(window, cx);
        let state = workspace.side.agents.as_ref().unwrap();
        assert_eq!(state.pending_session.as_deref(), Some("linked-session"));
        assert!(!state.detail_in_pane);
    });
}
