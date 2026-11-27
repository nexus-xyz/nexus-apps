//! Streamed answers (`wb:delta`), as the Nexus app's `workbench/store.ts` (`draft`) keeps them: the text of each message
//! the agent is still writing, by session, until the message's final `text` event lands and the transcript shows it.
//! The workbench sends a delta at most every 50 ms; it is never stored. This build's own runner (`code_run.rs`), the
//! background service and each server (`machines.rs`) hand their deltas and events here, and the open session's view
//! draws the text after its transcript (`workspace/code_view.rs`).
//!
//! A delta with an `offset` carries only what follows that many UTF-16 units of the text sent before. One that does
//! not fit what is held (the session opened mid-answer) drops that message's text: the final event brings it whole.
//! A sub-agent's text (`parent`) streams apart from the main answer, as `subagent.ts` keeps it: its key is prefixed
//! with its call, and it shows inside the sub-agent's open row, never as the main answer.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use nexus_workbench::event::{Delta, TaskStatus, WbEvent};

/// The text each session's messages have streamed so far, in the order they began.
#[derive(Debug, Default)]
pub struct Streams(HashMap<String, Vec<Draft>>);

#[derive(Debug)]
struct Draft {
    key: String,
    text: String,
    /// Offsets count UTF-16 units. Keep the count so each suffix scans only its own text.
    utf16_len: u64,
}

impl Draft {
    fn new(key: String, text: &str) -> Self {
        Self {
            key,
            text: text.to_string(),
            utf16_len: utf16_len(text),
        }
    }
}

impl Streams {
    /// Takes a delta. True when what a session shows changed.
    pub fn delta(&mut self, delta: &Delta) -> bool {
        let key = draft_key(&delta.id, delta.parent.as_deref());
        let messages = self.0.entry(delta.thread_id.clone()).or_default();
        let at = messages.iter().position(|draft| draft.key == key);
        match (delta.offset, at) {
            (None, Some(at)) => messages[at] = Draft::new(key, &delta.text),
            (None, None) => messages.push(Draft::new(key, &delta.text)),
            (Some(offset), Some(at)) if messages[at].utf16_len == offset => {
                let draft = &mut messages[at];
                draft.text.push_str(&delta.text);
                draft.utf16_len += utf16_len(&delta.text);
            }
            // It does not follow what is held: wait for the final text.
            (Some(_), Some(at)) => {
                messages.remove(at);
            }
            (Some(0), None) => messages.push(Draft::new(key, &delta.text)),
            (Some(_), None) => return false,
        }
        if messages.is_empty() {
            self.0.remove(&delta.thread_id);
        }
        true
    }

    /// Takes a session's stored event: a message's final text replaces its stream, a turn's end drops what is left of the
    /// main answer, and a sub-agent's end what is left of its own.
    /// True when what the session shows changed.
    pub fn event(&mut self, thread: &str, event: &WbEvent) -> bool {
        let Some(messages) = self.0.get_mut(thread) else {
            return false;
        };
        let before = messages.len();
        match event {
            WbEvent::Text { id, parent, .. } => {
                let key = draft_key(id, parent.as_deref());
                messages.retain(|draft| draft.key != key)
            }
            // A turn's end drops the main answer's drafts; a sub-agent may write on past it (`store.ts`'s `ingest`).
            WbEvent::TurnEnd { .. } => messages.retain(|draft| draft.key.contains(SEP)),
            // A sub-agent's end drops its own drafts.
            WbEvent::Task {
                status,
                tool_use_id: Some(call),
                ..
            } if *status != TaskStatus::Running => {
                let prefix = draft_key("", Some(call));
                messages.retain(|draft| !draft.key.starts_with(&prefix))
            }
            WbEvent::Session { .. } | WbEvent::Error { .. } => messages.clear(),
            _ => {}
        }
        let changed = messages.len() != before;
        if messages.is_empty() {
            self.0.remove(thread);
        }
        changed
    }

    /// What session `thread` is still writing, its messages one after the other; None when nothing streams.
    pub fn text(&self, thread: &str) -> Option<String> {
        self.joined(thread, |key| !key.contains(SEP))
    }

    /// What the sub-agent of call `parent` in session `thread` is still writing; None when nothing streams.
    pub fn agent_text(&self, thread: &str, parent: &str) -> Option<String> {
        let prefix = draft_key("", Some(parent));
        self.joined(thread, |key| key.starts_with(&prefix))
    }

    fn joined(&self, thread: &str, mine: impl Fn(&str) -> bool) -> Option<String> {
        let messages = self.0.get(thread)?;
        let text = messages
            .iter()
            .filter(|draft| mine(&draft.key))
            .map(|draft| draft.text.as_str())
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        (!text.is_empty()).then_some(text)
    }
}

/// What parts a sub-agent's draft key: its call, then its message id (`subagent.ts`'s `SEP`).
const SEP: char = '\u{0}';

/// A draft's key: the main answer's is its message id; a sub-agent's is prefixed with its call (`draftKey`).
fn draft_key(id: &str, parent: Option<&str>) -> String {
    match parent {
        Some(parent) => format!("{parent}{SEP}{id}"),
        None => id.to_string(),
    }
}

/// A JavaScript string's length, which `offset` counts in.
fn utf16_len(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

/// Every session's streams, for every window.
pub fn streams() -> &'static Mutex<Streams> {
    static STREAMS: OnceLock<Mutex<Streams>> = OnceLock::new();
    STREAMS.get_or_init(Mutex::default)
}

/// Takes a delta into [`streams`]. True when what a session shows changed.
pub fn delta(delta: &Delta) -> bool {
    streams().lock().is_ok_and(|mut streams| streams.delta(delta))
}

/// Takes a stored event into [`streams`]. True when what a session shows changed.
pub fn event(thread: &str, event: &WbEvent) -> bool {
    streams().lock().is_ok_and(|mut streams| streams.event(thread, event))
}

/// What session `thread` is still writing.
pub fn text(thread: &str) -> Option<String> {
    streams().lock().ok().and_then(|streams| streams.text(thread))
}

/// What each of `calls`' sub-agents in session `thread` is still writing, by call; the ones writing nothing left out.
pub fn agent_texts<'a>(thread: &str, calls: impl IntoIterator<Item = &'a str>) -> Vec<(String, String)> {
    let Ok(streams) = streams().lock() else {
        return Vec::new();
    };
    calls
        .into_iter()
        .filter_map(|call| streams.agent_text(thread, call).map(|text| (call.to_string(), text)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn delta(id: &str, text: &str, offset: Option<u64>) -> Delta {
        Delta {
            thread_id: "t".into(),
            id: id.into(),
            text: text.into(),
            parent: None,
            offset,
        }
    }

    #[test]
    fn grows_a_message_by_its_suffixes_until_its_final_text() {
        let mut streams = Streams::default();
        assert!(streams.delta(&delta("m1", "Héllo", None)));
        // "Héllo" is 5 UTF-16 units; the next piece follows them.
        assert!(streams.delta(&delta("m1", " wörld", Some(5))));
        assert_eq!(streams.text("t").as_deref(), Some("Héllo wörld"));
        assert!(streams.delta(&delta("m2", "Next", Some(0))));
        assert_eq!(streams.text("t").as_deref(), Some("Héllo wörld\n\nNext"));
        let landed = serde_json::from_value::<WbEvent>(
            json!({ "kind": "text", "id": "m1", "text": "Héllo wörld", "parent": null }),
        );
        assert!(streams.event("t", &landed.expect("event")));
        assert_eq!(streams.text("t").as_deref(), Some("Next"));
    }

    #[test]
    fn a_piece_that_does_not_fit_waits_for_the_final_text() {
        let mut streams = Streams::default();
        streams.delta(&delta("m1", "abc", None));
        assert!(streams.delta(&delta("m1", "xyz", Some(7))));
        assert_eq!(streams.text("t"), None);
        // Opened mid-answer: a suffix with nothing held shows nothing.
        assert!(!streams.delta(&delta("m2", "tail", Some(12))));
        assert_eq!(streams.text("t"), None);
    }

    #[test]
    fn suffix_offsets_count_surrogate_pairs_and_combining_characters() {
        let mut streams = Streams::default();
        assert!(streams.delta(&delta("m", "A🦀e\u{301}", None)));
        // A + the surrogate pair + e + the combining accent = five UTF-16 units.
        assert!(streams.delta(&delta("m", " 文🦀", Some(5))));
        assert!(streams.delta(&delta("m", "!", Some(9))));
        assert_eq!(streams.text("t").as_deref(), Some("A🦀e\u{301} 文🦀!"));
        assert!(streams.delta(&delta("m", "invalid", Some(9))));
        assert_eq!(streams.text("t"), None);
    }

    #[test]
    fn a_full_replacement_resets_the_suffix_offset() {
        let mut streams = Streams::default();
        streams.delta(&delta("m", "a longer draft", None));
        streams.delta(&delta("m", "🦀", None));
        assert!(streams.delta(&delta("m", "!", Some(2))));
        assert_eq!(streams.text("t").as_deref(), Some("🦀!"));
        assert!(streams.delta(&delta("m", "stale", Some(14))));
        assert_eq!(streams.text("t"), None);
        // A zero-offset suffix can begin a new draft after the mismatched one was dropped.
        assert!(streams.delta(&delta("m", "🦀", Some(0))));
        assert!(streams.delta(&delta("m", "?", Some(2))));
        assert_eq!(streams.text("t").as_deref(), Some("🦀?"));
    }

    #[test]
    fn a_sub_agents_text_and_a_turns_end() {
        let mut streams = Streams::default();
        let mut sub = delta("s1", "inside", None);
        sub.parent = Some("toolu_1".into());
        assert!(streams.delta(&sub));
        // It is the sub-agent's, never the main answer.
        assert_eq!(streams.text("t"), None);
        assert_eq!(streams.agent_text("t", "toolu_1").as_deref(), Some("inside"));
        assert_eq!(streams.agent_text("t", "toolu_2"), None);
        streams.delta(&delta("m1", "half an answer", None));
        let end = WbEvent::TurnEnd {
            turn_id: "turn-1".into(),
            status: nexus_workbench::event::TurnStatus::Interrupted,
            cost_usd: None,
            duration_ms: 1,
            error: None,
            cursor: None,
        };
        assert!(streams.event("t", &end));
        assert_eq!(streams.text("t"), None);
        // A sub-agent writes on past the turn, until it ends.
        assert_eq!(streams.agent_text("t", "toolu_1").as_deref(), Some("inside"));
        let ended = serde_json::from_value::<WbEvent>(json!({
            "kind": "task", "taskId": "a1", "toolUseId": "toolu_1", "taskType": "local_agent", "agentType": null,
            "status": "completed", "description": "", "toolCount": null, "totalTokens": null, "durationMs": null
        }));
        assert!(streams.event("t", &ended.expect("event")));
        assert_eq!(streams.agent_text("t", "toolu_1"), None);
    }

    /// `subagent.test.ts`'s "the store drops a sub-agent's draft when its final text lands".
    #[test]
    fn drops_a_sub_agents_draft_when_its_final_text_lands() {
        let mut streams = Streams::default();
        streams.delta(&delta("m1:0", "Main so far", None));
        let mut sub = delta("s1:0", "Sub so", None);
        sub.parent = Some("task_1".into());
        streams.delta(&sub);
        let landed = |id: &str, parent: Option<&str>| -> WbEvent {
            serde_json::from_value(json!({ "kind": "text", "id": id, "text": "done", "parent": parent }))
                .expect("event")
        };
        // The main answer's text with the same id leaves the sub-agent's draft be, and the other way round.
        assert!(!streams.event("t", &landed("s1:0", None)));
        assert!(streams.event("t", &landed("s1:0", Some("task_1"))));
        assert_eq!(streams.agent_text("t", "task_1"), None);
        assert_eq!(streams.text("t").as_deref(), Some("Main so far"));
        assert!(streams.event("t", &landed("m1:0", None)));
        assert_eq!(streams.text("t"), None);
    }
}
