//! Fault-injected runner tests. This stand-in does not establish live Gateway/device integration.
use super::*;
use crate::event::{PermissionMode, SessionAgent, ThreadStatus, ThreadSummary, Via, WbEvent};
use crate::store::{Store, ThreadRecord};
use crate::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Default)]
enum Fault {
    #[default]
    None,
    Before,
    After,
    Partial,
    Ack(u64),
    Hang,
    /// Gateway's 409 `key_version_behind`: nothing stored.
    Rotated,
}
#[derive(Default)]
struct Server {
    logs: HashMap<String, Vec<Event>>,
    attempts: Vec<Batch>,
    fault: Fault,
    head_override: Option<Option<u64>>,
    deleted: Vec<String>,
}
struct FakeBackend {
    identity: Identity,
    allowed: AtomicBool,
    controls: AtomicBool,
    policy: Mutex<ContentPolicy>,
    policy_failure: AtomicBool,
    key_failure: AtomicBool,
    /// The key step refuses as an unconfirmed approver does.
    approver_unconfirmed: AtomicBool,
    epoch: AtomicU64,
    delay_ms: AtomicU64,
    calls: AtomicUsize,
    key_calls: AtomicUsize,
    server: Mutex<Server>,
    entered: tokio::sync::Notify,
    key_gate: AtomicBool,
    key_entered: tokio::sync::Notify,
    key_resume: tokio::sync::Notify,
    /// Phone control: inbox reads, and the control loop's wake-up while its live stream is open.
    inbox_reads: AtomicUsize,
    watch_wake: Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// The device list names Gateway's trust.
    gateway_trust: AtomicBool,
}
impl FakeBackend {
    fn new(user: &str) -> Arc<Self> {
        Arc::new(Self {
            identity: Identity {
                scope: "https://gateway.example|org".into(),
                user: user.into(),
            },
            allowed: AtomicBool::new(true),
            controls: AtomicBool::new(false),
            policy: Mutex::new(ContentPolicy::UserOnly),
            policy_failure: AtomicBool::new(false),
            key_failure: AtomicBool::new(false),
            approver_unconfirmed: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            delay_ms: AtomicU64::new(0),
            calls: AtomicUsize::new(0),
            key_calls: AtomicUsize::new(0),
            server: Mutex::new(Server::default()),
            entered: tokio::sync::Notify::new(),
            key_gate: AtomicBool::new(false),
            key_entered: tokio::sync::Notify::new(),
            key_resume: tokio::sync::Notify::new(),
            inbox_reads: AtomicUsize::new(0),
            watch_wake: Mutex::new(None),
            gateway_trust: AtomicBool::new(false),
        })
    }
    async fn delay(&self) {
        let delay = self.delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
    }
    fn fault(&self, fault: Fault) {
        self.server.lock().expect("server").fault = fault;
    }
    fn attempts(&self) -> Vec<Batch> {
        self.server.lock().expect("server").attempts.clone()
    }
}
impl Backend for FakeBackend {
    fn supports_control(&self) -> bool {
        self.controls.load(Ordering::SeqCst)
    }
    fn device_trust(&self) -> DeviceTrust {
        if self.gateway_trust.load(Ordering::SeqCst) {
            DeviceTrust::Gateway
        } else {
            DeviceTrust::Approvals
        }
    }
    fn control_commands(&self) -> BoxFuture<'_, Result<Vec<super::control::Command>>> {
        self.inbox_reads.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Vec::new()) })
    }
    fn control_watch(&self, wake: Arc<tokio::sync::Notify>) -> BoxFuture<'_, Result<()>> {
        *self.watch_wake.lock().expect("wake") = Some(wake);
        Box::pin(futures::future::pending())
    }
    fn identity(&self) -> &Identity {
        &self.identity
    }
    fn allow_local_cache(&self) -> bool {
        self.allowed.load(Ordering::SeqCst)
    }
    fn policy(&self) -> BoxFuture<'_, Result<Policy>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            self.delay().await;
            if self.policy_failure.load(Ordering::SeqCst) {
                return Err("policy unavailable".into());
            }
            Ok(Policy {
                content: *self.policy.lock().expect("policy"),
            })
        })
    }
    fn head(&self, thread: &str) -> BoxFuture<'_, Result<Option<u64>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let server = self.server.lock().expect("server");
        let through = server
            .head_override
            .unwrap_or_else(|| server.logs.get(thread).and_then(|log| log.last().map(|e| e.seq)));
        Box::pin(async move {
            self.delay().await;
            Ok(through)
        })
    }
    fn key(&self, _: &str, _: Option<u64>, _: ContentPolicy) -> BoxFuture<'_, Result<Arc<dyn ContentKey>>> {
        self.key_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            self.delay().await;
            if self.key_failure.load(Ordering::SeqCst) {
                return Err("untrusted holder".into());
            }
            if self.approver_unconfirmed.load(Ordering::SeqCst) {
                return Err(super::trust::APPROVER_UNCONFIRMED.into());
            }
            if self.key_gate.swap(false, Ordering::SeqCst) {
                self.key_entered.notify_one();
                self.key_resume.notified().await;
            }
            Ok(Arc::new(TestKey(self.epoch.load(Ordering::SeqCst))) as Arc<dyn ContentKey>)
        })
    }
    fn append(&self, batch: Batch) -> BoxFuture<'_, Result<u64>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            self.delay().await;
            let fault = {
                let mut server = self.server.lock().expect("server");
                server.attempts.push(batch.clone());
                std::mem::take(&mut server.fault)
            };
            self.entered.notify_one();
            if matches!(fault, Fault::Hang) {
                futures::future::pending::<()>().await;
            }
            if matches!(fault, Fault::Before) {
                return Err("offline".into());
            }
            if matches!(fault, Fault::Rotated) {
                return Err(KEY_ROTATED.into());
            }
            if let Fault::Ack(seq) = fault {
                return Ok(seq);
            }
            let mut server = self.server.lock().expect("server");
            let log = server.logs.entry(batch.thread.clone()).or_default();
            let n = if matches!(fault, Fault::Partial) {
                1
            } else {
                batch.events.len()
            };
            for event in batch.events.into_iter().take(n) {
                if let Some(existing) = log.get(event.seq as usize) {
                    assert_eq!(existing, &event, "retry changed");
                } else {
                    assert_eq!(event.seq as usize, log.len(), "gap");
                    log.push(event);
                }
            }
            let through = log.last().expect("nonempty append").seq;
            if matches!(fault, Fault::After) {
                Err("response lost after commit".into())
            } else {
                Ok(through)
            }
        })
    }
    fn delete(&self, thread: &str) -> BoxFuture<'_, Result<()>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut server = self.server.lock().expect("server");
        if matches!(server.fault, Fault::Before) {
            server.fault = Fault::None;
            return Box::pin(async { Err("offline delete".into()) });
        }
        server.logs.remove(thread);
        server.deleted.push(thread.to_string());
        Box::pin(async { Ok(()) })
    }
}
/// Test-only RustCrypto adapter. Production key storage, HPKE and trust verification are still to do.
struct TestKey(u64);
fn aad(thread: &str, epoch: u64, seq: u64) -> Vec<u8> {
    let mut aad = Vec::new();
    for field in [
        b"nexus-event-v1".as_slice(),
        thread.as_bytes(),
        &epoch.to_be_bytes(),
        &seq.to_be_bytes(),
    ] {
        aad.extend_from_slice(&(field.len() as u16).to_be_bytes());
        aad.extend_from_slice(field);
    }
    aad
}
impl ContentKey for TestKey {
    fn epoch(&self) -> u64 {
        self.0
    }
    fn seal(&self, thread: &str, seq: u64, plaintext: &[u8]) -> Result<Vec<u8>> {
        let cipher = Aes256Gcm::new(&[(self.0 + 1) as u8; 32].into());
        let random = uuid::Uuid::new_v4();
        let nonce: [u8; 12] = random.as_bytes()[..12].try_into().expect("nonce");
        let sealed = cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad(thread, self.0, seq),
                },
            )
            .map_err(|_| "test encryption".to_string())?;
        Ok([nonce.to_vec(), sealed].concat())
    }
}
fn open(thread: &str, event: &Event) -> Result<Plaintext> {
    let epoch = event.epoch.expect("epoch");
    let sealed = base64::engine::general_purpose::STANDARD
        .decode(event.sealed.as_ref().expect("sealed"))
        .expect("base64");
    let nonce: [u8; 12] = sealed[..12].try_into().expect("nonce");
    let cipher = Aes256Gcm::new(&[(epoch + 1) as u8; 32].into());
    let plaintext = cipher
        .decrypt(
            &Nonce::from(nonce),
            Payload {
                msg: &sealed[12..],
                aad: &aad(thread, epoch, event.seq),
            },
        )
        .map_err(|_| "cannot open".to_string())?;
    serde_json::from_slice(&plaintext).map_err(|e| e.to_string())
}
#[path = "tests/gateway_trust.rs"]
mod gateway_trust;

struct TestHost(Mutex<Option<Arc<FakeBackend>>>);
impl Host for TestHost {
    fn session_sync(&self) -> Option<Arc<dyn Backend>> {
        self.0.lock().expect("backend").clone().map(|b| b as Arc<dyn Backend>)
    }
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, Result<Launch>> {
        Box::pin(async { Err("no process".into()) })
    }
    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
    fn price(&self, _: &str, _: &crate::event::Usage) -> Option<f64> {
        None
    }
    fn top_price(&self, _: &crate::event::Usage) -> Option<f64> {
        Some(0.001)
    }
    fn emit(&self, _: Emit) {}
}
struct Fixture {
    root: PathBuf,
    host: Arc<TestHost>,
    backend: Arc<FakeBackend>,
    wb: Workbench,
}
impl Fixture {
    fn new() -> Self {
        Self::with_memory(false)
    }
    fn with_memory(memory: bool) -> Self {
        let root = std::env::temp_dir().join(format!("nexus-session-sync-{}", uuid::Uuid::new_v4()));
        let backend = FakeBackend::new("user-a");
        let host = Arc::new(TestHost(Mutex::new(Some(backend.clone()))));
        let store = if memory {
            Store::in_memory()
        } else {
            Store::open(&root.join("test.sqlite3"))
        }
        .expect("store");
        let wb = Workbench::new(
            store,
            host.clone(),
            Config {
                scope: backend.identity.scope.clone(),
                shell: crate::process::Shell::sh(),
                ..Config::default()
            },
        )
        .expect("workbench");
        Self {
            root,
            host,
            backend,
            wb,
        }
    }
    fn thread(&self, id: &str) {
        self.wb
            .store()
            .insert_thread(&record(id, &self.backend.identity.scope))
            .expect("thread");
        self.wb.register_session_sync(id, Some(&self.backend.identity));
    }
    fn event(&self, id: &str, text: &str) {
        self.wb
            .store()
            .append(
                id,
                123,
                &WbEvent::User {
                    text: text.into(),
                    images: vec![],
                    from: None,
                },
            )
            .expect("event");
    }
    fn target(&self, id: &str) -> Target {
        self.wb.store().sync_target(id).expect("target").expect("registered")
    }
    fn ready(&self) {
        self.wb
            .inner
            .session_sync
            .try_lock()
            .expect("coordinator")
            .retry
            .clear();
    }
    fn reopen(&self) -> Workbench {
        Workbench::new(
            Store::open(&self.root.join("test.sqlite3")).expect("store"),
            self.host.clone(),
            Config {
                scope: self.backend.identity.scope.clone(),
                shell: crate::process::Shell::sh(),
                ..Config::default()
            },
        )
        .expect("restart")
    }
}
#[test]
fn removed_profile_does_not_hide_other_agents_in_discovery() {
    let fixture = Fixture::new();
    let root = fixture.root.join("accounts");
    let claude = root.join("claude/a_1");
    std::fs::create_dir_all(&claude).expect("Claude profile");
    let codex = root.join("codex/a_1");
    std::fs::create_dir_all(&codex).expect("Codex profile");
    fixture.wb.inner.config.lock().expect("config").accounts = Some(root);
    fixture.wb.use_account_of(SessionAgent::Claude, Some("a_1".into()));
    fixture.wb.use_account_of(SessionAgent::Codex, Some("a_1".into()));
    std::fs::remove_dir(&codex).expect("remove only the Codex profile");
    assert_eq!(
        fixture
            .wb
            .available_choice_profiles(&[SessionAgent::Claude, SessionAgent::Codex, SessionAgent::Nexus]),
        [(SessionAgent::Claude, claude)].into(),
    );
    assert_eq!(fixture.wb.active_choice_profile(SessionAgent::Codex), None);
    assert!(
        fixture
            .wb
            .choice_profile(SessionAgent::Codex, Some("a_1".into()))
            .is_err()
    );
}

#[tokio::test]
async fn first_phone_subscription_start_reuses_only_matching_admission() {
    use crate::AdmittedExecution;
    use crate::computer::{BillingPayer, Selection};

    let fixture = Fixture::new();
    let wb = &fixture.wb;
    let project = wb.store().add_project(&fixture.root, 1).expect("project");
    let selection = Selection {
        via: Via::Subscription,
        payer: BillingPayer::Plan,
        model: Some("selected-model".into()),
        effort: None,
    };
    let discovery_error = "Execution settings are not available on this computer.";
    for (id, admitted, expected) in [
        ("same-profile", Some(None), "no process"),
        (
            "changed-profile",
            Some(Some(fixture.root.join("old-account"))),
            discovery_error,
        ),
        ("without-admission", None, discovery_error),
    ] {
        let mut thread = record(id, &fixture.backend.identity.scope);
        thread.summary.project_id = project.id.clone();
        thread.summary.model = selection.model.clone();
        wb.store().insert_thread(&thread).expect("thread");
        wb.store()
            .computer_require_payer(id, &thread.scope, selection.payer)
            .expect("payer");
        let admitted = admitted.map(|profile| AdmittedExecution {
            agent: SessionAgent::Codex,
            profile,
            selection: selection.clone(),
        });
        wb.send_creation_message(id, "Start", vec![], "", admitted.as_ref())
            .await
            .expect("send");
        let events = wb.reader().events_after(id, 0).expect("events");
        assert!(
            events
                .iter()
                .any(|event| { matches!(&event.event, WbEvent::Error { text, .. } if text == expected) }),
            "{id}: {events:?}"
        );
    }
}

#[test]
fn failed_and_rewound_startup_does_not_count_as_a_spawn() {
    let fixture = Fixture::new();
    let thread = "profile-check";
    fixture.thread(thread);
    fixture
        .wb
        .store()
        .append(
            thread,
            1,
            &WbEvent::TurnEnd {
                turn_id: "first".into(),
                status: crate::event::TurnStatus::Error,
                cost_usd: None,
                duration_ms: 0,
                error: Some("Model unavailable on this profile".into()),
                cursor: None,
            },
        )
        .expect("failed launch");
    assert!(!fixture.wb.reader().has_spawned_agent(thread).expect("history"));

    fixture
        .wb
        .store()
        .append(
            thread,
            2,
            &WbEvent::Rewind {
                before: 1,
                cursor: None,
            },
        )
        .expect("native reset");
    assert!(!fixture.wb.reader().has_spawned_agent(thread).expect("history"));
    fixture
        .wb
        .store()
        .append(
            thread,
            3,
            &WbEvent::Session {
                nexus_session_id: None,
                native_id: None,
                model: None,
                via: crate::event::Via::Subscription,
                resumed: false,
            },
        )
        .expect("successful spawn");
    assert!(fixture.wb.reader().has_spawned_agent(thread).expect("history"));
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
pub(super) fn record(id: &str, scope: &str) -> ThreadRecord {
    ThreadRecord {
        summary: ThreadSummary {
            name: None,
            id: id.into(),
            project_id: "project".into(),
            title: "secret title".into(),
            agent: SessionAgent::Codex,
            model: None,
            permission_mode: PermissionMode::Default,
            effort: None,
            fast: false,
            via: Via::Subscription,
            provider: None,
            account: None,
            status: ThreadStatus::Idle,
            cost_usd: 0.0,
            nexus_sessions: vec![],
            context_tokens: None,
            context_window: None,
            created_at: 1,
            updated_at: 1,
            listed_at: Some(1),
            acted_at: Some(1),
            archived: false,
            pinned: false,
            worktree: None,
            pull_requests: Vec::new(),
            pending: None,
            capabilities: None,
            parent_thread_id: None,
            source_thread_id: None,
            side_chat: false,
            spawned_by: None,
            group_id: None,
            depth: 0,
            cross_project: false,
            orchestration: false,
            waits_on: vec![],
            pending_results: 0,
            start_choices: vec![],
            halt: None,
            crew_paused: false,
            crew_tripped: false,
            crew_spend_trip: None,
            in_background: false,
            background: vec![],
            background_total: 0,
            at_depth_limit: false,
            handoff_from: None,
            limited: None,
            aside: Default::default(),
        },
        native_id: uuid::Uuid::new_v4().to_string(),
        native_started: false,
        scope: scope.into(),
        fork_from: None,
        seed: None,
        automatic_title: false,
        delivered_seq: 0,
        halt: None,
    }
}
#[tokio::test]
async fn encrypted_copy_opens_on_a_separate_test_reader_without_plaintext_leaks() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "secret prompt");
    let report = f.wb.sync_sessions_once().await;
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.confirmed_events, 1);
    let batches = f.backend.attempts();
    let batch = &batches[0];
    assert_eq!(uuid::Uuid::parse_str(&batch.thread).expect("UUID").get_version_num(), 7);
    assert_eq!(batch.events[0].seq, 0);
    let wire = serde_json::to_string(batch).expect("wire");
    for secret in ["secret prompt", "secret title", "native", "project", "user-a"] {
        assert!(!wire.contains(secret), "{secret}");
    }
    let opened = open(&batch.thread, &batch.events[0]).expect("separate reader");
    assert!(opened.event_json.contains("secret prompt"));
    assert_eq!(opened.thread.title, "secret title");
    assert!(open(&uuid::Uuid::now_v7().to_string(), &batch.events[0]).is_err());
    let mut wrong = batch.events[0].clone();
    wrong.seq += 1;
    assert!(open(&batch.thread, &wrong).is_err());
    assert!(f.wb.sync_sessions_once().await.failures.is_empty());
    assert_eq!(f.backend.attempts().len(), 1);
}
#[tokio::test]
async fn offline_retry_reuses_durable_ciphertext_across_restart() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "one");
    f.event("a", "two");
    f.backend.fault(Fault::Before);
    assert_eq!(f.wb.sync_sessions_once().await.failures.len(), 1);
    assert_eq!(f.target("a").confirmed, 0);
    let reopened = f.reopen();
    assert_eq!(reopened.sync_sessions_once().await.confirmed_events, 2);
    let attempts = f.backend.attempts();
    assert_eq!(attempts[0], attempts[1]);
    assert!(f.wb.store().sync_pending("a").expect("outbox").is_none());
}
#[tokio::test]
async fn committed_upload_with_lost_ack_is_reconciled_before_new_epoch() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "old");
    f.backend.fault(Fault::After);
    assert_eq!(f.wb.sync_sessions_once().await.failures.len(), 1);
    f.backend.epoch.store(1, Ordering::SeqCst);
    f.event("a", "after revoke");
    let reopened = f.reopen();
    assert_eq!(reopened.sync_sessions_once().await.confirmed_events, 2);
    let attempts = f.backend.attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].events[0].epoch, Some(0));
    assert_eq!(attempts[1].events[0].epoch, Some(1));
    assert_eq!(attempts[1].events[0].seq, 1);
}
#[tokio::test]
async fn partial_ack_keeps_the_exact_remaining_ciphertext() {
    let f = Fixture::new();
    f.thread("a");
    for text in ["one", "two", "three"] {
        f.event("a", text);
    }
    f.backend.fault(Fault::Partial);
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 1);
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 2);
    let attempts = f.backend.attempts();
    assert_eq!(attempts[1].events, attempts[0].events[1..]);
}
#[tokio::test]
async fn invalid_ack_does_not_skip_events() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "keep me");
    f.backend.fault(Fault::Ack(99));
    assert_eq!(
        f.wb.sync_sessions_once().await.failures[0].1,
        FailureKind::InvalidAcknowledgement
    );
    assert_eq!(f.target("a").confirmed, 0);
    assert!(f.wb.store().sync_pending("a").expect("outbox").is_some());
    f.ready();
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 1);
}
#[tokio::test]
async fn server_cannot_advance_beyond_or_regress_the_confirmed_log() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "one");
    f.backend.server.lock().expect("server").head_override = Some(Some(u64::MAX));
    assert_eq!(
        f.wb.sync_sessions_once().await.failures[0].1,
        FailureKind::InvalidAcknowledgement
    );
    assert!(f.backend.attempts().is_empty());
    f.backend.server.lock().expect("server").head_override = None;
    f.ready();
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 1);
    f.event("a", "two");
    f.backend.server.lock().expect("server").head_override = Some(None);
    assert_eq!(
        f.wb.sync_sessions_once().await.failures[0].1,
        FailureKind::InvalidAcknowledgement
    );
    assert_eq!(f.target("a").confirmed, 1);
}
#[tokio::test]
async fn no_cache_and_in_memory_stores_make_no_sync_calls() {
    for memory in [false, true] {
        let f = Fixture::with_memory(memory);
        f.thread("a");
        f.event("a", "private");
        if !memory {
            f.backend.allowed.store(false, Ordering::SeqCst);
        }
        assert!(f.wb.sync_sessions_once().await.failures.is_empty());
        assert_eq!(f.backend.calls.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn never_leaves_sends_only_allowlisted_observations_and_no_keys() {
    let f = Fixture::new();
    *f.backend.policy.lock().expect("policy") = ContentPolicy::NeverLeaves;
    f.thread("a");
    f.event("a", "private");
    f.wb.store()
        .append(
            "a",
            125,
            &WbEvent::Usage {
                model: "test-model".into(),
                input_tokens: 2,
                output_tokens: 3,
                cache_read_tokens: 4,
                cache_write_tokens: 5,
                cost_usd: 0.0000001,
                parent: Some("secret-parent".into()),
            },
        )
        .expect("usage");
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 2);
    let batch = &f.backend.attempts()[0];
    assert!(batch.events.iter().all(|e| e.sealed.is_none() && e.epoch.is_none()));
    assert_eq!(batch.events[1].usage.as_ref().expect("usage").cost_usd, "0.0000001");
    let wire = serde_json::to_string(batch).expect("wire");
    for secret in ["private", "secret", "parent", "title"] {
        assert!(!wire.contains(secret));
    }
    assert_eq!(f.backend.key_calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn policy_change_blocks_uncertain_content_instead_of_replacing_it() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "private");
    f.backend.fault(Fault::Before);
    f.wb.sync_sessions_once().await;
    let original = f.wb.store().sync_pending("a").expect("outbox");
    *f.backend.policy.lock().expect("policy") = ContentPolicy::NeverLeaves;
    f.ready();
    assert_eq!(
        f.wb.sync_sessions_once().await.failures[0].1,
        FailureKind::PolicyChanged
    );
    assert_eq!(f.backend.attempts().len(), 1);
    assert_eq!(f.wb.store().sync_pending("a").expect("outbox"), original);
}
#[tokio::test]
async fn identity_switch_and_legacy_logs_cannot_adopt_another_users_content() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "belongs to a");
    let other = FakeBackend::new("user-b");
    *f.host.0.lock().expect("host") = Some(other.clone());
    assert!(f.wb.sync_sessions_once().await.failures.is_empty());
    assert!(other.attempts().is_empty());
    f.wb.store()
        .insert_thread(&record("legacy", &other.identity.scope))
        .expect("legacy");
    f.event("legacy", "old private log");
    f.wb.register_session_sync("legacy", Some(&other.identity));
    assert!(f.wb.store().sync_target("legacy").expect("target").is_none());
    *f.host.0.lock().expect("host") = Some(f.backend.clone());
    f.wb.set_scope("another-gateway|org");
    assert!(f.wb.sync_sessions_once().await.failures.is_empty());
    assert!(f.backend.attempts().is_empty());
}
#[tokio::test]
async fn account_switch_during_key_preparation_prevents_the_upload() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "private");
    f.backend.key_gate.store(true, Ordering::SeqCst);
    let wb = f.wb.clone();
    let task = tokio::spawn(async move { wb.sync_sessions_once().await });
    f.backend.key_entered.notified().await;
    *f.host.0.lock().expect("host") = Some(FakeBackend::new("user-b"));
    f.backend.key_resume.notify_one();
    assert_eq!(task.await.expect("pass").failures[0].1, FailureKind::IdentityChanged);
    assert!(f.backend.attempts().is_empty());
}
#[tokio::test]
async fn unknown_event_fields_survive_byte_for_byte() {
    let f = Fixture::new();
    f.thread("a");
    let raw = r#"{ "kind":"futureEvent", "unknown": {"secret":"exact"}, "n":1.00 }"#;
    rusqlite::Connection::open(f.root.join("test.sqlite3"))
        .expect("fixture connection")
        .execute("insert into events (thread_id,seq,at,body) values ('a',1,42,?1)", [raw])
        .expect("raw fixture");
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 1);
    let batch = &f.backend.attempts()[0];
    assert_eq!(open(&batch.thread, &batch.events[0]).expect("open").event_json, raw);
}
#[tokio::test]
async fn oversized_event_blocks_only_its_thread_without_skipping_it() {
    let f = Fixture::new();
    f.thread("a");
    f.thread("b");
    f.event("a", &"x".repeat(MAX_PLAINTEXT + 1));
    f.event("b", "small");
    let report = f.wb.sync_sessions_once().await;
    assert_eq!(report.confirmed_events, 1);
    assert_eq!(report.failures, vec![("a".into(), FailureKind::TooLarge)]);
    assert_eq!(f.target("a").confirmed, 0);
    assert_eq!(f.target("b").confirmed, 1);
}
#[tokio::test]
async fn batches_are_bounded_by_count_and_bytes() {
    let f = Fixture::new();
    f.thread("a");
    for _ in 0..MAX_EVENTS + 3 {
        f.event("a", "one");
    }
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, MAX_EVENTS);
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 3);
    for batch in f.backend.attempts() {
        assert!(batch.events.len() <= MAX_EVENTS);
        assert!(serde_json::to_vec(&batch).expect("bytes").len() <= MAX_BATCH_BYTES);
    }
}
#[tokio::test]
async fn deletion_retries_durably_without_resurrecting_the_outbox() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "delete me");
    f.backend.fault(Fault::After);
    f.wb.sync_sessions_once().await;
    let sync_id = f.target("a").sync_id;
    f.wb.store().delete_thread("a").expect("delete");
    assert!(f.wb.store().sync_pending("a").expect("outbox").is_none());
    f.backend.fault(Fault::Before);
    let reopened = f.reopen();
    assert_eq!(reopened.sync_sessions_once().await.failures.len(), 1);
    let reopened = f.reopen();
    assert_eq!(reopened.sync_sessions_once().await.deleted_threads, 1);
    assert!(reopened.store().sync_target("a").expect("target").is_none());
    let server = f.backend.server.lock().expect("server");
    assert!(!server.logs.contains_key(&sync_id));
    assert_eq!(server.attempts.len(), 1);
}
/// A command announced live wakes the phone control loop at once, well before its 2 s poll.
#[tokio::test(start_paused = true)]
async fn a_live_command_wakes_the_control_loop_before_its_poll() {
    let f = Fixture::new();
    f.backend.controls.store(true, Ordering::SeqCst);
    f.wb.set_code_control(true).expect("phone control on");
    let wb = f.wb.clone();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(async move { wb.run_session_sync(stopped).await });
    // Yielding keeps the paused clock still, so the poll's tick cannot be what reads the inbox again.
    async fn until(done: impl Fn() -> bool) {
        for _ in 0..100_000 {
            if done() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("the control loop did not run");
    }
    until(|| f.backend.watch_wake.lock().expect("wake").is_some() && f.backend.inbox_reads.load(Ordering::SeqCst) > 0)
        .await;
    let wake = f
        .backend
        .watch_wake
        .lock()
        .expect("wake")
        .clone()
        .expect("the live stream is open");
    let before = f.backend.inbox_reads.load(Ordering::SeqCst);
    let started = tokio::time::Instant::now();
    wake.notify_one();
    until(|| f.backend.inbox_reads.load(Ordering::SeqCst) > before).await;
    assert!(started.elapsed() < CONTROL_INTERVAL, "{:?}", started.elapsed());
    stop.send(true).expect("stop");
    worker.await.expect("worker stopped");
}
#[tokio::test]
async fn shutdown_cancels_a_hung_upload_and_keeps_its_durable_retry() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "keep me");
    f.backend.fault(Fault::Hang);
    let wb = f.wb.clone();
    let worker = tokio::spawn(async move { wb.run_session_sync_until_shutdown().await });
    f.backend.entered.notified().await;
    f.wb.shutdown().await;
    tokio::time::timeout(Duration::from_secs(1), worker)
        .await
        .expect("worker stopped")
        .expect("worker");
    assert!(f.wb.store().sync_pending("a").expect("outbox").is_some());
    let reopened = f.reopen();
    assert_eq!(reopened.sync_sessions_once().await.confirmed_events, 1);
}
#[test]
fn decimal_strings_preserve_exponents_without_rounding() {
    for (source, expanded) in [
        ("1e-7", "0.0000001"),
        ("1.5e-7", "0.00000015"),
        ("1.23e2", "123"),
        ("1.23e3", "1230"),
        ("0.001", "0.001"),
    ] {
        assert_eq!(decimal(source), Ok(expanded.into()));
    }
}

#[tokio::test]
async fn tombstone_deletion_does_not_depend_on_content_policy() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "delete me");
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 1);
    f.wb.store().delete_thread("a").expect("delete");
    f.backend.policy_failure.store(true, Ordering::SeqCst);
    let report = f.wb.sync_sessions_once().await;
    assert!(report.failures.is_empty());
    assert_eq!(report.deleted_threads, 1);
    assert!(f.wb.store().sync_target("a").expect("target").is_none());
}

#[tokio::test]
async fn failed_policy_and_untrusted_keys_leave_content_local() {
    for policy_failure in [true, false] {
        let f = Fixture::new();
        f.thread("a");
        f.event("a", "private");
        if policy_failure {
            f.backend.policy_failure.store(true, Ordering::SeqCst);
        } else {
            f.backend.key_failure.store(true, Ordering::SeqCst);
        }
        assert_eq!(f.wb.sync_sessions_once().await.failures[0].1, FailureKind::Unavailable);
        assert!(f.backend.attempts().is_empty());
        assert_eq!(f.target("a").confirmed, 0);
        assert!(f.wb.store().sync_pending("a").expect("outbox").is_none());
    }
}

#[tokio::test]
async fn epoch_rollback_is_rejected_before_staging_content() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "epoch one");
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 1);
    f.event("a", "epoch one");
    f.backend.epoch.store(1, Ordering::SeqCst);
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 1);
    f.event("a", "must not use zero");
    f.backend.epoch.store(0, Ordering::SeqCst);
    assert_eq!(f.wb.sync_sessions_once().await.failures[0].1, FailureKind::InvalidEpoch);
    assert_eq!(f.target("a").confirmed, 2);
    assert_eq!(f.backend.attempts().len(), 2);
}

#[tokio::test]
async fn policy_change_during_key_preparation_sends_no_content() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "private");
    f.backend.key_gate.store(true, Ordering::SeqCst);
    let wb = f.wb.clone();
    let task = tokio::spawn(async move { wb.sync_sessions_once().await });
    f.backend.key_entered.notified().await;
    *f.backend.policy.lock().expect("policy") = ContentPolicy::NeverLeaves;
    f.backend.key_resume.notify_one();
    assert_eq!(task.await.expect("pass").failures[0].1, FailureKind::PolicyChanged);
    assert!(f.backend.attempts().is_empty());
    assert_eq!(f.target("a").confirmed, 0);
}

#[tokio::test(start_paused = true)]
async fn request_deadline_preserves_the_retry_and_backoff_prevents_a_tight_loop() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "private");
    f.backend.fault(Fault::Hang);
    let started = tokio::time::Instant::now();
    let report = f.wb.sync_sessions_once().await;
    assert_eq!(report.failures[0].1, FailureKind::Unavailable);
    assert!(started.elapsed() >= REQUEST_TIMEOUT);
    assert!(started.elapsed() < Duration::from_secs(7));
    assert!(f.wb.store().sync_pending("a").expect("outbox").is_some());
    f.wb.sync_sessions_once().await;
    assert_eq!(f.backend.attempts().len(), 1);
    f.ready();
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 1);
}

#[tokio::test]
async fn a_thread_that_keeps_failing_waits_up_to_five_minutes_and_a_success_resets_it() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "private");
    f.backend.key_failure.store(true, Ordering::SeqCst);
    let id = f.target("a").sync_id;
    // Twelve failures already: without the cap the next wait would be 250 ms * 4096, about 17 minutes.
    f.wb.inner
        .session_sync
        .try_lock()
        .expect("coordinator")
        .retry
        .insert(id.clone(), (12, Instant::now()));
    assert_eq!(f.wb.sync_sessions_once().await.failures[0].1, FailureKind::Unavailable);
    let (failures, due) = f.wb.inner.session_sync.try_lock().expect("coordinator").retry[&id];
    assert_eq!(failures, 13);
    let wait = due.saturating_duration_since(Instant::now());
    assert!(wait > Duration::from_secs(250) && wait <= MAX_RETRY, "waits {wait:?}");
    f.backend.key_failure.store(false, Ordering::SeqCst);
    // Due now, with its failure count kept, so only a success can empty the table.
    f.wb.inner
        .session_sync
        .try_lock()
        .expect("coordinator")
        .retry
        .insert(id.clone(), (13, Instant::now()));
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 1);
    assert!(
        f.wb.inner
            .session_sync
            .try_lock()
            .expect("coordinator")
            .retry
            .is_empty()
    );
}

/// While this device's approver is unconfirmed, one key try stands for every thread for a minute, the pass
/// says why, and the first pass after the person confirms syncs everything.
#[tokio::test]
async fn an_unconfirmed_approver_holds_every_thread_for_a_minute_and_says_why() {
    let f = Fixture::new();
    for name in ["a", "b"] {
        f.thread(name);
        f.event(name, "private");
    }
    f.backend.approver_unconfirmed.store(true, Ordering::SeqCst);
    let first = f.wb.sync_sessions_once().await;
    assert_eq!(first.failures.len(), 2, "{:?}", first.failures);
    assert!(
        first
            .failures
            .iter()
            .all(|(_, kind)| *kind == FailureKind::ApproverUnconfirmed)
    );
    assert!(f.wb.session_sync_waits_for_approver().await);
    let tries = f.backend.key_calls.load(Ordering::SeqCst);

    f.ready();
    let held = f.wb.sync_sessions_once().await;
    assert_eq!(held.failures, vec![(String::new(), FailureKind::ApproverUnconfirmed)]);
    assert_eq!(
        f.backend.key_calls.load(Ordering::SeqCst),
        tries,
        "no key try while it waits"
    );

    // Ten more minutes unconfirmed: each minute's try fails again, and no per-thread backoff builds up behind it.
    for _ in 0..10 {
        f.wb.inner.session_sync.try_lock().expect("coordinator").approver_wait = Some(Instant::now());
        let again = f.wb.sync_sessions_once().await;
        assert_eq!(again.failures.len(), 2, "{:?}", again.failures);
    }
    assert!(
        f.wb.inner
            .session_sync
            .try_lock()
            .expect("coordinator")
            .retry
            .is_empty()
    );

    // The person confirms the approver, and the minute has passed: the next pass syncs both threads.
    f.backend.approver_unconfirmed.store(false, Ordering::SeqCst);
    f.wb.inner.session_sync.try_lock().expect("coordinator").approver_wait = Some(Instant::now());
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 2);
    assert!(!f.wb.session_sync_waits_for_approver().await);
}

#[tokio::test(start_paused = true)]
async fn an_unconfigured_host_keeps_owned_and_new_sessions_local_until_shutdown() {
    let f = Fixture::new();
    f.thread("owned");
    f.event("owned", "pending");
    *f.host.0.lock().expect("host") = None;
    f.thread("local");
    f.event("local", "local only");
    assert!(f.wb.store().sync_target("local").expect("target").is_none());
    let wb = f.wb.clone();
    let worker = tokio::spawn(async move { wb.run_session_sync_until_shutdown().await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    f.wb.delete_thread("local").await.expect("delete local session");
    f.wb.shutdown().await;
    worker.await.expect("worker stopped");
    assert_eq!(f.backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.backend.key_calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.target("owned").confirmed, 0);
    assert_eq!(f.wb.events("owned", 0).expect("pending log").len(), 1);
}

#[tokio::test]
async fn malformed_usage_stays_local_without_publishing_keys_or_skipping_events() {
    let valid = serde_json::json!({
        "kind": "usage", "model": "test/model", "inputTokens": 1, "outputTokens": 2,
        "cacheReadTokens": 0, "cacheWriteTokens": 0, "costUsd": 0.01
    });
    for (field, value) in [
        ("model", Some(serde_json::json!(""))),
        ("costUsd", Some(serde_json::Value::Null)),
        ("costUsd", None),
        ("inputTokens", None),
    ] {
        let f = Fixture::new();
        f.thread("a");
        f.event("a", "replace with malformed usage");
        f.event("a", "must not skip the earlier event");
        let mut usage = valid.clone();
        match value {
            Some(value) => {
                usage[field] = value;
            }
            None => {
                usage.as_object_mut().expect("object").remove(field);
            }
        }
        rusqlite::Connection::open(f.root.join("test.sqlite3"))
            .expect("fixture connection")
            .execute(
                "update events set body = ?1 where thread_id = 'a' and seq = 1",
                [usage.to_string()],
            )
            .expect("malformed fixture");
        let report = f.wb.sync_sessions_once().await;
        assert_eq!(report.failures, vec![("a".into(), FailureKind::InvalidMetadata)]);
        assert_eq!(f.target("a").confirmed, 0);
        assert!(f.wb.store().sync_pending("a").expect("outbox").is_none());
        assert_eq!(f.backend.key_calls.load(Ordering::SeqCst), 0);
        assert!(f.backend.attempts().is_empty());
    }
}

#[tokio::test]
async fn escaped_payload_limit_is_checked_before_publishing_keys_on_every_retry() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", &"\"".repeat(MAX_PLAINTEXT / 3));
    let raw_bytes: i64 = rusqlite::Connection::open(f.root.join("test.sqlite3"))
        .expect("fixture connection")
        .query_row(
            "select length(cast(body as blob)) from events where thread_id = 'a'",
            [],
            |row| row.get(0),
        )
        .expect("raw size");
    assert!(raw_bytes < MAX_PLAINTEXT as i64);
    for _ in 0..2 {
        f.ready();
        let report = f.wb.sync_sessions_once().await;
        assert_eq!(report.failures, vec![("a".into(), FailureKind::TooLarge)]);
        assert_eq!(f.target("a").confirmed, 0);
        assert!(f.wb.store().sync_pending("a").expect("outbox").is_none());
    }
    assert_eq!(f.backend.key_calls.load(Ordering::SeqCst), 0);
    assert!(f.backend.attempts().is_empty());
}

#[tokio::test]
async fn invalid_initial_epochs_never_stage_or_upload_content() {
    for epoch in [1, i64::MAX as u64 + 1, u64::MAX] {
        let f = Fixture::new();
        f.thread("a");
        f.event("a", "private");
        f.backend.epoch.store(epoch, Ordering::SeqCst);
        let report = f.wb.sync_sessions_once().await;
        assert_eq!(report.failures, vec![("a".into(), FailureKind::InvalidEpoch)]);
        assert_eq!(f.target("a").confirmed, 0);
        assert!(f.wb.store().sync_pending("a").expect("outbox").is_none());
        assert!(f.backend.attempts().is_empty());
    }
}

#[tokio::test]
async fn switching_accounts_prunes_retry_state_for_unreachable_threads() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "pending");
    f.backend.fault(Fault::Before);
    assert_eq!(f.wb.sync_sessions_once().await.failures.len(), 1);
    assert_eq!(f.wb.inner.session_sync.try_lock().expect("coordinator").retry.len(), 1);
    let other = FakeBackend::new("other");
    *f.host.0.lock().expect("host") = Some(other.clone());
    assert!(f.wb.sync_sessions_once().await.failures.is_empty());
    assert!(
        f.wb.inner
            .session_sync
            .try_lock()
            .expect("coordinator")
            .retry
            .is_empty()
    );
    assert!(other.attempts().is_empty());
    assert!(f.wb.store().sync_pending("a").expect("original outbox").is_some());
}

/// Session `id` of `agent` in a new repository, bound to sync as creation binds it, with the checkpoint a fork starts
/// from.
fn forkable(f: &Fixture, id: &str, agent: SessionAgent) {
    let repo = f.root.join("repo");
    std::fs::create_dir_all(&repo).expect("repo");
    for args in [
        vec!["init", "--quiet", "--initial-branch=main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@nexus.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "fixture",
        ],
    ] {
        let output = crate::process::command("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .expect("git fixture");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    let project = f.wb.store().add_project(&repo, 1).expect("project");
    let mut source = record(id, &f.backend.identity.scope);
    source.summary.project_id = project.id;
    source.summary.agent = agent;
    f.wb.store().insert_thread(&source).expect("source");
    let identity = f.wb.session_sync_identity(&source.scope);
    f.wb.register_session_sync(id, identity.as_ref());
    f.wb.set_worktree_roots(Some(f.root.join("trees")), None);
    crate::git::checkpoint(&repo, &format!("{}{:010}", crate::checkpoint_prefix(id), 0)).expect("checkpoint");
}

#[tokio::test]
async fn forks_inherit_verified_ownership_only_with_an_installed_adapter() {
    for enabled in [true, false] {
        let f = Fixture::new();
        if !enabled {
            *f.host.0.lock().expect("host") = None;
        }
        forkable(&f, "source", SessionAgent::Codex);
        let fork = tokio::time::timeout(Duration::from_secs(5), f.wb.fork_thread("source", Some(0)))
            .await
            .expect("fork does not deadlock")
            .expect("fork");
        let target = f.wb.store().sync_target(&fork.id).expect("fork target");
        if enabled {
            let target = target.expect("inherited owner");
            assert_eq!(target.identity, f.backend.identity);
            assert_ne!(target.sync_id, f.target("source").sync_id);
            assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 1);
        } else {
            assert!(target.is_none());
            assert!(f.wb.sync_sessions_once().await.failures.is_empty());
            assert_eq!(f.backend.calls.load(Ordering::SeqCst), 0);
        }
        f.wb.set_scope("another organization");
        let error = tokio::time::timeout(Duration::from_secs(1), f.wb.fork_thread("source", Some(0)))
            .await
            .expect("scope check does not deadlock")
            .expect_err("wrong organization");
        assert!(error.contains("not in this organization"));
    }
}

#[tokio::test]
async fn byte_bound_splits_large_batches_without_losing_events() {
    let f = Fixture::new();
    f.thread("a");
    for _ in 0..10 {
        f.event("a", &"x".repeat(60 * 1024));
    }
    let mut count = 0;
    for _ in 0..10 {
        count += f.wb.sync_sessions_once().await.confirmed_events;
    }
    assert_eq!(count, 10);
    let batches = f.backend.attempts();
    assert!(batches.len() > 1);
    assert!(
        batches
            .iter()
            .all(|batch| serde_json::to_vec(batch).expect("bytes").len() <= MAX_BATCH_BYTES)
    );
}

#[tokio::test]
async fn creation_binds_before_the_first_event_and_inheritance_requires_that_owner() {
    let f = Fixture::new();
    f.wb.store()
        .insert_thread(&record("new", &f.backend.identity.scope))
        .expect("thread");
    f.wb.register_session_sync("new", Some(&f.backend.identity));
    f.event("new", "first");
    assert_eq!(
        f.wb.inherited_sync_identity(&f.wb.record("new").expect("record")),
        Some(f.backend.identity.clone())
    );
    *f.host.0.lock().expect("host") = Some(FakeBackend::new("other"));
    assert!(
        f.wb.inherited_sync_identity(&f.wb.record("new").expect("record"))
            .is_none()
    );
    assert!(
        f.wb.inherited_sync_identity(&record("missing", &f.backend.identity.scope))
            .is_none()
    );
}

#[test]
fn only_agents_gateway_stores_sync() {
    // Gateway's `SyncedAgent` and `synced_events.agent`'s check name these three.
    for agent in SessionAgent::ALL {
        assert_eq!(
            syncs(agent),
            matches!(agent.as_str(), "claude" | "codex" | "nexus"),
            "{agent:?}"
        );
    }
}

#[tokio::test]
async fn antigravity_sessions_stay_local_while_others_sync() {
    let f = Fixture::new();
    let agy = |id: &str| {
        let mut record = record(id, &f.backend.identity.scope);
        record.summary.agent = SessionAgent::Antigravity;
        record
    };
    // A new session, and a fork or side chat that inherits an identity.
    f.wb.store().insert_thread(&agy("agy")).expect("thread");
    f.wb.register_session_sync("agy", Some(&f.backend.identity));
    f.wb.store()
        .insert_thread_with_identity(&agy("agy-fork"), std::iter::empty(), Some(&f.backend.identity))
        .expect("fork");
    f.thread("codex");
    for id in ["agy", "agy-fork", "codex"] {
        f.event(id, "hello");
    }
    for id in ["agy", "agy-fork"] {
        assert!(f.wb.store().sync_target(id).expect("target").is_none(), "{id}");
    }
    let codex = f.target("codex").sync_id;
    let report = f.wb.sync_sessions_once().await;
    assert!(report.failures.is_empty());
    assert_eq!(report.confirmed_events, 1);
    let server = f.backend.server.lock().expect("server");
    assert_eq!(server.logs.keys().collect::<Vec<_>>(), [&codex]);
}

#[tokio::test]
async fn claude_code_fork_of_antigravity_session_stays_local() {
    // A known limit. A fork inherits sync only from its source's owner record, which proves who owns the history it
    // copies. An Antigravity session has no such record, so even a Claude Code fork of it stays on this computer.
    // Binding the fork to the signed-in identity instead would upload copied history that no owner record vouches for.
    let f = Fixture::new();
    forkable(&f, "agy", SessionAgent::Antigravity);
    let to_claude = crate::ForkSubscription {
        agent: SessionAgent::Claude,
        account: None,
    };
    let fork = tokio::time::timeout(
        Duration::from_secs(5),
        f.wb.fork_thread_with_subscription("agy", Some(0), Some(to_claude)),
    )
    .await
    .expect("fork does not deadlock")
    .expect("fork");
    assert_eq!(fork.agent, SessionAgent::Claude);
    assert!(f.wb.store().sync_target(&fork.id).expect("fork target").is_none());
    assert!(f.wb.sync_sessions_once().await.failures.is_empty());
    assert_eq!(f.backend.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn opening_the_store_unbinds_antigravity_sessions_bound_before_the_agent_check() {
    let f = Fixture::new();
    // Bound as a Codex session, then saved as Antigravity: the row a store bound before `sync_register` checked agents.
    f.thread("agy");
    f.event("agy", "hello");
    let mut agy = f.wb.record("agy").expect("record");
    agy.summary.agent = SessionAgent::Antigravity;
    f.wb.store().save_thread(&agy).expect("save");
    // Its upload failed, so a batch is staged and nothing is confirmed.
    f.backend.fault(Fault::Before);
    assert_eq!(f.wb.sync_sessions_once().await.failures.len(), 1);
    assert!(f.wb.store().sync_pending("agy").expect("outbox").is_some());
    let old = f.target("agy").sync_id;
    f.thread("codex");
    f.event("codex", "hello");

    let wb = f.reopen();
    assert!(wb.store().sync_target("agy").expect("target").is_none());
    assert!(wb.store().sync_pending("agy").expect("outbox").is_none());
    assert!(wb.store().sync_target("codex").expect("target").is_some());
    assert_eq!(
        wb.record("agy").expect("session stays").summary.agent,
        SessionAgent::Antigravity
    );
    // Gateway is told to delete it, which also drops any key it holds, and its events are never sent again.
    let report = wb.sync_sessions_once().await;
    assert!(report.failures.is_empty());
    assert_eq!(report.deleted_threads, 1);
    assert_eq!(report.confirmed_events, 1);
    let server = f.backend.server.lock().expect("server");
    assert_eq!(server.deleted, [old.as_str()]);
    assert!(!server.logs.contains_key(&old));
}

#[tokio::test(start_paused = true)]
async fn valid_slow_requests_complete_without_an_unreachable_pass_deadline() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "slow but healthy");
    f.backend.delay_ms.store(3000, Ordering::SeqCst);
    let started = tokio::time::Instant::now();
    let report = f.wb.sync_sessions_once().await;
    assert!(report.failures.is_empty());
    assert_eq!(report.confirmed_events, 1);
    assert!(started.elapsed() >= Duration::from_secs(15));
}

#[tokio::test]
async fn recreating_a_local_id_keeps_old_deletion_separate_from_new_uploads() {
    let f = Fixture::new();
    f.thread("same");
    f.event("same", "old");
    assert_eq!(f.wb.sync_sessions_once().await.confirmed_events, 1);
    let old = f.target("same").sync_id;
    f.wb.store().delete_thread("same").expect("delete");
    f.thread("same");
    f.event("same", "replacement");
    let new = f.target("same").sync_id;
    assert_ne!(old, new);
    let report = f.wb.sync_sessions_once().await;
    assert!(report.failures.is_empty());
    assert_eq!(report.confirmed_events, 1);
    assert_eq!(report.deleted_threads, 1);
    assert_eq!(f.target("same").confirmed, 1);
    let server = f.backend.server.lock().expect("server");
    assert!(!server.logs.contains_key(&old));
    assert!(server.logs.contains_key(&new));
}

#[tokio::test]
async fn inheritance_rejects_a_replacement_source_incarnation() {
    let f = Fixture::new();
    f.thread("source");
    f.event("source", "user a content");
    let source = f.wb.record("source").expect("source");
    let captured = f.wb.inherited_sync_identity(&source).expect("owner a");
    f.wb.store().delete_thread("source").expect("delete");
    let other = FakeBackend::new("user-b");
    *f.host.0.lock().expect("host") = Some(other.clone());
    f.wb.store()
        .insert_thread(&record("source", &other.identity.scope))
        .expect("replacement");
    f.wb.register_session_sync("source", Some(&other.identity));
    assert!(f.wb.inherited_sync_identity(&source).is_none());
    f.wb.store()
        .insert_thread(&record("fork", &other.identity.scope))
        .expect("fork");
    f.wb.register_session_sync("fork", Some(&captured));
    assert!(f.wb.store().sync_target("fork").expect("owner").is_none());
}

#[tokio::test]
async fn unrelated_thread_deletion_does_not_wait_for_a_slow_upload() {
    let f = Fixture::new();
    f.thread("a");
    f.thread("b");
    f.thread("unrelated");
    f.event("a", "slow");
    f.event("b", "other");
    f.backend.fault(Fault::Hang);
    let wb = f.wb.clone();
    let pass = tokio::spawn(async move { wb.sync_sessions_once().await });
    f.backend.entered.notified().await;
    tokio::time::timeout(Duration::from_millis(100), f.wb.delete_thread("unrelated"))
        .await
        .expect("independent deletion")
        .expect("deleted");
    pass.abort();
    let _ = pass.await;
}

#[tokio::test]
async fn a_refusal_after_rotation_reseals_the_refused_events_with_the_new_key() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "one");
    f.event("a", "two");
    f.backend.fault(Fault::Rotated);
    let report = f.wb.sync_sessions_once().await;
    assert_eq!(report.failures, vec![("a".to_string(), FailureKind::KeyRotated)]);
    assert!(
        f.wb.store().sync_pending("a").expect("outbox").is_none(),
        "the refused ciphertext is dropped"
    );
    assert_eq!(f.target("a").confirmed, 0);
    f.backend.epoch.store(1, Ordering::SeqCst);
    f.ready();
    let report = f.wb.sync_sessions_once().await;
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.confirmed_events, 2);
    let attempts = f.backend.attempts();
    assert_eq!(attempts.len(), 2);
    assert!(attempts[0].events.iter().all(|event| event.epoch == Some(0)));
    assert!(attempts[1].events.iter().all(|event| event.epoch == Some(1)));
    assert_eq!(attempts[0].events.len(), attempts[1].events.len());
    for event in &attempts[1].events {
        assert!(open(&attempts[1].thread, event).is_ok());
    }
}

#[tokio::test]
async fn an_uncertain_failure_still_retries_the_same_ciphertext() {
    let f = Fixture::new();
    f.thread("a");
    f.event("a", "one");
    f.backend.fault(Fault::Before);
    assert_eq!(
        f.wb.sync_sessions_once().await.failures,
        vec![("a".to_string(), FailureKind::Unavailable)]
    );
    f.backend.epoch.store(1, Ordering::SeqCst);
    f.ready();
    assert!(f.wb.sync_sessions_once().await.failures.is_empty());
    let attempts = f.backend.attempts();
    assert_eq!(attempts[0], attempts[1], "unchanged after an uncertain failure");
}

#[tokio::test]
async fn remote_creation_rechecks_scope_after_the_creation_gate() {
    for change in ["scope", "disabled", "reenabled"] {
        let f = Fixture::new();
        let project = f.wb.add_project(&f.root).expect("existing open project");
        let request_id = uuid::Uuid::new_v4().to_string();
        let local = crate::creation_thread_id(&request_id).expect("local id");
        let gate = f.wb.creation_gate(&local);
        let held = gate.lock().await;
        let new = crate::NewThread {
            name: None,
            project_id: project.id,
            agent: SessionAgent::Codex,
            model: None,
            permission_mode: PermissionMode::Default,
            effort: None,
            via: Via::Nexus,
            provider: None,
            text: "A phone prompt must not outlive its current admission while waiting".into(),
            images: vec![],
            worktree: None,
            parent: None,
            orchestration: false,
        };
        f.backend.controls.store(true, Ordering::SeqCst);
        f.wb.set_code_control(true).expect("enabled by actual setter");
        let generation =
            f.wb.store()
                .computer_info(&f.backend.identity.scope, &f.backend.identity.user)
                .expect("generation")
                .generation;
        let guard = crate::CreationGuard {
            identity: f.backend.identity.clone(),
            expires_at: crate::now_ms() + 60_000,
            generation,
            sender: "phone".into(),
            selection: None,
        };
        let mut blocked =
            Box::pin(f.wb.create_thread_once_mentioning_guarded(new, &request_id, String::new(), Some(guard)));
        assert!(
            futures::poll!(&mut blocked).is_pending(),
            "the real creation gate is held"
        );
        match change {
            "scope" => f.wb.set_scope("https://gateway.example|another-org"),
            _ => {
                f.wb.set_code_control(false).expect("disable by actual setter");
                if change == "reenabled" {
                    f.wb.set_code_control(true).expect("re-enable by actual setter");
                }
            }
        }
        drop(held);
        let outcome = blocked.await;
        assert!(
            !f.wb.has_thread(&local),
            "{change}: a phone request waiting for the gate must create no row in either scope"
        );
        assert!(outcome.is_err());
        assert!(f.wb.store().sync_target(&local).expect("binding").is_none());
        assert!(f.backend.attempts().is_empty());
    }
}
