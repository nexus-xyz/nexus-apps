//! Session switching through the production JSON API and real scripted agent subprocesses.
mod support;

use futures::future::BoxFuture;
use nexus_workbench::agent_switch::AgentSwitch;
use nexus_workbench::api::{AgentInfo, Api, Machine};
use nexus_workbench::event::{Decision, SessionAgent, StoredEvent, ThreadStatus, Via, WbEvent};
use nexus_workbench::store::Store;
use nexus_workbench::{Config, NewWorktree, Workbench};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use support::*;

struct SyntheticMachine {
    home: PathBuf,
    installed: bool,
    plans: bool,
}
impl Machine for SyntheticMachine {
    fn tool_path(&self) -> String {
        std::env::var("PATH").unwrap_or_default()
    }
    fn home(&self) -> Result<PathBuf, String> {
        Ok(self.home.clone())
    }
    fn agent(&self, agent: SessionAgent) -> AgentInfo {
        AgentInfo {
            agent,
            path: self.installed.then(|| "/synthetic-agent".into()),
            version: None,
            models: None,
            problem: None,
        }
    }
    fn subscriptions_allowed(&self) -> BoxFuture<'static, bool> {
        let allowed = self.plans;
        Box::pin(async move { allowed })
    }
}
fn api(s: &Setup, installed: bool, plans: bool) -> Api {
    Api::new(
        s.workbench.clone(),
        Arc::new(SyntheticMachine {
            home: s.root.join("home"),
            installed,
            plans,
        }),
    )
}
fn target(from: SessionAgent, to: SessionAgent) -> AgentSwitch {
    AgentSwitch {
        expected_agent: from,
        agent: to,
        model: None,
        effort: None,
        via: Via::Nexus,
        account: None,
    }
}
async fn switch(api: &Api, id: &str, choice: &AgentSwitch) -> Result<Value, String> {
    let mut args = serde_json::to_value(choice).expect("choice");
    args["threadId"] = json!(id);
    api.call("wb_thread_switch_agent", args).await
}
fn saved(s: &Setup, id: &str) -> nexus_workbench::store::ThreadRecord {
    Store::open(&s.root.join("workbench.sqlite3"))
        .expect("store")
        .thread(id)
        .expect("read")
        .expect("thread")
}
fn boundaries(events: &[StoredEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e.event, WbEvent::AgentSwitched { .. }))
        .count()
}

#[tokio::test]
async fn switching_both_directions_keeps_one_thread_worktree_and_history_and_reaches_the_new_process() {
    let s = setup("switch-roundtrip", bounded());
    let source = root(
        &s,
        nexus_workbench::NewThread {
            worktree: Some(NewWorktree::default()),
            ..new_root(&s.project, "Remember TASK_MARKER and preserve the draft file")
        },
    )
    .await;
    let original = saved(&s, &source.id);
    let cwd = source.worktree.as_ref().expect("worktree").path.clone();
    std::fs::write(PathBuf::from(&cwd).join("draft.txt"), "uncommitted work").expect("draft");
    let capture = s.root.join("input.txt");
    s.host
        .extra
        .lock()
        .expect("fixture")
        .insert("FAKE_AGENT_INPUT_RECORD".into(), capture.to_string_lossy().into());
    let api = api(&s, true, true);
    for (at, from, to, goes_back) in [
        (2, SessionAgent::Claude, SessionAgent::Codex, false),
        (3, SessionAgent::Codex, SessionAgent::Claude, true),
    ] {
        let choice = target(from, to);
        let result = switch(&api, &source.id, &choice)
            .await
            .expect("switch through JSON API");
        assert_eq!(result["id"], source.id);
        assert_eq!(result["agent"], to.as_str());
        let record = saved(&s, &source.id);
        assert_ne!(record.native_id, original.native_id);
        assert!(!record.native_started);
        // Back to Claude: it goes on from its own first conversation, as a fork of it.
        assert_eq!(record.fork_from, goes_back.then(|| original.native_id.clone()));
        assert_eq!(record.summary.worktree, source.worktree);
        assert_eq!(record.summary.title, source.title);
        assert_eq!(record.summary.permission_mode, source.permission_mode);
        assert_eq!(
            std::fs::read_to_string(PathBuf::from(&cwd).join("draft.txt")).expect("draft"),
            "uncommitted work"
        );
        // An identical retry cannot add a second boundary or create another provider conversation.
        switch(&api, &source.id, &choice).await.expect("idempotent retry");
        s.workbench.send(&source.id, "Continue").await.expect("next message");
        let events = wait_for(&s.workbench, &source.id, |events| turn_ends(events).len() == at).await;
        let input = std::fs::read_to_string(&capture).expect("actual protocol input");
        assert!(input.contains("Continue"));
        // A fresh agent is told the original request; one going back already has it, and is told what it missed.
        if goes_back {
            assert!(input.contains("You are back on this Nexus task"));
        } else {
            assert!(input.contains("TASK_MARKER"));
        }
        assert!(
            events
                .iter()
                .rev()
                .any(|e| matches!(&e.event, WbEvent::Session { resumed: false, .. }))
        );
        assert_eq!(boundaries(&events), at - 1);
        assert_eq!(launches_of(&s, &source.id).last().expect("launch").agent, to);
        assert_eq!(
            launches_of(&s, &source.id).last().expect("launch").cwd,
            PathBuf::from(&cwd)
        );
    }
    // Claude went on from its own conversation: the fake names a fork of `<id>` `<id>-forked`.
    assert_eq!(
        saved(&s, &source.id).native_id,
        format!("{}-forked", original.native_id)
    );
    assert!(saved(&s, &source.id).summary.cost_usd >= source.cost_usd);
    s.workbench.shutdown().await;
}

#[tokio::test]
async fn a_switched_conversation_resumes_after_the_app_reopens() {
    let s = setup("switch-reopen", bounded());
    let source = root(&s, new_root(&s.project, "Remember the initial goal")).await;
    switch(
        &api(&s, true, true),
        &source.id,
        &target(SessionAgent::Claude, SessionAgent::Codex),
    )
    .await
    .expect("switch");
    s.workbench.send(&source.id, "Continue").await.expect("send");
    wait_for(&s.workbench, &source.id, |e| turn_ends(e).len() == 2).await;
    let native = saved(&s, &source.id).native_id;
    s.workbench.shutdown().await;
    let reopened = Workbench::new(
        Store::open(&s.root.join("workbench.sqlite3")).expect("store"),
        s.host.clone(),
        Config {
            scope: "gw|org".into(),
            shell: nexus_workbench::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("reopen");
    reopened.send(&source.id, "Continue again").await.expect("resume");
    let events = wait_for(&reopened, &source.id, |e| turn_ends(e).len() == 3).await;
    assert_eq!(saved(&s, &source.id).native_id, native);
    assert_eq!(thread(&reopened, &source.id).agent, SessionAgent::Codex);
    assert!(
        events
            .iter()
            .rev()
            .any(|e| matches!(e.event, WbEvent::Session { resumed: true, .. }))
    );
    reopened.shutdown().await;
}

#[tokio::test]
async fn switching_stops_running_descendants_and_does_not_start_the_destination_early() {
    let s = setup("switch-live-tree", bounded());
    let source = s
        .workbench
        .create_thread(new_root(&s.project, "slow"))
        .await
        .expect("root");
    wait_for(&s.workbench, &source.id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::TurnStart { .. }))
    })
    .await;
    let mut worker = child(SessionAgent::Codex, "slow");
    worker.title = Some("Fixture worker".into());
    let child = s.workbench.start_child(&source.id, worker).await.expect("child");
    wait_for(&s.workbench, &child.id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::TurnStart { .. }))
    })
    .await;
    switch(
        &api(&s, true, true),
        &source.id,
        &target(SessionAgent::Claude, SessionAgent::Codex),
    )
    .await
    .expect("live switch");
    assert!(!s.workbench.is_live(&source.id) && !s.workbench.is_live(&child.id));
    assert_eq!(thread(&s.workbench, &source.id).status, ThreadStatus::Idle);
    assert_eq!(launches_of(&s, &source.id).len(), 1);
    s.workbench
        .send(&source.id, "Continue")
        .await
        .expect("destination turn");
    wait_for(&s.workbench, &source.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(launches_of(&s, &source.id).len(), 2);
    s.workbench.shutdown().await;
}

#[tokio::test]
async fn switching_revokes_old_approvals_and_preserves_context_for_the_next_message() {
    let s = setup("switch-approval", bounded());
    let source = s
        .workbench
        .create_thread(new_root(&s.project, "tool"))
        .await
        .expect("root");
    let request = approval(&s.workbench, &source.id).await;
    switch(
        &api(&s, true, true),
        &source.id,
        &target(SessionAgent::Claude, SessionAgent::Codex),
    )
    .await
    .expect("switch");
    assert!(
        s.workbench
            .approve(&source.id, &request, Decision::Allow, None, None, None)
            .is_err()
    );
    assert!(saved(&s, &source.id).seed.expect("handoff").contains("User: tool"));
    s.workbench
        .send(&source.id, "Continue")
        .await
        .expect("destination turn");
    wait_for(&s.workbench, &source.id, |e| turn_ends(e).len() == 2).await;
    s.workbench.shutdown().await;
}

#[tokio::test]
async fn unavailable_agents_and_disallowed_plans_leave_the_original_process_and_history_unchanged() {
    let s = setup("switch-destination-refusal", bounded());
    let source = root(&s, new_root(&s.project, "Hello")).await;
    let before = saved(&s, &source.id);
    assert!(
        switch(
            &api(&s, false, true),
            &source.id,
            &target(SessionAgent::Claude, SessionAgent::Codex)
        )
        .await
        .is_err()
    );
    let mut choice = target(SessionAgent::Claude, SessionAgent::Codex);
    choice.via = Via::Subscription;
    assert!(switch(&api(&s, true, false), &source.id, &choice).await.is_err());
    assert!(s.workbench.is_live(&source.id));
    assert_eq!(saved(&s, &source.id).native_id, before.native_id);
    assert_eq!(boundaries(&s.workbench.events(&source.id, 0).expect("history")), 0);
    s.workbench.shutdown().await;
}

#[tokio::test]
async fn another_scope_and_a_stale_confirmation_cannot_change_the_session() {
    let s = setup("switch-source-refusal", bounded());
    let source = root(&s, new_root(&s.project, "Hello")).await;
    assert!(
        s.workbench
            .switch_agent(&source.id, target(SessionAgent::Codex, SessionAgent::Codex))
            .await
            .is_err()
    );
    s.workbench.set_scope("gw|different");
    assert!(
        s.workbench
            .switch_agent(&source.id, target(SessionAgent::Claude, SessionAgent::Codex))
            .await
            .is_err()
    );
    assert!(s.workbench.is_live(&source.id));
    s.workbench.set_scope("gw|org");
    assert_eq!(thread(&s.workbench, &source.id).agent, SessionAgent::Claude);
    s.workbench.shutdown().await;
}

#[tokio::test]
async fn a_failed_boundary_write_rolls_back_the_agent_and_native_conversation() {
    let s = setup("switch-atomic", bounded());
    let source = root(&s, new_root(&s.project, "Hello")).await;
    let before = saved(&s, &source.id);
    let db = rusqlite::Connection::open(s.root.join("workbench.sqlite3")).expect("fixture connection");
    db.execute_batch("create trigger refuse_switch before insert on events when json_extract(new.body, '$.kind') = 'agentSwitched' begin select raise(abort, 'switch fixture'); end;").expect("failure fixture");
    assert!(
        switch(
            &api(&s, true, true),
            &source.id,
            &target(SessionAgent::Claude, SessionAgent::Codex)
        )
        .await
        .is_err()
    );
    let after = saved(&s, &source.id);
    assert_eq!(after.summary.agent, before.summary.agent);
    assert_eq!(after.native_id, before.native_id);
    assert_eq!(after.seed, before.seed);
    assert_eq!(boundaries(&s.workbench.events(&source.id, 0).expect("history")), 0);
    s.workbench.shutdown().await;
}

#[tokio::test]
async fn the_desktop_reads_live_updates_from_the_new_provider_before_its_turn_ends() {
    let s = setup("switch-live-updates", bounded());
    *s.host.choice_identity.lock().expect("identity fixture") = Some("switch-person".into());
    let source = root(&s, new_root(&s.project, "Remember the initial goal")).await;
    let api = api(&s, true, true);
    switch(&api, &source.id, &target(SessionAgent::Claude, SessionAgent::Codex))
        .await
        .expect("switch");
    s.workbench
        .send(&source.id, "question nonblocking")
        .await
        .expect("new provider turn");
    wait_for(&s.workbench, &source.id, |events| {
        events.iter().any(|e| {
            matches!(
                &e.event, WbEvent::Text { text, .. } if text == "Checking another file while you answer."
            )
        })
    })
    .await;
    let live = api
        .call("wb_thread_stream", json!({"threadId":source.id,"afterSeq":0}))
        .await
        .expect("desktop live snapshot");
    assert_eq!(live["threadId"], source.id);
    assert_eq!(live["summary"]["agent"], "codex");
    assert!(
        live["incarnation"].is_string(),
        "the destination still owns a live process"
    );
    assert!(
        live["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["event"]["kind"] == "text" && e["event"]["text"] == "Checking another file while you answer.")
    );
    assert_eq!(turn_ends(&s.workbench.events(&source.id, 0).expect("events")).len(), 1);
    assert!(
        s.host
            .summaries
            .lock()
            .expect("emitted summaries")
            .iter()
            .any(|summary| summary.id == source.id && summary.agent == SessionAgent::Codex)
    );
    s.workbench.shutdown().await;
}
