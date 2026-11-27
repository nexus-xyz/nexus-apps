//! What the app is told about a crew: the summaries the workbench
//! sends (`wb:thread`, recorded by the test host) carry each halt, and are sent again when one changes with no status
//! change to carry it.

mod support;

use nexus_workbench::Limits;
use nexus_workbench::event::{SessionAgent, ThreadStatus, WbEvent};
use support::*;

fn halt_shown(s: &Setup, id: &str) -> Option<String> {
    shown(s, id).and_then(|summary| summary.halt)
}

/// Stop on a lead that runs nothing shows its halt and the cascade's on a child that was not running either.
#[tokio::test]
async fn stop_shows_every_halt_in_the_crew() {
    let s = setup("shown-stop", bounded());
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let c = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    assert_eq!((halt_shown(&s, &lead.id), halt_shown(&s, &c.id)), (None, None));
    s.workbench.stop(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        (halt_shown(&s, &lead.id), halt_shown(&s, &c.id)),
        (Some("person".to_string()), Some("parent".to_string()))
    );
    // The list reads the same halts from the rows.
    assert_eq!(thread(&s.workbench, &c.id).halt.as_deref(), Some("parent"));

    // The person's message to the lead clears the cascade's halt below it, and the child shows that at once.
    s.workbench
        .send(&lead.id, "Carry on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    until("the child shows its halt cleared", || {
        shown(&s, &c.id).is_some_and(|t| t.halt.is_none())
    })
    .await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// How many summaries of session `id` the workbench sent.
fn sent(s: &Setup, id: &str) -> usize {
    let summaries = s.host.summaries.lock().unwrap_or_else(|p| p.into_inner());
    summaries.iter().filter(|summary| summary.id == id).count()
}

/// A person message that clears no halt and ends no pause below the lead sends no summary of the sessions below it.
#[tokio::test]
async fn a_message_that_changes_nothing_below_sends_nothing_below() {
    let s = setup("shown-quiet", bounded());
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let c = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    until("the lead settles", || {
        thread(&s.workbench, &lead.id).status == ThreadStatus::Idle
    })
    .await;
    let before = sent(&s, &c.id);
    s.workbench
        .send(&lead.id, "Carry on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &lead.id, |e| turn_ends(e).len() >= 2).await;
    until("the lead settles again", || {
        thread(&s.workbench, &lead.id).status == ThreadStatus::Idle
    })
    .await;
    assert_eq!(sent(&s, &c.id), before);
    assert_eq!(halt_shown(&s, &c.id), None);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Interrupt on a session with no turn changes no status: its summary still goes out with the halt.
#[tokio::test]
async fn interrupt_shows_the_halt() {
    let s = setup("shown-interrupt", bounded());
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    assert_eq!(thread(&s.workbench, &lead.id).status, ThreadStatus::Idle);
    s.workbench.interrupt(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(halt_shown(&s, &lead.id).as_deref(), Some("person"));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A root is below the depth limit and a child at the default limit is at it: the app hides the switch there.
#[tokio::test]
async fn the_depth_limit_shows_on_the_deepest_session() {
    let s = setup("shown-depth", bounded());
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let c = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    assert_eq!((lead.at_depth_limit, c.at_depth_limit), (false, true));
    assert!(shown(&s, &c.id).is_some_and(|t| t.at_depth_limit));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A spend trip reaches an idle member without waiting for that session's next status change.
#[tokio::test]
async fn a_spend_trip_is_shown_on_idle_crew_members() {
    let s = setup(
        "shown-spend",
        Limits {
            crew_spend_usd: Some(0.02),
            ..bounded()
        },
    );
    let lead = root_at_work(&s, new_root(&s.project, "relay Lead")).await;
    let c = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    assert_eq!(thread(&s.workbench, &c.id).status, ThreadStatus::Idle);
    assert!(!shown(&s, &c.id).expect("child shown").crew_tripped);
    for _ in 0..20 {
        if s.workbench
            .list_children(&lead.id)
            .expect("children")
            .limits
            .spend_left_usd
            == Some(0.0)
        {
            break;
        }
        let before = s
            .workbench
            .events(&lead.id, 0)
            .expect("events")
            .iter()
            .filter(|e| matches!(e.event, WbEvent::Usage { .. }))
            .count();
        // Staged and renamed: the stand-in could read a file written in place while still empty.
        let staged = s.host.memory.join("relay.tmp");
        std::fs::write(&staged, "USAGE").expect("fake usage");
        std::fs::rename(staged, s.host.memory.join("relay")).expect("fake usage");
        wait_for(&s.workbench, &lead.id, |events| {
            events
                .iter()
                .filter(|e| matches!(e.event, WbEvent::Usage { .. }))
                .count()
                > before
        })
        .await;
    }
    until("idle child shows the spend trip", || {
        shown(&s, &c.id).is_some_and(|summary| summary.crew_tripped && summary.crew_paused)
    })
    .await;
    assert!(shown(&s, &lead.id).expect("lead shown").crew_tripped);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

async fn spend_at_former_default(explicit: bool) {
    use nexus_workbench::crew_chips::{Facts, Place, crew_chips};
    let mut limits = bounded();
    if explicit {
        limits.crew_spend_usd = Some(20.0);
    }
    let s = setup(&format!("cap-default-{explicit}"), limits);
    *s.host.call_price.lock().expect("fixture call price") = Some(0.0);
    let lead = root_at_work(&s, new_root(&s.project, "relay Lead")).await;
    *s.host.call_price.lock().expect("fixture call price") = Some(20.40);
    let worker = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    let summary = thread(&s.workbench, &worker.id);
    assert_eq!(summary.crew_paused, explicit, "crew pause after $20.40");
    let left = s
        .workbench
        .list_children(&lead.id)
        .expect("list crew")
        .limits
        .spend_left_usd;
    assert_eq!(left, explicit.then_some(0.0));
    if explicit {
        let chips = crew_chips(
            &Facts::of(&summary),
            &Place {
                root_title: Some("Linear connector".into()),
                linked: true,
                ..Place::default()
            },
        );
        assert_eq!(
            chips[0].text,
            "Spend cap of $20 reached ($20.40 spent) · write to the top session «Linear connector» to continue"
        );
        s.workbench.set_orchestration_limits(bounded());
        assert!(
            !thread(&s.workbench, &worker.id).crew_paused,
            "turning the cap off releases the crew"
        );
    }
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn no_cap_by_default_at_twenty_dollars() {
    spend_at_former_default(false).await;
}

#[tokio::test]
async fn explicit_cap_shows_the_estimate_and_can_be_disabled() {
    spend_at_former_default(true).await;
}
