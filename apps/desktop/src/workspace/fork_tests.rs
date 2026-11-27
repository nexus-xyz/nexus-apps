//! Headless: a Code session's Fork, through a deferred fixture transport. No runner, no git, no network.
use core::prelude::v1::test;
use std::sync::{Arc, Mutex};

use futures::channel::oneshot;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::workspace::code_view::CodeView;
use crate::workspace::machine_source::MachineSource;
use crate::workspace::runs::{Runs, Source};
use crate::workspace::{Auth, View, Workspace};

const ID: &str = "fork-source";

type Reply = oneshot::Sender<Result<Value, String>>;

/// The fixture's answers: the recorded turns and the log at once, the fork when the test sends it.
#[derive(Clone, Default)]
struct Rpc {
    turns: Value,
    events: Value,
    fork: Arc<Mutex<Vec<(Value, Reply)>>>,
}

impl Rpc {
    fn runs(&self) -> Runs {
        let rpc = self.clone();
        Runs::deferred_source(
            Source::Local {
                data: "fixture-store".into(),
                scope: "fixture-org".into(),
            },
            Arc::new(move |name, args| {
                let (send, receive) = oneshot::channel();
                match name {
                    "wb_changed_turns" => drop(send.send(Ok(rpc.turns.clone()))),
                    "wb_thread_events" => drop(send.send(Ok(rpc.events.clone()))),
                    "wb_thread_fork" => rpc.fork.lock().unwrap().push((args, send)),
                    _ => drop(send.send(Err("Not in this fixture".into()))),
                }
                receive
            }),
        )
    }

    fn answer_fork(&self, answer: Result<Value, String>) -> Value {
        let (args, reply) = self.fork.lock().unwrap().pop().expect("a fork was asked for");
        reply.send(answer).unwrap();
        args
    }
}

fn event(seq: i64, event: Value) -> Value {
    json!({ "threadId": ID, "seq": seq, "at": 0, "event": event })
}

fn log() -> Value {
    json!([
        event(1, json!({ "kind": "user", "text": "Fix the login" })),
        event(
            4,
            json!({ "kind": "turnEnd", "turnId": "a", "status": "completed", "costUsd": null, "durationMs": 1, "error": null })
        ),
        event(5, json!({ "kind": "user", "text": "Now add a test" })),
        event(
            9,
            json!({ "kind": "turnEnd", "turnId": "b", "status": "completed", "costUsd": null, "durationMs": 1, "error": null })
        ),
    ])
}

fn fixture<'a>(cx: &'a mut TestAppContext, rpc: &Rpc) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
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
        workspace.code = None;
        workspace.runner = None;
        workspace.auth = Auth::SignedIn;
        let mut view = CodeView::new(ID.into(), window, cx);
        view.loaded = true;
        workspace.view = View::Code(view);
        workspace
    });
    cx.run_until_parked();
    let runs = rpc.runs();
    workspace.update_in(cx, |workspace, _, _| {
        let mut session = crate::workspace::session_keys::tests::session(ID);
        session.project_id = "proj".into();
        session.has_worktree = true;
        workspace.projects = vec![crate::code::CodeProject {
            id: "proj".into(),
            name: "proj".into(),
            branch: Some("main".into()),
            pinned: false,
            remotes: Vec::new(),
            sessions: vec![session],
            machine: None,
            added_at: 0,
        }];
        workspace.run_fixtures.insert(ID.into(), runs);
    });
    (workspace, cx)
}

fn shown(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.debug_bounds(selector).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is drawn"));
    cx.simulate_click(bounds.center(), gpui_kit::Modifiers::none());
}

/// Fork in the title menu: "Fork from which point?", with each recorded turn, the latest first.
fn open_fork(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.toggle_session_menu(ID, true, window, cx)
    });
    click(cx, "session-fork-fork-source");
}

fn open_id(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, _| match &workspace.view {
        View::Code(view) => Some(view.id.clone()),
        _ => None,
    })
}

fn notice(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, _| {
        workspace.notices.current().map(|notice| notice.text.clone())
    })
}

#[cfg(debug_assertions)]
#[gpui_kit::test]
fn fork_offers_each_recorded_turn(cx: &mut TestAppContext) {
    let rpc = Rpc {
        turns: json!([0, 4, 9]),
        events: log(),
        ..Rpc::default()
    };
    let (workspace, cx) = fixture(cx, &rpc);
    open_fork(&workspace, cx);
    assert!(shown(cx, "session-fork-turn-9-fork-source"), "the latest turn");
    assert!(shown(cx, "session-fork-turn-4-fork-source"), "the turn before it");
    assert!(
        !shown(cx, "session-fork-turn-0-fork-source"),
        "the folder before the first turn is no point"
    );
    assert!(!shown(cx, "fork-nothing-yet"));

    click(cx, "session-fork-turn-4-fork-source");
    cx.run_until_parked();
    let args = rpc.answer_fork(Err("not yet".into()));
    assert_eq!(args["turn"], json!(4), "the picked turn is forked");
}

#[cfg(debug_assertions)]
#[gpui_kit::test]
fn fork_says_nothing_to_fork_yet_without_turns(cx: &mut TestAppContext) {
    let rpc = Rpc {
        turns: json!([0]),
        events: json!([]),
        ..Rpc::default()
    };
    let (workspace, cx) = fixture(cx, &rpc);
    open_fork(&workspace, cx);
    assert!(shown(cx, "fork-nothing-yet"));
    assert!(!shown(cx, "session-fork-turn-0-fork-source"));
}

#[gpui_kit::test]
fn a_fork_shows_its_progress_then_its_failure_in_the_notice(cx: &mut TestAppContext) {
    let rpc = Rpc::default();
    let (workspace, cx) = fixture(cx, &rpc);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.fork_session(ID.into(), Some(9), None, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        notice(&workspace, cx).as_deref(),
        Some("Forking…"),
        "progress shows while the worktree is made"
    );

    rpc.answer_fork(Err("This session's folder is gone.".into()));
    cx.run_until_parked();
    assert_eq!(
        notice(&workspace, cx).as_deref(),
        Some("Couldn't fork it: This session's folder is gone."),
        "the failure and its reason show where the person looks"
    );
    assert_eq!(open_id(&workspace, cx).as_deref(), Some(ID), "the source stays open");
}

#[gpui_kit::test]
fn a_fork_opens_when_it_is_ready_and_a_second_click_makes_no_second_fork(cx: &mut TestAppContext) {
    let rpc = Rpc::default();
    let (workspace, cx) = fixture(cx, &rpc);
    for _ in 0..2 {
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.fork_session(ID.into(), None, None, window, cx)
        });
    }
    cx.run_until_parked();
    assert_eq!(rpc.fork.lock().unwrap().len(), 1, "one fork at a time");
    rpc.answer_fork(Ok(json!({
        "id": "the-fork", "projectId": "proj", "title": "Fork of fork-source", "agent": "claude", "model": null,
        "permissionMode": "default", "via": "subscription", "status": "idle", "costUsd": 0.0, "nexusSessions": [],
        "createdAt": 1, "updatedAt": 1, "archived": false
    })));
    cx.run_until_parked();
    assert_eq!(open_id(&workspace, cx), Some("the-fork".into()));
    assert_eq!(
        notice(&workspace, cx),
        None,
        "the progress notice goes when the fork opens"
    );
}
