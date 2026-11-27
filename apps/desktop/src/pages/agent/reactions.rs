//! Tapbacks on the Assistant's bubbles, as in Messages (brand v5 A1·D: "a reaction is one icon in a 22 px pill on the
//! corner"). Hover a bubble, pick one of six, and it shows on the bubble's corner. Picking it again takes it off; picking
//! another replaces it, so a bubble has one at most.
//!
//! They are kept on this computer, per conversation, with the Assistant's other marks (`workspace/assistant.rs`). A
//! bubble is named by its place in the thread (`key`), not by its words, so nothing of the conversation is kept. The
//! agent API has no reactions yet: keeping them on the server is a follow-up.

use serde::{Deserialize, Serialize};

/// The six tapbacks, in Messages' order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tapback {
    Heart,
    Up,
    Down,
    Laugh,
    Emphasis,
    Question,
}

impl Tapback {
    pub const ALL: [Tapback; 6] = [
        Tapback::Heart,
        Tapback::Up,
        Tapback::Down,
        Tapback::Laugh,
        Tapback::Emphasis,
        Tapback::Question,
    ];

    pub fn glyph(self) -> &'static str {
        match self {
            Tapback::Heart => "❤️",
            Tapback::Up => "👍",
            Tapback::Down => "👎",
            Tapback::Laugh => "😂",
            Tapback::Emphasis => "‼️",
            Tapback::Question => "❓",
        }
    }

    /// What a screen reader and the tooltip say.
    pub fn name(self) -> &'static str {
        match self {
            Tapback::Heart => "Love",
            Tapback::Up => "Like",
            Tapback::Down => "Dislike",
            Tapback::Laugh => "Laugh",
            Tapback::Emphasis => "Emphasize",
            Tapback::Question => "Question",
        }
    }
}

/// A bubble's place in the thread: the entry, and for Nexus's reply the message in it.
pub fn key(entry: usize, message: Option<usize>) -> String {
    match message {
        Some(message) => format!("{entry}.{message}"),
        None => entry.to_string(),
    }
}

/// How many conversations keep their tapbacks: the oldest goes first.
pub const KEPT: usize = 50;

/// The tapbacks of each conversation, the most recently changed last.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reactions(Vec<Conversation>);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Conversation {
    id: String,
    marks: Vec<(String, Tapback)>,
}

impl Reactions {
    /// The tapback on bubble `key` of `conversation`.
    pub fn get(&self, conversation: &str, key: &str) -> Option<Tapback> {
        let kept = self.0.iter().find(|kept| kept.id == conversation)?;
        kept.marks.iter().find(|(at, _)| at == key).map(|(_, tapback)| *tapback)
    }

    /// Pick `tapback` on bubble `key` of `conversation`: the same one again takes it off. Returns what the bubble
    /// shows now.
    pub fn toggle(&mut self, conversation: &str, key: &str, tapback: Tapback) -> Option<Tapback> {
        let at = match self.0.iter().position(|kept| kept.id == conversation) {
            Some(at) => at,
            None => {
                self.0.push(Conversation {
                    id: conversation.to_string(),
                    marks: Vec::new(),
                });
                self.0.len() - 1
            }
        };
        let mut kept = self.0.remove(at);
        let now = match kept.marks.iter().position(|(mark, _)| mark == key) {
            Some(ix) if kept.marks[ix].1 == tapback => {
                kept.marks.remove(ix);
                None
            }
            Some(ix) => {
                kept.marks[ix].1 = tapback;
                Some(tapback)
            }
            None => {
                kept.marks.push((key.to_string(), tapback));
                Some(tapback)
            }
        };
        if !kept.marks.is_empty() {
            self.0.push(kept);
        }
        if self.0.len() > KEPT {
            self.0.drain(..self.0.len() - KEPT);
        }
        now
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tapback_toggles_and_another_replaces_it() {
        let mut reactions = Reactions::default();
        assert_eq!(
            reactions.toggle("c1", &key(1, Some(0)), Tapback::Heart),
            Some(Tapback::Heart)
        );
        assert_eq!(reactions.get("c1", "1.0"), Some(Tapback::Heart));
        assert_eq!(reactions.get("c2", "1.0"), None, "per conversation");
        assert_eq!(reactions.toggle("c1", "1.0", Tapback::Laugh), Some(Tapback::Laugh));
        assert_eq!(reactions.get("c1", "1.0"), Some(Tapback::Laugh), "one per bubble");
        assert_eq!(reactions.toggle("c1", "1.0", Tapback::Laugh), None);
        assert_eq!(reactions.get("c1", "1.0"), None);
        assert!(reactions.is_empty(), "a conversation with none is not kept");
    }

    #[test]
    fn they_survive_a_round_trip_and_keep_only_the_latest_conversations() {
        let mut reactions = Reactions::default();
        for ix in 0..KEPT + 3 {
            reactions.toggle(&format!("c{ix}"), &key(0, None), Tapback::Up);
        }
        assert_eq!(reactions.get("c0", "0"), None, "the oldest went");
        assert_eq!(reactions.get(&format!("c{}", KEPT + 2), "0"), Some(Tapback::Up));
        let text = serde_json::to_string(&reactions).expect("serialize");
        assert!(text.contains(r#""up""#), "{text}");
        let back: Reactions = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back, reactions);
    }

    #[test]
    fn six_tapbacks_in_messages_order() {
        let glyphs: Vec<_> = Tapback::ALL.iter().map(|tapback| tapback.glyph()).collect();
        assert_eq!(glyphs, ["❤️", "👍", "👎", "😂", "‼️", "❓"]);
    }
}
