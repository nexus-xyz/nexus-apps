//! The voice pane in a window with no display (`voice-pane.test.ts`): fake ears, a fake agent API and a fake voice.

use std::cell::RefCell;

use std::rc::Rc;
use std::time::Duration;

use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use nexus_desktop_core::agent::Message;

use super::{Heard, Hearing, Senses, Speaker, VoiceEvent, VoicePane, VoiceState};
use crate::agent::{Chunk, Stream};

#[derive(Default)]
struct World {
    ears: Vec<UnboundedSender<Heard>>,
    asks: Vec<(String, UnboundedSender<Chunk>)>,
    said: Vec<String>,
    stops: usize,
    speaking: bool,
}

#[derive(Clone, Default)]
struct Fake(Rc<RefCell<World>>);

impl Senses for Fake {
    fn hear(&self) -> Hearing {
        let (out, events) = unbounded();
        self.0.borrow_mut().ears.push(out);
        Hearing { events, _stop: None }
    }

    fn ask(&self, call: crate::agent::Call) -> Stream {
        let crate::agent::Call::Ask { body } = call else {
            panic!("only asks")
        };
        let (out, chunks) = unbounded();
        self.0.borrow_mut().asks.push((body, out));
        Stream { chunks, _stop: None }
    }
}

impl Speaker for Fake {
    fn speak(&mut self, text: &str) -> bool {
        let mut world = self.0.borrow_mut();
        world.said.push(text.to_string());
        world.speaking = true;
        true
    }

    fn stop(&mut self) {
        let mut world = self.0.borrow_mut();
        world.stops += 1;
        world.speaking = false;
    }

    fn speaking(&self) -> bool {
        self.0.borrow().speaking
    }
}

impl Fake {
    fn hear(&self, heard: Heard) {
        let world = self.0.borrow();
        world
            .ears
            .last()
            .expect("open ears")
            .unbounded_send(heard)
            .expect("the pane listens");
    }

    /// The agent API answers the last ask with `messages`, in conversation `c-1`.
    fn answer(&self, messages: serde_json::Value) {
        let done = serde_json::json!({ "status": 200, "conversation": "c-1", "messages": messages });
        let world = self.0.borrow();
        let (_, out) = world.asks.last().expect("an ask");
        for chunk in [
            Chunk::Head {
                status: 200,
                content_type: "text/event-stream".into(),
            },
            Chunk::Data(format!("event: done\ndata: {done}\n\n").into_bytes()),
            Chunk::End,
        ] {
            out.unbounded_send(chunk).expect("the pane reads the answer");
        }
    }

    fn finish_speaking(&self) {
        self.0.borrow_mut().speaking = false;
    }
}

/// A window with an open pane whose ears opened, and every event it sent.
fn pane(
    cx: &mut TestAppContext,
) -> (
    Entity<VoicePane>,
    Fake,
    Rc<RefCell<Vec<VoiceEvent>>>,
    &mut VisualTestContext,
) {
    let fake = Fake::default();
    let (senses, speaker) = (fake.clone(), fake.clone());
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
            crate::palette::ThemeChoice::Dark,
            gpui_kit::WindowAppearance::Dark,
        )));
    });
    let (pane, cx) =
        cx.add_window_view(move |window, cx| VoicePane::new(Rc::new(senses), Box::new(speaker), None, window, cx));
    let events = Rc::new(RefCell::new(Vec::new()));
    let seen = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(&pane, move |_, event: &VoiceEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach()
    });
    assert_eq!(fake.0.borrow().ears.len(), 1, "the microphone opens when voice starts");
    fake.hear(Heard::Opened);
    cx.run_until_parked();
    (pane, fake, events, cx)
}

fn state(pane: &Entity<VoicePane>, cx: &mut VisualTestContext) -> VoiceState {
    pane.read_with(cx, |pane, _| pane.state())
}

fn shows(selector: &'static str, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.debug_bounds(selector).is_some()
}

const WRITE: &str = r#"[{ "kind": "answer", "text": "Here's the text. Send it?", "model": "m" },
    { "kind": "pending_action", "card": "c1", "approveUrl": "https://nexus.example/a", "detail": "",
      "write": { "connector": "gmail", "operation": "send", "summary": "Text Ana, Dev and Sam", "effect": "write" } }]"#;

/// The pane after "Tell Ana, Dev and Sam about Friday." and Nexus's answer, speaking.
fn speaking(
    cx: &mut TestAppContext,
) -> (
    Entity<VoicePane>,
    Fake,
    Rc<RefCell<Vec<VoiceEvent>>>,
    &mut VisualTestContext,
) {
    let (pane, fake, events, cx) = pane(cx);
    fake.hear(Heard::Turn("Tell Ana, Dev and Sam about Friday.".into()));
    cx.run_until_parked();
    assert_eq!(state(&pane, cx), VoiceState::Thinking);
    fake.answer(serde_json::from_str(WRITE).expect("JSON"));
    cx.run_until_parked();
    assert_eq!(state(&pane, cx), VoiceState::Speaking);
    (pane, fake, events, cx)
}

#[gpui_kit::test]
fn it_listens_with_the_pane_focused_and_mute_type_end_in_order(cx: &mut TestAppContext) {
    let (pane, _, _, cx) = pane(cx);
    assert_eq!(state(&pane, cx), VoiceState::Listening);
    let focused = cx.update(|window, cx| pane.read(cx).focus.is_focused(window));
    assert!(focused, "the pane takes the focus, not End");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let mute = cx.debug_bounds("voice-mute").expect("Mute");
    let typing = cx.debug_bounds("voice-type").expect("Type");
    let end = cx.debug_bounds("voice-end").expect("End");
    assert!(mute.left() < typing.left() && typing.left() < end.left());
}

#[gpui_kit::test]
fn a_spoken_turn_is_asked_in_the_conversation_said_and_captioned_both_sides(cx: &mut TestAppContext) {
    let (pane, fake, events, cx) = speaking(cx);
    let body = fake.0.borrow().asks[0].0.clone();
    assert!(body.contains("Tell Ana, Dev and Sam about Friday."), "{body}");
    assert_eq!(
        fake.0.borrow().said,
        ["Here's the text. Send it? Text Ana, Dev and Sam. That needs your approval in the thread."]
    );
    assert!(shows("voice-caption-you", cx) && shows("voice-caption-nexus", cx));
    assert!(events.borrow().contains(&VoiceEvent::Kept("c-1".into())));
    // The voice ends: it rests on Needs you, the dot under the mark.
    fake.finish_speaking();
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    assert_eq!(state(&pane, cx), VoiceState::NeedsYou);
    assert!(shows("voice-mark-dot", cx));
    // The next turn continues the conversation.
    fake.hear(Heard::Turn("And book the Uber.".into()));
    cx.run_until_parked();
    let body = fake.0.borrow().asks[1].0.clone();
    assert!(body.contains("\"conversation\":\"c-1\""), "{body}");
}

#[gpui_kit::test]
fn space_talks_over_nexus_and_it_rests_after_600_ms(cx: &mut TestAppContext) {
    let (pane, fake, _, cx) = speaking(cx);
    cx.simulate_keystrokes("space");
    assert_eq!(state(&pane, cx), VoiceState::Interrupted);
    assert_eq!(fake.0.borrow().stops, 1);
    cx.executor().advance_clock(Duration::from_millis(YIELD_MS - 1));
    cx.run_until_parked();
    assert_eq!(state(&pane, cx), VoiceState::Interrupted, "it yields 600 ms");
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    // The answer asked for a write: the call rests on Needs you, and the person's next words make it listen.
    assert_eq!(state(&pane, cx), VoiceState::NeedsYou);
    fake.hear(Heard::Words("and book".into()));
    cx.run_until_parked();
    assert_eq!(state(&pane, cx), VoiceState::Listening);
}

#[gpui_kit::test]
fn esc_leaves_voice_and_lands_the_card_and_the_write(cx: &mut TestAppContext) {
    let (pane, fake, events, cx) = speaking(cx);
    cx.simulate_keystrokes("escape");
    assert!(pane.read_with(cx, |pane, _| pane.ended()));
    assert_eq!(fake.0.borrow().stops, 1, "the voice stops at once");
    let events = events.borrow();
    let Some(VoiceEvent::Ended { summary, writes }) = events.last() else {
        panic!("the call ended: {events:?}")
    };
    let summary = summary.as_ref().expect("a card");
    assert!(summary.acted);
    assert_eq!(summary.done, ["Text Ana, Dev and Sam: waits for your approval"]);
    assert!(matches!(writes.as_slice(), [Message::PendingAction { card, .. }] if card == "c1"));
}

#[gpui_kit::test]
fn an_answer_still_coming_lands_once_it_arrives(cx: &mut TestAppContext) {
    let (pane, fake, events, cx) = pane(cx);
    fake.hear(Heard::Turn("What's happening with dinner?".into()));
    cx.run_until_parked();
    pane.update(cx, |pane, cx| pane.end(cx));
    assert!(
        events
            .borrow()
            .iter()
            .all(|event| !matches!(event, VoiceEvent::Ended { .. }))
    );
    fake.answer(serde_json::json!([{ "kind": "answer", "text": "Booked: Nopa, Friday at 7:30.", "model": "m" }]));
    cx.run_until_parked();
    assert!(fake.0.borrow().said.is_empty(), "an answer after the end is not spoken");
    let events = events.borrow();
    let Some(VoiceEvent::Ended { summary, writes }) = events.last() else {
        panic!("the call ended")
    };
    let summary = summary.as_ref().expect("a card");
    assert_eq!(summary.turns.len(), 2);
    assert!(!summary.acted && writes.is_empty());
}

#[gpui_kit::test]
fn a_poor_connection_offers_text_and_typing_asks_a_turn(cx: &mut TestAppContext) {
    let (pane, fake, events, cx) = pane(cx);
    fake.hear(Heard::Lost);
    cx.run_until_parked();
    assert_eq!(state(&pane, cx), VoiceState::PoorConnection);
    assert!(shows("voice-switch-to-text", cx));
    let bounds = cx.debug_bounds("voice-switch-to-text").expect("the offer");
    cx.simulate_click(bounds.center(), gpui_kit::Modifiers::none());
    assert!(events.borrow().contains(&VoiceEvent::SwitchToText));
    assert!(
        pane.read_with(cx, |pane, _| pane.ended()),
        "switching to text ends the call"
    );
}

#[gpui_kit::test]
fn a_typed_turn_is_asked(cx: &mut TestAppContext) {
    let (pane, fake, _, cx) = pane(cx);
    pane.update(cx, |pane, cx| pane.type_words("Book the Uber", cx));
    cx.run_until_parked();
    assert!(fake.0.borrow().asks[0].0.contains("Book the Uber"));
}

#[gpui_kit::test]
fn mute_closes_the_ears_and_says_so(cx: &mut TestAppContext) {
    let (pane, fake, _, cx) = pane(cx);
    pane.update(cx, |pane, cx| pane.toggle_mute(cx));
    cx.run_until_parked();
    assert_eq!(state(&pane, cx), VoiceState::Muted);
    assert!(fake.0.borrow().ears[0].is_closed(), "the microphone closed");
    pane.update(cx, |pane, cx| pane.toggle_mute(cx));
    assert_eq!(fake.0.borrow().ears.len(), 2, "unmuting opens it again");
}

use super::super::call::YIELD_MS;
