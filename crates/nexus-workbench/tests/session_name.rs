//! A session's unique name (session_name.rs) through the workbench: set at creation or by an update, refused
//! when another session holds it (the refusal names the holder), cleared, and given to Claude Code as `--name` on a
//! fresh start and on a resume. The stand-in `claude` records what it was started with (`FAKE_LAUNCH_RECORD`).

mod support;

use std::time::{Duration, Instant};

use nexus_workbench::ThreadPatch;
use serde_json::Value;
use support::*;

fn named(name: Option<&str>) -> ThreadPatch {
    ThreadPatch {
        name: Some(name.map(String::from)),
        ..ThreadPatch::default()
    }
}

/// Every launch from here on records its arguments in the setup's `launch.json`.
fn record_launches(s: &Setup) {
    let path = s.root.join("launch.json");
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_LAUNCH_RECORD".into(), path.to_string_lossy().into_owned());
}

/// The arguments of the last launch recorded, once `ready` holds for them.
async fn launched(s: &Setup, ready: impl Fn(&[String]) -> bool) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let args: Vec<String> = std::fs::read_to_string(s.root.join("launch.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|value| {
                value["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|arg| arg.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if ready(&args) {
            return args;
        }
        assert!(Instant::now() < deadline, "no launch recorded: {args:?}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The value after `flag` in `args`.
fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let at = args.iter().position(|arg| arg == flag)?;
    args.get(at + 1).map(String::as_str)
}

/// The name Claude Code was given: one `--name=<name>` entry, so the value is never an argument of its own.
fn name_of(args: &[String]) -> Option<&str> {
    assert!(!args.iter().any(|arg| arg == "--name"), "--name is one entry: {args:?}");
    args.iter().find_map(|arg| arg.strip_prefix("--name="))
}

#[tokio::test]
async fn a_name_is_unique_refused_with_its_holder_and_can_be_cleared() {
    let s = setup("name-unique", bounded());
    let first = root(&s, new_root(&s.project, "one")).await;
    let second = root(&s, new_root(&s.project, "two")).await;
    // No session has a name until someone gives it one.
    assert_eq!(first.name, None);
    let set = s
        .workbench
        .update(&first.id, named(Some("orchestrator")))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(set.name.as_deref(), Some("orchestrator"));
    assert_eq!(set.updated_at, first.updated_at, "a name is not activity");
    // Setting the same name again on its holder is no clash.
    assert!(s.workbench.update(&first.id, named(Some("orchestrator"))).is_ok());
    let clash = s.workbench.update(&second.id, named(Some("orchestrator")));
    assert_eq!(clash, Err(format!("orchestrator is taken by {}.", first.id)));
    // Case does not make a second name: Orchestrator is the same one.
    let clash = s.workbench.update(&second.id, named(Some("Orchestrator")));
    assert_eq!(clash, Err(format!("Orchestrator is taken by {}.", first.id)));
    // Its holder may change its case.
    assert!(s.workbench.update(&first.id, named(Some("Orchestrator"))).is_ok());
    assert!(s.workbench.update(&first.id, named(Some("orchestrator"))).is_ok());
    assert_eq!(thread(&s.workbench, &second.id).name, None, "a refusal changes nothing");
    // A name that is no name, or looks like an id, is refused.
    assert!(s.workbench.update(&second.id, named(Some("t_x"))).is_err());
    assert!(s.workbench.update(&second.id, named(Some("two words"))).is_err());
    assert!(
        s.workbench
            .update(&second.id, named(Some("--dangerously-skip-permissions")))
            .is_err()
    );
    // A creation that asks for a taken name is refused before anything starts.
    let mut taken = new_root(&s.project, "three");
    taken.name = Some("orchestrator".into());
    let refused = s.workbench.create_thread(taken).await;
    assert_eq!(
        refused.map(|thread| thread.id),
        Err(format!("orchestrator is taken by {}.", first.id))
    );
    assert_eq!(s.workbench.threads().unwrap_or_default().len(), 2);
    // Cleared, the name is free for another session.
    let cleared = s
        .workbench
        .update(&first.id, named(None))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(cleared.name, None);
    let moved = s
        .workbench
        .update(&second.id, named(Some("orchestrator")))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(moved.name.as_deref(), Some("orchestrator"));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn claude_code_gets_the_name_on_a_start_and_on_a_resume_only_when_set() {
    let s = setup("name-launch", bounded());
    record_launches(&s);
    let unnamed = root(&s, new_root(&s.project, "plain")).await;
    let args = launched(&s, |args| after(args, "--session-id").is_some()).await;
    assert_eq!(name_of(&args), None, "no name, no --name: {args:?}");
    s.workbench.stop(&unnamed.id).unwrap_or_else(|e| panic!("{e}"));

    let mut new = new_root(&s.project, "named");
    new.name = Some("orchestrator".into());
    let started = root(&s, new).await;
    assert_eq!(started.name.as_deref(), Some("orchestrator"));
    let args = launched(&s, |args| args.iter().any(|arg| arg.starts_with("--name"))).await;
    assert!(after(&args, "--session-id").is_some(), "a fresh start: {args:?}");
    assert_eq!(name_of(&args), Some("orchestrator"));

    // A rename reaches the next start, which resumes the conversation.
    s.workbench
        .update(&started.id, named(Some("lead")))
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench.stop(&started.id).unwrap_or_else(|e| panic!("{e}"));
    let deadline = Instant::now() + Duration::from_secs(20);
    while thread(&s.workbench, &started.id).capabilities.is_some() {
        assert!(Instant::now() < deadline, "the agent did not stop");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    s.workbench
        .send(&started.id, "again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let args = launched(&s, |args| after(args, "--resume").is_some()).await;
    assert_eq!(name_of(&args), Some("lead"), "{args:?}");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The `/rename` lines the stand-in `claude` processes were sent, in order (`$FAKE_CLAUDE_DIR/renames`).
fn renames(s: &Setup) -> Vec<String> {
    std::fs::read_to_string(s.root.join("memory").join("renames"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

#[tokio::test]
async fn an_accepted_name_reaches_the_running_claude_once_and_shows_no_turn() {
    let s = setup("name-forward", bounded());
    record_launches(&s);
    let first = root(&s, new_root(&s.project, "one")).await;
    let second = root(&s, new_root(&s.project, "two")).await;
    s.workbench
        .update(&first.id, named(Some("lead")))
        .unwrap_or_else(|e| panic!("{e}"));
    // Refused, cleared: nothing goes to the agent.
    assert!(s.workbench.update(&second.id, named(Some("lead"))).is_err());
    s.workbench
        .update(&second.id, named(None))
        .unwrap_or_else(|e| panic!("{e}"));
    // A stand-in reads its lines in order, so a turn that ends after the updates proves what each was sent.
    for id in [&first.id, &second.id] {
        s.workbench.send(id, "again").await.unwrap_or_else(|e| panic!("{e}"));
        wait_for(&s.workbench, id, |e| turn_ends(e).len() == 2).await;
    }
    assert_eq!(renames(&s), ["/rename lead"]);
    // The rename is no message and no turn: one turn end per message, and no "/rename" among the messages.
    let events = s.workbench.events(&first.id, 0).unwrap_or_default();
    let said: Vec<String> = users(&events).into_iter().map(|(text, _)| text).collect();
    assert_eq!(said, ["one", "again"]);
    assert_eq!(turn_ends(&events).len(), 2);

    // With no process, the name waits for the next start's --name.
    s.workbench.stop(&first.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &first.id).await;
    s.workbench
        .update(&first.id, named(Some("solo")))
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .send(&first.id, "later")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let args = launched(&s, |args| args.iter().any(|arg| arg == "--name=solo")).await;
    assert_eq!(name_of(&args), Some("solo"));
    wait_for(&s.workbench, &first.id, |e| turn_ends(e).len() == 3).await;
    assert_eq!(renames(&s), ["/rename lead"]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The texts the agent wrote into the transcript.
fn texts(events: &[nexus_workbench::event::StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            nexus_workbench::event::WbEvent::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The number of turn starts in a log.
fn turn_starts(events: &[nexus_workbench::event::StoredEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e.event, nexus_workbench::event::WbEvent::TurnStart { .. }))
        .count()
}

/// Waits until the stand-in `claude` processes were sent exactly `expected` `/rename` lines.
async fn until_renames(s: &Setup, expected: &[&str]) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while renames(s) != expected {
        assert!(
            Instant::now() < deadline,
            "renames {:?}, expected {expected:?}",
            renames(s)
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_name_on_an_idle_claude_opens_no_turn_and_writes_no_text() {
    let s = setup("name-idle", bounded());
    let first = root(&s, new_root(&s.project, "one")).await;
    s.workbench
        .update(&first.id, named(Some("lead")))
        .unwrap_or_else(|e| panic!("{e}"));
    until_renames(&s, &["/rename lead"]).await;
    // The stand-in answers its lines in order: a message's turn end proves the rename's answer was read first.
    s.workbench
        .send(&first.id, "again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &first.id, |e| turn_ends(e).len() == 2).await;
    // Claude Code 2.1.312 answers /rename with a <synthetic> "Session renamed to: lead": it is no transcript text.
    assert!(
        !texts(&events).iter().any(|text| text.contains("Session renamed")),
        "{:?}",
        texts(&events)
    );
    // One turn per message: the rename opened none, so "again" started a turn of its own.
    assert_eq!(turn_starts(&events), 2);
    let said: Vec<String> = users(&events).into_iter().map(|(text, _)| text).collect();
    assert_eq!(said, ["one", "again"]);
    let status = thread(&s.workbench, &first.id).status;
    assert_eq!(status, nexus_workbench::event::ThreadStatus::Idle, "{status:?}");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_name_set_during_a_turn_reaches_claude_once_when_the_turn_ends() {
    let s = setup("name-busy", bounded());
    let first = root(&s, new_root(&s.project, "one")).await;
    // "relay" keeps the turn open until the test writes END.
    s.workbench
        .send(&first.id, "relay")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &first.id, |e| turn_starts(e) == 2).await;
    // Two names during the turn: only the last one is sent.
    for name in ["test1", "test2"] {
        s.workbench
            .update(&first.id, named(Some(name)))
            .unwrap_or_else(|e| panic!("{e}"));
    }
    assert!(renames(&s).is_empty(), "nothing goes in mid-turn: {:?}", renames(&s));
    trigger_with(&s, "relay", "END");
    wait_for(&s.workbench, &first.id, |e| turn_ends(e).len() == 2).await;
    until_renames(&s, &["/rename test2"]).await;
    // Once: a later turn finds nothing more sent, and the rename made no turn of its own.
    s.workbench
        .send(&first.id, "again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &first.id, |e| turn_ends(e).len() == 3).await;
    assert_eq!(renames(&s), ["/rename test2"]);
    assert_eq!(turn_starts(&events), 3);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
