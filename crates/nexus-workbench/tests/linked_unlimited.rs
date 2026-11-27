//! Subsessions with no limit set, as Nexus runs them by default, and a running tree that picks up a change to the
//! limits, through the public workbench with the stand-in `claude`.

mod support;

use nexus_workbench::Limits;
use nexus_workbench::event::{SessionAgent, StoredEvent, WbEvent};
use support::*;

fn started(events: &[StoredEvent]) -> bool {
    events.iter().any(|e| matches!(e.event, WbEvent::TurnStart { .. }))
}

/// With no limit set, a tree goes past every number the limits used to hold: more than one level deep, more than
/// four children at work, more than eight starts, and automatic turns past twelve. Nothing refuses and nothing pauses.
#[tokio::test]
async fn with_no_limit_set_nothing_refuses_or_pauses() {
    let s = setup("unlimited", Limits::default());
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    // Five children that hold their turn: all at work at once.
    let mut holding = Vec::new();
    for n in 0..5 {
        let title = format!("Holder {n}");
        holding.push(start(&s, &lead.id, SessionAgent::Claude, &title, "slow Hold on").await);
    }
    for child in &holding {
        wait_for(&s.workbench, &child.id, started).await;
    }
    // Four more: nine starts in the tree.
    let mut quick = Vec::new();
    for n in 0..4 {
        let title = format!("Builder {n}");
        quick.push(start(&s, &lead.id, SessionAgent::Claude, &title, "Build it").await);
    }
    let first = thread(&s.workbench, &holding[0].id);
    assert_eq!(first.depth, 1);
    assert!(
        first.orchestration && !first.at_depth_limit,
        "a subsession may start its own"
    );
    let grandchild = start(&s, &first.id, SessionAgent::Claude, "Helper", "Help").await;
    assert_eq!(thread(&s.workbench, &grandchild.id).depth, 2);
    let listed = s.workbench.list_children(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    assert!(listed.limits.live >= 5, "{:?}", listed.limits);
    assert_eq!(
        (
            listed.limits.max_live,
            listed.limits.starts_left,
            listed.limits.turns_left
        ),
        (None, None, None),
        "no limit is left to count down"
    );
    // Thirteen automatic turns: one past the twelve the default used to allow.
    let c = &quick[0];
    wait_for(&s.workbench, &c.id, |e| turn_ends(e).len() == 1).await;
    for round in 0..13 {
        s.workbench
            .message_child(&lead.id, &c.id, "Again")
            .await
            .unwrap_or_else(|e| panic!("turn {round}: {e}"));
        wait_for(&s.workbench, &c.id, |e| turn_ends(e).len() == round + 2).await;
    }
    for child in &holding {
        s.workbench
            .stop_child(&lead.id, &child.id)
            .unwrap_or_else(|e| panic!("{e}"));
    }
    assert!(!thread(&s.workbench, &lead.id).crew_paused);
    end_relay(&s, &lead.id).await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A tree paused by a limit goes on as soon as the person turns limits off, with no message from them, and a limit
/// turned on again applies to the same tree at once: its counters kept counting.
#[tokio::test]
async fn a_running_tree_picks_up_a_change_to_the_limits() {
    let one_turn = Limits {
        max_auto_turns: Some(1),
        ..Limits::default()
    };
    let s = setup("limits-change", one_turn);
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    let c = start(&s, &lead.id, SessionAgent::Claude, "Builder", "Build it").await;
    wait_for(&s.workbench, &c.id, |e| turn_ends(e).len() == 1).await;
    s.workbench
        .message_child(&lead.id, &c.id, "Again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &c.id, |e| turn_ends(e).len() == 2).await;
    until("the tree pauses", || shown(&s, &lead.id).is_some_and(|t| t.crew_paused)).await;

    s.workbench.set_orchestration_limits(Limits::default());
    for id in [&lead.id, &c.id] {
        assert!(
            shown(&s, id).is_some_and(|t| !t.crew_paused),
            "{id}: the pause ends on the change"
        );
    }
    s.workbench
        .message_child(&lead.id, &c.id, "Once more")
        .await
        .unwrap_or_else(|e| panic!("limits off: {e}"));
    wait_for(&s.workbench, &c.id, |e| turn_ends(e).len() == 3).await;

    s.workbench.set_orchestration_limits(one_turn);
    assert!(
        shown(&s, &c.id).is_some_and(|t| t.crew_paused),
        "the counters kept counting, so the limit applies at once"
    );
    end_relay(&s, &lead.id).await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
