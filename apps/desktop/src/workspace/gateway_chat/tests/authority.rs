//! Auth boundaries are independent from native-window and request incarnations.
use super::{Probe, account, ask, complete, delta, fixture, named};
use crate::gateway::Event;
use gpui_kit::TestAppContext;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[gpui_kit::test]
fn independent_epoch_and_read_revision_reject_actual_queued_stream_replies(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    let read_epoch = Arc::new(AtomicU64::new(2));
    probe.emit(
        a.side,
        Event::ReadSnapshot {
            epoch: read_epoch.clone(),
            expected: 1,
            revision: None,
            event: Box::new(Event::Delta {
                turn: a.turn,
                text: "old credential".into(),
            }),
        },
    );
    let revision = Arc::new(AtomicU64::new(2));
    probe.emit(
        a.side,
        Event::ReadSnapshot {
            epoch: read_epoch,
            expected: 2,
            revision: Some((revision, 1)),
            event: Box::new(Event::Delta {
                turn: a.turn,
                text: "old revision".into(),
            }),
        },
    );
    delta(&probe, &a, "current");
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.entries.last().expect("answer").streamed, "current")
    });
}

#[gpui_kit::test]
fn signed_in_aba_is_sticky_even_when_latest_credential_fingerprint_matches_again(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "pending creation");
    *probe.marker.lock().expect("marker") = Some("fixture-identity-b".into());
    probe.emit(a.side, Event::SignedIn);
    *probe.marker.lock().expect("marker") = Some("fixture-identity-a".into());
    probe.emit(a.side, Event::SignedIn);
    named(&probe, &a, "late-old-created");
    cx.run_until_parked();
    assert_eq!(probe.stops(), vec![(a.side, a.turn)]);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("keep this draft", window, cx));
        workspace.submit_now(false, window, cx);
    });
    workspace.read_with(cx, |workspace, cx| {
        assert_eq!(workspace.composer.read(cx).value().as_ref(), "keep this draft");
        assert!(workspace.asking.is_none());
        assert!(workspace.entries.last().expect("stopped answer").stopped);
    });
    assert_eq!(probe.asks().len(), 1);
}

#[gpui_kit::test]
fn matching_account_read_preserves_owner_but_person_only_change_revokes(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    probe.reply(&a, Event::Account(account("org-a", "person-a")));
    delta(&probe, &a, "valid");
    cx.run_until_parked();
    assert!(probe.stops().is_empty());
    let mut changed = account("org-a", "person-a");
    changed.person_id = Some("person-b".into());
    probe.reply(&a, Event::Account(changed));
    cx.run_until_parked();
    assert_eq!(probe.stops(), vec![(a.side, a.turn)]);
    workspace.update(cx, |workspace, cx| {
        workspace.ask("forbidden".into(), Default::default(), cx)
    });
    assert_eq!(probe.asks().len(), 1);
}

#[gpui_kit::test]
fn org_only_change_and_explicit_gateway_teardown_stop_all_named_and_unnamed_runs(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| workspace.new_chat(window, cx));
    let b = ask(&workspace, cx, &probe, "unnamed B");
    probe.reply(&a, Event::Account(account("org-b", "person-a")));
    cx.run_until_parked();
    let stops = probe.stops();
    assert!(stops.contains(&(a.side, a.turn)));
    assert!(stops.contains(&(b.side, b.turn)));
    assert_eq!(stops.len(), 2);
    let (fresh, cx) = fixture(cx, &probe);
    let c = ask(&fresh, cx, &probe, "gateway boundary");
    // clear_session is the production Gateway replacement teardown, without changing persisted preferences.
    fresh.update(cx, |workspace, cx| workspace.clear_session(cx));
    assert!(probe.stops().contains(&(c.side, c.turn)));
}

#[gpui_kit::test]
fn independent_window_sides_cannot_stop_or_adopt_one_anothers_turns(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (primary, cx) = fixture(cx, &probe);
    let a = ask(&primary, cx, &probe, "primary");
    named(&probe, &a, "same-id");
    cx.run_until_parked();
    let saved = primary.update_in(cx, |workspace, window, cx| {
        let saved = workspace.retain_primary_terminals(cx).expect("primary");
        window.remove_window();
        saved
    });
    cx.run_until_parked();
    let (secondary, cx) = fixture(cx, &probe);
    let b = ask(&secondary, cx, &probe, "secondary");
    assert_ne!(a.side, b.side);
    named(&probe, &b, "same-id");
    delta(&probe, &b, "secondary text");
    cx.run_until_parked();
    drop(saved);
    assert_eq!(probe.stops(), vec![(a.side, a.turn)]);
    complete(&probe, &b, "secondary final");
    cx.run_until_parked();
    secondary.read_with(cx, |workspace, _| {
        assert_eq!(
            workspace.entries.last().expect("secondary answer").streamed,
            "secondary final"
        )
    });
}

#[gpui_kit::test]
fn retained_execution_rejects_fresh_primary_with_different_gateway_and_discards_old_lease(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    let saved = workspace.update_in(cx, |workspace, window, cx| {
        let saved = workspace.retain_primary_terminals(cx).expect("primary");
        window.remove_window();
        saved
    });
    cx.run_until_parked();
    let mut other = probe.clone();
    other.url = "https://other-gateway.example.test".into();
    let (fresh, cx) = fixture(cx, &other);
    fresh.update_in(cx, |workspace, window, cx| {
        assert!(!workspace.apply_primary_retained(saved, window, cx))
    });
    assert_eq!(probe.stops(), vec![(a.side, a.turn)]);
    fresh.read_with(cx, |workspace, _| {
        assert!(workspace.chat_execution.is_none());
        assert!(workspace.entries.is_empty());
    });
}

#[gpui_kit::test]
fn pinned_side_epoch_refusal_is_observable_before_auth_events_drain(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "pending A");
    // Same final fingerprint after an epoch boundary must not authorize this old execution backend.
    probe.epoch.fetch_add(1, Ordering::SeqCst);
    workspace.update(cx, |workspace, cx| {
        workspace.ask("forbidden continuation".into(), Default::default(), cx)
    });
    assert_eq!(probe.asks().len(), 1);
    assert_eq!(probe.stops(), vec![(a.side, a.turn)]);
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.asking.is_none());
        assert!(workspace.entries.last().expect("stopped answer").stopped);
    });
}

#[gpui_kit::test]
fn queued_next_dispatch_straddling_credential_epoch_cannot_send_or_remain_busy(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.enqueue_chat("must not cross identity".into(), Default::default(), false, window, cx)
    });
    probe.revoke_next_dispatch();
    complete(&probe, &a, "captured completion");
    cx.run_until_parked();
    assert_eq!(probe.asks().len(), 1);
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.asking.is_none());
        let binding = workspace.chat_execution.as_ref().expect("sticky revoked binding");
        let state = binding.state.borrow();
        assert!(state.revoked);
        assert!(state.runs.is_empty());
    });
}
