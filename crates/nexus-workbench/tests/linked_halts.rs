//! Halts, the cascades and the spend cap of linked sessions through the public
//! workbench, with the stand-in `claude`: the person's Stop and Interrupt, Delete, a turn an agent starts on its own,
//! and a tree past its spend cap. A child held "starting" waits in the project's setup script for a file `setup-go`. Each test
//! replays one scenario.

mod support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use nexus_workbench::event::{
    Sender, SessionAgent, SetupStatus, StoredEvent, TaskStatus, ThreadStatus, TurnStatus, WbEvent,
};
use nexus_workbench::{BlockReason, Config, Halt, Limits, OrchCode, StartChild, Wait, WaitMode, Workbench};
use support::*;

fn started(events: &[StoredEvent]) -> bool {
    events.iter().any(|e| matches!(e.event, WbEvent::TurnStart { .. }))
}

fn turn_starts(events: &[StoredEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e.event, WbEvent::TurnStart { .. }))
        .count()
}

fn halt(s: &Setup, id: &str) -> Option<Halt> {
    s.workbench.halt(id).unwrap_or_else(|e| panic!("{e}"))
}

/// The project's new worktrees run a setup script that waits for a file `setup-go` (30 s at most).
fn hold_setups(s: &Setup) {
    let go = s.host.memory.join("setup-go");
    let script = format!(
        "i=0; while [ ! -e '{}' ] && [ $i -lt 600 ]; do sleep 0.05; i=$((i+1)); done",
        go.display()
    );
    s.workbench
        .set_project_setup(&s.project, Some(&script))
        .unwrap_or_else(|e| panic!("{e}"));
}

fn in_setup(events: &[StoredEvent]) -> bool {
    events.iter().any(|e| matches!(e.event, WbEvent::Setup { .. }))
}

fn setup_over(events: &[StoredEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e.event, WbEvent::Setup { status, .. } if status != SetupStatus::Running))
}

fn tasks_ended(events: &[StoredEvent], status: TaskStatus) -> bool {
    events
        .iter()
        .any(|e| matches!(&e.event, WbEvent::Task { status: s, .. } if *s == status))
}

fn own_turns_wait(s: &Setup) {
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_OWN_TURN_MS".into(), "10000".into());
}

/// [`end_relay`] on another workbench over the same store (after a restart of the app).
async fn end_relay_in(workbench: &Workbench, s: &Setup, lead: &str) {
    let before = turn_ends(&workbench.events(lead, 0).unwrap_or_default()).len();
    trigger_with(s, "relay", "END");
    wait_for(workbench, lead, |e| turn_ends(e).len() > before).await;
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// The person stops a running child. Its parent can't message it again, not after
/// a restart of the app and a person message to the parent either; only a person message to the child frees it.
#[tokio::test]
async fn f3_stop_survives() {
    let s = setup("f3", bounded());
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    let c = start(&s, &lead.id, SessionAgent::Claude, "Builder", "slow Build it").await;
    wait_for(&s.workbench, &c.id, started).await;
    s.workbench.stop(&c.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &c.id).await;
    wait_for(&s.workbench, &c.id, |e| turn_ends(e) == [TurnStatus::Interrupted]).await;
    assert_eq!(halt(&s, &c.id), Some(Halt::Person));
    assert_eq!(halt(&s, &lead.id), None, "a stop halts down the tree, never up");
    let refused = s.workbench.message_child(&lead.id, &c.id, "Go on").await;
    assert_eq!(refused.err().map(|e| e.code), Some(OrchCode::Halted));
    end_relay(&s, &lead.id).await;
    s.workbench.shutdown().await;

    let workbench = reopen(&s, bounded());
    assert_eq!(
        workbench.halt(&c.id).ok().flatten(),
        Some(Halt::Person),
        "a halt is a row"
    );
    let before = turn_starts(&workbench.events(&lead.id, 0).unwrap_or_default());
    workbench
        .send(&lead.id, "relay Go on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &lead.id, |e| turn_starts(e) > before).await;
    let refused = workbench.message_child(&lead.id, &c.id, "Go on").await;
    assert_eq!(refused.err().map(|e| e.code), Some(OrchCode::Halted));
    assert_eq!(launches_of(&s, &c.id).len(), 1);
    end_relay_in(&workbench, &s, &lead.id).await;

    workbench
        .send(&c.id, "Carry on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &c.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(workbench.halt(&c.id).ok().flatten(), None);
    workbench
        .message_child(&lead.id, &c.id, "And the tests")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&workbench, &c.id, |e| turn_ends(e).len() == 3).await;
    workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Stop on a lead whose child is still being made. The child is
/// halted and ends as stopped at once, with no agent; its setup task finds it ended and sends nothing, and keeps its
/// worktree. The person's next message to the lead carries its stopped result and does not start it.
#[tokio::test]
async fn f4_stop_reaches_a_starting_child() {
    let s = setup("f4-starting", bounded());
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    hold_setups(&s);
    let c = start(&s, &lead.id, SessionAgent::Claude, "Builder", "Build it").await;
    wait_for(&s.workbench, &c.id, in_setup).await;
    s.workbench.stop(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    // At once: the stop does not wait for the worktree or its script.
    assert_eq!(
        turn_ends(&s.workbench.events(&c.id, 0).unwrap_or_default()),
        [TurnStatus::Interrupted]
    );
    assert_eq!(thread(&s.workbench, &c.id).status, ThreadStatus::Interrupted);
    assert_eq!(
        (halt(&s, &lead.id), halt(&s, &c.id)),
        (Some(Halt::Person), Some(Halt::Parent))
    );
    // Its setup script was stopped too; its task keeps the worktree and sends nothing.
    wait_for(&s.workbench, &c.id, setup_over).await;
    until_stopped(&s.workbench, &lead.id).await;
    let worktree = thread(&s.workbench, &c.id)
        .worktree
        .unwrap_or_else(|| panic!("the worktree is kept"));
    assert!(std::path::Path::new(&worktree.path).exists());

    s.workbench
        .send(&lead.id, "Go on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &lead.id, |e| turn_ends(e).len() == 2).await;
    let rows = users(&events);
    assert!(rows[rows.len() - 2].0.contains("status=\"stopped\""), "{rows:?}");
    assert!(launches_of(&s, &c.id).is_empty(), "the child never ran");
    assert!(!s.workbench.is_live(&c.id));
    assert_eq!(halt(&s, &c.id), None, "the person's message to its lead frees it");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The reaper stops an idle lead and leaves its running child as it
/// is (Idle does not cascade); the person's Stop on that lead, which runs no agent, still stops its child.
#[tokio::test]
async fn f4_stop_on_a_lead_that_is_not_running() {
    let s = setup_with("f4-idle", bounded(), true, |config| Config {
        idle_after: Duration::from_millis(1),
        ..config
    });
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let c = start(&s, &lead.id, SessionAgent::Claude, "Builder", "slow Build it").await;
    wait_for(&s.workbench, &c.id, started).await;
    until("the reaper stops the lead", || {
        s.workbench.reap_idle();
        !s.workbench.is_live(&lead.id)
    })
    .await;
    assert!(s.workbench.is_live(&c.id), "Idle does not cascade");
    assert_eq!((halt(&s, &lead.id), halt(&s, &c.id)), (None, None));

    s.workbench.stop(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &c.id).await;
    wait_for(&s.workbench, &c.id, |e| turn_ends(e) == [TurnStatus::Interrupted]).await;
    assert_eq!(
        (halt(&s, &lead.id), halt(&s, &c.id)),
        (Some(Halt::Person), Some(Halt::Parent))
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Deleting a lead whose child is still being made ends the child at
/// once. It becomes a session of the person's own that never ran, with no halt and its worktree, which runs on the
/// person's message.
#[tokio::test]
async fn u14_delete_ends_a_starting_child() {
    let s = setup("u14", bounded());
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    hold_setups(&s);
    let c = start(&s, &lead.id, SessionAgent::Claude, "Builder", "Build it").await;
    wait_for(&s.workbench, &c.id, in_setup).await;
    s.workbench
        .delete_thread(&lead.id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!s.workbench.is_live(&lead.id));
    let freed = thread(&s.workbench, &c.id);
    assert_eq!(freed.parent_thread_id, None);
    assert_eq!(freed.status, ThreadStatus::Interrupted);
    assert_eq!(halt(&s, &c.id), None);
    assert_eq!(
        turn_ends(&s.workbench.events(&c.id, 0).unwrap_or_default()),
        [TurnStatus::Interrupted]
    );
    wait_for(&s.workbench, &c.id, setup_over).await;
    until("its setup task keeps the worktree", || {
        thread(&s.workbench, &c.id)
            .worktree
            .is_some_and(|worktree| std::path::Path::new(&worktree.path).exists())
    })
    .await;
    assert!(launches_of(&s, &c.id).is_empty(), "the child never ran");

    s.workbench
        .send(&c.id, "Build it after all")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &c.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(launches_of(&s, &c.id).len(), 1);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Deleting a lead stops its running child and grandchild and waits for
/// them. Every descendant becomes a root at depth 0, with its halt cleared and its worktree kept.
#[tokio::test]
async fn delete_frees_the_children() {
    let limits = Limits {
        max_depth: Some(2),
        ..bounded()
    };
    let s = setup("delete-frees", limits);
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let middle = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Lead the builders")).await;
    let leaf = start(&s, &middle.id, SessionAgent::Claude, "Builder", "slow Build it").await;
    wait_for(&s.workbench, &leaf.id, started).await;
    s.workbench
        .delete_thread(&lead.id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    for id in [&lead.id, &middle.id, &leaf.id] {
        assert!(!s.workbench.is_live(id), "{id} still runs");
    }
    let freed = thread(&s.workbench, &middle.id);
    assert_eq!(freed.parent_thread_id, None);
    assert_eq!(halt(&s, &middle.id), None);
    assert_eq!((freed.depth, thread(&s.workbench, &leaf.id).depth), (0, 0));
    assert!(freed.orchestration, "its orchestration stays as it was");
    let worktree = freed.worktree.unwrap_or_else(|| panic!("a worktree"));
    assert!(std::path::Path::new(&worktree.path).exists(), "its worktree is kept");
    assert_eq!(thread(&s.workbench, &leaf.id).parent_thread_id.as_deref(), None);
    assert_eq!(halt(&s, &leaf.id), None);
    assert_eq!(
        turn_ends(&s.workbench.events(&leaf.id, 0).unwrap_or_default()),
        [TurnStatus::Interrupted]
    );

    s.workbench
        .send(&middle.id, "Go on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(halt(&s, &leaf.id), None);
    wait_for(&s.workbench, &middle.id, |e| turn_ends(e).len() == 2).await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The person interrupts a child whose background sub-agent
/// runs on. When that ends, the turn Claude Code starts on its own to answer it is interrupted at once; the child
/// stays halted, and its result waits for its lead.
#[tokio::test]
async fn u2_interrupted_child_runs_no_turn_of_its_own() {
    let s = setup("u2", bounded());
    own_turns_wait(&s);
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    // A lead that is not running (and not halted) keeps its results pending.
    s.workbench.restart(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &lead.id).await;
    let c = start(&s, &lead.id, SessionAgent::Claude, "Surveyor", "slow bgagent Survey it").await;
    wait_for(&s.workbench, &c.id, started).await;
    s.workbench.interrupt(&c.id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &c.id, |e| turn_ends(e) == [TurnStatus::Interrupted]).await;
    assert_eq!(halt(&s, &c.id), Some(Halt::Person));
    assert!(s.workbench.is_live(&c.id), "an interrupt keeps the process");
    assert!(!has_result(&s.workbench, &lead.id, &c.id), "its sub-agent still works");

    trigger(&s, "bgagent-end");
    wait_for(&s.workbench, &c.id, |e| tasks_ended(e, TaskStatus::Completed)).await;
    trigger(&s, "bgagent-turn");
    wait_for(&s.workbench, &c.id, |e| {
        turn_ends(e) == [TurnStatus::Interrupted, TurnStatus::Interrupted]
    })
    .await;
    assert_eq!(halt(&s, &c.id), Some(Halt::Person));
    until("the result is pending", || has_result(&s.workbench, &lead.id, &c.id)).await;
    assert!(!s.workbench.is_live(&lead.id), "a result never starts its lead");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Once the person interrupted a lead, its background sub-agent
/// still runs, but the lead's tools refuse it: no start, no message, no wait.
#[tokio::test]
async fn u7_background_calls_follow_the_halt() {
    let s = setup("u7", bounded());
    let lead = root_at_work(&s, new_root(&s.project, "relay bgagent Plan the release")).await;
    let c = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    s.workbench.interrupt(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &lead.id, |e| turn_ends(e) == [TurnStatus::Interrupted]).await;
    wait_for(&s.workbench, &lead.id, |e| {
        e.iter()
            .any(|e| matches!(&e.event, WbEvent::Task { status, .. } if *status == TaskStatus::Running))
    })
    .await;
    assert_eq!(halt(&s, &lead.id), Some(Halt::Person));
    let started = s
        .workbench
        .start_child(&lead.id, child(SessionAgent::Claude, "Build more"))
        .await;
    assert_eq!(code(started), OrchCode::Halted);
    let messaged = s.workbench.message_child(&lead.id, &c.id, "And the tests").await;
    assert_eq!(messaged.err().map(|e| e.code), Some(OrchCode::Halted));
    let waited = s.workbench.wait_children(&lead.id, Wait::default()).await;
    assert_eq!(waited.err().map(|e| e.code), Some(OrchCode::Halted));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The tree's spend reaches its cap while a child works and another's
/// background sub-agent runs. The child's turn is interrupted and the sub-agent stopped, with no halt; the tree is
/// paused, and the turn the other child's agent then starts on its own is interrupted at once. The lead's own turn,
/// which the person started, runs on, and only the person's next message to it resets the spend.
#[tokio::test]
async fn limits_spend_interrupts() {
    // A child's turn costs 0.005 at list price: the second crosses the cap.
    let limits = Limits {
        crew_spend_usd: Some(0.013),
        ..bounded()
    };
    let s = setup("spend", limits);
    own_turns_wait(&s);
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    let a = start(&s, &lead.id, SessionAgent::Claude, "Surveyor", "bgagent Survey it").await;
    wait_for(&s.workbench, &a.id, |e| turn_ends(e).len() == 1).await;
    let left = s
        .workbench
        .list_children(&lead.id)
        .unwrap_or_else(|e| panic!("{e}"))
        .limits;
    assert!(close(left.spend_left_usd.expect("cap enabled"), 0.003), "{left:?}");

    let b = start(&s, &lead.id, SessionAgent::Claude, "Builder", "slow Build it").await;
    wait_for(&s.workbench, &b.id, |e| turn_ends(e) == [TurnStatus::Interrupted]).await;
    wait_for(&s.workbench, &a.id, |e| tasks_ended(e, TaskStatus::Stopped)).await;
    for id in [&lead.id, &a.id, &b.id] {
        assert_eq!(halt(&s, id), None, "a trip halts nothing");
    }
    assert!(turn_ends(&s.workbench.events(&lead.id, 0).unwrap_or_default()).is_empty());
    let left = s
        .workbench
        .list_children(&lead.id)
        .unwrap_or_else(|e| panic!("{e}"))
        .limits;
    assert!(close(left.spend_left_usd.expect("cap enabled"), 0.0), "{left:?}");
    let refused = s
        .workbench
        .start_child(&lead.id, child(SessionAgent::Claude, "Build more"))
        .await;
    assert_eq!(code(refused), OrchCode::Paused);
    let refused = s.workbench.message_child(&lead.id, &b.id, "Go on").await;
    assert_eq!(refused.err().map(|e| e.code), Some(OrchCode::Paused));
    let got = s
        .workbench
        .wait_children(
            &lead.id,
            Wait {
                session_ids: Some(vec![b.id.clone()]),
                timeout: Duration::from_secs(20),
                mode: WaitMode::All,
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(got.results.is_empty(), "results wait: {got:?}");
    assert_eq!(got.blocked.len(), 1);
    assert_eq!(got.blocked[0].reason, BlockReason::Paused);

    // The sub-agent's end: the turn Claude Code starts to answer it is interrupted at once in the tripped tree.
    trigger(&s, "bgagent-turn");
    wait_for(&s.workbench, &a.id, |e| {
        turn_ends(e) == [TurnStatus::Completed, TurnStatus::Interrupted]
    })
    .await;
    assert!(turn_ends(&s.workbench.events(&lead.id, 0).unwrap_or_default()).is_empty());
    end_relay(&s, &lead.id).await;

    s.workbench
        .send(&lead.id, "Go on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &lead.id, |e| turn_ends(e).len() == 2).await;
    let left = s
        .workbench
        .list_children(&lead.id)
        .unwrap_or_else(|e| panic!("{e}"))
        .limits;
    assert!(close(left.spend_left_usd.expect("cap enabled"), 0.008), "{left:?}");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A child whose worktree is ready after its tree tripped ends as stopped,
/// with no agent, instead of starting in the paused tree.
#[tokio::test]
async fn u5_paused_tree_starts_no_child() {
    let limits = Limits {
        crew_spend_usd: Some(0.009),
        ..bounded()
    };
    let s = setup("u5", limits);
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    hold_setups(&s);
    let waiting = start(&s, &lead.id, SessionAgent::Claude, "Waiter", "Build it").await;
    wait_for(&s.workbench, &waiting.id, in_setup).await;
    s.workbench
        .set_project_setup(&s.project, None)
        .unwrap_or_else(|e| panic!("{e}"));
    let spender = start(&s, &lead.id, SessionAgent::Claude, "Builder", "Build it").await;
    wait_for(&s.workbench, &spender.id, |e| turn_ends(e).len() == 1).await;
    until("the tree trips", || {
        s.workbench
            .list_children(&lead.id)
            .is_ok_and(|children| close(children.limits.spend_left_usd.expect("cap enabled"), 0.0))
    })
    .await;
    trigger(&s, "setup-go");
    wait_for(&s.workbench, &waiting.id, |e| turn_ends(e) == [TurnStatus::Interrupted]).await;
    assert_eq!(thread(&s.workbench, &waiting.id).status, ThreadStatus::Interrupted);
    assert!(launches_of(&s, &waiting.id).is_empty(), "it never ran");
    assert_eq!(halt(&s, &waiting.id), None, "a pause halts nothing");
    end_relay(&s, &lead.id).await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A call the host has no list price for counts at the catalog's highest toward the cap, so one such call trips a tree
/// that priced calls leave room in. The session's own cost shows it unpriced.
#[tokio::test]
async fn an_unpriced_call_counts_at_the_top_price() {
    let limits = Limits {
        crew_spend_usd: Some(0.02),
        ..bounded()
    };
    let s = setup("top-price", limits);
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    let priced = start(&s, &lead.id, SessionAgent::Claude, "Builder", "Build it").await;
    wait_for(&s.workbench, &priced.id, |e| turn_ends(e).len() == 1).await;
    let left = s
        .workbench
        .list_children(&lead.id)
        .unwrap_or_else(|e| panic!("{e}"))
        .limits;
    assert!(close(left.spend_left_usd.expect("cap enabled"), 0.01), "{left:?}");

    let unpriced = s
        .workbench
        .start_child(
            &lead.id,
            StartChild {
                model: Some(UNPRICED.into()),
                ..child(SessionAgent::Claude, "Build more")
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &unpriced.id, |e| turn_ends(e).len() == 1).await;
    let costs: Vec<f64> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Usage { cost_usd, .. } => Some(*cost_usd),
            _ => None,
        })
        .collect();
    assert_eq!(costs, [0.0], "its own cost has no list price");
    until("the tree trips", || {
        s.workbench
            .list_children(&lead.id)
            .is_ok_and(|children| close(children.limits.spend_left_usd.expect("cap enabled"), 0.0))
    })
    .await;
    let refused = s
        .workbench
        .start_child(&lead.id, child(SessionAgent::Claude, "Build again"))
        .await;
    assert_eq!(code(refused), OrchCode::Paused);
    end_relay(&s, &lead.id).await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// With no catalog at all, the spend cap could count nothing: `start_session` answers `unpriced_model`.
#[tokio::test]
async fn start_child_refuses_unpriced_model_when_the_host_has_no_prices() {
    let s = setup(
        "no-prices",
        Limits {
            crew_spend_usd: Some(20.0),
            ..bounded()
        },
    );
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    s.host.no_prices.store(true, Ordering::SeqCst);
    let refused = s
        .workbench
        .start_child(&lead.id, child(SessionAgent::Claude, "Build it"))
        .await;
    assert_eq!(code(refused), OrchCode::UnpricedModel);
    s.workbench.set_orchestration_limits(bounded());
    finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The person interrupts a lead whose child then finishes: the lead runs on, idle, but takes no result message; the
/// result goes in front of the person's next message, which frees the lead.
#[tokio::test]
async fn an_interrupted_lead_takes_its_results_with_the_persons_next_message() {
    let s = setup("interrupt-lead", bounded());
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    let c = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    until("the lead counts the result", || {
        thread(&s.workbench, &lead.id).pending_results == 1
    })
    .await;
    s.workbench.interrupt(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &lead.id, |e| turn_ends(e) == [TurnStatus::Interrupted]).await;
    assert!(s.workbench.is_live(&lead.id));
    // The reaper offers every running session its results again: not into a halted one.
    s.workbench.reap_idle();
    let events = s.workbench.events(&lead.id, 0).unwrap_or_default();
    assert_eq!(users(&events).len(), 1, "no result message: {:?}", users(&events));
    assert_eq!(turn_ends(&events).len(), 1);
    assert!(has_result(&s.workbench, &lead.id, &c.id));

    s.workbench
        .send(&lead.id, "Go on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &lead.id, |e| turn_ends(e).len() == 2).await;
    let rows = users(&events);
    assert!(rows[rows.len() - 2].1.is_some(), "the result rides in front: {rows:?}");
    assert_eq!(halt(&s, &lead.id), None);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The parent's agent stops its child (`stop_session`): the child and its running child stop, with no halt on either,
/// so the parent may message the child again at once.
#[tokio::test]
async fn stop_child_stops_the_subtree_without_a_halt() {
    let limits = Limits {
        max_depth: Some(2),
        ..bounded()
    };
    let s = setup("stop-child", limits);
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    let c = start(&s, &lead.id, SessionAgent::Claude, "Builder", "slow Build it").await;
    wait_for(&s.workbench, &c.id, started).await;
    let leaf = start(&s, &c.id, SessionAgent::Claude, "Tester", "slow Test it").await;
    wait_for(&s.workbench, &leaf.id, started).await;
    assert_eq!(s.workbench.stop_child(&lead.id, &c.id).ok(), Some(true));
    until_stopped(&s.workbench, &c.id).await;
    until_stopped(&s.workbench, &leaf.id).await;
    for id in [&lead.id, &c.id, &leaf.id] {
        assert_eq!(halt(&s, id), None, "{id} is halted");
    }
    s.workbench
        .message_child(&lead.id, &c.id, "Build it again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &c.id, |e| turn_ends(e).len() >= 2).await;
    end_relay(&s, &lead.id).await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Restart and the interrupt's escalation stop a lead's process only: its running child, and every halt below it,
/// stay as they were.
#[tokio::test]
async fn restart_and_the_interrupt_escalation_do_not_cascade() {
    let s = setup_with("no-cascade", bounded(), true, |config| Config {
        interrupt_grace: Duration::from_millis(200),
        ..config
    });
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    let c = start(&s, &lead.id, SessionAgent::Claude, "Builder", "slow Build it").await;
    wait_for(&s.workbench, &c.id, started).await;
    s.workbench.restart(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &lead.id).await;
    assert!(s.workbench.is_live(&c.id), "Restart does not cascade");
    assert_eq!((halt(&s, &lead.id), halt(&s, &c.id)), (None, None));

    // A turn that ignores the interrupt: 200 ms on, its process is stopped.
    s.workbench
        .send(&lead.id, "slow deaf Plan again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &lead.id, |e| turn_starts(e) == 2).await;
    s.workbench.interrupt(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &lead.id).await;
    assert!(s.workbench.is_live(&c.id), "the escalation does not cascade");
    assert_eq!((halt(&s, &lead.id), halt(&s, &c.id)), (Some(Halt::Person), None));
    s.workbench.stop(&c.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &c.id).await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Writes the session's `created_at` straight into the store: text where a number belongs makes its row unreadable.
fn set_created_at(s: &Setup, id: &str, value: rusqlite::types::Value) {
    let db = rusqlite::Connection::open(s.root.join("workbench.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let _ = db.busy_timeout(Duration::from_secs(5));
    db.execute(
        "update threads set created_at = ?2 where id = ?1",
        rusqlite::params![id, value],
    )
    .unwrap_or_else(|e| panic!("{e}"));
}

/// The person's Stop and Interrupt reach the process when a row can't be read: Stop on a lead whose child's row is
/// unreadable stops the lead, and Stop on that child stops it; Interrupt on a session whose own row is unreadable
/// interrupts its turn. Each says the error after.
#[tokio::test]
async fn stop_and_interrupt_reach_the_process_when_a_row_cant_be_read() {
    let s = setup("unreadable-stop", bounded());
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    let c = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    assert!(s.workbench.is_live(&c.id));
    set_created_at(&s, &c.id, "x".to_string().into());
    assert!(s.workbench.stop(&lead.id).is_err(), "the walk below it fails");
    until_stopped(&s.workbench, &lead.id).await;
    assert_eq!(halt(&s, &lead.id), Some(Halt::Person));
    assert!(s.workbench.stop(&c.id).is_err(), "its own row fails");
    until_stopped(&s.workbench, &c.id).await;

    let prompt = "slow Plan another";
    let other = s
        .workbench
        .create_thread(new_root(&s.project, prompt))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    // TurnStart can precede the agent reading the prompt.
    wait_for(&s.workbench, &other.id, |events| {
        let prompt_seq = events.iter().find_map(|event| match &event.event {
            WbEvent::User { text, .. } if text == prompt => Some(event.seq),
            _ => None,
        });
        prompt_seq.is_some_and(|prompt_seq| {
            events
                .iter()
                .any(|event| matches!(event.event, WbEvent::Read { seq } if seq == prompt_seq))
        })
    })
    .await;
    set_created_at(&s, &other.id, "x".to_string().into());
    assert!(s.workbench.interrupt(&other.id).is_err(), "its halt can't be written");
    set_created_at(&s, &other.id, other.created_at.into());
    let events = wait_for(&s.workbench, &other.id, |events| {
        events
            .iter()
            .any(|event| matches!(event.event, WbEvent::TurnEnd { .. }))
    })
    .await;
    assert_eq!(turn_ends(&events).first(), Some(&TurnStatus::Interrupted));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The person sends a message while the lead's turn runs, then interrupts that turn: the queued message's turn frees
/// the lead when it begins, so the lead starts sessions and takes their results again.
#[tokio::test]
async fn a_queued_persons_message_frees_the_session_when_its_turn_begins() {
    let s = setup("interrupt-queued", bounded());
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    s.workbench
        .queue(&lead.id, "Go on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench.interrupt(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    // Logged while the interrupted turn ran: its own turn comes after.
    let events = wait_for(&s.workbench, &lead.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted, TurnStatus::Completed]);
    assert_eq!(halt(&s, &lead.id), None);

    let c = start(&s, &lead.id, SessionAgent::Claude, "Builder", "Build it").await;
    let events = wait_for(&s.workbench, &lead.id, |e| turn_ends(e).len() == 3).await;
    let rows = users(&events);
    assert!(
        matches!(&rows[rows.len() - 1].1, Some(Sender::Session { thread_id, .. }) if *thread_id == c.id),
        "the result comes in on its own: {rows:?}"
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The person sends a message while the lead's turn runs, then presses Stop: the stopping run's last turn end does
/// not let that message's turn clear the halt, since the message never runs. The Stop holds.
#[tokio::test]
async fn a_stop_holds_over_the_persons_message_queued_behind_it() {
    let s = setup("stop-queued", bounded());
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    s.workbench
        .queue(&lead.id, "Go on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench.stop(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &lead.id).await;
    assert_eq!(halt(&s, &lead.id), Some(Halt::Person));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A linked session's message to a running session the person interrupted is refused when its turn would begin, and
/// its row is taken back (`QueueCancel`), so it reads as not sent.
#[tokio::test]
async fn a_message_a_halt_refuses_reads_as_not_sent() {
    let s = setup("refused-row", bounded());
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let c = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    assert!(s.workbench.is_live(&c.id));
    s.workbench.interrupt(&c.id).unwrap_or_else(|e| panic!("{e}"));
    let from = Sender::Session {
        thread_id: lead.id.clone(),
        title: lead.title.clone(),
    };
    let refused = s.workbench.send_from(&c.id, "Go on", vec![], Some(from)).await;
    assert_eq!(
        refused.err().as_deref(),
        Some("The person stopped this session. Only they can start it again.")
    );
    let events = s.workbench.events(&c.id, 0).unwrap_or_default();
    let row = events
        .iter()
        .find(|e| matches!(&e.event, WbEvent::User { text, .. } if text == "Go on"))
        .map(|e| e.seq)
        .unwrap_or_else(|| panic!("no row: {events:#?}"));
    assert!(
        events
            .iter()
            .any(|e| matches!(e.event, WbEvent::QueueCancel { seq } if seq == row)),
        "{events:#?}"
    );
    assert_eq!(turn_ends(&events).len(), 1, "no turn took it");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A halt on a session above the parent refuses `start_session`, even once the person freed the parent itself: only a
/// person message to the halted session above frees the tree.
#[tokio::test]
async fn a_halt_above_the_parent_refuses_a_start() {
    let limits = Limits {
        max_depth: Some(2),
        ..bounded()
    };
    let s = setup("halt-above", limits);
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let middle = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Lead the builders")).await;
    s.workbench.stop(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &middle.id).await;
    assert_eq!(
        (halt(&s, &lead.id), halt(&s, &middle.id)),
        (Some(Halt::Person), Some(Halt::Parent))
    );
    s.workbench
        .send(&middle.id, "Go on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    answered(&s.workbench, &middle.id, "Go on").await;
    assert_eq!(halt(&s, &middle.id), None);
    let refused = s
        .workbench
        .start_child(&middle.id, child(SessionAgent::Claude, "Build it"))
        .await;
    assert_eq!(code(refused), OrchCode::Halted);

    s.workbench
        .send(&lead.id, "Go on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(halt(&s, &lead.id), None);
    finished_child(&s, &middle.id, child(SessionAgent::Claude, "Build it")).await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A child whose parent the person interrupted while its worktree was made ends as stopped, with no agent, when its
/// setup ends: a halt above it holds its first prompt.
#[tokio::test]
async fn a_child_made_while_its_parent_was_interrupted_does_not_run() {
    let s = setup("interrupt-starting", bounded());
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    hold_setups(&s);
    let c = start(&s, &lead.id, SessionAgent::Claude, "Builder", "Build it").await;
    wait_for(&s.workbench, &c.id, in_setup).await;
    s.workbench.interrupt(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &lead.id, |e| turn_ends(e) == [TurnStatus::Interrupted]).await;
    assert_eq!(halt(&s, &c.id), None, "an interrupt halts nothing below");
    trigger(&s, "setup-go");
    wait_for(&s.workbench, &c.id, |e| turn_ends(e) == [TurnStatus::Interrupted]).await;
    assert!(launches_of(&s, &c.id).is_empty(), "it never ran");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

async fn wait_for_prompt_read(s: &Setup, id: &str, prompt: &str) {
    wait_for(&s.workbench, id, |events| {
        let prompt_seq = events
            .iter()
            .rfind(|event| matches!(&event.event, WbEvent::User { text, .. } if text == prompt))
            .map(|event| event.seq);
        prompt_seq.is_some_and(|prompt_seq| {
            events
                .iter()
                .any(|event| matches!(event.event, WbEvent::Read { seq } if seq == prompt_seq))
        })
    })
    .await;
}

async fn check_stopped_child(
    prompt: &str,
    label: &str,
    expected_cost: Option<f64>,
    stop: impl FnOnce(&Workbench, &str, &str),
) {
    let grace = Duration::from_millis(300);
    let s = setup_with(label, Limits::default(), true, |config| Config {
        interrupt_grace: grace,
        ..config
    });
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    // The first turn is automatic. Send the next prompt as the person so the automatic-turn halt does not label it.
    let child = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    s.workbench.send(&child.id, prompt).await.expect("send prompt");
    wait_for_prompt_read(&s, &child.id, prompt).await;
    stop(&s.workbench, &lead.id, &child.id);
    until_stopped(&s.workbench, &child.id).await;

    let stored = s.workbench.events(&child.id, 0).expect("child events");
    assert_eq!(turn_ends(&stored), [TurnStatus::Completed, TurnStatus::Interrupted]);
    let end = stored
        .iter()
        .rev()
        .find(|row| matches!(row.event, WbEvent::TurnEnd { .. }))
        .expect("turn end");
    assert!(matches!(end.event, WbEvent::TurnEnd { cost_usd, .. } if cost_usd == expected_cost));
    assert!(s.host.turn_end_events.lock().expect("turn ends").contains(end));
    let answer = s
        .workbench
        .wait_children(
            &lead.id,
            Wait {
                session_ids: Some(vec![child.id.clone()]),
                mode: WaitMode::All,
                ..Wait::default()
            },
        )
        .await
        .expect("parent wait");
    assert_eq!(answer.results.len(), 1);
    assert_eq!(answer.results[0].seq, end.seq);
    assert_eq!(answer.results[0].status.as_str(), "stopped");
    assert!(answer.text().contains("status=\"stopped\""));

    until("child process end", || {
        !s.host.process_ends.lock().expect("process ends").is_empty()
    })
    .await;
    let process = s.host.process_ends.lock().expect("process ends")[0].clone();
    if expected_cost.is_some() {
        assert_eq!((process.exit_code, process.signal), (0, None));
    } else {
        assert_eq!(process.signal.as_deref(), Some("SIGTERM"));
    }
    end_relay(&s, &lead.id).await;
    s.workbench.shutdown().await;
    if let Err(error) = std::fs::remove_dir_all(&s.root) {
        eprintln!("could not remove {}: {error}", s.root.display());
    }
}

#[tokio::test]
async fn interrupt_escalation_labels_success_on_stdin_close_as_interrupted() {
    check_stopped_child("slow deaf", "escalation-eof", Some(0.01), |wb, _, id| {
        wb.interrupt(id).expect("interrupt child");
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn interrupt_escalation_labels_process_termination_as_interrupted() {
    check_stopped_child("slow deaf hold-after-eof", "escalation-term", None, |wb, _, id| {
        wb.interrupt(id).expect("interrupt child");
    })
    .await;
}

#[tokio::test]
async fn stop_on_a_deaf_agent_labels_success_on_stdin_close_as_interrupted() {
    check_stopped_child("slow deaf", "stop-eof", Some(0.01), |wb, _, id| {
        wb.stop(id).expect("stop child");
    })
    .await;
}

#[tokio::test]
async fn stop_on_a_deaf_agent_during_interrupt_grace_labels_success_as_interrupted() {
    check_stopped_child("slow deaf", "stop-during-grace", Some(0.01), |wb, _, id| {
        wb.interrupt(id).expect("interrupt child");
        wb.stop(id).expect("stop child");
    })
    .await;
}

#[tokio::test]
async fn stop_on_a_deaf_agent_by_its_parent_returns_stopped() {
    check_stopped_child("slow deaf", "parent-stop", Some(0.01), |wb, parent, id| {
        assert!(wb.stop_child(parent, id).expect("parent stops child"));
    })
    .await;
}

#[tokio::test]
async fn completion_before_interrupt_escalation_spares_the_next_turn() {
    let grace = Duration::from_secs(60);
    let s = setup_with("completion-before-escalation", Limits::default(), true, |config| {
        Config {
            interrupt_grace: grace,
            ..config
        }
    });
    s.host
        .extra
        .lock()
        .expect("fake switches")
        .insert("FAKE_CLAUDE_SLOW_MS".into(), "1000".into());
    let prompt = "slow deaf Finish the first task";
    let lead = root_at_work(&s, new_root(&s.project, prompt)).await;
    wait_for_prompt_read(&s, &lead.id, prompt).await;
    s.workbench.interrupt(&lead.id).expect("interrupt first turn");
    // Queue a distinct turn, and leave it running across the first turn's deadline.
    let next = "relay Work on the next task";
    s.workbench.queue(&lead.id, next).await.expect("queue next turn");
    wait_for_prompt_read(&s, &lead.id, next).await;
    // Advance the timer with the second turn active, then ask the stand-in to finish that turn.
    tokio::time::pause();
    tokio::time::advance(grace).await;
    tokio::task::yield_now().await;
    tokio::time::resume();
    end_relay(&s, &lead.id).await;

    let stored = s.workbench.events(&lead.id, 0).expect("turns");
    assert_eq!(turn_ends(&stored), [TurnStatus::Completed, TurnStatus::Completed]);
    assert!(s.workbench.is_live(&lead.id));
    let emitted = s.host.turn_end_events.lock().expect("turn ends").clone();
    assert_eq!(turn_ends(&emitted), [TurnStatus::Completed, TurnStatus::Completed]);
    s.workbench.shutdown().await;
    if let Err(error) = std::fs::remove_dir_all(&s.root) {
        eprintln!("could not remove {}: {error}", s.root.display());
    }
}
