//! The person's decisions on waiting writes in the thread (brand v5 A9), without GPUI: which card is being
//! approved or declined, how each ended, and "Always allow this kind". The card is `thread_cards.rs`; the words and
//! the strict reads are `nexus_desktop_core::agent_cards`'s, as the kit's `agent-writes.ts`.

use std::collections::{HashMap, HashSet};

use nexus_desktop_core::agent::Message;
use nexus_desktop_core::agent_cards::{self as cards, Decision, EditField, WriteDecided};
use serde_json::Value;

use crate::agent::{Call, Chunk};

/// Where a card's decision stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Sent; the agent API has not answered.
    Working(Decision),
    /// How it ended: written, declined, failed or still running.
    Ended(WriteDecided),
    /// The agent API refused it, in the person's words. The card offers its buttons again.
    Refused(String),
}

/// Every card's decision in this thread.
#[derive(Debug, Default)]
pub struct Decisions {
    phases: HashMap<String, Phase>,
    always: HashSet<String>,
    /// The answer as it arrives, by card: its status and body.
    reading: HashMap<String, (Option<u16>, Vec<u8>)>,
    /// Edit's fields, open on a card (A24·b).
    editing: HashMap<String, Editing>,
    /// An edit's answer as it arrives, by the card it edits.
    edit_reading: HashMap<String, (Option<u16>, Vec<u8>)>,
    /// The edited card that replaces a card, by the card it replaces.
    replaced: HashMap<String, Message>,
}

/// A card's Edit while it is open (A24·b): its fields, the text they started with, and where Save stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Editing {
    pub fields: Vec<EditField>,
    pub start: Vec<(String, String)>,
    /// Save was pressed; the agent API has not answered.
    pub saving: bool,
    /// Why the last Save did not go, in the person's words.
    pub problem: Option<String>,
}

impl Decisions {
    /// Edit's fields are open on `card`.
    pub fn editing(&self, card: &str) -> Option<&Editing> {
        self.editing.get(card)
    }

    /// Open Edit on `card` with its fields and their starting text; false while it is decided or has no fields.
    pub fn begin_edit(&mut self, card: &str, fields: Vec<EditField>, start: Vec<(String, String)>) -> bool {
        if fields.is_empty() || matches!(self.phases.get(card), Some(Phase::Working(_) | Phase::Ended(_))) {
            return false;
        }
        self.editing.insert(
            card.to_string(),
            Editing {
                fields,
                start,
                saving: false,
                problem: None,
            },
        );
        true
    }

    /// Close Edit without saving.
    pub fn cancel_edit(&mut self, card: &str) {
        if self.editing.get(card).is_some_and(|editing| !editing.saving) {
            self.editing.remove(card);
        }
    }

    /// Save `card`'s fields as typed: the call to make, with only the changed fields; or None, with the reason on the
    /// card, when nothing changed. None too while a Save is sent.
    pub fn save_edit(&mut self, card: &str, values: &[(String, String)]) -> Option<Call> {
        let editing = self.editing.get_mut(card)?;
        if editing.saving {
            return None;
        }
        let changes = cards::write_edit_changes(&editing.fields, &editing.start, values);
        if changes.is_empty() {
            editing.problem = Some("Change a field, or approve it as it is.".into());
            return None;
        }
        let path = cards::write_edit_path(card)?;
        editing.saving = true;
        editing.problem = None;
        self.edit_reading.insert(card.to_string(), (None, Vec::new()));
        Some(Call::EditWrite {
            path,
            body: cards::write_edit_body(&changes),
        })
    }

    /// The next piece of `card`'s edit answer.
    pub fn edit_chunk(&mut self, card: &str, chunk: Chunk) {
        let Some((status, body)) = self.edit_reading.get_mut(card) else {
            return;
        };
        match chunk {
            Chunk::Head { status: code, .. } => *status = Some(code),
            Chunk::Data(bytes) => body.extend_from_slice(&bytes),
            Chunk::Broken => self.edit_answered(card, 502, None),
            Chunk::End => {
                let Some((status, body)) = self.edit_reading.remove(card) else {
                    return;
                };
                let body: Option<Value> = serde_json::from_slice(&body).ok();
                self.edit_answered(card, status.unwrap_or(502), body.as_ref());
            }
        }
    }

    /// `card`'s whole edit answer. The edited card replaces it and Edit closes; a refusal stays on the card.
    pub fn edit_answered(&mut self, card: &str, status: u16, body: Option<&Value>) {
        self.edit_reading.remove(card);
        let ok = (200..300).contains(&status);
        match body.filter(|_| ok).and_then(cards::write_edited_of) {
            Some((_, message)) => {
                self.editing.remove(card);
                self.always.remove(card);
                self.replaced.insert(card.to_string(), message);
            }
            None => {
                if let Some(editing) = self.editing.get_mut(card) {
                    editing.saving = false;
                    editing.problem = Some(cards::write_edit_problem(if ok { 502 } else { status }, body));
                }
            }
        }
    }

    /// `message` as the thread shows it: the edited card in place of the one it replaces.
    pub fn resolve<'a>(&'a self, message: &'a Message) -> &'a Message {
        match message {
            Message::PendingAction { card, .. } => self.replaced.get(card).unwrap_or(message),
            _ => message,
        }
    }
}

impl Decisions {
    pub fn phase(&self, card: &str) -> Option<&Phase> {
        self.phases.get(card)
    }

    /// The card ended: nothing waits on the person any more.
    pub fn ended(&self, card: &str) -> bool {
        matches!(self.phases.get(card), Some(Phase::Ended(_)))
    }

    /// "Always allow this kind" is ticked on the card.
    pub fn always(&self, card: &str) -> bool {
        self.always.contains(card)
    }

    pub fn toggle_always(&mut self, card: &str) {
        if !self.always.remove(card) {
            self.always.insert(card.to_string());
        }
    }

    /// Decide `card`: the call to make, or None while it is sent or once it ended, or for a card id the agent API
    /// could not have written. Always applies to an approve only.
    pub fn start(&mut self, card: &str, decision: Decision) -> Option<Call> {
        if matches!(self.phases.get(card), Some(Phase::Working(_) | Phase::Ended(_))) {
            return None;
        }
        let path = cards::write_decision_path(card, decision)?;
        let body = cards::write_decision_body(decision, self.always(card));
        self.phases.insert(card.to_string(), Phase::Working(decision));
        self.reading.insert(card.to_string(), (None, Vec::new()));
        Some(Call::Decide { path, body })
    }

    /// The next piece of `card`'s answer.
    pub fn chunk(&mut self, card: &str, chunk: Chunk) {
        let Some((status, body)) = self.reading.get_mut(card) else {
            return;
        };
        match chunk {
            Chunk::Head { status: code, .. } => *status = Some(code),
            Chunk::Data(bytes) => body.extend_from_slice(&bytes),
            Chunk::Broken => {
                self.reading.remove(card);
                self.phases.insert(
                    card.to_string(),
                    Phase::Refused(cards::write_decision_problem(502, None)),
                );
            }
            Chunk::End => {
                let Some((status, body)) = self.reading.remove(card) else {
                    return;
                };
                let body: Option<Value> = serde_json::from_slice(&body).ok();
                self.answered(card, status.unwrap_or(502), body.as_ref());
            }
        }
    }

    /// `card`'s whole answer: its status and its JSON body, when it had one. A view that gathers its answers (an
    /// errand's thread) ends the decision here.
    pub fn answered(&mut self, card: &str, status: u16, body: Option<&Value>) {
        self.reading.remove(card);
        let phase = if (200..300).contains(&status) {
            match body.and_then(cards::write_decided_of) {
                Some(decided) => Phase::Ended(decided),
                None => Phase::Refused(cards::write_decision_problem(502, None)),
            }
        } else {
            Phase::Refused(cards::write_decision_problem(status, body))
        };
        self.phases.insert(card.to_string(), phase);
    }

    /// A new conversation: no card of the old one stays.
    pub fn clear(&mut self) {
        *self = Decisions::default();
    }
}

#[cfg(test)]
mod tests {
    use super::{Call, Chunk, Decision, Decisions, Phase};

    fn card() -> String {
        format!("card_{}", "b".repeat(32))
    }

    fn answer(decisions: &mut Decisions, card: &str, status: u16, body: &str) {
        decisions.chunk(
            card,
            Chunk::Head {
                status,
                content_type: "application/json".into(),
            },
        );
        decisions.chunk(card, Chunk::Data(body.as_bytes().to_vec()));
        decisions.chunk(card, Chunk::End);
    }

    #[test]
    fn an_approve_sends_once_with_always_and_ends_with_the_agents_words() {
        let card = card();
        let mut decisions = Decisions::default();
        decisions.toggle_always(&card);
        let Some(Call::Decide { path, body }) = decisions.start(&card, Decision::Approve) else {
            panic!("decides")
        };
        assert_eq!(path, format!("/agent/writes/{card}/approve"));
        assert_eq!(body, r#"{"always":true}"#);
        assert_eq!(decisions.start(&card, Decision::Decline), None, "one at a time");
        assert_eq!(decisions.phase(&card), Some(&Phase::Working(Decision::Approve)));
        answer(
            &mut decisions,
            &card,
            200,
            &format!(r#"{{"card":"{card}","decision":"approve","outcome":"written","text":"Done.","always":true}}"#),
        );
        assert!(decisions.ended(&card));
        let Some(Phase::Ended(decided)) = decisions.phase(&card) else {
            panic!("ended")
        };
        assert_eq!((decided.text.as_str(), decided.always), ("Done.", true));
        assert_eq!(decisions.start(&card, Decision::Approve), None, "decided once");
    }

    #[test]
    fn a_refusal_says_why_and_offers_the_card_again() {
        let card = card();
        let mut decisions = Decisions::default();
        decisions.toggle_always(&card);
        decisions.toggle_always(&card);
        let Some(Call::Decide { body, .. }) = decisions.start(&card, Decision::Decline) else {
            panic!("decides")
        };
        assert_eq!(body, "{}");
        answer(&mut decisions, &card, 410, r#"{"error":"expired"}"#);
        assert!(matches!(decisions.phase(&card), Some(Phase::Refused(text)) if text.contains("expired")));
        assert!(decisions.start(&card, Decision::Decline).is_some(), "it may try again");
        decisions.chunk(&card, Chunk::Broken);
        assert!(matches!(decisions.phase(&card), Some(Phase::Refused(_))));
        // An answer that is not one, and a card id the API could not have written.
        decisions.start(&card, Decision::Decline);
        answer(&mut decisions, &card, 200, "{}");
        assert!(matches!(decisions.phase(&card), Some(Phase::Refused(_))));
        assert_eq!(decisions.start("card_1/../../agent/memory", Decision::Approve), None);
        decisions.clear();
        assert_eq!(decisions.phase(&card), None);
    }

    fn email(card: &str) -> nexus_desktop_core::agent::Message {
        serde_json::from_value(serde_json::json!({
            "kind": "pending_action", "card": card, "inChat": "approve-or-decline", "approveUrl": "https://n.test/a", "detail": "d",
            "write": { "connector": "outlook", "operation": "send", "summary": "Send", "effect": "mutate", "arguments": { "to": ["a@x.io"], "body": "Hi." } },
            "edit": [ { "name": "to", "label": "To", "kind": "addresses" }, { "name": "body", "label": "Message", "kind": "lines" } ]
        }))
        .expect("a card")
    }

    #[test]
    fn an_edit_sends_the_changed_fields_and_the_edited_card_replaces_the_card() {
        use nexus_desktop_core::agent_cards as cards;
        let card = card();
        let message = email(&card);
        let mut decisions = Decisions::default();
        assert!(decisions.begin_edit(
            &card,
            cards::write_edit_fields(&message),
            cards::write_edit_values(&message)
        ));
        let same = cards::write_edit_values(&message);
        assert_eq!(decisions.save_edit(&card, &same), None, "nothing changed");
        assert!(
            decisions
                .editing(&card)
                .is_some_and(|editing| editing.problem.is_some())
        );
        let typed = vec![
            ("to".to_string(), "a@x.io".to_string()),
            ("body".to_string(), "Hello.".to_string()),
        ];
        let Some(Call::EditWrite { path, body }) = decisions.save_edit(&card, &typed) else {
            panic!("edits")
        };
        assert_eq!(path, format!("/agent/writes/{card}"));
        assert_eq!(body, r#"{"arguments":{"body":"Hello."}}"#);
        assert_eq!(decisions.save_edit(&card, &typed), None, "one at a time");
        // A refusal keeps Edit open with the reason, and Save may go again.
        answer_edit(
            &mut decisions,
            &card,
            400,
            r#"{"error":"bad_request","detail":"body is text."}"#,
        );
        let editing = decisions.editing(&card).expect("still open");
        assert_eq!(
            (editing.saving, editing.problem.as_deref()),
            (false, Some("body is text."))
        );
        assert!(decisions.save_edit(&card, &typed).is_some());
        let new = format!("card_{}", "c".repeat(32));
        let action = serde_json::to_value(serde_json::json!({
            "kind": "pending_action", "card": new, "edited": true, "inChat": "approve-or-decline",
            "write": { "connector": "outlook", "operation": "send", "summary": "Send", "effect": "mutate" }
        }))
        .expect("json");
        answer_edit(
            &mut decisions,
            &card,
            200,
            &serde_json::json!({ "replaces": card, "action": action }).to_string(),
        );
        assert!(decisions.editing(&card).is_none());
        let shown = decisions.resolve(&message);
        assert!(
            matches!(shown, nexus_desktop_core::agent::Message::PendingAction { card, edited: true, .. } if *card == new)
        );
        assert_eq!(decisions.resolve(&email(&new)), &email(&new), "an unedited card stays");
    }

    fn answer_edit(decisions: &mut Decisions, card: &str, status: u16, body: &str) {
        decisions.edit_chunk(
            card,
            Chunk::Head {
                status,
                content_type: "application/json".into(),
            },
        );
        decisions.edit_chunk(card, Chunk::Data(body.as_bytes().to_vec()));
        decisions.edit_chunk(card, Chunk::End);
    }
}
