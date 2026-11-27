//! Real in-memory store, registered producers, handle/append and coherent read; no agent process/toolchain fixtures.
use super::Partial;
use crate::agents::TaskBook;
use crate::driver::{Out, Protocol, Say};
use crate::event::{SessionAgent, ThreadSummary, TurnStatus, Usage, Via, WbEvent};
use crate::store::{Store, ThreadRecord};
use crate::{
    Config, Emit, Host, Launch, LaunchRequest, Live, LiveState, ProcessEnd, Result, Workbench, event, orchestra,
    short_id,
};
use futures::future::BoxFuture;
use serde_json::json;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::{mpsc, watch};
#[derive(Default)]
struct Owner {
    identity: Mutex<String>,
    ended: Mutex<Vec<String>>,
}
impl Host for Owner {
    fn session_choice_identity(&self) -> Option<String> {
        Some(self.identity.lock().expect("fixture identity lock").clone())
    }
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, Result<Launch>> {
        Box::pin(async { panic!("snapshot fixture never launches") })
    }
    fn ended(&self, id: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        self.ended.lock().expect("fixture ended-session lock").push(id);
        Box::pin(async { true })
    }
    fn price(&self, _: &str, _: &Usage) -> Option<f64> {
        None
    }
    fn emit(&self, _: Emit) {}
}
struct Silent;
impl Protocol for Silent {
    fn open(&mut self) -> Vec<Out> {
        vec![]
    }
    fn say(&mut self, _: Say) -> Vec<Out> {
        vec![]
    }
    fn read(&mut self, _: &str) -> Vec<Out> {
        vec![]
    }
}
fn producer() -> Arc<Live> {
    let (tx, _) = mpsc::unbounded_channel();
    let (_, exited) = watch::channel(false);
    let protocol: Box<dyn Protocol> = Box::new(Silent);
    let agent = SessionAgent::Claude;
    Arc::new(Live {
        saved_provider: false,
        partial: Mutex::default(),
        created: Mutex::default(),
        identity: Some("person-a".into()),
        recovery_identity: String::new(),
        agent,
        antigravity_args: Vec::new(),
        scope: "scope-a".into(),
        protocol: Mutex::new(protocol),
        status: Mutex::new(()),
        tx,
        exited,
        state: Mutex::new(LiveState {
            pid: None,
            pending: HashMap::new(),
            turns: VecDeque::new(),
            background: HashSet::new(),
            book: TaskBook::default(),
            base_cost: 0.0,
            confirmed: 0.0,
            estimate: 0.0,
            nexus_session: None,
            local_run: None,
            initialized: false,
            name_due: false,
            stop: None,
            last_activity: Instant::now(),
            last_delta: HashMap::new(),
            unwritten: None,
            usage_written: None,
            streamed: HashMap::new(),
            stderr: String::new(),
            explained: false,
            requests: 0,
            cut_short: false,
            reaped: false,
            changes: HashMap::new(),
            said: 0,
            mode_said: 0,
            spawned: false,
            // The last run's last checkpoint, if still being written: this run's first turn waits for it too (R5).
            checkpoint: None,
            releasing: false,
            release_queued: false,
            interrupt_after_release: None,
            spend_notice_pending: false,
            unsent: VecDeque::new(),
            unread: VecDeque::new(),
            steering: HashSet::new(),
            done: false,
            cursor: None,
            capabilities: None,
            connection: event::SessionConnection {
                generation: short_id(),
                via: Some(Via::Nexus),
                ..event::SessionConnection::default()
            },
            last_connection: None,
            account_revision: 0,
            mcp_revision: 0,
            skills_revision: 0,
            task_asks: HashMap::new(),
            owed: 0,
            owed_since: Instant::now(),
            notice_due: None,
            grant: Some(orchestra::Grant::new()),
            refused: None,
            account_dir: None,
            browser: Default::default(),
            resend: None,
            transcript: None,
        }),
    })
}
fn fixture() -> (Workbench, Arc<Owner>, Arc<Live>) {
    let store = Store::in_memory().expect("in-memory fixture store");
    let project = store
        .add_project(Path::new("/fixture/snapshot"), 100)
        .expect("fixture project insertion");
    let summary:ThreadSummary=serde_json::from_value(json!({"id":"side","projectId":project.id,"title":"Side","agent":"claude","model":null,"permissionMode":"default","via":"nexus","status":"idle","costUsd":0.,"nexusSessions":[],"createdAt":100,"updatedAt":100,"archived":false,"sideChat":true,"sourceThreadId":"parent"})).expect("valid fixture thread summary");
    store
        .insert_thread(&ThreadRecord {
            summary,
            native_id: "native".into(),
            native_started: false,
            scope: "scope-a".into(),
            fork_from: None,
            seed: None,
            automatic_title: false,
            delivered_seq: 0,
            halt: None,
        })
        .expect("fixture thread insertion");
    let owner = Arc::new(Owner {
        identity: Mutex::new("person-a".into()),
        ..Owner::default()
    });
    let wb = Workbench::new(
        store,
        owner.clone(),
        Config {
            scope: "scope-a".into(),
            ..Config::default()
        },
    )
    .expect("fixture workbench initialization");
    let live = producer();
    wb.inner
        .live
        .lock()
        .expect("fixture live-producer registry")
        .insert("side".into(), live.clone());
    (wb, owner, live)
}
fn text(id: &str) -> WbEvent {
    WbEvent::Text {
        id: id.into(),
        parent: None,
        text: "complete".into(),
    }
}
#[test]
fn coherent_snapshot_survives_log_reads_and_final_text_removes_exact_partial() {
    let (wb, _, live) = fixture();
    // Full cumulative text is kept even inside the ordinary50ms sent-delta throttle.
    live.state().last_delta.insert(
        Some("child".into()),
        Instant::now() + std::time::Duration::from_secs(60),
    );
    wb.handle(
        "side",
        &live,
        Out::Delta {
            id: "m".into(),
            parent: Some("child".into()),
            text: "Hé😀".into(),
        },
    )
    .expect("child delta handled before throttle");
    wb.partial_delta("side", &live, "main", None, "main partial");
    assert!(crate::api::COMMANDS.contains(&"wb_thread_stream"));
    let snapshot = wb.thread_stream("side", 0).expect("initial coherent stream snapshot");
    assert_eq!(snapshot.messages.len(), 2);
    wb.events("side", 0).expect("durable event read");
    assert_eq!(
        wb.thread_stream("side", 0)
            .expect("logical snapshot after durable read")
            .messages,
        snapshot.messages
    );
    let final_row = wb
        .append_from("side", &live, text("main"))
        .expect("main final text appended");
    let snapshot = wb.thread_stream("side", 0).expect("snapshot after main final text");
    assert_eq!(snapshot.through_seq, final_row.seq);
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| matches!(&event.event,WbEvent::Text{id,..} if id=="main"))
    );
    assert!(snapshot.messages.iter().all(|message| message.id != "main"));
    assert_eq!(snapshot.messages[0].parent.as_deref(), Some("child"));
    wb.partial_delta("side", &live, "main", None, "late same message");
    assert!(
        wb.thread_stream("side", 0)
            .expect("snapshot after late finalized delta")
            .messages
            .iter()
            .all(|message| message.id != "main")
    );
    let wire = serde_json::to_string(&snapshot).expect("snapshot serializes without credentials");
    assert!(!wire.contains("person-a") && !wire.contains("scope-a"));
    assert!(live.state().nexus_session.is_none()); // plan/no Nexus session ID does not prevent a run incarnation.
}
#[test]
fn old_producer_delta_and_final_event_cannot_mutate_replacement_cut() {
    let (wb, _, old) = fixture();
    wb.partial_delta("side", &old, "m", None, "old");
    let old_incarnation = wb.thread_stream("side", 0).expect("old producer snapshot").incarnation;
    let fresh = producer();
    wb.inner
        .live
        .lock()
        .expect("replacement producer registry")
        .insert("side".into(), fresh.clone());
    wb.partial_delta("side", &fresh, "m", None, "current");
    wb.partial_delta("side", &old, "m", None, "old late delta");
    assert!(wb.append_from("side", &old, text("m")).is_err());
    wb.handle("side", &old, Out::Event(text("m")))
        .expect("retired producer output ignored");
    let cut = wb.thread_stream("side", 0).expect("replacement coherent snapshot");
    assert_ne!(cut.incarnation, old_incarnation);
    assert_eq!(cut.messages[0].text, "current");
    assert!(cut.events.is_empty());
}
#[test]
fn frozen_identity_scope_and_cursor_are_checked_by_the_owner() {
    let (wb, owner, live) = fixture();
    wb.partial_delta("side", &live, "m", None, "private");
    *owner.identity.lock().expect("fixture identity revocation lock") = "person-b".into();
    assert!(wb.thread_stream("side", 0).is_err());
    *owner.identity.lock().expect("fixture identity restoration lock") = "person-a".into();
    wb.set_scope("scope-b");
    assert!(wb.thread_stream("side", 0).is_err());
    wb.set_scope("scope-a");
    assert_eq!(
        wb.thread_stream("side", 0)
            .expect("authorized snapshot after scope restoration")
            .messages[0]
            .text,
        "private"
    );
    assert!(wb.thread_stream("side", -1).is_err());
    assert!(wb.thread_stream("side", 99).is_err());
}
#[test]
fn interrupt_then_new_turn_on_same_producer_resumes_partials() {
    let (wb, _, live) = fixture();
    live.state().spawned = true;
    wb.partial_delta("side", &live, "old", None, "before stop");
    wb.interrupt("side").expect("fixture interrupt succeeds");
    assert!(
        wb.thread_stream("side", 0)
            .expect("snapshot after interrupt")
            .messages
            .is_empty()
    );
    // An interactive process can remain alive after an interrupt. The next authorized turn starts
    // on its first delta, which must survive the TurnStart/reset boundary in the real handler.
    live.state().stop = None;
    wb.person_turn_begins("side");
    wb.handle(
        "side",
        &live,
        Out::Delta {
            id: "next".into(),
            parent: None,
            text: "next partial".into(),
        },
    )
    .expect("first resumed delta handled");
    assert_eq!(
        wb.thread_stream("side", 0)
            .expect("first resumed partial snapshot")
            .messages[0]
            .text,
        "next partial"
    );
    wb.append_from(
        "side",
        &live,
        WbEvent::TurnEnd {
            turn_id: "next-turn".into(),
            status: TurnStatus::Interrupted,
            cost_usd: None,
            duration_ms: 1,
            error: None,
            cursor: None,
        },
    )
    .expect("resumed turn final event appended");
    assert!(
        wb.thread_stream("side", 0)
            .expect("snapshot after resumed turn completion")
            .messages
            .is_empty()
    );
}
#[test]
fn catchup_is_incremental_and_withholds_partials_until_all_completions_are_read() {
    let (wb, _, live) = fixture();
    for _ in 0..260 {
        wb.append_from("side", &live, WbEvent::Notice { text: "row".into() })
            .expect("catchup notice appended");
    }
    wb.partial_delta("side", &live, "m", None, "current");
    let first = wb.thread_stream("side", 0).expect("first bounded catchup snapshot");
    assert!(first.more);
    assert_eq!(first.events.len(), 256);
    assert!(first.messages.is_empty());
    let next = wb
        .thread_stream("side", first.events.last().expect("first catchup page has events").seq)
        .expect("remaining catchup snapshot");
    assert!(!next.more);
    assert_eq!(next.events.len(), 4);
    assert_eq!(next.messages[0].text, "current");
}
#[tokio::test]
async fn retired_finish_still_disposes_its_own_session_without_publishing_over_replacement() {
    let (wb, owner, old) = fixture();
    old.state().nexus_session = Some("old-run-session".into());
    let fresh = producer();
    wb.inner
        .live
        .lock()
        .expect("replacement producer registry")
        .insert("side".into(), fresh.clone());
    wb.partial_delta("side", &fresh, "m", None, "new");
    wb.finish("side", &old, (0, None), Instant::now()).await;
    assert_eq!(
        &*owner.ended.lock().expect("ended-session receipt lock"),
        &["old-run-session"]
    );
    assert!(Arc::ptr_eq(
        &wb.live("side").expect("replacement producer remains registered"),
        &fresh
    ));
    assert_eq!(
        wb.thread_stream("side", 0)
            .expect("replacement snapshot after retired cleanup")
            .messages[0]
            .text,
        "new"
    );
}
#[test]
fn logical_partial_has_a_bounded_size_and_keeps_subagents_separate() {
    let mut partial = Partial::default();
    partial.delta("m", None, "main");
    partial.delta("m", Some("child"), "child");
    assert_eq!(partial.messages.len(), 2);
    partial.event(&text("m"));
    assert_eq!(partial.messages.len(), 1);
    partial.delta("huge", None, &"x".repeat(1024 * 1024 + 1));
    assert_eq!(partial.messages.len(), 1);
    partial.event(&serde_json::from_value::<WbEvent>(json!({"kind":"task","taskId":"t","toolUseId":"child","taskType":"local_agent","status":"completed","description":""})).expect("valid completed subagent task event"));
    assert!(partial.messages.is_empty());
}

#[test]
fn finalized_overflow_stays_durable_only_for_the_rest_of_that_run() {
    let mut partial = Partial::default();
    for n in 0..4097 {
        partial.event(&text(&format!("m-{n}")));
    }
    assert!(partial.overflow);
    assert!(partial.finalized.is_empty());
    partial.event(&WbEvent::TurnStart { turn_id: "next".into() });
    partial.delta("m-0", None, "late finalized output");
    assert!(partial.messages.is_empty());
}

#[test]
fn concurrent_final_append_and_snapshot_never_describe_finalized_text_as_live() {
    let (wb, _, live) = fixture();
    for n in 0..32 {
        let id = format!("m-{n}");
        wb.partial_delta("side", &live, &id, None, "partial");
        let snapshot = std::thread::scope(|scope| {
            let final_row = scope.spawn(|| wb.append_from("side", &live, text(&id)).expect("append final text"));
            let read = scope.spawn(|| wb.thread_stream("side", 0).expect("concurrent coherent snapshot"));
            let snapshot = read.join().expect("snapshot reader thread completes");
            final_row.join().expect("final append thread completes");
            snapshot
        });
        let completed = snapshot
            .events
            .iter()
            .any(|row| matches!(&row.event,WbEvent::Text{id:done,..} if done==&id));
        if completed {
            assert!(snapshot.messages.iter().all(|message| message.id != id));
        }
    }
}
