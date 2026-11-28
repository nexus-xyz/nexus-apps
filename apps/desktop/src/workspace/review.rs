//! Native review fixtures. Only data is synthetic: the window and picker use production rendering and handlers.

use super::{
    Auth, Place, View, Workspace,
    code_view::CodeView,
    machine_source::MachineSource,
    new_code::{CodeForm, LaunchMenu},
    overlay::Overlay,
};
use crate::{
    code::{Block, CodeProject, CodeSession},
    palette::ThemeChoice,
};
use futures::{StreamExt, channel::mpsc::unbounded};
use gpui_kit::*;
use nexus_workbench::{
    api::AgentInfo,
    event::{Effort, PermissionMode, SessionAgent},
};
use serde_json::{Value, json};
use std::sync::Arc;

const THREAD: &str = "picker-review-session";
const PROJECT: &str = "picker-review-project";
#[derive(Clone, Copy)]
pub(crate) enum Scene {
    Connections,
    Standard,
    Fast,
}

pub(crate) fn agents() -> Vec<AgentInfo> {
    [
        (
            SessionAgent::Codex,
            vec![
                ("default", "GPT-6.2-Sol", Some("gpt-6.2-sol")),
                ("gpt-6.2-sol", "GPT-6.2-Sol", None),
                ("gpt-6.2-astra", "GPT-6.2-Astra", None),
            ],
        ),
        (
            SessionAgent::Claude,
            vec![
                ("default", "Opus 5.5", Some("claude-opus-5-5")),
                ("claude-opus-5-5", "Opus 5.5", None),
                ("sonnet", "Sonnet", None),
                ("haiku", "Haiku", None),
            ],
        ),
        (SessionAgent::Antigravity, vec![]),
        (SessionAgent::Cursor, vec![]),
    ]
    .into_iter()
    .map(|(agent, models)| AgentInfo {
        agent,
        path: (!models.is_empty()).then(|| format!("/native-review/{}", agent.as_str())),
        version: None,
        problem: None,
        models: Some(
            models
                .into_iter()
                .map(|(id, name, resolved)| nexus_workbench::agent_models::Model {
                    id: id.into(),
                    name: name.into(),
                    resolved_id: resolved.map(str::to_string),
                    efforts: vec![Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh],
                })
                .collect(),
        ),
    })
    .collect()
}

impl Workspace {
    pub(crate) fn for_review(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let gateway =
            crate::gateway::Gateway::for_view_test_at(crate::code_run::runtime().clone(), "http://127.0.0.1:9");
        let (_, events) = unbounded();
        let (_, arrivals) = unbounded();
        let mut workspace = Self::with_machine_source(
            Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = Task::ready(());
        workspace._working_tick = Task::ready(());
        workspace._inbox_polls = [Task::ready(()), Task::ready(())];
        workspace.auth = Auth::SignedIn;
        workspace.sessions_loaded = true;
        workspace.agent = true;
        workspace.account = crate::gateway::Account::from_json(&json!({
            "tenant":{"id":"native-review","slug":"native-review","name":"Nexus Labs","kind":"company"},
            "user":{"id":"reviewer","name":"You","email":"review@example.test"}, "role":"member"
        }));
        workspace.code_agents = agents();
        workspace.code_form = CodeForm::default();
        workspace.code_form.agent = SessionAgent::Codex;
        workspace.code_form.project = Some(PROJECT.into());
        workspace.code_form.effort = Some(Effort::Xhigh);
        workspace.code_form.mode = PermissionMode::BypassPermissions;
        workspace.code_form.access = Some(crate::code_run::AgentAccess {
            subscriptions: true,
            answer: Some(json!({})),
        });
        let now = chrono::Utc::now().timestamp_millis();
        let thread = serde_json::from_value(json!({
            "id":THREAD, "projectId":PROJECT, "title":"Model picker review", "agent":"claude",
            "model":"claude-opus-5-5", "permissionMode":"bypassPermissions", "effort":"high", "via":"nexus",
            "fast":false, "status":"idle", "costUsd":0.0, "nexusSessions":[], "contextTokens":null,
            "contextWindow":null, "createdAt":now, "updatedAt":now, "archived":false
        }))
        .expect("native review thread");
        workspace.projects = vec![CodeProject {
            id: PROJECT.into(),
            name: "nexus".into(),
            branch: Some("main".into()),
            pinned: true,
            remotes: vec![],
            sessions: vec![CodeSession::of(thread)],
            machine: None,
        }];
        workspace.code_overview_loaded = true;
        let (send, mut receive) = unbounded::<Value>();
        let transport = super::runs::Runs::deferred_source(
            super::runs::Source::Local {
                data: crate::review::root().expect("review root").into(),
                scope: "native-review".into(),
            },
            Arc::new(move |name, args| {
                let (answer, read) = futures::channel::oneshot::channel();
                let result = if name == "wb_thread_update" {
                    send.unbounded_send(args)
                        .map(|_| Value::Null)
                        .map_err(|error| error.to_string())
                } else {
                    Err("This review uses sample data and starts no agents.".into())
                };
                if answer.send(result).is_err() {
                    eprintln!("native review caller closed");
                }
                read
            }),
        );
        workspace.run_fixtures.insert(THREAD.into(), transport);
        cx.spawn(async move |this, cx| {
            while let Some(args) = receive.next().await {
                if this
                    .update(cx, |workspace, cx| {
                        if let Some(session) = workspace
                            .projects
                            .first_mut()
                            .and_then(|project| project.sessions.first_mut())
                        {
                            if let Some(fast) = args["fast"].as_bool() {
                                session.fast = fast;
                            }
                            if let Some(model) = args["model"].as_str() {
                                session.model = Some(model.into());
                            }
                            if let Some(effort) = args["effort"].as_str() {
                                session.effort = serde_json::from_value(json!(effort)).ok();
                            }
                            if let Some(mode) = args["permissionMode"]
                                .as_str()
                                .and_then(|mode| serde_json::from_value(json!(mode)).ok())
                            {
                                session.mode = mode;
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        workspace.review_scene(Scene::Connections, window, cx);
        workspace
    }

    pub(crate) fn review_fast_on(&self) -> bool {
        self.code_session().is_some_and(|session| session.fast)
    }

    pub(crate) fn review_fast(&mut self, fast: bool, cx: &mut Context<Self>) {
        self.set_session_fast(fast, cx);
    }

    pub(crate) fn review_theme(&mut self, theme: ThemeChoice, window: &mut Window, cx: &mut Context<Self>) {
        self.set_theme(theme, cx);
        super::apply_kit_theme(crate::palette::Palette::is_dark(theme, window.appearance()), window, cx);
        cx.notify();
    }

    pub(crate) fn review_scene(&mut self, scene: Scene, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Default::default();
        match scene {
            Scene::Connections => {
                self.view = View::NewCode;
                self.title = "New session".into();
                self.history = vec![Place::NewCode];
                self.history_at = 0;
                self.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
            }
            Scene::Standard | Scene::Fast => {
                let mut view = CodeView::new(THREAD.into(), window, cx);
                let blocks = vec![
                    Block::User {
                        seq: 1,
                        at: chrono::Utc::now().timestamp_millis(),
                        text: "Make the model picker clearer and add the Cursor logo.".into(),
                        from: None,
                        rewound: false,
                        images: 0,
                        image_data: vec![],
                    },
                    Block::Text("I’ll simplify the model labels and make agent setup one click away.".into()),
                ];
                let mut transcript = crate::code::Transcript::of(Vec::new());
                transcript.starts = vec![1, 2];
                transcript.last = 2;
                transcript.start = Some(1);
                transcript.blocks = blocks;
                view.set(transcript, cx);
                view.dock.menu = Some(super::code_dock::Menu::Model);
                self.view = View::Code(view);
                self.title = "Model picker review".into();
                self.history = vec![Place::Code(THREAD.into())];
                self.history_at = 0;
                self.set_session_fast(matches!(scene, Scene::Fast), cx);
            }
        }
        self.prepare_model_search(window, cx);
        if matches!(scene, Scene::Connections) {
            self.model_cursor_on_open = false;
            self.model_scroll.scroll_to_bottom();
        }
        cx.notify();
    }
}
