//! Headless production-path tests. No credentials, settings writes, or agent processes are used.

use super::{CodeForm, LaunchMenu, LaunchRow, PanelPick};
use crate::model::ModelChoice;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use crate::workspace::{Auth, View, Workspace, machine_source::MachineSource, model_menu::MenuRow};
use gpui_kit::{Entity, Focusable, Keystroke, TestAppContext, VisualTestContext, point, px};
use nexus_workbench::{
    api::AgentInfo,
    event::{Effort, PermissionMode, SessionAgent},
};
use std::sync::Arc;

fn fixture(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
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
        // Native file watchers can wake GPUI from an OS thread when other store tests write.
        workspace._code_poll = gpui_kit::Task::ready(());
        workspace
    });
    workspace.update_in(cx, |workspace, _, _| {
        workspace.code_form = CodeForm::default();
        workspace.code_form.org = None;
        workspace.auth = Auth::SignedIn;
        workspace.view = View::Chat;
        workspace.models = vec![
            chat_model("openai/gpt-5", "openai", true, false),
            chat_model("anthropic/claude-5", "anthropic", true, false),
            chat_model("openai/gpt-6", "openai", false, false),
            chat_model("openai/gpt-7", "openai", true, true),
        ];
        workspace.code_agents = vec![
            agent(SessionAgent::Claude, &["sonnet", "opus"]),
            agent(SessionAgent::Codex, &["gpt-5", "gpt-6"]),
        ];
    });
    (workspace, cx)
}

fn chat_model(id: &str, maker: &str, usable: bool, off: bool) -> ModelChoice {
    ModelChoice {
        id: id.into(),
        name: id.into(),
        maker: maker.into(),
        hint: None,
        usable,
        off,
        output_usd: None,
    }
}

fn agent(agent: SessionAgent, models: &[&str]) -> AgentInfo {
    AgentInfo {
        agent,
        path: Some(format!("/fixture/{}", agent.as_str())),
        version: None,
        problem: None,
        models: Some(
            models
                .iter()
                .map(|id| nexus_workbench::agent_models::Model {
                    id: (*id).into(),
                    name: (*id).into(),
                    resolved_id: None,
                    efforts: Vec::new(),
                })
                .collect(),
        ),
    }
}

#[gpui_kit::test]
fn chat_open_clears_search_and_restores_the_picked_row_and_focus(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.model = Some(workspace.models[1].clone());
        workspace.model_query = "gpt".into();
        workspace
            .model_search
            .update(cx, |input, cx| input.set_value("gpt", window, cx));
        workspace.model_scroll.set_offset(point(px(0.), px(-300.)));
        workspace.toggle_model_menu(window, cx);
        assert_eq!(workspace.model_cursor, 1);
        assert_eq!(workspace.model_scroll.offset(), point(px(0.), px(0.)));
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.model_query.is_empty());
        assert!(workspace.model_search.read(cx).value().is_empty());
        assert_eq!(
            workspace.model_cursor, 1,
            "opening must preserve the restored cursor after pending tasks"
        );
        assert!(workspace.model_search.read(cx).focus_handle(cx).is_focused(window));
        assert_eq!(workspace.menu_maker().as_deref(), Some("anthropic"));
        assert_eq!(
            workspace.model.as_ref().map(|m| m.id.as_str()),
            Some("anthropic/claude-5")
        );
    });
}

#[gpui_kit::test]
fn chat_query_changes_reset_scroll_and_clearing_restores_the_section(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.toggle_model_menu(window, cx);
        workspace.model_maker = Some("openai".into());
        workspace.sync_model_rows();
        workspace.model_cursor = 2;
        workspace.model_scroll.set_offset(point(px(0.), px(-300.)));
        workspace
            .model_search
            .update(cx, |input, cx| input.replace_all("claude", window, cx));
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        assert_eq!(workspace.model_query, "claude");
        assert_eq!(workspace.menu_rows(), [MenuRow::Model(1)]);
        assert_eq!(workspace.model_cursor, 0);
        assert_eq!(workspace.model_scroll.offset(), point(px(0.), px(0.)));
        workspace
            .model_search
            .update(cx, |input, cx| input.replace_all(" \t ", window, cx));
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(workspace.menu_maker().as_deref(), Some("openai"));
        assert_eq!(
            workspace.menu_rows(),
            [MenuRow::Auto, MenuRow::Model(0), MenuRow::Model(2)]
        );
        assert!(
            workspace.model.is_none(),
            "searching and clearing do not commit a model"
        );
    });
}

#[gpui_kit::test]
fn chat_keyboard_rejects_unavailable_rows_and_commits_usable_models_and_auto(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.model = Some(workspace.models[1].clone());
        workspace.toggle_model_menu(window, cx);
        workspace.model_maker = Some("openai".into());
        workspace.sync_model_rows();
        workspace.model_cursor = 2;
        workspace.on_keystroke(&Keystroke::parse("enter").unwrap(), window, cx);
        assert!(
            workspace.overlay.model_menu(),
            "unavailable row does not close the menu"
        );
        assert_eq!(
            workspace.model.as_ref().map(|m| m.id.as_str()),
            Some("anthropic/claude-5")
        );
        workspace.pick_menu_row(MenuRow::Model(3), window, cx);
        assert!(
            workspace.overlay.model_menu(),
            "policy-off rows cannot activate even with stale input"
        );
        workspace.model_cursor = 1;
        workspace.on_keystroke(&Keystroke::parse("enter").unwrap(), window, cx);
        assert!(!workspace.overlay.model_menu());
        assert_eq!(workspace.model.as_ref().map(|m| m.id.as_str()), Some("openai/gpt-5"));
        assert!(workspace.composer.read(cx).focus_handle(cx).is_focused(window));
        workspace.toggle_model_menu(window, cx);
        workspace.sync_model_rows();
        workspace.model_cursor = 0;
        workspace.on_keystroke(&Keystroke::parse("enter").unwrap(), window, cx);
        assert!(workspace.model.is_none());
        assert!(!workspace.overlay.model_menu());
        assert!(workspace.composer.read(cx).focus_handle(cx).is_focused(window));
        workspace.toggle_model_menu(window, cx);
        workspace.on_keystroke(&Keystroke::parse("escape").unwrap(), window, cx);
        assert!(!workspace.overlay.model_menu());
        assert!(workspace.composer.read(cx).focus_handle(cx).is_focused(window));
    });
}

#[gpui_kit::test]
fn code_picks_switch_agents_and_clear_effort_only_for_changed_models(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.view = View::NewCode;
        workspace.code_form.agent = SessionAgent::Claude;
        workspace.code_form.model = Some("sonnet".into());
        workspace.code_form.effort = Some(Effort::High);
        workspace.code_form.remember();
        workspace.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
        workspace.prepare_model_search(window, cx);
        // Default, then Auto, then the agent's models: the cursor starts on the saved model.
        assert_eq!(workspace.model_cursor, 2);
        assert_eq!(
            workspace.launch_rows()[1].model.as_deref(),
            Some(crate::workspace::code_picker::AUTO)
        );
        assert_eq!(
            workspace.launch_rows()[workspace.model_cursor].model.as_deref(),
            Some("sonnet")
        );
        workspace.model_query = "gpt-5".into();
        let row = LaunchRow {
            agent: SessionAgent::Codex,
            model: Some("gpt-5".into()),
        };
        workspace.pick_model_row(row.clone(), window, cx);
        assert_eq!(workspace.code_form.agent, SessionAgent::Codex);
        assert_eq!(workspace.code_form.model.as_deref(), Some("gpt-5"));
        assert_eq!(workspace.code_form.effort, None);
        assert_eq!(workspace.overlay.launch_menu(), None);
        assert!(workspace.composer.read(cx).focus_handle(cx).is_focused(window));
        workspace.code_form.effort = Some(Effort::High);
        workspace.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
        workspace.pick_model_row(row, window, cx);
        assert_eq!(workspace.code_form.effort, Some(Effort::High));
        workspace.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
        workspace.model_query.clear();
        workspace.pick_model_row(
            LaunchRow {
                agent: SessionAgent::Codex,
                model: None,
            },
            window,
            cx,
        );
        assert_eq!(workspace.code_form.effort, None);
        assert_eq!(workspace.code_form.model, None);
    });
}

#[gpui_kit::test]
fn code_search_empty_results_stale_rows_and_missing_agents_cannot_commit(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.view = View::NewCode;
        workspace.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
        workspace.prepare_model_search(window, cx);
        workspace.model_query = "no matching model".into();
        assert!(workspace.launch_rows().is_empty());
        workspace.on_keystroke(&Keystroke::parse("enter").unwrap(), window, cx);
        assert_eq!(workspace.overlay.launch_menu(), Some(LaunchMenu::Model));
        assert_eq!(workspace.code_form.agent, SessionAgent::Claude);
        assert_eq!(workspace.code_form.model, None);
        workspace.pick_model_row(
            LaunchRow {
                agent: SessionAgent::Codex,
                model: Some("gpt-5".into()),
            },
            window,
            cx,
        );
        assert_eq!(
            workspace.code_form.agent,
            SessionAgent::Claude,
            "a stale click cannot activate hidden search results"
        );
        workspace.model_query.clear();
        workspace.pick_model_row(
            LaunchRow {
                agent: SessionAgent::Claude,
                model: Some("removed".into()),
            },
            window,
            cx,
        );
        assert_eq!(workspace.code_form.model, None);
        workspace.code_agents.clear();
        workspace.pick_model_row(
            LaunchRow {
                agent: SessionAgent::Codex,
                model: None,
            },
            window,
            cx,
        );
        assert_eq!(workspace.code_form.agent, SessionAgent::Claude);
        assert_eq!(workspace.overlay.launch_menu(), Some(LaunchMenu::Model));
        workspace.dismiss_code_model_menu(window, cx);
        assert_eq!(workspace.overlay.launch_menu(), None);
        assert!(workspace.composer.read(cx).focus_handle(cx).is_focused(window));
    });
}

#[gpui_kit::test]
fn dismissed_code_picker_rejects_an_old_click(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.view = View::NewCode;
        workspace.pick_model_row(
            LaunchRow {
                agent: SessionAgent::Codex,
                model: Some("gpt-5".into()),
            },
            window,
            cx,
        );
        assert_eq!(workspace.overlay.launch_menu(), None);
        assert_eq!(workspace.code_form.agent, SessionAgent::Claude);
        assert_eq!(workspace.code_form.model, None);
    });
}

#[gpui_kit::test]
fn favorites_removal_reconciles_the_actual_picker_cursor(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.view = View::NewCode;
        workspace.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
        let first = LaunchRow {
            agent: SessionAgent::Claude,
            model: Some("sonnet".into()),
        };
        let last = LaunchRow {
            agent: SessionAgent::Codex,
            model: Some("gpt-5".into()),
        };
        workspace.code_form.toggle_favorite(&first);
        workspace.code_form.toggle_favorite(&last);
        workspace.prepare_model_search(window, cx);
        // Favorites list first, and each shows once.
        assert_eq!(workspace.launch_panel().1, 2);
        assert_eq!(workspace.launch_rows()[..2], [first.clone(), last.clone()]);
        assert_eq!(workspace.launch_rows().iter().filter(|row| **row == last).count(), 1);
        workspace.sync_model_rows();
        workspace.model_cursor = 1;
        workspace.code_form.toggle_favorite(&last);
        workspace.sync_model_rows();
        // Unstarred, the row moves back under its agent, and the cursor follows it.
        assert_eq!(workspace.launch_panel().1, 1);
        assert_eq!(workspace.launch_rows()[workspace.model_cursor], last);
        workspace.on_keystroke(&Keystroke::parse("enter").unwrap(), window, cx);
        assert_eq!(workspace.code_form.model.as_deref(), Some("gpt-5"));
        assert_eq!(workspace.overlay.launch_menu(), None);
    });
}

#[gpui_kit::test]
fn code_open_waits_for_the_catalog_before_restoring_the_picked_row(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.view = View::NewCode;
        workspace.code_form.agent = SessionAgent::Claude;
        workspace.code_form.model = Some("opus".into());
        let catalog = std::mem::take(&mut workspace.code_agents);
        workspace.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
        workspace.prepare_model_search(window, cx);
        workspace.sync_model_rows();
        assert!(workspace.launch_rows().is_empty());
        assert_eq!(workspace.model_cursor, 0);
        workspace.code_agents = catalog;
        workspace.sync_model_rows();
        assert_eq!(
            workspace.model_cursor, 3,
            "ready catalog restores the saved model, not Default or Auto"
        );
        assert_eq!(
            workspace.launch_rows()[workspace.model_cursor].model.as_deref(),
            Some("opus")
        );
    });
}

#[gpui_kit::test]
fn an_existing_code_session_offers_both_agents_without_applying_an_unconfirmed_switch(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        // Without a dedicated default entry, the agent's first model names its Default row.
        workspace.code_agents = vec![
            agent(SessionAgent::Claude, &["opus", "sonnet"]),
            agent(SessionAgent::Codex, &["gpt-5", "gpt-6"]),
        ];
        let thread = serde_json::from_value(serde_json::json!({
            "id": "picker-session", "projectId": "local-project", "title": "Fixture", "agent": "claude",
            "model": "sonnet", "permissionMode": "default", "effort": "high", "via": "nexus",
            "status": "idle", "costUsd": 0.0, "nexusSessions": [], "contextTokens": null,
            "contextWindow": null, "createdAt": 0, "updatedAt": 1, "archived": false,
        }))
        .expect("fixture thread");
        workspace.archived_code = vec![crate::code::CodeSession::of(thread)];
        workspace.projects = vec![crate::code::CodeProject {
            id: "unrelated-remote-project".into(),
            name: "Remote fixture".into(),
            branch: None,
            pinned: false,
            remotes: Vec::new(),
            sessions: Vec::new(),
            machine: Some("disconnected-fixture".into()),
            added_at: 0,
        }];
        workspace.code_form.project = Some("unrelated-remote-project".into());
        let mut view = crate::workspace::code_view::CodeView::new("picker-session".into(), window, cx);
        view.dock.menu = Some(crate::workspace::code_dock::Menu::Model);
        workspace.view = View::Code(view);
        workspace.prepare_model_search(window, cx);
        assert!(
            !workspace.picker_harness_waiting(),
            "the local session ignores the new form's remote target"
        );
        let rows = workspace.launch_rows();
        assert_eq!(rows.len(), 6);
        assert!(rows.iter().any(|row| row.agent == SessionAgent::Claude));
        assert!(rows.iter().any(|row| row.agent == SessionAgent::Codex));
        workspace.model_query = "SONNET".into();
        assert_eq!(
            workspace.launch_rows(),
            [LaunchRow {
                agent: SessionAgent::Claude,
                model: Some("sonnet".into())
            }]
        );
        workspace.pick_model_row(
            LaunchRow {
                agent: SessionAgent::Codex,
                model: Some("gpt-5".into()),
            },
            window,
            cx,
        );
        let session = workspace.code_session().expect("session stays open");
        assert_eq!(session.agent, SessionAgent::Claude);
        assert_eq!(session.model.as_deref(), Some("sonnet"));
        assert_eq!(session.effort, Some(Effort::High));
        assert!(
            workspace.session_pick().is_some(),
            "invalid selection leaves the menu open"
        );
        workspace.archived_code[0].machine = Some("disconnected-session-fixture".into());
        assert!(
            workspace.launch_rows().is_empty(),
            "an unknown remote cannot borrow the local catalog"
        );
        workspace.pick_model_row(
            LaunchRow {
                agent: SessionAgent::Claude,
                model: Some("opus".into()),
            },
            window,
            cx,
        );
        assert_eq!(workspace.archived_code[0].model.as_deref(), Some("sonnet"));
        workspace.archived_code[0].machine = None;
        assert_eq!(
            workspace.launch_rows().len(),
            1,
            "local catalog returns with the original session context"
        );
        workspace.dismiss_code_model_menu(window, cx);
        assert!(workspace.session_pick().is_none());
        assert!(workspace.composer.read(cx).focus_handle(cx).is_focused(window));
        if let View::Code(view) = &mut workspace.view {
            view.dock.menu = Some(crate::workspace::code_dock::Menu::Model);
        }
        workspace.archived_code.clear();
        assert!(
            workspace.launch_rows().is_empty(),
            "a missing session summary is not a new-session picker"
        );
        workspace.pick_model_row(
            LaunchRow {
                agent: SessionAgent::Codex,
                model: Some("gpt-5".into()),
            },
            window,
            cx,
        );
        assert_eq!(workspace.code_form.agent, SessionAgent::Claude);
        assert_eq!(workspace.code_form.model, None);
        workspace.on_keystroke(&Keystroke::parse("escape").unwrap(), window, cx);
        let View::Code(view) = &workspace.view else {
            panic!("the session view stays open")
        };
        assert_eq!(
            view.dock.menu, None,
            "Escape closes even after the session summary disappears"
        );
        assert!(workspace.composer.read(cx).focus_handle(cx).is_focused(window));
    });
}

#[gpui_kit::test]
fn session_picker_lists_only_agents_its_session_can_switch_to(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.code_agents = vec![
            agent(SessionAgent::Claude, &["opus"]),
            agent(SessionAgent::Codex, &["gpt-5"]),
            agent(SessionAgent::Antigravity, &["gemini-3.8-flash"]),
        ];
        let session = |id: &str, agent: &str, model: &str, via: &str| {
            crate::code::CodeSession::of(
                serde_json::from_value(serde_json::json!({
                    "id": id, "projectId": "local-project", "title": "Fixture", "agent": agent, "model": model,
                    "permissionMode": "default", "effort": null, "via": via, "status": "idle", "costUsd": 0.0,
                    "nexusSessions": [], "contextTokens": null, "contextWindow": null, "createdAt": 0,
                    "updatedAt": 1, "archived": false,
                }))
                .expect("fixture thread"),
            )
        };
        workspace.archived_code = vec![
            session("claude-session", "claude", "opus", "nexus"),
            session("agy-session", "antigravity", "gemini-3.8-flash", "subscription"),
        ];
        // New session's form points at a remote project, where Antigravity is not offered. The open local sessions
        // must not read it.
        workspace.projects = vec![crate::code::CodeProject {
            id: "unrelated-remote-project".into(),
            name: "Remote fixture".into(),
            branch: None,
            pinned: false,
            remotes: Vec::new(),
            sessions: Vec::new(),
            machine: Some("disconnected-fixture".into()),
            added_at: 0,
        }];
        workspace.code_form.project = Some("unrelated-remote-project".into());
        let mut listed = |id: &str, window: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<Workspace>| {
            let mut view = crate::workspace::code_view::CodeView::new(id.into(), window, cx);
            view.dock.menu = Some(crate::workspace::code_dock::Menu::Model);
            workspace.view = View::Code(view);
            workspace.prepare_model_search(window, cx);
            let mut agents: Vec<SessionAgent> = Vec::new();
            for row in workspace.launch_rows() {
                if !agents.contains(&row.agent) {
                    agents.push(row.agent);
                }
            }
            agents
        };
        // Claude Code switches to Codex only: agy's rows would fail after the confirmation.
        assert_eq!(
            listed("claude-session", window, cx),
            [SessionAgent::Claude, SessionAgent::Codex]
        );
        // An agy session switches to no agent, and keeps its own models.
        assert_eq!(listed("agy-session", window, cx), [SessionAgent::Antigravity]);
    });
}

#[gpui_kit::test]
fn a_favorite_shortcut_picks_the_row_its_badge_is_on(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.view = View::NewCode;
        workspace.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
        let removed = LaunchRow {
            agent: SessionAgent::Claude,
            model: Some("removed".into()),
        };
        let kept = LaunchRow {
            agent: SessionAgent::Codex,
            model: Some("gpt-5".into()),
        };
        workspace.code_form.toggle_favorite(&removed);
        workspace.code_form.toggle_favorite(&kept);
        workspace.prepare_model_search(window, cx);
        // The removed favorite is not listed, so the kept one is first and badged 1.
        assert_eq!(workspace.launch_panel().1, 1);
        assert_eq!(workspace.launch_rows()[0], kept);
        let key = if cfg!(target_os = "macos") { "cmd-1" } else { "ctrl-1" };
        workspace.on_keystroke(&Keystroke::parse(key).unwrap(), window, cx);
        assert_eq!(workspace.code_form.agent, SessionAgent::Codex);
        assert_eq!(workspace.code_form.model.as_deref(), Some("gpt-5"));
        assert_eq!(workspace.overlay.launch_menu(), None);
    });
}

#[gpui_kit::test]
fn saved_server_model_selection_controls_send_and_catalog_picks_clear_it(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.auth = Auth::Local;
        workspace.view = View::NewCode;
        workspace.code_form.local_subscription = true;
        workspace.composer_empty = false;
        workspace.code_agents = vec![agent(SessionAgent::Opencode, &["zai/glm-5.3"])];
        let id = nexus_workbench::provider::InstanceId::new("server:test".into());
        workspace.code_form.agent = SessionAgent::Opencode;
        workspace.code_form.provider = Some(id.clone());
        workspace.code_form.providers = vec![super::LocalProviderChoice {
            id,
            name: "Server".into(),
            agents: vec![SessionAgent::Opencode],
            server_models: Some(vec![]),
        }];
        assert!(!workspace.composer_send_enabled(), "server with no models");
        workspace.code_form.providers[0].server_models = Some(vec!["team/model".into(), "team/other".into()]);
        assert!(!workspace.composer_send_enabled(), "server model has not been picked");
        workspace.code_form.pick_server_model("team/model");
        assert!(workspace.composer_send_enabled(), "a saved model is picked");

        workspace.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
        workspace.pick_model_row(
            LaunchRow {
                agent: SessionAgent::Opencode,
                model: Some("zai/glm-5.3".into()),
            },
            window,
            cx,
        );
        // Servers serve only OpenCode, which runs local sessions only where `nexus_agents` lists it (Linux today).
        // Elsewhere the picker does not offer its rows, so the pick changes nothing.
        if SessionAgent::Opencode.supported_on_current_platform() {
            assert!(workspace.code_form.provider.is_none());
            assert_eq!(workspace.code_form.model.as_deref(), Some("zai/glm-5.3"));
            assert!(!workspace.composer_send_enabled(), "no saved provider for this agent");
        } else {
            assert!(
                workspace.code_form.provider.is_some(),
                "a row this system cannot run is not picked"
            );
            assert!(workspace.composer_send_enabled(), "the saved model stays picked");
        }
    });
}

#[gpui_kit::test]
fn permissions_on_a_highlighted_row_picks_that_row_for_a_new_session(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.view = View::NewCode;
        workspace.code_form.agent = SessionAgent::Claude;
        workspace.code_form.model = Some("sonnet".into());
        workspace.code_form.remember();
        workspace.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
        workspace.prepare_model_search(window, cx);
        let codex = LaunchRow {
            agent: SessionAgent::Codex,
            model: Some("gpt-5".into()),
        };
        workspace.pick_panel_mode(codex, PermissionMode::BypassPermissions, cx);
        assert_eq!(workspace.code_form.agent, SessionAgent::Codex);
        assert_eq!(workspace.code_form.model.as_deref(), Some("gpt-5"));
        assert_eq!(workspace.code_form.mode, PermissionMode::BypassPermissions);
        assert_eq!(
            workspace.overlay.launch_menu(),
            Some(LaunchMenu::Model),
            "the panel stays open"
        );
    });
}

#[gpui_kit::test]
fn permissions_keep_an_open_sessions_model_and_reasoning_moves_it(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.code_agents = vec![
            agent(SessionAgent::Claude, &["opus", "sonnet"]),
            agent(SessionAgent::Codex, &["gpt-5", "gpt-6"]),
        ];
        let thread = serde_json::from_value(serde_json::json!({
            "id": "panel-session", "projectId": "local-project", "title": "Fixture", "agent": "claude",
            "model": "sonnet", "permissionMode": "default", "effort": "high", "via": "nexus",
            "status": "idle", "costUsd": 0.0, "nexusSessions": [], "contextTokens": null,
            "contextWindow": null, "createdAt": 0, "updatedAt": 1, "archived": false,
        }))
        .expect("fixture thread");
        workspace.archived_code = vec![crate::code::CodeSession::of(thread)];
        let mut view = crate::workspace::code_view::CodeView::new("panel-session".into(), window, cx);
        view.dock.menu = Some(crate::workspace::code_dock::Menu::Model);
        workspace.view = View::Code(view);
        workspace.prepare_model_search(window, cx);
        // The session's own model: the mode goes alone.
        let sonnet = LaunchRow {
            agent: SessionAgent::Claude,
            model: Some("sonnet".into()),
        };
        assert_eq!(workspace.panel_session_model(&sonnet), (None, None));
        // Another highlighted model: the session moves to it with the mode, and its effort clears.
        let opus = LaunchRow {
            agent: SessionAgent::Claude,
            model: Some("opus".into()),
        };
        assert_eq!(
            workspace.panel_session_model(&opus),
            (Some(Some("opus".into())), Some(None))
        );
        // Permissions on it: the mode is the session's, so it goes alone and the model stays.
        assert_eq!(
            workspace.panel_session_update(&opus, PanelPick::Mode(PermissionMode::Plan)),
            (None, None, Some(PermissionMode::Plan))
        );
        // A Reasoning level is that row's model's: the session moves to it with that level.
        assert_eq!(
            workspace.panel_session_update(&opus, PanelPick::Effort(Some(Effort::Low))),
            (Some(Some("opus".into())), Some(Some(Effort::Low)), None)
        );
        assert_eq!(
            workspace.panel_session_update(&sonnet, PanelPick::Effort(Some(Effort::Low))),
            (None, Some(Some(Effort::Low)), None)
        );
    });
}

/// The Reasoning on-state of an unpicked row is the effort picking it would keep: its agent's saved one.
#[gpui_kit::test]
fn reasoning_for_an_unpicked_row_shows_what_picking_it_keeps(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.view = View::NewCode;
        workspace.code_form.agent = SessionAgent::Codex;
        workspace.code_form.model = Some("gpt-5".into());
        workspace.code_form.effort = Some(Effort::Low);
        workspace.code_form.remember_model();
        workspace.code_form.take_agent(SessionAgent::Claude);
        workspace.code_form.model = Some("sonnet".into());
        workspace.code_form.effort = Some(Effort::High);
        workspace.code_form.remember_model();
        workspace.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
        workspace.prepare_model_search(window, cx);
        let row = |agent, model: &str| LaunchRow {
            agent,
            model: Some(model.into()),
        };
        assert_eq!(
            workspace.launch_row_effort(&row(SessionAgent::Claude, "sonnet")),
            Some(Effort::High)
        );
        assert_eq!(workspace.launch_row_effort(&row(SessionAgent::Claude, "opus")), None);
        assert_eq!(
            workspace.launch_row_effort(&row(SessionAgent::Codex, "gpt-5")),
            Some(Effort::Low)
        );
        assert_eq!(workspace.launch_row_effort(&row(SessionAgent::Codex, "gpt-6")), None);
        // Picking the Codex row keeps exactly what it showed.
        assert!(workspace.select_launch_row(row(SessionAgent::Codex, "gpt-5")));
        assert_eq!(workspace.code_form.effort, Some(Effort::Low));
    });
}

/// An organization switch reads who pays again: the old organization's payer and pay notice never show in the new one.
/// `Event::Account` for another organization calls `org_pays_changed`; the test calls it alone, because the
/// whole account path starts presence and cloud reads on other threads, which the test scheduler refuses.
#[gpui_kit::test]
fn an_organization_switch_forgets_the_old_organizations_payer_and_spend(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.code_form.access = Some(crate::code_run::AgentAccess {
            subscriptions: true,
            answer: Some(serde_json::json!({ "org": "org-a" })),
        });
        workspace.spend = Some(serde_json::json!({ "org": "org-a" }));
        workspace.org_pays_changed();
        // Organization B's spend read fails: nothing of A's comes back.
        workspace.handle(crate::gateway::Event::Spend(None), window, cx);
        assert!(
            workspace.code_form.access.is_none(),
            "A's payer access must not survive the switch"
        );
        assert!(workspace.spend.is_none(), "A's spend must not survive the switch");
    });
}

/// The organization menu's switch forgets who pays before it clears the account, so B never shows A's payer or spend,
/// even when B's spend read fails.
#[gpui_kit::test]
fn the_org_menu_switch_forgets_the_old_organizations_payer_and_spend(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.code_form.access = Some(crate::code_run::AgentAccess {
            subscriptions: true,
            answer: Some(serde_json::json!({ "org": "org-a" })),
        });
        workspace.spend = Some(serde_json::json!({ "org": "org-a" }));
        let b = crate::workspace::org_menu::Org {
            id: "org-b".into(),
            slug: Some("b".into()),
            name: "B".into(),
            personal: false,
            current: false,
            logo: None,
            role: None,
            provider_org_id: None,
        };
        workspace.switch_org(&b, window, cx);
        workspace.handle(crate::gateway::Event::Spend(None), window, cx);
        assert!(
            workspace.code_form.access.is_none(),
            "A's payer access must not survive the switch"
        );
        assert!(workspace.spend.is_none(), "A's spend must not survive the switch");
    });
}

/// A sign-out forgets who pays and what was spent with the rest of the session.
#[gpui_kit::test]
fn a_sign_out_forgets_the_payer_and_spend(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.code_form.access = Some(crate::code_run::AgentAccess {
            subscriptions: true,
            answer: None,
        });
        workspace.spend = Some(serde_json::json!({ "org": "org-a" }));
        workspace.clear_session(cx);
        assert!(workspace.code_form.access.is_none());
        assert!(workspace.spend.is_none());
    });
}

/// An organization switch while a chat answers stops that answer as Stop does: the chat stays open, so an answer left
/// streaming would show Thinking and animate forever. `Event::Account` for another organization calls
/// `cancel_live_chats`; the test calls it alone, for the same reason as the payer test above.
#[gpui_kit::test]
fn an_organization_switch_stops_the_open_chats_running_answer(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.turn += 1;
        workspace.asking = Some(workspace.turn);
        workspace.entries = vec![
            crate::workspace::Entry::user(1, "question".into()),
            crate::workspace::Entry::answer(2, "half an", true, cx),
        ];
        workspace.cancel_live_chats();
        let answer = workspace.entries.last().expect("the answer stays on screen");
        assert!(workspace.asking.is_none());
        assert!(!answer.streaming, "the answer must not keep showing Thinking");
        assert!(answer.stopped, "the answer shows as stopped, as after Stop");
        assert_eq!(answer.streamed, "half an", "what streamed stays");
    });
}

/// The composer's "+" opens its menu, and a second press on it closes the menu. The menu's backdrop takes that press,
/// so the button cannot reopen the menu on the same press.
// GPUI only records selector bounds when its dependency has debug assertions.
#[cfg(debug_assertions)]
#[gpui_kit::test]
fn a_second_click_on_attach_closes_its_menu(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    let click_attach = |cx: &mut VisualTestContext| {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let attach = cx
            .debug_bounds("composer-attach")
            .expect("the attach button is visible");
        cx.simulate_click(attach.center(), gpui_kit::Modifiers::none());
    };
    click_attach(cx);
    workspace.update_in(cx, |workspace, _, _| {
        assert!(workspace.overlay.attach_menu(), "the first click opens")
    });
    click_attach(cx);
    workspace.update_in(cx, |workspace, _, _| {
        assert!(!workspace.overlay.attach_menu(), "the second click closes")
    });
}

#[cfg(debug_assertions)]
#[gpui_kit::test]
fn cross_agent_confirmation_cancel_and_confirm_use_the_same_session_and_original_source(cx: &mut TestAppContext) {
    use std::sync::Mutex;
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    cx.run_until_parked();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let replies = Arc::new(Mutex::new(Vec::new()));
    let transport = {
        let calls = calls.clone();
        let replies = replies.clone();
        crate::workspace::runs::Runs::deferred_source(
            crate::workspace::runs::Source::Local {
                data: "fixture-store".into(),
                scope: "fixture-org".into(),
            },
            Arc::new(move |name, args| {
                let (send, receive) = futures::channel::oneshot::channel();
                if name == "wb_thread_switch_agent" {
                    calls.lock().expect("fixture lock").push((name, args));
                    replies.lock().expect("fixture lock").push(send);
                } else {
                    let _ = send.send(Err("No transcript in this fixture".into()));
                }
                receive
            }),
        )
    };
    let prepare = |workspace: &mut Workspace, window: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<Workspace>| {
        workspace.pick_model_row(
            LaunchRow {
                agent: SessionAgent::Codex,
                model: Some("gpt-5".into()),
            },
            window,
            cx,
        );
        assert!(matches!(&workspace.view, View::Code(view) if view.agent_switch.is_some()));
    };
    workspace.update_in(cx, |workspace, window, cx| {
        workspace
            .run_fixtures
            .insert("switch-ui-session".into(), transport.clone());
        workspace.code_error = None;
        workspace.code_overview_loaded = true;
        workspace.code_agents = vec![
            agent(SessionAgent::Claude, &["sonnet"]),
            agent(SessionAgent::Codex, &["gpt-5"]),
        ];
        let summary = serde_json::from_value(serde_json::json!({
            "id":"switch-ui-session", "projectId":"local-project", "title":"Keep this thread", "agent":"claude",
            "model":"sonnet", "permissionMode":"default", "effort":"high", "via":"nexus",
            "status":"idle", "costUsd":1.0, "nexusSessions":[], "contextTokens":12000,
            "contextWindow":200000, "createdAt":0, "updatedAt":1, "archived":false
        }))
        .expect("summary");
        workspace.archived_code = vec![crate::code::CodeSession::of(summary)];
        let mut view = crate::workspace::code_view::CodeView::new("switch-ui-session".into(), window, cx);
        view.dock.menu = Some(crate::workspace::code_dock::Menu::Model);
        workspace.view = View::Code(view);
        prepare(workspace, window, cx);
        assert_eq!(
            workspace.code_session().expect("current session").agent,
            SessionAgent::Claude
        );
        assert!(
            calls.lock().expect("fixture lock").is_empty(),
            "choosing a row must not switch before confirmation"
        );
    });
    let click = |cx: &mut VisualTestContext, id: &'static str| {
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _, _| {
            assert!(
                matches!(&workspace.view, View::Code(view) if view.agent_switch.is_some()),
                "confirmation must stay open"
            );
            assert!(
                workspace.code_error.is_none(),
                "unexpected store error: {:?}",
                workspace.code_error
            );
            assert!(workspace.code_session().is_some(), "session must remain visible");
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let bounds = cx.debug_bounds(id).unwrap_or_else(|| panic!("missing {id}"));
        cx.simulate_click(bounds.center(), gpui_kit::Modifiers::none());
    };
    click(cx, "agent-switch-cancel");
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(matches!(&workspace.view, View::Code(view) if view.agent_switch.is_none()));
        assert!(calls.lock().expect("fixture lock").is_empty());
        let View::Code(view) = &mut workspace.view else {
            panic!("session")
        };
        view.dock.menu = Some(crate::workspace::code_dock::Menu::Model);
        // The catalog refresh is isolated from the user's installed agents.
        workspace.code_agents = vec![
            agent(SessionAgent::Claude, &["sonnet"]),
            agent(SessionAgent::Codex, &["gpt-5"]),
        ];
        prepare(workspace, window, cx);
    });
    click(cx, "agent-switch-confirm");
    workspace.update_in(cx, |workspace, _, cx| {
        let recorded = calls.lock().expect("fixture lock");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "wb_thread_switch_agent");
        assert_eq!(recorded[0].1["threadId"], "switch-ui-session");
        assert_eq!(recorded[0].1["expectedAgent"], "claude");
        assert_eq!(recorded[0].1["agent"], "codex");
        assert_eq!(recorded[0].1["model"], "gpt-5");
        drop(recorded);
        workspace.confirm_agent_switch(cx);
        assert_eq!(
            calls.lock().expect("fixture lock").len(),
            1,
            "a second press cannot launch another switch"
        );
    });
    replies
        .lock()
        .expect("fixture lock")
        .pop()
        .expect("switch reply")
        .send(Err("Destination unavailable".into()))
        .expect("reply delivered");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert!(matches!(&workspace.view, View::Code(view) if view.agent_switch.as_ref().is_some_and(|s| !s.busy)));
        assert_eq!(
            workspace.code_session().expect("current session").agent,
            SessionAgent::Claude
        );
    });
}

#[gpui_kit::test]
fn local_code_keeps_subscription_routing_and_only_offers_code_projects(cx: &mut TestAppContext) {
    use nexus_workbench::event::Via;

    let _agents = crate::code_run::fixture_agents(vec![]);
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.local_mode.opt_in = crate::local_providers_opt_in::enabled_for_test();
        workspace.auth = Auth::Local;
        workspace.view = View::NewCode;
        workspace.code_form.local_subscription = true;

        assert!(workspace.project_picks().is_empty());
        assert_eq!(workspace.code_form.effective_via(), Via::Subscription);
        workspace.open_settings(crate::workspace::settings_view::SettingsSection::Appearance, window, cx);
        assert!(matches!(workspace.view, View::Settings(_)));
        workspace.start_new_session(window, cx);
        assert!(matches!(workspace.view, View::NewCode));
    });
}
