//! Dispatch identity protects both cold and warm chat reads across navigation and local turns.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Apply {
    Ignore,
    Replace,
    Metadata,
}
struct Pending {
    id: String,
    sequence: u64,
    replace: bool,
    warmed: bool,
}
#[derive(Default)]
pub(super) struct WarmChatRead {
    pending: Option<Pending>,
}
impl WarmChatRead {
    pub fn dispatched(&mut self, id: String, sequence: u64, replace: bool) {
        self.pending = Some(Pending {
            id,
            sequence,
            replace,
            warmed: false,
        });
    }
    pub fn clear(&mut self) {
        self.pending = None;
    }
    pub fn replacing(&self, id: &str) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.id == id && pending.replace)
    }
    pub fn mutated(&mut self, id: Option<&str>) {
        if let Some(pending) = self.pending.as_mut()
            && Some(pending.id.as_str()) == id
        {
            pending.replace = false;
        }
    }
    pub fn receive(&mut self, id: &str, sequence: u64, cached: bool) -> Apply {
        let Some(pending) = self
            .pending
            .as_mut()
            .filter(|pending| pending.id == id && pending.sequence == sequence)
        else {
            return Apply::Ignore;
        };
        if cached {
            if !pending.replace || pending.warmed {
                return Apply::Ignore;
            }
            pending.warmed = true;
            return Apply::Replace;
        }
        let replace = std::mem::replace(&mut pending.replace, false);
        if replace { Apply::Replace } else { Apply::Metadata }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{Event, OpenedChat};
    use serde_json::json;

    /// Replays actual channel events against the same reducer used by Workspace::handle.
    struct View {
        read: WarmChatRead,
        opening: bool,
        replacements: Vec<String>,
        metadata: usize,
    }
    impl View {
        fn new() -> Self {
            Self {
                read: WarmChatRead::default(),
                opening: false,
                replacements: vec![],
                metadata: 0,
            }
        }
        fn open(&mut self, id: &str, sequence: u64) {
            self.read.clear();
            self.opening = true;
            self.read.dispatched(id.into(), sequence, true);
        }
        fn deliver(&mut self, event: Event) {
            let Event::ChatRead { read, id, cached, chat } = event else {
                panic!("expected chat read")
            };
            match self.read.receive(&id, read, cached) {
                Apply::Replace => {
                    self.opening = false;
                    self.replacements.push(chat.expect("chat").id);
                }
                Apply::Metadata => {
                    self.metadata += 1;
                }
                Apply::Ignore => {}
            }
        }
    }
    fn answer(id: &str, read: u64, cached: bool) -> Event {
        Event::ChatRead {
            read,
            id: id.into(),
            cached,
            chat: Ok(OpenedChat::from_detail(id.into(), &json!({"messages": []}))),
        }
    }
    #[test]
    fn queued_a1_cannot_open_or_consume_a2_after_navigation() {
        let mut view = View::new();
        view.open("a", 1);
        view.open("b", 2);
        view.open("a", 3);
        view.deliver(answer("a", 1, true));
        assert!(view.opening, "old warm reply must not close the newer loading state");
        assert!(view.replacements.is_empty());
        view.deliver(answer("a", 3, true));
        assert_eq!(view.replacements, ["a"]);
        view.deliver(answer("a", 1, false));
        view.deliver(answer("b", 2, false));
        assert_eq!(view.replacements.len(), 1);
        view.deliver(answer("a", 3, false));
        assert_eq!(
            view.replacements,
            ["a", "a"],
            "latest fresh reply replaces its warm snapshot"
        );
        assert_eq!(view.metadata, 0);
    }
    #[test]
    fn stale_cold_failures_do_not_end_the_latest_open() {
        let mut view = View::new();
        view.open("a", 1);
        view.open("a", 2);
        view.deliver(Event::ChatRead {
            read: 1,
            id: "a".into(),
            cached: false,
            chat: Err("old failure".into()),
        });
        assert!(view.opening);
        view.deliver(answer("a", 2, false));
        assert!(!view.opening);
        assert_eq!(view.replacements, ["a"]);
    }
    #[test]
    fn local_turn_completion_does_not_rearm_the_old_snapshot() {
        let mut view = View::new();
        view.open("chat", 1);
        view.deliver(answer("chat", 1, true));
        view.read.mutated(Some("chat"));
        view.deliver(answer("chat", 1, false));
        assert_eq!(view.replacements.len(), 1);
        assert_eq!(view.metadata, 1);
        view.read.dispatched("chat".into(), 2, false); // Stream-end metadata refresh.
        view.deliver(answer("chat", 2, true));
        view.deliver(answer("chat", 2, false));
        assert_eq!(view.replacements.len(), 1);
        assert_eq!(view.metadata, 2);
    }
    #[test]
    fn detached_a_cannot_consume_b_reads_or_replay_a_snapshot_on_resume() {
        let mut view = View::new();
        view.open("a", 1);
        view.deliver(answer("a", 1, true));
        view.read.mutated(Some("a")); // The local turn starts, then navigation detaches it.
        view.open("b", 2);
        view.read.mutated(Some("a")); // Detached A starts its queued follow-up.
        view.deliver(answer("a", 1, false));
        assert!(view.opening);
        view.deliver(answer("b", 2, true));
        view.deliver(answer("b", 2, false));
        assert_eq!(view.replacements, ["a", "b", "b"]);
        view.read.clear(); // reset_chat, followed by resume_live_chat, dispatches no stored read.
        view.deliver(answer("a", 1, false));
        view.deliver(answer("b", 2, false));
        assert_eq!(view.replacements, ["a", "b", "b"]);
        assert_eq!(view.metadata, 0);
    }
    #[test]
    fn refreshing_an_invalidated_warm_open_preserves_its_replacement_intent() {
        let mut view = View::new();
        view.open("chat", 1);
        view.deliver(answer("chat", 1, true));
        assert!(!view.opening);
        let replace = view.opening || view.read.replacing("chat");
        view.read.dispatched("chat".into(), 2, replace);
        view.deliver(answer("chat", 1, false));
        view.deliver(answer("chat", 2, false));
        assert_eq!(view.replacements.len(), 2);
    }
}
