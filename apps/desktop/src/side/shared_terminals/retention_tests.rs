//! Deferred per-instance terminal backend; the GPUI scheduler never owns a real PTY.
use super::{Guard, Output, SharedTerminals, batch};
use crate::code_run::CodeRunner;
use crate::side::terminal::Terminal;
use futures::channel::oneshot;
use gpui_kit::{AppContext, Context, Entity, IntoElement, Render, TestAppContext, Window, div};
use serde_json::{Value, json};
use std::rc::Rc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("nexus-commands-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("isolated Commands fixture directory");
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Request {
    op: String,
    args: Value,
    answer: oneshot::Sender<Result<Value, String>>,
    valid: Guard,
}
#[derive(Clone, Default)]
struct Rpc(Arc<Mutex<Vec<Request>>>);
impl Rpc {
    fn install(&self, runner: &CodeRunner) {
        let requests = self.0.clone();
        runner.fixture_terminal(move |_, op, args, valid| {
            let (answer, receive) = oneshot::channel();
            requests.lock().unwrap().push(Request {
                op,
                args,
                answer,
                valid,
            });
            receive
        });
    }
    fn take(&self, op: &str) -> Request {
        let mut requests = self.0.lock().unwrap();
        let at = requests
            .iter()
            .position(|request| request.op == op)
            .unwrap_or_else(|| panic!("missing {op}"));
        requests.remove(at)
    }
    fn count(&self, op: &str) -> usize {
        self.0.lock().unwrap().iter().filter(|request| request.op == op).count()
    }
}

#[gpui_kit::test]
fn a_held_old_read_never_advances_the_transferred_cursor(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let data = TempDir::new();
    let (runner, _events) = crate::code_run::test_runner(data.path().to_path_buf(), "scope");
    let rpc = Rpc::default();
    rpc.install(&runner);
    let (host, cx) = cx.add_window_view(|window, cx| {
        HostView(cx.new(|cx| SharedTerminals::new(Rc::new(runner), "session".into(), window, cx)))
    });
    let old = host.read_with(cx, |host, _| host.0.clone());
    old.update(cx, |view, cx| {
        let send = view.output_sender("same-command".into());
        let output = cx.new(|cx| Terminal::attached(send, cx));
        output.update(cx, |view, cx| {
            view.feed(b"prefix\x1b[3", true, false, cx);
        });
        view.outputs.insert(
            "same-command".into(),
            Output {
                view: output,
                cursor: 9,
            },
        );
    });
    cx.run_until_parked();
    rpc.take("list").answer.send(Ok(json!([entry("running")]))).unwrap();
    cx.run_until_parked();
    let held = rpc.take("read");
    assert_eq!(held.args["after"], 9);
    let saved = old.update(cx, |view, cx| view.retain(cx));
    assert!(!(held.valid)());
    let fresh = cx.update(|window, cx| cx.new(|cx| SharedTerminals::restore(saved, Arc::new(|| true), window, cx)));
    // A result may have already been produced, but its old publication was canceled.
    let _ = held
        .answer
        .send(Ok(json!({"bytes":b"1mred".to_vec(), "next":14, "truncated":false})));
    cx.run_until_parked();
    rpc.take("list").answer.send(Ok(json!([entry("running")]))).unwrap();
    cx.run_until_parked();
    let next = rpc.take("read");
    assert_eq!(next.args["after"], 9, "held bytes were never applied");
    next.answer
        .send(Ok(json!({"bytes":b"1mred".to_vec(), "next":14, "truncated":false})))
        .unwrap();
    cx.run_until_parked();
    fresh.read_with(cx, |view, _| assert_eq!(view.outputs["same-command"].cursor, 14));
    old.read_with(cx, |view, _| assert!(view.outputs.is_empty()));
}
struct HostView(Entity<SharedTerminals>);
impl Render for HostView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
fn entry(status: &str) -> Value {
    json!({"id":"same-command", "name":"Command", "command":"echo once", "cwd":"/fixture",
        "status":status, "owner":"user", "private":false, "exitCode":null})
}

#[gpui_kit::test]
fn staging_and_receipts_cross_retirement_once_and_new_controls_remain_live(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let data = TempDir::new();
    let (runner, _events) = crate::code_run::test_runner(data.path().to_path_buf(), "scope");
    let rpc = Rpc::default();
    rpc.install(&runner);
    let runner = Rc::new(runner);
    let (host, cx) = cx.add_window_view(|window, cx| {
        HostView(cx.new(|cx| SharedTerminals::new(runner.clone(), "session".into(), window, cx)))
    });
    let old = host.read_with(cx, |host, _| host.0.clone());
    old.update(cx, |view, cx| {
        view.query = "nondefault query".into();
        view.show_history = true;
        view.history_allowed = true;
        view.selected = Some("same-command".into());
        view.collapsed.insert("same-command".into());
        view.run("echo once".into(), None, cx);
    });
    let saved = old.update(cx, |view, cx| view.retain(cx));
    let fresh = cx.update(|window, cx| cx.new(|cx| SharedTerminals::restore(saved, Arc::new(|| true), window, cx)));
    rpc.take("request_user").answer.send(Ok(entry("awaitingUser"))).unwrap();
    cx.run_until_parked();
    let execute = rpc.take("execute");
    assert!(
        (execute.valid)(),
        "presentation retirement preserves admitted continuation"
    );
    execute.answer.send(Ok(entry("running"))).unwrap();
    cx.run_until_parked();
    fresh.update(cx, |view, cx| {
        view.drain_receipts(cx);
        assert!(!view.busy);
        assert_eq!(view.query, "nondefault query");
        assert_eq!(view.selected.as_deref(), Some("same-command"));
        assert!(view.collapsed.contains("same-command"));
        assert!(view.accepted.contains("same-command"));
        assert_eq!(view.entries.len(), 1);
        view.drain_receipts(cx);
        assert_eq!(view.entries.len(), 1, "receipt applied once");
    });
    old.update(cx, |view, cx| view.perform("stop", json!({"id":"same-command"}), cx));
    assert_eq!(rpc.count("stop"), 0);
    fresh.update(cx, |view, cx| view.perform("stop", json!({"id":"same-command"}), cx));
    assert!(
        (rpc.take("stop").valid)(),
        "fresh controls address the original command"
    );
}

#[gpui_kit::test]
fn owner_fence_reaches_already_admitted_work_and_never_revives_old_presentations(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let data = TempDir::new();
    let (runner, _events) = crate::code_run::test_runner(data.path().to_path_buf(), "scope");
    let rpc = Rpc::default();
    rpc.install(&runner);
    let (host, cx) = cx.add_window_view(|window, cx| {
        HostView(cx.new(|cx| SharedTerminals::new(Rc::new(runner), "session".into(), window, cx)))
    });
    let view = host.read_with(cx, |host, _| host.0.clone());
    view.update(cx, |view, cx| view.run("echo once".into(), None, cx));
    let request = rpc.take("request_user");
    let live = Arc::new(AtomicBool::new(true));
    let valid = live.clone();
    view.update(cx, |view, _| {
        view.set_owner_guard(Arc::new(move || valid.load(Ordering::SeqCst)))
    });
    live.store(false, Ordering::SeqCst);
    assert!(
        !(request.valid)(),
        "registry retirement also fences previously queued actions"
    );
    request.answer.send(Ok(entry("awaitingUser"))).unwrap();
    cx.run_until_parked();
    assert_eq!(rpc.count("execute"), 0);
    view.update(cx, |view, cx| {
        view.retire(cx);
        view.set_owner_guard(Arc::new(|| true));
        assert!(!view.is_current());
        view.run("must not run".into(), None, cx);
    });
    assert_eq!(rpc.count("request_user"), 0);
}

#[gpui_kit::test]
fn input_delete_and_undo_receipts_follow_the_state_and_old_callbacks_are_inert(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let data = TempDir::new();
    let (runner, _events) = crate::code_run::test_runner(data.path().to_path_buf(), "scope");
    let rpc = Rpc::default();
    rpc.install(&runner);
    let (host, cx) = cx.add_window_view(|window, cx| {
        HostView(cx.new(|cx| SharedTerminals::new(Rc::new(runner), "session".into(), window, cx)))
    });
    let old = host.read_with(cx, |host, _| host.0.clone());
    let sender = old.read_with(cx, |view, _| view.output_sender("same-command".into()));
    sender("input", json!({"data":"one"}));
    old.update(cx, |view, cx| view.perform("delete", json!({"id":"same-command"}), cx));
    let saved = old.update(cx, |view, cx| view.retain(cx));
    sender("input", json!({"data":"stale"}));
    assert_eq!(rpc.count("input"), 1);
    rpc.take("input")
        .answer
        .send(Err("input acknowledgement".into()))
        .unwrap();
    rpc.take("delete").answer.send(Ok(json!({}))).unwrap();
    cx.run_until_parked();
    let fresh = cx.update(|window, cx| cx.new(|cx| SharedTerminals::restore(saved, Arc::new(|| true), window, cx)));
    fresh.update(cx, |view, cx| {
        view.drain_receipts(cx);
        assert_eq!(view.error.as_deref(), Some("input acknowledgement"));
        assert_eq!(view.undo_ids, vec!["same-command"]);
        assert!(view.pending_input.borrow().is_empty());
        view.undo_delete(vec!["same-command".into()], cx);
    });
    let undo = rpc.take("restore");
    let saved = fresh.update(cx, |view, cx| view.retain(cx));
    undo.answer.send(Ok(json!({}))).unwrap();
    cx.run_until_parked();
    let returned = cx.update(|window, cx| cx.new(|cx| SharedTerminals::restore(saved, Arc::new(|| true), window, cx)));
    returned.update(cx, |view, cx| {
        view.drain_receipts(cx);
        assert!(view.show_history);
        assert!(view.undo_ids.is_empty());
        assert_eq!(view.pending_actions, 0);
        let sender = view.output_sender("same-command".into());
        sender("input", json!({"data":"fresh"}));
    });
    assert!((rpc.take("input").valid)());
    assert_eq!(rpc.count("restore"), 0, "restore was issued once");
}

#[gpui_kit::test]
fn bulk_decision_failures_aggregate_once_across_transfer_and_keep_success_bookkeeping(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let data = TempDir::new();
    let (runner, _events) = crate::code_run::test_runner(data.path().to_path_buf(), "scope");
    let rpc = Rpc::default();
    rpc.install(&runner);
    let (host, cx) = cx.add_window_view(|window, cx| {
        HostView(cx.new(|cx| SharedTerminals::new(Rc::new(runner), "session".into(), window, cx)))
    });
    let old = host.read_with(cx, |host, _| host.0.clone());
    old.update(cx, |view, cx| {
        view.entries = ["one", "two", "three"]
            .into_iter()
            .map(|id| {
                let mut value = entry("awaitingUser");
                value["id"] = json!(id);
                serde_json::from_value(value).expect("pending command fixture")
            })
            .collect();
        view.perform_pending_batch("execute", batch::snapshot(&view.entries), cx);
    });
    let first = rpc.take("execute");
    let second = rpc.take("execute");
    let third = rpc.take("execute");
    let first_id = first.args["id"].as_str().expect("first command ID").to_string();
    let success_id = second.args["id"].as_str().expect("successful command ID").to_string();
    let third_id = third.args["id"].as_str().expect("third command ID").to_string();
    first
        .answer
        .send(Err("first failure".into()))
        .expect("first decision receiver");
    cx.run_until_parked();
    old.update(cx, |view, cx| {
        view.drain_receipts(cx);
        assert!(view.error.is_none(), "batch reports only when all decisions settle");
        assert!(!view.accepted.contains(&first_id));
        assert!(view.busy);
    });
    let saved = old.update(cx, |view, cx| view.retain(cx));
    let fresh = cx.update(|window, cx| cx.new(|cx| SharedTerminals::restore(saved, Arc::new(|| true), window, cx)));
    let mut successful = entry("running");
    successful["id"] = json!(success_id);
    second
        .answer
        .send(Ok(successful))
        .expect("successful decision receiver");
    cx.run_until_parked();
    fresh.update(cx, |view, cx| {
        view.drain_receipts(cx);
        assert!(view.error.is_none());
        assert!(view.accepted.contains(&success_id));
        assert!(
            view.entries
                .iter()
                .any(|entry| entry.id == success_id && entry.status == "running")
        );
    });
    third
        .answer
        .send(Err("second failure".into()))
        .expect("final decision receiver");
    cx.run_until_parked();
    fresh.update(cx, |view, cx| {
        view.drain_receipts(cx);
        assert_eq!(
            view.error.as_deref(),
            Some("2 decision(s) could not be completed: first failure")
        );
        assert!(!view.accepted.contains(&first_id));
        assert!(!view.accepted.contains(&third_id));
        assert!(view.accepted.contains(&success_id));
        assert!(!view.busy);
        assert_eq!(view.pending_actions, 0);
        view.drain_receipts(cx);
        assert_eq!(
            view.error.as_deref(),
            Some("2 decision(s) could not be completed: first failure")
        );
    });
}
