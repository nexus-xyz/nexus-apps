//! Antigravity sessions, run by the stand-in `fake-agy`. Each mode passes its `toolPermission` via `--settings`, and
//! Auto (or the Sandbox switch) passes `--sandbox`. Over ACP, Auto sets `auto` when agy offers it, and `accept-edits`
//! when it does not. Full access always sets `accept-edits`. Each keeps the session in its chosen mode. Every ask agy
//! still makes reaches the person, in every mode.

mod support;

use std::sync::atomic::Ordering;

use nexus_workbench::NewThread;
use nexus_workbench::event::{Decision, PermissionMode, SessionAgent, StoredEvent, ThreadStatus, TurnStatus, WbEvent};
use support::*;

/// An Antigravity session in `mode`, whose first message is `text`.
fn agy(s: &Setup, mode: PermissionMode, text: &str) -> NewThread {
    NewThread {
        agent: SessionAgent::Antigravity,
        permission_mode: mode,
        orchestration: false,
        ..new_root(&s.project, text)
    }
}

/// agy offers no `auto` mode, and refuses it, as agy 1.5.10 does for a Gemini Enterprise account.
fn without_auto(s: &Setup) {
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_AGY_NO_AUTO".into(), "1".into());
}

/// The modes agy took, in order.
fn modes(s: &Setup) -> Vec<String> {
    std::fs::read_to_string(s.host.memory.join("modes"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

/// Each start's arguments, in order.
fn starts(s: &Setup) -> Vec<Vec<String>> {
    std::fs::read_to_string(s.host.memory.join("args"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect()
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

/// The person allows `ask`, and the turn ends with the command's result.
async fn allow_and_finish(s: &Setup, id: &str, ask: &str) -> Vec<StoredEvent> {
    s.workbench
        .approve(id, ask, Decision::Allow, None, None, None)
        .unwrap_or_else(|e| panic!("the ask was answered without the person: {e}"));
    let events = wait_for(&s.workbench, id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(tool_results(&events), [("notes.txt".to_string(), false)]);
    events
}

/// A session in `mode` on agy that offers `auto` or not, whose first turn runs a command the person allows: the
/// modes agy took, the mode the session showed while agy asked, and the notices.
async fn run_command(
    name: &str,
    mode: PermissionMode,
    offers_auto: bool,
) -> (Vec<String>, PermissionMode, Vec<String>) {
    let s = setup(name, Default::default());
    if !offers_auto {
        without_auto(&s);
    }
    let created = s
        .workbench
        .create_thread(agy(&s, mode, "please run ls"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let ask = approval(&s.workbench, &created.id).await;
    let shown = thread(&s.workbench, &created.id).permission_mode;
    let events = allow_and_finish(&s, &created.id, &ask).await;
    // The first start of the app: no run has said yet whether agy offers `auto`.
    assert_eq!(starts(&s), [nexus_workbench::acp::args(mode, false, None)]);
    let took = modes(&s);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
    (took, shown, notices(&events))
}

const ACCOUNT_POLICY_NOTICE: &str = "Your Antigravity account policy decides which commands run without asking.";

/// Without agy's `auto`, Auto and Full access set `accept-edits`, keep their mode, and show the account-policy notice
/// once: the account's policy can still make agy ask in either.
#[tokio::test]
async fn full_access_and_auto_without_agy_auto_use_accept_edits_over_acp_and_keep_their_mode() {
    for (name, mode) in [
        ("agy-no-auto-full", PermissionMode::BypassPermissions),
        ("agy-no-auto-auto", PermissionMode::Auto),
    ] {
        assert_eq!(
            run_command(name, mode, false).await,
            (
                vec!["accept-edits".to_string()],
                mode,
                vec![ACCOUNT_POLICY_NOTICE.to_string()]
            ),
            "{mode:?}"
        );
    }
}

/// Where agy offers `auto`, Auto takes it and Full access still sets `accept-edits`. Both keep their mode, and the
/// person still answers agy's ask.
#[tokio::test]
async fn full_access_and_auto_with_agy_auto_keep_their_mode_and_still_ask_the_person() {
    for (name, mode, took) in [
        ("agy-auto-full", PermissionMode::BypassPermissions, "accept-edits"),
        ("agy-auto-auto", PermissionMode::Auto, "auto"),
    ] {
        assert_eq!(
            run_command(name, mode, true).await,
            (vec![took.to_string()], mode, Vec::new()),
            "{mode:?}"
        );
    }
}

/// Switching to a mode with different `--settings` or `--sandbox` restarts an idle agy at the next message and loads
/// the same conversation; switching between modes with identical CLI args keeps the running agy.
#[tokio::test]
async fn mode_switch_restarts_an_idle_session_when_launch_args_change() {
    let s = setup("agy-mode-restart", Default::default());
    without_auto(&s);
    let created = s
        .workbench
        .create_thread(agy(&s, PermissionMode::Default, "hello"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;

    // Switch Default -> Auto: CLI args change (`request-review` -> `--sandbox` + `proceed-in-sandbox`).
    s.workbench
        .update(&created.id, mode(PermissionMode::Auto))
        .unwrap_or_else(|e| panic!("{e}"));
    until("the session shows Auto", || {
        thread(&s.workbench, &created.id).permission_mode == PermissionMode::Auto
    })
    .await;
    s.workbench
        .send(&created.id, "in auto")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 2).await;

    // Switch Auto -> AcceptEdits: CLI args change back to `request-review` without `--sandbox`.
    s.workbench
        .update(&created.id, mode(PermissionMode::AcceptEdits))
        .unwrap_or_else(|e| panic!("{e}"));
    until("the session shows AcceptEdits", || {
        thread(&s.workbench, &created.id).permission_mode == PermissionMode::AcceptEdits
    })
    .await;
    s.workbench
        .send(&created.id, "in accept edits")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 3).await;

    // Switch AcceptEdits -> Default: CLI args are identical (`request-review`), so agy is NOT restarted.
    s.workbench
        .update(&created.id, mode(PermissionMode::Default))
        .unwrap_or_else(|e| panic!("{e}"));
    until("the session shows Default", || {
        thread(&s.workbench, &created.id).permission_mode == PermissionMode::Default
    })
    .await;
    s.workbench
        .send(&created.id, "back in default")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 4).await;

    assert_eq!(
        starts(&s),
        [
            nexus_workbench::acp::args(PermissionMode::Default, false, None),
            nexus_workbench::acp::args(PermissionMode::Auto, false, Some(false)),
            nexus_workbench::acp::args(PermissionMode::AcceptEdits, false, Some(false)),
        ]
    );
    let loads = std::fs::read_to_string(s.host.memory.join("loads")).unwrap_or_default();
    assert_eq!(
        loads.lines().collect::<Vec<_>>(),
        ["fake-agy-session", "fake-agy-session"]
    );
    assert_eq!(notices(&events), [ACCOUNT_POLICY_NOTICE]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Auto on a personal plan: the app's first start of agy cannot know agy offers `auto`, so it runs Auto in the sandbox.
/// Once agy said, the next message starts agy again without the sandbox, on the same conversation, and the one after
/// keeps it.
#[tokio::test]
async fn personal_auto_starts_again_without_the_sandbox_once_agy_offers_auto() {
    let s = setup("agy-personal-auto", Default::default());
    let created = s
        .workbench
        .create_thread(agy(&s, PermissionMode::Auto, "hello"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    for (text, turns) in [("again", 2), ("and again", 3)] {
        s.workbench
            .send(&created.id, text)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == turns).await;
    }
    assert_eq!(
        starts(&s),
        [
            vec![
                "--sandbox".to_string(),
                "--settings".into(),
                r#"{"toolPermission":"proceed-in-sandbox"}"#.into(),
                "acp".into()
            ],
            vec!["acp".to_string()],
        ]
    );
    let loads = std::fs::read_to_string(s.host.memory.join("loads")).unwrap_or_default();
    assert_eq!(loads.lines().collect::<Vec<_>>(), ["fake-agy-session"]);
    assert_eq!(modes(&s), ["auto", "auto"]);
    assert_eq!(
        notices(&s.workbench.events(&created.id, 0).unwrap_or_default()),
        Vec::<String>::new()
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// agy lists `auto` and refuses it (agy 1.5.10 on Gemini Enterprise). An Ask first session opening after an Auto one
/// asks for no `auto`, so its list proves nothing: the Auto session's next message keeps its sandboxed agy.
#[tokio::test]
async fn a_listed_but_refused_auto_keeps_auto_in_the_sandbox_when_another_session_opens() {
    let s = setup("agy-listed-auto", Default::default());
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_AGY_REFUSE_AUTO".into(), "1".into());
    let auto = s
        .workbench
        .create_thread(agy(&s, PermissionMode::Auto, "hello"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &auto.id, |e| turn_ends(e).len() == 1).await;
    let ask = s
        .workbench
        .create_thread(agy(&s, PermissionMode::Default, "hello"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &ask.id, |e| turn_ends(e).len() == 1).await;
    s.workbench
        .send(&auto.id, "again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &auto.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(
        starts(&s),
        [
            nexus_workbench::acp::args(PermissionMode::Auto, false, None),
            nexus_workbench::acp::args(PermissionMode::Default, false, Some(false)),
        ]
    );
    assert_eq!(launches_of(&s, &auto.id).len(), 1);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A switch to Full access while agy waits on an ask: agy takes `accept-edits`, and the ask still waits for the
/// person.
#[tokio::test]
async fn switch_to_full_access_leaves_the_waiting_ask_to_the_person() {
    let s = setup("agy-switch", Default::default());
    let created = s
        .workbench
        .create_thread(agy(&s, PermissionMode::Default, "please run ls"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let ask = approval(&s.workbench, &created.id).await;

    s.workbench
        .update(&created.id, mode(PermissionMode::BypassPermissions))
        .unwrap_or_else(|e| panic!("{e}"));
    // A second switch, answered after the first: once it is saved, the first switch's answer was handled whole.
    s.workbench
        .update(&created.id, mode(PermissionMode::Plan))
        .unwrap_or_else(|e| panic!("{e}"));
    until("the session shows Plan", || {
        thread(&s.workbench, &created.id).permission_mode == PermissionMode::Plan
    })
    .await;
    assert_eq!(modes(&s), ["accept-edits", "plan"]);
    let events = allow_and_finish(&s, &created.id, &ask).await;
    assert_eq!(notices(&events), Vec::<String>::new());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A session `name` of `agent` whose first turn ended, then a flip of the Sandbox switch to `flip` (none: no flip),
/// then a second message: the session's id, once its second turn ended.
async fn second_message_after(s: &Setup, agent: SessionAgent, flip: Option<bool>) -> String {
    let created = s
        .workbench
        .create_thread(NewThread {
            agent,
            permission_mode: PermissionMode::AcceptEdits,
            orchestration: false,
            ..new_root(&s.project, "hello")
        })
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    if let Some(on) = flip {
        s.host.antigravity_sandbox.store(on, Ordering::SeqCst);
    }
    s.workbench
        .send(&created.id, "hello again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 2).await;
    created.id
}

/// The Sandbox switch turned on while an Antigravity session is open: its next message, between turns, starts agy
/// again with `--sandbox` on the same conversation, and says nothing more.
#[tokio::test]
async fn sandbox_flip_restarts_an_idle_session_at_its_next_message() {
    let s = setup("agy-flip", Default::default());
    let id = second_message_after(&s, SessionAgent::Antigravity, Some(true)).await;
    assert_eq!(
        starts(&s),
        [
            nexus_workbench::acp::args(PermissionMode::AcceptEdits, false, None),
            nexus_workbench::acp::args(PermissionMode::AcceptEdits, true, Some(true)),
        ]
    );
    assert_eq!(launches_of(&s, &id).len(), 2);
    // The second agy loads the conversation the first one opened.
    let loads = std::fs::read_to_string(s.host.memory.join("loads")).unwrap_or_default();
    assert_eq!(loads.lines().collect::<Vec<_>>(), ["fake-agy-session"]);
    assert_eq!(
        notices(&s.workbench.events(&id, 0).unwrap_or_default()),
        Vec::<String>::new()
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn session_without_a_sandbox_flip_keeps_its_agy() {
    let s = setup("agy-no-flip", Default::default());
    second_message_after(&s, SessionAgent::Antigravity, None).await;
    assert_eq!(
        starts(&s),
        [nexus_workbench::acp::args(PermissionMode::AcceptEdits, false, None)]
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The Sandbox switch is Antigravity's: a flip leaves Claude Code's and Codex's processes running.
#[tokio::test]
async fn sandbox_flip_leaves_claude_code_and_codex_running() {
    for (name, agent) in [
        ("flip-claude", SessionAgent::Claude),
        ("flip-codex", SessionAgent::Codex),
    ] {
        let s = setup(name, Default::default());
        let id = second_message_after(&s, agent, Some(true)).await;
        assert_eq!(launches_of(&s, &id).len(), 1, "{agent:?} started again");
        s.workbench.shutdown().await;
        let _ = std::fs::remove_dir_all(&s.root);
    }
}

/// The Sandbox switch on: agy starts with `--sandbox` in every mode, Plan included.
#[tokio::test]
async fn sandbox_switch_starts_agy_with_sandbox_in_every_mode() {
    let s = setup("agy-sandbox", Default::default());
    s.host.antigravity_sandbox.store(true, Ordering::SeqCst);
    let modes = [
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::Plan,
        PermissionMode::Auto,
        PermissionMode::BypassPermissions,
    ];
    for mode in modes {
        let created = s
            .workbench
            .create_thread(agy(&s, mode, "hello"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    }
    // Only agy taking `auto` says it offers it, and no session before Auto's asked for it.
    let expected: Vec<Vec<String>> = modes
        .iter()
        .map(|mode| nexus_workbench::acp::args(*mode, true, None))
        .collect();
    assert_eq!(starts(&s), expected);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// ACP's answer to an ask has no text, so the reason given with a denial goes to agy as the person's own queued message:
/// a `User` row, in line behind the message the person queued before it. Every turn ends, and the session goes idle.
#[tokio::test]
async fn a_denials_reason_goes_as_the_persons_queued_message_behind_one_queued_before_it() {
    let s = setup("agy-deny-reason", Default::default());
    let created = s
        .workbench
        .create_thread(agy(&s, PermissionMode::Default, "please run ls"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let id = &created.id;
    let ask = approval(&s.workbench, id).await;
    s.workbench
        .queue(id, "then say hi")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .answer(id, &ask, Decision::Deny, Some("list docs instead".into()), None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, id, |e| turn_ends(e).len() == 3).await;
    let said: Vec<String> = users(&events).into_iter().map(|(text, _)| text).collect();
    assert_eq!(said, ["please run ls", "then say hi", "list docs instead"]);
    let answers: Vec<&str> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        answers,
        [
            "You said: please run ls",
            "You said: then say hi",
            "You said: list docs instead"
        ]
    );
    // The denied card keeps the reason.
    assert!(events.iter().any(|e| matches!(&e.event,
        WbEvent::ApprovalResolved { decision: Decision::Deny, message: Some(m), .. } if m == "list docs instead")));
    for _ in 0..400 {
        if thread(&s.workbench, id).status == ThreadStatus::Idle {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(thread(&s.workbench, id).status, ThreadStatus::Idle);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// agy holds Nexus's notice for the end of the turn under way, which reserves the next turn for it. The person presses
/// Stop: that reserved turn is not the person's, so the halt interrupts it as it begins. Its prompt would ask to run a
/// command, and the session would wait for the person.
#[tokio::test]
async fn a_held_notice_after_a_stop_is_interrupted_as_its_turn_begins() {
    let s = setup("agy-notice-halted", Default::default());
    let created = s
        .workbench
        .create_thread(agy(&s, PermissionMode::Default, "please run ls"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let id = &created.id;
    approval(&s.workbench, id).await;
    s.workbench
        .steer_notice(id, "Your command finished, run the next one.")
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench.interrupt(id).unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted, TurnStatus::Interrupted]);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_ne!(thread(&s.workbench, id).status, ThreadStatus::Approval);
    assert!(s.workbench.halt(id).unwrap_or_else(|e| panic!("{e}")).is_some());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
