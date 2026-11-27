//! Exercise the real awaited launch boundary without starting an agent or PTY.
use crate::event::{ThreadSummary, Usage};
use crate::store::{Store, ThreadRecord};
use crate::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd, Workbench};
use futures::future::BoxFuture;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

struct HeldHost(Mutex<Option<tokio::sync::oneshot::Receiver<bool>>>);
impl Host for HeldHost {
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, crate::Result<Launch>> {
        Box::pin(async { panic!("guard fixture never launches an agent") })
    }
    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
    fn price(&self, _: &str, _: &Usage) -> Option<f64> {
        None
    }
    fn emit(&self, _: Emit) {}
    fn marks_commits(&self) -> BoxFuture<'static, bool> {
        let reply = self
            .0
            .lock()
            .expect("held lookup lock")
            .take()
            .expect("one held lookup");
        Box::pin(async move { reply.await.unwrap_or(false) })
    }
}

#[tokio::test]
async fn revoked_during_commit_lookup_cannot_launch_and_a_fresh_owner_can_stage() {
    let store = Store::in_memory().expect("fixture store");
    let project = store.add_project(&std::env::temp_dir(), 100).expect("fixture project");
    let summary: ThreadSummary = serde_json::from_value(json!({
        "id":"commands-guard", "projectId":project.id, "title":"Guard fixture", "agent":"claude",
        "model":null, "permissionMode":"default", "via":"nexus", "status":"idle", "costUsd":0.,
        "nexusSessions":[], "createdAt":100, "updatedAt":100, "archived":false
    }))
    .expect("fixture thread summary");
    store
        .insert_thread(&ThreadRecord {
            summary,
            native_id: "native".into(),
            native_started: false,
            scope: "scope".into(),
            fork_from: None,
            seed: None,
            automatic_title: false,
            delivered_seq: 0,
            halt: None,
        })
        .expect("fixture thread");
    let (release, wait) = tokio::sync::oneshot::channel();
    let wb = Workbench::new(
        store,
        Arc::new(HeldHost(Mutex::new(Some(wait)))),
        Config {
            terminal: true,
            scope: "scope".into(),
            ..Config::default()
        },
    )
    .expect("fixture workbench");
    let staged = wb
        .terminal(
            "commands-guard",
            "request_user",
            json!({"command":"echo guarded", "request_id":"one"}),
            true,
        )
        .await
        .expect("stage original command");
    let live = AtomicBool::new(true);
    let valid = || live.load(Ordering::SeqCst);
    let mut execute = std::pin::pin!(wb.terminal_guarded(
        "commands-guard",
        "execute",
        json!({"id":staged["id"], "command":staged["command"]}),
        true,
        &valid
    ));
    assert!(futures::poll!(execute.as_mut()).is_pending());
    live.store(false, Ordering::SeqCst);
    release.send(false).expect("release held commit lookup");
    assert_eq!(
        execute.await.expect_err("revoked owner cannot launch"),
        "terminal_not_found"
    );
    let entry = wb
        .inner
        .terminals
        .find(
            "scope",
            "commands-guard",
            staged["id"].as_str().expect("staged command id"),
        )
        .expect("original entry survives refused launch");
    assert_eq!(entry.info().status, "awaitingUser");
    assert!(entry.info().started_at.is_none());
    let fresh = wb
        .terminal_guarded(
            "commands-guard",
            "request_user",
            json!({"command":"echo fresh", "request_id":"two"}),
            true,
            &|| true,
        )
        .await
        .expect("fresh owner can stage");
    assert_eq!(fresh["status"], "awaitingUser");
    assert_ne!(fresh["id"], staged["id"]);
}
