//! The workbench driving the stand-in `claude` (src/bin/fake-claude.rs) end to end: a turn, an approval, an
//! interrupt, a crash, a resume after the app restarts, and a quit in the middle of a turn.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use nexus_workbench::event::{
    Decision, Effort, Image, LimitStop, Pending, PermissionMode, SessionAgent, StoredEvent, TaskStatus, ThreadStatus,
    ThreadSummary, TurnStatus, Usage, Via, WbEvent,
};
use nexus_workbench::plans::{Limit, PlanUsage};
use nexus_workbench::process::{self, Spawn};
use nexus_workbench::store::Store;
use nexus_workbench::{Config, Emit, Host, Launch, LaunchRequest, NewThread, NewWorktree, ProcessEnd, Workbench};

#[derive(Default)]
struct TestHost {
    emitted: Mutex<Vec<Emit>>,
    launches: Mutex<Vec<LaunchRequest>>,
    ended: Mutex<Vec<(String, ProcessEnd)>>,
    memory: PathBuf,
    /// More environment for the stand-in (its `FAKE_CLAUDE_*` switches).
    extra: Mutex<HashMap<String, String>>,
    /// The Claude plans `plans` answers with.
    plans: Mutex<Vec<PlanUsage>>,
    /// How long `launch` takes (Gateway opening the Nexus session).
    launch_delay: Mutex<Option<Duration>>,
    /// What `launch` fails with, after its delay, when set.
    launch_error: Mutex<Option<String>>,
    /// The model `launch` reports back (as Auto routing does), when set.
    launch_model: Mutex<Option<String>>,
    /// Holds up the next emit of a failed turn's end this long, as a slow screen update would: the workbench is
    /// then in the middle of a run's last writes.
    stall_failed_turn: Mutex<Option<Duration>>,
    /// Holds up the next emit of an allowed approval this long: the person's answer is then half-way through.
    stall_answer: Mutex<Option<Duration>>,
    /// Holds the thread update after an interrupted turn, before queued messages go out.
    interrupted_turn_gate: Mutex<Option<(Arc<EmitGate>, Option<std::thread::ThreadId>)>>,
    /// Holds the thread update right after a run's first turn starts, before its message is sent.
    first_turn_gate: Mutex<Option<(Arc<EmitGate>, Option<std::thread::ThreadId>)>>,
    /// While set, Gateway does not take an end (it is unreachable, say).
    refuse_ends: Mutex<bool>,
}

impl Host for TestHost {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch, String>> {
        let n = {
            let mut launches = self.launches.lock().unwrap_or_else(|p| p.into_inner());
            launches.push(request.clone());
            launches.len()
        };
        let mut env: HashMap<String, String> = [
            (
                "ANTHROPIC_BASE_URL".to_string(),
                "https://gateway.test/v1/anthropic".to_string(),
            ),
            ("FAKE_CLAUDE_DIR".to_string(), self.memory.to_string_lossy().to_string()),
        ]
        .into_iter()
        .collect();
        env.extend(self.extra.lock().unwrap_or_else(|p| p.into_inner()).clone());
        let delay = *self.launch_delay.lock().unwrap_or_else(|p| p.into_inner());
        let error = self.launch_error.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let model = self.launch_model.lock().unwrap_or_else(|p| p.into_inner()).clone();
        Box::pin(async move {
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            if let Some(error) = error {
                return Err(error);
            }
            Ok(Launch {
                session_mcp: Default::default(),
                attribution: None,
                spawn: Spawn {
                    program: PathBuf::from(env!("CARGO_BIN_EXE_fake-claude")),
                    args: request.args,
                    env,
                    cwd: request.cwd,
                },
                nexus_session: Some(format!("ns_{n}")),
                local_run: None,
                local_provider: None,
                model,
            })
        })
    }

    fn ended(&self, nexus_session: String, end: ProcessEnd) -> BoxFuture<'static, bool> {
        self.ended
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((nexus_session, end));
        let taken = !*self.refuse_ends.lock().unwrap_or_else(|p| p.into_inner());
        Box::pin(async move { taken })
    }

    fn price(&self, _model: &str, usage: &Usage) -> Option<f64> {
        Some(usage.output_tokens as f64 * 0.0001)
    }

    fn emit(&self, emit: Emit) {
        let failed_turn = matches!(
            &emit,
            Emit::Event(StoredEvent {
                event: WbEvent::TurnEnd {
                    status: TurnStatus::Error,
                    ..
                },
                ..
            })
        );
        let answer = matches!(
            &emit,
            Emit::Event(StoredEvent {
                event: WbEvent::ApprovalResolved {
                    decision: Decision::Allow,
                    ..
                },
                ..
            })
        );
        let interrupted_turn = matches!(
            &emit,
            Emit::Event(StoredEvent {
                event: WbEvent::TurnEnd {
                    status: TurnStatus::Interrupted,
                    ..
                },
                ..
            })
        );
        let turn_start = matches!(
            &emit,
            Emit::Event(StoredEvent {
                event: WbEvent::TurnStart { .. },
                ..
            })
        );
        let thread_update = matches!(&emit, Emit::Thread(_));
        self.emitted.lock().unwrap_or_else(|p| p.into_inner()).push(emit);
        let first_turn = {
            let mut armed = self.first_turn_gate.lock().unwrap_or_else(|p| p.into_inner());
            if turn_start && let Some((_, started @ None)) = armed.as_mut() {
                *started = Some(std::thread::current().id());
            }
            // `begin_turn` emits TurnStart and its thread update on one thread, with nothing between them.
            if thread_update
                && armed
                    .as_ref()
                    .is_some_and(|(_, started)| *started == Some(std::thread::current().id()))
            {
                armed.take().map(|(gate, _)| gate)
            } else {
                None
            }
        };
        if let Some(gate) = first_turn {
            // Emitted after the store and the run's state are released: a follow-up can arrive while it is held.
            gate.hold();
        }
        let gate = {
            let mut armed = self.interrupted_turn_gate.lock().unwrap_or_else(|p| p.into_inner());
            if interrupted_turn && let Some((_, ended)) = armed.as_mut() {
                *ended = Some(std::thread::current().id());
            }
            // The worker emits TurnEnd and its thread update on one thread, with no await between them.
            // Interrupt emits its update on the main test thread, never on a runtime worker.
            // Hold only the worker's update; holding Interrupt itself prevents the test from releasing the gate.
            if thread_update
                && armed
                    .as_ref()
                    .is_some_and(|(_, ended)| *ended == Some(std::thread::current().id()))
            {
                armed.take().map(|(gate, _)| gate)
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            // Thread updates emit after releasing the store; queue edits and Stop can finish while held.
            gate.hold();
        }
        if answer && let Some(stall) = self.stall_answer.lock().unwrap_or_else(|p| p.into_inner()).take() {
            std::thread::sleep(stall);
        }
        if failed_turn && let Some(stall) = self.stall_failed_turn.lock().unwrap_or_else(|p| p.into_inner()).take() {
            std::thread::sleep(stall);
        }
    }

    fn plans(&self) -> BoxFuture<'static, Vec<PlanUsage>> {
        let plans = self.plans.lock().unwrap_or_else(|p| p.into_inner()).clone();
        Box::pin(async move { plans })
    }
}

struct Setup {
    host: Arc<TestHost>,
    workbench: Workbench,
    project: String,
    db: PathBuf,
    root: PathBuf,
}

fn setup(name: &str) -> Setup {
    let root = std::env::temp_dir().join(format!("nexus-workbench-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("repo");
    let memory = root.join("memory");
    let _ = std::fs::create_dir_all(&repo);
    let _ = std::fs::create_dir_all(&memory);
    let db = root.join("workbench.sqlite3");
    let host = Arc::new(TestHost {
        memory,
        ..TestHost::default()
    });
    let workbench = open(&host, &db);
    let project = workbench.add_project(&repo).map(|p| p.id).unwrap_or_default();
    Setup {
        host,
        workbench,
        project,
        db,
        root,
    }
}

fn open(host: &Arc<TestHost>, db: &std::path::Path) -> Workbench {
    let store = Store::open(db).unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "gw|org".into(),
        interrupt_grace: Duration::from_secs(5),
        shell: nexus_workbench::process::Shell::sh(),
        transcripts: db.parent().map(|dir| dir.join("transcripts")),
        ..Config::default()
    };
    Workbench::new(store, host.clone(), config).unwrap_or_else(|e| panic!("{e}"))
}

fn new_thread(project: &str, text: &str) -> NewThread {
    NewThread {
        name: None,
        project_id: project.into(),
        agent: SessionAgent::Claude,
        model: None,
        permission_mode: PermissionMode::Default,
        effort: None,
        via: Via::Nexus,
        provider: None,
        text: text.into(),
        images: vec![],
        worktree: None,
        parent: None,
        orchestration: false,
    }
}

fn thread(workbench: &Workbench, id: &str) -> ThreadSummary {
    workbench
        .threads()
        .unwrap_or_default()
        .into_iter()
        .find(|t| t.id == id)
        .unwrap_or_else(|| panic!("no thread {id}"))
}

/// Waits (up to ten seconds) until the session's stored events satisfy `done`.
async fn wait_for(workbench: &Workbench, id: &str, done: impl Fn(&[StoredEvent]) -> bool) -> Vec<StoredEvent> {
    for _ in 0..400 {
        let events = workbench.events(id, 0).unwrap_or_default();
        if done(&events) {
            return events;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out; events: {:#?}", workbench.events(id, 0).unwrap_or_default());
}

fn turn_ends(events: &[StoredEvent]) -> Vec<TurnStatus> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::TurnEnd { status, .. } => Some(*status),
            _ => None,
        })
        .collect()
}

fn texts(events: &[StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn names_the_session_from_its_own_first_reply_without_changing_the_log() {
    let s = setup("generated-title");
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_TITLE".into(), "Preserve unsent prompt drafts".into());
    let prompt = "Please investigate why my unsent prompts disappear when I switch sessions";
    let created = s
        .workbench
        .create_thread(new_thread(&s.project, prompt))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &created.id, |events| turn_ends(events).len() == 1).await;
    assert_eq!(thread(&s.workbench, &created.id).title, "Preserve unsent prompt drafts");
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, WbEvent::User { text, .. } if text == prompt))
    );
    assert_eq!(texts(&events), ["Message 1 of this session."]);
    assert!(
        s.host
            .emitted
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .all(|emit| { !matches!(emit, Emit::Delta(delta) if delta.text.contains("nexus-title")) })
    );
    assert_eq!(s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).len(), 1);
    s.workbench.shutdown().await;
    let reopened = open(&s.host, &s.db);
    assert_eq!(thread(&reopened, &created.id).title, "Preserve unsent prompt drafts");
    reopened
        .send(&created.id, "Continue")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&reopened, &created.id, |events| turn_ends(events).len() == 2).await;
    let saved = Store::open(&s.db)
        .unwrap_or_else(|e| panic!("{e}"))
        .thread(&created.id)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("missing thread"));
    let prompts = std::fs::read_to_string(s.host.memory.join(format!("{}.prompts", saved.native_id)))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(prompts.matches("[Nexus desktop navigation metadata]").count(), 1);
    assert_eq!(prompts.lines().last(), Some("Continue"));
    assert_eq!(thread(&reopened, &created.id).title, "Preserve unsent prompt drafts");
    reopened.shutdown().await;
}

#[tokio::test]
async fn an_explicit_same_text_rename_wins_over_an_in_flight_generated_title() {
    let s = setup("manual-title");
    let gate = s.root.join("release-title");
    s.host.extra.lock().unwrap_or_else(|p| p.into_inner()).extend([
        ("FAKE_CLAUDE_TITLE".into(), "Preserve unsent prompt drafts".into()),
        ("FAKE_CLAUDE_TITLE_GATE".into(), gate.to_string_lossy().into_owned()),
    ]);
    let created = s
        .workbench
        .create_thread(new_thread(&s.project, "Keep this title"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .update(
            &created.id,
            nexus_workbench::ThreadPatch {
                title: Some(created.title.clone()),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&gate, "ready").unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &created.id, |events| turn_ends(events).len() == 1).await;
    assert_eq!(thread(&s.workbench, &created.id).title, "Keep this title");
    assert_eq!(texts(&events), ["Message 1 of this session."]);
    s.workbench.shutdown().await;
    let saved = Store::open(&s.db)
        .unwrap_or_else(|e| panic!("{e}"))
        .thread(&created.id)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("missing thread"));
    assert!(!saved.automatic_title);
}

#[tokio::test]
async fn a_pin_is_saved_and_keeps_the_last_active_time() {
    let s = setup("pin");
    let created = s
        .workbench
        .create_thread(new_thread(&s.project, "hi"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |events| turn_ends(events).len() == 1).await;
    let before = thread(&s.workbench, &created.id);
    assert!(!before.pinned);
    let pin = |pinned| nexus_workbench::ThreadPatch {
        pinned: Some(pinned),
        ..Default::default()
    };
    let pinned = s
        .workbench
        .update(&created.id, pin(true))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(pinned.pinned);
    assert_eq!(pinned.updated_at, before.updated_at, "a pin is not activity");
    s.workbench.shutdown().await;
    let reopened = open(&s.host, &s.db);
    let restarted = thread(&reopened, &created.id);
    assert!(restarted.pinned, "the pin survives a restart");
    let unpinned = reopened
        .update(&created.id, pin(false))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!unpinned.pinned);
    assert_eq!(unpinned.updated_at, restarted.updated_at);
    reopened.shutdown().await;
}

fn snooze(until: Option<i64>) -> nexus_workbench::ThreadPatch {
    nexus_workbench::ThreadPatch {
        snoozed_until: Some(until),
        ..Default::default()
    }
}

/// A snooze is stored, holds until its time on any clock, and is not activity.
#[tokio::test]
async fn a_snooze_is_saved_and_comes_back_at_its_time() {
    use nexus_workbench::snooze::snoozed;
    let s = setup("snooze");
    let created = s
        .workbench
        .create_thread(new_thread(&s.project, "hi"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |events| turn_ends(events).len() == 1).await;
    let before = thread(&s.workbench, &created.id);
    let until = before.updated_at + 60 * 60 * 1000;
    let snoozed_now = s
        .workbench
        .update(&created.id, snooze(Some(until)))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(snoozed_now.aside.snoozed_until, Some(until));
    assert_eq!(snoozed_now.updated_at, before.updated_at, "a snooze is not activity");
    s.workbench.shutdown().await;
    let reopened = open(&s.host, &s.db);
    let restarted = thread(&reopened, &created.id);
    assert_eq!(
        restarted.aside.snoozed_until,
        Some(until),
        "the snooze survives a restart"
    );
    // A fake clock: snoozed a minute before its time, back at it.
    assert!(snoozed(&restarted, until - 60_000));
    assert!(!snoozed(&restarted, until));
    assert!(
        reopened
            .wake(&created.id, until)
            .unwrap_or_else(|e| panic!("{e}"))
            .is_none(),
        "nothing to wake once its time passed"
    );
    let woken = reopened
        .update(&created.id, snooze(None))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(woken.aside.snoozed_until, None, "Unsnooze");
    assert_eq!(woken.updated_at, restarted.updated_at);
    reopened.shutdown().await;
}

/// A question or an approval brings a snoozed session back early, and so does its pull request's news.
#[tokio::test]
async fn a_question_or_pull_request_news_wakes_a_snoozed_session_early() {
    let s = setup("snooze-wake");
    let created = s
        .workbench
        .create_thread(new_thread(&s.project, "hi"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |events| turn_ends(events).len() == 1).await;
    let far = nexus_workbench::now_ms() + 24 * 60 * 60 * 1000;
    s.workbench
        .update(&created.id, snooze(Some(far)))
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .send(&created.id, "use a tool")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Approval { .. }))
    })
    .await;
    let asking = thread(&s.workbench, &created.id);
    assert_eq!(asking.status, ThreadStatus::Approval);
    assert_eq!(asking.aside.snoozed_until, None, "it needs the person: awake");
    s.workbench.shutdown().await;

    let reopened = open(&s.host, &s.db);
    reopened
        .update(&created.id, snooze(Some(far)))
        .unwrap_or_else(|e| panic!("{e}"));
    let woken = reopened
        .wake(&created.id, far - 1)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("a snoozed session wakes"));
    assert_eq!(woken.aside.snoozed_until, None);
    reopened.shutdown().await;
}

/// A merged pull request settles an idle session once; "Keep in list" brings it back and opts it out; a
/// busy session never settles, and a new turn brings a settled one back.
#[tokio::test]
async fn a_merge_settles_an_idle_session_and_never_a_busy_or_kept_one() {
    let s = setup("settle");
    let created = s
        .workbench
        .create_thread(new_thread(&s.project, "use a tool"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &created.id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Approval { .. }))
    })
    .await;
    assert!(
        s.workbench
            .settle_merged(&created.id, 7)
            .unwrap_or_else(|e| panic!("{e}"))
            .is_none(),
        "waiting on the person: busy, never settles"
    );
    let Some(request_id) = events.iter().find_map(|e| match &e.event {
        WbEvent::Approval { request_id, .. } => Some(request_id.clone()),
        _ => None,
    }) else {
        panic!("no approval")
    };
    s.workbench
        .approve(&created.id, &request_id, Decision::Allow, None, None, None)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    let idle = thread(&s.workbench, &created.id);
    let settled = s
        .workbench
        .settle_merged_at(&created.id, 7, 5_000)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("an idle session settles on its merge"));
    assert_eq!(settled.aside.settled_at, Some(5_000));
    assert_eq!(settled.aside.settled_by, Some(7));
    assert_eq!(settled.updated_at, idle.updated_at, "settling is not activity");
    assert!(
        s.workbench
            .settle_merged(&created.id, 7)
            .unwrap_or_else(|e| panic!("{e}"))
            .is_none()
    );
    s.workbench.shutdown().await;

    let reopened = open(&s.host, &s.db);
    assert_eq!(
        thread(&reopened, &created.id).aside.settled_at,
        Some(5_000),
        "settled across a restart"
    );
    let kept = reopened
        .update(
            &created.id,
            nexus_workbench::ThreadPatch {
                keep_listed: Some(true),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(kept.aside.settled_at, None, "Keep in list brings it back");
    assert!(
        reopened
            .settle_merged(&created.id, 8)
            .unwrap_or_else(|e| panic!("{e}"))
            .is_none(),
        "kept in the list: no merge settles it"
    );
    let unkept = reopened
        .update(
            &created.id,
            nexus_workbench::ThreadPatch {
                keep_listed: Some(false),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!unkept.aside.keep_listed);
    reopened
        .settle_merged(&created.id, 8)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("a later merge settles it"));
    reopened.send(&created.id, "hi").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&reopened, &created.id, |e| turn_ends(e).len() == 2).await;
    let resumed = thread(&reopened, &created.id);
    assert_eq!(resumed.aside.settled_at, None, "a new turn brings it back");
    assert!(
        reopened
            .settle_merged(&created.id, 8)
            .unwrap_or_else(|e| panic!("{e}"))
            .is_none(),
        "the same merge does not settle it again"
    );
    let archived = reopened
        .archive_merged(&created.id, 9)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("a merge archives it when the person asked"));
    assert!(archived.archived);
    assert_eq!(archived.aside.settled_by, Some(9));
    let restored = reopened
        .update(
            &created.id,
            nexus_workbench::ThreadPatch {
                archived: Some(false),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(restored.aside.settled_at.is_some(), "restored, it stays in Settled");
    assert!(
        reopened
            .archive_merged(&created.id, 9)
            .unwrap_or_else(|e| panic!("{e}"))
            .is_none(),
        "the same merge does not archive it again"
    );
    reopened.shutdown().await;
}

/// The person settles a session by hand (a drag to "Settled" in the sidebar): it leaves the list with no merge, ends
/// "Keep in list", is not activity, and comes back by hand or with a new turn.
#[tokio::test]
async fn the_person_settles_a_session_by_hand_and_brings_it_back() {
    let s = setup("settle-by-hand");
    let created = s
        .workbench
        .create_thread(new_thread(&s.project, "hi"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    let idle = thread(&s.workbench, &created.id);
    let patch = |settled: Option<bool>, keep_listed: Option<bool>| nexus_workbench::ThreadPatch {
        settled,
        keep_listed,
        ..Default::default()
    };
    s.workbench
        .update(&created.id, patch(None, Some(true)))
        .unwrap_or_else(|e| panic!("{e}"));
    let settled = s
        .workbench
        .update(&created.id, patch(Some(true), None))
        .unwrap_or_else(|e| panic!("{e}"));
    let at = settled.aside.settled_at.unwrap_or_else(|| panic!("settled"));
    assert!(!settled.aside.keep_listed, "settling by hand ends Keep in list");
    assert_eq!(settled.aside.settled_by, None, "no merge settled it");
    assert_eq!(settled.updated_at, idle.updated_at, "settling is not activity");
    let again = s
        .workbench
        .update(&created.id, patch(Some(true), None))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(again.aside.settled_at, Some(at), "settled already: it keeps its time");
    let back = s
        .workbench
        .update(&created.id, patch(Some(false), None))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(back.aside.settled_at, None, "back in the list");
    s.workbench
        .update(&created.id, patch(Some(true), None))
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .send(&created.id, "again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(
        thread(&s.workbench, &created.id).aside.settled_at,
        None,
        "a new turn brings it back"
    );
    s.workbench.shutdown().await;
}

#[tokio::test]
async fn runs_a_turn_and_numbers_its_events() {
    let s = setup("turn");
    let created = s
        .workbench
        .create_thread(new_thread(&s.project, "Where do calls go? env"))
        .await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;

    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e.event {
            WbEvent::User { .. } => "user",
            WbEvent::TurnStart { .. } => "turnStart",
            WbEvent::Session { .. } => "session",
            WbEvent::Commands { .. } => "commands",
            WbEvent::Connection { .. } => "connection",
            WbEvent::Text { .. } => "text",
            WbEvent::Usage { .. } => "usage",
            WbEvent::TurnEnd { .. } => "turnEnd",
            WbEvent::Read { .. } => "read",
            _ => "other",
        })
        .collect();
    // Claude Code read the message back as it took it in: confirmed read.
    assert_eq!(
        kinds,
        [
            "user",
            "turnStart",
            "session",
            "connection",
            "connection",
            "commands",
            "read",
            "text",
            "usage",
            "turnEnd"
        ]
    );
    assert_eq!(events[6].event, WbEvent::Read { seq: 1 });
    assert_eq!(
        events.iter().map(|e| e.seq).collect::<Vec<_>>(),
        (1..=10).collect::<Vec<i64>>()
    );
    // What the agent offers on a "/" is kept for the composer.
    assert!(matches!(&events[5].event, WbEvent::Commands { commands } if commands == &["compact", "review"]));
    assert_eq!(
        texts(&events),
        ["Message 1 of this session. Calls go to https://gateway.test/v1/anthropic."]
    );
    let WbEvent::Session {
        nexus_session_id,
        resumed,
        ..
    } = &events[2].event
    else {
        panic!("not a session event")
    };
    assert_eq!((nexus_session_id.as_deref(), *resumed), (Some("ns_1"), false));

    // Streamed text reached the screen and was never stored.
    let deltas = s
        .host
        .emitted
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|e| matches!(e, Emit::Delta(delta) if delta.thread_id == id))
        .count();
    assert!(deltas >= 1);
    assert!(
        s.host
            .emitted
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .all(|e| !matches!(e, Emit::Event(StoredEvent { seq: 0, .. })))
    );

    // Claude Code's own figure replaced the estimate at the end of the turn.
    let summary = thread(&s.workbench, &id);
    assert_eq!(summary.status, ThreadStatus::Idle);
    assert!((summary.cost_usd - 0.01).abs() < 1e-9, "cost {}", summary.cost_usd);
    assert_eq!(summary.context_tokens, Some(1150));
    assert_eq!(summary.context_window, Some(200000));
    assert_eq!(summary.nexus_sessions, ["ns_1"]);

    // A second message goes to the same process, and the cost adds up.
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).len(), 1);
    assert!((thread(&s.workbench, &id).cost_usd - 0.02).abs() < 1e-9);

    s.workbench.shutdown().await;
    let ended = s.host.ended.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!(ended.len(), 1);
    assert_eq!((ended[0].0.as_str(), ended[0].1.exit_code), ("ns_1", 0));
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Makes the store at `db` refuse to log events of `kind`, as a full disk would (`on: false` lets them in again). A
/// trigger, through a connection of its own: the workbench's connection sees it at its next write.
fn refuse_events(db: &std::path::Path, kind: &str, on: bool) {
    let sql = if on {
        format!(
            "create trigger refuse_{kind} before insert on events when json_extract(new.body, '$.kind') = '{kind}' \
             begin select raise(fail, 'database or disk is full'); end;"
        )
    } else {
        format!("drop trigger refuse_{kind};")
    };
    let db = rusqlite::Connection::open(db).unwrap_or_else(|e| panic!("{e}"));
    db.execute_batch(&sql).unwrap_or_else(|e| panic!("{e}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_log_that_cannot_be_written_once_the_agent_runs_fails_the_session_and_the_next_message_starts_over() {
    let s = setup("store-full");
    // The agent is spawned, then the first write after it (its `session` event) fails.
    refuse_events(&s.db, "session", true);
    // The error reaches the person either way: as the call's error, or in the log.
    let _ = s.workbench.create_thread(new_thread(&s.project, "hello")).await;
    let id = s
        .workbench
        .threads()
        .unwrap_or_default()
        .first()
        .map(|t| t.id.clone())
        .unwrap_or_else(|| panic!("no session"));
    until_stopped(&s.workbench, &id).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert_eq!(turn_ends(&events), [TurnStatus::Error]);
    assert!(
        events.iter().any(
            |e| matches!(&e.event, WbEvent::Error { text, detail } if text.contains("could not be saved")
                && detail.as_deref().is_some_and(|d| d.contains("disk is full")))
        ),
        "{events:#?}"
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Error);
    // The agent was stopped and its Nexus session ended.
    let ended = s.host.ended.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!(ended.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["ns_1"]);

    // Space again: the next message starts a new agent, without a restart.
    refuse_events(&s.db, "session", false);
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Error, TurnStatus::Completed]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_log_that_cannot_record_the_first_turn_fails_the_launch_and_the_next_message_starts_over() {
    let s = setup("store-full-first-turn");
    // Recording the first turn fails before the agent is launched: the launch ends there, with nothing to stop.
    refuse_events(&s.db, "turnStart", true);
    let _ = s.workbench.create_thread(new_thread(&s.project, "hello")).await;
    let id = s
        .workbench
        .threads()
        .unwrap_or_default()
        .first()
        .map(|t| t.id.clone())
        .unwrap_or_else(|| panic!("no session"));
    until_stopped(&s.workbench, &id).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Error { text, .. } if text.contains("could not be saved"))),
        "{events:#?}"
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Error);
    // No agent was launched, so no Nexus session was opened to end.
    assert!(s.host.ended.lock().unwrap_or_else(|p| p.into_inner()).is_empty());

    refuse_events(&s.db, "turnStart", false);
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).contains(&TurnStatus::Completed)).await;
    assert!(turn_ends(&events).contains(&TurnStatus::Completed), "{events:#?}");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_even_the_failure_cannot_be_logged_the_screen_still_learns_the_session_failed() {
    let s = setup("store-full-everything");
    // Nothing about this run can be saved: not its first turn, nor how it ended, nor the error.
    for kind in ["turnStart", "turnEnd", "error"] {
        refuse_events(&s.db, kind, true);
    }
    let _ = s.workbench.create_thread(new_thread(&s.project, "hello")).await;
    let id = s
        .workbench
        .threads()
        .unwrap_or_default()
        .first()
        .map(|t| t.id.clone())
        .unwrap_or_else(|| panic!("no session"));
    until_stopped(&s.workbench, &id).await;
    let told = s
        .host
        .emitted
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .any(|e| matches!(e, Emit::Thread(summary) if summary.id == id && summary.status == ThreadStatus::Error));
    assert!(told, "the session's failure never reached the screen");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn an_answer_that_lands_after_the_turn_ended_leaves_the_session_idle() {
    let s = setup("late-answer");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "use a tool"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Approval { .. }))
    })
    .await;
    let request_id = events
        .iter()
        .find_map(|e| match &e.event {
            WbEvent::Approval { request_id, .. } => Some(request_id.clone()),
            _ => None,
        })
        .unwrap_or_default();
    // The answer reaches the agent, then its own bookkeeping is held up; the agent finishes the turn meanwhile.
    *s.host.stall_answer.lock().unwrap_or_else(|p| p.into_inner()) = Some(Duration::from_millis(500));
    let workbench = s.workbench.clone();
    let (thread_id, request) = (id.clone(), request_id.clone());
    let answer =
        tokio::task::spawn_blocking(move || workbench.approve(&thread_id, &request, Decision::Allow, None, None, None));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    answer
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
    // Nothing is under way: the session must not say Running.
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn keeps_the_agents_own_reports_on_background_tasks() {
    let s = setup("tasks");
    let created = s.workbench.create_thread(new_thread(&s.project, "bgtask")).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;

    let tasks: Vec<(String, TaskStatus, Option<String>)> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Task {
                task_id,
                status,
                tool_use_id,
                ..
            } => Some((task_id.clone(), *status, tool_use_id.clone())),
            _ => None,
        })
        .collect();
    let task = |id: &str, status, tool: Option<&str>| (id.to_string(), status, tool.map(String::from));
    assert_eq!(
        tasks,
        [
            task("bash1", TaskStatus::Running, Some("toolu_bg1")),
            task("agent1", TaskStatus::Running, Some("toolu_bg2")),
            task("bash1", TaskStatus::Completed, None),
            task("bash1", TaskStatus::Completed, Some("toolu_bg1")),
            task("agent1", TaskStatus::Stopped, None),
            task("agent1", TaskStatus::Stopped, Some("toolu_bg2")),
        ]
    );
    // The full list of what still runs is kept each time it changes.
    let lists: Vec<Vec<String>> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::BackgroundTasks { tasks } => Some(tasks.iter().map(|t| t.task_id.clone()).collect()),
            _ => None,
        })
        .collect();
    assert_eq!(lists, [vec!["bash1"], vec!["bash1", "agent1"], vec!["agent1"], vec![]]);
    let usage = events.iter().find_map(|e| match &e.event {
        WbEvent::Task {
            tool_count: Some(n),
            total_tokens,
            agent_type,
            ..
        } => Some((*n, *total_tokens, agent_type.clone())),
        _ => None,
    });
    assert_eq!(usage, Some((4, Some(1200), None)));
    // Reports belong to the turn that was running, and none started a turn of their own.
    assert_eq!(turn_ends(&events), [TurnStatus::Completed]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A sub-agent's reports reach the log through the core's record of its tasks (agents.rs): its start; its progress
/// once, as the turn ends, with the model its own frame named; and both its ends, the second with its summary and
/// usage. The foreground command and the progress after the end leave no row, even once the next turn's end writes
/// what the book holds.
#[tokio::test]
async fn a_sub_agents_reports_reach_the_log_through_the_book() {
    let s = setup("subagents");
    let created = s.workbench.create_thread(new_thread(&s.project, "subagents")).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    let last = |e: &[StoredEvent]| {
        e.iter()
            .any(|e| matches!(&e.event, WbEvent::Task { task_id, .. } if task_id == "last1"))
    };
    wait_for(&s.workbench, &id, last).await;
    // The next turn's end writes every row the book holds: a progress it kept after the end would show now.
    s.workbench.send(&id, "next").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    // Each row's task, status, tool count, activity and model.
    type Row<'a> = (&'a str, TaskStatus, Option<u64>, Option<String>, Option<String>);
    let rows: Vec<Row> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Task {
                task_id,
                status,
                tool_count,
                detail,
                ..
            } if task_id != "last1" => Some((
                task_id.as_str(),
                *status,
                *tool_count,
                detail.as_ref().and_then(|d| d.activity.clone()),
                detail.as_ref().and_then(|d| d.model.clone()),
            )),
            _ => None,
        })
        .collect();
    let model = Some("claude-sub-1".to_string());
    assert_eq!(
        rows,
        [
            ("agent1", TaskStatus::Running, None, None, None),
            (
                "agent1",
                TaskStatus::Running,
                Some(3),
                Some("Running step 3".into()),
                model
            ),
            ("agent1", TaskStatus::Completed, Some(3), None, None),
            ("agent1", TaskStatus::Completed, Some(3), None, None),
        ]
    );
    // The progress went out before the turn's end; the ends came after it.
    let at = |pred: &dyn Fn(&WbEvent) -> bool| events.iter().position(|e| pred(&e.event));
    let progress = at(&|e| matches!(e, WbEvent::Task { detail: Some(d), .. } if d.progress == Some(true)));
    let end = at(&|e| matches!(e, WbEvent::TurnEnd { .. }));
    assert!(
        matches!((progress, end), (Some(p), Some(e)) if p < e),
        "{progress:?} {end:?}"
    );
    s.workbench.shutdown().await;
}

/// Where in the log each event of this kind (by its `kind` in JSON) is.
fn positions(events: &[StoredEvent], kind: &str) -> Vec<usize> {
    events
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            serde_json::to_value(&e.event)
                .ok()
                .and_then(|v| v["kind"].as_str().map(String::from))
                .as_deref()
                == Some(kind)
        })
        .map(|(at, _)| at)
        .collect()
}

/// The request ids expired so far.
fn expired(events: &[StoredEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::ApprovalResolved {
                request_id,
                decision: Decision::Expired,
                ..
            } => Some(request_id.as_str()),
            _ => None,
        })
        .collect()
}

/// A background sub-agent's work after the session's turn is its own: its text and tool open no turn, and its
/// approvals outlive the turn. A foreground sub-agent's approval ends with the turn.
#[tokio::test]
async fn a_late_sub_agent_frame_opens_no_turn_and_its_approval_outlives_the_turn() {
    let s = setup("lateagent-turn");
    let created = s.workbench.create_thread(new_thread(&s.project, "lateagent")).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    let late = |e: &[StoredEvent]| texts(e).iter().any(|t| t == "Working late");
    let events = wait_for(&s.workbench, &id, late).await;
    let late_at = events
        .iter()
        .position(|e| matches!(&e.event, WbEvent::Text { text, .. } if text == "Working late"));
    assert_eq!(positions(&events, "turnStart").len(), 1);
    let end = positions(&events, "turnEnd");
    assert!(
        matches!((end.first(), late_at), (Some(end), Some(late)) if end < &late),
        "{end:?} {late_at:?}"
    );
    assert_eq!(expired(&events), ["sub-ask-3"]);
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Approval);
    s.workbench.shutdown().await;
}

/// A notice after the turn (a sub-agent's compaction, which names no parent) opens no session turn: the session is not
/// held "running" by work that is not the lead's, and the next message starts its turn at once.
#[tokio::test]
async fn a_notice_after_the_turn_opens_no_turn() {
    let s = setup("late-compact");
    let created = s.workbench.create_thread(new_thread(&s.project, "latecompact")).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    let compacted = |e: &[StoredEvent]| {
        e.iter()
            .any(|e| matches!(&e.event, WbEvent::Notice { text } if text.contains("compacted")))
    };
    let events = wait_for(&s.workbench, &id, compacted).await;
    assert_eq!(positions(&events, "turnStart").len(), 1, "{events:#?}");
    assert_eq!(turn_ends(&events).len(), 1);
    assert_ne!(thread(&s.workbench, &id).status, ThreadStatus::Running);
    s.workbench.send(&id, "next").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(positions(&events, "turnStart").len(), 2);
    s.workbench.shutdown().await;
}

/// The person's message while a background sub-agent works after the turn starts its own turn at once: the
/// sub-agent's frames hold no turn it could wait behind.
#[tokio::test]
async fn a_message_during_late_sub_agent_work_starts_its_turn_at_once() {
    let s = setup("lateagent-message");
    let created = s.workbench.create_thread(new_thread(&s.project, "lateagent")).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| texts(e).iter().any(|t| t == "Working late")).await;
    s.workbench
        .send(&id, "meanwhile")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let started = std::time::Instant::now();
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    // Its turn is the message's: it starts right after the message, not behind work of the sub-agent's.
    let message = events
        .iter()
        .rposition(|e| matches!(&e.event, WbEvent::User { text, .. } if text == "meanwhile"));
    let second = positions(&events, "turnStart").get(1).copied();
    assert!(
        matches!((message, second), (Some(m), Some(t)) if t > m),
        "{message:?} {second:?}"
    );
    // The sub-agent's questions are still open: its work goes on beside the person's turn.
    assert_eq!(expired(&events), ["sub-ask-3"]);
    s.workbench.shutdown().await;
}

/// Answering a background sub-agent's approval after the turn reaches it. Its end expires the approval it left open.
/// The main agent's answer to it opens a turn of its own, and a result with no turn open still logs a pair.
#[tokio::test]
async fn an_agents_end_expires_its_approvals_and_the_main_agents_answer_opens_a_turn() {
    let s = setup("lateagent-end");
    let created = s.workbench.create_thread(new_thread(&s.project, "lateagent")).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    let late = |e: &[StoredEvent]| texts(e).iter().any(|t| t == "Working late");
    wait_for(&s.workbench, &id, late).await;
    s.workbench
        .approve(&id, "sub-ask-1", Decision::Allow, None, None, None)
        .unwrap_or_else(|e| panic!("{e}"));
    let last = |e: &[StoredEvent]| {
        e.iter()
            .any(|e| matches!(&e.event, WbEvent::Task { task_id, .. } if task_id == "last2"))
    };
    let events = wait_for(&s.workbench, &id, last).await;
    let allowed = events.iter().position(|e| {
        matches!(&e.event, WbEvent::ToolResult { id, output, parent: Some(parent), .. }
            if id == "toolu_la_cmd" && output == "allowed" && parent == "toolu_la1")
    });
    let agent_end = events.iter().position(
        |e| matches!(&e.event, WbEvent::Task { task_id, status: TaskStatus::Completed, .. } if task_id == "agent2"),
    );
    let expiry = events.iter().position(|e| {
        matches!(&e.event, WbEvent::ApprovalResolved { request_id, decision: Decision::Expired, .. } if request_id == "sub-ask-2")
    });
    let starts = positions(&events, "turnStart");
    assert_eq!(starts.len(), 3, "{starts:?}");
    assert_eq!(positions(&events, "turnEnd").len(), 3);
    // The command's result and the agent's end come before the main agent's own turn.
    assert!(
        matches!((allowed, agent_end, expiry), (Some(a), Some(e), Some(x)) if a < starts[1] && e < x),
        "{allowed:?} {agent_end:?} {expiry:?}"
    );
    assert_eq!(expired(&events), ["sub-ask-3", "sub-ask-2"]);
    let answer = events
        .iter()
        .position(|e| matches!(&e.event, WbEvent::Text { text, parent: None, .. } if text == "agent2 finished"));
    assert!(
        matches!(answer, Some(at) if at > starts[1] && at < starts[2]),
        "{answer:?} {starts:?}"
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
}

/// An agent that runs again ends its earlier run's approvals with its new start, though that run's own end comes late
/// and is dropped as an older run's.
#[tokio::test]
async fn a_new_run_ends_the_approvals_of_the_run_before() {
    let s = setup("rerun-ask");
    let created = s.workbench.create_thread(new_thread(&s.project, "rerunask")).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    let rerun = events.iter().position(|e| {
        matches!(&e.event, WbEvent::Task { task_id, detail: Some(d), .. } if task_id == "agent5" && d.run_id.as_deref() == Some("run-5b"))
    });
    let expiry = events.iter().position(|e| {
        matches!(&e.event, WbEvent::ApprovalResolved { request_id, decision: Decision::Expired, .. } if request_id == "sub-ask-5")
    });
    assert!(
        matches!((rerun, expiry), (Some(r), Some(x)) if r < x),
        "{rerun:?} {expiry:?}"
    );
    // The older run's end never reached the log.
    assert!(!events.iter().any(
        |e| matches!(&e.event, WbEvent::Task { task_id, status: TaskStatus::Stopped, .. } if task_id == "agent5")
    ));
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
}

/// The task rows the core wrote when the agent's process ended: each stopped task, and the reason it gave.
fn process_ends(events: &[StoredEvent]) -> Vec<(String, Option<String>)> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Task {
                task_id,
                status: TaskStatus::Stopped,
                detail: Some(detail),
                ..
            } if detail.reason.as_deref() == Some("process_ended") => Some((task_id.clone(), detail.reason.clone())),
            _ => None,
        })
        .collect()
}

/// A session whose agent runs a background sub-agent (`agent2`) and a foreground one (`agent3`) past its turn.
async fn agents_running(name: &str, words: &str) -> (Setup, String) {
    let s = setup(name);
    let created = s.workbench.create_thread(new_thread(&s.project, words)).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    let late = |e: &[StoredEvent]| texts(e).iter().any(|t| t == "Working late");
    wait_for(&s.workbench, &id, late).await;
    (s, id)
}

/// Stop ends every sub-agent: the log says so with one stopped row per task, before the turn end and the notices, so
/// no reader has to guess from a notice's words.
#[tokio::test]
async fn stop_logs_a_stopped_row_per_running_task() {
    let (s, id) = agents_running("ends-stop", "lateagent").await;
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    let ends = process_ends(&events);
    let tasks: Vec<&str> = ends.iter().map(|(task, _)| task.as_str()).collect();
    assert_eq!(tasks, ["agent2", "agent3"]);
    let first_end = events.iter().position(
        |e| matches!(&e.event, WbEvent::Task { detail: Some(d), .. } if d.reason.as_deref() == Some("process_ended")),
    );
    let notice = events.iter().position(|e| matches!(e.event, WbEvent::Notice { .. }));
    assert!(
        matches!((first_end, notice), (Some(end), Some(notice)) if end < notice),
        "{first_end:?} {notice:?}"
    );
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Restart ends every sub-agent too, with the same rows.
#[tokio::test]
async fn restart_logs_a_stopped_row_per_running_task() {
    let (s, id) = agents_running("ends-restart", "lateagent").await;
    s.workbench.restart(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    let tasks: Vec<String> = process_ends(&events).into_iter().map(|(task, _)| task).collect();
    assert_eq!(tasks, ["agent2", "agent3"]);
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A crash ends every sub-agent: the same rows come before the error.
#[tokio::test]
async fn a_crash_logs_a_stopped_row_per_running_task() {
    let (s, id) = agents_running("ends-crash", "lateagent thenexit").await;
    until_stopped(&s.workbench, &id).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    let tasks: Vec<String> = process_ends(&events).into_iter().map(|(task, _)| task).collect();
    assert_eq!(tasks, ["agent2", "agent3"]);
    let error = events.iter().position(|e| matches!(e.event, WbEvent::Error { .. }));
    let last_end = events.iter().rposition(|e| {
        matches!(
            &e.event,
            WbEvent::Task {
                status: TaskStatus::Stopped,
                ..
            }
        )
    });
    assert!(
        matches!((last_end, error), (Some(end), Some(error)) if end < error),
        "{last_end:?} {error:?}"
    );
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The tasks of each status the log ended, by id, sorted.
fn ended_tasks(events: &[StoredEvent]) -> Vec<String> {
    let mut ended: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Task {
                task_id,
                status: TaskStatus::Stopped,
                ..
            } => Some(task_id.clone()),
            _ => None,
        })
        .collect();
    ended.sort();
    ended.dedup();
    ended
}

/// A session whose agent runs `fake-claude`'s team (`boss`, its foreground `worker`, its background `helper` and its
/// shell `bshell`), with `env` for the stand-in. Without `slow` in `words`, the turn has ended.
async fn team_running(name: &str, words: &str, env: &[(&str, &str)]) -> (Setup, String) {
    let s = setup(name);
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let created = s.workbench.create_thread(new_thread(&s.project, words)).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    let slow = words.contains("slow");
    wait_for(&s.workbench, &id, |e| {
        let shell = e
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Task { task_id, .. } if task_id == "bshell"));
        shell && (slow || turn_ends(e).len() == 1)
    })
    .await;
    (s, id)
}

/// The session's summary says what its running agent accepts once the agent reports its version, and nothing once
/// its process ended: capabilities are live only. The screen hears both.
#[tokio::test]
async fn capabilities_reach_the_summary_after_init_and_leave_at_the_end() {
    let s = setup("capabilities");
    let created = s.workbench.create_thread(new_thread(&s.project, "hi")).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    let capabilities = thread(&s.workbench, &id)
        .capabilities
        .unwrap_or_else(|| panic!("no capabilities"));
    assert_eq!(capabilities.version.as_deref(), Some("2.1.292"));
    assert_eq!(capabilities.stop, ["agent", "shell"]);
    assert!(capabilities.interrupt_spares_agents);
    let pushed = |s: &Setup| {
        s.host
            .emitted
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .rev()
            .find_map(|e| match e {
                Emit::Thread(t) if t.id == id => Some(t.capabilities.is_some()),
                _ => None,
            })
    };
    assert_eq!(pushed(&s), Some(true));
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    assert_eq!(thread(&s.workbench, &id).capabilities, None);
    assert_eq!(pushed(&s), Some(false));
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A stop of one sub-agent stops it, its foreground sub-agent and its shell, each with its own stopped row, and its
/// background sub-agent runs on (Claude Code's `stop_task`). Asked again, or about a task the book never saw, the core
/// refuses without asking: the stand-in would answer with an error of its own.
#[tokio::test]
async fn stops_one_sub_agent_and_logs_each_stopped_row() {
    let (s, id) = team_running("task-stop", "team", &[]).await;
    s.workbench
        .task_stop(&id, "boss")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| ended_tasks(e).len() == 3).await;
    assert_eq!(ended_tasks(&events), ["boss", "bshell", "worker"]);
    assert!(
        !events.iter().any(|e| matches!(&e.event, WbEvent::Notice { .. })),
        "{events:#?}"
    );
    for task in ["boss", "nope"] {
        let refused = s.workbench.task_stop(&id, task).await;
        assert_eq!(refused, Err("That task is not running.".into()), "{task}");
    }
    // A kind of task the capabilities do not list.
    let refused = s.workbench.task_stop(&id, "flow").await;
    assert_eq!(refused, Err("Claude Code can't stop this kind of task.".into()));
    // The background sub-agent still runs, and can be stopped on its own.
    s.workbench
        .task_stop(&id, "helper")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| ended_tasks(e).len() == 4).await;
    assert_eq!(ended_tasks(&events), ["boss", "bshell", "helper", "worker"]);
    // None of these ends came from the process.
    assert_eq!(process_ends(&events), []);
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A stop the agent refuses is the command's error and a notice in the log, which names the task.
#[tokio::test]
async fn a_refused_stop_is_an_error_and_a_notice() {
    let (s, id) = team_running("task-stop-refused", "team", &[("FAKE_CLAUDE_REFUSE_STOPS", "1")]).await;
    let refused = s.workbench.task_stop(&id, "helper").await;
    assert_eq!(refused, Err("fake: refused".into()));
    let notices: Vec<String> = s
        .workbench
        .events(&id, 0)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| match e.event {
            WbEvent::Notice { text } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(notices, ["Claude Code could not stop “Helper”: fake: refused"]);
    let _ = std::fs::remove_dir_all(&s.root);
}

/// An agent that never answers a stop: the command gives up after five seconds, with no notice, since nothing says
/// the agent refused.
#[tokio::test]
async fn an_unanswered_stop_is_an_error_without_a_notice() {
    let (s, id) = team_running("task-stop-unanswered", "team", &[("FAKE_CLAUDE_IGNORE_STOPS", "1")]).await;
    let unanswered = s.workbench.task_stop(&id, "helper").await;
    assert_eq!(unanswered, Err("Claude Code did not answer.".into()));
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert!(
        !events.iter().any(|e| matches!(&e.event, WbEvent::Notice { .. })),
        "{events:#?}"
    );
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A stop waiting for its answer when the agent's run ends hears "The agent stopped." at once, not after five seconds.
/// Once the run is stopping, a stop is refused: the agent's input is closing, so it would never read the request.
#[tokio::test]
async fn a_run_that_ends_answers_a_waiting_stop() {
    let (s, id) = team_running("task-stop-ended", "team", &[("FAKE_CLAUDE_IGNORE_STOPS", "1")]).await;
    let workbench = s.workbench.clone();
    let task = id.clone();
    let started = std::time::Instant::now();
    let waiting = tokio::spawn(async move { workbench.task_stop(&task, "helper").await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    let late = s.workbench.task_stop(&id, "boss").await;
    assert!(
        matches!(
            late.as_ref().map_err(String::as_str),
            Err("The agent is stopping." | "The agent is not running.")
        ),
        "{late:?}"
    );
    let answer = waiting.await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(answer, Err("The agent stopped.".into()));
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    until_stopped(&s.workbench, &id).await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A long foreground command moves to the background: Claude Code marks it backgrounded, its start row reaches the log
/// (it had none in the foreground), its call ends with a receipt, and the turn ends. Then its output can be read. A
/// command the book has no foreground record of is refused without asking: one never reported, or the one already moved.
#[tokio::test]
async fn moves_a_foreground_command_to_the_background() {
    let s = setup("task-background");
    let created = s.workbench.create_thread(new_thread(&s.project, "fgwait")).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter()
            .any(|e| matches!(&e.event, WbEvent::Tool { id, .. } if id == "toolu_fg"))
    })
    .await;
    // Its start reaches the core right after its call.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let refused = s.workbench.task_background(&id, "toolu_nope").await;
    assert_eq!(refused, Err("That command is not running in the foreground.".into()));
    let moved = s.workbench.task_background(&id, "toolu_fg").await;
    assert_eq!(moved, Ok(true));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    let start = events.iter().find_map(|e| match &e.event {
        WbEvent::Task {
            task_id,
            status: TaskStatus::Running,
            detail,
            ..
        } if task_id == "b_fg" => Some(detail.as_ref().and_then(|d| d.backgrounded)),
        _ => None,
    });
    assert_eq!(start, Some(Some(true)), "{events:#?}");
    assert!(events.iter().any(|e| matches!(&e.event,
        WbEvent::ToolResult { id, output, .. } if id == "toolu_fg" && output.starts_with("Command was manually backgrounded"))));
    let again = s.workbench.task_background(&id, "toolu_fg").await;
    assert_eq!(again, Err("That command is not running in the foreground.".into()));
    let output = s
        .workbench
        .task_output(&id, "b_fg")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(output.output, "tick 1\ntick 2\ntick 3\n");
    assert!(!output.truncated);
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A move the agent refuses is the command's error and a notice that names the command.
#[tokio::test]
async fn a_refused_move_is_an_error_and_a_notice() {
    let s = setup("task-background-refused");
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_REFUSE_STOPS".into(), "1".into());
    let created = s.workbench.create_thread(new_thread(&s.project, "fgwait")).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter()
            .any(|e| matches!(&e.event, WbEvent::Tool { id, .. } if id == "toolu_fg"))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let refused = s.workbench.task_background(&id, "toolu_fg").await;
    assert_eq!(refused, Err("fake: refused".into()));
    let notices: Vec<String> = s
        .workbench
        .events(&id, 0)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| match e.event {
            WbEvent::Notice { text } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(
        notices,
        [
            "Terminal bypass (host_unsupported): This host does not offer shared terminal execution. The provider requested command toolu_fg outside the workbench; Nexus did not rerun it.",
            "Claude Code could not move “make long” to the background: fake: refused",
        ]
    );
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Reading a shell's output is not the agent at work: an output view left open does not keep an idle agent alive.
#[tokio::test]
async fn reading_output_keeps_no_agent_awake() {
    // Background work waits twelve times the idle limit: 600 ms here.
    let (s, id) = idle_setup("idle-output", Duration::from_millis(50), 1000, "team").await;
    for _ in 0..12 {
        s.workbench
            .task_output(&id, "bshell")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    s.workbench.reap_idle();
    until_stopped(&s.workbench, &id).await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A shell's output is read live and cut to its last 8 KiB; a sub-agent's is refused without asking (Claude Code would
/// refuse it too, with words of its own).
#[tokio::test]
async fn reads_a_shells_output_and_refuses_a_sub_agents() {
    let (s, id) = team_running("task-output", "team", &[("FAKE_CLAUDE_BIG_OUTPUT", "1")]).await;
    let output = s
        .workbench
        .task_output(&id, "bshell")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(output.output.len(), nexus_workbench::TASK_OUTPUT_MAX);
    assert!(output.output.ends_with("end\n"));
    assert_eq!(output.total_bytes, 20000);
    assert!(output.truncated);
    let refused = s.workbench.task_output(&id, "helper").await;
    assert_eq!(
        refused,
        Err("Claude Code can't read the output of this kind of task.".into())
    );
    // Reading is not acting: nothing reaches the log.
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert!(
        !events.iter().any(|e| matches!(&e.event, WbEvent::Notice { .. })),
        "{events:#?}"
    );
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A Claude Code older than the recorded one offers no control, and the core refuses a stop without asking. With no
/// agent running, it refuses too.
#[tokio::test]
async fn an_older_claude_code_offers_no_stop() {
    let (s, id) = team_running("task-stop-old", "team", &[("FAKE_CLAUDE_VERSION", "2.1.291")]).await;
    let capabilities = thread(&s.workbench, &id)
        .capabilities
        .expect("command discovery is independent from task controls");
    assert!(capabilities.stop.is_empty());
    assert!(capabilities.background.is_empty());
    assert!(capabilities.output.is_empty());
    assert_eq!(capabilities.version.as_deref(), Some("2.1.291"));
    let refused = s.workbench.task_stop(&id, "helper").await;
    assert_eq!(refused, Err("This version of Claude Code can't stop one task.".into()));
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let refused = s.workbench.task_stop(&id, "helper").await;
    assert_eq!(refused, Err("The agent is not running.".into()));
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Lucas's first condition for `perTaskStopAffordance`: Interrupt now ends only the lead's turn and the agents run on
/// (the stand-in spares them only when `initialize` declared it), and Stop and Restart still end every one of them,
/// each with its stopped row.
#[tokio::test]
async fn stop_and_restart_end_every_sub_agent_when_interrupt_spares_them() {
    for restart in [false, true] {
        let name = if restart { "spared-restart" } else { "spared-stop" };
        let (s, id) = team_running(name, "team slow", &[]).await;
        s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
        let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
        assert_eq!(ended_tasks(&events), Vec::<String>::new(), "the interrupt spared them");
        if restart {
            s.workbench.restart(&id).unwrap_or_else(|e| panic!("{e}"));
        } else {
            s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
        }
        until_stopped(&s.workbench, &id).await;
        let events = s.workbench.events(&id, 0).unwrap_or_default();
        let mut ended: Vec<String> = process_ends(&events).into_iter().map(|(task, _)| task).collect();
        ended.sort();
        assert_eq!(
            ended,
            ["boss", "bshell", "flow", "helper", "worker"],
            "restart: {restart}"
        );
        let _ = std::fs::remove_dir_all(&s.root);
    }
}

/// A task report that comes after the turn ended is kept, and starts no turn of its own.
#[tokio::test]
async fn a_late_task_report_starts_no_turn() {
    let s = setup("late-report");
    let created = s.workbench.create_thread(new_thread(&s.project, "latereport")).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| {
        e.iter()
            .any(|e| matches!(&e.event, WbEvent::Task { task_id, .. } if task_id == "late1"))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let events = s.workbench.events(&id, 0).unwrap_or(events);
    let starts = events
        .iter()
        .filter(|e| matches!(e.event, WbEvent::TurnStart { .. }))
        .count();
    assert_eq!(starts, 1, "{events:#?}");
    assert_eq!(turn_ends(&events), [TurnStatus::Completed]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn relays_approvals() {
    let s = setup("approval");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "use a tool"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Approval { .. }))
    })
    .await;
    let Some(WbEvent::Approval {
        request_id,
        tool,
        suggestions,
        ..
    }) = events
        .iter()
        .map(|e| &e.event)
        .find(|e| matches!(e, WbEvent::Approval { .. }))
    else {
        panic!("no approval")
    };
    assert_eq!(tool, "Bash");
    assert_eq!(suggestions[0].label, "Always allow Bash(echo:*) for this session");
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Approval);

    s.workbench
        .approve(&id, request_id, Decision::AllowSession, None, Some(0), None)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    let output = events.iter().find_map(|e| match &e.event {
        WbEvent::ToolResult { output, .. } => Some(output.clone()),
        _ => None,
    });
    assert_eq!(output.as_deref(), Some("hi (remembered)"));
    assert!(
        s.workbench
            .approve(&id, request_id, Decision::Allow, None, None, None)
            .is_err()
    );

    // A denial reaches the agent with its reason.
    s.workbench
        .send(&id, "another tool")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| {
        e.iter().filter(|e| matches!(e.event, WbEvent::Approval { .. })).count() == 2
    })
    .await;
    let second = events.iter().rev().find_map(|e| match &e.event {
        WbEvent::Approval { request_id, .. } => Some(request_id.clone()),
        _ => None,
    });
    let second = second.unwrap_or_default();
    s.workbench
        .approve(&id, &second, Decision::Deny, Some("not now".into()), None, None)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert!(events.iter().any(
        |e| matches!(&e.event, WbEvent::ToolResult { output, is_error: true, .. } if output == "denied: not now")
    ));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn interrupts_a_turn_and_expires_its_approval() {
    let s = setup("interrupt");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "slow work"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Text { .. }))
    })
    .await;
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted]);
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    assert!(s.workbench.is_live(&id), "an interrupt keeps the agent");

    s.workbench
        .send(&id, "a tool please")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Approval { .. }))
    })
    .await;
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert!(events.iter().any(|e| matches!(
        e.event,
        WbEvent::ApprovalResolved {
            decision: Decision::Expired,
            ..
        }
    )));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn reports_a_crash_and_resumes_after_it() {
    let s = setup("crash");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench
        .send(&id, "crash now")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Error { .. }))
    })
    .await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed, TurnStatus::Error]);
    for _ in 0..200 {
        if !s.workbench.is_live(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Interrupted);
    let ended = s.host.ended.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!(ended.first().map(|(_, end)| end.exit_code), Some(3));

    // The next message starts the agent again on the same conversation.
    s.workbench.resume(&id).await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this session.")
    );
    let launches = s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert!(launches[1].args.contains(&"--resume".to_string()));
    assert_eq!(thread(&s.workbench, &id).nexus_sessions, ["ns_1", "ns_2"]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn survives_quitting_in_the_middle_of_a_turn() {
    let s = setup("quit");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "slow and steady"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Text { .. }))
    })
    .await;
    s.workbench.shutdown().await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted]);
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Notice { text } if text == "Stopped when Nexus quit."))
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Interrupted);
    assert_eq!(s.host.ended.lock().unwrap_or_else(|p| p.into_inner()).len(), 1);
    drop(s.workbench);

    // The app starts again: the session is there, and the next message resumes it.
    let workbench = open(&s.host, &s.db);
    assert_eq!(thread(&workbench, &id).status, ThreadStatus::Interrupted);
    workbench.resume(&id).await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this session.")
    );
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn marks_a_session_interrupted_after_the_app_died() {
    let s = setup("died");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "use a tool"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Approval { .. }))
    })
    .await;
    // No shutdown: as if the app was killed. A new workbench on the same file finds the session mid-turn.
    let workbench = open(&s.host, &s.db);
    let events = workbench.events(&id, 0).unwrap_or_default();
    assert!(events.iter().any(|e| matches!(
        e.event,
        WbEvent::ApprovalResolved {
            decision: Decision::Expired,
            ..
        }
    )));
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted]);
    assert_eq!(thread(&workbench, &id).status, ThreadStatus::Interrupted);
    // Its Nexus session, never ended, is ended now; once.
    workbench.end_lost_sessions().await;
    workbench.end_lost_sessions().await;
    let ended = s.host.ended.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!(ended.len(), 1);
    assert_eq!(
        (ended[0].0.as_str(), ended[0].1.signal.as_deref()),
        ("ns_1", Some("lost"))
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A lost Nexus session leaves the ledger only once Gateway took its end: one the app died again before ending, or
/// that Gateway did not take, is ended at the next launch.
#[tokio::test]
async fn a_lost_session_stays_until_gateway_takes_its_end() {
    let s = setup("lost-again");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "use a tool"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Approval { .. }))
    })
    .await;
    let ended = |host: &TestHost| -> Vec<(String, Option<String>)> {
        host.ended
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(session, end)| (session.clone(), end.signal.clone()))
            .collect()
    };
    // The app is killed; the next launch dies too, before it ends anything.
    drop(open(&s.host, &s.db));
    // The launch after that cannot reach Gateway.
    *s.host.refuse_ends.lock().unwrap_or_else(|p| p.into_inner()) = true;
    open(&s.host, &s.db).end_lost_sessions().await;
    *s.host.refuse_ends.lock().unwrap_or_else(|p| p.into_inner()) = false;
    // The next one ends it, and the one after that has nothing left to end.
    open(&s.host, &s.db).end_lost_sessions().await;
    open(&s.host, &s.db).end_lost_sessions().await;
    let lost = Some("lost".to_string());
    assert_eq!(
        ended(&s.host),
        [("ns_1".to_string(), lost.clone()), ("ns_1".to_string(), lost)]
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn stops_an_idle_agent() {
    let root = std::env::temp_dir().join(format!("nexus-workbench-idle-{}", std::process::id()));
    let _ = std::fs::create_dir_all(root.join("repo"));
    let host = Arc::new(TestHost {
        memory: root.clone(),
        ..TestHost::default()
    });
    let store = Store::in_memory().unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "s".into(),
        idle_after: Duration::from_millis(100),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(store, host.clone(), config).unwrap_or_else(|e| panic!("{e}"));
    assert!(!workbench.persistent());
    let project = workbench
        .add_project(&root.join("repo"))
        .map(|p| p.id)
        .unwrap_or_default();
    let id = workbench
        .create_thread(new_thread(&project, "hi"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &id, |e| turn_ends(e).len() == 1).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    workbench.reap_idle();
    for _ in 0..200 {
        if !workbench.is_live(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!workbench.is_live(&id));
    assert_eq!(thread(&workbench, &id).status, ThreadStatus::Idle);
    assert!(
        !workbench
            .events(&id, 0)
            .unwrap_or_default()
            .iter()
            .any(|e| matches!(e.event, WbEvent::Error { .. }))
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A workbench whose agents count as idle after `idle_after`, with one finished turn of `text` in a new session; the
/// stand-in's background shells run `background_ms`.
async fn idle_setup(name: &str, idle_after: Duration, background_ms: u64, text: &str) -> (Setup, String) {
    let mut s = setup(name);
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_BACKGROUND_MS".into(), background_ms.to_string());
    let store = Store::in_memory().unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "gw|org".into(),
        idle_after,
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    s.workbench = Workbench::new(store, s.host.clone(), config).unwrap_or_else(|e| panic!("{e}"));
    s.project = s
        .workbench
        .add_project(&s.root.join("repo"))
        .map(|p| p.id)
        .unwrap_or_default();
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, text))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    (s, id)
}

#[tokio::test]
async fn the_idle_timeout_waits_for_background_work() {
    let (s, id) = idle_setup(
        "idle-background",
        Duration::from_millis(100),
        700,
        "run it in the background",
    )
    .await;
    // The turn ended, but its background shell still runs: the agent is not idle.
    assert!(s.workbench.has_active_work());
    // Changing organizations must not hide work from the desktop's inhibitor.
    s.workbench.set_scope("gw|another-org");
    assert!(s.workbench.has_active_work());
    s.workbench.set_scope("gw|org");
    tokio::time::sleep(Duration::from_millis(250)).await;
    s.workbench.reap_idle();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(s.workbench.is_live(&id), "reaped while background work ran");
    // Once the shell has finished, the agent is idle again.
    tokio::time::sleep(Duration::from_millis(600)).await;
    s.workbench.reap_idle();
    until_stopped(&s.workbench, &id).await;
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    assert!(!s.workbench.has_active_work());
    // Its background work had ended on its own: nothing to end with the agent.
    assert!(!background_ended(&s.workbench, &id));
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Whether the session says the agent's background work stopped with it (tasks.ts ends those tasks on it).
fn background_ended(workbench: &Workbench, id: &str) -> bool {
    workbench
        .events(id, 0)
        .unwrap_or_default()
        .iter()
        .any(|e| matches!(&e.event, WbEvent::Notice { text } if text == "What the agent left running in the background stopped with it."))
}

#[tokio::test]
async fn background_work_that_never_reports_back_holds_the_agent_only_so_long() {
    let (s, id) = idle_setup("idle-background-lost", Duration::from_millis(100), 60_000, "background").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    s.workbench.reap_idle();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(s.workbench.is_live(&id));
    // Twelve times the idle time without a word: stopped anyway.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    s.workbench.reap_idle();
    until_stopped(&s.workbench, &id).await;
    // The shell was still on the agent's list: the session says it stopped too, or it would show as running for good.
    assert!(background_ended(&s.workbench, &id));
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Waits until no agent runs for the session.
async fn until_stopped(workbench: &Workbench, id: &str) {
    for _ in 0..400 {
        if !workbench.is_live(id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the agent of {id} is still running");
}

#[tokio::test]
async fn resumes_the_conversation_claude_code_reports() {
    let s = setup("fork");
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_FORK".into(), "1".into());
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    // The next start resumes the id Claude Code ran under, so it finds the conversation.
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed, TurnStatus::Completed]);
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this session.")
    );
    let launches = s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let resumed = launches[1]
        .args
        .iter()
        .position(|a| a == "--resume")
        .map(|i| launches[1].args[i + 1].clone());
    assert!(resumed.is_some_and(|r| r.ends_with("-fork")));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The value after `name` in a launch's arguments.
fn launch_arg(launch: &LaunchRequest, name: &str) -> Option<String> {
    let at = launch.args.iter().position(|a| a == name)?;
    launch.args.get(at + 1).cloned()
}

/// A session whose first turn ended, whose agent then stopped, and whose conversation Claude Code no longer has.
async fn session_claude_code_lost(s: &Setup) -> String {
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let first = s.host.launches.lock().unwrap_or_else(|p| p.into_inner())[0].clone();
    let native = launch_arg(&first, "--session-id").unwrap_or_else(|| panic!("no --session-id"));
    std::fs::remove_file(s.host.memory.join(&native)).unwrap_or_else(|e| panic!("{e}"));
    id
}

#[tokio::test]
async fn a_resume_claude_code_cannot_open_continues_from_nexuss_copy() {
    let s = setup("unresumed");
    let id = session_claude_code_lost(&s).await;
    // Before the fix, this and every later message failed: "No conversation found with session ID".
    s.workbench.send(&id, "go on").await.unwrap_or_else(|e| panic!("{e}"));
    // The fake reacts to the word "tool" in the transcript disclaimer with one simulated command approval.
    let asked = wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Approval { .. }))
    })
    .await;
    let request = asked
        .iter()
        .find_map(|e| match &e.event {
            WbEvent::Approval { request_id, .. } => Some(request_id.clone()),
            _ => None,
        })
        .unwrap_or_default();
    s.workbench
        .approve(&id, &request, Decision::Allow, None, None, None)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| {
        turn_ends(e).len() == 3 && turn_ends(e).last() == Some(&TurnStatus::Completed)
    })
    .await;
    assert_eq!(
        turn_ends(&events),
        [TurnStatus::Completed, TurnStatus::Interrupted, TurnStatus::Completed]
    );
    assert!(!events.iter().any(|e| matches!(e.event, WbEvent::Error { .. })));
    assert!(
        events.iter().any(
            |e| matches!(&e.event, WbEvent::Notice { text } if text == nexus_workbench::resume_recovery::UNRESUMED)
        )
    );
    // The message shows once.
    let sent = events
        .iter()
        .filter(|e| matches!(&e.event, WbEvent::User { text, .. } if text == "go on"))
        .count();
    assert_eq!(sent, 1);
    let launches = s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!(launches.len(), 3);
    let lost = launch_arg(&launches[0], "--session-id");
    assert_eq!(launch_arg(&launches[1], "--resume"), lost);
    let fresh = launch_arg(&launches[2], "--session-id").unwrap_or_else(|| panic!("a fresh conversation"));
    assert!(Some(&fresh) != lost.as_ref());
    assert!(launch_arg(&launches[2], "--resume").is_none());
    // The fresh conversation got the message once, with what was said before it.
    let prompts = std::fs::read_to_string(s.host.memory.join(format!("{fresh}.prompts"))).unwrap_or_default();
    let prompts: Vec<&str> = prompts.lines().collect();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(prompts[0].contains("User: hello") && prompts[0].contains("Assistant: Message 1 of this session."));
    assert!(prompts[0].contains("could not be opened"), "{}", prompts[0]);
    assert!(
        prompts[0].contains("</conversation>  The new message follows.  go on"),
        "{}",
        prompts[0]
    );
    until_idle(&s.workbench, &id).await;

    // The next message resumes the fresh conversation, which Claude Code has.
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 4).await;
    assert_eq!(turn_ends(&events).last(), Some(&TurnStatus::Completed));
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this session.")
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_fresh_start_after_a_failed_resume_is_not_retried() {
    let s = setup("unresumed-twice");
    let id = session_claude_code_lost(&s).await;
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_FAIL_START".into(), "1".into());
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Error { .. }))
    })
    .await;
    until_stopped(&s.workbench, &id).await;
    // One resume, one fresh start, and no third try.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).len(), 3);
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert!(events.iter().any(|e| matches!(
        &e.event,
        WbEvent::Error { text, .. } if text == "Claude Code could not start (exit 1)."
    )));
    assert_eq!(
        turn_ends(&events),
        [TurnStatus::Completed, TurnStatus::Interrupted, TurnStatus::Error]
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Error);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Runs the stand-in with `HOME` at `<root>/home` and `CLAUDE_CONFIG_DIR` at its folder `profile`: it then keeps
/// its transcripts as Claude Code does. Returns the profile.
fn use_profile(s: &Setup, profile: &str) -> PathBuf {
    let home = s.root.join("home");
    let dir = home.join(profile);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    let mut extra = s.host.extra.lock().unwrap_or_else(|p| p.into_inner());
    extra.insert("HOME".into(), home.to_string_lossy().into_owned());
    extra.insert("CLAUDE_CONFIG_DIR".into(), dir.to_string_lossy().into_owned());
    dir
}

/// Claude Code's transcripts of conversation `native` under `profile`, in any project folder.
fn transcripts_of(profile: &std::path::Path, native: &str) -> Vec<PathBuf> {
    std::fs::read_dir(profile.join("projects"))
        .map(|dirs| {
            dirs.flatten()
                .map(|dir| dir.path().join(format!("{native}.jsonl")))
                .filter(|path| path.is_file())
                .collect()
        })
        .unwrap_or_default()
}

/// A session whose first turn ended and whose agent then stopped, with Nexus's copy of its transcript written. Returns
/// the session and its conversation's id.
async fn session_with_a_copy(s: &Setup) -> (String, String) {
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let first = s.host.launches.lock().unwrap_or_else(|p| p.into_inner())[0].clone();
    let native = launch_arg(&first, "--session-id").unwrap_or_else(|| panic!("no --session-id"));
    let copy = s.root.join("transcripts").join(&id).join(format!("{native}.jsonl"));
    for _ in 0..400 {
        if copy.is_file() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(std::fs::read_to_string(&copy).map(|t| t.lines().count()).ok(), Some(1));
    (id, native)
}

/// The next message resumes Claude Code's own conversation: its second message, and no notice of a fresh start.
async fn resumes_its_own_conversation(s: &Setup, id: &str, native: &str) {
    s.workbench.send(id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed, TurnStatus::Completed]);
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this session.")
    );
    assert!(
        !events.iter().any(
            |e| matches!(&e.event, WbEvent::Notice { text } if text == nexus_workbench::resume_recovery::UNRESUMED)
        )
    );
    let launches = s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!(launches.len(), 2);
    assert_eq!(launch_arg(&launches[1], "--resume").as_deref(), Some(native));
}

#[tokio::test]
async fn a_transcript_claude_code_lost_comes_back_from_nexuss_copy() {
    let s = setup("transcript-copy");
    let profile = use_profile(&s, ".claude");
    let (id, native) = session_with_a_copy(&s).await;
    for path in transcripts_of(&profile, &native) {
        std::fs::remove_file(path).unwrap_or_else(|e| panic!("{e}"));
    }
    resumes_its_own_conversation(&s, &id, &native).await;
    assert_eq!(transcripts_of(&profile, &native).len(), 1);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_transcript_under_another_profile_is_put_back() {
    let s = setup("transcript-profile");
    let first = use_profile(&s, ".claude-work");
    let (id, native) = session_with_a_copy(&s).await;
    std::fs::remove_dir_all(s.root.join("transcripts")).unwrap_or_else(|e| panic!("{e}"));
    // The app now starts Claude Code with the default profile, as one opened from the dock does.
    let second = use_profile(&s, ".claude");
    assert!(transcripts_of(&second, &native).is_empty());
    resumes_its_own_conversation(&s, &id, &native).await;
    assert_eq!(transcripts_of(&second, &native).len(), 1);
    assert_eq!(transcripts_of(&first, &native).len(), 1);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_transcript_under_an_old_project_folder_is_put_back() {
    let s = setup("transcript-moved");
    let profile = use_profile(&s, ".claude");
    let (id, native) = session_with_a_copy(&s).await;
    std::fs::remove_dir_all(s.root.join("transcripts")).unwrap_or_else(|e| panic!("{e}"));
    // Filed under the folder the session worked in before it moved.
    let old = profile.join("projects").join("-somewhere-else");
    std::fs::create_dir_all(&old).unwrap_or_else(|e| panic!("{e}"));
    for path in transcripts_of(&profile, &native) {
        std::fs::rename(&path, old.join(format!("{native}.jsonl"))).unwrap_or_else(|e| panic!("{e}"));
    }
    resumes_its_own_conversation(&s, &id, &native).await;
    assert_eq!(transcripts_of(&profile, &native).len(), 2);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn deleting_a_session_drops_nexuss_copy() {
    let s = setup("transcript-delete");
    use_profile(&s, ".claude");
    let (id, _) = session_with_a_copy(&s).await;
    s.workbench.delete_thread(&id).await.unwrap_or_else(|e| panic!("{e}"));
    assert!(!s.root.join("transcripts").join(&id).exists());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Waits until the session's agent is idle between turns, or gone.
async fn until_idle(workbench: &Workbench, id: &str) {
    for _ in 0..400 {
        if !workbench.is_live(id) || thread(workbench, id).status == ThreadStatus::Idle {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the agent of {id} is still busy");
}

#[tokio::test]
async fn saves_a_mode_change_only_once_claude_code_accepts_it() {
    let s = setup("mode");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    let patch = |mode| nexus_workbench::ThreadPatch {
        permission_mode: Some(mode),
        ..Default::default()
    };
    // Accepted: saved when the answer comes.
    s.workbench
        .update(&id, patch(PermissionMode::AcceptEdits))
        .unwrap_or_else(|e| panic!("{e}"));
    for _ in 0..200 {
        if thread(&s.workbench, &id).permission_mode == PermissionMode::AcceptEdits {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(thread(&s.workbench, &id).permission_mode, PermissionMode::AcceptEdits);
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;

    // Refused: the session keeps the mode the agent still runs in, and says why.
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_REJECT_MODES".into(), "1".into());
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    let summary = s
        .workbench
        .update(&id, patch(PermissionMode::BypassPermissions))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(summary.permission_mode, PermissionMode::AcceptEdits);
    wait_for(&s.workbench, &id, |e| {
        e.iter()
            .any(|e| matches!(&e.event, WbEvent::Notice { text } if text.contains("did not apply")))
    })
    .await;
    assert_eq!(thread(&s.workbench, &id).permission_mode, PermissionMode::AcceptEdits);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_change_the_agent_never_answered_applies_at_the_next_start() {
    let s = setup("unanswered");
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_IGNORE_CHANGES".into(), "1".into());
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench
        .update(
            &id,
            nexus_workbench::ThreadPatch {
                permission_mode: Some(PermissionMode::AcceptEdits),
                model: Some(Some("claude-other".into())),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    // Shown as pending while the agent has not answered.
    assert_eq!(
        thread(&s.workbench, &id).pending,
        Some(Pending {
            permission_mode: Some(PermissionMode::AcceptEdits),
            model: Some(Some("claude-other".into())),
            effort: None,
            fast: None,
        })
    );
    // The agent ends before it answers: the session keeps the change instead of dropping it.
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let summary = thread(&s.workbench, &id);
    assert_eq!(summary.permission_mode, PermissionMode::AcceptEdits);
    assert_eq!(summary.model.as_deref(), Some("claude-other"));
    assert_eq!(summary.pending, None);
    assert_eq!(last_shown(&s, &id).map(|t| t.pending), Some(None));
    // The next start runs with the kept change.
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    let launches = s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert!(
        launches[1]
            .args
            .windows(2)
            .any(|w| w == ["--permission-mode", "acceptEdits"])
    );
    assert!(launches[1].args.windows(2).any(|w| w == ["--model", "claude-other"]));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The last summary of session `id` the screen was sent.
fn last_shown(s: &Setup, id: &str) -> Option<ThreadSummary> {
    s.host
        .emitted
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .rev()
        .find_map(|emit| match emit {
            Emit::Thread(summary) if summary.id == id => Some((**summary).clone()),
            _ => None,
        })
}

/// Waits (up to ten seconds) until `read` gives a summary that satisfies `done`.
async fn until_thread(
    read: impl Fn() -> Option<ThreadSummary>,
    done: impl Fn(&ThreadSummary) -> bool,
) -> ThreadSummary {
    for _ in 0..400 {
        if let Some(summary) = read().filter(|summary| done(summary)) {
            return summary;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out; the session: {:#?}", read());
}

/// A mode, model and effort change made while the agent works show at once, as pending, in the answer
/// to the change and in the list; each is saved, and leaves the pending state, once the agent accepts it.
#[tokio::test]
async fn a_change_shows_as_pending_until_the_agent_accepts_it() {
    let s = setup("pending");
    let (id, _) = asking(&s, &[("FAKE_CLAUDE_ACK_DELAY_MS", "300")]).await;
    let summary = s
        .workbench
        .update(
            &id,
            nexus_workbench::ThreadPatch {
                permission_mode: Some(PermissionMode::Plan),
                model: Some(Some("claude-other".into())),
                effort: Some(Some(Effort::High)),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let waiting = Some(Pending {
        permission_mode: Some(PermissionMode::Plan),
        model: Some(Some("claude-other".into())),
        effort: Some(Some(Effort::High)),
        fast: None,
    });
    assert_eq!(summary.permission_mode, PermissionMode::Default);
    assert_eq!(summary.pending, waiting);
    assert_eq!(thread(&s.workbench, &id).pending, waiting);
    assert_eq!(last_shown(&s, &id).and_then(|t| t.pending), waiting);
    let accepted = |t: &ThreadSummary| {
        t.pending.is_none()
            && t.permission_mode == PermissionMode::Plan
            && t.model.as_deref() == Some("claude-other")
            && t.effort == Some(Effort::High)
    };
    until_thread(|| Some(thread(&s.workbench, &id)), accepted).await;
    until_thread(|| last_shown(&s, &id), accepted).await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A refused change leaves the pending state, the session keeps the value in force, and the log says
/// which change the agent refused. The refusal saves no value, so it shows the session again itself.
#[tokio::test]
async fn a_refused_change_goes_back_to_the_value_in_force() {
    let s = setup("pending-refused");
    let (id, _) = asking(
        &s,
        &[("FAKE_CLAUDE_ACK_DELAY_MS", "300"), ("FAKE_CLAUDE_REJECT_MODES", "1")],
    )
    .await;
    pick_mode(&s, &id, PermissionMode::BypassPermissions);
    assert_eq!(
        last_shown(&s, &id).and_then(|t| t.pending),
        Some(Pending {
            permission_mode: Some(PermissionMode::BypassPermissions),
            ..Default::default()
        })
    );
    let settled = |t: &ThreadSummary| t.pending.is_none() && t.permission_mode == PermissionMode::Default;
    until_thread(|| Some(thread(&s.workbench, &id)), settled).await;
    until_thread(|| last_shown(&s, &id), settled).await;
    wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| {
            matches!(&e.event, WbEvent::Notice { text }
                if text == "Claude Code did not apply the permission mode change: not allowed")
        })
    })
    .await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A session waiting on its question, with the stand-in's switches `env` set: the session and the request id.
async fn asking(s: &Setup, env: &[(&str, &str)]) -> (String, String) {
    {
        let mut extra = s.host.extra.lock().unwrap_or_else(|p| p.into_inner());
        extra.insert("FAKE_CLAUDE_MODE_SUGGESTION".into(), "1".into());
        extra.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    }
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "use a tool"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Approval { .. }))
    })
    .await;
    let request = events
        .iter()
        .find_map(|e| match &e.event {
            WbEvent::Approval { request_id, .. } => Some(request_id.clone()),
            _ => None,
        })
        .unwrap_or_default();
    (id, request)
}

fn pick_mode(s: &Setup, id: &str, mode: PermissionMode) {
    s.workbench
        .update(
            id,
            nexus_workbench::ThreadPatch {
                permission_mode: Some(mode),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
}

/// The picker's change goes to the agent first and an "allow for this
/// session" answer second, so the agent ends in the answer's mode; the change's late acknowledgement must not
/// overwrite it.
#[tokio::test]
async fn a_late_mode_acknowledgement_does_not_undo_a_newer_answer() {
    let s = setup("mode-order");
    let (id, request) = asking(&s, &[("FAKE_CLAUDE_ACK_DELAY_MS", "300")]).await;
    pick_mode(&s, &id, PermissionMode::Plan);
    s.workbench
        .approve(&id, &request, Decision::AllowSession, None, Some(0), None)
        .unwrap_or_else(|e| panic!("{e}"));
    // The picker's change, still unanswered, will never be saved over the answer's mode: it does not show as pending.
    let now = thread(&s.workbench, &id);
    assert_eq!((now.permission_mode, now.pending), (PermissionMode::AcceptEdits, None));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    // The acknowledgement of the picker's change lands now.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(thread(&s.workbench, &id).permission_mode, PermissionMode::AcceptEdits);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The agent exits with the picker's change unanswered, after a newer answer switched the mode.
/// The next start runs in the newer mode.
#[tokio::test]
async fn an_unanswered_change_does_not_undo_a_newer_answer_at_exit() {
    let s = setup("mode-exit");
    let (id, request) = asking(&s, &[("FAKE_CLAUDE_IGNORE_CHANGES", "1")]).await;
    pick_mode(&s, &id, PermissionMode::Plan);
    s.workbench
        .approve(&id, &request, Decision::AllowSession, None, Some(0), None)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    assert_eq!(thread(&s.workbench, &id).permission_mode, PermissionMode::AcceptEdits);
    let _ = std::fs::remove_dir_all(&s.root);
}

/// With a stop in between: a pick the agent never answered, then a stop, then a newer pick (saved at
/// once). The agent's exit must not bring the older pick back.
#[tokio::test]
async fn an_unanswered_pick_does_not_undo_a_pick_made_while_stopping() {
    let s = setup("mode-stopping");
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_IGNORE_CHANGES".into(), "1".into());
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    pick_mode(&s, &id, PermissionMode::Plan);
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    pick_mode(&s, &id, PermissionMode::AcceptEdits);
    until_stopped(&s.workbench, &id).await;
    assert_eq!(thread(&s.workbench, &id).permission_mode, PermissionMode::AcceptEdits);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_change_made_while_the_agent_stops_is_saved_at_once() {
    let s = setup("stopping-change");
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_IGNORE_CHANGES".into(), "1".into());
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    // The agent is on its way out (or already gone): nobody would answer the change, so it is saved now, not when
    // the process ends.
    let summary = s
        .workbench
        .update(
            &id,
            nexus_workbench::ThreadPatch {
                permission_mode: Some(PermissionMode::AcceptEdits),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        (summary.permission_mode, summary.pending),
        (PermissionMode::AcceptEdits, None)
    );
    assert_eq!(thread(&s.workbench, &id).permission_mode, PermissionMode::AcceptEdits);
    until_stopped(&s.workbench, &id).await;
    assert_eq!(thread(&s.workbench, &id).permission_mode, PermissionMode::AcceptEdits);
    // No agent at all: saved at once too, with nothing pending.
    let summary = s
        .workbench
        .update(
            &id,
            nexus_workbench::ThreadPatch {
                effort: Some(Some(Effort::Low)),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((summary.effort, summary.pending), (Some(Effort::Low), None));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn says_when_a_queued_message_was_never_read() {
    let s = setup("queued");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "slow, then dies"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Text { .. }))
    })
    .await;
    // Sent behind the running turn; the agent dies before it gets to it.
    s.workbench.queue(&id, "hello").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Error { .. }))
    })
    .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Notice { text } if text.contains("before it read your last message")))
    );
    until_stopped(&s.workbench, &id).await;
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Interrupted);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The number of the person's message `text` in the session's log.
fn user_seq(events: &[StoredEvent], text: &str) -> i64 {
    events
        .iter()
        .find(|e| matches!(&e.event, WbEvent::User { text: t, .. } if t == text))
        .map(|e| e.seq)
        .unwrap_or_else(|| panic!("no message {text:?}: {events:#?}"))
}

/// A session working on a `slow` turn, with `queued` sent behind it; and each queued message's number.
async fn busy_with_queue(s: &Setup, queued: &[&str]) -> (String, Vec<i64>) {
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "slow"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Text { .. }))
    })
    .await;
    for text in queued {
        s.workbench.queue(&id, text).await.unwrap_or_else(|e| panic!("{e}"));
    }
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    let seqs = queued.iter().map(|text| user_seq(&events, text)).collect();
    (id, seqs)
}

#[tokio::test]
async fn a_message_sent_as_queued_waits_for_the_turn_instead_of_joining_it() {
    let s = setup("queued-intent");
    let (id, _) = busy_with_queue(&s, &[]).await;
    s.workbench
        .send_message_as(&id, "after this turn", vec![], "", true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    let seq = user_seq(&events, "after this turn");
    // Not sent into the turn: no steer is logged, and it is still waiting, so it can be taken back.
    assert!(!events.iter().any(|e| e.event == WbEvent::QueueSteer { seq }));
    s.workbench.queue_cancel(&id, seq).unwrap_or_else(|e| panic!("{e}"));
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn ordinary_followups_reach_claude_at_the_next_tool_boundary() {
    let s = setup("auto-followup");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "queue-probe"))
        .await
        .expect("create")
        .id;
    wait_for(&s.workbench, &id, |events| {
        events
            .iter()
            .any(|event| matches!(&event.event, WbEvent::Tool { id, .. } if id.starts_with("probe-")))
    })
    .await;
    let sent = Instant::now();
    s.workbench.send(&id, "include the queued change").await.expect("send");
    let seq = user_seq(
        &s.workbench.events(&id, 0).expect("events"),
        "include the queued change",
    );
    let events = wait_for(&s.workbench, &id, |events| {
        events
            .iter()
            .any(|event| matches!(event.event, WbEvent::Read { seq: read } if read == seq))
    })
    .await;
    eprintln!("Claude follow-up read in {} ms", sent.elapsed().as_millis());
    assert!(turn_ends(&events).is_empty());
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Running);
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, WbEvent::ToolResult { id, .. } if id.starts_with("probe-")))
    );
    s.workbench.interrupt(&id).expect("interrupt");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn claude_burst_followups_keep_order_duplicates_images_and_receipts() {
    let s = setup("auto-burst");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "queue-probe"))
        .await
        .expect("create")
        .id;
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Tool { .. }))
    })
    .await;
    for n in 0..20 {
        let text = match n {
            0 | 1 => "same message".into(),
            2 => String::new(),
            _ => format!("follow-up {n}"),
        };
        let images = if n == 2 || n == 3 {
            vec![Image {
                media_type: "image/png".into(),
                data: "AAAA".into(),
                name: Some("queue.png".into()),
            }]
        } else {
            vec![]
        };
        s.workbench.send_message(&id, &text, images).await.expect("send");
    }
    let seqs: Vec<_> = s
        .workbench
        .events(&id, 0)
        .expect("events")
        .iter()
        .filter_map(|event| matches!(event.event, WbEvent::User { .. }).then_some(event.seq))
        .skip(1)
        .collect();
    let events = wait_for(&s.workbench, &id, |events| {
        seqs.iter().all(|seq| {
            events
                .iter()
                .any(|event| matches!(event.event, WbEvent::Read { seq: read } if read == *seq))
        }) && texts(events)
            .iter()
            .filter(|text| text.starts_with("Steered input: "))
            .count()
            == 20
    })
    .await;
    let receipts: Vec<_> = events
        .iter()
        .filter_map(|event| match event.event {
            WbEvent::Read { seq } if seqs.contains(&seq) => Some(seq),
            _ => None,
        })
        .collect();
    assert_eq!(receipts, seqs);
    assert_eq!(seqs.len(), 20);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::TurnStart { .. }))
            .count(),
        1
    );
    assert!(turn_ends(&events).is_empty());
    let inputs: Vec<serde_json::Value> = texts(&events)
        .iter()
        .filter_map(|text| text.strip_prefix("Steered input: "))
        .map(|text| serde_json::from_str(text).expect("input JSON"))
        .collect();
    assert_eq!(inputs[2][0]["type"], "image");
    assert_eq!(inputs[2][0]["source"]["data"], "AAAA");
    assert_eq!(inputs[3][0]["type"], "image");
    assert_eq!(inputs[3][1]["text"], "follow-up 3");
    s.workbench.interrupt(&id).expect("interrupt");
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    s.workbench.send(&id, "next normal turn").await.expect("next turn");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this session.")
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claude_followups_during_launch_are_read_once_in_order() {
    let s = setup("auto-startup");
    let (id, create) = launching(&s, "queue-probe").await;
    s.workbench.send(&id, "startup first").await.expect("first");
    s.workbench.send(&id, "startup second").await.expect("second");
    create.await.expect("create task").expect("create");
    let events = wait_for(&s.workbench, &id, |events| {
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::Read { .. }))
            .count()
            == 3
    })
    .await;
    assert!(turn_ends(&events).is_empty());
    let seqs: Vec<_> = events
        .iter()
        .filter_map(|event| matches!(event.event, WbEvent::User { .. }).then_some(event.seq))
        .collect();
    let receipts: Vec<_> = events
        .iter()
        .filter_map(|event| match event.event {
            WbEvent::Read { seq } => Some(seq),
            _ => None,
        })
        .collect();
    assert_eq!(receipts, seqs);
    s.workbench.interrupt(&id).expect("interrupt");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The person sends a follow-up after the run's first turn is recorded, before its opening message is on its way.
/// The agent must still read the opening message first: the follow-up joins that turn, it does not overtake it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_followup_in_the_first_turns_start_does_not_overtake_the_opening_message() {
    let s = setup("first-turn-gap");
    let (arrived, waiting) = tokio::sync::oneshot::channel();
    let gate = Arc::new(EmitGate {
        arrived: Mutex::new(Some(arrived)),
        released: Mutex::new(false),
        wake: Condvar::new(),
    });
    *s.host.first_turn_gate.lock().unwrap_or_else(|p| p.into_inner()) = Some((gate.clone(), None));
    let held = HeldEmit(gate);
    let (workbench, new) = (s.workbench.clone(), new_thread(&s.project, "queue-probe"));
    let create = tokio::spawn(async move { workbench.create_thread(new).await });
    // The first turn is recorded and its thread update is held.
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || waiting.blocking_recv()),
    )
    .await
    .expect("the first turn never reached emit")
    .expect("the emit wait panicked")
    .expect("the emit gate was dropped");
    let id = s
        .workbench
        .threads()
        .unwrap_or_default()
        .into_iter()
        .next()
        .expect("thread")
        .id;
    tokio::time::timeout(Duration::from_secs(5), s.workbench.send(&id, "follow-up in the gap"))
        .await
        .expect("the follow-up never returned")
        .expect("follow-up");
    drop(held);
    create.await.expect("create task").expect("create");
    let events = wait_for(&s.workbench, &id, |events| {
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::Read { .. }))
            .count()
            == 2
    })
    .await;
    let seqs: Vec<_> = events
        .iter()
        .filter_map(|event| matches!(event.event, WbEvent::User { .. }).then_some(event.seq))
        .collect();
    let receipts: Vec<_> = events
        .iter()
        .filter_map(|event| match event.event {
            WbEvent::Read { seq } => Some(seq),
            _ => None,
        })
        .collect();
    assert_eq!(receipts, seqs, "the follow-up overtook the opening message");
    assert!(turn_ends(&events).is_empty());
    s.workbench.interrupt(&id).expect("interrupt");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn claude_runs_a_followup_as_the_next_turn_when_no_step_remains() {
    let s = setup("auto-no-step");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "slow nostep"))
        .await
        .expect("create")
        .id;
    wait_for(&s.workbench, &id, |events| !texts(events).is_empty()).await;
    s.workbench
        .send(&id, "follow-up after the last step")
        .await
        .expect("send");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert!(
        texts(&events)
            .iter()
            .any(|text| text == "Heard next turn: follow-up after the last step")
    );
    let seq = user_seq(&events, "follow-up after the last step");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::Read { seq: read } if read == seq))
            .count(),
        1
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    assert_turns_nest(&events);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The person's message reached the turn under way, and the person stopped that turn before the agent took the
/// message in. Claude Code answers it after the stop, in a turn of its own: that turn is the person's, so the stop's
/// halt does not end it.
#[tokio::test]
async fn a_message_sent_into_a_turn_is_answered_after_the_person_stops_it() {
    let s = setup("steer-then-stop");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "slow nostep hold"))
        .await
        .expect("create")
        .id;
    wait_for(&s.workbench, &id, |events| !texts(events).is_empty()).await;
    s.workbench.send(&id, "read this before you stop").await.expect("send");
    let seq = user_seq(
        &s.workbench.events(&id, 0).expect("events"),
        "read this before you stop",
    );
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| event.event == WbEvent::QueueSteer { seq })
    })
    .await;
    s.workbench.interrupt(&id).expect("interrupt");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted, TurnStatus::Completed]);
    assert!(
        texts(&events)
            .iter()
            .any(|text| text == "Heard next turn: read this before you stop")
    );
    assert!(events.iter().any(|event| event.event == WbEvent::Read { seq }));
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    assert_turns_nest(&events);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// What Nexus tells the agent in the notice tests: no word the stand-in acts on.
const NOTICE: &str = "Your command finished.";

/// Nexus's own notice joins the turn under way at the agent's next step, as the person's message does. The session
/// shows it as Nexus's: a notice row, and no `User` row or steer of a queued message.
#[tokio::test]
async fn a_nexus_notice_reaches_claude_at_its_next_step() {
    let s = setup("notice-steer");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "slow"))
        .await
        .expect("create")
        .id;
    wait_for(&s.workbench, &id, |events| !texts(events).is_empty()).await;
    s.workbench.steer_notice(&id, NOTICE).expect("notice");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed]);
    assert!(
        texts(&events).contains(&format!("Heard mid-turn: {NOTICE}")),
        "{:?}",
        texts(&events)
    );
    assert_eq!(notices(&events), [NOTICE]);
    assert!(!events.iter().any(|event| is_user(event, NOTICE)));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.event, WbEvent::QueueSteer { .. }))
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Nexus's notice reached the turn under way, and the person stopped that turn before the agent took it in. The agent
/// answers it after the stop, in a turn of its own that is not the person's: the stop's halt holds and ends that turn.
/// The person's message would run on and lift the halt
/// (`a_message_sent_into_a_turn_is_answered_after_the_person_stops_it`).
#[tokio::test]
async fn a_nexus_notice_answered_after_a_stop_is_not_the_persons_turn() {
    let s = setup("notice-then-stop");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "slow nostep hold"))
        .await
        .expect("create")
        .id;
    wait_for(&s.workbench, &id, |events| !texts(events).is_empty()).await;
    s.workbench.steer_notice(&id, NOTICE).expect("notice");
    s.workbench.interrupt(&id).expect("interrupt");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert!(
        texts(&events).contains(&format!("Heard next turn: {NOTICE}")),
        "{:?}",
        texts(&events)
    );
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted, TurnStatus::Interrupted]);
    assert!(
        s.workbench.halt(&id).expect("halt").is_some(),
        "the notice lifted the halt"
    );
    assert!(!events.iter().any(|event| is_user(event, NOTICE)));
    assert_turns_nest(&events);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A notice that reaches the turn after its last step is answered in a turn the agent starts itself. That turn is
/// tracked like any other: it ends, and the session settles idle.
#[tokio::test]
async fn a_nexus_notice_after_the_last_step_runs_as_a_turn_of_its_own() {
    let s = setup("notice-no-step");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "slow nostep"))
        .await
        .expect("create")
        .id;
    wait_for(&s.workbench, &id, |events| !texts(events).is_empty()).await;
    s.workbench.steer_notice(&id, NOTICE).expect("notice");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed, TurnStatus::Completed]);
    assert!(
        texts(&events).contains(&format!("Heard next turn: {NOTICE}")),
        "{:?}",
        texts(&events)
    );
    assert_turns_nest(&events);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = s.workbench.events(&id, 0).expect("events");
    assert_eq!(turn_ends(&events).len(), 2);
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The person's Interrupt halts the session: Nexus's notice is refused, as a linked session's message is, and leaves no
/// row. With no turn under way and no halt, a notice starts a turn.
#[tokio::test]
async fn a_halted_session_refuses_a_nexus_notice_and_an_idle_one_takes_it() {
    let s = setup("notice-halted");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "slow"))
        .await
        .expect("create")
        .id;
    wait_for(&s.workbench, &id, |events| !texts(events).is_empty()).await;
    s.workbench.interrupt(&id).expect("interrupt");
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    let refused = s
        .workbench
        .steer_notice(&id, NOTICE)
        .expect_err("a halt refuses the notice");
    assert!(refused.contains("stopped"), "{refused}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = s.workbench.events(&id, 0).expect("events");
    assert!(notices(&events).iter().all(|text| text != NOTICE), "{events:#?}");
    assert_eq!(turn_ends(&events).len(), 1);
    assert!(s.workbench.halt(&id).expect("halt").is_some());

    // The person's message lifts the halt; once its turn ends, the notice starts one of its own.
    s.workbench.send(&id, "go on").await.expect("send");
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    s.workbench.steer_notice(&id, NOTICE).expect("notice");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 3).await;
    assert_eq!(turn_ends(&events)[2], TurnStatus::Completed);
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 3 of this session.")
    );
    assert_eq!(notices(&events), [NOTICE]);
    assert!(!events.iter().any(|event| is_user(event, NOTICE)));
    assert_turns_nest(&events);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The person queues a message while a turn runs, then presses Stop: the Stop ends only the turn under way, and the
/// queued message runs as the next turn. It was written before the Stop, but the person still wants it answered, as
/// the Claude Code extension in VS Code does.
#[tokio::test]
async fn a_message_queued_before_stop_runs_as_the_next_turn() {
    let s = setup("stop-keeps-queued");
    let (id, seqs) = busy_with_queue(&s, &["go on then"]).await;
    s.workbench.interrupt(&id).expect("interrupt");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted, TurnStatus::Completed]);
    assert!(texts(&events).iter().any(|text| text == "Message 2 of this session."));
    assert!(events.iter().any(|event| event.event == WbEvent::Read { seq: seqs[0] }));
    assert_eq!(
        s.workbench.halt(&id).expect("halt"),
        None,
        "the queued turn lifts the Stop's halt"
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    assert!(s.workbench.is_live(&id), "a Stop keeps the agent");
    assert_turns_nest(&events);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Stop, then Stop again while the queued message's turn runs: each Stop ends the turn under way, so both turns end
/// interrupted, and the agent keeps the conversation.
#[tokio::test]
async fn stop_pressed_twice_stops_the_queued_turn_too() {
    let s = setup("stop-twice");
    let (id, seqs) = busy_with_queue(&s, &["slow second"]).await;
    s.workbench.interrupt(&id).expect("first interrupt");
    // The agent has read the queued message: its turn is under way.
    let events = wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| event.event == WbEvent::Read { seq: seqs[0] })
    })
    .await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted]);
    s.workbench.interrupt(&id).expect("second interrupt");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted, TurnStatus::Interrupted]);
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    assert!(s.workbench.is_live(&id), "neither Stop ends the agent");
    assert_turns_nest(&events);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A repo whose checkpoints wait while the file `hold` exists in the setup's folder: git's `reference-transaction` hook
/// runs for the checkpoint's `update-ref`, writes `holding`, and waits. Returns both files' paths.
fn hold_checkpoints(s: &Setup) -> (PathBuf, PathBuf) {
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);
    let (hold, holding) = (s.root.join("hold"), s.root.join("holding"));
    let hook = repo.join(".git").join("hooks").join("reference-transaction");
    let script = format!(
        "#!/bin/sh\n[ \"$1\" = prepared ] || exit 0\ncat > /dev/null\n[ -e '{hold}' ] || exit 0\n: > '{holding}'\n\
         i=0; while [ -e '{hold}' ] && [ $i -lt 600 ]; do sleep 0.05; i=$((i+1)); done\n",
        hold = hold.display(),
        holding = holding.display(),
    );
    std::fs::write(&hook, script).unwrap_or_else(|e| panic!("{e}"));
    // Git for Windows runs a hook through sh and needs no mode bit.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap_or_else(|e| panic!("{e}"));
    }
    (hold, holding)
}

/// A turn ends while the person's queued message waits for that turn's checkpoint, and the person presses Stop then:
/// the Stop goes to the turn before, which has ended, and the queued message runs. Its turn is not interrupted the
/// moment the agent reads it, and is not stopped at the escalation either.
#[tokio::test]
async fn a_message_held_for_the_checkpoint_runs_when_stop_arrives() {
    let s = setup("stop-held");
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_SLOW_MS".into(), "1500".into());
    let (hold, holding) = hold_checkpoints(&s);
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "relay"))
        .await
        .expect("create")
        .id;
    wait_for(&s.workbench, &id, |events| {
        events
            .iter()
            .any(|event| matches!(event.event, WbEvent::TurnStart { .. }))
    })
    .await;
    s.workbench.queue(&id, "slow second").await.expect("queue");
    std::fs::write(&hold, "").unwrap_or_else(|e| panic!("{e}"));
    let staged = s.host.memory.join("relay.tmp");
    std::fs::write(&staged, "END").unwrap_or_else(|e| panic!("{e}"));
    std::fs::rename(&staged, s.host.memory.join("relay")).unwrap_or_else(|e| panic!("{e}"));
    for _ in 0..400 {
        if holding.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(holding.exists(), "the first turn's checkpoint never started");
    let events = s.workbench.events(&id, 0).expect("events");
    assert_eq!(turn_ends(&events), [TurnStatus::Completed]);
    s.workbench.interrupt(&id).expect("interrupt");
    std::fs::remove_file(&hold).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(
        turn_ends(&events),
        [TurnStatus::Completed, TurnStatus::Completed],
        "the queued turn runs to its end"
    );
    assert_eq!(
        s.workbench.halt(&id).expect("halt"),
        None,
        "the queued turn lifts the Stop's halt"
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    assert!(s.workbench.is_live(&id));
    assert_turns_nest(&events);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_queued_message_can_be_taken_back_or_rewritten_before_the_agent_reads_it() {
    let s = setup("queue-edit");
    let (id, seqs) = busy_with_queue(&s, &["first recall", "second recall"]).await;
    s.workbench.queue_cancel(&id, seqs[0]).unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .queue_edit(&id, seqs[1], "rewritten recall")
        .unwrap_or_else(|e| panic!("{e}"));
    // Taken back already: nothing left to take back or rewrite.
    assert!(s.workbench.queue_cancel(&id, seqs[0]).is_err());
    assert!(s.workbench.queue_edit(&id, seqs[0], "again").is_err());
    // An empty rewrite is no message.
    assert!(s.workbench.queue_edit(&id, seqs[1], "  ").is_err());
    // The turn under way is no queued message.
    let first = user_seq(&s.workbench.events(&id, 0).unwrap_or_default(), "slow");
    assert!(s.workbench.queue_cancel(&id, first).is_err());

    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted, TurnStatus::Completed]);
    // The agent read only the rewrite.
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this session. Heard: rewritten recall")
    );
    assert!(events.iter().any(|e| e.event == WbEvent::QueueCancel { seq: seqs[0] }));
    assert!(events.iter().any(|e| e.event
        == WbEvent::QueueEdit {
            seq: seqs[1],
            text: "rewritten recall".into()
        }));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(turn_ends(&s.workbench.events(&id, 0).unwrap_or_default()).len(), 2);
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_queued_message_moved_in_the_queue_reaches_the_agent_in_its_new_order() {
    let s = setup("queue-move");
    // The fake agent repeats a message that asks it to recall.
    let (id, seqs) = busy_with_queue(&s, &["one recall", "two recall", "three recall"]).await;
    s.workbench
        .queue_move(&id, seqs[2], Some(seqs[0]))
        .unwrap_or_else(|e| panic!("{e}"));
    // Only waiting messages move, and only next to another waiting one.
    let first = user_seq(&s.workbench.events(&id, 0).unwrap_or_default(), "slow");
    assert!(s.workbench.queue_move(&id, first, None).is_err());
    assert!(s.workbench.queue_move(&id, seqs[1], Some(first)).is_err());

    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 4).await;
    let heard: Vec<String> = texts(&events)
        .iter()
        .filter_map(|text| text.split_once("Heard: ").map(|(_, heard)| heard.to_string()))
        .collect();
    assert_eq!(heard, ["three recall", "one recall", "two recall"]);
    assert!(events.iter().any(|e| e.event
        == WbEvent::QueueMove {
            seq: seqs[2],
            before: Some(seqs[0])
        }));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_queue_held_for_a_rewrite_waits_and_goes_on_once_it_is_saved() {
    let s = setup("queue-hold");
    let (id, seqs) = busy_with_queue(&s, &["first recall", "second recall"]).await;
    // The person starts rewriting the second message: the queue holds, even for the first.
    s.workbench
        .queue_hold(&id, seqs[1], true)
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        turn_ends(&s.workbench.events(&id, 0).unwrap_or_default()).len(),
        1,
        "no queued message starts while one is being rewritten"
    );
    assert_eq!(
        thread(&s.workbench, &id).status,
        ThreadStatus::Idle,
        "a paused queue reads as idle"
    );
    // Still waiting: it can be rewritten, and the save lets the queue go on.
    s.workbench
        .queue_edit(&id, seqs[1], "rewritten recall")
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    let heard: Vec<String> = texts(&events)
        .iter()
        .filter_map(|text| text.split_once("Heard: ").map(|(_, heard)| heard.to_string()))
        .collect();
    assert_eq!(heard, ["first recall", "rewritten recall"]);
    assert_turns_nest(&events);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_paused_queue_sends_a_message_sent_now_and_holds_again_after_it() {
    let s = setup("queue-hold-steer");
    let (id, seqs) = busy_with_queue(&s, &["first recall", "second recall"]).await;
    s.workbench
        .queue_hold(&id, seqs[0], true)
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Send now on the second: it starts although the first is still being rewritten.
    s.workbench.queue_steer(&id, seqs[1]).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert!(
        texts(&events)
            .last()
            .is_some_and(|t| t.ends_with("Heard: second recall"))
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        turn_ends(&s.workbench.events(&id, 0).unwrap_or_default()).len(),
        2,
        "the first still waits for its rewrite"
    );
    // Cancel the rewrite: the first goes as it was.
    s.workbench
        .queue_hold(&id, seqs[0], false)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    assert!(
        texts(&events)
            .last()
            .is_some_and(|t| t.ends_with("Heard: first recall"))
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

fn notices(events: &[StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Notice { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_restart_ends_the_agent_says_so_and_the_next_message_resumes() {
    let s = setup("restart");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench.restart(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let events = s.workbench.events(&id, 0).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(notices(&events), [nexus_workbench::RESTARTED]);

    // Not running now: a second restart only says so again.
    s.workbench.restart(&id).unwrap_or_else(|e| panic!("{e}"));
    let events = s.workbench.events(&id, 0).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(notices(&events).len(), 2);

    // The next message starts a new process on the same conversation.
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    let launches = s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!(launches.len(), 2);
    assert!(launches[1].args.iter().any(|a| a == "--resume"));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A second Restart while the first one's process is still stopping adds no notice of its own: the first one's end
/// records it.
#[tokio::test]
async fn a_second_restart_while_the_first_is_stopping_adds_no_extra_notice() {
    let s = setup("restart-twice");
    let (id, _) = busy_with_queue(&s, &[]).await;
    s.workbench.restart(&id).unwrap_or_else(|e| panic!("{e}"));
    s.workbench.restart(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = s.workbench.events(&id, 0).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(notices(&events), [nexus_workbench::RESTARTED]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The same while the agent is still launching: the first Restart's notice goes in at once, and a second one before the
/// launch has ended adds none.
#[tokio::test]
async fn a_second_restart_while_launching_adds_no_extra_notice() {
    let s = setup("restart-twice-launching");
    let (id, create) = launching(&s, "hello").await;
    s.workbench.restart(&id).unwrap_or_else(|e| panic!("{e}"));
    s.workbench.restart(&id).unwrap_or_else(|e| panic!("{e}"));
    create
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    *s.host.launch_delay.lock().unwrap_or_else(|p| p.into_inner()) = None;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = s.workbench.events(&id, 0).unwrap_or_else(|e| panic!("{e}"));
    // The cancelled launch also says the message was not read; that one is the launch's, not a restart's.
    let restarted = notices(&events)
        .into_iter()
        .filter(|n| n == nexus_workbench::RESTARTED)
        .count();
    assert_eq!(restarted, 1, "{:?}", notices(&events));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Two Restarts at the same moment, from two threads: one notice, from the process end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_restarts_at_the_same_moment_add_one_notice() {
    let s = setup("restart-race");
    let (id, _) = busy_with_queue(&s, &[]).await;
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let both: Vec<_> = (0..2)
        .map(|_| {
            let (workbench, id, barrier) = (s.workbench.clone(), id.clone(), barrier.clone());
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                workbench.restart(&id)
            })
        })
        .collect();
    for one in both {
        one.await
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
    }
    until_stopped(&s.workbench, &id).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = s.workbench.events(&id, 0).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(notices(&events), [nexus_workbench::RESTARTED]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn send_now_gives_a_queued_message_to_the_turn_under_way() {
    let s = setup("queue-steer");
    let (id, seqs) = busy_with_queue(&s, &["not yet", "now please"]).await;
    s.workbench.queue_steer(&id, seqs[1]).unwrap_or_else(|e| panic!("{e}"));
    assert!(s.workbench.queue_steer(&id, seqs[1]).is_err());
    // The running turn took it in and ended; the other queued message then got its own turn.
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed, TurnStatus::Completed]);
    let said = texts(&events);
    assert!(said.iter().any(|t| t == "Heard mid-turn: now please"), "{said:?}");
    assert_eq!(said.last().map(String::as_str), Some("Message 2 of this session."));
    assert!(events.iter().any(|e| e.event == WbEvent::QueueSteer { seq: seqs[1] }));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert_eq!(turn_ends(&events).len(), 2);
    // Each message is confirmed read once the agent took it in: the steer mid-turn, after its step; the other with
    // its own turn.
    let read = |seq: i64| events.iter().position(|e| e.event == WbEvent::Read { seq });
    let steered = read(seqs[1]).unwrap_or_else(|| panic!("the steer was never confirmed read"));
    let heard = events
        .iter()
        .position(|e| matches!(&e.event, WbEvent::Text { text, .. } if text == "Heard mid-turn: now please"))
        .unwrap_or(0);
    assert!(steered < heard, "read at {steered}, answered at {heard}");
    assert!(read(seqs[0]).is_some());
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// "Send now" when the turn has no step left to take the message in: the agent ends the turn, then answers it as a turn
/// of its own, one `Result` more than the log has turns. The log's turn labels shift for a while (the steer's turn
/// is recorded as the next queued message's), but every turn ends and the session settles idle.
#[tokio::test]
async fn send_now_still_settles_when_the_agent_runs_it_as_a_turn_of_its_own() {
    let s = setup("queue-steer-fallthrough");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "slow nostep"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Text { .. }))
    })
    .await;
    s.workbench.queue(&id, "later").await.unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .queue(&id, "now please")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    s.workbench
        .queue_steer(&id, user_seq(&events, "now please"))
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed; 3]);
    let said = texts(&events);
    assert!(said.iter().any(|t| t == "Heard next turn: now please"), "{said:?}");
    assert!(
        said.iter().any(|t| t.starts_with("Message 2 of this session.")),
        "{said:?}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(turn_ends(&s.workbench.events(&id, 0).unwrap_or_default()).len(), 3);
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Where in the agent's replies the one that heard `text` is.
fn heard_at(said: &[String], text: &str) -> Option<usize> {
    said.iter().position(|t| t.ends_with(&format!("Heard: {text}")))
}

/// Signals that emit reached the held event, then waits for the test to release it.
struct EmitGate {
    arrived: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    released: Mutex<bool>,
    wake: Condvar,
}

impl EmitGate {
    fn hold(&self) {
        if let Some(arrived) = self.arrived.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = arrived.send(());
        }
        let released = self.released.lock().unwrap_or_else(|p| p.into_inner());
        drop(
            self.wake
                .wait_while(released, |released| !*released)
                .unwrap_or_else(|p| p.into_inner()),
        );
    }
}

/// Releases the runtime worker even if an assertion panics.
struct HeldEmit(Arc<EmitGate>);

impl Drop for HeldEmit {
    fn drop(&mut self) {
        *self.0.released.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.0.wake.notify_all();
    }
}

fn hold_interrupted_turn(host: &TestHost) -> (HeldEmit, tokio::sync::oneshot::Receiver<()>) {
    let (arrived, waiting) = tokio::sync::oneshot::channel();
    let gate = Arc::new(EmitGate {
        arrived: Mutex::new(Some(arrived)),
        released: Mutex::new(false),
        wake: Condvar::new(),
    });
    *host.interrupted_turn_gate.lock().unwrap_or_else(|p| p.into_inner()) = Some((gate.clone(), None));
    (HeldEmit(gate), waiting)
}

/// A turn ends with two messages queued; while the workbench is still writing that end, the person sends the second
/// one now. It must still reach the agent after the first, which left the queue before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_now_as_a_turn_ends_does_not_overtake_the_message_already_on_its_way() {
    let s = setup("queue-steer-turn-end");
    let (id, seqs) = busy_with_queue(&s, &["first recall", "second recall"]).await;
    let (held, arrived) = hold_interrupted_turn(&s.host);
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    // The turn ended and its thread update is held: queued messages have not reached the agent.
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || arrived.blocking_recv()),
    )
    .await
    .expect("the interrupted turn never reached emit")
    .expect("the emit wait panicked")
    .expect("the emit gate was dropped");
    s.workbench.queue_steer(&id, seqs[1]).unwrap_or_else(|e| panic!("{e}"));
    drop(held);
    let events = wait_for(&s.workbench, &id, |e| {
        let said = texts(e);
        heard_at(&said, "first recall").is_some() && heard_at(&said, "second recall").is_some()
    })
    .await;
    let said = texts(&events);
    assert!(
        heard_at(&said, "first recall") < heard_at(&said, "second recall"),
        "the message sent now overtook the one already on its way: {said:?}"
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The same "Send now" as a turn ends, then a stop before the queued messages go out: the agent never reads either,
/// and the session says so for the one sent now rather than drop it without a word.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_names_a_send_now_message_the_agent_never_read() {
    let s = setup("queue-steer-stop");
    let (id, seqs) = busy_with_queue(&s, &["first recall", "second recall"]).await;
    let (held, arrived) = hold_interrupted_turn(&s.host);
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || arrived.blocking_recv()),
    )
    .await
    .expect("the interrupted turn never reached emit")
    .expect("the emit wait panicked")
    .expect("the emit gate was dropped");
    s.workbench.queue_steer(&id, seqs[1]).unwrap_or_else(|e| panic!("{e}"));
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    drop(held);
    until_stopped(&s.workbench, &id).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    let said = texts(&events);
    assert_eq!(heard_at(&said, "second recall"), None, "{said:?}");
    assert!(
        events.iter().any(|e| matches!(&e.event, WbEvent::Notice { text } if text.contains("never read") || text.contains("stopped before it read"))),
        "nothing says the message sent now was never read: {events:#?}"
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn shows_only_the_current_organizations_sessions() {
    let s = setup("scope");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "slow"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Text { .. }))
    })
    .await;
    // The person switches organization while the session works.
    s.workbench.set_scope("gw|other");
    let before = s.host.emitted.lock().unwrap_or_else(|p| p.into_inner()).len();
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    let emitted = s.host.emitted.lock().unwrap_or_else(|p| p.into_inner())[before..].to_vec();
    assert!(
        emitted
            .iter()
            .all(|e| !matches!(e, Emit::Event(_) | Emit::Thread(_) | Emit::Delta(_))),
        "{emitted:#?}"
    );
    assert!(s.workbench.threads().unwrap_or_default().is_empty());
    s.workbench.set_scope("gw|org");
    assert_eq!(s.workbench.threads().unwrap_or_default().len(), 1);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_message_sent_while_stopping_starts_a_fresh_agent() {
    let s = setup("stopping");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hi"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;

    // Stop, then send at once: the message must reach a new agent, not the closed input of the one stopping.
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed, TurnStatus::Completed]);
    assert_eq!(s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).len(), 2);
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = process::command("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .status()
        .is_ok_and(|status| status.success());
    assert!(ok, "git {args:?}");
}

#[tokio::test]
async fn runs_a_session_in_its_own_worktree_and_removes_it_only_when_safe() {
    let s = setup("worktree");
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    git(&repo, &["commit", "--quiet", "--allow-empty", "-m", "one"]);
    let store = Store::open(&s.root.join("wt.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "gw|org".into(),
        worktrees: Some(s.root.join("worktrees")),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(store, s.host.clone(), config).unwrap_or_else(|e| panic!("{e}"));
    let project = workbench
        .add_project(&repo)
        .map(|p| p.id)
        .unwrap_or_else(|e| panic!("{e}"));

    let mut new = new_thread(&project, "Fix the login bug, please");
    new.worktree = Some(NewWorktree::default());
    let created = workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
    let worktree = created.worktree.clone().unwrap_or_else(|| panic!("a worktree"));
    assert!(
        worktree.branch.starts_with("nexus/fix-the-login-bug-please-"),
        "{}",
        worktree.branch
    );
    assert_eq!(worktree.base, "main");
    assert!(
        worktree
            .path
            .starts_with(&*s.root.join("worktrees/repo").to_string_lossy())
    );
    wait_for(&workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    let launched = s
        .host
        .launches
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .last()
        .cloned();
    assert_eq!(launched.map(|l| l.cwd), Some(PathBuf::from(&worktree.path)));

    // Running: refused. Stopped but holding a commit on no remote: refused, and the session still runs.
    let refused = workbench.remove_worktree(&created.id).await;
    assert!(refused.is_err());
    workbench.stop(&created.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&workbench, &created.id).await;
    let path = std::path::Path::new(&worktree.path);
    git(path, &["commit", "--quiet", "--allow-empty", "-m", "work"]);
    let unpushed = workbench.remove_worktree(&created.id).await.err().unwrap_or_default();
    assert!(unpushed.contains("aren't pushed"), "{unpushed}");

    // Pushed: removed, the session reads but no longer runs.
    let remote = s.root.join("remote.git");
    git(&s.root, &["init", "--quiet", "--bare", &remote.to_string_lossy()]);
    git(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
    git(path, &["push", "--quiet", "origin", "HEAD"]);
    let removed = workbench
        .remove_worktree(&created.id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(removed.worktree.is_some_and(|w| w.removed));
    assert!(!path.exists());
    assert!(workbench.send(&created.id, "again").await.is_err());

    // Without a worktrees folder the option is refused rather than silently ignored.
    let mut new = new_thread(&s.project, "elsewhere");
    new.worktree = Some(NewWorktree::default());
    assert!(s.workbench.create_thread(new).await.is_err());
    let _ = std::fs::remove_dir_all(&s.root);
}

async fn turns_recorded(workbench: &Workbench, id: &str, n: usize) -> Vec<i64> {
    for _ in 0..400 {
        let turns = workbench.changed_turns(id).await.unwrap_or_default();
        if turns.len() >= n {
            return turns;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{id} has fewer than {n} recorded turns");
}

#[tokio::test]
async fn shows_what_each_turn_changed_and_the_whole_session() {
    let s = setup("changes");
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);

    let created = s
        .workbench
        .create_thread(new_thread(&s.project, "hi"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    let first = turns_recorded(&s.workbench, &created.id, 1).await;

    // What changes before the next turn ends is that turn's (as the agent's own edits would be).
    std::fs::write(repo.join("a.txt"), "two\n").unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .send(&created.id, "again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 2).await;
    let turns = turns_recorded(&s.workbench, &created.id, 2).await;
    assert_eq!(turns[0], first[0]);

    let quiet = s
        .workbench
        .changes(&created.id, Some(turns[0]))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(quiet.diff, "");
    let second = s
        .workbench
        .changes(&created.id, Some(turns[1]))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(second.diff.contains("-one\n+two"), "{}", second.diff);
    let all = s
        .workbench
        .changes(&created.id, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(all.diff.contains("-one\n+two") && !all.truncated, "{}", all.diff);
    assert!(s.workbench.changes(&created.id, Some(999)).await.is_err());
    // The person's own branch and index are untouched.
    let log = process::command("git")
        .args(["-C", &repo.to_string_lossy(), "rev-list", "--count", "HEAD"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();
    assert_eq!(log, "1");
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn changes_read_as_the_turn_ends_wait_for_its_checkpoint() {
    let s = setup("changes-at-end");
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);

    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "write"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    // The crew's compare card reads a member's changes the moment its summary stops being busy, not once the
    // checkpoint's ref exists.
    for _ in 0..400 {
        let idle = s
            .workbench
            .threads()
            .unwrap_or_default()
            .iter()
            .any(|thread| thread.id == id && !thread.status.busy());
        if idle && !turn_ends(&s.workbench.events(&id, 0).unwrap_or_default()).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let all = s.workbench.changes(&id, None).await.unwrap_or_else(|e| panic!("{e}"));
    assert!(all.diff.contains("msg-1.txt"), "{}", all.diff);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_queued_message_waits_for_the_last_turns_checkpoint() {
    let s = setup("checkpoint-queue");
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);

    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "write pause"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    // Each round: a turn that writes msg-<n>.txt, and behind it a queued message whose turn writes the next file the
    // moment the agent reads it. A turn's checkpoint must hold its own file and not the queued turn's.
    let rounds = 3;
    for round in 0..rounds {
        let n = 2 * round + 1;
        if round > 0 {
            s.workbench
                .queue(&id, "write pause")
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        }
        wait_for(&s.workbench, &id, |_| repo.join(format!("msg-{n}.txt")).exists()).await;
        s.workbench.queue(&id, "write").await.unwrap_or_else(|e| panic!("{e}"));
        wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2 * (round + 1)).await;
    }
    let turns = turns_recorded(&s.workbench, &id, 2 * rounds).await;
    for (i, turn) in turns.iter().enumerate() {
        let diff = s
            .workbench
            .changes(&id, Some(*turn))
            .await
            .unwrap_or_else(|e| panic!("{e}"))
            .diff;
        let own = format!("msg-{}.txt", i + 1);
        let next = format!("msg-{}.txt", i + 2);
        assert!(diff.contains(&own), "turn {} lacks its file: {diff}", i + 1);
        assert!(
            !diff.contains(&next),
            "turn {} holds the next turn's file: {diff}",
            i + 1
        );
    }
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn archiving_a_session_drops_its_checkpoints() {
    let s = setup("archive-refs");
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hi"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&s.workbench, &id, 1).await;
    let refs = || {
        process::command("git")
            .args(["-C", &repo.to_string_lossy(), "for-each-ref", "--format=%(refname)"])
            .arg(format!("refs/nexus/checkpoints/{id}/"))
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).lines().count())
            .unwrap_or_default()
    };
    assert!(refs() >= 2, "the folder before the turn and after it");
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    // The app's `wb_thread_update` is a sync command on the main thread, which has no async runtime: archive from a
    // plain thread too.
    let wb = s.workbench.clone();
    let thread = id.clone();
    std::thread::spawn(move || {
        wb.update(
            &thread,
            nexus_workbench::ThreadPatch {
                archived: Some(true),
                ..Default::default()
            },
        )
        .map(|_| ())
    })
    .join()
    .unwrap_or_else(|_| panic!("archiving panicked outside an async runtime"))
    .unwrap_or_else(|e| panic!("{e}"));
    for _ in 0..200 {
        if refs() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(refs(), 0);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The refs under `prefix` in `repo`.
fn refs_in(repo: &std::path::Path, prefix: &str) -> usize {
    process::command("git")
        .args([
            "-C",
            &repo.to_string_lossy(),
            "for-each-ref",
            "--format=%(refname)",
            prefix,
        ])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).lines().count())
        .unwrap_or_default()
}

#[tokio::test]
async fn deleting_a_running_session_stops_it_and_leaves_the_folder() {
    let s = setup("delete");
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hi"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&s.workbench, &id, 1).await;
    let prefix = format!("refs/nexus/checkpoints/{id}/");
    assert!(refs_in(&repo, &prefix) >= 2);
    assert!(s.workbench.is_live(&id), "the agent waits for the next message");

    s.workbench.delete_thread(&id).await.unwrap_or_else(|e| panic!("{e}"));
    assert!(!s.workbench.is_live(&id));
    assert!(s.workbench.threads().unwrap_or_default().iter().all(|t| t.id != id));
    assert_eq!(s.workbench.events(&id, 0).unwrap_or_default(), vec![]);
    assert_eq!(refs_in(&repo, &prefix), 0);
    // The person's folder is theirs: untouched.
    assert_eq!(
        std::fs::read_to_string(repo.join("a.txt")).ok().as_deref(),
        Some("one\n")
    );
    // The Nexus session it ran in was ended.
    assert_eq!(s.host.ended.lock().unwrap_or_else(|p| p.into_inner()).len(), 1);
    assert!(s.workbench.delete_thread(&id).await.is_err(), "already gone");
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn deleting_a_dirty_worktree_requires_confirmation_and_preserves_local_commits() {
    let s = setup("delete-worktree");
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    git(&repo, &["commit", "--quiet", "--allow-empty", "-m", "one"]);
    let store = Store::open(&s.root.join("wt.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "gw|org".into(),
        worktrees: Some(s.root.join("worktrees")),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(store, s.host.clone(), config).unwrap_or_else(|e| panic!("{e}"));
    let project = workbench
        .add_project(&repo)
        .map(|p| p.id)
        .unwrap_or_else(|e| panic!("{e}"));
    let mut new = new_thread(&project, "Fix the login bug");
    new.worktree = Some(NewWorktree::default());
    let created = workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
    let worktree = created.worktree.clone().unwrap_or_else(|| panic!("a worktree"));
    wait_for(&workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    let path = std::path::Path::new(&worktree.path);
    let branch_exists = || {
        process::command("git")
            .args(["-C", &repo.to_string_lossy(), "rev-parse", "--verify", "--quiet"])
            .arg(format!("refs/heads/{}", worktree.branch))
            .status()
            .is_ok_and(|status| status.success())
    };

    // A commit on no remote: refused, and the session, its worktree and branch all stay.
    git(path, &["commit", "--quiet", "--allow-empty", "-m", "work"]);
    let refused = workbench.delete_thread(&created.id).await.err().unwrap_or_default();
    assert!(refused.contains("aren't pushed"), "{refused}");
    assert!(
        workbench
            .threads()
            .unwrap_or_default()
            .iter()
            .any(|t| t.id == created.id)
    );
    assert!(path.exists() && branch_exists());

    // Untracked files block normal deletion. A confirmed delete leaves the local commit on its branch.
    let draft = path.join("draft.txt");
    std::fs::write(&draft, "keep until confirmed").expect("write draft");
    assert_eq!(
        workbench
            .delete_thread(&created.id)
            .await
            .expect_err("normal deletion should refuse a dirty worktree"),
        nexus_workbench::git::WORKTREE_UNCOMMITTED
    );
    assert!(draft.exists() && branch_exists());

    let nested = path.join("nested");
    std::fs::create_dir(&nested).expect("nested repository directory");
    git(&nested, &["init", "--quiet"]);
    git(
        &nested,
        &["commit", "--quiet", "--allow-empty", "-m", "local nested history"],
    );
    // An initialized submodule can also have its own unsaved edits and local commits.
    let submodule_origin = s.root.join("submodule-origin");
    std::fs::create_dir(&submodule_origin).expect("create submodule origin");
    git(&submodule_origin, &["init", "--quiet"]);
    git(
        &submodule_origin,
        &["commit", "--quiet", "--allow-empty", "-m", "submodule base"],
    );
    git(
        path,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--quiet",
            submodule_origin.to_str().expect("UTF-8 submodule path"),
            "test-submodule",
        ],
    );
    let submodule = path.join("test-submodule");
    git(
        &submodule,
        &["commit", "--quiet", "--allow-empty", "-m", "submodule local commit"],
    );
    std::fs::write(submodule.join("scratch.txt"), "discard after confirmation").expect("write submodule draft");

    git(path, &["switch", "--detach", "--quiet"]);
    git(path, &["commit", "--quiet", "-m", "detached worktree changes"]);

    let side = workbench.side_chat(&created.id).await.expect("create side chat");
    let path_arg = path.to_str().expect("worktree path");
    git(&repo, &["worktree", "lock", path_arg]);
    assert!(workbench.delete_thread_with_options(&created.id, true).await.is_err());
    assert_eq!(workbench.folder(&side.id).expect("side chat still exists"), path);
    assert!(draft.exists() && nested.join(".git").exists() && submodule.join("scratch.txt").exists());
    git(&repo, &["worktree", "unlock", path_arg]);

    workbench
        .delete_thread_with_options(&created.id, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!path.exists());
    assert!(branch_exists());
    assert!(workbench.threads().unwrap_or_default().is_empty());
    assert_eq!(refs_in(&repo, &format!("refs/nexus/checkpoints/{}/", created.id)), 0);
    assert!(repo.exists());
    let _ = std::fs::remove_dir_all(&s.root);
}

/// On Windows a program the agent started holds its worktree open, and `git worktree remove` fails after it
/// has dropped git's record of the worktree. Once the agent is stopped and nothing would be lost, the session goes all
/// the same, and the delete says which folder stayed. A session an older build left that way deletes too. A read-only
/// folder stands in for the open file.
#[cfg(unix)]
#[tokio::test]
async fn a_worktree_folder_that_wont_go_does_not_keep_the_session() {
    use std::os::unix::fs::PermissionsExt;
    let s = setup("delete-stuck");
    let repo = s.root.join("repo");
    let remote = s.root.join("remote.git");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    git(&repo, &["commit", "--quiet", "--allow-empty", "-m", "one"]);
    git(&s.root, &["init", "--quiet", "--bare", &remote.to_string_lossy()]);
    git(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
    git(&repo, &["push", "--quiet", "origin", "main"]);
    let store = Store::open(&s.root.join("wt.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "gw|org".into(),
        worktrees: Some(s.root.join("worktrees")),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(store, s.host.clone(), config).unwrap_or_else(|e| panic!("{e}"));
    let project = workbench
        .add_project(&repo)
        .map(|p| p.id)
        .unwrap_or_else(|e| panic!("{e}"));
    let mut sessions = Vec::new();
    for title in ["Stuck before", "Locked now"] {
        let mut new = new_thread(&project, title);
        new.worktree = Some(NewWorktree::default());
        let created = workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
        wait_for(&workbench, &created.id, |e| turn_ends(e).len() == 1).await;
        let path = PathBuf::from(&created.worktree.unwrap_or_else(|| panic!("a worktree")).path);
        sessions.push((created.id, path));
    }
    let listed = |id: &str| workbench.threads().unwrap_or_default().iter().any(|t| t.id == id);

    // As an older build left it: git's record of the worktree is gone, its folder is not.
    let (id, path) = &sessions[0];
    let out = process::command("git")
        .args(["-C", &path.to_string_lossy(), "rev-parse", "--absolute-git-dir"])
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    std::fs::remove_dir_all(String::from_utf8_lossy(&out.stdout).trim()).unwrap_or_else(|e| panic!("{e}"));
    let left = workbench.delete_thread(id).await.unwrap_or_else(|e| panic!("{e}"));
    assert!(!listed(id));
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(left[0].contains(&path.display().to_string()), "{left:?}");
    assert!(path.exists(), "the app deletes only what git does");

    // A file git can't delete: the session goes, and the delete names the folder.
    let (id, path) = &sessions[1];
    let locked = path.join("locked");
    std::fs::create_dir(&locked).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(locked.join("a.txt"), "kept\n").unwrap_or_else(|e| panic!("{e}"));
    git(path, &["add", "."]);
    git(path, &["commit", "--quiet", "-m", "locked"]);
    git(path, &["push", "--quiet", "origin", "HEAD"]);
    let set_mode = |mode| std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(mode));
    set_mode(0o555).unwrap_or_else(|e| panic!("{e}"));
    // As root, a read-only folder is still writable: nothing to stand in for the open file.
    if std::fs::write(locked.join("probe"), "").is_err() {
        let left = workbench.delete_thread(id).await.unwrap_or_else(|e| panic!("{e}"));
        assert!(!listed(id));
        assert_eq!(left.len(), 1, "{left:?}");
        assert!(left[0].contains(&path.display().to_string()), "{left:?}");
    }
    let _ = set_mode(0o755);
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A workbench with a worktrees folder over `s`'s repository, made a git repository with one commit; and its project.
fn forking_bench(s: &Setup) -> (Workbench, String) {
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);
    let store = Store::open(&s.root.join("fork.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "gw|org".into(),
        worktrees: Some(s.root.join("worktrees")),
        accounts: Some(s.root.join("accounts")),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(store, s.host.clone(), config).unwrap_or_else(|e| panic!("{e}"));
    let project = workbench
        .add_project(&repo)
        .map(|p| p.id)
        .unwrap_or_else(|e| panic!("{e}"));
    (workbench, project)
}

fn last_args(s: &Setup) -> Vec<String> {
    s.host
        .launches
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .last()
        .map(|launch| launch.args.clone())
        .unwrap_or_default()
}

fn arg_after(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

#[tokio::test]
async fn forks_a_session_from_its_latest_turn_with_the_conversation() {
    let s = setup("fork-latest");
    let (workbench, project) = forking_bench(&s);
    let repo = s.root.join("repo");
    let source = workbench
        .create_thread(new_thread(&project, "write it"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &source.id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&workbench, &source.id, 1).await;
    let native = arg_after(&last_args(&s), "--session-id").unwrap_or_default();

    let fork = workbench
        .fork_thread(&source.id, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_ne!(fork.id, source.id);
    let worktree = fork.worktree.clone().unwrap_or_else(|| panic!("a worktree"));
    let dir = std::path::Path::new(&worktree.path);
    // The files as the turn left them, the agent's edit uncommitted; the project folder is not the fork's.
    assert!(dir.join("msg-1.txt").exists() && dir.join("a.txt").exists());
    assert_ne!(dir, repo.as_path());
    // The conversation so far shows in the fork, then a note that it is one.
    let copied = workbench.events(&fork.id, 0).unwrap_or_default();
    assert_eq!(texts(&copied), vec!["Message 1 of this session.".to_string()]);
    assert!(matches!(copied.last().map(|e| &e.event), Some(WbEvent::Notice { .. })));
    assert!(!workbench.is_live(&fork.id), "a fork waits for its first message");

    // Its first message forks Claude Code's own conversation: it goes on from message 1.
    workbench
        .send(&fork.id, "and then")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &fork.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this session.")
    );
    let args = last_args(&s);
    assert_eq!(arg_after(&args, "--resume"), Some(native.clone()));
    assert!(args.iter().any(|a| a == "--fork-session"));
    assert_eq!(
        s.host
            .launches
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .last()
            .map(|l| l.cwd.clone()),
        Some(dir.to_path_buf())
    );

    // Afterwards the fork resumes its own conversation, not the source's.
    workbench.stop(&fork.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&workbench, &fork.id).await;
    workbench
        .send(&fork.id, "again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &fork.id, |e| turn_ends(e).len() == 3).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 3 of this session.")
    );
    let args = last_args(&s);
    assert_eq!(arg_after(&args, "--resume"), Some(format!("{native}-forked")));
    assert!(!args.iter().any(|a| a == "--fork-session"));

    // The source goes on where it was.
    workbench
        .send(&source.id, "more")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &source.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this session.")
    );
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_fork_sends_its_result_back_to_its_source() {
    let s = setup("fork-send-back");
    let (workbench, project) = forking_bench(&s);
    let source = workbench
        .create_thread(new_thread(&project, "write it"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &source.id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&workbench, &source.id, 1).await;
    let not_a_fork = workbench.send_back(&source.id).await.err().unwrap_or_default();
    assert!(not_a_fork.contains("not a fork"), "{not_a_fork}");

    let fork = workbench
        .fork_thread(&source.id, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(fork.source_thread_id.as_deref(), Some(source.id.as_str()));
    // The answer copied from the source is not the fork's.
    let nothing = workbench.send_back(&fork.id).await.err().unwrap_or_default();
    assert!(nothing.contains("no answer"), "{nothing}");

    workbench
        .send(&fork.id, "and then")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &fork.id, |e| turn_ends(e).len() == 2).await;
    workbench.send_back(&fork.id).await.unwrap_or_else(|e| panic!("{e}"));
    // The source gets it as the person's message, and its agent answers it in a turn of its own.
    let events = wait_for(&workbench, &source.id, |e| turn_ends(e).len() == 2).await;
    let sent = events
        .iter()
        .rev()
        .find_map(|stored| match &stored.event {
            WbEvent::User { text, from: None, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let worktree = fork.worktree.clone().unwrap_or_else(|| panic!("a worktree"));
    assert!(
        sent.starts_with(&format!(
            "Result from the fork [{}](session:{}):\n\nMessage 2 of this session.",
            fork.title, fork.id
        )),
        "{sent}"
    );
    assert!(
        sent.contains(&format!("git -C {} diff {}", worktree.path, worktree.base)),
        "{sent}"
    );

    // Its source deleted, it has nowhere to send.
    workbench
        .delete_thread(&source.id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let gone = workbench.send_back(&fork.id).await.err().unwrap_or_default();
    assert!(gone.contains("is gone"), "{gone}");
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

fn launch_cwd(s: &Setup) -> Option<PathBuf> {
    s.host
        .launches
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .last()
        .map(|launch| launch.cwd.clone())
}

fn raise(mode: PermissionMode) -> nexus_workbench::ThreadPatch {
    nexus_workbench::ThreadPatch {
        permission_mode: Some(mode),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_side_chat_forks_the_session_in_its_folder_with_its_permissions() {
    let s = setup("side-chat");
    let (workbench, project) = forking_bench(&s);
    let repo = s.root.join("repo");
    let mut new = new_thread(&project, "write it");
    new.permission_mode = PermissionMode::BypassPermissions;
    let session = workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &session.id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&workbench, &session.id, 1).await;
    let native = arg_after(&last_args(&s), "--session-id").unwrap_or_default();

    let side = workbench.side_chat(&session.id).await.unwrap_or_else(|e| panic!("{e}"));
    // Tied to its session, flagged so the app keeps it out of the sidebar, with no worktree of its own.
    assert!(side.side_chat);
    assert_eq!(side.source_thread_id.as_deref(), Some(session.id.as_str()));
    assert_eq!(side.worktree, None);
    assert!(thread(&workbench, &side.id).side_chat);
    // It works in the session's folder, with the session's permissions.
    assert_eq!(workbench.folder(&side.id).ok(), Some(repo.clone()));
    assert_eq!(side.permission_mode, PermissionMode::BypassPermissions);
    // It starts with what the session knows.
    let copied = workbench.events(&side.id, 0).unwrap_or_default();
    assert_eq!(texts(&copied), vec!["Message 1 of this session.".to_string()]);
    // A side chat is no source for a fork or a side chat.
    let no_fork = workbench.fork_thread(&side.id, None).await.err().unwrap_or_default();
    assert!(no_fork.contains("can't be forked"), "{no_fork}");
    let no_side = workbench.side_chat(&side.id).await.err().unwrap_or_default();
    assert!(no_side.contains("no side chats"), "{no_side}");

    // The session idle, its first message forks the session's own conversation, in the session's folder.
    workbench
        .send(&side.id, "why that file?")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &side.id, |e| turn_ends(e).len() == 2).await;
    let args = last_args(&s);
    assert_eq!(arg_after(&args, "--resume"), Some(native));
    assert!(args.iter().any(|a| a == "--fork-session"));
    assert_eq!(
        arg_after(&args, "--permission-mode").as_deref(),
        Some("bypassPermissions")
    );
    assert_eq!(launch_cwd(&s), Some(repo.clone()));
    // It keeps no checkpoints: restoring the shared folder would undo the session's work.
    assert!(workbench.changed_turns(&side.id).await.unwrap_or_default().is_empty());
    // Its own composer's mode holds from then on.
    let lowered = workbench
        .update(&side.id, raise(PermissionMode::Default))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(lowered.permission_mode == PermissionMode::Default || lowered.pending.is_some());
    // The session is untouched.
    assert_eq!(
        turn_ends(&workbench.events(&session.id, 0).unwrap_or_default()).len(),
        1
    );
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_side_chat_forked_at_its_sessions_end_is_named_from_its_first_reply() {
    let s = setup("side-chat-title");
    let (workbench, project) = forking_bench(&s);
    let session = workbench
        .create_thread(new_thread(&project, "write it"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &session.id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&workbench, &session.id, 1).await;
    let side = workbench.side_chat(&session.id).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(side.title, nexus_workbench::side_chat::TITLE);

    // Its first message titles it the moment it is sent, before any reply.
    let untitled = workbench.side_chat(&session.id).await.unwrap_or_else(|e| panic!("{e}"));
    workbench
        .send(&untitled.id, "where is the config read?\nmore detail")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(thread(&workbench, &untitled.id).title, "where is the config read?");
    wait_for(&workbench, &untitled.id, |e| turn_ends(e).len() == 2).await;
    // An agent that skips the title line leaves the first message's title.
    assert_eq!(thread(&workbench, &untitled.id).title, "where is the config read?");

    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_TITLE".into(), "Why that file changed".into());

    // Its first start forks the session's conversation, and its agent still names it.
    workbench
        .send(&side.id, "why that file?")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &side.id, |e| turn_ends(e).len() == 2).await;
    assert!(last_args(&s).iter().any(|a| a == "--fork-session"));
    assert_eq!(thread(&workbench, &side.id).title, "Why that file changed");
    assert!(texts(&events).iter().all(|text| !text.contains("nexus-title")));
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_side_chat_on_a_running_session_brings_its_answer_back_into_the_queue() {
    let s = setup("side-chat-running");
    let (workbench, project) = forking_bench(&s);
    let session = workbench
        .create_thread(new_thread(&project, "write it"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &session.id, |e| turn_ends(e).len() == 1).await;
    workbench
        .send(&session.id, "slow work")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &session.id, |e| texts(e).len() == 2).await;

    // Mid-turn, the session's conversation can't fork: the side chat is told what was said up to its last finished
    // turn, as a new conversation.
    let side = workbench.side_chat(&session.id).await.unwrap_or_else(|e| panic!("{e}"));
    let copied = workbench.events(&side.id, 0).unwrap_or_default();
    assert_eq!(texts(&copied), vec!["Message 1 of this session.".to_string()]);
    let nothing = workbench.send_back(&side.id).await.err().unwrap_or_default();
    assert!(nothing.contains("no answer to bring back"), "{nothing}");
    workbench
        .send(&side.id, "what is it doing?")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &side.id, |e| turn_ends(e).len() == 2).await;
    let args = last_args(&s);
    assert!(!args.iter().any(|a| a == "--fork-session"));
    assert!(arg_after(&args, "--session-id").is_some());

    // Its first message titled it.
    let side = thread(&workbench, &side.id);
    assert_eq!(side.title, "what is it doing?");
    // Bring back waits behind the running turn: it never interrupts it.
    workbench.send_back(&side.id).await.unwrap_or_else(|e| panic!("{e}"));
    let events = workbench.events(&session.id, 0).unwrap_or_default();
    let brought = format!(
        "Result from the side chat [{}](session:{}):\n\nMessage 1 of this session.\n\nIt works in this session's \
         folder, so any files it changed are already changed here.",
        side.title, side.id
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::User { text, from: None, .. } if *text == brought)),
        "{events:#?}"
    );
    assert_eq!(turn_ends(&events).len(), 1, "the session's turn still runs");
    assert_eq!(thread(&workbench, &session.id).status, ThreadStatus::Running);
    // Once that turn ends, the session's agent answers it in a turn of its own.
    workbench.interrupt(&session.id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &session.id, |e| turn_ends(e).len() == 3).await;
    assert_eq!(
        turn_ends(&events),
        [TurnStatus::Completed, TurnStatus::Interrupted, TurnStatus::Completed]
    );
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn an_unused_side_chat_goes_when_closed_and_every_side_chat_goes_with_its_session() {
    let s = setup("side-chat-delete");
    let (workbench, project) = forking_bench(&s);
    let repo = s.root.join("repo");
    let session = workbench
        .create_thread(new_thread(&project, "write it"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &session.id, |e| turn_ends(e).len() == 1).await;
    let not_side = workbench.close_side_chat(&session.id).await.err().unwrap_or_default();
    assert!(not_side.contains("not a side chat"), "{not_side}");

    let unused = workbench.side_chat(&session.id).await.unwrap_or_else(|e| panic!("{e}"));
    assert!(
        workbench
            .close_side_chat(&unused.id)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    );
    assert!(
        workbench
            .threads()
            .unwrap_or_default()
            .iter()
            .all(|t| t.id != unused.id)
    );

    let used = workbench.side_chat(&session.id).await.unwrap_or_else(|e| panic!("{e}"));
    workbench
        .send(&used.id, "write a note")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &used.id, |e| turn_ends(e).len() == 2).await;
    // A side chat the person wrote in stays when closed; it has Delete.
    assert!(
        !workbench
            .close_side_chat(&used.id)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    );
    let kept = workbench.side_chat(&session.id).await.unwrap_or_else(|e| panic!("{e}"));

    // Deleting the session deletes its side chats, and their shared folder stays: it is the project's.
    workbench
        .delete_thread(&session.id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let left: Vec<String> = workbench
        .threads()
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(!left.contains(&used.id) && !left.contains(&kept.id), "{left:?}");
    assert!(!workbench.is_live(&used.id));
    assert!(repo.join("a.txt").exists());
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_side_chat_works_in_its_sessions_worktree_and_leaves_it_when_deleted() {
    let s = setup("side-chat-worktree");
    let (workbench, project) = forking_bench(&s);
    let source = workbench
        .create_thread(new_thread(&project, "write it"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &source.id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&workbench, &source.id, 1).await;
    // A fork has a worktree of its own: its side chat works there too.
    let fork = workbench
        .fork_thread(&source.id, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let worktree = PathBuf::from(fork.worktree.clone().unwrap_or_else(|| panic!("a worktree")).path);
    let side = workbench.side_chat(&fork.id).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(workbench.folder(&side.id).ok(), Some(worktree.clone()));
    workbench.send(&side.id, "look").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &side.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(launch_cwd(&s), Some(worktree.clone()));
    // Its agent works there: the worktree stays while it runs.
    let busy = workbench.remove_worktree(&fork.id).await.err().unwrap_or_default();
    assert!(busy.contains("side chats"), "{busy}");
    // The worktree is the fork's: deleting the side chat leaves it.
    workbench
        .delete_thread(&side.id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(worktree.exists());
    assert!(thread(&workbench, &fork.id).worktree.is_some_and(|w| !w.removed));
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn forks_a_session_from_an_earlier_turn_with_a_summary_of_the_conversation() {
    let s = setup("fork-earlier");
    let (workbench, project) = forking_bench(&s);
    let source = workbench
        .create_thread(new_thread(&project, "write the first file"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &source.id, |e| turn_ends(e).len() == 1).await;
    workbench
        .send(&source.id, "write the second file")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &source.id, |e| turn_ends(e).len() == 2).await;
    let turns = turns_recorded(&workbench, &source.id, 2).await;

    let fork = workbench
        .fork_thread(&source.id, Some(turns[0]))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let dir = PathBuf::from(fork.worktree.clone().map(|w| w.path).unwrap_or_default());
    assert!(dir.join("msg-1.txt").exists());
    assert!(
        !dir.join("msg-2.txt").exists(),
        "the second turn's file is not in the fork"
    );
    let copied = workbench.events(&fork.id, 0).unwrap_or_default();
    assert_eq!(texts(&copied), vec!["Message 1 of this session.".to_string()]);

    // A new conversation, told what was said up to that turn.
    workbench
        .send(&fork.id, "recall")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &fork.id, |e| turn_ends(e).len() == 2).await;
    let reply = texts(&events).last().cloned().unwrap_or_default();
    assert!(reply.starts_with("Message 1 of this session."), "{reply}");
    assert!(
        reply.contains("write the first file") && reply.contains("Message 1 of this session."),
        "{reply}"
    );
    assert!(!reply.contains("write the second file"), "{reply}");
    let args = last_args(&s);
    assert!(arg_after(&args, "--session-id").is_some() && !args.iter().any(|a| a == "--resume"));

    // No snapshot at that point: refused.
    assert!(workbench.fork_thread(&source.id, Some(999)).await.is_err());
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_fork_after_a_revert_leaves_the_reverted_turns_out() {
    let s = setup("fork-reverted");
    let (workbench, project) = forking_bench(&s);
    let source = workbench
        .create_thread(new_thread(&project, "write the first file"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &source.id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&workbench, &source.id, 1).await;
    workbench
        .send(&source.id, "write the second file")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &source.id, |e| turn_ends(e).len() == 2).await;
    let turns = turns_recorded(&workbench, &source.id, 2).await;
    assert_eq!(workbench.revert_to_turn(&source.id, turns[0]).await, Ok(1));

    // The reverted turn can't be forked from; "latest" means the latest one kept.
    assert!(workbench.fork_thread(&source.id, Some(turns[1])).await.is_err());
    let fork = workbench
        .fork_thread(&source.id, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let dir = PathBuf::from(fork.worktree.clone().map(|w| w.path).unwrap_or_default());
    assert!(dir.join("msg-1.txt").exists() && !dir.join("msg-2.txt").exists());
    let copied = workbench.events(&fork.id, 0).unwrap_or_default();
    assert_eq!(texts(&copied), vec!["Message 1 of this session.".to_string()]);
    assert!(!copied.iter().any(|e| matches!(e.event, WbEvent::Revert { .. })));

    // Claude Code's own conversation still has the reverted turn, so the fork is told what was said instead.
    workbench
        .send(&fork.id, "recall")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &fork.id, |e| turn_ends(e).len() == 2).await;
    let reply = texts(&events).last().cloned().unwrap_or_default();
    assert!(
        reply.contains("write the first file") && !reply.contains("write the second file"),
        "{reply}"
    );
    assert!(!last_args(&s).iter().any(|a| a == "--fork-session"));
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn deleting_a_session_whose_worktree_folder_was_removed_by_hand_still_works() {
    let s = setup("delete-worktree-gone");
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    git(&repo, &["commit", "--quiet", "--allow-empty", "-m", "one"]);
    let store = Store::open(&s.root.join("wt.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "gw|org".into(),
        worktrees: Some(s.root.join("worktrees")),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(store, s.host.clone(), config).unwrap_or_else(|e| panic!("{e}"));
    let project = workbench
        .add_project(&repo)
        .map(|p| p.id)
        .unwrap_or_else(|e| panic!("{e}"));
    let mut new = new_thread(&project, "Fix the login bug");
    new.worktree = Some(NewWorktree::default());
    let created = workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
    let worktree = created.worktree.clone().unwrap_or_else(|| panic!("a worktree"));
    wait_for(&workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    // Gone from the disk, but git still lists it: its branch counts as checked out until the worktree is pruned.
    std::fs::remove_dir_all(&worktree.path).unwrap_or_else(|e| panic!("{e}"));
    workbench
        .delete_thread(&created.id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(workbench.threads().unwrap_or_default().is_empty());
    let branch = process::command("git")
        .args(["-C", &repo.to_string_lossy(), "rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{}", worktree.branch))
        .status()
        .is_ok_and(|status| status.success());
    assert!(!branch);
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn moves_a_session_on_the_plan_to_the_account_with_the_most_left() {
    let s = setup("rotate");
    let (accounts, home) = (s.root.join("accounts"), s.root.join("home-claude"));
    let _ = std::fs::create_dir_all(&home);
    let extra =
        nexus_workbench::accounts::add(&accounts, SessionAgent::Claude, &home).unwrap_or_else(|e| panic!("{e}"));
    let plan = |account: Option<&str>, email: &str, session: f64| PlanUsage {
        agent: SessionAgent::Claude,
        account: account.map(String::from),
        email: Some(email.into()),
        plan: Some("Claude Max".into()),
        signed_in: true,
        active: false,
        limits: vec![Limit {
            models: Vec::new(),
            label: "Session".into(),
            used_percent: session,
            resets_at: None,
        }],
        resets: None,
        error: None,
        google_cloud: false,
        unavailable: false,
        fetched_at: None,
    };
    *s.host.plans.lock().unwrap_or_else(|p| p.into_inner()) = vec![
        plan(None, "work@example.com", 92.0),
        plan(Some(&extra), "me@example.com", 10.0),
    ];
    let config = Config {
        scope: "gw|org".into(),
        accounts: Some(accounts.clone()),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(
        Store::in_memory().unwrap_or_else(|e| panic!("{e}")),
        s.host.clone(),
        config,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let project = workbench
        .add_project(&s.root.join("repo"))
        .map(|p| p.id)
        .unwrap_or_default();
    let last_launch = || {
        s.host
            .launches
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .last()
            .cloned()
    };
    let notices = |events: &[StoredEvent]| -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match &e.event {
                WbEvent::Notice { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    };

    // Claude Code's own login is at 92%: the session starts on the account with room, and its log says why.
    let on_plan = NewThread {
        name: None,
        via: Via::Subscription,
        provider: None,
        ..new_thread(&project, "hello")
    };
    let id = workbench
        .create_thread(on_plan)
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(
        notices(&events),
        ["Running on me@example.com's Claude plan: work@example.com has used 92% of its session or weekly limit."]
    );
    assert_eq!(thread(&workbench, &id).account.as_deref(), Some(extra.as_str()));
    assert_eq!(
        last_launch().and_then(|l| l.account_dir),
        Some(accounts.join("claude").join(&extra))
    );
    assert_eq!(workbench.active_account(), Some(extra.clone()));

    // A call reports that account at 95% of its session limit: the next message resumes the conversation where more
    // is left, between turns.
    workbench.send(&id, "nearlimit").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &id, |e| turn_ends(e).len() == 2).await;
    // The app hears it too, for the account the session runs on (`wb:plan`).
    let reported = s
        .host
        .emitted
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .rev()
        .find_map(|emit| match emit {
            Emit::Plan(report) => Some(report.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no plan report"));
    assert_eq!(
        (reported.agent, reported.account.as_deref()),
        (SessionAgent::Claude, Some(extra.as_str()))
    );
    let used: Vec<(&str, f64)> = reported
        .limits
        .iter()
        .map(|limit| (limit.label.as_str(), limit.used_percent))
        .collect();
    assert_eq!(used, [("Session", 95.0), ("Weekly", 40.0)]);
    workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &id, |e| turn_ends(e).len() == 3).await;
    assert_eq!(
        notices(&events).last().map(String::as_str),
        Some("Running on work@example.com's Claude plan: me@example.com has used 95% of its session or weekly limit.")
    );
    assert_eq!(turn_ends(&events), [TurnStatus::Completed; 3]);
    let launch = last_launch().unwrap_or_else(|| panic!("no launch"));
    assert_eq!(launch.account_dir, None);
    assert!(launch.args.contains(&"--resume".to_string()));
    assert_eq!(thread(&workbench, &id).account, None);
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 3 of this session.")
    );

    // Through Gateway, accounts mean nothing.
    let gateway = workbench
        .create_thread(new_thread(&project, "hi"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(thread(&workbench, &gateway).account, None);
    assert_eq!(last_launch().and_then(|l| l.account_dir), None);

    // Auto-rotation off, on the account the person chose: a new session starts there at 95%, without a move.
    workbench.set_rotation(false);
    workbench.use_account(Some(extra.clone()));
    let chosen = workbench
        .create_thread(NewThread {
            name: None,
            via: Via::Subscription,
            provider: None,
            ..new_thread(&project, "chosen")
        })
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &chosen, |e| turn_ends(e).len() == 1).await;
    assert!(notices(&events).is_empty());
    assert_eq!(thread(&workbench, &chosen).account.as_deref(), Some(extra.as_str()));
    // The older session resumes on Claude Code's own login, and new sessions still start on the chosen account.
    workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    while workbench.is_live(&id) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    workbench.send(&id, "later").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &id, |e| turn_ends(e).len() == 4).await;
    assert_eq!(last_launch().and_then(|l| l.account_dir), None);
    assert_eq!(workbench.active_account(), Some(extra.clone()));

    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// What "Continue at reset" and archiving send through `wb_thread_update`.
fn continue_at_reset(on: bool) -> nexus_workbench::ThreadPatch {
    nexus_workbench::ThreadPatch {
        continue_at_reset: Some(on),
        ..Default::default()
    }
}

fn continues(events: &[StoredEvent]) -> usize {
    events
        .iter()
        .filter(|e| is_user(e, "Continue where you left off."))
        .count()
}

/// Waits until the session's `ends`-th turn has ended and its summary holds the plan's stop. The workbench stores the
/// turn's end before it saves the stop, so the app reads the stop from the summary (`wb:thread`), not from the event.
async fn wait_stopped(workbench: &Workbench, id: &str, ends: usize) -> ThreadSummary {
    wait_for(workbench, id, |e| {
        turn_ends(e).len() == ends && thread(workbench, id).limited.is_some()
    })
    .await;
    thread(workbench, id)
}

/// The fake's reset, `FAKE_CLAUDE_RESETS_AT`'s default, in milliseconds.
const RESET: i64 = 1_792_605_600_000;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_its_plan_stopped_continues_once_at_the_reset_even_after_a_restart() {
    let s = setup("limitreset");
    let on_plan = |text: &str| NewThread {
        name: None,
        via: Via::Subscription,
        provider: None,
        ..new_thread(&s.project, text)
    };
    let id = s
        .workbench
        .create_thread(on_plan("hitlimit"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    // No other account has room: the session offers Continue at reset, off until the person turns it on.
    let stopped = wait_stopped(&s.workbench, &id, 1).await;
    assert_eq!(
        turn_ends(&s.workbench.events(&id, 0).unwrap_or_default()),
        [TurnStatus::Error]
    );
    assert_eq!(
        stopped.limited,
        Some(LimitStop {
            resets_at: Some(RESET),
            scheduled: false
        })
    );
    s.workbench.continue_at_resets(RESET + 3_600_000).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(continues(&s.workbench.events(&id, 0).unwrap_or_default()), 0);

    // Turned on: saved without moving the session among the others, and nothing before the reset.
    let on = s
        .workbench
        .update(&id, continue_at_reset(true))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(on.limited.map(|stop| stop.scheduled), Some(true));
    assert_eq!(on.updated_at, stopped.updated_at);
    s.workbench.continue_at_resets(RESET).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(continues(&s.workbench.events(&id, 0).unwrap_or_default()), 0);

    // The app restarts before the reset: the choice is still there.
    s.workbench.shutdown().await;
    let workbench = open(&s.host, &s.db);
    assert_eq!(thread(&workbench, &id).limited.map(|stop| stop.scheduled), Some(true));

    // Past the reset, two timers at once: the session continues once, and the stop is gone.
    let after = RESET + 60_000;
    tokio::join!(workbench.continue_at_resets(after), workbench.continue_at_resets(after));
    let events = wait_for(&workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(continues(&events), 1);
    assert_eq!(turn_ends(&events), [TurnStatus::Error, TurnStatus::Completed]);
    assert_eq!(thread(&workbench, &id).limited, None);
    workbench.continue_at_resets(after + 3_600_000).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(continues(&workbench.events(&id, 0).unwrap_or_default()), 1);
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_archiving_or_turning_it_off_cancels_the_continue_at_reset() {
    let s = setup("limitcancel");
    let id = s
        .workbench
        .create_thread(NewThread {
            name: None,
            via: Via::Subscription,
            provider: None,
            ..new_thread(&s.project, "hitlimit")
        })
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_stopped(&s.workbench, &id, 1).await;
    let after = RESET + 60_000;
    let stop_again = |n: usize| {
        let workbench = s.workbench.clone();
        let id = id.clone();
        async move {
            workbench.send(&id, "hitlimit").await.unwrap_or_else(|e| panic!("{e}"));
            wait_stopped(&workbench, &id, n).await;
            workbench
                .update(&id, continue_at_reset(true))
                .unwrap_or_else(|e| panic!("{e}"));
        }
    };

    // Turned off again.
    s.workbench
        .update(&id, continue_at_reset(true))
        .unwrap_or_else(|e| panic!("{e}"));
    let off = s
        .workbench
        .update(&id, continue_at_reset(false))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(off.limited.map(|stop| stop.scheduled), Some(false));
    s.workbench.continue_at_resets(after).await;

    // The person writes: their message takes the Continue's place.
    stop_again(2).await;
    s.workbench.send(&id, "hello").await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(thread(&s.workbench, &id).limited, None);
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    s.workbench.continue_at_resets(after).await;

    // Archived.
    stop_again(4).await;
    let archived = s
        .workbench
        .update(
            &id,
            nexus_workbench::ThreadPatch {
                archived: Some(true),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(archived.limited, None);
    s.workbench.continue_at_resets(after).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert_eq!(continues(&events), 0);
    assert_eq!(turn_ends(&events).len(), 4);

    // Nothing to continue after: the switch is refused.
    let error = s.workbench.update(&id, continue_at_reset(true)).err();
    assert!(error.is_some_and(|e| e.contains("not waiting")));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn another_claude_account_with_room_goes_before_the_continue_at_reset() {
    let s = setup("limitrotate");
    let (accounts, home) = (s.root.join("accounts"), s.root.join("home-claude"));
    let _ = std::fs::create_dir_all(&home);
    let extra =
        nexus_workbench::accounts::add(&accounts, SessionAgent::Claude, &home).unwrap_or_else(|e| panic!("{e}"));
    let plan = |account: Option<&str>, email: &str, session: f64| PlanUsage {
        agent: SessionAgent::Claude,
        account: account.map(String::from),
        email: Some(email.into()),
        plan: Some("Claude Max".into()),
        signed_in: true,
        active: false,
        limits: vec![Limit {
            models: Vec::new(),
            label: "Session".into(),
            used_percent: session,
            resets_at: None,
        }],
        resets: None,
        error: None,
        google_cloud: false,
        unavailable: false,
        fetched_at: None,
    };
    *s.host.plans.lock().unwrap_or_else(|p| p.into_inner()) = vec![
        plan(None, "work@example.com", 50.0),
        plan(Some(&extra), "me@example.com", 10.0),
    ];
    let config = Config {
        scope: "gw|org".into(),
        accounts: Some(accounts.clone()),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(
        Store::in_memory().unwrap_or_else(|e| panic!("{e}")),
        s.host.clone(),
        config,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let project = workbench
        .add_project(&s.root.join("repo"))
        .map(|p| p.id)
        .unwrap_or_default();
    let id = workbench
        .create_thread(NewThread {
            name: None,
            via: Via::Subscription,
            provider: None,
            ..new_thread(&project, "hitlimit")
        })
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    // Claude Code's own login is used up, and the other account has room: only Continue now, which moves there.
    let stopped = wait_stopped(&workbench, &id, 1).await;
    assert_eq!(stopped.account, None);
    assert_eq!(
        stopped.limited,
        Some(LimitStop {
            resets_at: None,
            scheduled: false
        })
    );
    assert!(workbench.update(&id, continue_at_reset(true)).is_err());
    workbench.resume(&id).await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Error, TurnStatus::Completed]);
    let moved = thread(&workbench, &id);
    assert_eq!(moved.account.as_deref(), Some(extra.as_str()));
    assert_eq!(moved.limited, None);
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The person chose another account in Settings after the session hit its limit. Continuing moves the open
/// session there, not to the account with the most left.
#[tokio::test]
async fn a_used_up_session_continues_on_the_account_the_person_chose() {
    let s = setup("limitchosen");
    let (accounts, home) = (s.root.join("accounts"), s.root.join("home-claude"));
    let _ = std::fs::create_dir_all(&home);
    let add =
        || nexus_workbench::accounts::add(&accounts, SessionAgent::Claude, &home).unwrap_or_else(|e| panic!("{e}"));
    let chosen = add();
    // An account's id is the time in milliseconds it was added.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let emptiest = add();
    let plan = |account: Option<&str>, email: &str, session: f64| PlanUsage {
        agent: SessionAgent::Claude,
        account: account.map(String::from),
        email: Some(email.into()),
        plan: Some("Claude Max".into()),
        signed_in: true,
        active: false,
        limits: vec![Limit {
            models: Vec::new(),
            label: "Session".into(),
            used_percent: session,
            resets_at: None,
        }],
        resets: None,
        error: None,
        google_cloud: false,
        unavailable: false,
        fetched_at: None,
    };
    *s.host.plans.lock().unwrap_or_else(|p| p.into_inner()) = vec![
        plan(None, "work@example.com", 50.0),
        plan(Some(&chosen), "me@example.com", 30.0),
        plan(Some(&emptiest), "spare@example.com", 10.0),
    ];
    let config = Config {
        scope: "gw|org".into(),
        accounts: Some(accounts.clone()),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(
        Store::in_memory().unwrap_or_else(|e| panic!("{e}")),
        s.host.clone(),
        config,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let project = workbench
        .add_project(&s.root.join("repo"))
        .map(|p| p.id)
        .unwrap_or_default();
    let id = workbench
        .create_thread(NewThread {
            name: None,
            via: Via::Subscription,
            provider: None,
            ..new_thread(&project, "hitlimit")
        })
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    let stopped = wait_stopped(&workbench, &id, 1).await;
    assert_eq!(stopped.account, None);

    workbench.use_account(Some(chosen.clone()));
    workbench.resume(&id).await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Error, TurnStatus::Completed]);
    assert_eq!(thread(&workbench, &id).account.as_deref(), Some(chosen.as_str()));
    let launch = s
        .host
        .launches
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("no launch"));
    assert_eq!(launch.account_dir, Some(accounts.join("claude").join(&chosen)));
    assert!(launch.args.contains(&"--resume".to_string()));
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The turn events in order make sense: a turn starts only when none is open, and only the open one ends.
fn assert_turns_nest(events: &[StoredEvent]) {
    let mut open: Option<&str> = None;
    for e in events {
        match &e.event {
            WbEvent::TurnStart { turn_id } => {
                assert!(open.is_none(), "turn {turn_id} starts inside {open:?}: {events:#?}");
                open = Some(turn_id);
            }
            WbEvent::TurnEnd { turn_id, .. } => {
                assert_eq!(open, Some(turn_id.as_str()), "{events:#?}");
                open = None;
            }
            _ => {}
        }
    }
}

fn is_user(e: &StoredEvent, text: &str) -> bool {
    matches!(&e.event, WbEvent::User { text: t, .. } if t == text)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_sent_while_a_crashed_agent_cleans_up_starts_after_it() {
    let s = setup("cleanup");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;

    // The agent crashes, and its clean-up is held up right after it ended the turn.
    *s.host.stall_failed_turn.lock().unwrap_or_else(|p| p.into_inner()) = Some(Duration::from_millis(500));
    s.workbench
        .send(&id, "crash now")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).contains(&TurnStatus::Error)).await;

    // A message sent now waits for the clean-up, then starts the next agent.
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    // Long enough for a clean-up still under way to write what it has left.
    tokio::time::sleep(Duration::from_millis(700)).await;
    let events = s.workbench.events(&id, 0).unwrap_or(events);
    assert_turns_nest(&events);
    let crash = events
        .iter()
        .position(|e| matches!(&e.event, WbEvent::Error { text, .. } if text.contains("stopped unexpectedly")))
        .unwrap_or_else(|| panic!("no crash error: {events:#?}"));
    let again = events
        .iter()
        .position(|e| is_user(e, "again"))
        .unwrap_or_else(|| panic!("no second message"));
    assert!(
        crash < again,
        "the old agent wrote after the new one started: {events:#?}"
    );
    assert_eq!(
        turn_ends(&events),
        [TurnStatus::Completed, TurnStatus::Error, TurnStatus::Completed]
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    assert!(s.workbench.is_live(&id));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Starts a session whose launch takes a while, and returns its id while the launch is under way.
async fn launching(s: &Setup, text: &str) -> (String, tokio::task::JoinHandle<Result<ThreadSummary, String>>) {
    *s.host.launch_delay.lock().unwrap_or_else(|p| p.into_inner()) = Some(Duration::from_millis(400));
    let (workbench, new) = (s.workbench.clone(), new_thread(&s.project, text));
    let create = tokio::spawn(async move { workbench.create_thread(new).await });
    for _ in 0..200 {
        if let Some(summary) = s.workbench.threads().unwrap_or_default().into_iter().next()
            && s.workbench.is_live(&summary.id)
        {
            assert!(s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).len() <= 1);
            return (summary.id, create);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the session did not start");
}

/// A stop or an interrupt while the agent starts: the agent never runs the message, the Nexus session the launch
/// opened is ended, and the next message starts a fresh agent.
async fn cancels_the_launch(name: &str, cancel: impl Fn(&Workbench, &str)) {
    let s = setup(name);
    let (id, create) = launching(&s, "hello").await;
    cancel(&s.workbench, &id);
    create
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted]);
    assert!(texts(&events).is_empty(), "the agent ran the message: {events:#?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Notice { text } if text.contains("before it read your last message")))
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    // The Nexus session is ended if the launch got as far as opening it (the stop may land before it asks).
    let ended = s.host.ended.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let launched = s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).len();
    let expected: Vec<String> = (1..=launched).map(|n| format!("ns_{n}")).collect();
    assert_eq!(
        ended.iter().map(|(session, _)| session.clone()).collect::<Vec<_>>(),
        expected
    );

    *s.host.launch_delay.lock().unwrap_or_else(|p| p.into_inner()) = None;
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_turns_nest(&events);
    assert_eq!(texts(&events), ["Message 1 of this session."]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_while_the_agent_starts_cancels_it() {
    cancels_the_launch("launch-stop", |workbench, id| {
        workbench.stop(id).unwrap_or_else(|e| panic!("{e}"))
    })
    .await;
}

/// A stop while the agent starts, and then the launch fails: the session reads as stopped, not as an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_wins_over_a_launch_that_then_fails() {
    let s = setup("launch-stop-fail");
    *s.host.launch_error.lock().unwrap_or_else(|p| p.into_inner()) = Some("Gateway said no".into());
    let (id, create) = launching(&s, "hello").await;
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    create
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted]);
    assert!(
        !events.iter().any(|e| matches!(e.event, WbEvent::Error { .. })),
        "a stopped launch shows no error: {events:#?}"
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Removing a project stops none of its agents: a running one still takes the next message. With none running, the
/// message fails before it is logged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_projects_running_agent_still_takes_messages() {
    let s = setup("removed-project");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench.remove_project(&s.project).unwrap_or_else(|e| panic!("{e}"));
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed, TurnStatus::Completed]);

    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let before = s.workbench.events(&id, 0).unwrap_or_default().len();
    let error = s.workbench.send(&id, "once more").await;
    assert_eq!(error, Err("This session's project is no longer open.".to_string()));
    assert_eq!(
        s.workbench.events(&id, 0).unwrap_or_default().len(),
        before,
        "nothing logged"
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interrupt_while_the_agent_starts_cancels_it() {
    cancels_the_launch("launch-interrupt", |workbench, id| {
        workbench.interrupt(id).unwrap_or_else(|e| panic!("{e}"))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_start_says_which_queued_messages_were_never_read() {
    let s = setup("launch-fail");
    *s.host.launch_error.lock().unwrap_or_else(|p| p.into_inner()) = Some("Gateway is down.".into());
    let (id, create) = launching(&s, "hello").await;
    // Two more messages queue behind the first while the agent starts.
    s.workbench.send(&id, "second").await.unwrap_or_else(|e| panic!("{e}"));
    s.workbench.send(&id, "third").await.unwrap_or_else(|e| panic!("{e}"));
    create
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert_turns_nest(&events);
    assert_eq!(turn_ends(&events), [TurnStatus::Error]);
    assert!(
        events.iter().any(
            |e| matches!(&e.event, WbEvent::Notice { text } if text.contains("could not start, so it did not read your last 2 messages"))
        ),
        "{events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Error { text, .. } if text == "Gateway is down."))
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Error);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Whether the process `pid` still runs (a zombie waiting to be reaped does not).
#[cfg(unix)]
fn running(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // The state follows the name, which is in parentheses.
        Ok(stat) => !stat
            .rsplit(')')
            .next()
            .is_some_and(|rest| rest.trim_start().starts_with('Z')),
        Err(_) if cfg!(target_os = "linux") => false,
        Err(_) => nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok(),
    }
}

#[cfg(unix)]
fn kill_pid(pid: i32) {
    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGKILL);
}

/// A session whose agent starts a process that outlives it (fake-claude's `orphan`), then crashes: the session and
/// the leftover process's pid, once the crash is in the log.
#[cfg(unix)]
async fn crash_leaving_a_process(name: &str, text: &str) -> (Setup, String, i32) {
    let s = setup(name);
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, text))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::Error { .. }))
    })
    .await;
    let pid = std::fs::read_to_string(s.root.join("repo/orphan.pid"))
        .ok()
        .and_then(|pid| pid.trim().parse().ok())
        .unwrap_or_else(|| panic!("fake-claude wrote no orphan.pid"));
    (s, id, pid)
}

#[cfg(unix)]
#[tokio::test]
async fn a_crashed_agents_leftover_processes_end_with_it() {
    let (s, id, pid) = crash_leaving_a_process("orphan", "orphan").await;
    until_stopped(&s.workbench, &id).await;
    let mut ended = false;
    for _ in 0..80 {
        if !running(pid) {
            ended = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    kill_pid(pid);
    assert!(ended, "the agent's leftover process {pid} still runs");
    let _ = std::fs::remove_dir_all(&s.root);
}

#[cfg(unix)]
#[tokio::test]
async fn a_crash_is_reported_even_while_a_leftover_process_holds_the_agents_output() {
    // A process the agent started keeps its stdout open: the crash must not wait for that process to end (30 s).
    let (s, id, pid) = crash_leaving_a_process("holdout", "orphan holdout").await;
    until_stopped(&s.workbench, &id).await;
    let status = thread(&s.workbench, &id).status;
    kill_pid(pid);
    assert_eq!(status, ThreadStatus::Interrupted);
    let _ = std::fs::remove_dir_all(&s.root);
}

#[cfg(unix)]
#[tokio::test]
async fn a_crash_is_reported_when_a_process_out_of_the_agents_group_holds_its_output() {
    // The leftover left the agent's process group, so ending the group misses it: only the drain deadline ends the
    // read, in about `DRAIN`, not when the leftover ends (30 s).
    let started = std::time::Instant::now();
    let (s, id, pid) = crash_leaving_a_process("detach", "orphan holdout detach").await;
    until_stopped(&s.workbench, &id).await;
    let took = started.elapsed();
    let status = thread(&s.workbench, &id).status;
    let still = running(pid);
    kill_pid(pid);
    assert!(still, "the detached process {pid} should be out of the stop's reach");
    assert_eq!(status, ThreadStatus::Interrupted);
    assert!(took < Duration::from_secs(8), "the crash took {took:?} to show");
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn reverts_to_an_earlier_turn_and_the_agent_forgets_the_later_ones() {
    let s = setup("revert");
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);

    // Three turns, each writing a file of its own (`write`: msg-<n>.txt).
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "write first"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    for (n, text) in [(2, "write second"), (3, "write third")] {
        turns_recorded(&s.workbench, &id, n - 1).await;
        s.workbench.send(&id, text).await.unwrap_or_else(|e| panic!("{e}"));
        wait_for(&s.workbench, &id, |e| turn_ends(e).len() == n).await;
    }
    let turns = turns_recorded(&s.workbench, &id, 3).await;
    // Each turn end keeps where Claude Code's conversation stood: its answer's message.
    let cursors: Vec<String> = s
        .workbench
        .events(&id, 0)
        .unwrap_or_default()
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::TurnEnd { cursor, .. } => cursor.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(cursors.len(), 3);
    assert!(
        cursors[0].ends_with("-a1") && cursors[2].ends_with("-a3"),
        "{cursors:?}"
    );

    assert_eq!(s.workbench.revert_preview(&id, turns[0]).await, Ok(2));
    assert_eq!(s.workbench.revert_to_turn(&id, turns[0]).await, Ok(2));
    assert!(repo.join("msg-1.txt").exists());
    assert!(!repo.join("msg-2.txt").exists() && !repo.join("msg-3.txt").exists());
    assert!(
        !s.workbench.is_live(&id),
        "the agent stops, so it starts again from the reverted point"
    );
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert!(
        matches!(events.last().map(|e| &e.event), Some(WbEvent::Revert { turn, files: 2 }) if *turn == turns[0]),
        "{events:#?}"
    );
    // A reverted turn can't be reverted to, nor an event that is no turn's end.
    assert!(s.workbench.revert_to_turn(&id, turns[1]).await.is_err());
    assert!(s.workbench.revert_to_turn(&id, 1).await.is_err());

    // Claude Code resumes up to the first turn's answer: this is its second message, not its fourth.
    s.workbench
        .send(&id, "write again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 4).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this session.")
    );
    let launches = s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let args = launches.last().map(|l| l.args.clone()).unwrap_or_default();
    let at = args
        .iter()
        .position(|a| a == "--resume-session-at")
        .map(|i| args[i + 1].clone());
    assert!(at.is_some_and(|at| at == cursors[0]), "{args:?}");
    // The new turn's changes count from the reverted folder: only what it wrote. The revert is no turn of its own.
    let turns = turns_recorded(&s.workbench, &id, 4).await;
    assert_eq!(turns.len(), 4);
    let changes = s
        .workbench
        .changes(&id, Some(turns[3]))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        changes.diff.contains("+write again") && !changes.diff.contains("msg-3"),
        "{}",
        changes.diff
    );

    // While a turn runs, nothing is reverted.
    s.workbench.send(&id, "slow").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| {
        matches!(
            e.last().map(|e| &e.event),
            Some(WbEvent::TurnStart { .. } | WbEvent::Text { .. })
        )
    })
    .await;
    let refused = s.workbench.revert_to_turn(&id, turns[0]).await;
    assert!(refused.is_err_and(|e| e.contains("working")));
    assert!(repo.join("msg-2.txt").exists());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn without_a_recorded_point_the_next_message_tells_the_agent() {
    let s = setup("revert-tell");
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);
    // Claude Code reports no message ids (as in logs from before reverts): turn ends carry no cursor.
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_NO_UUID".into(), "1".into());
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "write first"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&s.workbench, &id, 1).await;
    s.workbench
        .send(&id, "write second")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    let turns = turns_recorded(&s.workbench, &id, 2).await;
    assert_eq!(s.workbench.revert_to_turn(&id, turns[0]).await, Ok(1));
    s.workbench.send(&id, "next").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    let launches = s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert!(
        !launches
            .last()
            .is_some_and(|l| l.args.iter().any(|a| a == "--resume-session-at"))
    );
    let session = launches
        .last()
        .and_then(|l| {
            l.args
                .iter()
                .position(|a| a == "--resume")
                .map(|i| l.args[i + 1].clone())
        })
        .unwrap_or_default();
    let prompts = std::fs::read_to_string(s.host.memory.join(format!("{session}.prompts"))).unwrap_or_default();
    let last = prompts.lines().last().unwrap_or_default();
    assert!(last.contains("reverted") && last.ends_with("next"), "{prompts}");
    // Told once: the message after it goes as written.
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    s.workbench.send(&id, "after").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 4).await;
    let prompts = std::fs::read_to_string(s.host.memory.join(format!("{session}.prompts"))).unwrap_or_default();
    assert_eq!(prompts.lines().last(), Some("after"));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn records_each_turns_changes_in_a_folder_below_the_repository_top_and_on_a_detached_head() {
    let s = setup("subfolder");
    let repo = s.root.join("repo");
    let app = repo.join("app");
    std::fs::create_dir_all(&app).unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(app.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);
    // A detached HEAD, as in the middle of a rebase or on a tag.
    git(&repo, &["checkout", "--quiet", "--detach"]);
    let project = s
        .workbench
        .add_project(&app)
        .map(|p| p.id)
        .unwrap_or_else(|e| panic!("{e}"));

    let created = s
        .workbench
        .create_thread(new_thread(&project, "hi"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&s.workbench, &created.id, 1).await;
    std::fs::write(app.join("a.txt"), "two\n").unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .send(&created.id, "again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 2).await;
    let turns = turns_recorded(&s.workbench, &created.id, 2).await;
    let second = s
        .workbench
        .changes(&created.id, Some(turns[1]))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        second.diff.contains("b/a.txt") && second.diff.contains("-one\n+two"),
        "{}",
        second.diff
    );

    // Archiving drops its checkpoints from the repository, as for a session at the top.
    s.workbench.stop(&created.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &created.id).await;
    let prefix = format!("refs/nexus/checkpoints/{}/", created.id);
    assert!(refs_in(&repo, &prefix) > 0);
    s.workbench
        .update(
            &created.id,
            nexus_workbench::ThreadPatch {
                archived: Some(true),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    for _ in 0..200 {
        if refs_in(&repo, &prefix) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(refs_in(&repo, &prefix), 0);
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_fork_that_fails_part_way_leaves_nothing_behind() {
    let s = setup("fork-fails");
    let (workbench, project) = forking_bench(&s);
    let repo = s.root.join("repo");
    let source = workbench
        .create_thread(new_thread(&project, "write it"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &source.id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&workbench, &source.id, 1).await;
    workbench.stop(&source.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&workbench, &source.id).await;

    // The fork's last write (its "Forked from" note) fails, as on a full disk, after the conversation was copied.
    let db = rusqlite::Connection::open(s.root.join("fork.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let refuse = "create trigger refuse_notice before insert on events when json_extract(new.body, '$.kind') = \
                  'notice' begin select raise(fail, 'database or disk is full'); end;";
    db.execute_batch(refuse).unwrap_or_else(|e| panic!("{e}"));
    let error = workbench.fork_thread(&source.id, None).await.err().unwrap_or_default();
    assert!(error.contains("disk is full"), "{error}");
    // No half-copied session, no stray log, no worktree folder, no branch.
    assert_eq!(
        workbench
            .threads()
            .unwrap_or_default()
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        [source.id.as_str()]
    );
    let stray: i64 = db
        .query_row(
            "select count(*) from events where thread_id <> ?1",
            [&source.id],
            |row| row.get(0),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(stray, 0);
    let folders = std::fs::read_dir(s.root.join("worktrees/repo")).map_or(0, |dir| dir.count());
    assert_eq!(folders, 0);
    let branches = process::command("git")
        .arg("-C")
        .arg(&repo)
        .args(["for-each-ref", "--format=%(refname)", "refs/heads/"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();
    assert_eq!(branches, "refs/heads/main");

    // With space again, the fork goes through.
    db.execute_batch("drop trigger refuse_notice;")
        .unwrap_or_else(|e| panic!("{e}"));
    let fork = workbench
        .fork_thread(&source.id, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(matches!(
        workbench
            .events(&fork.id, 0)
            .unwrap_or_default()
            .last()
            .map(|e| &e.event),
        Some(WbEvent::Notice { .. })
    ));
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn many_model_calls_in_a_turn_update_the_session_a_few_times_not_once_each() {
    let s = setup("many-calls");
    let started = std::time::Instant::now();
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "manycalls"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    let took = started.elapsed();
    let usages = events
        .iter()
        .filter(|e| matches!(e.event, WbEvent::Usage { .. }))
        .count();
    let updates = s
        .host
        .emitted
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|e| matches!(e, Emit::Thread(summary) if summary.id == id))
        .count();
    println!("[perf] {usages} usage events: {updates} session updates, turn took {took:?}");
    // Every call is still in the log; the session's summary is written at most about once a second, and at the end.
    assert_eq!(usages, 501);
    assert!(updates < 20, "{updates} session updates for {usages} calls");
    // The end of the turn has the last main call's context and Claude Code's cost.
    let summary = thread(&s.workbench, &id);
    assert_eq!(summary.context_tokens, Some(1150));
    assert!((summary.cost_usd - 0.01).abs() < 1e-9, "cost {}", summary.cost_usd);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A session in a fresh repository with `n` finished turns, each writing a file of its own (msg-<n>.txt).
async fn session_with_turns(s: &Setup, workbench: &Workbench, project: &str, n: usize) -> (String, Vec<i64>) {
    let id = workbench
        .create_thread(new_thread(project, "write 1"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(workbench, &id, |e| turn_ends(e).len() == 1).await;
    for k in 2..=n {
        turns_recorded(workbench, &id, k - 1).await;
        workbench
            .send(&id, &format!("write {k}"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        wait_for(workbench, &id, |e| turn_ends(e).len() == k).await;
    }
    let _ = s;
    let turns = turns_recorded(workbench, &id, n).await;
    (id, turns)
}

fn init_repo(s: &Setup) -> PathBuf {
    let repo = s.root.join("repo");
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);
    repo
}

/// The turn an agent dies in keeps where its conversation stood, so the rewind a revert
/// asked for is done by that turn, and the next message neither rewinds again nor carries the revert note.
#[tokio::test]
async fn a_turn_the_agent_died_in_after_a_revert_keeps_its_cursor() {
    let s = setup("revert-died");
    init_repo(&s);
    let (id, turns) = session_with_turns(&s, &s.workbench, &s.project, 2).await;
    s.workbench
        .revert_to_turn(&id, turns[0])
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .send(&id, "slow dies")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    let died = events.iter().rev().find_map(|e| match &e.event {
        WbEvent::TurnEnd { status, cursor, .. } => Some((*status, cursor.clone())),
        _ => None,
    });
    assert!(
        matches!(&died, Some((TurnStatus::Error, Some(cursor))) if cursor.ends_with("-a2")),
        "{died:?}"
    );
    until_stopped(&s.workbench, &id).await;
    s.workbench.send(&id, "next").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 4).await;
    let args = last_args(&s);
    assert_eq!(arg_after(&args, "--resume-session-at"), None, "{args:?}");
    let session = arg_after(&args, "--resume").unwrap_or_default();
    let prompts = std::fs::read_to_string(s.host.memory.join(format!("{session}.prompts"))).unwrap_or_default();
    assert_eq!(prompts.lines().last(), Some("next"), "{prompts}");
    // The conversation went on from the turn that died: Claude Code still has it.
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 3 of this session.")
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// After an archive (which drops the checkpoints), the next message's base is no "before
/// the first turn": a fork from the latest turn waits for a turn to end, then has that turn's files and conversation.
#[tokio::test]
async fn after_an_archive_a_fork_does_not_start_from_the_folder_as_it_is() {
    let s = setup("archive-fork");
    let (workbench, project) = forking_bench(&s);
    let repo = s.root.join("repo");
    let (id, _) = session_with_turns(&s, &workbench, &project, 1).await;
    workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&workbench, &id).await;
    let archive = |archived: bool| {
        workbench
            .update(
                &id,
                nexus_workbench::ThreadPatch {
                    archived: Some(archived),
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("{e}"))
    };
    archive(true);
    let prefix = format!("refs/nexus/checkpoints/{id}/");
    for _ in 0..200 {
        if refs_in(&repo, &prefix) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(refs_in(&repo, &prefix), 0);
    workbench
        .send(&id, "slow write 2")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &id, |_| repo.join("msg-2.txt").exists()).await;
    assert!(
        workbench
            .fork_thread(&id, None)
            .await
            .is_err_and(|e| e.contains("no recorded files")),
        "the folder now is not the session's start"
    );
    assert!(workbench.fork_thread(&id, Some(0)).await.is_err());
    workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &id, |e| turn_ends(e).len() == 2).await;
    let turns = turns_recorded(&workbench, &id, 1).await;
    // The turn's diff counts from the folder as the message found it.
    let diff = workbench
        .changes(&id, Some(turns[0]))
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .diff;
    assert!(diff.contains("msg-2.txt") && !diff.contains("msg-1.txt"), "{diff}");
    let fork = workbench.fork_thread(&id, None).await.unwrap_or_else(|e| panic!("{e}"));
    let copied = workbench.events(&fork.id, 0).unwrap_or_default();
    assert!(
        copied
            .iter()
            .any(|e| matches!(&e.event, WbEvent::User { text, .. } if text == "slow write 2")),
        "{copied:#?}"
    );
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn manual_compaction_uses_the_native_command_and_keeps_arguments() {
    let s = setup("manual-compact");
    let summary = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let id = summary.id;
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    s.workbench
        .compact(&id, Some("Keep the test requirements".into()))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::User { .. }))
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, WbEvent::Notice { text } if text.contains("compacted")))
    );
    let before = events
        .iter()
        .filter(|event| matches!(&event.event, WbEvent::Notice { text } if text.contains("compacted")))
        .count();
    let error = s
        .workbench
        .compact(&id, Some("insufficient".into()))
        .await
        .expect_err("no boundary is not completion");
    assert!(error.contains("Not enough messages"));
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 3).await;
    assert_eq!(
        before,
        events
            .iter()
            .filter(|event| matches!(&event.event, WbEvent::Notice { text } if text.contains("compacted")))
            .count()
    );
    assert!(s.workbench.send(&id, "/not-a-command arg").await.is_err());
    assert!(s.workbench.send(&id, "/compact").await.is_err());
    s.workbench.restart(&id).unwrap_or_else(|e| panic!("{e}"));
    assert!(s.workbench.send(&id, "/review arg").await.is_err());
}

#[tokio::test]
async fn repeated_initialization_keeps_current_process_command_discovery() {
    let s = setup("commands-reinit");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .id;
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    s.workbench
        .send(&id, "reinitialize")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    let capabilities = thread(&s.workbench, &id).capabilities.expect("live discovery");
    assert_eq!(
        capabilities.commands.as_deref(),
        Some(["compact".to_string(), "review".to_string()].as_slice())
    );
    assert!(capabilities.compact);
    s.workbench
        .compact(&id, None)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    s.workbench.stop(&id).unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn stale_native_commands_never_start_another_process_or_enter_queue_edits() {
    let s = setup("stale-commands");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .id;
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    s.workbench
        .send(&id, "/review slow retained arguments")
        .await
        .expect("current native command");
    assert!(s.workbench.send(&id, "/review duplicate while busy").await.is_err());
    s.workbench.interrupt(&id).expect("end the synthetic held command");
    let accepted = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert!(
        accepted.iter().any(
            |event| matches!(&event.event, WbEvent::User { text, .. } if text == "/review slow retained arguments")
        )
    );
    s.workbench
        .send(&id, "removecommands")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 3).await;
    let launches = s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).len();
    assert_eq!(
        thread(&s.workbench, &id)
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.commands.as_deref()),
        Some([].as_slice())
    );
    assert!(s.workbench.send(&id, "/review arguments").await.is_err());
    assert!(s.workbench.queue_edit(&id, 2, "/model").is_err());
    assert_eq!(s.workbench.events(&id, 0).unwrap_or_default().len(), events.len());
    s.workbench.restart(&id).unwrap_or_else(|error| panic!("{error}"));
    assert!(s.workbench.send(&id, "/review arguments").await.is_err());
    assert_eq!(
        s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).len(),
        launches
    );
}

#[tokio::test]
async fn connection_reads_metadata_without_model_turns_and_discards_replaced_process_replies() {
    let s = setup("connection-runtime");
    let id = s
        .workbench
        .create_thread(new_thread(&s.project, "hello"))
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .id;
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    let first = s
        .workbench
        .connection(&id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(first.skills.as_deref(), Some(["fixture-check".to_string()].as_slice()));
    assert_eq!(
        first.plugins.as_deref(),
        Some(["fixture-plugin".to_string()].as_slice())
    );
    assert!(first.account.is_none(), "no unsupported identity probe");
    s.workbench
        .update(
            &id,
            nexus_workbench::ThreadPatch {
                model: Some(Some("synthetic-new-model".into())),
                permission_mode: Some(PermissionMode::AcceptEdits),
                ..Default::default()
            },
        )
        .expect("change existing controls");
    wait_for(&s.workbench, &id, |_| thread(&s.workbench, &id).pending.is_none()).await;
    let changed = thread(&s.workbench, &id);
    let changed_connection = changed
        .capabilities
        .and_then(|capabilities| capabilities.connection)
        .expect("current process");
    assert!(
        changed_connection.model.is_none() && changed_connection.permissions.is_none(),
        "accepted settings invalidate older runtime observations"
    );
    assert_eq!(changed.model.as_deref(), Some("synthetic-new-model"));

    assert_eq!(turn_ends(&s.workbench.events(&id, 0).unwrap_or_default()).len(), 1);
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_MCP_DELAY_MS".into(), "150".into());
    s.workbench.restart(&id).unwrap_or_else(|error| panic!("{error}"));
    s.workbench
        .send(&id, "hello again")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    let old = thread(&s.workbench, &id)
        .capabilities
        .and_then(|capabilities| capabilities.connection)
        .expect("second process")
        .generation
        .clone();
    let pending = s.workbench.connection(&id);
    let replace = async {
        tokio::time::sleep(Duration::from_millis(25)).await;
        s.workbench.restart(&id).unwrap_or_else(|error| panic!("{error}"));
        s.workbench
            .send(&id, "new process")
            .await
            .unwrap_or_else(|error| panic!("{error}"));
    };
    let (answer, _) = tokio::join!(pending, replace);
    assert!(answer.is_err(), "a replaced process cannot publish its reply");
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 3).await;
    let current = thread(&s.workbench, &id)
        .capabilities
        .and_then(|capabilities| capabilities.connection)
        .expect("new process");
    assert_ne!(current.generation, old);
    assert!(
        !serde_json::to_string(&s.workbench.events(&id, 0).unwrap_or_default())
            .expect("safe history")
            .contains("SECRET_")
    );
    s.workbench.stop(&id).unwrap_or_else(|error| panic!("{error}"));
}

/// An agent keeps running when the app moves to another organization, on the token it started with. Its scope stays
/// in `live_scopes` until it stops, so the desktop app revokes that token only then (its bridge.rs `switched`).
#[tokio::test]
async fn an_agents_scope_stays_live_after_a_switch_until_it_stops() {
    let s = setup("live-scopes");
    assert!(s.workbench.live_scopes().is_empty());
    let created = s.workbench.create_thread(new_thread(&s.project, "Hello")).await;
    let id = created.map(|t| t.id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench.set_scope("gw|other");
    assert_eq!(s.workbench.live_scopes().into_iter().collect::<Vec<_>>(), ["gw|org"]);
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    assert!(s.workbench.live_scopes().is_empty());
}

#[tokio::test]
async fn forks_with_another_subscription_without_reusing_private_agent_state() {
    use nexus_workbench::ForkSubscription;

    let s = setup("fork-subscription");
    let (workbench, project) = forking_bench(&s);
    workbench.set_rotation(false);
    let source = workbench
        .create_thread(new_thread(&project, "write it"))
        .await
        .expect("subscription fork fixture");
    wait_for(&workbench, &source.id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&workbench, &source.id, 1).await;
    let account = workbench
        .add_account(SessionAgent::Claude, &s.root.join("home"))
        .expect("subscription fork fixture");
    let fork = workbench
        .fork_thread_with_subscription(
            &source.id,
            None,
            Some(ForkSubscription {
                agent: SessionAgent::Claude,
                account: Some(account.clone()),
            }),
        )
        .await
        .expect("subscription fork fixture");
    assert_eq!(fork.via, Via::Subscription);
    assert_eq!(fork.account.as_deref(), Some(account.as_str()));
    let stored = Store::open(&s.root.join("fork.sqlite3")).expect("subscription fork fixture");
    let record = stored
        .thread(&fork.id)
        .expect("subscription fork fixture")
        .expect("subscription fork fixture");
    assert!(record.fork_from.is_none());
    assert!(
        record
            .seed
            .as_deref()
            .expect("subscription fork fixture")
            .contains("write it")
    );
    assert!(
        record
            .seed
            .as_deref()
            .expect("subscription fork fixture")
            .contains("Message 1")
    );
    workbench
        .send(&fork.id, "continue")
        .await
        .expect("subscription fork fixture");
    wait_for(&workbench, &fork.id, |e| turn_ends(e).len() == 2).await;
    {
        let launches = s.host.launches.lock().expect("subscription fork fixture");
        let launch = launches.last().expect("subscription fork fixture");
        assert_eq!(
            launch.account_dir,
            Some(
                workbench
                    .account_dir(SessionAgent::Claude, &account)
                    .expect("subscription fork fixture")
            )
        );
        assert!(
            !launch
                .args
                .iter()
                .any(|arg| arg == "--resume" || arg == "--fork-session")
        );
    }
    let codex = workbench
        .fork_thread_with_subscription(
            &source.id,
            None,
            Some(ForkSubscription {
                agent: SessionAgent::Codex,
                account: None,
            }),
        )
        .await
        .expect("subscription fork fixture");
    assert_eq!(codex.agent, SessionAgent::Codex);
    assert_eq!(codex.via, Via::Subscription);
    assert!(codex.model.is_none() && codex.effort.is_none());
    let record = stored
        .thread(&codex.id)
        .expect("subscription fork fixture")
        .expect("subscription fork fixture");
    assert!(record.fork_from.is_none() && record.seed.is_some());
    assert_eq!(
        stored
            .thread(&source.id)
            .expect("subscription fork fixture")
            .expect("subscription fork fixture")
            .summary
            .account,
        source.account
    );
    assert!(
        workbench
            .fork_thread_with_subscription(
                &source.id,
                None,
                Some(ForkSubscription {
                    agent: SessionAgent::Claude,
                    account: Some("a_1".into()),
                })
            )
            .await
            .is_err()
    );
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_plain_fork_keeps_its_sources_account_after_the_active_account_changes() {
    let s = setup("fork-keeps-account");
    let (workbench, project) = forking_bench(&s);
    workbench.set_rotation(false);
    let source = workbench
        .create_thread(NewThread {
            name: None,
            via: Via::Subscription,
            provider: None,
            ..new_thread(&project, "write it")
        })
        .await
        .expect("plain fork fixture");
    wait_for(&workbench, &source.id, |e| turn_ends(e).len() == 1).await;
    turns_recorded(&workbench, &source.id, 1).await;
    assert_eq!(source.account, None);
    let account = workbench
        .add_account(SessionAgent::Claude, &s.root.join("home"))
        .expect("plain fork fixture");
    workbench.use_account(Some(account.clone()));
    let fork = workbench
        .fork_thread(&source.id, None)
        .await
        .expect("plain fork fixture");
    workbench.send(&fork.id, "continue").await.expect("plain fork fixture");
    wait_for(&workbench, &fork.id, |e| turn_ends(e).len() == 2).await;
    let launches = s.host.launches.lock().expect("plain fork fixture");
    assert_eq!(launches.last().expect("plain fork launch").account_dir, None);
    assert_eq!(workbench.active_account(), Some(account));
}

/// The tool result the fake's command reported, once it reported one.
fn command_output(events: &[StoredEvent]) -> Option<String> {
    events.iter().find_map(|e| match &e.event {
        WbEvent::ToolResult { output, .. } => Some(output.clone()),
        _ => None,
    })
}

/// In Auto and Full access, a request Claude Code still sends is the person's. It shows, and the command runs once
/// they allow it. In auto mode, Claude Code asks after three blocked actions in a row
/// (code.claude.com/docs/en/permission-modes).
#[tokio::test]
async fn auto_and_full_access_show_claudes_ask_to_the_person() {
    for mode in [PermissionMode::Auto, PermissionMode::BypassPermissions] {
        let s = setup(&format!("unasked-{}", mode.as_str()));
        let id = s
            .workbench
            .create_thread(NewThread {
                permission_mode: mode,
                ..new_thread(&s.project, "use a tool")
            })
            .await
            .map(|t| t.id)
            .unwrap_or_else(|e| panic!("{e}"));
        let events = wait_for(&s.workbench, &id, |e| {
            e.iter().any(|e| matches!(e.event, WbEvent::Approval { .. }))
                && thread(&s.workbench, &id).status == ThreadStatus::Approval
        })
        .await;
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.event, WbEvent::ApprovalResolved { .. })),
            "{mode:?}"
        );
        let request = events
            .iter()
            .find_map(|e| match &e.event {
                WbEvent::Approval { request_id, .. } => Some(request_id.clone()),
                _ => None,
            })
            .unwrap_or_default();
        s.workbench
            .approve(&id, &request, Decision::Allow, None, None, None)
            .unwrap_or_else(|e| panic!("{e}"));
        let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
        assert_eq!(command_output(&events).as_deref(), Some("hi"), "{mode:?}");
        s.workbench.shutdown().await;
        let _ = std::fs::remove_dir_all(&s.root);
    }
}

/// A switch to Auto, then to Full access, leaves the request Claude Code already waits on to the person. The command
/// runs only once they allow it.
#[tokio::test]
async fn switch_to_auto_or_full_access_leaves_the_open_request_to_the_person() {
    let s = setup("auto-switch");
    let (id, request) = asking(&s, &[]).await;
    pick_mode(&s, &id, PermissionMode::Auto);
    wait_for(&s.workbench, &id, |_| {
        thread(&s.workbench, &id).permission_mode == PermissionMode::Auto
    })
    .await;
    // Full access is sent only now, so its acknowledgement comes after Auto's. The workbench reads the agent's lines
    // one at a time. So once Full access is saved, Auto's `allow_pending` has run.
    pick_mode(&s, &id, PermissionMode::BypassPermissions);
    wait_for(&s.workbench, &id, |_| {
        thread(&s.workbench, &id).permission_mode == PermissionMode::BypassPermissions
    })
    .await;
    let events = s.workbench.events(&id, 0).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.event, WbEvent::ApprovalResolved { .. }))
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Approval);
    s.workbench
        .approve(&id, &request, Decision::Allow, None, None, None)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(command_output(&events).as_deref(), Some("hi"));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Settings' choices, as the app hands them to the workbench that runs its sessions (its background service's too,
/// A new plan session starts on the chosen account's folder, and an account removed since falls back to
/// Claude Code's own login.
#[tokio::test]
async fn a_new_plan_session_starts_on_the_handed_account() {
    use nexus_workbench::accounts::Choice;

    let s = setup("handed-account");
    let (workbench, project) = forking_bench(&s);
    let account = workbench
        .add_account(SessionAgent::Claude, &s.root.join("home"))
        .expect("handed account fixture");
    let handed = |account: &str| Choice {
        agent: SessionAgent::Claude,
        account: Some(account.into()),
        rotate: false,
    };
    workbench.apply_account_choices(&[handed(&account)]);
    let session = workbench
        .create_thread(NewThread {
            name: None,
            via: Via::Subscription,
            provider: None,
            ..new_thread(&project, "on the chosen account")
        })
        .await
        .expect("handed account fixture");
    wait_for(&workbench, &session.id, |e| turn_ends(e).len() == 1).await;
    let launched = s
        .host
        .launches
        .lock()
        .expect("handed account fixture")
        .last()
        .and_then(|launch| launch.account_dir.clone());
    assert_eq!(
        launched,
        Some(
            workbench
                .account_dir(SessionAgent::Claude, &account)
                .expect("handed account fixture")
        )
    );
    assert_eq!(workbench.active_account(), Some(account));

    workbench.apply_account_choices(&[handed("a_1")]);
    assert_eq!(workbench.active_account(), None);
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn auto_launch_persists_routed_model_and_sends_prompt_counts() {
    let s = setup("auto-launch");
    let (workbench, project) = forking_bench(&s);
    *s.host.launch_model.lock().expect("lock") = Some("claude-opus-5-5".into());
    let prompt = "Refactor this:\n```rust\nfn main() {}\n```";
    let session = workbench
        .create_thread(NewThread {
            model: Some("auto".into()),
            via: Via::Subscription,
            provider: None,
            ..new_thread(&project, prompt)
        })
        .await
        .expect("auto thread");
    wait_for(&workbench, &session.id, |e| turn_ends(e).len() == 1).await;

    let launched = s.host.launches.lock().expect("launches").clone();
    assert_eq!(launched.len(), 1);
    assert_eq!(launched[0].model.as_deref(), Some("auto"));
    let counts = launched[0].prompt_counts.expect("prompt counts sent");
    assert_eq!(counts.code_blocks, 1);
    assert!(counts.input_tokens > 0);
    assert!(!counts.images);

    let updated = workbench
        .threads()
        .expect("list")
        .into_iter()
        .find(|t| t.id == session.id)
        .expect("session");
    assert_eq!(updated.model.as_deref(), Some("claude-opus-5-5"));

    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
