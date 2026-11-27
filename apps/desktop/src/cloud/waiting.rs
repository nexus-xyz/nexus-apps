//! The messages a person sends to a new cloud session while its machine starts. They wait here in order, and
//! [`super::start_session`] sends them once the session is made, each once, each answered before the next. Ending the
//! session drops them. At most [`MAX_WAITING`] wait: each holds its attachments in memory.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use crate::attachments::Draft;

/// How many messages may wait for one start.
pub const MAX_WAITING: usize = 20;

/// A message typed while the session starts: its text and what the composer held besides.
#[derive(Debug, Clone, Default)]
pub struct Message {
    /// Names it while it waits ([`Waiting::push`] sets it): a position moves when the front goes out to send.
    pub id: u64,
    pub text: String,
    pub attachments: Draft,
}

/// The queue of one start, shared by the session's view and the start itself. Cloned, it is the same queue.
#[derive(Clone, Default)]
pub struct Waiting(Arc<Mutex<Queue>>);

#[derive(Default)]
struct Queue {
    messages: VecDeque<Message>,
    state: State,
    last_id: u64,
}

/// What [`Waiting::push`] did with a message.
#[derive(Debug)]
pub enum Push {
    /// It waits, or the session was ended and it is dropped.
    Queued,
    /// Every message already went: it comes back, to be sent as any message is.
    Late(Message),
    /// [`MAX_WAITING`] messages wait already: it comes back, and the person is told.
    Full(Message),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum State {
    /// The session is not made yet: messages wait.
    #[default]
    Open,
    /// Every message went: a new one is sent as any message is.
    Sent,
    /// The session was ended: nothing waits, and nothing more is sent.
    Ended,
}

impl Waiting {
    fn with<T>(&self, change: impl FnOnce(&mut Queue) -> T) -> T {
        change(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Queue `message` behind the others. Once they all went, it comes back to be sent as any message is. After the
    /// end it is dropped. A full queue gives it back.
    pub fn push(&self, mut message: Message) -> Push {
        self.with(|queue| match queue.state {
            State::Open if queue.messages.len() >= MAX_WAITING => Push::Full(message),
            State::Open => {
                queue.last_id += 1;
                message.id = queue.last_id;
                queue.messages.push_back(message);
                Push::Queued
            }
            State::Sent => Push::Late(message),
            State::Ended => Push::Queued,
        })
    }

    /// Whether no more messages can wait.
    pub fn is_full(&self) -> bool {
        self.with(|queue| queue.state == State::Open && queue.messages.len() >= MAX_WAITING)
    }

    /// The next message to send, taken off the queue. None once none is left: the queue is then done, and a later
    /// [`Waiting::push`] comes back to its caller.
    pub fn next(&self) -> Option<Message> {
        self.with(|queue| {
            if queue.state != State::Open {
                return None;
            }
            let next = queue.messages.pop_front();
            if next.is_none() {
                queue.state = State::Sent;
            }
            next
        })
    }

    /// A message whose send failed goes back first, for the next try.
    pub fn put_back(&self, message: Message) {
        self.with(|queue| {
            if queue.state != State::Ended {
                queue.state = State::Open;
                queue.messages.push_front(message);
            }
        });
    }

    /// Takes waiting message `id` back off the queue. None when it went out meanwhile.
    pub fn take_back(&self, id: u64) -> Option<Message> {
        self.with(|queue| {
            let at = queue.messages.iter().position(|message| message.id == id)?;
            queue.messages.remove(at)
        })
    }

    /// Everything still waiting, taken off the queue (Edit puts it back in the composer).
    pub fn take_all(&self) -> Vec<Message> {
        self.with(|queue| queue.messages.drain(..).collect())
    }

    /// The session was ended: what waits is dropped, and nothing more is sent.
    pub fn end(&self) {
        self.with(|queue| {
            queue.messages.clear();
            queue.state = State::Ended;
        });
    }

    pub fn ended(&self) -> bool {
        self.with(|queue| queue.state == State::Ended)
    }

    /// Whether messages still wait for the session to be made.
    pub fn is_open(&self) -> bool {
        self.with(|queue| queue.state == State::Open)
    }

    /// The waiting messages' ids and texts, in order.
    pub fn texts(&self) -> Vec<(u64, String)> {
        self.with(|queue| {
            queue
                .messages
                .iter()
                .map(|message| (message.id, message.text.clone()))
                .collect()
        })
    }
}

impl fmt::Debug for Waiting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (count, state) = self.with(|queue| (queue.messages.len(), queue.state));
        f.debug_struct("Waiting")
            .field("messages", &count)
            .field("state", &state)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> Message {
        Message {
            id: 0,
            text: text.into(),
            attachments: Draft::default(),
        }
    }

    fn drain(waiting: &Waiting) -> Vec<String> {
        std::iter::from_fn(|| waiting.next())
            .map(|message| message.text)
            .collect()
    }

    #[test]
    fn messages_go_in_the_order_they_were_sent() {
        let waiting = Waiting::default();
        for text in ["one", "two", "three"] {
            assert!(matches!(waiting.push(message(text)), Push::Queued));
        }
        let texts: Vec<String> = waiting.texts().into_iter().map(|(_, text)| text).collect();
        assert_eq!(texts, ["one", "two", "three"]);
        assert_eq!(drain(&waiting), ["one", "two", "three"]);
    }

    #[test]
    fn each_message_goes_once() {
        let waiting = Waiting::default();
        let shared = waiting.clone();
        assert!(matches!(waiting.push(message("one")), Push::Queued));
        assert_eq!(drain(&shared), ["one"]);
        // Done: a second drain (a retry) sends nothing again.
        assert!(drain(&shared).is_empty());
        assert!(!waiting.is_open());
        // A message after that comes back, to be sent as any message is.
        let Push::Late(late) = waiting.push(message("late")) else {
            panic!("sent as any message")
        };
        assert_eq!(late.text, "late");
        assert!(drain(&shared).is_empty());
    }

    #[test]
    fn a_failed_send_goes_first_on_the_next_try() {
        let waiting = Waiting::default();
        for text in ["one", "two"] {
            assert!(matches!(waiting.push(message(text)), Push::Queued));
        }
        let first = waiting.next().expect("one");
        waiting.put_back(first);
        assert!(matches!(waiting.push(message("three")), Push::Queued));
        assert_eq!(drain(&waiting), ["one", "two", "three"]);
    }

    #[test]
    fn ending_the_session_drops_what_waits() {
        let waiting = Waiting::default();
        let shared = waiting.clone();
        assert!(matches!(waiting.push(message("one")), Push::Queued));
        waiting.end();
        assert!(shared.ended());
        assert!(waiting.texts().is_empty());
        assert!(drain(&shared).is_empty());
        // Nothing queues or goes back after the end.
        assert!(matches!(waiting.push(message("two")), Push::Queued));
        waiting.put_back(message("three"));
        assert!(drain(&shared).is_empty());
    }

    #[test]
    fn a_message_taken_back_is_not_sent() {
        let waiting = Waiting::default();
        for text in ["one", "two", "three"] {
            assert!(matches!(waiting.push(message(text)), Push::Queued));
        }
        let ids: Vec<u64> = waiting.texts().into_iter().map(|(id, _)| id).collect();
        assert_eq!(
            waiting.take_back(ids[1]).map(|message| message.text).as_deref(),
            Some("two")
        );
        assert_eq!(drain(&waiting), ["one", "three"]);
    }

    /// The chip the person pressed is the one that comes back, even when the front went out to send meanwhile.
    #[test]
    fn taking_back_by_id_survives_the_front_going_out() {
        let waiting = Waiting::default();
        for text in ["one", "two", "three"] {
            assert!(matches!(waiting.push(message(text)), Push::Queued));
        }
        let ids: Vec<u64> = waiting.texts().into_iter().map(|(id, _)| id).collect();
        // The chip of "two" was drawn at position 1; `send_waiting` took "one" before the press landed.
        assert_eq!(waiting.next().map(|message| message.text).as_deref(), Some("one"));
        assert_eq!(
            waiting.take_back(ids[1]).map(|message| message.text).as_deref(),
            Some("two")
        );
        // A press on the chip of a message that went out takes nothing back.
        assert!(waiting.take_back(ids[0]).is_none());
        assert_eq!(drain(&waiting), ["three"]);
    }

    #[test]
    fn no_more_than_the_cap_wait() {
        let waiting = Waiting::default();
        for ix in 0..MAX_WAITING {
            assert!(matches!(waiting.push(message(&ix.to_string())), Push::Queued));
        }
        assert!(waiting.is_full());
        let Push::Full(over) = waiting.push(message("over")) else {
            panic!("full")
        };
        assert_eq!(over.text, "over");
        assert_eq!(waiting.texts().len(), MAX_WAITING);
        // One goes out: there is room again.
        assert!(waiting.next().is_some());
        assert!(!waiting.is_full());
    }
}
