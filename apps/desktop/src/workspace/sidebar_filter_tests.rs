//! Headless checks for Projects search scope, recovery, and keyboard focus.

use super::{CodeProject, CodeSession, Workspace, matches, searches_messages, snippet_parts};
use gpui_kit::{Entity, Focusable, Task, TestAppContext, VisualTestContext, point, px};

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
        workspace.sessions = vec![crate::model::SessionSummary {
            id: "chat".into(),
            title: "Gateway release".into(),
            day: None,
            at_ms: None,
        }];
        let session = |id: &str, title: &str| {
            let summary = serde_json::from_value(serde_json::json!({
                "id": id, "projectId": "p", "title": title, "agent": "claude", "model": null,
                "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0,
                "nexusSessions": [], "contextTokens": null, "contextWindow": null,
                "createdAt": 0, "updatedAt": 1, "archived": false
            }))
            .unwrap();
            CodeSession::of(summary)
        };
        workspace.projects = vec![CodeProject {
            id: "p".into(),
            name: "Gateway".into(),
            branch: Some("main".into()),
            pinned: false,
            remotes: vec![],
            machine: None,
            added_at: 0,
            sessions: vec![
                session("retries", "Fix provider retries"),
                session("picker", "Polish model picker"),
            ],
        }];
        workspace.folded_projects.insert("p".into());
    });
    (workspace, cx)
}

#[gpui_kit::test]
fn project_search_stays_local_and_escape_restores_focus_and_expansion(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.sidebar_filter.scroll.set_offset(point(px(0.), px(-64.)));
        workspace.toggle_project_filter(window, cx);
        assert!(workspace.project_filter_open());
        assert!(
            workspace
                .render_filter_field(crate::palette::Palette::for_choice(
                    crate::palette::ThemeChoice::Light,
                    window.appearance()
                ))
                .is_none()
        );
        let input = workspace.sidebar_filter.input.clone().unwrap();
        assert!(input.focus_handle(cx).is_focused(window));
        workspace.filter_typed("retries".into(), cx);
        assert!(workspace.filter_kept().chats.is_empty());
        let kept = workspace.filter_kept();
        assert_eq!(kept.projects.len(), 1);
        assert_eq!(
            kept.projects[0]
                .1
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            ["retries"]
        );
        assert_eq!(workspace.sessions[0].title, "Gateway release");
        assert!(
            workspace.sidebar_filter.pause.is_none(),
            "project search must not start message search"
        );
        assert!(workspace.filter_hits().is_empty());
        assert!(workspace.folded_projects.contains("p"));
        assert!(workspace.filter_key("escape", window, cx));
        assert!(workspace.project_filter_open());
        assert!(!workspace.filtering());
        assert!(workspace.filter_key("escape", window, cx));
        assert!(!workspace.sidebar_filter.open);
        assert!(
            workspace
                .sidebar_filter
                .project_trigger
                .as_ref()
                .unwrap()
                .is_focused(window)
        );
        assert_eq!(workspace.sidebar_filter.scroll.offset(), point(px(0.), px(-64.)));
        assert!(workspace.folded_projects.contains("p"));
    });
}

#[gpui_kit::test]
fn project_search_matches_parent_branch_and_titles_and_rejects_stale_broad_hits(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_filter(window, cx);
        workspace.filter_typed("Gateway".into(), cx);
        let stale = workspace.sidebar_filter.seq | super::SEQ_BIT;
        assert_eq!(workspace.filter_kept().chats.len(), 1);
        workspace.toggle_project_filter(window, cx);
        assert!(workspace.sidebar_filter.query.is_empty());
        workspace.filter_searched(stale, Ok(vec![]), cx);
        assert!(workspace.filter_hits().is_empty());
        workspace.filter_typed("GATEWAY main".into(), cx);
        assert_eq!(workspace.filter_kept().projects[0].1.len(), 2);
        assert!(
            workspace
                .filter_targets()
                .iter()
                .all(|(target, _)| matches!(target, super::Open::Code(_)))
        );
        workspace.filter_typed("missing".into(), cx);
        assert!(workspace.filter_kept().projects.is_empty());
        workspace.clear_filter(window, cx);
        assert_eq!(workspace.filter_kept().projects[0].1.len(), 2);
        workspace.open_filter(window, cx);
        assert!(!workspace.project_filter_open());
        assert!(workspace.sidebar_filter.query.is_empty());
        assert_eq!(workspace.filter_kept().chats.len(), 1);
    });
}

#[test]
fn every_word_has_to_match_somewhere_in_any_case() {
    assert!(matches("", &["anything"]));
    assert!(matches("FIX", &["Fix the login"]));
    assert!(matches("nexus fix", &["Fix the login", "nexus"]));
    assert!(!matches("nexus deploy", &["Fix the login", "nexus"]));
    assert!(matches("  fix   login ", &["Fix the login"]));
}

#[test]
fn messages_are_searched_from_three_characters() {
    assert!(!searches_messages("fi"));
    assert!(!searches_messages("  fi  "));
    assert!(searches_messages(" fix"));
}

#[test]
fn a_snippet_splits_into_plain_and_matched_runs() {
    let parts = snippet_parts("fix the \u{2}login\u{3} bug\nnow");
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], ("fix the ".to_string(), false));
    assert_eq!(parts[1], ("login".to_string(), true));
    assert_eq!(parts[2], (" bug now".to_string(), false));
    assert!(snippet_parts("").is_empty());
}
