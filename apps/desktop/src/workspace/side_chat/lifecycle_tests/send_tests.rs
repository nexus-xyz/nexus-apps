//! The side chat sends as the session composer does: the composer clears at once, messages leave in order, Ctrl+Enter
//! queues, and a refused message comes back ahead of what was typed since.
use super::super::SideChat;
use super::{Rpc, attachment, summary};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use serde_json::json;

fn chat<'a>(cx: &'a mut TestAppContext, rpc: &Rpc) -> (Entity<SideChat>, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.add_window_view(|window, cx| {
        SideChat::new(
            summary("send-side", "parent"),
            "Parent".into(),
            rpc.runs("machine"),
            window,
            cx,
        )
    })
}

fn type_and_send(chat: &Entity<SideChat>, cx: &mut VisualTestContext, text: &str, queued: bool) {
    chat.update_in(cx, |chat, window, cx| {
        chat.input.update(cx, |input, cx| input.set_value(text, window, cx));
        chat.send(queued, window, cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_sent_message_leaves_the_composer_at_once_and_the_next_waits_its_turn(cx: &mut TestAppContext) {
    let rpc = Rpc::default();
    let (chat, cx) = chat(cx, &rpc);
    chat.update(cx, |chat, _| chat.draft.files.push(attachment(7, 5)));
    type_and_send(&chat, cx, "first", false);
    chat.read_with(cx, |chat, cx| {
        assert_eq!(
            chat.input.read(cx).value().as_ref(),
            "",
            "the text left before the answer"
        );
        assert!(chat.draft.is_empty(), "the files left with it");
    });
    // The next message is typed and sent while the first is in flight: it waits in line.
    type_and_send(&chat, cx, "second", true);
    assert_eq!(rpc.count("wb_send_files"), 1);
    chat.read_with(cx, |chat, cx| assert_eq!(chat.input.read(cx).value().as_ref(), ""));
    let first = rpc.take("wb_send_files");
    assert_eq!(first.args["text"], "first");
    assert_eq!(first.args["files"].as_array().unwrap().len(), 1);
    assert!(first.args.get("queued").is_none(), "Enter joins the turn");
    first.answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    let second = rpc.take("wb_send_files");
    assert_eq!(second.args["text"], "second");
    assert_eq!(second.args["queued"], true, "Ctrl+Enter queues behind the turn");
    second.answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| {
        assert!(!chat.sending);
        assert!(chat.outbox.is_empty());
        assert_eq!(chat.problem, None);
    });
}

/// The sent message shows in the history before any read; a landed send reads at once, and the read's own message
/// takes the bubble's place.
#[gpui_kit::test]
fn a_sent_message_shows_at_once_and_the_landed_send_reads_it(cx: &mut TestAppContext) {
    use nexus_workbench::event::{StoredEvent, WbEvent};
    let rpc = Rpc::default();
    let (chat, cx) = chat(cx, &rpc);
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    rpc.poll("send-side", "parent");
    cx.run_until_parked();
    type_and_send(&chat, cx, "show me now", false);
    chat.read_with(cx, |chat, _| assert_eq!(chat.echo_texts(), ["show me now"]));
    rpc.take("wb_send_files").answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    // No two-second wait: the landed send woke the read.
    let user = StoredEvent {
        thread_id: "send-side".into(),
        seq: 1,
        at: 110,
        event: WbEvent::User {
            text: "show me now".into(),
            from: None,
            images: vec![],
        },
    };
    rpc.take("wb_thread_events")
        .answer
        .send(Ok(serde_json::to_value(vec![user]).unwrap()))
        .unwrap();
    rpc.take("wb_overview")
        .answer
        .send(Ok(json!({"threads":[summary("send-side","parent")]})))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| {
        assert!(chat.echo.is_empty(), "the stored message took the bubble's place");
        assert!(
            chat.blocks
                .iter()
                .any(|block| matches!(block.inner(), crate::code::Block::User { .. }))
        );
    });
}

/// A used-up budget holds the send in the composer, as the session's; the raise sends it, queued as it was pressed.
#[gpui_kit::test]
fn a_used_up_budget_holds_the_send_until_it_is_raised(cx: &mut TestAppContext) {
    let rpc = Rpc::default();
    let (chat, cx) = chat(cx, &rpc);
    chat.update_in(cx, |chat, window, cx| {
        chat.set_budget(Some("The budget is used up.".into()), window, cx)
    });
    type_and_send(&chat, cx, "held for the budget", true);
    assert_eq!(rpc.count("wb_send_files"), 0);
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), "held for the budget");
        assert_eq!(
            chat.budget_note().as_deref(),
            Some("The budget is used up. Your message sends once it is raised.")
        );
    });
    chat.update_in(cx, |chat, window, cx| chat.set_budget(None, window, cx));
    cx.run_until_parked();
    let send = rpc.take("wb_send_files");
    assert_eq!(send.args["text"], "held for the budget");
    assert_eq!(send.args["queued"], true);
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), "");
        assert_eq!(chat.budget_note(), None);
    });
}

/// A queued message dragged onto another moves there through the engine, as in the session's queue.
#[gpui_kit::test]
fn a_queued_message_dropped_on_another_moves_there(cx: &mut TestAppContext) {
    use crate::code::Block;
    use crate::workspace::queue_drag::QueuedDrag;
    let rpc = Rpc::default();
    let (chat, cx) = chat(cx, &rpc);
    chat.update_in(cx, |chat, window, cx| {
        chat.blocks = vec![
            Block::Queued {
                seq: 2,
                text: "first".into(),
            },
            Block::Queued {
                seq: 3,
                text: "second".into(),
            },
            Block::Queued {
                seq: 4,
                text: "third".into(),
            },
        ];
        let palette =
            crate::palette::Palette::for_choice(crate::palette::ThemeChoice::Dark, gpui_kit::WindowAppearance::Dark);
        // From another queue: nothing moves.
        let other = QueuedDrag::new("another".into(), 2, "first".into(), palette);
        chat.drop_queued(&other, 4, window, cx);
        let drag = QueuedDrag::new("send-side".into(), 2, "first".into(), palette);
        chat.drop_queued(&drag, 3, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(rpc.count("wb_queue_move"), 1);
    let moved = rpc.take("wb_queue_move");
    assert_eq!(moved.args["seq"], 2);
    assert_eq!(moved.args["before"], 4, "dragged down, it lands after its target");
}

/// A reply to a passage stacks over the composer, and the next send quotes each passage over its reply, then the
/// composer's text, as a session's stacked replies.
#[gpui_kit::test]
fn stacked_replies_go_with_the_next_send(cx: &mut TestAppContext) {
    let rpc = Rpc::default();
    let (chat, cx) = chat(cx, &rpc);
    chat.update_in(cx, |chat, window, cx| {
        chat.replies.selected = Some("the passage".into());
        chat.reply_to_selection(window, cx);
        assert_eq!(chat.replies.composing.as_deref(), Some("the passage"));
        chat.replies
            .input
            .update(cx, |input, cx| input.set_value("my reply", window, cx));
        chat.add_reply(window, cx);
        assert!(chat.replies.composing.is_none());
        assert_eq!(chat.replies.stack.len(), 1);
    });
    let expected = chat.read_with(cx, |chat, _| {
        crate::workspace::stacked_replies::message(&chat.replies.stack, "and this")
    });
    assert!(expected.starts_with("> the passage"), "{expected}");
    type_and_send(&chat, cx, "and this", false);
    let send = rpc.take("wb_send_files");
    assert_eq!(send.args["text"], expected.as_str());
    chat.read_with(cx, |chat, _| {
        assert!(chat.replies.stack.is_empty(), "the stack went with it")
    });
}

/// Rewind puts a sent message back in the composer; Edit rewinds to before it and sends the rewrite, as on a
/// session's message.
#[gpui_kit::test]
fn rewind_and_edit_act_on_a_sent_side_message(cx: &mut TestAppContext) {
    use crate::code::Block;
    let rpc = Rpc::default();
    let (chat, cx) = chat(cx, &rpc);
    chat.update_in(cx, |chat, window, cx| {
        chat.blocks = vec![Block::User {
            seq: 5,
            at: 110,
            text: "old question".into(),
            from: None,
            rewound: false,
            images: 0,
            image_data: vec![],
        }];
        chat.ask_rewind(5, cx);
        chat.confirm_rewind(window, cx);
    });
    cx.run_until_parked();
    let rewind = rpc.take("wb_rewind_message");
    assert_eq!(rewind.args["seq"], 5);
    rewind.answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, cx| {
        assert!(chat.rewinding.is_none());
        assert_eq!(chat.input.read(cx).value().as_ref(), "old question");
    });
    chat.update_in(cx, |chat, window, cx| {
        chat.input.update(cx, |input, cx| input.set_value("", window, cx));
        chat.start_edit(5, window, cx);
        chat.editing
            .as_ref()
            .unwrap()
            .input
            .update(cx, |input, cx| input.set_value("new question", window, cx));
        chat.send_edit(window, cx);
    });
    cx.run_until_parked();
    rpc.take("wb_rewind_message").answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    let send = rpc.take("wb_send");
    assert_eq!(send.args["text"], "new question");
    send.answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, cx| {
        assert!(chat.editing.is_none());
        assert_eq!(chat.input.read(cx).value().as_ref(), "", "the composer is left alone");
    });
}

#[gpui_kit::test]
fn a_refused_message_comes_back_ahead_of_what_was_typed_since(cx: &mut TestAppContext) {
    let rpc = Rpc::default();
    let (chat, cx) = chat(cx, &rpc);
    chat.update(cx, |chat, _| chat.draft.files.push(attachment(7, 5)));
    type_and_send(&chat, cx, "refused", false);
    chat.update_in(cx, |chat, window, cx| {
        chat.input
            .update(cx, |input, cx| input.set_value("typed since", window, cx));
        chat.draft.files.push(attachment(8, 5));
    });
    rpc.take("wb_send_files")
        .answer
        .send(Err("The host refused it.".into()))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), "refused\n\ntyped since");
        assert_eq!(chat.draft.files.iter().map(|file| file.id).collect::<Vec<_>>(), [7, 8]);
        assert!(chat.echo.is_empty(), "a refused message leaves the history");
        assert_eq!(
            chat.problem.as_deref(),
            Some("The host refused it. The message was not sent; it is back in the composer.")
        );
    });
}
