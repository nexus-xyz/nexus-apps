//! A wake-up Claude Code sets for itself (`ScheduleWakeup`), followed by the workbench: the session shows it, the idle
//! reaper spares the agent that holds it, and a run that ends first says the wake-up will not come.

mod support;

use std::time::Duration;

use nexus_workbench::event::{StoredEvent, WbEvent};
use support::*;

fn notices(events: &[StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Notice { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn lost(events: &[StoredEvent]) -> Vec<String> {
    notices(events)
        .into_iter()
        .filter(|text| text.starts_with("The wake-up the agent set for "))
        .collect()
}

#[tokio::test]
async fn the_session_shows_the_wake_up_and_a_stop_says_it_will_not_come() {
    let s = setup_with("wake-stop", bounded(), false, |config| config);
    let before = nexus_workbench::now_ms();
    let session = root(&s, new_root(&s.project, "wakeup")).await;
    let wake = session.wake.clone().expect("the session shows its wake-up");
    assert_eq!(wake.reason, "watching CI run");
    let due = wake.at - wake.set_at;
    assert_eq!(due, 600_000, "ten minutes after the call");
    assert!(wake.set_at >= before);
    assert!(lost(&s.workbench.events(&session.id, 0).unwrap_or_default()).is_empty());

    s.workbench.stop(&session.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &session.id).await;
    let events = s.workbench.events(&session.id, 0).unwrap_or_default();
    let lost = lost(&events);
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert!(
        lost[0].ends_with("will not come: the agent was stopped. Send a message to continue."),
        "{}",
        lost[0]
    );
    assert_eq!(thread(&s.workbench, &session.id).wake, None);
    assert_eq!(shown(&s, &session.id).and_then(|summary| summary.wake), None);
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_refused_call_sets_no_wake_up() {
    let s = setup_with("wake-refused", bounded(), false, |config| config);
    let session = root(&s, new_root(&s.project, "wakeup-refused")).await;
    assert_eq!(session.wake, None);
    s.workbench.stop(&session.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &session.id).await;
    assert!(lost(&s.workbench.events(&session.id, 0).unwrap_or_default()).is_empty());
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A setup whose wake-ups are due `delay` seconds after the call, with no shortest delay, and come within `grace`.
fn timed(name: &str, delay: &str, grace: Duration) -> Setup {
    let s = setup_with(name, bounded(), false, |config| nexus_workbench::Config {
        wake_timing: nexus_workbench::wake::Timing {
            delays: (Duration::ZERO, Duration::from_secs(3600)),
            grace,
        },
        ..config
    });
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_CLAUDE_WAKE_SECONDS".into(), delay.into());
    s
}

/// Its time and grace passed and the agent began no turn: the log says it did not come, once, and the session is
/// shown again without it, with a reply to read. The agent keeps running: the person's next message reaches it.
#[tokio::test]
async fn a_wake_up_that_does_not_come_is_reported_once() {
    let s = timed("wake-missed", "0.2", Duration::from_millis(300));
    let session = root(&s, new_root(&s.project, "wakeup")).await;
    assert!(session.wake.is_some());
    let events = wait_for(&s.workbench, &session.id, |events| !lost(events).is_empty()).await;
    let lost_now = lost(&events);
    assert_eq!(lost_now.len(), 1, "{lost_now:?}");
    assert!(
        lost_now[0].ends_with(" did not come. Send a message to continue."),
        "{}",
        lost_now[0]
    );
    until("the session is shown without its wake-up", || {
        shown(&s, &session.id).is_some_and(|summary| summary.wake.is_none() && summary.updated_at > session.updated_at)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(lost(&s.workbench.events(&session.id, 0).unwrap_or_default()).len(), 1);
    assert!(s.workbench.is_live(&session.id));
    s.workbench.stop(&session.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &session.id).await;
    let events = s.workbench.events(&session.id, 0).unwrap_or_default();
    assert_eq!(lost(&events).len(), 1, "a stop after the miss says nothing more");
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A turn the agent begins on its own at its wake-up's time is the wake-up: it is no longer shown, and nothing says
/// it was lost. One it begins long before is some other turn, and the wake-up stays.
#[tokio::test]
async fn an_own_turn_at_the_time_is_the_wake_up_and_an_early_one_is_not() {
    for (delay, fired) in [("0.1", true), ("600", false)] {
        let s = timed(&format!("wake-fired-{fired}"), delay, Duration::from_secs(60));
        let session = root(&s, new_root(&s.project, "wakeup autoturn")).await;
        assert!(session.wake.is_some());
        tokio::time::sleep(Duration::from_millis(200)).await;
        let native = nexus_workbench::store::Store::open(&s.root.join("workbench.sqlite3"))
            .unwrap_or_else(|e| panic!("{e}"))
            .thread(&session.id)
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("no session"))
            .native_id;
        trigger(&s, &format!("automatic-{native}"));
        wait_for(&s.workbench, &session.id, |events| {
            events
                .iter()
                .filter(|e| matches!(e.event, WbEvent::TurnStart { .. }))
                .count()
                == 2
        })
        .await;
        trigger(&s, &format!("finish-automatic-{native}"));
        wait_for(&s.workbench, &session.id, |events| turn_ends(events).len() == 2).await;
        assert_eq!(thread(&s.workbench, &session.id).wake.is_none(), fired, "delay {delay}");
        s.workbench.stop(&session.id).unwrap_or_else(|e| panic!("{e}"));
        until_stopped(&s.workbench, &session.id).await;
        let lost = lost(&s.workbench.events(&session.id, 0).unwrap_or_default());
        assert_eq!(lost.len(), usize::from(!fired), "delay {delay}: {lost:?}");
        let _ = std::fs::remove_dir_all(&s.root);
    }
}

/// The wake-up lives in the agent's process: stopping an idle agent would cancel it. Without one, the same agent is
/// stopped.
#[tokio::test]
async fn the_idle_reaper_spares_an_agent_waiting_on_its_wake_up() {
    for (text, spared) in [("wakeup", true), ("hello", false)] {
        let s = setup_with(&format!("wake-idle-{spared}"), bounded(), false, |config| {
            nexus_workbench::Config {
                idle_after: Duration::from_millis(50),
                ..config
            }
        });
        let session = root(&s, new_root(&s.project, text)).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        s.workbench.reap_idle();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(s.workbench.is_live(&session.id), spared, "{text}");
        if spared {
            s.workbench.stop(&session.id).unwrap_or_else(|e| panic!("{e}"));
        }
        until_stopped(&s.workbench, &session.id).await;
        let _ = std::fs::remove_dir_all(&s.root);
    }
}
