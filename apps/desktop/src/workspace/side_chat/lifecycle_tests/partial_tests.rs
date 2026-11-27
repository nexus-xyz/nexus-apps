//! Actual deferred snapshot RPCs cross fresh UI incarnations; no bare global cache is blessed.
use super::super::{RetainedChat, SideChat};
use super::{Rpc, summary};
use gpui_kit::TestAppContext;
use nexus_workbench::event::{StoredEvent, ThreadStatus, WbEvent};
use nexus_workbench::stream_snapshot::{Message, Snapshot};
use serde_json::json;
use std::{cell::Cell, rc::Rc, time::Duration};
fn parked(cx: &mut TestAppContext, rpc: &Rpc, valid: Rc<Cell<bool>>) -> RetainedChat {
    let (chat, cx) = cx.add_window_view(|window, cx| {
        SideChat::new(
            summary("qualified", "parent"),
            "Parent".into(),
            rpc.runs("machine"),
            window,
            cx,
        )
    });
    chat.update_in(cx, |chat, window, cx| {
        chat.pending.guard(Rc::new(move || valid.get()));
        let saved = chat.retain(cx);
        window.remove_window();
        saved
    })
}
fn snapshot(incarnation: &str, revision: u64, text: &str) -> Snapshot {
    let mut summary = summary("qualified", "parent");
    summary.status = ThreadStatus::Running;
    Snapshot {
        thread_id: "qualified".into(),
        summary,
        incarnation: Some(incarnation.into()),
        revision,
        through_seq: 0,
        events: vec![],
        more: false,
        messages: if text.is_empty() {
            vec![]
        } else {
            vec![Message {
                id: "m".into(),
                parent: None,
                text: text.into(),
            }]
        },
    }
}
fn ready(rpc: &Rpc) {
    rpc.take("wb_thread_events").answer.send(Ok(json!([]))).unwrap();
    let mut running = summary("qualified", "parent");
    running.status = ThreadStatus::Running;
    rpc.take("wb_overview")
        .answer
        .send(Ok(json!({"threads":[running]})))
        .unwrap();
}
#[gpui_kit::test]
fn qualified_partial_reply_updates_fresh_ui_and_preserves_unchanged_text_entity(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let saved = parked(cx, &rpc, Rc::new(Cell::new(true)));
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("machine"), window, cx));
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    ready(&rpc);
    cx.run_until_parked();
    let read = rpc.take("wb_thread_stream");
    assert_eq!(read.args["threadId"], "qualified");
    read.answer
        .send(Ok(
            serde_json::to_value(snapshot("run-a", 1, "current partial")).unwrap()
        ))
        .unwrap();
    cx.run_until_parked();
    let id = chat.read_with(cx, |chat, _| {
        assert_eq!(chat.streamed.as_ref().unwrap().0, "current partial");
        chat.streamed.as_ref().unwrap().1.entity_id()
    });
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    rpc.take("wb_thread_stream")
        .answer
        .send(Ok(
            serde_json::to_value(snapshot("run-a", 1, "current partial")).unwrap()
        ))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| {
        assert_eq!(chat.streamed.as_ref().unwrap().1.entity_id(), id)
    });
    for (revision, text) in [(2, "current partial grows"), (3, "replacement")] {
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
        rpc.take("wb_thread_stream")
            .answer
            .send(Ok(serde_json::to_value(snapshot("run-a", revision, text)).unwrap()))
            .unwrap();
        cx.run_until_parked();
        chat.read_with(cx, |chat, cx| {
            let (source, view) = chat.streamed.as_ref().unwrap();
            assert_eq!(source, text);
            assert_eq!(view.entity_id(), id, "changed snapshots keep the parser entity");
            // These plain paragraphs retain their line break in the pinned parser's rendered projection.
            assert_eq!(view.read(cx).rendered_text().as_str(), format!("{text}\n"));
        });
    }
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    rpc.take("wb_thread_stream")
        .answer
        .send(Ok(serde_json::to_value(snapshot("run-a", 4, "")).unwrap()))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| assert!(chat.streamed.is_none()));
}
#[gpui_kit::test]
fn qualified_partial_older_than_durable_completion_never_resurrects(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let saved = parked(cx, &rpc, Rc::new(Cell::new(true)));
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("machine"), window, cx));
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    ready(&rpc);
    cx.run_until_parked();
    let old = rpc.take("wb_thread_stream");
    // A source-validated durable read completes while the earlier partial request is still in flight.
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    rpc.take("wb_thread_events")
        .answer
        .send(Ok(json!([StoredEvent {
            thread_id: "qualified".into(),
            seq: 1,
            at: 110,
            event: WbEvent::Text {
                id: "m".into(),
                parent: None,
                text: "complete".into()
            }
        }])))
        .unwrap();
    let mut running = summary("qualified", "parent");
    running.status = ThreadStatus::Running;
    rpc.take("wb_overview")
        .answer
        .send(Ok(json!({"threads":[running]})))
        .unwrap();
    cx.run_until_parked();
    old.answer
        .send(Ok(serde_json::to_value(snapshot("run-a", 1, "old partial")).unwrap()))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| {
        assert!(chat.streamed.is_none());
        assert_eq!(chat.events.len(), 1);
    });
}
#[gpui_kit::test]
fn qualified_partial_stop_and_ui_replacement_refuse_held_old_incarnation(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let saved = parked(cx, &rpc, Rc::new(Cell::new(true)));
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("machine"), window, cx));
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    ready(&rpc);
    cx.run_until_parked();
    let old = rpc.take("wb_thread_stream");
    chat.update_in(cx, |chat, window, cx| {
        chat.command("wb_interrupt", json!({"threadId":"qualified"}), false, None, window, cx)
    });
    cx.run_until_parked();
    let stopped = rpc.take("wb_interrupt");
    // A normal durable poll may complete before Stop's held response. It must not start
    // another partial RPC that can carry pre-Stop text past the acknowledgement.
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    ready(&rpc);
    cx.run_until_parked();
    assert_eq!(rpc.count("wb_thread_stream"), 1);
    stopped.answer.send(Ok(json!(summary("qualified", "parent")))).unwrap();
    cx.run_until_parked();
    let _ = old
        .answer
        .send(Ok(serde_json::to_value(snapshot("run-a", 99, "before stop")).unwrap()));
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| assert!(chat.streamed.is_none()));
    // Same source, fresh UI/epoch and run B; the cancelled A request cannot satisfy B.
    let saved = chat.update_in(cx, |chat, window, cx| {
        let saved = chat.retain(cx);
        window.remove_window();
        saved
    });
    cx.run_until_parked();
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("machine"), window, cx));
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    ready(&rpc);
    cx.run_until_parked();
    rpc.take("wb_thread_stream")
        .answer
        .send(Ok(serde_json::to_value(snapshot("run-b", 1, "new producer")).unwrap()))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| {
        assert_eq!(chat.streamed.as_ref().unwrap().0, "new producer")
    });
}
#[gpui_kit::test]
fn qualified_partial_legacy_refusal_reprobes_same_source_after_bounded_cooldown(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let saved = parked(cx, &rpc, Rc::new(Cell::new(true)));
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("machine"), window, cx));
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    ready(&rpc);
    cx.run_until_parked();
    rpc.take("wb_thread_stream")
        .answer
        .send(Err("Unknown command wb_thread_stream.".into()))
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert_eq!(rpc.count("wb_thread_stream"), 1);
    ready(&rpc);
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(28));
    cx.run_until_parked();
    assert_eq!(rpc.count("wb_thread_stream"), 2);
    rpc.take("wb_thread_stream")
        .answer
        .send(Ok(serde_json::to_value(snapshot(
            "upgraded-run",
            1,
            "after peer upgrade",
        ))
        .unwrap()))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| {
        assert_eq!(chat.streamed.as_ref().unwrap().0, "after peer upgrade")
    });
}
#[gpui_kit::test]
fn qualified_partial_authority_revocation_rejects_actual_reply(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let valid = Rc::new(Cell::new(true));
    let saved = parked(cx, &rpc, valid.clone());
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("machine"), window, cx));
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    ready(&rpc);
    cx.run_until_parked();
    let read = rpc.take("wb_thread_stream");
    valid.set(false);
    read.answer
        .send(Ok(
            serde_json::to_value(snapshot("old", 1, "must stay private")).unwrap()
        ))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| assert!(chat.streamed.is_none()));
}

#[gpui_kit::test]
fn qualified_partial_source_change_rejects_actual_reply(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rpc = Rpc::default();
    let valid = Rc::new(Cell::new(true));
    let saved = parked(cx, &rpc, valid.clone());
    let (chat, cx) = cx.add_window_view(|window, cx| SideChat::restore(saved, rpc.runs("machine"), window, cx));
    chat.update_in(cx, |chat, window, cx| chat.active(true, window, cx));
    cx.run_until_parked();
    ready(&rpc);
    cx.run_until_parked();
    let read = rpc.take("wb_thread_stream");
    chat.update(cx, |chat, _| chat.runs = rpc.runs("another-machine"));
    read.answer
        .send(Ok(
            serde_json::to_value(snapshot("old", 1, "must stay private")).unwrap()
        ))
        .unwrap();
    cx.run_until_parked();
    chat.read_with(cx, |chat, _| assert!(chat.streamed.is_none()));
}
