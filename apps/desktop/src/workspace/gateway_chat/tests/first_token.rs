//! The execution owner is observed before a first ask can deliver tokens.
use super::{Probe, ask, delta, fixture};
use crate::{gateway::Event, workspace::View};
use gpui_kit::{RequestFrameOptions, TestAppContext};
use std::sync::atomic::Ordering;

#[gpui_kit::test]
fn a_first_ask_observes_tokens_before_the_workspace_renders(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    workspace.update(cx, |workspace, cx| {
        assert!(workspace.chat_execution.is_none());
        workspace.ask("Question".into(), Default::default(), cx);
        assert!(workspace.chat_execution.is_some());
        assert!(
            workspace._chat_execution.is_some(),
            "the first ask installs its observer before a render or explicit projection"
        );
    });
    let asked = probe.asks().last().expect("actual Gateway ask").clone();
    delta(&probe, &asked, "first token");
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.entries.last().expect("answer").streamed, "first token");
    });
}

#[gpui_kit::test]
fn queued_tokens_reach_the_existing_draw_before_the_async_owner_task_is_polled(cx: &mut TestAppContext) {
    cx.defer_draws_to_platform();
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    workspace.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    let asked = ask(&workspace, cx, &probe, "Question");
    let window = workspace.read_with(cx, |workspace, _| {
        assert!(workspace.first_token_armed);
        workspace.window_handle
    });
    let native = cx.cx.test_window(window);
    delta(&probe, &asked, "first ");
    delta(&probe, &asked, "second");
    native.simulate_frame_request(RequestFrameOptions::default());
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.entries.last().expect("answer").streamed, "first ");
        assert!(!workspace.first_token_armed, "the first text ends passive delivery");
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.entries.last().expect("answer").streamed, "first second");
    });
}

#[gpui_kit::test]
fn a_parked_owner_keeps_async_delivery_after_a_bounded_drain_and_rebinds_the_real_projection(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let asked = ask(&workspace, cx, &probe, "Question");
    let saved = workspace.update(cx, |workspace, cx| {
        workspace.retain_gateway_chat(cx).expect("retained owner")
    });
    let owner = saved.owner.as_ref().expect("parked owner").clone();
    let key = saved.selected.expect("visible turn");
    cx.run_until_parked();
    for _ in 0..64 {
        probe.reply(
            &asked,
            Event::Delta {
                turn: asked.turn + 100,
                text: "ignored".into(),
            },
        );
    }
    delta(&probe, &asked, "first ");
    delta(&probe, &asked, "second");
    let received = owner.update(cx, |owner, cx| owner.drain_first_token(key, cx));
    assert_eq!(received, 64, "a draw never drains more than its event budget");
    assert!(
        saved.state.borrow().runs[&key]
            .entries
            .last()
            .expect("answer")
            .streamed
            .is_empty()
    );
    cx.run_until_parked();
    assert_eq!(
        saved.state.borrow().runs[&key].entries.last().expect("answer").streamed,
        "first second"
    );
    let (fresh, cx) = fixture(cx, &probe);
    fresh.update_in(cx, |workspace, window, cx| {
        let selected = workspace.adopt_gateway_chat(Some(saved), window, cx);
        workspace.restore_gateway_selection(selected, window, cx);
        assert_eq!(workspace.entries.last().expect("fresh answer").streamed, "first second");
        assert!(!workspace.first_token_armed);
    });
}

#[gpui_kit::test]
fn a_pending_hook_does_not_read_an_offscreen_chats_events(cx: &mut TestAppContext) {
    cx.defer_draws_to_platform();
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    workspace.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    let asked = ask(&workspace, cx, &probe, "Question");
    let (window, state, key) = workspace.update(cx, |workspace, _| {
        workspace.view = View::NewCode;
        (
            workspace.window_handle,
            workspace.chat_execution.as_ref().expect("owner").state.clone(),
            workspace.chat_execution_at.expect("turn"),
        )
    });
    let native = cx.cx.test_window(window);
    delta(&probe, &asked, "background");
    native.simulate_frame_request(RequestFrameOptions::default());
    assert!(
        state.borrow().runs[&key]
            .entries
            .last()
            .expect("answer")
            .streamed
            .is_empty()
    );
    workspace.read_with(cx, |workspace, _| assert!(!workspace.first_token_armed));
    cx.run_until_parked();
    assert_eq!(
        state.borrow().runs[&key].entries.last().expect("answer").streamed,
        "background"
    );
}

#[gpui_kit::test]
fn first_token_hooks_stop_on_failure_and_refuse_revoked_authority(cx: &mut TestAppContext) {
    cx.defer_draws_to_platform();
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    workspace.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    let asked = ask(&workspace, cx, &probe, "Question");
    let window = workspace.read_with(cx, |workspace, _| workspace.window_handle);
    let native = cx.cx.test_window(window);
    probe.reply(
        &asked,
        Event::AnswerFailed {
            turn: asked.turn,
            error: "fixture failure".into(),
        },
    );
    native.simulate_frame_request(RequestFrameOptions::default());
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.asking.is_none());
        assert!(!workspace.first_token_armed);
    });
    let asked = ask(&workspace, cx, &probe, "Next question");
    probe.epoch.fetch_add(1, Ordering::SeqCst);
    delta(&probe, &asked, "revoked text");
    native.simulate_frame_request(RequestFrameOptions::default());
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.asking.is_none());
        assert!(!workspace.first_token_armed);
        assert!(workspace.entries.last().expect("answer").streamed.is_empty());
    });
}

#[gpui_kit::test]
fn waiting_for_a_first_token_keeps_an_idle_reduced_motion_window_parked_and_stops_on_cancel(cx: &mut TestAppContext) {
    cx.defer_draws_to_platform();
    cx.update(|cx| cx.set_reduce_motion(true));
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    cx.cx.update(|cx| cx.set_reduce_motion(true));
    workspace.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    let asked = ask(&workspace, cx, &probe, "Question");
    let window = workspace.read_with(cx, |workspace, _| workspace.window_handle);
    let native = cx.cx.test_window(window);
    for _ in 0..6 {
        native.simulate_scheduled_frame();
    }
    workspace.read_with(cx, |workspace, _| assert!(workspace.first_token_armed));
    assert!(
        !native.frame_scheduled(),
        "waiting for a token creates no animation loop"
    );
    let wakes = native.frame_wake_count();
    native.simulate_frame_request(RequestFrameOptions::default());
    assert_eq!(
        native.frame_wake_count(),
        wakes,
        "a passive hook leaves clean ticks alone"
    );
    workspace.update_in(cx, |workspace, window, cx| workspace.stop(window, cx));
    for _ in 0..6 {
        native.simulate_scheduled_frame();
    }
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.asking.is_none());
        assert!(!workspace.first_token_armed);
    });
    assert_eq!(probe.stops(), vec![(asked.side, asked.turn)]);
    assert!(!native.frame_scheduled(), "cancel returns the window to idle");
}
