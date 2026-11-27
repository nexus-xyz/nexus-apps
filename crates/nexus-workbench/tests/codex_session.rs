//! The workbench driving the stand-in `codex app-server` (src/bin/fake-codex.rs) end to end: a turn, approvals for a
//! command and an edit, an interrupt before Codex names the turn, and a crash with the thread resumed after it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use nexus_workbench::event::{
    Decision, Image, PermissionMode, SessionAgent, StoredEvent, TaskStatus, ThreadStatus, ThreadSummary, TurnStatus,
    Usage, Via, WbEvent,
};
use nexus_workbench::process::{self, Spawn};
use nexus_workbench::store::Store;
use nexus_workbench::{Config, Emit, Host, Launch, LaunchRequest, NewThread, ProcessEnd, Workbench};

#[derive(Default)]
struct TestHost {
    recovery_identity: Mutex<String>,
    fail_next_launch: std::sync::atomic::AtomicBool,
    emitted: Mutex<Vec<Emit>>,
    launches: Mutex<Vec<LaunchRequest>>,
    ended: Mutex<Vec<(String, ProcessEnd)>>,
    memory: PathBuf,
    /// More environment for the stand-in (its `FAKE_CODEX_*` switches).
    extra: HashMap<String, String>,
}

impl Host for TestHost {
    fn update_recovery_identity(&self) -> Option<String> {
        Some(self.recovery_identity.lock().expect("recovery identity lock").clone())
    }
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch, String>> {
        let n = {
            let mut launches = self.launches.lock().unwrap_or_else(|p| p.into_inner());
            launches.push(request.clone());
            launches.len()
        };
        if self.fail_next_launch.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Box::pin(async { Err("The agent executable is unavailable.".into()) });
        }
        let mut env = self.extra.clone();
        env.insert("FAKE_CODEX_DIR".into(), self.memory.to_string_lossy().into_owned());
        Box::pin(async move {
            Ok(Launch {
                session_mcp: Default::default(),
                attribution: Some(nexus_workbench::event::SessionAttribution {
                    gateway: Some("https://gateway.fixture.test".into()),
                    organization: Some("fixture-org".into()),
                    team: Some("fixture-team".into()),
                    access: Some("byok".into()),
                    configured_mcp: Some(vec!["offline".into()]),
                }),
                spawn: Spawn {
                    program: PathBuf::from(env!("CARGO_BIN_EXE_fake-codex")),
                    args: request.args,
                    env,
                    cwd: request.cwd,
                },
                nexus_session: Some(format!("ns_{n}")),
                local_run: None,
                local_provider: None,
                model: None,
            })
        })
    }

    fn ended(&self, nexus_session: String, end: ProcessEnd) -> BoxFuture<'static, bool> {
        self.ended
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((nexus_session, end));
        Box::pin(async { true })
    }

    fn price(&self, model: &str, usage: &Usage) -> Option<f64> {
        // fake-codex's sub-agent runs on `gpt-child`, which costs ten times as much.
        let rate = if model == "gpt-child" { 0.001 } else { 0.0001 };
        Some(usage.output_tokens as f64 * rate)
    }

    fn emit(&self, emit: Emit) {
        self.emitted.lock().unwrap_or_else(|p| p.into_inner()).push(emit);
    }
}

struct Setup {
    host: Arc<TestHost>,
    workbench: Workbench,
    project: String,
    root: PathBuf,
}

fn setup(name: &str, extra: &[(&str, &str)]) -> Setup {
    let root = std::env::temp_dir().join(format!("nexus-workbench-codex-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("repo");
    let memory = root.join("memory");
    let _ = std::fs::create_dir_all(&repo);
    let _ = std::fs::create_dir_all(&memory);
    let host = Arc::new(TestHost {
        recovery_identity: Mutex::new("fixture-person".into()),
        memory,
        extra: extra.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        ..TestHost::default()
    });
    let store = Store::open(&root.join("workbench.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "gw|org".into(),
        worktrees: Some(root.join("worktrees")),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(store, host.clone(), config).unwrap_or_else(|e| panic!("{e}"));
    let project = workbench.add_project(&repo).map(|p| p.id).unwrap_or_default();
    Setup {
        host,
        workbench,
        project,
        root,
    }
}

async fn create(s: &Setup, text: &str, model: Option<&str>) -> String {
    s.workbench
        .create_thread(NewThread {
            name: None,
            project_id: s.project.clone(),
            agent: SessionAgent::Codex,
            model: model.map(String::from),
            permission_mode: PermissionMode::Default,
            effort: None,
            via: Via::Nexus,
            provider: None,
            text: text.into(),
            images: vec![],
            worktree: None,
            parent: None,
            orchestration: false,
        })
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"))
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

fn approvals(events: &[StoredEvent]) -> Vec<&WbEvent> {
    events
        .iter()
        .map(|e| &e.event)
        .filter(|e| matches!(e, WbEvent::Approval { .. }))
        .collect()
}

#[tokio::test]
async fn another_build_cannot_recover_a_running_owners_session() {
    let s = setup("second-build", &[]);
    let owner = nexus_workbench::owner::StoreOwner::take(&s.root).expect("first build owns its store");
    let id = create(&s, "slow", None).await;
    wait_for(&s.workbench, &id, |events| {
        events
            .iter()
            .any(|event| matches!(event.event, WbEvent::Read { seq: 1 }))
    })
    .await;

    assert!(nexus_workbench::owner::StoreOwner::take(&s.root).is_err());
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Running);
    assert!(turn_ends(&s.workbench.events(&id, 0).expect("events")).is_empty());

    s.workbench.stop(&id).expect("stop original agent");
    wait_for(&s.workbench, &id, |events| !turn_ends(events).is_empty()).await;
    s.workbench.shutdown().await;
    drop(owner);
    let next = nexus_workbench::owner::StoreOwner::take(&s.root).expect("next build may recover after exit");
    drop(next);
    std::fs::remove_dir_all(&s.root).expect("remove fixture");
}

#[tokio::test]
async fn questions_wait_for_answers_and_nonblocking_questions_keep_streaming() {
    for prompt in ["question", "nonblocking question"] {
        let s = setup(prompt, &[]);
        let id = create(&s, prompt, None).await;
        let events = wait_for(&s.workbench, &id, |events| {
            !approvals(events).is_empty() && (!prompt.starts_with("nonblocking") || !texts(events).is_empty())
        })
        .await;
        assert!(turn_ends(&events).is_empty());
        assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Approval);
        let WbEvent::Approval { request_id, input, .. } = approvals(&events)[0] else {
            panic!("expected question")
        };
        assert_eq!(input["isBlocking"], !prompt.starts_with("nonblocking"));
        s.workbench
            .approve(
                &id,
                request_id,
                Decision::Allow,
                Some("Every turn".into()),
                None,
                Some(serde_json::Map::from_iter([(
                    "usage".into(),
                    serde_json::json!(["Every turn"]),
                )])),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        let events = wait_for(&s.workbench, &id, |events| !turn_ends(events).is_empty()).await;
        assert!(
            texts(&events)
                .iter()
                .any(|text| text.contains("\"usage\":{\"answers\":[\"Every turn\"]}"))
        );
        assert!(events.iter().any(|stored| matches!(&stored.event, WbEvent::ApprovalResolved { decision: Decision::Allow, message: Some(message), .. } if message == "Every turn")));
        assert!(
            s.workbench
                .approve(&id, request_id, Decision::Allow, None, None, None)
                .is_err()
        );
        s.workbench.shutdown().await;
        let _ = std::fs::remove_dir_all(&s.root);
    }
}

/// A command or an edit Codex still asks about in Auto (its reviewer over its input budget, a category it does not
/// review) or in Full access is the person's: it shows, and the turn waits for their answer.
#[tokio::test]
async fn auto_and_full_access_show_codexs_ask_to_the_person() {
    for mode in [PermissionMode::Auto, PermissionMode::BypassPermissions] {
        for prompt in ["use a tool", "edit"] {
            let s = setup(&format!("unasked-{}-{}", mode.as_str(), prompt.len()), &[]);
            let id = s
                .workbench
                .create_thread(NewThread {
                    name: None,
                    project_id: s.project.clone(),
                    agent: SessionAgent::Codex,
                    model: None,
                    permission_mode: mode,
                    effort: None,
                    via: Via::Nexus,
                    provider: None,
                    text: prompt.into(),
                    images: vec![],
                    worktree: None,
                    parent: None,
                    orchestration: false,
                })
                .await
                .map(|t| t.id)
                .unwrap_or_else(|e| panic!("{e}"));
            let events = wait_for(&s.workbench, &id, |events| {
                !approvals(events).is_empty() && thread(&s.workbench, &id).status == ThreadStatus::Approval
            })
            .await;
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e.event, WbEvent::ApprovalResolved { .. })),
                "{mode:?} {prompt}"
            );
            assert!(turn_ends(&events).is_empty(), "{mode:?} {prompt}");
            s.workbench.shutdown().await;
            let _ = std::fs::remove_dir_all(&s.root);
        }
    }
}

#[tokio::test]
async fn interrupting_a_question_expires_it_without_an_answer() {
    let s = setup("interrupt-question", &[]);
    let id = create(&s, "question", None).await;
    wait_for(&s.workbench, &id, |events| !approvals(events).is_empty()).await;
    s.workbench.interrupt(&id).unwrap_or_else(|error| panic!("{error}"));
    let events = wait_for(&s.workbench, &id, |events| !turn_ends(events).is_empty()).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted]);
    assert!(events.iter().any(|stored| matches!(
        &stored.event,
        WbEvent::ApprovalResolved {
            decision: Decision::Expired,
            ..
        }
    )));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn followups_reach_codex_at_a_tool_boundary_without_send_now() {
    let s = setup("automatic-steer", &[]);
    let id = create(&s, "queue-probe", None).await;
    wait_for(&s.workbench, &id, |events| {
        events
            .iter()
            .any(|event| matches!(&event.event, WbEvent::Tool { id, .. } if id.starts_with("probe-")))
    })
    .await;
    let sent = Instant::now();
    s.workbench.send(&id, "include the queued change").await.expect("send");
    let seq = s
        .workbench
        .events(&id, 0)
        .expect("events")
        .into_iter()
        .find_map(|event| {
            matches!(&event.event, WbEvent::User { text, .. } if text == "include the queued change")
                .then_some(event.seq)
        })
        .expect("follow-up");
    let read = tokio::time::timeout(
        Duration::from_secs(2),
        wait_for(&s.workbench, &id, |events| {
            events
                .iter()
                .any(|event| matches!(event.event, WbEvent::Read { seq: read } if read == seq))
        }),
    )
    .await;
    let latency = sent.elapsed();
    let snapshot = s.workbench.events(&id, 0).expect("events");
    s.workbench.interrupt(&id).expect("interrupt");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
    assert!(
        snapshot
            .iter()
            .any(|event| matches!(&event.event, WbEvent::ToolResult { id, .. } if id.starts_with("probe-")))
    );
    assert!(
        read.is_ok(),
        "tools completed, but Nexus held the follow-up until turn end"
    );
    assert!(
        turn_ends(&snapshot).is_empty(),
        "the follow-up must join the active turn"
    );
    eprintln!("follow-up read in {} ms", latency.as_millis());
}

#[tokio::test]
async fn burst_followups_keep_order_images_and_one_read_receipt_each() {
    let s = setup("steer-burst", &[]);
    let id = create(&s, "queue-probe", None).await;
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Tool { .. }))
    })
    .await;
    for n in 0..20 {
        let text = match n {
            0 | 1 => "same message".to_string(),
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
        s.workbench.send_message(&id, &text, images).await.expect("send burst");
    }
    let queued: Vec<i64> = s
        .workbench
        .events(&id, 0)
        .expect("events")
        .iter()
        .filter_map(|event| matches!(event.event, WbEvent::User { .. }).then_some(event.seq))
        .skip(1)
        .collect();
    let events = wait_for(&s.workbench, &id, |events| {
        queued.iter().all(|seq| {
            events
                .iter()
                .any(|event| matches!(event.event, WbEvent::Read { seq: read } if read == *seq))
        }) && texts(events).len() == 20
    })
    .await;
    assert_eq!(queued.len(), 20);
    let receipts: Vec<i64> = events
        .iter()
        .filter_map(|event| match event.event {
            WbEvent::Read { seq } if queued.contains(&seq) => Some(seq),
            _ => None,
        })
        .collect();
    assert_eq!(receipts, queued);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::TurnStart { .. }))
            .count(),
        1
    );
    assert!(turn_ends(&events).is_empty());
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Running);
    let inputs: Vec<serde_json::Value> = texts(&events)
        .iter()
        .map(|text| {
            serde_json::from_str(text.strip_prefix("Steered input: ").expect("input echo")).expect("JSON input")
        })
        .collect();
    assert_eq!(
        inputs[2],
        serde_json::json!([{"type":"image","url":"data:image/png;base64,AAAA"}])
    );
    assert_eq!(inputs[3][0]["type"], "image");
    assert_eq!(inputs[3][1]["text"], "follow-up 3");
    s.workbench.interrupt(&id).expect("interrupt");
    wait_for(&s.workbench, &id, |events| !turn_ends(events).is_empty()).await;
    s.workbench.send(&id, "next normal turn").await.expect("next turn");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this thread.")
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn followups_during_startup_wait_for_the_thread_or_turn_id() {
    for (name, extra) in [
        ("thread", ("FAKE_CODEX_THREAD_MS", "300")),
        ("turn", ("FAKE_CODEX_LATE_TURN", "1")),
    ] {
        let s = setup(&format!("steer-before-{name}"), &[extra]);
        let id = create(&s, "queue-probe", None).await;
        s.workbench
            .send(&id, "first startup follow-up")
            .await
            .expect("first follow-up");
        s.workbench
            .send(&id, "second startup follow-up")
            .await
            .expect("second follow-up");
        let events = wait_for(&s.workbench, &id, |events| {
            events
                .iter()
                .filter(|event| matches!(event.event, WbEvent::Read { .. }))
                .count()
                == 3
        })
        .await;
        assert!(turn_ends(&events).is_empty());
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.event, WbEvent::TurnStart { .. }))
                .count(),
            1
        );
        s.workbench.interrupt(&id).expect("interrupt");
        s.workbench.shutdown().await;
        let _ = std::fs::remove_dir_all(&s.root);
    }
}

#[tokio::test]
async fn a_refused_automatic_steer_runs_next_once_and_the_session_stays_usable() {
    let s = setup("steer-refused", &[("FAKE_CODEX_REJECT_STEER", "1")]);
    let id = create(&s, "queue-probe", None).await;
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Tool { .. }))
    })
    .await;
    s.workbench.send(&id, "fallback message").await.expect("send");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(
        texts(&events),
        ["Message 1 of this thread.", "Message 2 of this thread."]
    );
    let seq = events
        .iter()
        .find_map(|event| {
            matches!(&event.event, WbEvent::User { text, .. } if text == "fallback message").then_some(event.seq)
        })
        .expect("follow-up");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::Read { seq: read } if read == seq))
            .count(),
        1
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.send(&id, "still responsive").await.expect("next message");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 3).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 3 of this thread.")
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_refused_steer_counts_one_unread_message_when_the_process_crashes() {
    for (name, rejection, prompt) in [
        ("fallback", "1", "crash fallback"),
        ("active", "crash", "fallback message"),
    ] {
        let s = setup(
            &format!("steer-refused-crash-{name}"),
            &[("FAKE_CODEX_REJECT_STEER", rejection)],
        );
        let id = create(&s, "queue-probe", None).await;
        wait_for(&s.workbench, &id, |events| {
            events.iter().any(|event| matches!(event.event, WbEvent::Tool { .. }))
        })
        .await;
        s.workbench.send(&id, prompt).await.expect("send fallback");
        let events = wait_for(&s.workbench, &id, |events| {
            events.iter().any(|event| match &event.event {
                WbEvent::Notice { text } => text.starts_with("The agent stopped before it read"),
                _ => false,
            })
        })
        .await;
        let notices: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.event {
                WbEvent::Notice { text } if text.starts_with("The agent stopped before it read") => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            notices,
            ["The agent stopped before it read your last message. Send it again to continue."]
        );
        let sent = events
            .iter()
            .find_map(|event| match &event.event {
                WbEvent::User { text, .. } if text == prompt => Some(event.seq),
                _ => None,
            })
            .expect("follow-up message");
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.event, WbEvent::Read { seq } if seq == sent))
        );
        s.workbench.shutdown().await;
        let _ = std::fs::remove_dir_all(&s.root);
    }
}

#[tokio::test]
async fn a_refused_steer_keeps_a_long_fallback_running_and_accepts_more_followups() {
    let s = setup("steer-refused-long", &[("FAKE_CODEX_REJECT_STEER", "1")]);
    let id = create(&s, "queue-probe", None).await;
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Tool { .. }))
    })
    .await;
    s.workbench.send(&id, "slow fallback").await.expect("send fallback");
    let events = wait_for(&s.workbench, &id, |events| {
        turn_ends(events).len() == 1
            && events
                .iter()
                .filter(|event| matches!(event.event, WbEvent::Read { .. }))
                .count()
                == 2
    })
    .await;
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Running);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::TurnStart { .. }))
            .count(),
        2
    );
    s.workbench
        .send(&id, "update the fallback")
        .await
        .expect("steer fallback");
    let events = wait_for(&s.workbench, &id, |events| {
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::Read { .. }))
            .count()
            == 3
    })
    .await;
    assert_eq!(turn_ends(&events).len(), 1);
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Running);
    s.workbench.interrupt(&id).expect("interrupt");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed, TurnStatus::Interrupted]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// What Nexus tells the agent in the notice tests: no word the stand-in acts on.
const NOTICE: &str = "Your command finished.";

fn notice_rows(events: &[StoredEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(&event.event, WbEvent::Notice { text } if text == NOTICE))
        .count()
}

fn user_rows(events: &[StoredEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(&event.event, WbEvent::User { text, .. } if text == NOTICE))
        .count()
}

/// Nexus's own notice reaches Codex as a steer of the turn under way (`turn/steer`), as the person's message does. The
/// session shows it as Nexus's: a notice row, and no `User` row.
#[tokio::test]
async fn a_nexus_notice_steers_codexs_turn_under_way() {
    let s = setup("notice-steer", &[]);
    let id = create(&s, "queue-probe", None).await;
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Tool { .. }))
    })
    .await;
    s.workbench.steer_notice(&id, NOTICE).expect("notice");
    let events = wait_for(&s.workbench, &id, |events| {
        texts(events)
            .iter()
            .any(|text| text.starts_with("Steered input: ") && text.contains(NOTICE))
    })
    .await;
    assert!(turn_ends(&events).is_empty(), "the notice must join the turn under way");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::TurnStart { .. }))
            .count(),
        1
    );
    assert_eq!(notice_rows(&events), 1);
    assert_eq!(user_rows(&events), 0);
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Running);
    s.workbench.interrupt(&id).expect("interrupt");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Codex refuses the notice's steer (its turn ended first): the notice runs once, as the next turn, and the session
/// settles idle. Whose turn it reserves is pinned in codex.rs (`a_refused_steer_says_whose_turn_it_reserves`).
#[tokio::test]
async fn a_refused_nexus_notice_runs_next_once() {
    let s = setup("notice-refused", &[("FAKE_CODEX_REJECT_STEER", "1")]);
    let id = create(&s, "queue-probe", None).await;
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Tool { .. }))
    })
    .await;
    s.workbench.steer_notice(&id, NOTICE).expect("notice");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed, TurnStatus::Completed]);
    assert_eq!(
        texts(&events),
        ["Message 1 of this thread.", "Message 2 of this thread."]
    );
    assert_eq!(notice_rows(&events), 1);
    assert_eq!(user_rows(&events), 0);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(turn_ends(&s.workbench.events(&id, 0).expect("events")).len(), 2);
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The turn ended before Codex refused the notice's steer, and the person pressed Stop in between, with no turn to
/// stop. The turn the refusal reserves for the notice is not the person's: the halt interrupts it as it begins, before
/// it does any work. Its prompt would probe until interrupted.
#[tokio::test]
async fn a_notice_refused_after_a_stop_is_interrupted_as_its_turn_begins() {
    let s = setup("notice-refused-halted", &[("FAKE_CODEX_REJECT_STEER", "late")]);
    let id = create(&s, "queue-probe", None).await;
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Tool { .. }))
    })
    .await;
    s.workbench
        .steer_notice(&id, "queue-probe: your command finished.")
        .expect("notice");
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    s.workbench.interrupt(&id).expect("interrupt");
    std::fs::write(s.host.memory.join("reject-steer"), "").expect("signal");
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed, TurnStatus::Interrupted]);
    let second = events
        .iter()
        .filter(|event| matches!(event.event, WbEvent::TurnStart { .. }))
        .nth(1)
        .map(|event| event.seq)
        .expect("the notice's turn");
    let probes = events
        .iter()
        .filter(|event| event.seq > second && matches!(event.event, WbEvent::Tool { .. }))
        .count();
    assert!(probes <= 1, "the notice's turn ran {probes} steps after the Stop");
    assert!(s.workbench.halt(&id).expect("halt").is_some());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn automatic_steering_does_not_answer_or_expire_an_approval() {
    for prompt in ["question", "use a tool"] {
        let s = setup(&format!("steer-approval-{prompt}"), &[]);
        let id = create(&s, prompt, None).await;
        let events = wait_for(&s.workbench, &id, |events| !approvals(events).is_empty()).await;
        let request_id = match &approvals(&events)[0] {
            WbEvent::Approval { request_id, .. } => request_id.clone(),
            _ => unreachable!(),
        };
        s.workbench
            .send(&id, "extra context while waiting")
            .await
            .expect("follow-up");
        let events = wait_for(&s.workbench, &id, |events| {
            events
                .iter()
                .filter(|event| matches!(event.event, WbEvent::Read { .. }))
                .count()
                == 2
        })
        .await;
        assert!(turn_ends(&events).is_empty());
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.event, WbEvent::ApprovalResolved { .. }))
        );
        assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Approval);
        let answers = (prompt == "question")
            .then(|| serde_json::Map::from_iter([("usage".into(), serde_json::json!(["Every turn"]))]));
        s.workbench
            .approve(&id, &request_id, Decision::Allow, None, None, answers)
            .expect("approve");
        wait_for(&s.workbench, &id, |events| !turn_ends(events).is_empty()).await;
        s.workbench.shutdown().await;
        let _ = std::fs::remove_dir_all(&s.root);
    }
}

#[tokio::test]
async fn generates_a_codex_title_and_keeps_read_receipts_and_responses_clean() {
    let s = setup(
        "generated-title",
        &[("FAKE_CODEX_TITLE", "Preserve unsent prompt drafts")],
    );
    let id = create(
        &s,
        "Please investigate why my unsent prompts disappear when I switch sessions",
        None,
    )
    .await;
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    assert_eq!(thread(&s.workbench, &id).title, "Preserve unsent prompt drafts");
    assert!(events.iter().any(|event| matches!(event.event, WbEvent::Read { .. })));
    let replies: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            WbEvent::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(replies, ["Message 1 of this thread."]);
    assert_eq!(s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).len(), 1);
    s.workbench.shutdown().await;
}

#[tokio::test]
async fn session_list_activity_changes_only_when_attention_is_needed() {
    let s = setup("sidebar-activity", &[]);
    let id = create(&s, "slow", None).await;
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Read { .. }))
    })
    .await;
    let running = thread(&s.workbench, &id);
    assert_eq!(running.status, ThreadStatus::Running);
    assert_eq!(running.listed_at, Some(running.created_at));
    let rename = |title: &str| {
        s.workbench
            .update(
                &id,
                nexus_workbench::ThreadPatch {
                    title: Some(title.into()),
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("{e}"))
    };
    assert_eq!(rename("Renamed while working").listed_at, running.listed_at);
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |_| {
        thread(&s.workbench, &id).status == ThreadStatus::Idle
    })
    .await;
    let finished = thread(&s.workbench, &id);
    assert_eq!(finished.listed_at, Some(finished.updated_at));
    assert!(finished.listed_at >= running.listed_at);
    assert_eq!(rename("Renamed after finishing").listed_at, finished.listed_at);

    s.workbench
        .send(&id, "use a tool")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |_| {
        thread(&s.workbench, &id).status == ThreadStatus::Approval
    })
    .await;
    let waiting = thread(&s.workbench, &id);
    assert_eq!(waiting.listed_at, Some(waiting.updated_at));
    assert!(waiting.listed_at >= finished.listed_at);
    assert_eq!(rename("Renamed while waiting").listed_at, waiting.listed_at);
    let WbEvent::Approval { request_id, .. } = approvals(&events)[0] else {
        panic!("no approval")
    };
    s.workbench
        .approve(&id, request_id, Decision::Allow, None, None, None)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |_| {
        thread(&s.workbench, &id).status == ThreadStatus::Idle
    })
    .await;
    let done = thread(&s.workbench, &id);
    assert_eq!(done.listed_at, Some(done.updated_at));
    assert!(done.listed_at >= waiting.listed_at);

    // Every emitted summary while working keeps the preceding creation or attention timestamp.
    let summaries: Vec<_> = s
        .host
        .emitted
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter_map(|event| match event {
            Emit::Thread(summary) if summary.id == id => Some(summary.clone()),
            _ => None,
        })
        .collect();
    for pair in summaries.windows(2) {
        if matches!(pair[1].status, ThreadStatus::Starting | ThreadStatus::Running) || pair[0].status == pair[1].status
        {
            assert_eq!(pair[1].listed_at, pair[0].listed_at);
        }
    }
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// only the person's own message moves a session in its project. Agent work, a rename, an interruption, an
/// approval and a finished turn keep `acted_at`.
#[tokio::test]
async fn acted_at_moves_only_when_the_person_sends_a_message() {
    let s = setup("sidebar-acted", &[]);
    let id = create(&s, "slow", None).await;
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Read { .. }))
    })
    .await;
    let running = thread(&s.workbench, &id);
    let first = running.acted_at.unwrap_or_else(|| panic!("a new session has acted_at"));
    assert!(first >= running.created_at);
    let renamed = s
        .workbench
        .update(
            &id,
            nexus_workbench::ThreadPatch {
                title: Some("Renamed while working".into()),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(renamed.acted_at, Some(first));
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |_| {
        thread(&s.workbench, &id).status == ThreadStatus::Idle
    })
    .await;
    assert_eq!(thread(&s.workbench, &id).acted_at, Some(first));

    std::thread::sleep(std::time::Duration::from_millis(5));
    s.workbench
        .send(&id, "use a tool")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |_| {
        thread(&s.workbench, &id).status == ThreadStatus::Approval
    })
    .await;
    let sent = thread(&s.workbench, &id).acted_at.unwrap_or_default();
    assert!(sent > first, "the person's message moves it");
    let WbEvent::Approval { request_id, .. } = approvals(&events)[0] else {
        panic!("no approval")
    };
    s.workbench
        .approve(&id, request_id, Decision::Allow, None, None, None)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |_| {
        thread(&s.workbench, &id).status == ThreadStatus::Idle
    })
    .await;
    assert_eq!(thread(&s.workbench, &id).acted_at, Some(sent));

    // No emitted summary moves it back, and only the message moved it forward.
    let acted: Vec<_> = s
        .host
        .emitted
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter_map(|event| match event {
            Emit::Thread(summary) if summary.id == id => summary.acted_at,
            _ => None,
        })
        .collect();
    assert!(acted.windows(2).all(|pair| pair[0] <= pair[1]));
    assert_eq!(acted.last(), Some(&sent));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn runs_a_codex_turn() {
    let s = setup("turn", &[]);
    let id = create(&s, "Which model? model", Some("gpt-6-sol")).await;
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e.event {
            WbEvent::User { .. } => "user",
            WbEvent::TurnStart { .. } => "turnStart",
            WbEvent::Session { .. } => "session",
            WbEvent::Connection { .. } => "connection",
            WbEvent::Text { .. } => "text",
            WbEvent::Usage { .. } => "usage",
            WbEvent::TurnEnd { .. } => "turnEnd",
            WbEvent::Read { .. } => "read",
            _ => "other",
        })
        .collect();
    // Codex recorded the message in the conversation: it is confirmed read.
    assert_eq!(
        kinds,
        [
            "user",
            "turnStart",
            "session",
            "connection",
            "connection",
            "read",
            "text",
            "usage",
            "turnEnd"
        ]
    );
    assert_eq!(texts(&events), ["Message 1 of this thread. Running gpt-6-sol."]);
    let launches = s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!(
        launches[0].args,
        [
            "-c",
            "model=\"gpt-6-sol\"",
            "-c",
            "features.default_mode_request_user_input=true",
            "app-server"
        ]
    );
    assert!(
        s.host
            .emitted
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .any(|e| matches!(e, Emit::Delta(delta) if delta.thread_id == id))
    );

    // Codex reports no cost: the turn costs what its calls priced at.
    let summary = thread(&s.workbench, &id);
    assert_eq!(summary.status, ThreadStatus::Idle);
    assert!((summary.cost_usd - 0.005).abs() < 1e-9, "cost {}", summary.cost_usd);
    assert_eq!(summary.context_tokens, Some(1150));
    assert_eq!(summary.context_window, Some(258400));

    // Codex takes a mode change with its next turn and answers at once: saved now, nothing pending.
    let changed = s
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
        (changed.permission_mode, changed.pending),
        (PermissionMode::AcceptEdits, None)
    );

    // A second message goes to the same app server.
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(texts(&events)[1], "Message 2 of this thread.");
    assert_eq!(s.host.launches.lock().unwrap_or_else(|p| p.into_inner()).len(), 1);
    s.workbench.shutdown().await;
    let ended = s.host.ended.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!((ended.len(), ended[0].1.exit_code), (1, 0));
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A model picked mid-session reaches Codex by its own name: a ChatGPT plan refuses the policy id
/// (`openai/gpt-6.1-terra`). The session keeps the policy id.
#[tokio::test]
async fn a_model_change_reaches_codex_without_the_vendor_prefix() {
    let s = setup("model-change", &[]);
    let id = create(&s, "model", Some("gpt-6-sol")).await;
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench
        .update(
            &id,
            nexus_workbench::ThreadPatch {
                model: Some(Some("openai/gpt-6.1-terra".into())),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench.send(&id, "model").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(texts(&events)[1], "Message 2 of this thread. Running gpt-6.1-terra.");
    assert_eq!(thread(&s.workbench, &id).model.as_deref(), Some("openai/gpt-6.1-terra"));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A sub-agent's model call is logged as its own: its usage names the sub-agent's Agent call and is priced with the
/// sub-agent's model. It comes after the session's last call and leaves the context meter at the session's (1150), not
/// its own (520).
#[tokio::test]
async fn a_child_usage_keeps_its_parent() {
    let s = setup("child-usage", &[]);
    let id = create(&s, "Ask a helper: subagent", None).await;
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    let parents: Vec<Option<&str>> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Usage { parent, .. } => Some(parent.as_deref()),
            _ => None,
        })
        .collect();
    assert_eq!(parents, [None, Some("call-1-child")]);
    assert_eq!(thread(&s.workbench, &id).context_tokens, Some(1150));
    // Each call is priced with its own agent's model: the session's 50 output tokens, the sub-agent's 20.
    let priced: Vec<(&str, f64)> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Usage { model, cost_usd, .. } => Some((model.as_str(), *cost_usd)),
            _ => None,
        })
        .collect();
    assert_eq!(priced.len(), 2);
    assert_eq!(priced[1].0, "gpt-child");
    assert!((priced[1].1 - 0.02).abs() < 1e-9, "{priced:?}");
    assert!((priced[0].1 - 0.005).abs() < 1e-9, "{priced:?}");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A child's work after the session's turn is kept and opens no turn: its text, its command and its approval. Its
/// answer reaches Codex; its call is priced with its model and counts toward the session's total, not the next turn's.
/// Its end closes its Agent call.
#[tokio::test]
async fn a_child_after_the_turn_keeps_its_work_and_bills_no_turn() {
    let s = setup("late-child", &[]);
    let id = create(&s, "Leave a helper running: latechild", None).await;
    let asked_late = |e: &[StoredEvent]| {
        approvals(e)
            .iter()
            .any(|a| matches!(a, WbEvent::Approval { tool_use_id: Some(id), .. } if id == "exec-late"))
    };
    let events = wait_for(&s.workbench, &id, asked_late).await;
    let starts = |e: &[StoredEvent]| {
        e.iter()
            .filter(|e| matches!(e.event, WbEvent::TurnStart { .. }))
            .count()
    };
    assert_eq!(starts(&events), 1);
    assert_eq!(turn_ends(&events).len(), 1);
    let call = "call-1-late";
    let named = |e: &WbEvent| e.parent() == Some(call);
    let text = events
        .iter()
        .position(|e| matches!(&e.event, WbEvent::Text { text, .. } if text == "Child at work"));
    let end = events.iter().position(|e| matches!(e.event, WbEvent::TurnEnd { .. }));
    assert!(
        matches!((end, text), (Some(end), Some(text)) if end < text),
        "{end:?} {text:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Tool { id, .. } if id == "exec-late") && named(&e.event))
    );
    // The child's plan after the turn is its own too.
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Tool { name, .. } if name == "TodoWrite") && named(&e.event))
    );
    // Its approval from inside the turn outlived the turn's end.
    let expired = |e: &[StoredEvent]| {
        e.iter()
            .filter_map(|e| match &e.event {
                WbEvent::ApprovalResolved {
                    request_id,
                    decision: Decision::Expired,
                    ..
                } => Some(request_id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert!(expired(&events).is_empty(), "{:?}", expired(&events));
    let Some(WbEvent::Approval { request_id, parent, .. }) = approvals(&events)
        .into_iter()
        .find(|a| matches!(a, WbEvent::Approval { tool_use_id: Some(id), .. } if id == "exec-late"))
        .cloned()
    else {
        panic!("no approval");
    };
    assert_eq!(parent.as_deref(), Some(call));
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Approval);
    s.workbench
        .approve(&id, &request_id, Decision::Allow, None, None, None)
        .unwrap_or_else(|e| panic!("{e}"));
    let closed = |e: &[StoredEvent]| {
        e.iter()
            .any(|e| matches!(&e.event, WbEvent::ToolResult { id, .. } if id == call))
    };
    let events = wait_for(&s.workbench, &id, closed).await;
    assert_eq!(starts(&events), 1);
    assert!(
        events.iter().any(
            |e| matches!(&e.event, WbEvent::ToolResult { id, output, .. } if id == "exec-late" && output == "made")
        )
    );
    // The child's end ends the approval it left open.
    assert_eq!(expired(&events), ["exec-early#901"]);
    assert!(events.iter().any(|e| matches!(
        &e.event,
        WbEvent::Task {
            status: TaskStatus::Completed,
            ..
        }
    )));
    // The child's call: 20 output tokens at the sub-agent model's rate, in the session's total and in no turn.
    let child_cost = events.iter().find_map(|e| match &e.event {
        WbEvent::Usage {
            parent: Some(p),
            cost_usd,
            ..
        } if p == call => Some(*cost_usd),
        _ => None,
    });
    assert!(
        matches!(child_cost, Some(cost) if (cost - 0.02).abs() < 1e-9),
        "{child_cost:?}"
    );
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    let turn_costs: Vec<Option<f64>> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::TurnEnd { cost_usd, .. } => Some(*cost_usd),
            _ => None,
        })
        .collect();
    assert!(
        turn_costs
            .iter()
            .all(|cost| matches!(cost, Some(c) if (c - 0.005).abs() < 1e-9)),
        "{turn_costs:?}"
    );
    let total = thread(&s.workbench, &id).cost_usd;
    assert!((total - 0.03).abs() < 1e-9, "{total}");
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

async fn until_stopped(workbench: &Workbench, id: &str) {
    for _ in 0..400 {
        if !workbench.is_live(id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the agent of {id} is still running");
}

/// The task rows the core wrote when the agent's process ended, by task id.
fn process_ends(events: &[StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Task {
                task_id,
                status: TaskStatus::Stopped,
                detail: Some(detail),
                ..
            } if detail.reason.as_deref() == Some("process_ended") => Some(task_id.clone()),
            _ => None,
        })
        .collect()
}

/// Stop ends a Codex child that works on after the turn: one stopped row for it, then the notice that the agent's
/// background work stopped with it.
#[tokio::test]
async fn stop_logs_a_stopped_row_for_a_running_codex_child() {
    let s = setup("codex-ends-stop", &[]);
    let id = create(&s, "Leave a helper running: latechild", None).await;
    let asked = |e: &[StoredEvent]| {
        approvals(e)
            .iter()
            .any(|a| matches!(a, WbEvent::Approval { tool_use_id: Some(id), .. } if id == "exec-late"))
    };
    wait_for(&s.workbench, &id, asked).await;
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    let ends = process_ends(&events);
    assert_eq!(ends.len(), 1, "{ends:?}");
    assert!(ends[0].contains("-late-"), "{ends:?}");
    let end = events.iter().position(|e| {
        matches!(
            &e.event,
            WbEvent::Task {
                status: TaskStatus::Stopped,
                ..
            }
        )
    });
    let notice = events.iter().position(|e| {
        matches!(&e.event, WbEvent::Notice { text } if text == "What the agent left running in the background stopped with it.")
    });
    assert!(
        matches!((end, notice), (Some(end), Some(notice)) if end < notice),
        "{end:?} {notice:?}"
    );
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A Codex child at work after the turn keeps its agent from being reaped as idle; once it ends, the agent is idle
/// again, and nothing is left to stop with it.
#[tokio::test]
async fn a_codex_child_keeps_its_agent_awake() {
    let mut s = setup("codex-idle-child", &[("FAKE_CODEX_CHILD_MS", "700")]);
    let config = Config {
        scope: "gw|org".into(),
        worktrees: Some(s.root.join("worktrees-idle")),
        idle_after: Duration::from_millis(100),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let store = Store::open(&s.root.join("idle.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    s.workbench = Workbench::new(store, s.host.clone(), config).unwrap_or_else(|e| panic!("{e}"));
    s.project = s
        .workbench
        .add_project(&s.root.join("repo"))
        .map(|p| p.id)
        .unwrap_or_default();
    let id = create(&s, "A quiet helper: sleeper", None).await;
    wait_for(&s.workbench, &id, |e| texts(e).iter().any(|t| t == "Sleeping")).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    s.workbench.reap_idle();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(s.workbench.is_live(&id), "reaped while a sub-agent worked");
    let ended = |e: &[StoredEvent]| {
        e.iter().any(|e| {
            matches!(
                &e.event,
                WbEvent::Task {
                    status: TaskStatus::Completed,
                    ..
                }
            )
        })
    };
    wait_for(&s.workbench, &id, ended).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    s.workbench.reap_idle();
    until_stopped(&s.workbench, &id).await;
    assert_eq!(
        process_ends(&s.workbench.events(&id, 0).unwrap_or_default()),
        Vec::<String>::new()
    );
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A stop of a Codex child interrupts its own turn, and its interrupted turn is its stopped row. The summary says Codex
/// stops sub-agents, and that its Interrupt is not known to spare them (Lucas's third condition: the screen can show
/// the difference). Once stopped, the core refuses another stop without asking.
#[tokio::test]
async fn stops_a_child_by_its_turn() {
    let s = setup("codex-task-stop", &[("FAKE_CODEX_CHILD_MS", "20000")]);
    let id = create(&s, "A quiet helper: sleeper", None).await;
    wait_for(&s.workbench, &id, |e| texts(e).iter().any(|t| t == "Sleeping")).await;
    let summary = thread(&s.workbench, &id);
    let capabilities = summary.capabilities.unwrap_or_else(|| panic!("no capabilities"));
    assert_eq!(capabilities.version.as_deref(), Some("0.164.0"));
    assert_eq!(capabilities.stop, ["agent"]);
    assert!(!capabilities.interrupt_spares_agents);
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    let child = events
        .iter()
        .find_map(|e| match &e.event {
            WbEvent::Task { task_id, .. } => Some(task_id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no child: {events:#?}"));
    let started = Instant::now();
    s.workbench
        .task_stop(&id, &child)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let stopped = |e: &[StoredEvent]| {
        e.iter().any(|e| {
            matches!(&e.event, WbEvent::Task { task_id, status: TaskStatus::Stopped, detail, .. }
                if *task_id == child && detail.as_ref().and_then(|d| d.reason.as_deref()).is_none())
        })
    };
    wait_for(&s.workbench, &id, stopped).await;
    assert!(started.elapsed() < Duration::from_secs(10), "stopped by its own end");
    let again = s.workbench.task_stop(&id, &child).await;
    assert_eq!(again, Err("That task is not running.".into()));
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A child whose turn Codex never named cannot be interrupted: the protocol refuses at once, without asking Codex and
/// without the five-second wait for an answer.
#[tokio::test]
async fn a_child_without_a_turn_is_refused_at_once() {
    let s = setup(
        "codex-task-stop-no-turn",
        &[("FAKE_CODEX_CHILD_MS", "20000"), ("FAKE_CODEX_CHILD_NO_TURN", "1")],
    );
    let id = create(&s, "A quiet helper: sleeper", None).await;
    wait_for(&s.workbench, &id, |e| texts(e).iter().any(|t| t == "Sleeping")).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    let child = events
        .iter()
        .find_map(|e| match &e.event {
            WbEvent::Task { task_id, .. } => Some(task_id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no child: {events:#?}"));
    let started = Instant::now();
    let refused = s.workbench.task_stop(&id, &child).await;
    assert_eq!(refused, Err("Codex has no turn of that sub-agent under way.".into()));
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Codex has no background commands: moving a command or reading a child's output is refused without asking it.
#[tokio::test]
async fn codex_refuses_background_and_output_without_asking() {
    let s = setup("codex-background-output", &[("FAKE_CODEX_CHILD_MS", "20000")]);
    let id = create(&s, "A quiet helper: sleeper", None).await;
    wait_for(&s.workbench, &id, |e| texts(e).iter().any(|t| t == "Sleeping")).await;
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    let (child, call) = events
        .iter()
        .find_map(|e| match &e.event {
            WbEvent::Task {
                task_id, tool_use_id, ..
            } => Some((task_id.clone(), tool_use_id.clone().unwrap_or_default())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no child: {events:#?}"));
    let refused = s.workbench.task_output(&id, &child).await;
    assert_eq!(refused, Err("Codex can't read the output of this kind of task.".into()));
    let refused = s.workbench.task_background(&id, &call).await;
    assert_eq!(refused, Err("That command is not running in the foreground.".into()));
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &id).await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn relays_codex_approvals() {
    let s = setup("approval", &[]);
    let id = create(&s, "use a tool", None).await;
    let events = wait_for(&s.workbench, &id, |e| approvals(e).len() == 1).await;
    let WbEvent::Approval {
        request_id,
        tool,
        input,
        suggestions,
        description,
        ..
    } = approvals(&events)[0].clone()
    else {
        panic!("no approval")
    };
    assert_eq!((tool.as_str(), input["command"].as_str()), ("Bash", Some("echo hi")));
    assert_eq!(description.as_deref(), Some("Wants to tool"));
    assert_eq!(suggestions[0].label, "Always allow echo hi from now on");
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Approval);
    s.workbench
        .approve(&id, &request_id, Decision::AllowSession, None, Some(0), None)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    assert!(events.iter().any(
        |e| matches!(&e.event, WbEvent::ToolResult { output, is_error: false, .. } if output == "hi (remembered)")
    ));

    // An edit asks too; a denial declines it, and its reason reaches the turn.
    s.workbench
        .send(&id, "edit notes")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| approvals(e).len() == 2).await;
    let WbEvent::Approval {
        request_id,
        tool,
        suggestions,
        ..
    } = approvals(&events)[1].clone()
    else {
        panic!("no approval")
    };
    assert_eq!(tool, "Write");
    assert_eq!(suggestions[0].label, "Allow all edits for this session");
    s.workbench
        .approve(&id, &request_id, Decision::Deny, Some("not there".into()), None, None)
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::ToolResult { output, is_error: true, .. } if output == "Declined."))
    );
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this thread. Told: not there.")
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The person queues two messages during a Codex turn and presses Stop: only the turn under way stops, and the first
/// queued message runs as the next turn. Stop again stops that turn too, and the last queued message then runs to its
/// end.
#[tokio::test]
async fn codex_stop_ends_only_the_turn_under_way_and_the_queued_messages_run_next() {
    let s = setup("stop-keeps-queued", &[]);
    let id = create(&s, "slow work", None).await;
    let read = |events: &[StoredEvent], seq: i64| events.iter().any(|event| event.event == WbEvent::Read { seq });
    wait_for(&s.workbench, &id, |events| read(events, 1)).await;
    for text in ["slow second", "last one"] {
        s.workbench.queue(&id, text).await.unwrap_or_else(|e| panic!("{e}"));
    }
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    let seq = |text: &str| {
        events
            .iter()
            .find(|e| matches!(&e.event, WbEvent::User { text: t, .. } if t == text))
            .map(|e| e.seq)
            .unwrap_or_else(|| panic!("no message {text:?}"))
    };
    let (second, last) = (seq("slow second"), seq("last one"));
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |events| read(events, second)).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted]);
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 3).await;
    assert_eq!(
        turn_ends(&events),
        [TurnStatus::Interrupted, TurnStatus::Interrupted, TurnStatus::Completed]
    );
    assert!(read(&events, last));
    assert!(texts(&events).iter().any(|text| text == "Message 3 of this thread."));
    assert_eq!(s.workbench.halt(&id).unwrap_or_else(|e| panic!("{e}")), None);
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    assert!(s.workbench.is_live(&id), "neither Stop ends the agent");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn an_interrupt_waits_for_codex_to_name_the_turn() {
    let s = setup("interrupt", &[("FAKE_CODEX_LATE_TURN", "1")]);
    let id = create(&s, "slow work", None).await;
    // The thread is live and `turn/start` answered without a turn id; `turn/started` comes 300 ms later.
    for _ in 0..200 {
        if thread(&s.workbench, &id).status == ThreadStatus::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let asked = Instant::now();
    s.workbench.interrupt(&id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted]);
    // Codex ended the turn itself, well before the grace after which the process is stopped.
    assert!(asked.elapsed() < Duration::from_secs(3), "took {:?}", asked.elapsed());
    assert!(s.workbench.is_live(&id), "an interrupt keeps the agent");
    assert!(s.host.ended.lock().unwrap_or_else(|p| p.into_inner()).is_empty());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn resumes_the_codex_thread_after_a_crash() {
    let s = setup("crash", &[]);
    let id = create(&s, "hello", None).await;
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench
        .send(&id, "crash now")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| {
        e.iter()
            .any(|e| matches!(&e.event, WbEvent::Error { text, .. } if text.starts_with("Codex stopped")))
    })
    .await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed, TurnStatus::Error]);
    for _ in 0..200 {
        if !s.workbench.is_live(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // The next message starts Codex again on the same thread (`thread/resume`), which remembers the first turn.
    s.workbench.resume(&id).await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this thread.")
    );
    let resumed = events.iter().rev().find_map(|e| match &e.event {
        WbEvent::Session { resumed, .. } => Some(*resumed),
        _ => None,
    });
    assert_eq!(resumed, Some(true));
    assert_eq!(thread(&s.workbench, &id).nexus_sessions, ["ns_1", "ns_2"]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

fn errors(events: &[StoredEvent]) -> Vec<(&str, Option<&str>)> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Error { text, detail } => Some((text.as_str(), detail.as_deref())),
            _ => None,
        })
        .collect()
}

/// Another Codex process (the ChatGPT app, which shares `~/.codex`) has the thread open: the person reads what to do,
/// once, with Codex's own words in the detail, and not Codex's log as a second "could not start".
#[tokio::test]
async fn a_thread_another_codex_holds_says_so_once() {
    let s = setup("held", &[]);
    let id = create(&s, "hello", None).await;
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    s.workbench
        .send(&id, "crash now")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let crashed = wait_for(&s.workbench, &id, |e| errors(e).len() == 1).await.len();
    for _ in 0..200 {
        if !s.workbench.is_live(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    std::fs::write(s.host.memory.join("writer-held"), "").unwrap_or_else(|e| panic!("{e}"));
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &id, |e| !errors(&e[crashed..]).is_empty()).await;
    for _ in 0..200 {
        if !s.workbench.is_live(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    let after = errors(&events[crashed..]);
    assert_eq!(after.len(), 1, "{after:#?}");
    let (text, detail) = after[0];
    assert!(text.contains("open in another Codex app"), "{text}");
    assert!(text.contains("fork"), "{text}");
    assert!(
        detail.is_some_and(|d| d.contains("already has an active writer")),
        "{detail:?}"
    );
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Error);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn forks_a_codex_thread_from_its_latest_turn() {
    let s = setup("fork", &[]);
    let repo = s.root.join("repo");
    for args in [
        vec!["init", "--quiet", "--initial-branch", "main"],
        vec!["commit", "--quiet", "--allow-empty", "-m", "one"],
    ] {
        let ok = process::command("git")
            .arg("-C")
            .arg(&repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(&args)
            .status()
            .is_ok_and(|status| status.success());
        assert!(ok, "git {args:?}");
    }
    let id = create(&s, "hello", None).await;
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    for _ in 0..400 {
        if !s.workbench.changed_turns(&id).await.unwrap_or_default().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let fork = s
        .workbench
        .fork_thread(&id, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    // Codex's own `thread/fork`: the fork goes on from the first turn.
    s.workbench
        .send(&fork.id, "and then")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &fork.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this thread.")
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_codex_side_chat_forks_the_thread_in_its_folder_without_git() {
    let s = setup("side-chat", &[]);
    let id = create(&s, "hello", None).await;
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    // A side chat needs no repository: it keeps no checkpoints.
    let side = s.workbench.side_chat(&id).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(s.workbench.folder(&side.id).ok(), s.workbench.folder(&id).ok());
    // Codex's own `thread/fork`: the side chat goes on from the session's first turn.
    s.workbench
        .send(&side.id, "and then")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &side.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this thread.")
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_revert_drops_the_later_turns_from_the_codex_thread() {
    let s = setup("revert", &[]);
    let repo = s.root.join("repo");
    for args in [
        &["init", "--quiet", "--initial-branch", "main"][..],
        &["commit", "--quiet", "--allow-empty", "-m", "one"][..],
    ] {
        let ok = process::command("git")
            .arg("-C")
            .arg(&repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .is_ok_and(|status| status.success());
        assert!(ok, "git {args:?}");
    }
    let id = create(&s, "one", None).await;
    for (n, text) in [(1, ""), (2, "two"), (3, "three")] {
        if !text.is_empty() {
            s.workbench.send(&id, text).await.unwrap_or_else(|e| panic!("{e}"));
        }
        wait_for(&s.workbench, &id, |e| turn_ends(e).len() == n).await;
        for _ in 0..400 {
            if s.workbench.changed_turns(&id).await.unwrap_or_default().len() >= n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    let turns = s.workbench.changed_turns(&id).await.unwrap_or_default();
    assert_eq!(turns.len(), 3);
    s.workbench
        .revert_to_turn(&id, turns[0])
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    // Codex resumes its thread and drops turn 2 and 3 (`thread/revert`) before the message: its second turn again.
    s.workbench.send(&id, "again").await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 4).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this thread.")
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Notice { text } if text.contains("could not forget")))
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn update_restart_protects_agents_across_scopes_and_blocks_new_turns() {
    let s = setup("update-restart", &[]);
    let id = create(&s, "slow", None).await;
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Read { .. }))
    })
    .await;
    s.workbench.set_scope("gw|another-org");
    assert!(
        s.workbench
            .reserve_update_restart()
            .expect_err("restart must refuse active work")
            .contains("Code sessions")
    );
    assert!(s.workbench.is_live(&id));
    s.workbench.set_scope("gw|org");
    s.workbench.interrupt(&id).expect("finish active turn");
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    assert!(s.workbench.is_live(&id), "the finished session keeps its idle process");
    s.workbench.set_scope("gw|another-org");
    s.workbench.reserve_update_restart().expect("reserve idle restart");
    s.workbench.set_scope("gw|org");
    assert!(thread(&s.workbench, &id).capabilities.is_some_and(|caps| caps.compact));
    assert!(
        s.workbench.compact(&id, None).await.is_err(),
        "restart blocks compaction"
    );
    assert!(
        s.workbench
            .send(&id, "second turn")
            .await
            .expect_err("restart must refuse active work")
            .contains("restarting")
    );
    assert_eq!(s.host.launches.lock().expect("launches lock").len(), 1);
    s.workbench.cancel_update_restart();
    s.workbench
        .send(&id, "second turn")
        .await
        .expect("send after canceled restart");
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(s.host.launches.lock().expect("launches lock").len(), 1);
    s.workbench.reserve_update_restart().expect("reserve restart again");
    s.workbench.shutdown().await;
    assert!(!s.workbench.is_live(&id), "idle process stops before installation");
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    s.workbench.cancel_update_restart();
    s.workbench
        .send(&id, "third turn")
        .await
        .expect("resume after failed install");
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 3).await;
    assert_eq!(s.host.launches.lock().expect("launches lock").len(), 2);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn update_restart_protects_background_children_after_the_lead_turn_ends() {
    let s = setup("update-restart-child", &[("FAKE_CODEX_CHILD_MS", "60000")]);
    let id = create(&s, "sleeper", None).await;
    wait_for(&s.workbench, &id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    assert!(approvals(&s.workbench.events(&id, 0).expect("events")).is_empty());
    s.workbench.set_scope("gw|another-org");
    assert!(
        s.workbench.reserve_update_restart().is_err(),
        "background child still works"
    );
    s.workbench.set_scope("gw|org");
    s.workbench.shutdown().await;
    s.workbench
        .reserve_update_restart()
        .expect("no background work remains");
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn confirmed_update_restart_freezes_only_the_work_the_person_confirmed() {
    let s = setup("confirmed-update-restart", &[]);
    let first = create(&s, "slow", None).await;
    wait_for(&s.workbench, &first, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Read { .. }))
    })
    .await;
    let confirmed = s.workbench.update_restart_sessions().expect("active sessions");
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].id, first);
    *s.host.recovery_identity.lock().expect("recovery identity lock") = "another-person".into();
    assert_eq!(
        s.workbench.update_restart_sessions().expect("active update sessions")[0].identity,
        "fixture-person",
        "switching accounts cannot reassign an active agent"
    );
    let second = create(&s, "slow", None).await;
    wait_for(&s.workbench, &second, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Read { .. }))
    })
    .await;
    assert!(
        s.workbench
            .reserve_confirmed_update_restart(&confirmed)
            .expect_err("new work needs confirmation")
            .contains("More agents")
    );
    assert!(s.workbench.is_live(&first) && s.workbench.is_live(&second));
    let confirmed = s.workbench.update_restart_sessions().expect("refresh confirmation");
    s.workbench.set_scope("gw|another-org");
    let saved = s
        .workbench
        .reserve_confirmed_update_restart(&confirmed)
        .expect("confirmed across scopes");
    assert_eq!(saved.len(), 2);
    assert!(
        s.workbench.reserve_confirmed_update_restart(&saved).is_err(),
        "a second click cannot release the first reservation"
    );
    s.workbench.set_scope("gw|org");
    assert!(s.workbench.send(&first, "must wait").await.is_err());
    s.workbench.shutdown().await;
    assert!(!s.workbench.is_live(&first) && !s.workbench.is_live(&second));
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn update_recovery_continues_the_saved_conversation_once_and_checks_scope() {
    let s = setup("update-recovery", &[]);
    let id = create(&s, "slow", None).await;
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Read { .. }))
    })
    .await;
    let saved = s.workbench.update_restart_sessions().expect("snapshot");
    s.workbench.reserve_confirmed_update_restart(&saved).expect("reserve");
    s.workbench.shutdown().await;
    s.workbench.cancel_update_restart();
    *s.host.recovery_identity.lock().expect("recovery identity lock") = "another-person".into();
    assert!(
        s.workbench.resume_after_update(&saved[0]).await.is_err(),
        "another account cannot continue saved work"
    );
    *s.host.recovery_identity.lock().expect("recovery identity lock") = "fixture-person".into();
    s.workbench.set_scope("gw|different-org");
    assert!(s.workbench.resume_after_update(&saved[0]).await.is_err());
    s.workbench.set_scope("gw|org");
    s.workbench
        .resume_after_update(&saved[0])
        .await
        .expect("continue saved conversation");
    s.workbench
        .resume_after_update(&saved[0])
        .await
        .expect("repeated click is harmless");
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    s.workbench
        .resume_after_update(&saved[0])
        .await
        .expect("completed continuation is not replayed");
    let messages = s
        .workbench
        .events(&id, 0)
        .expect("events")
        .into_iter()
        .filter(|event| matches!(&event.event, WbEvent::User { text, .. } if text == nexus_workbench::CONTINUE))
        .count();
    assert_eq!(messages, 1);
    assert_eq!(s.host.launches.lock().expect("launches lock").len(), 2);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn update_recovery_retries_a_failed_launch_without_replaying_a_success() {
    let s = setup("update-recovery-retry", &[]);
    let id = create(&s, "slow", None).await;
    wait_for(&s.workbench, &id, |events| {
        events.iter().any(|event| matches!(event.event, WbEvent::Read { .. }))
    })
    .await;
    let saved = s.workbench.update_restart_sessions().expect("snapshot");
    s.workbench.reserve_confirmed_update_restart(&saved).expect("reserve");
    s.workbench.shutdown().await;
    s.workbench.cancel_update_restart();
    s.host.fail_next_launch.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(s.workbench.resume_after_update(&saved[0]).await.is_err());
    assert!(!s.workbench.is_live(&id));
    s.workbench
        .resume_after_update(&saved[0])
        .await
        .expect("retry the failed launch");
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 3).await;
    s.workbench
        .resume_after_update(&saved[0])
        .await
        .expect("do not replay the successful retry");
    assert_eq!(s.host.launches.lock().expect("launches lock").len(), 3);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn update_restart_protects_pending_approvals() {
    let s = setup("update-restart-approval", &[]);
    let id = create(&s, "tool", None).await;
    wait_for(&s.workbench, &id, |events| !approvals(events).is_empty()).await;
    s.workbench.set_scope("gw|another-org");
    assert!(
        s.workbench.reserve_update_restart().is_err(),
        "approval needs an answer"
    );
    s.workbench.set_scope("gw|org");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn repeated_creation_requests_share_one_session_and_first_prompt() {
    let s = setup("create-once", &[]);
    let request_id = uuid::Uuid::new_v4().to_string();
    let new = || NewThread {
        name: None,
        project_id: s.project.clone(),
        agent: SessionAgent::Codex,
        model: None,
        permission_mode: PermissionMode::Default,
        effort: None,
        via: Via::Nexus,
        provider: None,
        text: "First prompt exactly once".into(),
        images: vec![],
        worktree: None,
        parent: None,
        orchestration: false,
    };
    let (first, second) = tokio::join!(
        s.workbench.create_thread_once(new(), &request_id),
        s.workbench.create_thread_once(new(), &request_id),
    );
    let first = first.expect("first creation");
    assert_eq!(first.id, format!("t_{request_id}"));
    assert_eq!(second.expect("same creation").id, first.id);
    let events = wait_for(&s.workbench, &first.id, |events| !turn_ends(events).is_empty()).await;
    assert_eq!(
        events
            .iter()
            .filter(|entry| matches!(entry.event, WbEvent::User { .. }))
            .count(),
        1
    );
    assert_eq!(s.host.launches.lock().expect("launch log").len(), 1);
    assert_eq!(s.workbench.threads().expect("sessions").len(), 1);
    assert_eq!(
        s.workbench
            .create_thread_once(new(), &request_id)
            .await
            .expect("reconciled creation")
            .id,
        first.id
    );
    assert_eq!(s.host.launches.lock().expect("launch log").len(), 1);
    assert!(s.workbench.create_thread_once(new(), "not-a-uuid").await.is_err());
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn manual_compaction_targets_one_quiet_process_without_a_prompt() {
    let s = setup("manual-compact", &[]);
    let id = create(&s, "hello", None).await;
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    s.workbench.compact(&id, None).await.unwrap_or_else(|e| panic!("{e}"));
    assert!(
        s.workbench.compact(&id, None).await.is_err(),
        "duplicate while the native turn runs"
    );
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::User { .. }))
            .count(),
        1
    );
    let targets = std::fs::read_to_string(s.host.memory.join("compactions")).unwrap_or_default();
    assert_eq!(targets.lines().count(), 1);
    assert!(targets.trim().starts_with("thr_"));
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, WbEvent::Notice { text } if text.contains("compacted")))
    );
    s.workbench.restart(&id).unwrap_or_else(|e| panic!("{e}"));
    assert!(s.workbench.compact(&id, None).await.is_err());
}

#[tokio::test]
async fn manual_compaction_refusal_ends_its_turn_without_prompt_fallback() {
    let s = setup("compact-refused", &[("FAKE_CODEX_COMPACT", "refuse")]);
    let id = create(&s, "hello", None).await;
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    assert!(s.workbench.compact(&id, Some("instructions".into())).await.is_err());
    assert!(!s.host.memory.join("compactions").exists());
    assert!(s.workbench.compact(&id, None).await.is_err());
    let events = wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(turn_ends(&events).last(), Some(&TurnStatus::Error));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::User { .. }))
            .count(),
        1
    );
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
}

#[tokio::test]
async fn manual_compaction_timeout_never_retries_as_a_model_prompt() {
    let s = setup("compact-timeout", &[("FAKE_CODEX_COMPACT", "silent")]);
    let id = create(&s, "hello", None).await;
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    let error = s.workbench.compact(&id, None).await.expect_err("bounded wait");
    assert!(error.contains("in time"));
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Running);
    assert!(s.workbench.compact(&id, None).await.is_err());
    assert_eq!(
        std::fs::read_to_string(s.host.memory.join("compactions"))
            .unwrap_or_default()
            .lines()
            .count(),
        1
    );
    assert_eq!(
        s.workbench
            .events(&id, 0)
            .unwrap_or_default()
            .iter()
            .filter(|event| matches!(event.event, WbEvent::User { .. }))
            .count(),
        1
    );
    s.workbench.stop(&id).unwrap_or_else(|e| panic!("{e}"));
}

#[tokio::test]
async fn connection_uses_the_selected_process_and_safe_paginated_runtime_evidence() {
    let s = setup("connection-safe", &[]);
    let id = create(&s, "hello", None).await;
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    let before = thread(&s.workbench, &id);
    let connection = s
        .workbench
        .connection(&id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(connection.model.as_deref(), Some("gpt-fake-1"));
    assert_eq!(
        thread(&s.workbench, &id).model,
        None,
        "a reported default must never pin the requested model"
    );
    assert_eq!(
        connection.account.as_ref().and_then(|account| account.email.as_deref()),
        Some("current@example.test")
    );
    assert_eq!(connection.attribution.team.as_deref(), Some("fixture-team"));
    assert_eq!(connection.attribution.access.as_deref(), Some("byok"));
    let servers = connection.mcp.expect("reported list");
    assert_eq!(
        servers
            .iter()
            .map(|server| (server.name.as_str(), server.status.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("tracker", "connected"),
            ("configured-only", "unavailable"),
            ("offline", "failed")
        ]
    );
    assert_eq!(
        connection.skills.as_deref(),
        Some(["fixture-check".to_string()].as_slice())
    );
    assert!(
        connection.plugins.is_none(),
        "never probe under-development plugin methods"
    );
    let current = thread(&s.workbench, &id);
    assert_eq!(
        current.updated_at, before.updated_at,
        "reading status must not reorder session activity"
    );
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert_eq!(turn_ends(&events).len(), 1);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::User { .. }))
            .count(),
        1
    );
    let serialized = serde_json::to_string(&events).expect("serialize safe observations");
    assert!(!serialized.contains("SECRET_"));
    assert!(!serialized.contains("/private/fixture"));
    let observations = events
        .iter()
        .filter(|event| matches!(event.event, WbEvent::Connection { .. }))
        .count();
    s.workbench
        .connection(&id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        s.workbench
            .events(&id, 0)
            .unwrap_or_default()
            .iter()
            .filter(|event| matches!(event.event, WbEvent::Connection { .. }))
            .count(),
        observations,
        "identical refreshes are not duplicated in history"
    );
    s.workbench.stop(&id).unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn newer_same_process_notifications_win_over_delayed_status_reads() {
    let s = setup("connection-race", &[("FAKE_CODEX_CONNECTION", "race")]);
    let id = create(&s, "hello", None).await;
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    s.workbench
        .connection(&id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let connection = thread(&s.workbench, &id)
        .capabilities
        .and_then(|capabilities| capabilities.connection)
        .expect("live observations");
    assert_eq!(
        connection.account.as_ref().and_then(|account| account.email.as_deref()),
        Some("current@example.test")
    );
    assert_eq!(
        connection
            .mcp
            .as_ref()
            .and_then(|servers| servers.iter().find(|server| server.name == "tracker"))
            .map(|server| server.status.as_str()),
        Some("failed")
    );
    assert!(
        connection.skills.is_none(),
        "skills/changed invalidates an older discovery reply"
    );
    let events = s.workbench.events(&id, 0).unwrap_or_default();
    assert_eq!(turn_ends(&events).len(), 1);
    assert!(
        !serde_json::to_string(&events)
            .expect("safe history")
            .contains("SECRET_")
    );
    s.workbench.stop(&id).unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn unknown_status_operations_are_unavailable_without_affecting_the_turn() {
    let s = setup("connection-unsupported", &[("FAKE_CODEX_CONNECTION", "unavailable")]);
    let id = create(&s, "hello", None).await;
    wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
    let connection = s
        .workbench
        .connection(&id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(connection.mcp.is_none());
    assert!(connection.mcp_unavailable.is_some());
    assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
    assert_eq!(turn_ends(&s.workbench.events(&id, 0).unwrap_or_default()).len(), 1);
    s.workbench.stop(&id).unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn failed_or_timed_out_refresh_clears_old_identity_and_discovery_without_a_turn() {
    for mode in ["expires", "timeout"] {
        let s = setup(&format!("connection-{mode}"), &[("FAKE_CODEX_CONNECTION", mode)]);
        let id = create(&s, "hello", None).await;
        wait_for(&s.workbench, &id, |events| turn_ends(events).len() == 1).await;
        let first = s.workbench.connection(&id).await.expect("first observation");
        assert!(first.account.is_some() && first.skills.is_some() && first.mcp.is_some());
        assert_eq!(first.provider.as_deref(), Some("openai"));
        let started = Instant::now();
        let next = s
            .workbench
            .connection(&id)
            .await
            .expect("unavailable fields are a usable snapshot");
        assert!(started.elapsed() < Duration::from_secs(6));
        assert!(next.account.is_none() && next.skills.is_none() && next.mcp.is_none());
        assert!(next.mcp_unavailable.is_some());
        assert_eq!(turn_ends(&s.workbench.events(&id, 0).unwrap_or_default()).len(), 1);
        assert_eq!(thread(&s.workbench, &id).status, ThreadStatus::Idle);
        s.workbench.stop(&id).expect("stop fake");
    }
}

/// A new Codex session on the person's plan starts on the active ChatGPT account. A started session keeps its account,
/// because Codex keeps each account's conversations apart. With no choice saved, Codex runs on its own login.
#[tokio::test]
async fn a_codex_session_on_the_plan_starts_on_the_active_account_and_keeps_it() {
    let s = setup("plan-account", &[]);
    let (accounts, home) = (s.root.join("accounts"), s.root.join("home-codex"));
    let _ = std::fs::create_dir_all(&home);
    let extra = nexus_workbench::accounts::add(&accounts, SessionAgent::Codex, &home).unwrap_or_else(|e| panic!("{e}"));
    let workbench = Workbench::new(
        Store::open(&s.root.join("accounts.sqlite3")).unwrap_or_else(|e| panic!("{e}")),
        s.host.clone(),
        Config {
            scope: "gw|org".into(),
            worktrees: Some(s.root.join("worktrees")),
            accounts: Some(accounts.clone()),
            shell: nexus_workbench::process::Shell::sh(),
            ..Config::default()
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let project = workbench
        .add_project(&s.root.join("repo"))
        .map(|p| p.id)
        .unwrap_or_default();
    let on_plan = |text: &str| NewThread {
        project_id: project.clone(),
        agent: SessionAgent::Codex,
        model: None,
        permission_mode: PermissionMode::Default,
        effort: None,
        via: Via::Subscription,
        provider: None,
        text: text.into(),
        images: vec![],
        worktree: None,
        parent: None,
        orchestration: false,
        name: None,
    };
    let last_account_dir = || {
        s.host
            .launches
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .last()
            .and_then(|launch| launch.account_dir.clone())
    };

    // No choice saved: Codex's own login, as before.
    let own = workbench
        .create_thread(on_plan("hello"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &own, |e| turn_ends(e).len() == 1).await;
    assert_eq!(thread(&workbench, &own).account, None);
    assert_eq!(last_account_dir(), None);

    // The person chooses the extra account: a new session starts there. Claude's choice stays as it was.
    workbench.use_account_of(SessionAgent::Codex, Some(extra.clone()));
    let chosen = workbench
        .create_thread(on_plan("hi"))
        .await
        .map(|t| t.id)
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &chosen, |e| turn_ends(e).len() == 1).await;
    assert_eq!(thread(&workbench, &chosen).account.as_deref(), Some(extra.as_str()));
    assert_eq!(last_account_dir(), Some(accounts.join("codex").join(&extra)));
    assert_eq!(workbench.active_account_of(SessionAgent::Codex), Some(extra.clone()));
    assert_eq!(workbench.active_account(), None);

    // The first session starts again on its own login: its conversation is there.
    workbench.stop(&own).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&workbench, &own).await;
    workbench.send(&own, "again").await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &own, |e| turn_ends(e).len() == 2).await;
    assert_eq!(thread(&workbench, &own).account, None);
    assert_eq!(last_account_dir(), None);

    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
