//! A CLI run's phone control: it takes only its own thread's commands from the shared device inbox, runs
//! each once, and leases its thread only while it can apply commands and Phone control is on.

mod support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use nexus_workbench::Result;
use nexus_workbench::cli_log::CliRun;
use nexus_workbench::event::{Agent, Decision, Via, WbEvent};
use nexus_workbench::session_sync::control::{Action, Command};
use nexus_workbench::session_sync::control_loop::{Performer, ThreadControl, set_cli_control};
use nexus_workbench::session_sync::{Backend, Batch, ContentKey, ContentPolicy, Identity, Policy};
use nexus_workbench::store::Store;
use support::*;

/// One device inbox, shared by every run on this computer.
struct Inbox {
    identity: Identity,
    heads: Mutex<HashMap<String, u64>>,
    commands: Mutex<Vec<Command>>,
    claims: Mutex<Vec<String>>,
    leases: Mutex<Vec<(String, bool)>>,
    results: Mutex<HashMap<String, bool>>,
    reads: AtomicUsize,
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

impl Inbox {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            identity: Identity {
                scope: "gw|org".into(),
                user: "person-a".into(),
            },
            heads: Mutex::default(),
            commands: Mutex::default(),
            claims: Mutex::default(),
            leases: Mutex::default(),
            results: Mutex::default(),
            reads: AtomicUsize::new(0),
        })
    }
    fn push(&self, thread: &str, action: Action) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.commands.lock().expect("commands").push(Command {
            id: id.clone(),
            thread_id: thread.into(),
            sender_device_id: "dev_phone".into(),
            expires_at: nexus_workbench::now_ms() + 60_000,
            sent_at: None,
            action,
        });
        id
    }
    fn result(&self, id: &str) -> Option<bool> {
        self.results.lock().expect("results").get(id).copied()
    }
    fn last_lease(&self) -> Option<(String, bool)> {
        self.leases.lock().expect("leases").last().cloned()
    }
}

impl Backend for Inbox {
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
    fn control_lease(&self, thread: &str, enabled: bool) -> BoxFuture<'_, Result<()>> {
        self.leases.lock().expect("leases").push((thread.into(), enabled));
        Box::pin(async { Ok(()) })
    }
    fn control_commands(&self) -> BoxFuture<'_, Result<Vec<Command>>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let commands = self.commands.lock().expect("commands").clone();
        Box::pin(async move { Ok(commands) })
    }
    // Deliberately lies on replay: the local receipt must still prevent a second run.
    fn control_claim(&self, _: &str, id: &str) -> BoxFuture<'_, Result<bool>> {
        self.claims.lock().expect("claims").push(id.into());
        Box::pin(async { Ok(true) })
    }
    fn control_finish(&self, _: &str, id: &str, completed: bool) -> BoxFuture<'_, Result<()>> {
        self.results.lock().expect("results").insert(id.into(), completed);
        Box::pin(async { Ok(()) })
    }
}

/// A stand-in agent: records what it ran.
#[derive(Default)]
struct Recorder {
    ready: AtomicBool,
    ran: Mutex<Vec<(String, String)>>,
}

impl Recorder {
    fn ready() -> Self {
        let recorder = Recorder::default();
        recorder.ready.store(true, Ordering::SeqCst);
        recorder
    }
    fn ran(&self) -> Vec<(String, String)> {
        self.ran.lock().expect("ran").clone()
    }
}

impl Performer for Recorder {
    fn ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }
    fn perform<'a>(&'a self, local: &'a str, _: Option<i64>, action: Action) -> BoxFuture<'a, Result<()>> {
        let what = match action {
            Action::Send { text } => text,
            Action::Stop => "stop".into(),
            Action::Approve { request_id, .. } => request_id,
        };
        self.ran.lock().expect("ran").push((local.into(), what));
        Box::pin(async { Ok(()) })
    }
}

/// A CLI conversation's thread, synced once: its local and synced ids.
async fn cli_thread(s: &Setup, backend: &Inbox, native: &str) -> (String, String) {
    let cwd = s.root.join("repo");
    let run = CliRun {
        agent: Agent::Codex,
        native_id: native,
        cwd: &cwd,
        model: None,
        via: Via::Nexus,
    };
    let thread = s.workbench.cli_thread(&run).expect("thread").expect("bound");
    let user = WbEvent::User {
        text: "Hello".into(),
        images: vec![],
        from: None,
    };
    s.workbench
        .cli_append(&thread.id, &[(nexus_workbench::now_ms(), user)], 1)
        .expect("append");
    let before: Vec<String> = backend.heads.lock().expect("heads").keys().cloned().collect();
    assert!(s.workbench.sync_sessions_once().await.failures.is_empty());
    let sync = backend
        .heads
        .lock()
        .expect("heads")
        .keys()
        .find(|key| !before.contains(key))
        .expect("synced")
        .clone();
    (thread.id, sync)
}

fn phone_control(s: &Setup, backend: &Inbox, enabled: bool) {
    // Another handle on the same file, as `nexus devices control` opens it.
    let store = Store::open(&s.root.join("workbench.sqlite3")).expect("store");
    set_cli_control(&store, &backend.identity, enabled).expect("switch");
}

#[tokio::test]
async fn a_run_takes_only_its_own_thread_s_commands_and_runs_each_once() {
    let s = setup("cli-control-own", bounded());
    let backend = Inbox::new();
    *s.host.sync.lock().expect("sync") = Some(backend.clone());
    let (local_a, sync_a) = cli_thread(&s, &backend, "conversation-a").await;
    let (local_b, sync_b) = cli_thread(&s, &backend, "conversation-b").await;
    phone_control(&s, &backend, true);
    let for_a = backend.push(&sync_a, Action::Send { text: "to a".into() });
    let for_b = backend.push(&sync_b, Action::Send { text: "to b".into() });
    let (agent_a, agent_b) = (Recorder::ready(), Recorder::ready());
    let (mut run_a, mut run_b) = (ThreadControl::new(&local_a), ThreadControl::new(&local_b));

    s.workbench.control_thread_once(&mut run_a, &agent_a).await;
    assert_eq!(agent_a.ran(), [(local_a.clone(), "to a".to_string())]);
    assert_eq!(
        *backend.claims.lock().expect("claims"),
        std::slice::from_ref(&for_a),
        "the other run's command is never claimed"
    );
    assert_eq!(backend.result(&for_a), Some(true));
    assert_eq!(backend.result(&for_b), None);

    s.workbench.control_thread_once(&mut run_b, &agent_b).await;
    assert_eq!(agent_b.ran(), [(local_b.clone(), "to b".to_string())]);
    assert_eq!(backend.result(&for_b), Some(true));

    // Gateway replays both: the receipts keep each from running twice.
    s.workbench.control_thread_once(&mut run_a, &agent_a).await;
    s.workbench.control_thread_once(&mut run_b, &agent_b).await;
    assert_eq!(agent_a.ran().len(), 1);
    assert_eq!(agent_b.ran().len(), 1);

    // A command no runner runs is rejected, and the agent never sees it.
    let expired = backend.push(
        &sync_a,
        Action::Approve {
            request_id: "item#0".into(),
            decision: Decision::Expired,
            message: None,
            answers: None,
        },
    );
    s.workbench.control_thread_once(&mut run_a, &agent_a).await;
    assert_eq!(backend.result(&expired), Some(false));
    assert_eq!(agent_a.ran().len(), 1);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

#[tokio::test]
async fn the_lease_is_on_only_while_the_run_can_act_and_phone_control_is_on() {
    let s = setup("cli-control-lease", bounded());
    let backend = Inbox::new();
    *s.host.sync.lock().expect("sync") = Some(backend.clone());
    let (local, sync) = cli_thread(&s, &backend, "conversation").await;
    let agent = Recorder::default();
    let mut run = ThreadControl::new(&local);
    let command = backend.push(&sync, Action::Stop);

    // On, but the agent cannot act yet (`nexus codex` with no app-server, or `nexus claude`).
    phone_control(&s, &backend, true);
    s.workbench.control_thread_once(&mut run, &agent).await;
    assert_eq!(backend.last_lease(), Some((sync.clone(), false)));
    assert!(!run.enabled());
    assert_eq!(backend.reads.load(Ordering::SeqCst), 0, "no command is read");

    // The agent can act, but Phone control is off: off by default, so a person turns it on.
    agent.ready.store(true, Ordering::SeqCst);
    phone_control(&s, &backend, false);
    s.workbench.control_thread_once(&mut run, &agent).await;
    assert_eq!(backend.reads.load(Ordering::SeqCst), 0);
    assert_eq!(backend.last_lease(), Some((sync.clone(), false)));

    phone_control(&s, &backend, true);
    s.workbench.control_thread_once(&mut run, &agent).await;
    assert_eq!(backend.last_lease(), Some((sync.clone(), true)));
    assert!(run.enabled());
    assert_eq!(backend.result(&command), Some(true));
    let leases = backend.leases.lock().expect("leases").len();
    s.workbench.control_thread_once(&mut run, &agent).await;
    assert_eq!(
        backend.leases.lock().expect("leases").len(),
        leases,
        "an unchanged lease is renewed every 15 seconds, not every pass"
    );

    // The terminal closes: its thread reads offline at once.
    s.workbench.control_thread_end(&mut run).await;
    assert_eq!(backend.last_lease(), Some((sync, false)));
    assert!(!run.enabled());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

#[tokio::test]
async fn a_live_event_is_logged_and_keeps_the_transcript_s_mark() {
    let s = setup("cli-control-live", bounded());
    let backend = Inbox::new();
    *s.host.sync.lock().expect("sync") = Some(backend.clone());
    let (local, _) = cli_thread(&s, &backend, "conversation").await;
    let notice = WbEvent::Notice {
        text: "Your phone sent a message.".into(),
    };
    s.workbench
        .cli_events(&local, &[(nexus_workbench::now_ms(), notice)])
        .expect("live event");
    let events = s.workbench.events(&local, 0).expect("events");
    assert!(matches!(&events.last().expect("one").event, WbEvent::Notice { text } if text.starts_with("Your phone")));
    let cwd = s.root.join("repo");
    let again = s
        .workbench
        .cli_thread(&CliRun {
            agent: Agent::Codex,
            native_id: "conversation",
            cwd: &cwd,
            model: None,
            via: Via::Nexus,
        })
        .expect("thread")
        .expect("bound");
    assert_eq!(again.read_to, 1, "the transcript is read from where it was");
    assert!(s.workbench.cli_events("t_gone", &[]).is_err());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

#[tokio::test]
async fn a_thread_gateway_does_not_have_yet_gets_no_lease() {
    let s = setup("cli-control-unconfirmed", bounded());
    let backend = Inbox::new();
    *s.host.sync.lock().expect("sync") = Some(backend.clone());
    phone_control(&s, &backend, true);
    let cwd = s.root.join("repo");
    let thread = s
        .workbench
        .cli_thread(&CliRun {
            agent: Agent::Codex,
            native_id: "new",
            cwd: &cwd,
            model: None,
            via: Via::Nexus,
        })
        .expect("thread")
        .expect("bound");
    let mut run = ThreadControl::new(&thread.id);
    s.workbench.control_thread_once(&mut run, &Recorder::ready()).await;
    assert!(backend.leases.lock().expect("leases").is_empty());
    s.workbench.control_thread_end(&mut run).await;
    assert!(backend.leases.lock().expect("leases").is_empty());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}
