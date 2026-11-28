//! The side composer's keys act as the session composer's (`keys.rs`).
use super::{draw, event, own_blocks, panel};
use gpui_kit::{Focusable, TestAppContext};
use nexus_workbench::event::{ThreadStatus, WbEvent};

#[gpui_kit::test]
fn up_recalls_what_was_sent_and_shift_enter_continues_a_list(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    chat.update_in(cx, |chat, window, cx| {
        chat.poll = None;
        chat.partial_poll = None;
        chat.events = vec![event(
            1,
            110,
            WbEvent::User {
                text: "first question".into(),
                images: vec![],
                from: None,
            },
        )];
        chat.blocks = own_blocks(&chat.events, chat.summary.created_at);
        chat.input.focus_handle(cx).focus(window, cx);
        window.activate_window();
    });
    draw(cx);
    cx.simulate_keystrokes("up");
    draw(cx);
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), "first question")
    });
    chat.update_in(cx, |chat, window, cx| {
        chat.input.update(cx, |input, cx| {
            input.set_value("- one", window, cx);
            input.set_selected_range(5..5, cx);
        })
    });
    cx.simulate_keystrokes("shift-enter");
    draw(cx);
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), "- one\n- ")
    });
}

/// As in the session, Esc closes what is open and never stops a running turn: Stop is its button, or Mod+.
#[gpui_kit::test]
fn escape_never_stops_a_running_side_turn(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    chat.update_in(cx, |chat, window, cx| {
        chat.poll = None;
        chat.partial_poll = None;
        chat.summary.status = ThreadStatus::Running;
        chat.input.focus_handle(cx).focus(window, cx);
        window.activate_window();
    });
    draw(cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(chat.read_with(cx, |chat, _| !chat.sending), "no interrupt went");
}
