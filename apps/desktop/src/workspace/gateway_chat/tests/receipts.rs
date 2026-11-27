//! Real deferred sends, accepted-question receipts and source-validated fresh projections.
use super::{Ask, Probe, ask, chat, complete, fixture, named};
use crate::{
    attachments::{Attachment, Draft},
    gateway::{Event, OpenedChat},
    model::Stored,
};
use gpui_kit::TestAppContext;
use serde_json::json;
use std::sync::Arc;
fn file(id: u64, name: &str) -> Attachment {
    Attachment {
        artifact: None,
        id,
        name: name.into(),
        bytes: Arc::new(b"fixture".to_vec()),
        media_type: None,
        thumbnail: None,
    }
}
fn fail(probe: &Probe, ask: &Ask) {
    probe.reply(
        ask,
        Event::AnswerFailed {
            turn: ask.turn,
            error: "fixture failure".into(),
        },
    );
}

#[gpui_kit::test]
fn queued_send_requeues_only_before_asked_and_preserves_later_items_across_reopen(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.enqueue_chat("B".into(), Default::default(), false, window, cx);
        workspace.enqueue_chat("C".into(), Default::default(), false, window, cx);
    });
    let saved = workspace.update_in(cx, |workspace, window, cx| {
        let saved = workspace.retain_primary_terminals(cx).expect("primary");
        window.remove_window();
        saved
    });
    cx.run_until_parked();
    complete(&probe, &a, "A done");
    cx.run_until_parked();
    let b = probe.asks()[1].clone();
    assert_eq!(b.prompt, "B");
    fail(&probe, &b);
    cx.run_until_parked();
    assert_eq!(probe.asks().len(), 2);
    let (fresh, cx) = fixture(cx, &probe);
    fresh.update_in(cx, |workspace, window, cx| {
        assert!(workspace.apply_primary_retained(saved, window, cx))
    });
    cx.run_until_parked();
    fresh.read_with(cx, |workspace, _| {
        let queue = workspace.chat_queue.queues.get("a").expect("held queue");
        assert!(queue.held());
        assert_eq!(
            queue.items().iter().map(|item| item.text.as_str()).collect::<Vec<_>>(),
            ["B", "C"]
        );
    });
    fresh.update_in(cx, |workspace, window, cx| workspace.resume_chat_queue(window, cx));
    let retry = probe.asks()[2].clone();
    assert_eq!(retry.prompt, "B");
    assert!(retry.turn > b.turn);
    probe.reply(
        &retry,
        Event::Asked {
            turn: retry.turn,
            question: Stored::default(),
        },
    );
    fail(&probe, &retry);
    cx.run_until_parked();
    fresh.read_with(cx, |workspace, _| {
        let queue = workspace.chat_queue.queues.get("a").expect("C still held");
        assert!(queue.held());
        assert_eq!(queue.items().len(), 1);
        assert_eq!(queue.items()[0].text, "C");
    });
    assert_eq!(probe.asks().len(), 3);
}

#[gpui_kit::test]
fn failed_edit_send_restores_once_beneath_newer_text_and_files_after_primary_reopen(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open(&chat("a"), window, cx);
        workspace.sync_draft(window, cx);
        workspace.give_back(
            true,
            "submitted edit".into(),
            Draft {
                files: vec![file(1, "submitted.txt")],
                ..Default::default()
            },
            None,
            window,
            cx,
        );
        workspace.sync_gateway_chat(window, cx);
    });
    let asked = probe.asks()[0].clone();
    assert_eq!(asked.attachments.files[0].name, "submitted.txt");
    workspace.update_in(cx, |workspace, window, cx| {
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("newer text", window, cx));
        workspace.draft.files.push(file(2, "newer.txt"));
        workspace.sync_draft(window, cx);
    });
    cx.run_until_parked();
    let saved = workspace.update_in(cx, |workspace, window, cx| {
        let saved = workspace.retain_primary_terminals(cx).expect("primary");
        window.remove_window();
        saved
    });
    cx.run_until_parked();
    fail(&probe, &asked);
    cx.run_until_parked();
    let (fresh, cx) = fixture(cx, &probe);
    fresh.update_in(cx, |workspace, window, cx| {
        assert!(workspace.apply_primary_retained(saved, window, cx))
    });
    cx.run_until_parked();
    fresh.update_in(cx, |workspace, window, cx| {
        workspace.sync_gateway_chat(window, cx);
        workspace.sync_gateway_chat(window, cx);
    });
    cx.run_until_parked();
    fresh.read_with(cx, |workspace, cx| {
        let text = workspace.composer.read(cx).value().to_string();
        assert!(text.starts_with("newer text"));
        assert_eq!(text.matches("submitted edit").count(), 1);
        assert_eq!(
            workspace
                .draft
                .files
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            ["newer.txt", "submitted.txt"]
        );
    });
    assert_eq!(probe.asks().len(), 1);
}

#[gpui_kit::test]
fn accepted_edit_receipt_cannot_be_overwritten_by_immediate_close_snapshot(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open(&chat("a"), window, cx);
        workspace.sync_draft(window, cx);
        workspace.give_back(true, "accepted edit".into(), Default::default(), None, window, cx);
        workspace.sync_gateway_chat(window, cx);
    });
    let asked = probe.asks()[0].clone();
    probe.reply(
        &asked,
        Event::Asked {
            turn: asked.turn,
            question: Stored::default(),
        },
    );
    cx.run_until_parked();
    let saved = workspace.update_in(cx, |workspace, window, cx| {
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("later", window, cx));
        workspace.sync_draft(window, cx);
        let saved = workspace.retain_primary_terminals(cx).expect("primary");
        window.remove_window();
        saved
    });
    cx.run_until_parked();
    fail(&probe, &asked);
    cx.run_until_parked();
    let (fresh, cx) = fixture(cx, &probe);
    fresh.update_in(cx, |workspace, window, cx| {
        assert!(workspace.apply_primary_retained(saved, window, cx))
    });
    cx.run_until_parked();
    fresh.read_with(cx, |workspace, cx| {
        assert_eq!(workspace.composer.read(cx).value().as_ref(), "later")
    });
    assert_eq!(probe.asks().len(), 1);
}

#[gpui_kit::test]
fn successful_durable_read_releases_completed_model_instead_of_shadowing_later_navigation(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    complete(&probe, &a, "done");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.read_chat("a".into());
        let stale = probe.reads().last().expect("first actual dispatch").1;
        workspace.read_chat("a".into());
        let current = probe.reads().last().expect("replacement actual dispatch").1;
        assert_ne!(stale, current);
        let stored = OpenedChat::from_detail("a".into(), &json!({"id":"a","messages":[]}));
        workspace.handle(
            Event::ChatRead {
                read: stale,
                id: "a".into(),
                cached: false,
                chat: Ok(stored.clone()),
            },
            window,
            cx,
        );
        assert!(workspace.chat_execution_at.is_some());
        workspace.handle(
            Event::ChatRead {
                read: current,
                id: "a".into(),
                cached: true,
                chat: Ok(stored.clone()),
            },
            window,
            cx,
        );
        assert!(workspace.chat_execution_at.is_some());
        workspace.handle(
            Event::ChatRead {
                read: current,
                id: "a".into(),
                cached: false,
                chat: Ok(stored),
            },
            window,
            cx,
        );
        assert!(workspace.chat_execution_at.is_none());
        workspace.new_chat(window, cx);
        assert!(!workspace.resume_gateway_chat(Some("a"), window, cx));
        workspace.open(&chat("a"), window, cx);
        assert!(workspace.opening);
    });
    assert_eq!(probe.asks().len(), 1);
}

#[gpui_kit::test]
fn parked_created_receipt_promotes_its_snapshot_once_without_stealing_reused_home_draft(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    workspace.update_in(cx, |workspace, window, cx| workspace.sync_draft(window, cx));
    let a = ask(&workspace, cx, &probe, "A");
    workspace.update_in(cx, |workspace, window, cx| {
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("old pending draft", window, cx));
        workspace.sync_draft(window, cx);
    });
    cx.run_until_parked();
    let saved = workspace.update_in(cx, |workspace, window, cx| {
        let saved = workspace.retain_primary_terminals(cx).expect("pending owner");
        window.remove_window();
        saved
    });
    cx.run_until_parked();
    let (fresh, cx) = fixture(cx, &probe);
    fresh.update_in(cx, |workspace, window, cx| {
        workspace.new_chat(window, cx);
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("new Home draft", window, cx));
        workspace.sync_draft(window, cx);
    });
    cx.run_until_parked();
    named(&probe, &a, "old-chat");
    named(&probe, &a, "old-chat");
    cx.run_until_parked();
    fresh.update_in(cx, |workspace, window, cx| {
        assert!(workspace.apply_primary_retained(saved, window, cx))
    });
    cx.run_until_parked();
    fresh.update_in(cx, |workspace, window, cx| {
        workspace.sync_draft(window, cx);
        assert!(workspace.current.is_none());
        assert_eq!(workspace.composer.read(cx).value().as_ref(), "new Home draft");
        workspace.open(&chat("old-chat"), window, cx);
        assert_eq!(workspace.composer.read(cx).value().as_ref(), "old pending draft");
        workspace.new_chat(window, cx);
        workspace.sync_draft(window, cx);
        assert_eq!(workspace.composer.read(cx).value().as_ref(), "new Home draft");
    });
    assert_eq!(probe.asks().len(), 1);
}

#[gpui_kit::test]
fn a_only_durable_metadata_cannot_be_assigned_to_running_queued_b(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.enqueue_chat("B".into(), Default::default(), false, window, cx)
    });
    complete(&probe, &a, "A final");
    cx.run_until_parked();
    let b = probe.asks()[1].clone();
    workspace.update_in(cx,|workspace,window,cx|{let read=probe.reads().last().expect("A completion actual read").1;
        let chat=OpenedChat::from_detail("a".into(),&json!({"messages":[{"role":"user","content":"A"},{"role":"assistant","content":"A final","costUsd":0.11}]}));
        workspace.handle(Event::ChatRead{read,id:"a".into(),cached:false,chat:Ok(chat)},window,cx);
        assert!(workspace.entries.last().expect("B still running").meta.cost_usd.is_none());
    });
    complete(&probe, &b, "B final");
    cx.run_until_parked();
    workspace.update_in(cx,|workspace,window,cx|{let read=probe.reads().last().expect("B completion current read").1;
        let chat=OpenedChat::from_detail("a".into(),&json!({"messages":[{"role":"user","content":"A"},{"role":"assistant","content":"A final","costUsd":0.11},{"role":"user","content":"B"},{"role":"assistant","content":"B final","costUsd":0.22}]}));
        workspace.handle(Event::ChatRead{read,id:"a".into(),cached:false,chat:Ok(chat)},window,cx);
        assert_eq!(workspace.entries[1].meta.cost_usd,Some(0.11));assert_eq!(workspace.entries[3].meta.cost_usd,Some(0.22));assert!(workspace.chat_execution_at.is_none());
    });
}

#[gpui_kit::test]
fn queued_follow_up_uses_latest_selected_model_after_primary_is_parked(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    let saved = workspace.update_in(cx, |workspace, window, cx| {
        workspace.pick_model(
            Some(crate::model::ModelChoice {
                id: "fixture-model-b".into(),
                name: "Fixture B".into(),
                maker: "fixture".into(),
                hint: None,
                usable: true,
                off: false,
                output_usd: None,
            }),
            window,
            cx,
        );
        workspace.enqueue_chat("B".into(), Default::default(), false, window, cx);
        let saved = workspace.retain_primary_terminals(cx).expect("primary");
        window.remove_window();
        saved
    });
    cx.run_until_parked();
    complete(&probe, &a, "A final");
    cx.run_until_parked();
    let calls = probe.asks();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].prompt, "B");
    assert_eq!(calls[1].model.as_deref(), Some("fixture-model-b"));
    drop(saved);
}

#[gpui_kit::test]
fn policy_preferences_and_repeated_captures_do_not_erase_required_next_dispatch_patch(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    workspace.update(cx, |workspace, _| {
        workspace.panel = true;
        workspace.choices.org_settings = Some(json!({"webSearch.engine":"off"}));
    });
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    assert!(!a.choices.web_search);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.handle(
            Event::OrgSettings(Some(json!({"webSearch.engine":"borrowed"}))),
            window,
            cx,
        );
        workspace.pick_model(None, window, cx);
        workspace.capture_gateway_choices(cx);
        assert_eq!(workspace.choices.session_web_search, Some(false));
    });
    super::delta(&probe, &a, "A still running");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        assert_eq!(workspace.choices.session_web_search, Some(false));
        workspace.enqueue_chat("B".into(), Default::default(), false, window, cx);
    });
    complete(&probe, &a, "A final");
    cx.run_until_parked();
    let calls = probe.asks();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].choices.web_search);
    assert!(calls[1].choices.patch_web_search);
}
