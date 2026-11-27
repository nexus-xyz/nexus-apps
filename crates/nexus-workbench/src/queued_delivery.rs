//! Code message delivery, matching `queued.ts`, `delivery.ts` and `queued-chips.tsx`.

use crate::event::WbEvent;
use std::collections::{BTreeMap, BTreeSet};

pub const READ_FLASH_MS: i64 = 2500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Queued,
    Sent,
    Read { at: i64 },
    Dropped,
}

impl Delivery {
    pub fn note(self, working: bool, agent: &str, clock: impl FnOnce(i64) -> String) -> String {
        match self {
            Self::Queued => "Queued: sent when this turn ends".into(),
            Self::Sent if working => format!("Sent: {agent} reads it at its next step"),
            Self::Sent => "Sent".into(),
            Self::Read { at } => format!("Read by {agent} at {}", clock(at)),
            Self::Dropped => format!("Not sent: the turn stopped before {agent} read it"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub seq: i64,
    pub text: String,
    pub images: usize,
}
impl Message {
    pub fn chip_text(&self) -> String {
        let text = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            text
        } else if self.images > 1 {
            format!("{} images", self.images)
        } else {
            "Image".into()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Queue {
    pub waiting: Vec<(i64, String)>,
    pub cancelled: Vec<i64>,
    pub sending: Vec<Message>,
    pub delivery: BTreeMap<i64, Delivery>,
    messages: BTreeMap<i64, Message>,
    running: bool,
    handoff: bool,
    handed_off: Option<i64>,
    /// Messages on their way (`sending`) that a turn has started on since they went out. When that turn ends with no
    /// `Read` for them, they leave the composer: the agent had its turn with them.
    started: BTreeSet<i64>,
}
impl Queue {
    pub fn of(events: &[(i64, i64, WbEvent)]) -> Self {
        let mut queue = Self::default();
        for (seq, at, event) in events {
            queue.apply(*seq, *at, event);
        }
        queue
    }
    pub fn apply(&mut self, seq: i64, at: i64, event: &WbEvent) {
        if let WbEvent::Rewind { before, .. } = event {
            if *before >= seq {
                return;
            }
            let ids: Vec<i64> = self
                .waiting
                .iter()
                .map(|(seq, _)| *seq)
                .chain(self.sending.iter().map(|m| m.seq))
                .collect();
            for id in ids {
                self.delivery.insert(id, Delivery::Dropped);
            }
            self.waiting.clear();
            self.sending.clear();
            self.started.clear();
            self.running = false;
            self.handoff = false;
            self.handed_off = None;
            return;
        }
        if let WbEvent::User {
            from: Some(_), text, ..
        } = event
        {
            if !crate::session_results::is_result_batch(text) {
                self.running = true;
            }
            return;
        }
        let change = matches!(
            event,
            WbEvent::QueueCancel { .. }
                | WbEvent::QueueEdit { .. }
                | WbEvent::QueueMove { .. }
                | WbEvent::QueueSteer { .. }
                | WbEvent::Read { .. }
        );
        if self.handoff && !change {
            self.handoff = false;
            if !matches!(event, WbEvent::TurnStart { .. }) {
                for (id, _) in self.waiting.drain(..) {
                    self.delivery.insert(id, Delivery::Dropped);
                }
                if let Some(id) = self.handed_off {
                    self.unsend(id, Delivery::Dropped);
                }
                self.running = false;
            }
            self.handed_off = None;
        }
        match event {
            WbEvent::User { text, images, .. } => {
                if self.running {
                    self.waiting.push((seq, text.clone()));
                    self.messages.insert(
                        seq,
                        Message {
                            seq,
                            text: text.clone(),
                            images: images.len(),
                        },
                    );
                    self.delivery.insert(seq, Delivery::Queued);
                } else {
                    self.running = true;
                }
            }
            WbEvent::TurnStart { .. } => {
                self.running = true;
                self.started.extend(self.sending.iter().map(|message| message.seq));
            }
            WbEvent::QueueCancel { seq: id } | WbEvent::QueueSteer { seq: id } => {
                if self.waiting.iter().any(|(seq, _)| seq == id) {
                    self.waiting.retain(|(seq, _)| seq != id);
                    if matches!(event, WbEvent::QueueSteer { .. }) {
                        self.send(*id);
                    } else {
                        self.delivery.remove(id);
                        self.cancelled.push(*id);
                    }
                }
            }
            WbEvent::QueueEdit { seq: id, text } => {
                if let Some((_, previous)) = self.waiting.iter_mut().find(|(seq, _)| seq == id) {
                    *previous = text.clone();
                    if let Some(message) = self.messages.get_mut(id) {
                        message.text = text.clone();
                    }
                }
            }
            WbEvent::QueueMove { seq: id, before } => move_before(&mut self.waiting, *id, *before, |(seq, _)| *seq),
            WbEvent::Read { seq: id } => {
                if self.delivery.contains_key(id) {
                    self.unsend(*id, Delivery::Read { at });
                }
            }
            WbEvent::TurnEnd { .. } => {
                // A turn ran with them and no `Read` came (the turn was stopped, the agent quit, or it never echoes):
                // they stay Sent, off the composer, instead of waiting there for a read that will not come. A message
                // sent into this turn after it began stays: the agent may still read it in a turn of its own.
                let done: Vec<i64> = self.started.iter().copied().collect();
                for id in done {
                    self.unsend(id, Delivery::Sent);
                }
                if self.waiting.is_empty() {
                    self.running = false;
                } else {
                    let (id, _) = self.waiting.remove(0);
                    self.send(id);
                    self.handed_off = Some(id);
                    self.handoff = true;
                }
            }
            _ => {}
        }
    }
    fn send(&mut self, id: i64) {
        if let Some(message) = self.messages.get(&id) {
            self.sending.push(message.clone());
        }
        self.delivery.insert(id, Delivery::Sent);
    }
    fn unsend(&mut self, id: i64, state: Delivery) {
        self.sending.retain(|message| message.seq != id);
        self.started.remove(&id);
        self.delivery.insert(id, state);
    }
    /// Chips only flash when a message the view showed as Sending was read.
    pub fn read_since(&self, before: &Self) -> Vec<Message> {
        before
            .sending
            .iter()
            .filter(|message| {
                !self.sending.iter().any(|m| m.seq == message.seq)
                    && matches!(self.delivery.get(&message.seq), Some(Delivery::Read { .. }))
            })
            .cloned()
            .collect()
    }
}

/// A `QueueMove` replayed on a line of queued messages: `seq` goes just before `before`, or last when `before` is
/// None. A move that names a message no longer in the line changes nothing, as the workbench refused it.
pub fn move_before<T>(line: &mut Vec<T>, seq: i64, before: Option<i64>, id: impl Fn(&T) -> i64) {
    if before == Some(seq) || before.is_some_and(|before| !line.iter().any(|item| id(item) == before)) {
        return;
    }
    let Some(from) = line.iter().position(|item| id(item) == seq) else {
        return;
    };
    let item = line.remove(from);
    match before.and_then(|before| line.iter().position(|item| id(item) == before)) {
        Some(to) => line.insert(to, item),
        None => line.push(item),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    fn fold(events: Value) -> Queue {
        let events: Vec<WbEvent> =
            serde_json::from_value(events).expect("the queue event fixtures follow the wire contract");
        let mut queue = Queue::default();
        for (index, event) in events.iter().enumerate() {
            queue.apply(index as i64, index as i64, event);
        }
        queue
    }
    fn user(text: &str) -> Value {
        json!({"kind":"user","text":text})
    }
    fn start() -> Value {
        json!({"kind":"turnStart","turnId":"t"})
    }
    fn end() -> Value {
        json!({"kind":"turnEnd","turnId":"t","status":"completed","durationMs":1,"costUsd":null,"error":null})
    }
    #[test]
    fn steer_sends_then_read_confirms_with_time() {
        let before = fold(json!([user("a"), start(), user("b"), {"kind":"queueSteer","seq":2}]));
        assert_eq!(before.delivery[&2], Delivery::Sent);
        assert_eq!(before.sending[0].text, "b");
        let mut after = before.clone();
        after.apply(4, 4000, &WbEvent::Read { seq: 2 });
        assert_eq!(after.delivery[&2], Delivery::Read { at: 4000 });
        assert!(after.sending.is_empty());
        assert_eq!(after.read_since(&before)[0].text, "b");
    }
    #[test]
    fn handoff_survives_queue_changes_but_drops_without_start() {
        let sent = fold(json!([user("a"), start(), user("b"), user("c"), end()]));
        assert_eq!(sent.delivery[&2], Delivery::Sent);
        assert_eq!(sent.delivery[&3], Delivery::Queued);
        let stopped =
            fold(json!([user("a"), start(), user("b"), user("c"), end(), {"kind":"notice","text":"stopped"}]));
        assert_eq!(stopped.delivery[&2], Delivery::Dropped);
        assert_eq!(stopped.delivery[&3], Delivery::Dropped);
        let read = fold(
            json!([user("a"), start(), user("b"), user("c"), {"kind":"queueSteer","seq":2}, end(), {"kind":"read","seq":2}, start()]),
        );
        assert!(matches!(read.delivery[&2], Delivery::Read { .. }));
        assert_eq!(read.delivery[&3], Delivery::Sent);
    }
    fn interrupted() -> Value {
        json!({"kind":"turnEnd","turnId":"t","status":"interrupted","durationMs":1,"costUsd":null,"error":null})
    }
    /// Stuck delivery chips: a message handed off at a turn's end, whose own turn then ends without a
    /// `Read`, left the composer only on a rewind. It came back on every later turn, and after every restart.
    #[test]
    fn a_handed_off_message_leaves_the_composer_when_its_turn_ends_unread() {
        for ending in [interrupted(), end()] {
            let queue = fold(json!([
                user("a"),
                start(),
                user("b"),
                end(),
                start(),
                ending,
                user("next"),
                start()
            ]));
            assert!(queue.sending.is_empty(), "{:?}", queue.sending);
            assert_eq!(queue.delivery[&2], Delivery::Sent);
        }
        // A read that comes later still confirms it.
        let mut late = fold(json!([user("a"), start(), user("b"), end(), start(), interrupted()]));
        late.apply(6, 6000, &WbEvent::Read { seq: 2 });
        assert_eq!(late.delivery[&2], Delivery::Read { at: 6000 });
    }
    /// A message sent into a turn that ends first stays on the composer until the turn of its own that reads it.
    #[test]
    fn a_steer_waits_past_the_turn_it_was_sent_into() {
        let mut queue = fold(json!([user("a"), start(), user("b"), {"kind":"queueSteer","seq":2}, end()]));
        assert_eq!(
            queue.sending[0].seq, 2,
            "the turn it was sent into ended before the agent took it"
        );
        queue.apply(5, 5, &serde_json::from_value(start()).expect("turn start"));
        queue.apply(6, 6000, &WbEvent::Read { seq: 2 });
        assert!(queue.sending.is_empty());
        assert_eq!(queue.delivery[&2], Delivery::Read { at: 6000 });
    }
    #[test]
    fn folds_cancellation_rewrite_and_empty_native_replay() {
        let queue = fold(
            json!([user("a"), start(), user("b"), user("c"), {"kind":"queueEdit","seq":3,"text":"c2"}, {"kind":"queueCancel","seq":2}]),
        );
        assert_eq!(queue.waiting, vec![(3, "c2".into())]);
        assert!(!queue.delivery.contains_key(&2));
        let replay =
            fold(json!([user("a"), start(), user("b"), {"kind":"queueSteer","seq":2}, {"kind":"rewind","before":4}]));
        assert_eq!(replay.delivery[&2], Delivery::Sent);
        assert_eq!(replay.sending.len(), 1);
        let reset = fold(json!([user("a"), start(), user("b"), {"kind":"rewind","before":0}]));
        assert_eq!(reset.delivery[&2], Delivery::Dropped);
        assert!(reset.waiting.is_empty());
    }
    #[test]
    fn a_move_reorders_the_queue_and_the_turn_end_sends_the_new_first() {
        let moved = fold(json!([
            user("a"), start(), user("b"), user("c"), user("d"),
            {"kind":"queueMove","seq":4,"before":2},
            {"kind":"queueMove","seq":2,"before":null},
        ]));
        let order: Vec<i64> = moved.waiting.iter().map(|(seq, _)| *seq).collect();
        assert_eq!(order, vec![4, 3, 2]);
        let mut ended = moved.clone();
        ended.apply(7, 7, &serde_json::from_value(end()).expect("turn end"));
        assert_eq!(ended.delivery[&4], Delivery::Sent);
        assert_eq!(ended.delivery[&3], Delivery::Queued);
        // A move that names a message no longer waiting changes nothing.
        let stale = fold(json!([
            user("a"), start(), user("b"), user("c"),
            {"kind":"queueCancel","seq":2},
            {"kind":"queueMove","seq":3,"before":2},
        ]));
        assert_eq!(stale.waiting, vec![(3, "c".into())]);
    }
    #[test]
    fn image_only_queue_and_notes_do_not_claim_a_read() {
        let queue = fold(
            json!([user("a"), start(), {"kind":"user","text":"","images":[{"mediaType":"image/png","data":"","name":null}]}]),
        );
        assert_eq!(queue.messages[&2].images, 1);
        assert_eq!(queue.messages[&2].chip_text(), "Image");
        assert_eq!(
            Delivery::Queued.note(true, "Codex", |_| String::new()),
            "Queued: sent when this turn ends"
        );
        assert_eq!(Delivery::Sent.note(false, "Codex", |_| String::new()), "Sent");
        assert_eq!(
            Delivery::Sent.note(true, "Codex", |_| String::new()),
            "Sent: Codex reads it at its next step"
        );
        assert_eq!(
            Delivery::Dropped.note(false, "Codex", |_| String::new()),
            "Not sent: the turn stopped before Codex read it"
        );
        assert_eq!(
            Delivery::Read { at: 4000 }.note(false, "Claude Code", |_| "14:05".into()),
            "Read by Claude Code at 14:05"
        );
    }
}
