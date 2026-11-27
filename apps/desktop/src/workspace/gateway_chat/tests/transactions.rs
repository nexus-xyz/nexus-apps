//! Actual submit/edit admission races retain payloads in the frozen original draft scope.
use super::super::super::drafts::{Submission, fixture_scopes, fixture_submission_state};
use super::{Probe, ask, chat, fixture, named};
use crate::attachments::{Attachment, Draft};
use crate::workspace::Workspace;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use std::sync::{Arc, atomic::Ordering};
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
fn prepare(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, text: &str) -> Submission {
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.sync_draft(window, cx);
        workspace
            .composer
            .update(cx, |input, cx| input.set_value(text, window, cx));
        workspace.draft.files = vec![file(10, "original.txt")];
        workspace.sync_draft(window, cx);
        workspace
            .gateway_draft_submission(cx)
            .expect("frozen synchronized source")
    })
}
fn assert_original(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, receipt: &Submission, text: &str) {
    workspace.read_with(cx, |_, cx| {
        let (draft, _) = fixture_submission_state(receipt, cx);
        assert_eq!(draft.text, text);
        assert_eq!(
            draft.files.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(),
            ["original.txt"]
        );
        assert_eq!(fixture_scopes(cx), 1);
    });
}
#[gpui_kit::test]
fn actual_first_submit_factory_aba_denial_keeps_unconsumed_original_text_and_files(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let receipt = prepare(&workspace, cx, "factory draft");
    cx.run_until_parked();
    let revision = workspace.read_with(cx, |_, cx| fixture_submission_state(&receipt, cx).1);
    probe.revoke_next_factory();
    workspace.update_in(cx, |workspace, window, cx| workspace.submit_now(false, window, cx));
    cx.run_until_parked();
    assert!(probe.asks().is_empty());
    assert_original(&workspace, cx, &receipt, "factory draft");
    workspace.read_with(cx, |workspace, cx| {
        assert_eq!(workspace.composer.read(cx).value().as_ref(), "factory draft");
        assert_eq!(fixture_submission_state(&receipt, cx).1, revision);
        assert!(workspace.chat_execution.is_none());
    });
}
#[gpui_kit::test]
fn actual_submit_next_guard_refusal_recovers_consumed_payload_only_in_original_scope(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let receipt = prepare(&workspace, cx, "guard draft");
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.ensure_gateway_execution(cx));
        workspace.sync_gateway_chat(window, cx);
    });
    cx.run_until_parked();
    // First pinned marker is the view synchronization; second is the pre-consumption admission check.
    probe.revoke_after_pinned_checks(2);
    workspace.update_in(cx, |workspace, window, cx| workspace.submit_now(false, window, cx));
    cx.run_until_parked();
    assert!(probe.asks().is_empty());
    assert_original(&workspace, cx, &receipt, "guard draft");
    workspace.read_with(cx, |workspace, _| assert!(workspace.asking.is_none()));
}
#[gpui_kit::test]
fn actual_submit_pinned_transport_refusal_finalizes_optimistic_answer_and_recovers_files(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let receipt = prepare(&workspace, cx, "dispatch draft");
    cx.run_until_parked();
    probe.revoke_next_dispatch();
    workspace.update_in(cx, |workspace, window, cx| workspace.submit_now(false, window, cx));
    cx.run_until_parked();
    assert!(probe.asks().is_empty());
    assert_original(&workspace, cx, &receipt, "dispatch draft");
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.asking.is_none());
        let answer = workspace.entries.last().expect("optimistic refused answer");
        assert!(answer.stopped);
        assert!(!answer.streaming);
    });
    assert_eq!(probe.stops().len(), 1);
}
#[gpui_kit::test]
fn frozen_refusal_merges_once_under_newer_identical_text_files_and_revision(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let receipt = prepare(&workspace, cx, "same text");
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.draft_submitted(Some(receipt.clone()), window, cx);
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("same text", window, cx));
        workspace.draft.files = vec![file(20, "newer.txt")];
        workspace.sync_draft(window, cx);
        let before = fixture_submission_state(&receipt, cx).1;
        workspace.draft_refused(Some(receipt.clone()), window, cx);
        let (draft, once) = fixture_submission_state(&receipt, cx);
        assert!(once > before);
        assert_eq!(draft.text, "same text");
        assert_eq!(
            draft.files.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(),
            ["newer.txt", "original.txt"]
        );
        workspace.draft_refused(Some(receipt.clone()), window, cx);
        let (draft, twice) = fixture_submission_state(&receipt, cx);
        assert_eq!(once, twice);
        assert_eq!(draft.text, "same text");
        assert_eq!(draft.files.len(), 2);
    });
}
#[gpui_kit::test]
fn token_swap_before_snapshot_cannot_recombine_old_account_and_new_credential_scope(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let receipt = prepare(&workspace, cx, "old scope");
    probe.epoch.fetch_add(1, Ordering::SeqCst);
    *probe.marker.lock().expect("marker") = Some("fixture-identity-b".into());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.sync_draft(window, cx);
        let frozen = workspace
            .gateway_draft_submission(cx)
            .expect("previous synchronized source");
        workspace.draft_submitted(Some(frozen.clone()), window, cx);
        workspace.draft_refused(Some(frozen), window, cx);
    });
    assert_original(&workspace, cx, &receipt, "old scope");
}
#[gpui_kit::test]
fn signed_in_before_new_account_does_not_mint_mixed_draft_scope(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let receipt = prepare(&workspace, cx, "old account draft");
    *probe.marker.lock().expect("marker") = Some("fixture-identity-b".into());
    probe.epoch.fetch_add(1, Ordering::SeqCst);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.discard_private_terminals(cx);
        assert!(workspace.gateway_account_authority.is_none());
        workspace.sync_draft(window, cx);
        workspace.submit_now(false, window, cx);
    });
    assert_original(&workspace, cx, &receipt, "old account draft");
    assert!(probe.asks().is_empty());
}
#[gpui_kit::test]
fn actual_edit_dispatch_refusal_restores_old_scope_beneath_newer_text_and_files(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    workspace.update_in(cx, |workspace, window, cx| workspace.open(&chat("a"), window, cx));
    let _current = prepare(&workspace, cx, "newer text");
    let payload = Draft {
        files: vec![file(30, "edited.txt")],
        ..Default::default()
    };
    let receipt = workspace.read_with(cx, |workspace, _| {
        workspace
            .gateway_payload_submission("edit payload", &payload)
            .expect("frozen edit source")
    });
    probe.revoke_next_dispatch();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.give_back(true, "edit payload".into(), payload, None, window, cx)
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, cx| {
        assert!(workspace.asking.is_none());
        assert!(workspace.edit_sent.is_none());
        let (draft, _) = fixture_submission_state(&receipt, cx);
        assert_eq!(draft.text, "newer text\n\nedit payload");
        assert_eq!(
            draft.files.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(),
            ["original.txt", "edited.txt"]
        );
        assert_eq!(fixture_scopes(cx), 1);
    });
    assert!(probe.asks().is_empty());
}
#[gpui_kit::test]
fn same_owner_queued_admission_refusal_keeps_exact_item_held_without_sent_receipt(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.enqueue_chat(
            "C".into(),
            Draft {
                files: vec![file(40, "queued.txt")],
                ..Default::default()
            },
            false,
            window,
            cx,
        );
        let item = workspace
            .chat_queue
            .queues
            .update("a", |queue| queue.take())
            .expect("taken explicit send");
        let seq = item.seq;
        workspace.commit_gateway_queues(cx);
        // A busy execution can deny an already-taken stale UI action; its item remains in this exact owner.
        workspace.send_queued(item, cx);
        let queue = workspace.chat_queue.queues.get("a").expect("held original item");
        assert!(queue.held());
        assert_eq!(queue.items()[0].seq, seq);
        assert_eq!(queue.items()[0].text, "C");
        assert_eq!(queue.items()[0].payload.files[0].name, "queued.txt");
        assert!(workspace.chat_queue.sent.is_none());
    });
    assert_eq!(probe.asks().len(), 1);
}

#[gpui_kit::test]
fn actual_send_now_epoch_refusal_discards_private_queue_but_keeps_original_scoped_payload(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    let receipt = prepare(&workspace, cx, "explicit Send now");
    cx.run_until_parked();
    probe.revoke_next_dispatch();
    workspace.update_in(cx, |workspace, window, cx| workspace.submit_now(true, window, cx));
    cx.run_until_parked();
    assert_eq!(probe.asks().len(), 1);
    assert_original(&workspace, cx, &receipt, "explicit Send now");
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.asking.is_none());
        assert!(workspace.chat_queue.sent.is_none());
        assert!(workspace.chat_queue.queues.get("a").is_none());
        let binding = workspace.chat_execution.as_ref().expect("sticky revoked owner");
        let state = binding.state.borrow();
        assert!(state.revoked);
        assert!(state.runs.is_empty());
        assert!(state.queues.get("a").is_none());
    });
}
