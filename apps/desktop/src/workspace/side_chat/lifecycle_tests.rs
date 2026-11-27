//! Real deferred Workbench replies cross native-window destruction without retaining UI entities.
use super::{SideChat, Workspace};
use crate::workspace::{View, code_view::CodeView, runs::Runs};
use futures::channel::oneshot;
use gpui_kit::{AppContext, Context, Task, TestAppContext, Window};
use nexus_workbench::event::{StoredEvent, ThreadSummary, WbEvent};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, Mutex},
};

struct Request {
    name: &'static str,
    args: Value,
    answer: oneshot::Sender<Result<Value, String>>,
}
#[derive(Clone, Default)]
struct Rpc {
    pending: Arc<Mutex<Vec<Request>>>,
    issued: Arc<Mutex<Vec<&'static str>>>,
}
impl Rpc {
    fn runs(&self, source: &str) -> Runs {
        self.runs_at(crate::workspace::runs::Source::Machine(source.into()))
    }
    fn runs_at(&self, source: crate::workspace::runs::Source) -> Runs {
        let rpc = self.clone();
        Runs::deferred_source(
            source,
            Arc::new(move |name, args| {
                let (answer, read) = oneshot::channel();
                rpc.issued.lock().unwrap().push(name);
                rpc.pending.lock().unwrap().push(Request { name, args, answer });
                read
            }),
        )
    }
    fn take(&self, name: &str) -> Request {
        let mut pending = self.pending.lock().unwrap();
        let at = pending
            .iter()
            .position(|request| request.name == name)
            .unwrap_or_else(|| panic!("missing deferred {name}"));
        pending.remove(at)
    }
    fn count(&self, name: &str) -> usize {
        self.issued
            .lock()
            .unwrap()
            .iter()
            .filter(|issued| **issued == name)
            .count()
    }
    fn poll(&self, id: &str, parent: &str) {
        self.take("wb_thread_events").answer.send(Ok(json!([]))).unwrap();
        self.take("wb_overview")
            .answer
            .send(Ok(json!({"threads":[summary(id,parent)]})))
            .unwrap();
    }
}
fn summary(id: &str, parent: &str) -> ThreadSummary {
    serde_json::from_value(
        json!({"id":id,"projectId":"p","title":"Side question","agent":"codex","model":null,
        "permissionMode":"default","via":"nexus","status":"idle","costUsd":0.,"nexusSessions":[],
        "createdAt":100,"updatedAt":100,"archived":false,"sideChat":true,"sourceThreadId":parent}),
    )
    .unwrap()
}
fn workspace(
    window: &mut Window,
    cx: &mut Context<Workspace>,
    rpc: &Rpc,
    source: &str,
    valid: Rc<Cell<bool>>,
) -> Workspace {
    let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let mut workspace = Workspace::with_machine_source(
        Arc::new(gateway),
        events,
        arrivals,
        false,
        crate::workspace::machine_source::MachineSource::isolated(),
        window,
        cx,
    );
    workspace._code_poll = Task::ready(());
    workspace.auth = crate::workspace::Auth::SignedIn;
    workspace.side.side_chat_fixture_runs = Some(rpc.runs(source));
    workspace.side.side_chat_fixture_guard = Some(Rc::new(move || valid.get()));
    workspace.view = View::Code(CodeView::new("parent".into(), window, cx));
    workspace
}

#[gpui_kit::test]
fn side_chat_pending_send_success_and_failure_survive_a_destroyed_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let old = Rpc::default();
    let fresh = Rpc::default();
    let (saved, old_id, write) = {
        let (chat, cx) = cx.add_window_view(|window, cx| {
            SideChat::new(
                summary("side", "parent"),
                "Parent".into(),
                old.runs("machine"),
                window,
                cx,
            )
        });
        chat.update_in(cx, |chat, window, cx| {
            chat.input
                .update(cx, |input, cx| input.set_value("submitted A", window, cx))
        });
        cx.run_until_parked();
        chat.update_in(cx, |chat, window, cx| chat.send(false, window, cx));
        cx.run_until_parked();
        let write = old.take("wb_send_files");
        assert_eq!(write.args["text"], "submitted A");
        let old_id = chat.entity_id();
        let saved = chat.update_in(cx, |chat, window, cx| {
            let saved = chat.retain(cx);
            chat.send(false, window, cx);
            window.remove_window();
            saved
        });
        assert_eq!(old.count("wb_send_files"), 1);
        (saved, old_id, write)
    };
    write.answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, fresh.runs("machine"), window, cx));
    cx.run_until_parked();
    assert_ne!(chat.entity_id(), old_id);
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), "");
        assert!(!chat.sending);
    });
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    fresh.poll("side", "parent");
    cx.run_until_parked();
    chat.update_in(cx, |chat, window, cx| {
        chat.input
            .update(cx, |input, cx| input.set_value("retry B", window, cx))
    });
    cx.run_until_parked();
    chat.update_in(cx, |chat, window, cx| chat.send(false, window, cx));
    cx.run_until_parked();
    fresh
        .take("wb_send_files")
        .answer
        .send(Err("fixture rejected".into()))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), "retry B");
        assert!(!chat.sending);
        assert_eq!(
            chat.problem.as_deref(),
            Some("fixture rejected The message was not sent; it is back in the composer.")
        );
    });
    assert_eq!(old.count("wb_send_files"), 1);
    assert_eq!(fresh.count("wb_send_files"), 1);
}

#[gpui_kit::test]
fn side_chat_pending_submission_never_erases_a_later_draft(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let (saved, write) = {
        let (chat, cx) = cx.add_window_view(|window, cx| {
            SideChat::new(
                summary("side", "parent"),
                "Parent".into(),
                rpc.runs("machine"),
                window,
                cx,
            )
        });
        chat.update_in(cx, |chat, window, cx| {
            chat.input.update(cx, |input, cx| input.set_value("A", window, cx))
        });
        cx.run_until_parked();
        chat.update_in(cx, |chat, window, cx| chat.send(false, window, cx));
        cx.run_until_parked();
        let write = rpc.take("wb_send_files");
        chat.update_in(cx, |chat, window, cx| {
            chat.input.update(cx, |input, cx| input.set_value("new B", window, cx))
        });
        cx.run_until_parked();
        let saved = chat.update_in(cx, |chat, window, cx| {
            let saved = chat.retain(cx);
            window.remove_window();
            saved
        });
        (saved, write)
    };
    write.answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("machine"), window, cx));
    cx.run_until_parked();
    chat.read_with(cx, |chat, cx| assert_eq!(chat.input.read(cx).value().as_ref(), "new B"));
}

#[gpui_kit::test]
fn side_chat_create_and_first_send_finish_while_parked_and_bind_fresh_runs(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    cx.update(gpui_kit::init);
    let old = Rpc::default();
    let fresh = Rpc::default();
    let valid = Rc::new(Cell::new(true));
    let saved = {
        let (workspace, cx) = cx.add_window_view(|window, cx| workspace(window, cx, &old, "machine", valid.clone()));
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.create_side_chat(Some("original question".into()), window, cx)
        });
        cx.run_until_parked();
        let create = old.take("wb_side_chat");
        assert_eq!(create.args["threadId"], "parent");
        let saved = workspace.update_in(cx, |workspace, window, cx| {
            let saved = workspace.retain_private_terminals(cx);
            window.remove_window();
            saved
        });
        create
            .answer
            .send(Ok(serde_json::to_value(summary("created", "parent")).unwrap()))
            .unwrap();
        cx.run_until_parked();
        let send = old.take("wb_send_files");
        assert_eq!(send.args["threadId"], "created");
        assert_eq!(send.args["text"], "original question");
        send.answer.send(Ok(json!(true))).unwrap();
        cx.run_until_parked();
        saved
    };
    let (workspace, cx) = cx.add_window_view(|window, cx| workspace(window, cx, &fresh, "machine", valid.clone()));
    workspace.update_in(cx, |workspace, window, cx| {
        {
            workspace.restore_private_terminals(saved, window, cx);
            // Completed parked receipt has not been consumed by the new UI watcher yet.
            workspace.create_side_chat(Some("must not start twice".into()), window, cx);
        }
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(
            workspace.side.side_chat_for.get("parent").map(String::as_str),
            Some("created")
        );
        assert!(!workspace.side.side_chat_busy);
        assert!(workspace.side.side_chats.contains_key("created"));
    });
    assert_eq!(old.count("wb_side_chat"), 1);
    assert_eq!(old.count("wb_send_files"), 1);
    assert_eq!(fresh.count("wb_side_chat"), 0);
    assert_eq!(fresh.count("wb_send_files"), 0);
    fresh.poll("created", "parent");
    cx.run_until_parked();
    let chat = workspace.read_with(cx, |workspace, _| workspace.open_side_chat().unwrap());
    chat.read_with(cx, |chat, cx| {
        assert!(chat.ready);
        assert!(!chat.allow_partial_stream);
        assert_eq!(chat.input.read(cx).value().as_ref(), "");
    });
}

#[gpui_kit::test]
fn side_chat_original_source_waits_then_reattaches_and_identity_revoke_is_eager(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let valid = Rc::new(Cell::new(true));
    let (saved, old_id) = {
        let (workspace, cx) = cx.add_window_view(|window, cx| workspace(window, cx, &rpc, "machine", valid.clone()));
        workspace.update_in(cx, |workspace, window, cx| {
            let chat = cx.new(|cx| {
                SideChat::new(
                    summary("side", "parent"),
                    "Parent".into(),
                    rpc.runs("machine"),
                    window,
                    cx,
                )
            });
            chat.update(cx, |chat, cx| {
                chat.input
                    .update(cx, |input, cx| input.set_value("kept draft", window, cx));
                chat.history.open.insert("fold".into());
                chat.draft.files.push(crate::attachments::Attachment {
                    artifact: None,
                    id: 9,
                    name: "notes.txt".into(),
                    bytes: Arc::new(b"kept bytes".to_vec()),
                    media_type: None,
                    thumbnail: None,
                });
            });
            let id = chat.entity_id();
            workspace.bind_side_chat("parent".into(), "side".into(), chat, window, cx);
            workspace.side.side_chat_visible = true;
            let saved = workspace.retain_private_terminals(cx);
            window.remove_window();
            (saved, id)
        })
    };
    let (workspace, cx) =
        cx.add_window_view(|window, cx| workspace(window, cx, &rpc, "different-machine", valid.clone()));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.restore_private_terminals(saved, window, cx);
        assert!(workspace.open_side_chat().is_none());
        assert!(workspace.waiting_side_chat().is_some());
        assert_eq!(workspace.waiting_side_chat().unwrap().preview().1, "kept draft");
        workspace.side.side_chat_fixture_runs = Some(rpc.runs("machine"));
        workspace.adopt_side_chats(window, cx);
        let chat = workspace.open_side_chat().unwrap();
        assert_ne!(chat.entity_id(), old_id);
        chat.read_with(cx, |chat, cx| {
            assert_eq!(chat.input.read(cx).value().as_ref(), "kept draft");
            assert_eq!(chat.draft.files[0].bytes.as_slice(), b"kept bytes");
            assert!(chat.history.open.contains("fold"));
        });
        valid.set(false);
        workspace.handle(
            crate::gateway::Event::SignInFailed("fixture revoked".into()),
            window,
            cx,
        );
        assert!(workspace.side.side_chats.is_empty() && workspace.side.side_chat_waiting.is_empty());
        chat.update(cx, |chat, cx| {
            chat.command("wb_send_back", json!({"threadId":"side"}), false, None, window, cx)
        });
        let saved = workspace.retain_private_terminals(cx);
        drop(saved);
        assert!(workspace.side.side_chat_for.is_empty());
    });
    assert_eq!(rpc.count("wb_send_back"), 0);
    assert_eq!(rpc.count("wb_send_files"), 0);
}

#[gpui_kit::test]
fn side_chat_revocation_cancels_a_parked_creation_before_first_send(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let valid = Rc::new(Cell::new(true));
    let (saved, create) = {
        let (workspace, cx) = cx.add_window_view(|window, cx| workspace(window, cx, &rpc, "machine", valid.clone()));
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.create_side_chat(Some("question".into()), window, cx)
        });
        cx.run_until_parked();
        let create = rpc.take("wb_side_chat");
        let saved = workspace.update_in(cx, |workspace, window, cx| {
            let saved = workspace.retain_private_terminals(cx);
            window.remove_window();
            saved
        });
        (saved, create)
    };
    valid.set(false);
    create
        .answer
        .send(Ok(serde_json::to_value(summary("side", "parent")).unwrap()))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(rpc.count("wb_send_files"), 0);
    drop(saved);
}

#[gpui_kit::test]
fn side_chat_bring_back_receipt_is_applied_once_after_window_replacement(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let (saved, reply) = {
        let (chat, cx) = cx.add_window_view(|window, cx| {
            SideChat::new(
                summary("side", "parent"),
                "Parent".into(),
                rpc.runs("machine"),
                window,
                cx,
            )
        });
        chat.update_in(cx, |chat, window, cx| {
            chat.command(
                "wb_send_back",
                json!({"threadId":"side"}),
                false,
                Some("queued"),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        let reply = rpc.take("wb_send_back");
        let saved = chat.update_in(cx, |chat, window, cx| {
            let saved = chat.retain(cx);
            window.remove_window();
            saved
        });
        (saved, reply)
    };
    reply.answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("machine"), window, cx));
    cx.run_until_parked();
    assert!(chat.read_with(cx, |chat, _| chat.brought_back && !chat.sending));
    chat.update_in(cx, |chat, window, cx| chat.apply_commands(window, cx));
    assert_eq!(rpc.count("wb_send_back"), 1);
}

#[gpui_kit::test]
fn side_chat_close_receipt_reaches_the_replacement_workspace_subscription(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let valid = Rc::new(Cell::new(true));
    let (saved, reply) = {
        let (workspace, cx) = cx.add_window_view(|window, cx| workspace(window, cx, &rpc, "machine", valid.clone()));
        let chat = workspace.update_in(cx, |workspace, window, cx| {
            let chat = cx.new(|cx| {
                SideChat::new(
                    summary("side", "parent"),
                    "Parent".into(),
                    rpc.runs("machine"),
                    window,
                    cx,
                )
            });
            workspace.bind_side_chat("parent".into(), "side".into(), chat.clone(), window, cx);
            chat.update(cx, |chat, cx| chat.close(window, cx));
            chat
        });
        cx.run_until_parked();
        let reply = rpc.take("wb_side_chat_close");
        let saved = workspace.update_in(cx, |workspace, window, cx| {
            let saved = workspace.retain_private_terminals(cx);
            window.remove_window();
            saved
        });
        drop(chat);
        (saved, reply)
    };
    reply.answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    let (workspace, cx) = cx.add_window_view(|window, cx| workspace(window, cx, &rpc, "machine", valid.clone()));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.view = View::Code(CodeView::new("other-parent".into(), window, cx));
        workspace.restore_private_terminals(saved, window, cx);
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.side.side_chat_for.is_empty());
        assert!(workspace.side.side_chats.is_empty());
    });
    assert_eq!(rpc.count("wb_side_chat_close"), 1);
}

#[gpui_kit::test]
fn side_chat_old_poll_is_cancelled_and_wrong_thread_events_never_become_ready(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let old = Rpc::default();
    let fresh = Rpc::default();
    let (saved, old_events, old_overview) = {
        let (chat, cx) = cx.add_window_view(|window, cx| {
            SideChat::new(
                summary("side", "parent"),
                "Parent".into(),
                old.runs("machine"),
                window,
                cx,
            )
        });
        chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
        cx.run_until_parked();
        let events = old.take("wb_thread_events");
        let overview = old.take("wb_overview");
        let saved = chat.update_in(cx, |chat, window, cx| {
            let saved = chat.retain(cx);
            window.remove_window();
            saved
        });
        (saved, events, overview)
    };
    // GPUI processes task cancellation on its foreground executor.
    cx.run_until_parked();
    assert!(old_events.answer.send(Ok(json!([]))).is_err());
    assert!(old_overview.answer.send(Ok(json!({"threads":[]}))).is_err());
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, fresh.runs("machine"), window, cx));
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    fresh
        .take("wb_thread_events")
        .answer
        .send(Ok(serde_json::to_value(vec![StoredEvent {
            thread_id: "other".into(),
            seq: 1,
            at: 110,
            event: WbEvent::Text {
                id: "wrong".into(),
                text: "never show this".into(),
                parent: None,
            },
        }])
        .unwrap()))
        .unwrap();
    fresh
        .take("wb_overview")
        .answer
        .send(Ok(json!({"threads":[summary("side","parent")]})))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| {
        assert!(!chat.ready);
        assert!(chat.events.is_empty());
    });
    chat.update_in(cx, |chat, window, cx| {
        chat.active(false, window, cx);
        chat.active(true, window, cx);
    });
    cx.run_until_parked();
    fresh.poll("side", "parent");
    cx.run_until_parked();
    assert!(chat.read_with(cx, |chat, _| chat.ready));
}

#[gpui_kit::test]
fn side_chat_restored_source_never_trusts_same_id_global_partial_text(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let id = "side-retained-stream-provenance-fixture";
    crate::stream::delta(&nexus_workbench::event::Delta {
        thread_id: id.into(),
        id: "old-source".into(),
        text: "unproven old account partial".into(),
        parent: None,
        offset: None,
    });
    let saved = {
        let (chat, cx) = cx.add_window_view(|window, cx| {
            SideChat::new(
                summary(id, "parent"),
                "Parent".into(),
                rpc.runs("original-machine"),
                window,
                cx,
            )
        });
        chat.update_in(cx, |chat, window, cx| {
            let saved = chat.retain(cx);
            window.remove_window();
            saved
        })
    };
    let (chat, cx) =
        cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("original-machine"), window, cx));
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    rpc.poll(id, "parent");
    cx.run_until_parked();
    chat.update(cx, |chat, cx| {
        assert!(chat.ready);
        chat.sync_stream(cx);
        assert!(chat.streamed.is_none());
        assert!(!chat.allow_partial_stream);
    });
    // Current source-qualified durable text still renders and enables useful commands.
    chat.update_in(cx, |chat, window, cx| {
        chat.active(false, window, cx);
        chat.active(true, window, cx);
    });
    cx.run_until_parked();
    let event = StoredEvent {
        thread_id: id.into(),
        seq: 1,
        at: 110,
        event: WbEvent::Text {
            id: "current".into(),
            text: "current source answer".into(),
            parent: None,
        },
    };
    rpc.take("wb_thread_events")
        .answer
        .send(Ok(serde_json::to_value(vec![event]).unwrap()))
        .unwrap();
    rpc.take("wb_overview")
        .answer
        .send(Ok(json!({"threads":[summary(id,"parent")]})))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| {
        assert!(chat.can_command());
        assert!(matches!(&chat.blocks[..],[crate::code::Block::Text(text)] if text=="current source answer"));
    });
    chat.update_in(cx, |chat, window, cx| {
        chat.active(false, window, cx);
        chat.active(true, window, cx);
    });
    cx.run_until_parked();
    let mut running = summary(id, "parent");
    running.status = nexus_workbench::event::ThreadStatus::Running;
    rpc.take("wb_thread_events").answer.send(Ok(json!([]))).unwrap();
    rpc.take("wb_overview")
        .answer
        .send(Ok(json!({"threads":[running]})))
        .unwrap();
    cx.run_until_parked();
    chat.update_in(cx, |chat, window, cx| {
        assert!(chat.summary.status.busy());
        chat.command("wb_interrupt", json!({"threadId":id}), false, None, window, cx);
    });
    cx.run_until_parked();
    rpc.take("wb_interrupt").answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    assert_eq!(rpc.count("wb_interrupt"), 1);
    crate::stream::event(
        id,
        &WbEvent::Text {
            id: "old-source".into(),
            text: String::new(),
            parent: None,
        },
    );
}

fn attachment(id: u64, size: usize) -> crate::attachments::Attachment {
    crate::attachments::Attachment {
        artifact: None,
        id,
        name: format!("file-{id}.bin"),
        bytes: Arc::new(vec![7; size]),
        media_type: None,
        thumbnail: None,
    }
}
fn chunk_answer(request: Request) {
    let chunk: nexus_workbench::attachments::Chunk = serde_json::from_value(request.args).unwrap();
    let received = (chunk.offset + nexus_workbench::attachments::CHUNK_BYTES as u64).min(chunk.size);
    let progress = nexus_workbench::attachments::Progress {
        received,
        done: received == chunk.size,
    };
    request
        .answer
        .send(Ok(serde_json::to_value(progress).unwrap()))
        .unwrap();
}

#[gpui_kit::test]
fn side_chat_pending_chunk_upload_finishes_parked_and_clears_only_submitted_ids(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let bytes = nexus_workbench::attachments::CHUNK_BYTES + 3;
    let (saved, first) = {
        let (chat, cx) = cx.add_window_view(|window, cx| {
            SideChat::new(
                summary("upload-side", "parent"),
                "Parent".into(),
                rpc.runs("machine"),
                window,
                cx,
            )
        });
        chat.update_in(cx, |chat, window, cx| {
            chat.draft.files.push(attachment(7, bytes));
            chat.input
                .update(cx, |input, cx| input.set_value("upload A", window, cx));
            chat.send(false, window, cx);
        });
        cx.run_until_parked();
        let first = rpc.take("wb_attachment_chunk");
        // A preparation begun earlier may finish after the submitted snapshot was taken.
        chat.update(cx, |chat, _| chat.draft.files.push(attachment(99, 5)));
        let saved = chat.update_in(cx, |chat, window, cx| {
            let saved = chat.retain(cx);
            window.remove_window();
            saved
        });
        (saved, first)
    };
    chunk_answer(first);
    cx.run_until_parked();
    chunk_answer(rpc.take("wb_attachment_chunk"));
    cx.run_until_parked();
    let send = rpc.take("wb_send_files");
    assert_eq!(send.args["files"].as_array().unwrap().len(), 1);
    assert!(send.args["files"][0]["upload"].is_string());
    send.answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("machine"), window, cx));
    cx.run_until_parked();
    chat.read_with(cx, |chat, cx| {
        assert!(!chat.sending);
        assert_eq!(chat.input.read(cx).value().as_ref(), "");
        assert_eq!(chat.draft.files.iter().map(|file| file.id).collect::<Vec<_>>(), [99]);
    });
    assert_eq!(rpc.count("wb_attachment_chunk"), 2);
    assert_eq!(rpc.count("wb_send_files"), 1);
}

/// A large file's chip shows its progress, and its ✕ cancels the send: nothing is sent, what went up is deleted, and
/// the text and the file stay, as in the session composer (`workspace/uploads.rs`).
#[gpui_kit::test]
fn side_chat_upload_shows_progress_and_cancel_keeps_the_message(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let (chat, cx) = cx.add_window_view(|window, cx| {
        SideChat::new(
            summary("upload-cancel", "parent"),
            "Parent".into(),
            rpc.runs("machine"),
            window,
            cx,
        )
    });
    let size = 2 * nexus_workbench::attachments::CHUNK_BYTES + 3;
    chat.update_in(cx, |chat, window, cx| {
        chat.draft.files.push(attachment(7, size));
        chat.input
            .update(cx, |input, cx| input.set_value("keep me", window, cx));
        chat.send(false, window, cx);
    });
    cx.run_until_parked();
    chunk_answer(rpc.take("wb_attachment_chunk"));
    cx.run_until_parked();
    let share = chat.read_with(cx, |chat, _| {
        chat.uploading
            .as_ref()
            .and_then(|draft| draft.upload.as_ref())
            .and_then(|run| run.share(7))
    });
    assert!(
        share.is_some_and(|share| share > 0. && share < 1.),
        "the chip shows progress: {share:?}"
    );
    chat.update(cx, |chat, _| {
        chat.uploading
            .as_ref()
            .and_then(|draft| draft.upload.as_ref())
            .unwrap()
            .cancel(7)
    });
    chunk_answer(rpc.take("wb_attachment_chunk"));
    cx.run_until_parked();
    rpc.take("wb_attachment_cancel").answer.send(Ok(Value::Null)).unwrap();
    cx.run_until_parked();
    assert_eq!(rpc.count("wb_attachment_chunk"), 2);
    assert_eq!(rpc.count("wb_send_files"), 0);
    chat.read_with(cx, |chat, cx| {
        assert!(!chat.sending);
        assert!(chat.uploading.is_none());
        assert_eq!(chat.draft.files.iter().map(|file| file.id).collect::<Vec<_>>(), [7]);
        assert_eq!(chat.input.read(cx).value().as_ref(), "keep me");
        assert_eq!(chat.problem, None);
        assert_eq!(
            chat.note.as_deref(),
            Some(nexus_workbench::attachment_upload::CANCELLED)
        );
    });
}

#[gpui_kit::test]
fn side_chat_revoked_upload_issues_no_later_chunk_or_final_send(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let valid = Rc::new(Cell::new(true));
    let (saved, first) = {
        let (chat, cx) = cx.add_window_view(|window, cx| {
            SideChat::new(
                summary("upload-revoke", "parent"),
                "Parent".into(),
                rpc.runs("machine"),
                window,
                cx,
            )
        });
        chat.update_in(cx, |chat, window, cx| {
            let valid = valid.clone();
            chat.authority(Rc::new(move || valid.get()));
            chat.draft
                .files
                .push(attachment(7, nexus_workbench::attachments::CHUNK_BYTES + 3));
            chat.send(false, window, cx);
        });
        cx.run_until_parked();
        let first = rpc.take("wb_attachment_chunk");
        let saved = chat.update_in(cx, |chat, window, cx| {
            let saved = chat.retain(cx);
            window.remove_window();
            saved
        });
        (saved, first)
    };
    valid.set(false);
    chunk_answer(first);
    cx.run_until_parked();
    assert_eq!(rpc.count("wb_attachment_chunk"), 1);
    assert_eq!(rpc.count("wb_send_files"), 0);
    drop(saved);
}

#[gpui_kit::test]
fn side_chat_pending_queue_rewrite_ack_keeps_newer_edit_after_reopen(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let (saved, write) = {
        let (chat, cx) = cx.add_window_view(|window, cx| {
            SideChat::new(
                summary("queue-side", "parent"),
                "Parent".into(),
                rpc.runs("machine"),
                window,
                cx,
            )
        });
        chat.update_in(cx, |chat, window, cx| {
            chat.edit_queued(42, "original".into(), window, cx);
            chat.rewrite
                .as_ref()
                .unwrap()
                .input
                .update(cx, |input, cx| input.set_value("submitted edit", window, cx));
            chat.save_rewrite(window, cx);
        });
        cx.run_until_parked();
        let write = rpc.take("wb_queue_edit");
        assert_eq!(write.args["seq"], 42);
        assert_eq!(write.args["text"], "submitted edit");
        chat.update_in(cx, |chat, window, cx| {
            chat.rewrite
                .as_ref()
                .unwrap()
                .input
                .update(cx, |input, cx| input.set_value("newer edit", window, cx))
        });
        let saved = chat.update_in(cx, |chat, window, cx| {
            let saved = chat.retain(cx);
            window.remove_window();
            saved
        });
        (saved, write)
    };
    write.answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("machine"), window, cx));
    cx.run_until_parked();
    chat.read_with(cx, |chat, cx| {
        assert!(!chat.sending);
        assert_eq!(
            chat.rewrite.as_ref().unwrap().input.read(cx).value().as_ref(),
            "newer edit"
        );
    });
    for _ in 0..2 {
        chat.update_in(cx, |chat, window, cx| {
            chat.active(false, window, cx);
            chat.active(true, window, cx);
        });
        cx.run_until_parked();
        rpc.poll("queue-side", "parent");
        cx.run_until_parked();
    }
    chat.read_with(cx, |chat, cx| {
        assert!(chat.rewrite.is_none());
        assert_eq!(chat.input.read(cx).value().as_ref(), "newer edit");
    });
    assert_eq!(rpc.count("wb_queue_edit"), 1);
}

#[gpui_kit::test]
fn side_chat_unavailable_source_preview_projects_confirmed_send_without_resending(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let valid = Rc::new(Cell::new(true));
    let (saved, write) = {
        let (workspace, cx) = cx.add_window_view(|window, cx| workspace(window, cx, &rpc, "machine", valid.clone()));
        workspace.update_in(cx, |workspace, window, cx| {
            let chat = cx.new(|cx| {
                SideChat::new(
                    summary("side", "parent"),
                    "Parent".into(),
                    rpc.runs("machine"),
                    window,
                    cx,
                )
            });
            workspace.bind_side_chat("parent".into(), "side".into(), chat.clone(), window, cx);
            workspace.side.side_chat_visible = true;
            chat.update(cx, |chat, cx| {
                chat.input
                    .update(cx, |input, cx| input.set_value("known submitted A", window, cx));
                chat.draft.files.push(attachment(7, 5));
                chat.send(false, window, cx);
            });
        });
        cx.run_until_parked();
        let write = rpc.take("wb_send_files");
        let saved = workspace.update_in(cx, |workspace, window, cx| {
            let saved = workspace.retain_private_terminals(cx);
            window.remove_window();
            saved
        });
        (saved, write)
    };
    write.answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    let (workspace, cx) =
        cx.add_window_view(|window, cx| workspace(window, cx, &rpc, "unavailable-source", valid.clone()));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.restore_private_terminals(saved, window, cx);
        let (_, text, files) = workspace.waiting_side_chat().unwrap().preview();
        assert!(text.is_empty() && files.is_empty());
        workspace.side.side_chat_fixture_runs = Some(rpc.runs("machine"));
        workspace.adopt_side_chats(window, cx);
        let chat = workspace.open_side_chat().unwrap();
        chat.read_with(cx, |chat, cx| {
            assert_eq!(chat.input.read(cx).value().as_ref(), "");
            assert!(chat.draft.is_empty());
        });
    });
    assert_eq!(rpc.count("wb_send_files"), 1);
}

struct NativeRoot(gpui_kit::Entity<SideChat>);
impl gpui_kit::Render for NativeRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui_kit::IntoElement {
        use gpui_kit::{ParentElement, Styled};
        gpui_kit::div().size_full().child(self.0.clone())
    }
}
#[gpui_kit::test]
fn side_chat_native_restored_send_click_uses_fresh_source_and_response_updates_draft(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    // Stop slides in as the turn starts (`toolbar_slot`); this test clicks it in the next frame, so it draws at rest.
    cx.update(|cx| cx.set_reduce_motion(true));
    let rpc = Rpc::default();
    let saved = {
        let (chat, cx) = cx.add_window_view(|window, cx| {
            SideChat::new(
                summary("native-side", "parent"),
                "Parent".into(),
                rpc.runs("machine"),
                window,
                cx,
            )
        });
        chat.update_in(cx, |chat, window, cx| {
            chat.input
                .update(cx, |input, cx| input.set_value("native kept draft", window, cx));
            let saved = chat.retain(cx);
            window.remove_window();
            saved
        })
    };
    let (root, cx) = cx.add_window_view(|window, cx| {
        NativeRoot(cx.new(|cx| SideChat::restore(saved, rpc.runs("machine"), window, cx)))
    });
    let chat = root.read_with(cx, |root, _| root.0.clone());
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    rpc.poll("native-side", "parent");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let send = cx.debug_bounds("side-send").expect("restored native send is drawn");
    let composer = cx
        .debug_bounds("side-chat-composer")
        .expect("restored composer is drawn");
    assert!(send.top() >= composer.top() && send.bottom() <= composer.bottom());
    cx.simulate_click(send.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    let write = rpc.take("wb_send_files");
    assert_eq!(write.args["text"], "native kept draft");
    assert_eq!(write.args["threadId"], "native-side");
    write.answer.send(Ok(json!(true))).unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, cx| assert_eq!(chat.input.read(cx).value().as_ref(), ""));
    chat.update_in(cx, |chat, window, cx| {
        chat.active(false, window, cx);
        chat.active(true, window, cx);
    });
    cx.run_until_parked();
    let mut running = summary("native-side", "parent");
    running.status = nexus_workbench::event::ThreadStatus::Running;
    rpc.take("wb_thread_events").answer.send(Ok(json!([]))).unwrap();
    rpc.take("wb_overview")
        .answer
        .send(Ok(json!({"threads":[running]})))
        .unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let stop = cx
        .debug_bounds("side-stop")
        .expect("source-validated running chat has native Stop");
    cx.simulate_click(stop.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    let interrupted = rpc.take("wb_interrupt");
    assert_eq!(interrupted.args["threadId"], "native-side");
    interrupted
        .answer
        .send(Ok(serde_json::to_value(summary("native-side", "parent")).unwrap()))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(rpc.count("wb_interrupt"), 1);
    assert!(chat.read_with(cx, |chat, _| !chat.sending && !chat.summary.status.busy()));
}

fn approval(id: &str, seq: i64, request: &str) -> StoredEvent {
    StoredEvent {
        thread_id: id.into(),
        seq,
        at: 110 + seq,
        event: WbEvent::Approval {
            request_id: request.into(),
            tool_use_id: None,
            tool: crate::workspace::code_dock::QUESTION_TOOL.into(),
            input: json!({"questions":[{"id":"normal","question":"Choose a color"},{"id":"masked","question":"Private answer","isSecret":true}]}),
            description: None,
            suggestions: vec![],
            parent: None,
        },
    }
}
#[gpui_kit::test]
fn side_chat_reopened_approval_keeps_normal_answers_but_never_secret_fields(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let (saved, old) = {
        let (chat, cx) = cx.add_window_view(|window, cx| {
            SideChat::new(
                summary("approval-side", "parent"),
                "Parent".into(),
                rpc.runs("machine"),
                window,
                cx,
            )
        });
        let saved = chat.update_in(cx, |chat, window, cx| {
            chat.events = vec![approval("approval-side", 1, "request-one")];
            chat.blocks = super::own_blocks(&chat.events, 100);
            chat.follow_request(window, cx);
            chat.answers[0].update(cx, |input, cx| input.set_value("normal kept", window, cx));
            chat.answers[1].update(cx, |input, cx| input.set_value("discard masked answer", window, cx));
            chat.secret
                .update(cx, |input, cx| input.set_value("discard masked entry", window, cx));
            let saved = chat.retain(cx);
            window.remove_window();
            saved
        });
        (saved, chat)
    };
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("machine"), window, cx));
    cx.run_until_parked();
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.answers[0].read(cx).value().as_ref(), "normal kept");
        assert_eq!(chat.answers[1].read(cx).value().as_ref(), "");
        assert_eq!(chat.secret.read(cx).value().as_ref(), "");
    });
    chat.update_in(cx, |_, window, cx| {
        old.update(cx, |old, cx| {
            assert!(!old.can_command());
            old.answer("allow", window, cx);
            old.answer_secret(true, window, cx);
        })
    });
    assert_eq!(rpc.count("wb_approve"), 0);
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    let resolved = StoredEvent {
        thread_id: "approval-side".into(),
        seq: 2,
        at: 112,
        event: WbEvent::ApprovalResolved {
            request_id: "request-one".into(),
            decision: nexus_workbench::event::Decision::Deny,
            message: None,
        },
    };
    rpc.take("wb_thread_events")
        .answer
        .send(Ok(serde_json::to_value(vec![
            resolved,
            approval("approval-side", 3, "request-two"),
        ])
        .unwrap()))
        .unwrap();
    rpc.take("wb_overview")
        .answer
        .send(Ok(json!({"threads":[summary("approval-side","parent")]})))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.request.as_deref(), Some("request-two"));
        assert!(chat.answers.iter().all(|input| input.read(cx).value().is_empty()));
    });
}

#[gpui_kit::test]
fn side_chat_local_store_locator_requires_matching_data_path_and_scope(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let valid = Rc::new(Cell::new(true));
    let source = |data: &str, scope: &str| crate::workspace::runs::Source::Local {
        data: data.into(),
        scope: scope.into(),
    };
    let original = source("/fixture/store-a", "gateway|org");
    let (saved, old_id) = {
        let (view, cx) = cx.add_window_view(|window, cx| workspace(window, cx, &rpc, "unused", valid.clone()));
        view.update_in(cx, |workspace, window, cx| {
            workspace.side.side_chat_fixture_runs = Some(rpc.runs_at(original.clone()));
            let chat = cx.new(|cx| {
                SideChat::new(
                    summary("local-side", "parent"),
                    "Parent".into(),
                    rpc.runs_at(original.clone()),
                    window,
                    cx,
                )
            });
            let old_id = chat.entity_id();
            chat.update(cx, |chat, cx| {
                chat.input
                    .update(cx, |input, cx| input.set_value("local draft", window, cx))
            });
            workspace.bind_side_chat("parent".into(), "local-side".into(), chat, window, cx);
            workspace.side.side_chat_visible = true;
            let saved = workspace.retain_private_terminals(cx);
            window.remove_window();
            (saved, old_id)
        })
    };
    let (view, cx) = cx.add_window_view(|window, cx| workspace(window, cx, &rpc, "unused", valid.clone()));
    view.update_in(cx, |workspace, window, cx| {
        workspace.side.side_chat_fixture_runs = Some(rpc.runs_at(source("/fixture/store-b", "gateway|org")));
        workspace.restore_private_terminals(saved, window, cx);
        assert!(workspace.open_side_chat().is_none());
        assert_eq!(workspace.waiting_side_chat().unwrap().preview().1, "local draft");
        workspace.side.side_chat_fixture_runs = Some(rpc.runs_at(source("/fixture/store-a", "gateway|another-org")));
        workspace.adopt_side_chats(window, cx);
        assert!(workspace.open_side_chat().is_none());
        assert_eq!(rpc.count("wb_overview"), 0);
        workspace.side.side_chat_fixture_runs = Some(rpc.runs_at(original));
        workspace.side.open = true;
        workspace.sync_side_chat(window, cx);
        assert!(workspace.waiting_side_chat().is_none());
        let chat = workspace.open_side_chat().unwrap();
        assert_ne!(chat.entity_id(), old_id);
        chat.read_with(cx, |chat, cx| {
            assert_eq!(chat.input.read(cx).value().as_ref(), "local draft")
        });
    });
    cx.run_until_parked();
    assert_eq!(rpc.count("wb_overview"), 1);
    rpc.poll("local-side", "parent");
    cx.run_until_parked();
    view.read_with(cx, |workspace, cx| {
        assert!(workspace.open_side_chat().unwrap().read(cx).can_command())
    });
}

mod partial_tests;
mod send_tests;
