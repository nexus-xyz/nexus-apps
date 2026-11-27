//! A long side chat lays out only the rows near the screen, and keeps its place across a primary close and reopen.
use super::SideChat;
use crate::code::Block;
use crate::workspace::runs::Runs;
use gpui_kit::component::text::TextViewState;
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, ListOffset, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, div, px,
};
use nexus_workbench::event::{StoredEvent, ThreadSummary, TurnStatus, WbEvent};

const TURNS: usize = 120;

fn summary() -> ThreadSummary {
    serde_json::from_value(serde_json::json!({
        "id": "side-long", "projectId": "p", "title": "A long side chat", "agent": "codex", "model": null,
        "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0,
        "nexusSessions": [], "createdAt": 100, "updatedAt": 100, "archived": false,
        "sideChat": true, "sourceThreadId": "parent"
    }))
    .expect("side chat summary")
}

fn event(seq: i64, event: WbEvent) -> StoredEvent {
    StoredEvent {
        thread_id: "side-long".into(),
        seq,
        at: 200 + seq,
        event,
    }
}

/// A question, its answer and its turn's end, `TURNS` times: the turn rows carry `side-turn-<ix>`.
fn long_history() -> Vec<StoredEvent> {
    (0..TURNS as i64)
        .flat_map(|n| {
            [
                event(
                    n * 3 + 1,
                    WbEvent::User {
                        text: format!("Side question {n}"),
                        images: vec![],
                        from: None,
                    },
                ),
                event(
                    n * 3 + 2,
                    WbEvent::Text {
                        id: format!("text-{n}"),
                        text: format!("Answer {n}"),
                        parent: None,
                    },
                ),
                event(
                    n * 3 + 3,
                    WbEvent::TurnEnd {
                        turn_id: format!("turn-{n}"),
                        status: TurnStatus::Completed,
                        cost_usd: None,
                        duration_ms: 1,
                        error: None,
                        cursor: None,
                    },
                ),
            ]
        })
        .collect()
}

/// The rows of turn ends, in order.
fn turns(chat: &SideChat) -> Vec<usize> {
    chat.blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| matches!(block, Block::Turn { .. }))
        .map(|(ix, _)| ix)
        .collect()
}

fn fill(chat: &mut SideChat, events: Vec<StoredEvent>, cx: &mut Context<SideChat>) {
    chat.events = events;
    chat.blocks = super::own_blocks(&chat.events, 100);
    for (ix, block) in chat.blocks.iter().enumerate() {
        if let Block::Text(text) = block {
            chat.texts.insert(ix, cx.new(|cx| TextViewState::markdown(text, cx)));
        }
    }
    cx.notify();
}

struct Panel(Entity<SideChat>);
impl Render for Panel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(480.)).h(px(640.)).child(self.0.clone())
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<SideChat>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (panel, cx) = cx.add_window_view(|window, cx| {
        Panel(cx.new(|cx| SideChat::new(summary(), "Parent session".into(), Runs::isolated(), window, cx)))
    });
    let chat = panel.read_with(cx, |panel, _| panel.0.clone());
    chat.update(cx, |chat, cx| fill(chat, long_history(), cx));
    draw(cx);
    (chat, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn shown(cx: &mut VisualTestContext, ix: usize) -> bool {
    let selector: &'static str = Box::leak(format!("side-turn-{ix}").into_boxed_str());
    cx.debug_bounds(selector).is_some()
}

#[gpui_kit::test]
fn a_long_side_chat_lays_out_only_the_rows_near_its_end(cx: &mut TestAppContext) {
    let (chat, cx) = open(cx);
    let turns = chat.read_with(cx, |chat, _| turns(chat));
    assert_eq!(turns.len(), TURNS);
    assert!(shown(cx, turns[TURNS - 1]), "the latest turn shows");
    assert!(
        !shown(cx, turns[0]),
        "the first turn is far above the screen and is not laid out"
    );
}

#[gpui_kit::test]
fn a_reader_scrolled_up_stays_on_their_row_while_new_rows_arrive(cx: &mut TestAppContext) {
    let (chat, cx) = open(cx);
    let turns = chat.read_with(cx, |chat, _| turns(chat));
    chat.update(cx, |chat, cx| {
        chat.scroll.list.scroll_to(ListOffset {
            item_ix: turns[10],
            offset_in_item: px(0.),
        });
        cx.notify();
    });
    draw(cx);
    assert!(shown(cx, turns[10]));
    assert!(!chat.read_with(cx, |chat, _| chat.scroll.at_end()));
    chat.update(cx, |chat, cx| {
        let mut events = chat.events.clone();
        events.extend(long_history().into_iter().take(3).map(|mut stored| {
            stored.seq += 1000;
            stored.at += 1000;
            stored
        }));
        fill(chat, events, cx);
    });
    draw(cx);
    assert!(shown(cx, turns[10]), "the new answer does not move the reader");
    let last = chat.read_with(cx, |chat, _| *self::turns(chat).last().unwrap());
    assert!(!shown(cx, last));
    chat.update(cx, |chat, cx| {
        chat.scroll.to_end();
        cx.notify();
    });
    draw(cx);
    assert!(shown(cx, last), "the jump to the latest goes to the new end");
}

#[gpui_kit::test]
fn a_long_side_chat_keeps_its_place_across_a_primary_close_and_reopen(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    for scrolled_up in [true, false] {
        let (saved, turns) = {
            let (panel, cx) = cx.add_window_view(|window, cx| {
                Panel(cx.new(|cx| SideChat::new(summary(), "Parent session".into(), Runs::isolated(), window, cx)))
            });
            let chat = panel.read_with(cx, |panel, _| panel.0.clone());
            chat.update(cx, |chat, cx| fill(chat, long_history(), cx));
            draw(cx);
            let turns = chat.read_with(cx, |chat, _| turns(chat));
            if scrolled_up {
                chat.update(cx, |chat, cx| {
                    chat.scroll.list.scroll_to(ListOffset {
                        item_ix: turns[10],
                        offset_in_item: px(0.),
                    });
                    cx.notify();
                });
                draw(cx);
            }
            let saved = chat.update_in(cx, |chat, window, cx| {
                let saved = chat.retain(cx);
                window.remove_window();
                saved
            });
            (saved, turns)
        };
        cx.run_until_parked();
        let (panel, cx) =
            cx.add_window_view(|window, cx| Panel(cx.new(|cx| SideChat::restore(saved, Runs::isolated(), window, cx))));
        draw(cx);
        let chat = panel.read_with(cx, |panel, _| panel.0.clone());
        assert_eq!(
            chat.read_with(cx, |chat, _| chat.texts.len()),
            TURNS,
            "the reopened chat draws its answers' text"
        );
        if scrolled_up {
            assert!(shown(cx, turns[10]), "the reopened chat shows the row the reader left");
            assert!(!shown(cx, turns[TURNS - 1]));
        } else {
            assert!(shown(cx, turns[TURNS - 1]), "the reopened chat is at its end");
            assert!(!shown(cx, turns[10]));
        }
    }
}
