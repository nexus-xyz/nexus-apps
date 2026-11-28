//! Follow-ups queued in a Gateway chat while its answer streams. Gateway's chat send streams
//! one answer per request, so the queue lives in the app: each item goes through the chat's own send path, one at a
//! time, after the answer before it ends. This mirrors Nexus UI's `chat/plain-queue.ts` (the web's plain sessions),
//! with its tests; the chips' text and save rules are `queued-chips.tsx`'s.
//!
//! A failed answer or a Stop holds the queue instead of draining it: a queue that keeps sending after something went
//! wrong sends into a conversation the person has not read. A held queue waits for Resume. Send now stops the answer,
//! and its item goes next however the answer ends.
//!
//! Nexus GPUI keeps the queues in memory for the window's life; a session opened while idle with messages waiting holds them in
//! the same way ([`Queues::hold`]), because no answer will end to send them.

use std::collections::{HashMap, HashSet};

/// A queued message: its place in the queue's order, its text, and what goes with it (each app's attachments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item<P> {
    pub seq: u64,
    pub text: String,
    pub payload: P,
}

/// One chat's queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queue<P> {
    items: Vec<Item<P>>,
    held: bool,
    /// The item the person is rewriting: nothing goes on its own while one is, so the old text never goes mid-edit.
    editing: Option<u64>,
    /// The next item's seq.
    next: u64,
}

impl<P> Default for Queue<P> {
    fn default() -> Self {
        Queue {
            items: Vec::new(),
            held: false,
            editing: None,
            next: 1,
        }
    }
}

/// How an answer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    Failed,
    Stopped,
}

impl<P> Queue<P> {
    pub fn items(&self) -> &[Item<P>] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether the queue waits for Resume.
    pub fn held(&self) -> bool {
        self.held
    }

    /// Adds a message at the end; returns its seq.
    pub fn enqueue(&mut self, text: String, payload: P) -> u64 {
        let seq = self.next;
        self.items.push(Item { seq, text, payload });
        self.next += 1;
        seq
    }

    /// Rewrites item `seq`'s text; what goes with it stays.
    pub fn edit(&mut self, seq: u64, text: String) {
        if let Some(item) = self.items.iter_mut().find(|item| item.seq == seq) {
            item.text = text;
        }
    }

    /// Takes an item out. An empty queue has nothing to hold.
    pub fn remove(&mut self, seq: u64) {
        self.items.retain(|item| item.seq != seq);
        self.held = self.held && !self.items.is_empty();
        self.editing.take_if(|editing| *editing == seq);
    }

    /// The item being rewritten, while one is.
    pub fn editing(&self) -> Option<u64> {
        self.editing
    }

    /// The person starts rewriting item `seq` (`Some`), or stops (`None`): the queue waits while they do.
    pub fn set_editing(&mut self, seq: Option<u64>) {
        self.editing = seq.filter(|seq| self.items.iter().any(|item| item.seq == *seq));
    }

    /// Moves an item to the front (Send now).
    pub fn promote(&mut self, seq: u64) {
        if let Some(at) = self.items.iter().position(|item| item.seq == seq) {
            let item = self.items.remove(at);
            self.items.insert(0, item);
        }
    }

    /// The queue waits for Resume; an empty one has nothing to hold.
    pub fn hold(&mut self) {
        if !self.items.is_empty() {
            self.held = true;
        }
    }

    /// Resume: the queue drains again, starting with its first item.
    pub fn resume(&mut self) {
        self.held = false;
    }

    /// The first item to send, out of the queue; `None` while held, empty or while an item is being rewritten.
    pub fn take(&mut self) -> Option<Item<P>> {
        if self.editing.is_some() {
            return None;
        }
        self.take_now()
    }

    /// The first item, which the person sent now: a rewrite of another item does not hold it back.
    pub fn take_now(&mut self) -> Option<Item<P>> {
        if self.held || self.items.is_empty() {
            return None;
        }
        let item = self.items.remove(0);
        self.editing.take_if(|editing| *editing == item.seq);
        Some(item)
    }

    /// An answer ended. A complete one hands the turn to the first item; a failed or stopped one holds the queue,
    /// unless the person asked to send an item now (`send_now`): then the first item goes regardless.
    pub fn settle(&mut self, outcome: Outcome, send_now: bool) -> Option<Item<P>> {
        if send_now {
            self.resume();
            return self.take_now();
        } else if outcome != Outcome::Complete {
            self.hold();
        }
        self.take()
    }

    /// A sent item Gateway never took (the send failed before the question was stored): back at the front, held.
    pub fn put_back(&mut self, item: Item<P>) {
        self.items.retain(|waiting| waiting.seq != item.seq);
        self.next = self.next.max(item.seq + 1);
        self.items.insert(0, item);
        self.held = true;
    }
}

/// One queue per chat, by its key (a session id), and the chats whose first item goes as soon as the current answer
/// ends, however it ends (Send now). An emptied queue leaves the map.
#[derive(Debug, Clone)]
pub struct Queues<P> {
    queues: HashMap<String, Queue<P>>,
    now: HashSet<String>,
}

impl<P> Default for Queues<P> {
    fn default() -> Self {
        Queues {
            queues: HashMap::new(),
            now: HashSet::new(),
        }
    }
}

impl<P> Queues<P> {
    /// Chat `key`'s queue, if anything waits in it.
    pub fn get(&self, key: &str) -> Option<&Queue<P>> {
        self.queues.get(key)
    }

    /// Changes chat `key`'s queue; an emptied one is dropped.
    pub fn update<R>(&mut self, key: &str, change: impl FnOnce(&mut Queue<P>) -> R) -> R {
        let queue = self.queues.entry(key.to_string()).or_default();
        let result = change(queue);
        if queue.is_empty() && queue.next == 1 {
            self.queues.remove(key);
        } else if queue.is_empty() {
            // Keep the seqs moving forward, as long as anything might still point at one.
            queue.held = false;
        }
        result
    }

    /// Adds a message to chat `key`'s queue; returns its seq.
    pub fn enqueue(&mut self, key: &str, text: String, payload: P) -> u64 {
        self.update(key, |queue| queue.enqueue(text, payload))
    }

    /// Marks item `seq` to go first when the current answer ends, even if it ends in a Stop.
    pub fn send_now(&mut self, key: &str, seq: u64) {
        self.now.insert(key.to_string());
        self.update(key, |queue| queue.promote(seq));
    }

    /// Chat `key`'s answer ended: the item to send next, if any.
    pub fn settle(&mut self, key: &str, outcome: Outcome) -> Option<Item<P>> {
        let send_now = self.now.remove(key);
        self.update(key, |queue| queue.settle(outcome, send_now))
    }

    /// Chat `key` shows while no answer runs: what waits in it is held, since no answer will end to send it.
    pub fn hold(&mut self, key: &str) {
        if let Some(queue) = self.queues.get_mut(key) {
            queue.hold();
        }
    }

    /// A chat got its lasting key (a new chat's session id, once Gateway made it): its queue moves there.
    pub fn rename(&mut self, from: &str, to: &str) {
        if let Some(queue) = self.queues.remove(from) {
            self.queues.insert(to.to_string(), queue);
        }
        if self.now.remove(from) {
            self.now.insert(to.to_string());
        }
    }

    /// Drops chat `key`'s queue and its Send now.
    pub fn clear(&mut self, key: &str) {
        self.queues.remove(key);
        self.now.remove(key);
    }
}

/// What a chip shows (`queued-chips.tsx` `chipText`): the text on one line, or "Image" for a message that is only
/// attachments.
pub fn chip_text(text: &str, attachments: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !line.is_empty() {
        line
    } else if attachments > 1 {
        format!("{attachments} images")
    } else {
        "Image".to_string()
    }
}

/// Whether a rewrite can be saved (`queued-chips.tsx` `canSave`): a message left with no text needs attachments.
pub fn can_save(text: &str, attachments: usize) -> bool {
    !text.trim().is_empty() || attachments > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(texts: &[&str]) -> Queue<()> {
        let mut queue = Queue::default();
        for text in texts {
            queue.enqueue(text.to_string(), ());
        }
        queue
    }

    fn texts(queue: &Queue<()>) -> Vec<&str> {
        queue.items().iter().map(|item| item.text.as_str()).collect()
    }

    #[test]
    fn sends_in_order_one_per_finished_answer() {
        let mut queue = queued(&["first", "second"]);
        assert_eq!(queue.settle(Outcome::Complete, false).expect("an item").text, "first");
        assert_eq!(queue.settle(Outcome::Complete, false).expect("an item").text, "second");
        assert!(queue.settle(Outcome::Complete, false).is_none());
    }

    #[test]
    fn holds_on_a_failed_or_stopped_answer_and_sends_nothing_while_held() {
        for outcome in [Outcome::Failed, Outcome::Stopped] {
            let mut queue = queued(&["first"]);
            assert!(queue.settle(outcome, false).is_none());
            assert!(queue.held());
            assert_eq!(texts(&queue), ["first"]);
            assert!(queue.settle(Outcome::Complete, false).is_none());
            assert!(queue.take().is_none());
        }
    }

    #[test]
    fn waits_while_an_item_is_rewritten_and_goes_on_after() {
        let mut queue = queued(&["first", "second"]);
        let second = queue.items()[1].seq;
        queue.set_editing(Some(second));
        assert!(
            queue.settle(Outcome::Complete, false).is_none(),
            "nothing goes mid-edit"
        );
        assert!(!queue.held(), "a rewrite is no failure to resume from");
        assert_eq!(texts(&queue), ["first", "second"]);
        queue.set_editing(None);
        assert_eq!(queue.take().expect("an item").text, "first");
    }

    #[test]
    fn send_now_goes_while_another_item_is_rewritten_and_ends_its_own_rewrite() {
        let mut queue = queued(&["first", "second"]);
        let (first, second) = (queue.items()[0].seq, queue.items()[1].seq);
        queue.set_editing(Some(first));
        queue.promote(second);
        assert_eq!(queue.settle(Outcome::Stopped, true).expect("sent now").text, "second");
        assert_eq!(queue.editing(), Some(first));
        queue.set_editing(Some(first));
        assert_eq!(queue.take_now().expect("sent now").text, "first");
        assert_eq!(queue.editing(), None, "the item left with its rewrite");
        queue.set_editing(Some(99));
        assert_eq!(queue.editing(), None, "no rewrite of an item that is not there");
    }

    #[test]
    fn resumes_with_the_first_item() {
        let mut queue = queued(&["first", "second"]);
        queue.hold();
        queue.resume();
        assert_eq!(queue.take().expect("an item").text, "first");
        assert_eq!(texts(&queue), ["second"]);
        assert!(!queue.held());
    }

    #[test]
    fn sends_the_promoted_item_even_when_the_answer_it_stopped_ends_as_stopped() {
        let mut queue = queued(&["first", "second"]);
        queue.promote(2);
        assert_eq!(queue.settle(Outcome::Stopped, true).expect("an item").text, "second");
        assert!(!queue.held());
    }

    #[test]
    fn puts_an_item_gateway_never_took_back_at_the_front_held() {
        let mut queue = queued(&["first", "second"]);
        let taken = queue.take().expect("an item");
        queue.put_back(taken);
        assert_eq!(texts(&queue), ["first", "second"]);
        assert!(queue.held());
    }

    #[test]
    fn edits_and_removes_by_seq_and_an_emptied_queue_is_no_longer_held() {
        let mut queue = queued(&["first", "second"]);
        queue.edit(2, "second, rewritten".into());
        assert_eq!(texts(&queue), ["first", "second, rewritten"]);
        queue.hold();
        queue.remove(1);
        assert_eq!(texts(&queue), ["second, rewritten"]);
        assert!(queue.held());
        queue.remove(2);
        assert!(!queue.held());
    }

    #[test]
    fn hold_does_nothing_to_an_empty_queue() {
        let mut queue = queued(&[]);
        queue.hold();
        assert!(!queue.held());
    }

    #[test]
    fn keeps_one_queue_per_chat() {
        let mut queues = Queues::default();
        queues.enqueue("a", "for a".into(), ());
        assert!(queues.get("b").is_none());
        assert_eq!(queues.get("a").expect("a queue").items().len(), 1);
        assert_eq!(queues.settle("a", Outcome::Complete).expect("an item").text, "for a");
        assert!(queues.settle("b", Outcome::Complete).is_none());
    }

    #[test]
    fn send_now_sends_the_chosen_item_when_the_stopped_answer_ends_once() {
        let mut queues = Queues::default();
        queues.enqueue("a", "first".into(), ());
        queues.enqueue("a", "second".into(), ());
        queues.send_now("a", 2);
        assert_eq!(queues.settle("a", Outcome::Stopped).expect("an item").text, "second");
        assert!(queues.settle("a", Outcome::Stopped).is_none());
        assert!(queues.get("a").expect("a queue").held());
    }

    #[test]
    fn a_chat_opened_idle_holds_what_waits() {
        let mut queues = Queues::default();
        queues.enqueue("a", "first".into(), ());
        queues.hold("a");
        assert!(queues.get("a").expect("a queue").held());
        assert!(queues.settle("a", Outcome::Complete).is_none());
        queues.hold("b");
        assert!(queues.get("b").is_none());
    }

    #[test]
    fn a_new_chats_queue_moves_to_its_session_id() {
        let mut queues = Queues::default();
        queues.enqueue("", "first".into(), ());
        queues.send_now("", 1);
        queues.rename("", "session");
        assert!(queues.get("").is_none());
        assert_eq!(
            queues.settle("session", Outcome::Stopped).expect("an item").text,
            "first"
        );
    }

    #[test]
    fn seqs_stay_unique_after_a_queue_empties() {
        let mut queues = Queues::default();
        queues.enqueue("a", "first".into(), ());
        queues.update("a", |queue| queue.remove(1));
        assert!(queues.get("a").is_some_and(|queue| queue.is_empty()));
        assert_eq!(queues.enqueue("a", "second".into(), ()), 2);
        queues.clear("a");
        assert!(queues.get("a").is_none());
    }

    #[test]
    fn a_chip_shows_one_line_or_its_attachments() {
        assert_eq!(chip_text("  two\n lines ", 0), "two lines");
        assert_eq!(chip_text("", 1), "Image");
        assert_eq!(chip_text(" ", 3), "3 images");
        assert!(can_save("text", 0));
        assert!(can_save("", 1));
        assert!(!can_save("  ", 0));
    }
}
