//! Delivery to a real workbench and stand-in agent. Transport authenticity is tested by gateway/control.rs.

mod support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use nexus_workbench::event::{Decision, TurnStatus, WbEvent};
use nexus_workbench::session_sync::control::{Action, Command};
use nexus_workbench::session_sync::{Backend, Batch, ContentKey, ContentPolicy, Identity, Policy};
use nexus_workbench::store::Store;
use nexus_workbench::{Config, Result, Workbench};
use support::*;

const DAY: i64 = 24 * 60 * 60 * 1000;

struct Mailbox {
    identity: Identity,
    heads: Mutex<HashMap<String, u64>>,
    commands: Mutex<Vec<Command>>,
    results: Mutex<HashMap<String, bool>>,
    reads: AtomicUsize,
    delay: Duration,
    entered: tokio::sync::Notify,
}

struct Key;
impl ContentKey for Key {
    fn epoch(&self) -> u64 {
        0
    }
    fn seal(&self, _: &str, _: u64, plaintext: &[u8]) -> Result<Vec<u8>> {
        Ok(plaintext.to_vec())
    }
}

impl Mailbox {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            identity: Identity {
                scope: "gw|org".into(),
                user: "person-a".into(),
            },
            heads: Mutex::default(),
            commands: Mutex::default(),
            results: Mutex::default(),
            reads: AtomicUsize::new(0),
            delay: Duration::ZERO,
            entered: tokio::sync::Notify::new(),
        })
    }
    fn push(&self, thread: &str, action: Action) -> String {
        self.push_sent(thread, action, None)
    }
    /// A command that waited for this computer: `sent_at` by the phone's clock, alive for a day from then.
    fn push_sent(&self, thread: &str, action: Action, sent_at: Option<i64>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let expires_at = sent_at.map_or(nexus_workbench::now_ms() + 60_000, |sent| sent + DAY);
        self.commands.lock().expect("commands").push(Command {
            id: id.clone(),
            thread_id: thread.into(),
            sender_device_id: "dev_phone".into(),
            expires_at,
            sent_at,
            action,
        });
        id
    }
}

impl Backend for Mailbox {
    fn identity(&self) -> &Identity {
        &self.identity
    }
    fn allow_local_cache(&self) -> bool {
        true
    }
    fn policy(&self) -> BoxFuture<'_, Result<Policy>> {
        Box::pin(async {
            Ok(Policy {
                content: ContentPolicy::UserOnly,
            })
        })
    }
    fn head(&self, thread: &str) -> BoxFuture<'_, Result<Option<u64>>> {
        let head = self.heads.lock().expect("heads").get(thread).copied();
        Box::pin(async move { Ok(head) })
    }
    fn key(&self, _: &str, _: Option<u64>, _: ContentPolicy) -> BoxFuture<'_, Result<Arc<dyn ContentKey>>> {
        Box::pin(async { Ok(Arc::new(Key) as Arc<dyn ContentKey>) })
    }
    fn append(&self, batch: Batch) -> BoxFuture<'_, Result<u64>> {
        let last = batch.events.last().expect("events").seq;
        self.heads.lock().expect("heads").insert(batch.thread, last);
        Box::pin(async move { Ok(last) })
    }
    fn delete(&self, _: &str) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn supports_control(&self) -> bool {
        true
    }
    fn control_commands(&self) -> BoxFuture<'_, Result<Vec<Command>>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let commands = self.commands.lock().expect("commands").clone();
        Box::pin(async move {
            self.entered.notify_one();
            tokio::time::sleep(self.delay).await;
            Ok(commands)
        })
    }
    // Deliberately lies on replay: the local receipt must still prevent duplicate side effects.
    fn control_claim(&self, _: &str, _: &str) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn control_finish(&self, _: &str, id: &str, completed: bool) -> BoxFuture<'_, Result<()>> {
        self.results.lock().expect("results").insert(id.into(), completed);
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn slow_phone_transport_cannot_starve_session_uploads() {
    let s = setup("phone-control-slow", bounded());
    let mut backend = Mailbox::new();
    Arc::get_mut(&mut backend).expect("only owner").delay = Duration::from_secs(3);
    *s.host.sync.lock().expect("sync") = Some(backend.clone());
    let thread = root(&s, new_root(&s.project, "Hello")).await;
    s.workbench.set_code_control(true).expect("enable");
    let (stop, watch) = tokio::sync::watch::channel(false);
    let workbench = s.workbench.clone();
    let worker = tokio::spawn(async move { workbench.run_session_sync(watch).await });
    backend.entered.notified().await;
    let before = backend
        .heads
        .lock()
        .expect("heads")
        .values()
        .copied()
        .max()
        .unwrap_or(0);
    s.workbench
        .send(&thread.id, "A new prompt during a delayed phone read")
        .await
        .expect("send");
    wait_for(&s.workbench, &thread.id, |events| turn_ends(events).len() == 2).await;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if backend.heads.lock().expect("heads").values().any(|seq| *seq > before) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("uploads progress during the 3 second control read");
    stop.send(true).expect("stop worker");
    worker.await.expect("worker ends");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

#[tokio::test]
async fn paired_device_actions_reach_an_idle_agent_approvals_and_stop_once_even_after_reopen() {
    let s = setup("phone-control", bounded());
    let backend = Mailbox::new();
    *s.host.sync.lock().expect("sync") = Some(backend.clone());
    let thread = root(&s, new_root(&s.project, "Hello")).await;
    let report = s.workbench.sync_sessions_once().await;
    assert!(report.failures.is_empty());
    let sync = backend
        .heads
        .lock()
        .expect("heads")
        .keys()
        .next()
        .expect("synced thread")
        .clone();
    let send = backend.push(&sync, Action::Send { text: "tool".into() });
    s.workbench.control_sessions_once().await;
    assert_eq!(backend.reads.load(Ordering::SeqCst), 0, "off by default");
    assert!(!s.workbench.code_control().expect("settings").enabled);
    s.workbench.set_code_control(true).expect("enable locally");
    s.workbench.control_sessions_once().await;
    let request = approval(&s.workbench, &thread.id).await;
    let approve = backend.push(
        &sync,
        Action::Approve {
            request_id: request.clone(),
            decision: Decision::Allow,
            message: None,
            answers: None,
        },
    );
    s.workbench.control_sessions_once().await;
    let events = wait_for(&s.workbench, &thread.id, |events| turn_ends(events).len() == 2).await;
    assert!(events.iter().any(|event|matches!(&event.event,WbEvent::ApprovalResolved{request_id,decision:Decision::Allow,..} if request_id==&request)));
    let slow = backend.push(&sync, Action::Send { text: "slow".into() });
    s.workbench.control_sessions_once().await;
    wait_for(&s.workbench, &thread.id, |events| {
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::TurnStart { .. }))
            .count()
            == 3
    })
    .await;
    let stop = backend.push(&sync, Action::Stop);
    s.workbench.control_sessions_once().await;
    let events = wait_for(&s.workbench, &thread.id, |events| turn_ends(events).len() == 3).await;
    assert_eq!(
        users(&events).into_iter().map(|(text, _)| text).collect::<Vec<_>>(),
        ["Hello", "tool", "slow"]
    );
    assert_eq!(turn_ends(&events).last(), Some(&TurnStatus::Interrupted));
    // As gateway_e2e/control.rs reads it: the interrupted turn's end is the log's last event.
    assert!(matches!(
        events.last().map(|event| &event.event),
        Some(WbEvent::TurnEnd {
            status: TurnStatus::Interrupted,
            ..
        })
    ));
    assert!(
        s.workbench.is_live(&thread.id),
        "the phone's Stop is the desktop's: the turn ends and the agent keeps the conversation"
    );
    assert_eq!(
        notices(&events),
        [
            "Your phone sent a message.",
            "Your phone allowed Bash.",
            "Your phone sent a message.",
            "Your phone stopped the turn."
        ]
    );
    assert_eq!(backend.results.lock().expect("results").len(), 4);
    for id in [send, approve, slow, stop] {
        assert_eq!(backend.results.lock().expect("results").get(&id), Some(&true));
    }
    let count = events.len();
    s.workbench.shutdown().await;
    let reopened = Workbench::new(
        Store::open(&s.root.join("workbench.sqlite3")).expect("reopen"),
        s.host.clone(),
        Config {
            scope: "gw|org".into(),
            shell: nexus_workbench::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("workbench");
    assert!(reopened.code_control().expect("durable opt-in").enabled);
    reopened.control_sessions_once().await;
    assert_eq!(
        reopened.events(&thread.id, 0).expect("events").len(),
        count,
        "a replay after restart has no new side effect"
    );
    reopened.set_code_control(false).expect("disable");
    backend.push(
        &sync,
        Action::Send {
            text: "must not arrive".into(),
        },
    );
    reopened.control_sessions_once().await;
    assert_eq!(reopened.events(&thread.id, 0).expect("events").len(), count);
    reopened.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

/// The phone's notices: each names its action, never what was sent.
fn notices(events: &[nexus_workbench::event::StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            WbEvent::Notice { text } if text.starts_with("Your phone") => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A synced session with Phone control on, its first turn ended: the synced thread's id.
async fn controlled(s: &Setup, backend: &Arc<Mailbox>) -> (String, String) {
    *s.host.sync.lock().expect("sync") = Some(backend.clone());
    let thread = root(s, new_root(&s.project, "Hello")).await;
    assert!(s.workbench.sync_sessions_once().await.failures.is_empty());
    s.workbench.set_code_control(true).expect("enable locally");
    let sync = backend
        .heads
        .lock()
        .expect("heads")
        .keys()
        .next()
        .expect("synced")
        .clone();
    (thread.id, sync)
}

fn result(backend: &Mailbox, id: &str) -> Option<bool> {
    backend.results.lock().expect("results").get(id).copied()
}

#[tokio::test]
async fn allow_for_this_session_from_the_phone_keeps_the_agent_s_session_rule() {
    let s = setup("phone-control-session", bounded());
    let backend = Mailbox::new();
    let (thread, sync) = controlled(&s, &backend).await;
    backend.push(&sync, Action::Send { text: "tool".into() });
    s.workbench.control_sessions_once().await;
    let request = approval(&s.workbench, &thread).await;
    let allow = backend.push(
        &sync,
        Action::Approve {
            request_id: request.clone(),
            decision: Decision::AllowSession,
            message: None,
            answers: None,
        },
    );
    s.workbench.control_sessions_once().await;
    let events = wait_for(&s.workbench, &thread, |events| turn_ends(events).len() == 2).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, WbEvent::ToolResult { output, .. } if output == "hi (remembered)")),
        "the stand-in agent got the session rule: {events:#?}"
    );
    assert!(events.iter().any(|event| matches!(&event.event,
        WbEvent::ApprovalResolved { request_id, decision: Decision::AllowSession, .. } if *request_id == request)));
    assert_eq!(
        notices(&events),
        [
            "Your phone sent a message.",
            "Your phone allowed Bash for this session."
        ]
    );
    assert_eq!(result(&backend, &allow), Some(true));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

#[tokio::test]
async fn every_claimed_command_gets_a_result_and_stop_on_an_idle_session_succeeds() {
    let s = setup("phone-control-results", bounded());
    let backend = Mailbox::new();
    let (thread, sync) = controlled(&s, &backend).await;
    let before = s.workbench.events(&thread, 0).expect("events").len();
    let gone = backend.push(&uuid::Uuid::new_v4().to_string(), Action::Stop);
    let expired = backend.push(
        &sync,
        Action::Approve {
            request_id: "perm-1".into(),
            decision: Decision::Expired,
            message: None,
            answers: None,
        },
    );
    let unknown = backend.push(
        &sync,
        Action::Approve {
            request_id: "no-such-request".into(),
            decision: Decision::Allow,
            message: None,
            answers: None,
        },
    );
    s.workbench.control_sessions_once().await;
    assert_eq!(
        result(&backend, &gone),
        Some(false),
        "a thread this computer no longer has"
    );
    assert_eq!(
        result(&backend, &expired),
        Some(false),
        "an action the runner never runs"
    );
    assert_eq!(result(&backend, &unknown), Some(false), "an approval already answered");
    assert_eq!(
        s.workbench.events(&thread, 0).expect("events").len(),
        before,
        "a rejected command leaves no notice"
    );
    backend.commands.lock().expect("commands").clear();
    // The turn ended before the phone's Stop arrived: nothing to stop is no failure.
    let idle = backend.push(&sync, Action::Stop);
    s.workbench.control_sessions_once().await;
    assert_eq!(result(&backend, &idle), Some(true));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

/// Messages the phone sent while this computer slept run once each, in send order, and say so. A Stop or an
/// approval answer that waited past the normal minute is rejected: a late one is wrong.
#[tokio::test]
async fn waited_messages_run_once_in_order_and_a_late_stop_or_answer_is_rejected() {
    let s = setup("phone-control-waited", bounded());
    let backend = Mailbox::new();
    let (thread, sync) = controlled(&s, &backend).await;
    let asleep = nexus_workbench::now_ms() - 2 * 60 * 60 * 1000;
    let late_stop = backend.push_sent(&sync, Action::Stop, Some(asleep));
    let late_answer = backend.push_sent(
        &sync,
        Action::Approve {
            request_id: "perm-1".into(),
            decision: Decision::Allow,
            message: None,
            answers: None,
        },
        Some(asleep),
    );
    let first = backend.push_sent(&sync, Action::Send { text: "first".into() }, Some(asleep));
    let second = backend.push_sent(&sync, Action::Send { text: "second".into() }, Some(asleep + 1));
    let stale = backend.push_sent(&sync, Action::Send { text: "too old".into() }, Some(asleep - DAY));
    s.workbench.control_sessions_once().await;
    let events = wait_for(&s.workbench, &thread, |events| users(events).len() == 3).await;
    assert_eq!(
        users(&events).into_iter().map(|(text, _)| text).collect::<Vec<_>>(),
        ["Hello", "first", "second"]
    );
    let waited = notices(&events);
    assert_eq!(waited.len(), 2, "{waited:?}");
    for notice in &waited {
        assert!(
            notice.starts_with("Your phone's message from ")
                && notice.ends_with(" arrived while this computer was asleep."),
            "{notice}"
        );
    }
    for (id, completed) in [(late_stop, false), (late_answer, false), (first, true), (second, true)] {
        assert_eq!(result(&backend, &id), Some(completed));
    }
    assert_eq!(result(&backend, &stale), None, "past its day: never claimed, never run");
    // Gateway's claim lies on replay here: the local receipt still runs each one once.
    s.workbench.control_sessions_once().await;
    assert_eq!(users(&s.workbench.events(&thread, 0).expect("events")).len(), 3);
    // A Stop the phone let wait still runs within the normal minute.
    backend.commands.lock().expect("commands").clear();
    let fresh_stop = backend.push_sent(&sync, Action::Stop, Some(nexus_workbench::now_ms()));
    s.workbench.control_sessions_once().await;
    assert_eq!(result(&backend, &fresh_stop), Some(true));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

#[tokio::test]
async fn a_command_read_before_phone_control_is_turned_off_is_rejected_not_run() {
    let s = setup("phone-control-off", bounded());
    let mut backend = Mailbox::new();
    Arc::get_mut(&mut backend).expect("only owner").delay = Duration::from_millis(300);
    let (thread, sync) = controlled(&s, &backend).await;
    let before = s.workbench.events(&thread, 0).expect("events").len();
    let send = backend.push(
        &sync,
        Action::Send {
            text: "must not arrive".into(),
        },
    );
    let workbench = s.workbench.clone();
    let pass = tokio::spawn(async move { workbench.control_sessions_once().await });
    backend.entered.notified().await;
    s.workbench
        .set_code_control(false)
        .expect("turn off while the read is out");
    pass.await.expect("pass ends");
    assert_eq!(result(&backend, &send), Some(false));
    assert_eq!(s.workbench.events(&thread, 0).expect("events").len(), before);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}
