//! Deferred production Workspace/owner lifecycle; every event traverses the app-scoped reader.
use super::super::{Auth, Workspace, machine_source::MachineSource};
use crate::gateway::{Account, Event};
use crate::model::{SessionSummary, Stored};
use gpui_kit::{Context, Entity, Task, TestAppContext, VisualTestContext, Window};
use serde_json::json;
use std::{cell::RefCell, rc::Rc, sync::Arc};
mod authority;
mod backend;
mod closed;
mod first_token;
mod frame_demand;
mod incremental;
mod receipts;
mod redraw;
mod stream_bench;
mod transactions;
mod transcript_pane;

pub(in crate::workspace) use backend::{Ask, Probe};
pub(in crate::workspace) fn account(org: &str, person: &str) -> Account {
    Account::from_json(&json!({"tenant":{"id":org,"slug":org,"name":"Fixture organization","kind":"company"},"user":{"id":person,"personId":person,"name":"A","email":format!("{person}@example.test")},"role":"member","permissions":[]})).expect("actual account wire fixture")
}
pub(in crate::workspace) fn fixture<'a>(
    cx: &'a mut TestAppContext,
    probe: &Probe,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.add_window_view(|window, cx| fixture_model(window, cx, probe))
}
pub(in crate::workspace) fn fixture_model(
    window: &mut Window,
    cx: &mut Context<Workspace>,
    probe: &Probe,
) -> Workspace {
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let mut workspace = Workspace::with_machine_source(
        Arc::new(probe.clone()),
        events,
        arrivals,
        false,
        MachineSource::isolated(),
        window,
        cx,
    );
    workspace._code_poll = Task::ready(());
    workspace.auth = Auth::SignedIn;
    workspace.side.open = false;
    workspace.code_scope = Some(format!("{}|org-a", probe.url));
    workspace.navigation_place_sink = Some(Rc::new(RefCell::new(vec![])));
    let account = account("org-a", "person-a");
    workspace.accept_navigation_identity(&account, cx);
    workspace.account = Some(account);
    workspace.gateway_account_authority = Some(crate::backend::ExecutionAuthority::from_read(
        probe.epoch.clone(),
        probe.epoch.load(std::sync::atomic::Ordering::SeqCst),
    ));
    super::super::drafts::fixture_memory_scope(&workspace, cx);
    workspace
}
pub(in crate::workspace) fn ask(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    probe: &Probe,
    text: &str,
) -> Ask {
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.ask(text.into(), Default::default(), cx);
        workspace.sync_gateway_chat(window, cx);
    });
    probe.asks().last().expect("actual Gateway ask").clone()
}
pub(in crate::workspace) fn named(probe: &Probe, asked: &Ask, id: &str) {
    probe.reply(
        asked,
        Event::Created {
            turn: asked.turn,
            id: id.into(),
            title: format!("Chat {id}"),
        },
    );
}
fn delta(probe: &Probe, asked: &Ask, text: &str) {
    probe.reply(
        asked,
        Event::Delta {
            turn: asked.turn,
            text: text.into(),
        },
    );
}
fn complete(probe: &Probe, asked: &Ask, text: &str) {
    probe.reply(
        asked,
        Event::Answered {
            turn: asked.turn,
            content: text.into(),
            page: None,
            sources: vec![],
        },
    );
}
fn chat(id: &str) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        title: format!("Chat {id}"),
        day: None,
        at_ms: None,
    }
}

#[gpui_kit::test]
fn actual_stream_keeps_its_side_and_partial_but_rebuilds_native_entities_after_primary_close(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (old, cx) = fixture(cx, &probe);
    let a = ask(&old, cx, &probe, "A");
    named(&probe, &a, "a");
    delta(&probe, &a, "before ");
    cx.run_until_parked();
    let old_answer = old.read_with(cx, |workspace, _| {
        workspace
            .entries
            .last()
            .expect("answer")
            .answer
            .clone()
            .expect("native answer")
    });
    let saved = old.update_in(cx, |workspace, window, cx| {
        let saved = workspace.retain_primary_terminals(cx).expect("primary owner");
        window.remove_window();
        saved
    });
    cx.run_until_parked();
    delta(&probe, &a, "while closed");
    cx.run_until_parked();
    let (fresh, cx) = fixture(cx, &probe);
    fresh.update_in(cx, |workspace, window, cx| {
        assert!(workspace.apply_primary_retained(saved, window, cx))
    });
    cx.run_until_parked();
    fresh.read_with(cx, |workspace, _| {
        assert_eq!(workspace.current.as_deref(), Some("a"));
        assert_eq!(workspace.asking, Some(a.turn));
        let answer = workspace.entries.last().expect("fresh answer");
        assert_eq!(answer.streamed, "before while closed");
        assert_ne!(
            answer.answer.as_ref().expect("fresh native answer").entity_id(),
            old_answer.entity_id()
        );
    });
    assert_eq!(probe.asks().len(), 1);
    fresh.update_in(cx, |_, window, cx| {
        old.update(cx, |workspace, cx| {
            workspace.ask("stale".into(), Default::default(), cx);
            workspace.enqueue_chat("stale queue".into(), Default::default(), true, window, cx);
            workspace.stop(window, cx);
        });
    });
    assert_eq!(probe.asks().len(), 1);
    assert!(probe.stops().is_empty());
    complete(&probe, &a, "final");
    cx.run_until_parked();
    fresh.read_with(cx, |workspace, _| {
        assert!(workspace.asking.is_none());
        assert_eq!(workspace.entries.last().expect("final answer").streamed, "final");
    });
}

#[gpui_kit::test]
fn unnamed_created_and_asked_receipts_dispatch_queued_follow_up_once_while_no_window_exists(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.enqueue_chat("B".into(), Default::default(), false, window, cx)
    });
    let saved = workspace.update_in(cx, |workspace, window, cx| {
        let saved = workspace.retain_primary_terminals(cx).expect("unnamed owner");
        window.remove_window();
        saved
    });
    cx.run_until_parked();
    named(&probe, &a, "new-a");
    probe.reply(
        &a,
        Event::Asked {
            turn: a.turn,
            question: Stored::default(),
        },
    );
    complete(&probe, &a, "A finished");
    cx.run_until_parked();
    let calls = probe.asks();
    assert_eq!(calls.len(), 2);
    let b = calls[1].clone();
    assert_eq!(b.prompt, "B");
    assert_eq!(b.session.as_deref(), Some("new-a"));
    assert_eq!(a.side, b.side);
    assert!(b.turn > a.turn);
    // Duplicate receipts for A cannot settle the new turn or dispatch B twice.
    complete(&probe, &a, "duplicate");
    probe.reply(
        &a,
        Event::Asked {
            turn: a.turn,
            question: Stored::default(),
        },
    );
    cx.run_until_parked();
    assert_eq!(probe.asks().len(), 2);
    let (fresh, cx) = fixture(cx, &probe);
    fresh.update_in(cx, |workspace, window, cx| {
        assert!(workspace.apply_primary_retained(saved, window, cx))
    });
    cx.run_until_parked();
    fresh.read_with(cx, |workspace, _| {
        assert_eq!(workspace.current.as_deref(), Some("new-a"));
        assert_eq!(workspace.asking, Some(b.turn));
        assert_eq!(
            workspace
                .entries
                .iter()
                .filter(|entry| entry.text.as_ref() == "B")
                .count(),
            1
        );
    });
    probe.reply(
        &b,
        Event::Asked {
            turn: b.turn,
            question: Stored::default(),
        },
    );
    complete(&probe, &b, "B finished");
    cx.run_until_parked();
    assert_eq!(probe.asks().len(), 2);
}

#[gpui_kit::test]
fn parked_lease_discard_and_last_active_owner_drop_abort_unnamed_transport(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "pending create");
    let saved = workspace.update_in(cx, |workspace, window, cx| {
        let saved = workspace.retain_primary_terminals(cx).expect("pending owner");
        window.remove_window();
        saved
    });
    cx.run_until_parked();
    // Retain the retired Workspace and its native answer; lease Drop must still revoke synchronously.
    drop(saved);
    assert_eq!(probe.stops(), vec![(a.side, a.turn)]);
    workspace.update(cx, |workspace, cx| {
        workspace.ask("old owner".into(), Default::default(), cx)
    });
    assert_eq!(probe.asks().len(), 1);
    let (active, cx) = fixture(cx, &probe);
    let b = ask(&active, cx, &probe, "no tray");
    active.update_in(cx, |_, window, _| window.remove_window());
    // Release inside an App update: its end flushes recursively dropped model entities.
    cx.cx.update(|_| drop(active));
    cx.run_until_parked();
    assert!(probe.stops().contains(&(b.side, b.turn)));
}

#[gpui_kit::test]
fn stop_without_terminal_event_holds_queue_and_send_now_resumes_exactly_one_follow_up(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    delta(&probe, &a, "partial");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.enqueue_chat("B".into(), Default::default(), false, window, cx);
        workspace.stop(window, cx);
    });
    assert_eq!(probe.stops(), vec![(a.side, a.turn)]);
    assert_eq!(probe.asks().len(), 1);
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.asking.is_none());
        assert!(workspace.entries.last().expect("stopped answer").stopped);
        assert!(workspace.chat_queue.queues.get("a").expect("held queue").held());
    });
    workspace.update_in(cx, |workspace, window, cx| workspace.resume_chat_queue(window, cx));
    let b = probe.asks()[1].clone();
    assert_eq!(b.prompt, "B");
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.enqueue_chat("C".into(), Default::default(), true, window, cx)
    });
    let calls = probe.asks();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[2].prompt, "C");
    assert!(calls[2].turn > b.turn);
    assert!(probe.stops().contains(&(b.side, b.turn)));
}
