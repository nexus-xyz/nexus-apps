//! Memory-only projection of the shared `synced_thread_page` reader. No keys or credentials enter this model.
use nexus_workbench::{
    event::{SessionAgent, WbEvent},
    session_sync::Status,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use uuid::Uuid;

pub(super) const MAX_EVENTS: usize = 1000;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Session {
    pub thread_id: String,
    pub person_id: String,
    pub agent: Option<SessionAgent>,
    pub status: Option<Status>,
    pub last_seq: Option<i64>,
    // Required: an incomplete answer must never make a private thread public.
    pub private: bool,
    #[serde(default)]
    pub followers: Vec<String>,
}
impl Session {
    pub fn visible_to(&self, person: &str) -> bool {
        !person.is_empty() && !self.person_id.is_empty() && (!self.private || self.person_id == person)
    }
    pub fn ended(&self) -> bool {
        matches!(
            self.status,
            Some(Status::Completed | Status::Interrupted | Status::Error)
        )
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Title {
    pub title: Option<String>,
    pub project_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "state")]
pub(super) enum Opened {
    #[serde(rename = "open")]
    Open { seq: i64, thread: Title, event: WbEvent },
    #[serde(rename = "local")]
    Local { seq: i64 },
    #[serde(rename = "no-key")]
    NoKey { seq: i64 },
    #[serde(rename = "unreadable")]
    Unreadable { seq: i64 },
}
impl Opened {
    pub fn seq(&self) -> i64 {
        match self {
            Self::Open { seq, .. } | Self::Local { seq } | Self::NoKey { seq } | Self::Unreadable { seq } => *seq,
        }
    }
}

#[derive(Deserialize)]
pub(super) struct Page {
    pub thread: Session,
    pub events: Vec<Opened>,
    pub more: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Connection {
    Connecting,
    Live,
    Reconnecting,
    Unavailable,
}

pub(super) struct Transcript {
    // A fresh incarnation on every open/reset prevents A -> B -> A replies from being accepted.
    pub incarnation: Uuid,
    pub selected: Option<String>,
    pub session: Option<Session>,
    pub events: BTreeMap<i64, Opened>,
    pub more: bool,
    pub connection: Connection,
    pub note: Option<&'static str>,
}
impl Default for Transcript {
    fn default() -> Self {
        Self {
            incarnation: Uuid::new_v4(),
            selected: None,
            session: None,
            events: BTreeMap::new(),
            more: false,
            connection: Connection::Connecting,
            note: None,
        }
    }
}
#[derive(Clone)]
pub(super) struct Ticket {
    incarnation: Uuid,
    thread: String,
}
impl Transcript {
    pub fn open(&mut self, thread: String) {
        *self = Self {
            selected: Some(thread),
            ..Self::default()
        };
    }
    pub fn ticket(&self) -> Option<Ticket> {
        Some(Ticket {
            incarnation: self.incarnation,
            thread: self.selected.clone()?,
        })
    }
    pub fn current(&self, ticket: &Ticket) -> bool {
        self.incarnation == ticket.incarnation && self.selected.as_deref() == Some(ticket.thread.as_str())
    }
    pub fn deny(&mut self) {
        self.incarnation = Uuid::new_v4();
        self.events.clear();
        self.session = None;
        self.more = false;
        self.connection = Connection::Unavailable;
        self.note = Some("This session is no longer available to this account.");
    }
    pub fn apply(&mut self, ticket: &Ticket, page: Page, person: &str, older: bool) -> bool {
        if !self.current(ticket) {
            return false;
        }
        if page.thread.thread_id != ticket.thread || !page.thread.visible_to(person) {
            self.deny();
            return false;
        }
        let invalid = page.events.len() > 100
            || page.events.iter().any(|event| event.seq() < 0)
            || page
                .events
                .windows(2)
                .any(|pair| pair[0].seq().checked_add(1) != Some(pair[1].seq()));
        if invalid {
            self.events.clear();
            self.session = None;
            self.note = Some("The session reader returned an unreadable page. Retrying.");
            self.connection = Connection::Reconnecting;
            return false;
        }
        // Initial/older pages and incremental pages must join the held interval. Never skip missing events.
        if let (Some(first), Some(last), Some(incoming_first), Some(incoming_last)) = (
            self.events.first_key_value(),
            self.events.last_key_value(),
            page.events.first(),
            page.events.last(),
        ) && (incoming_first.seq() > last.0.saturating_add(1) || incoming_last.seq().saturating_add(1) < *first.0)
        {
            self.note = Some("Waiting for missing session events. Retrying.");
            self.connection = Connection::Reconnecting;
            return false;
        }
        if self.events.is_empty() || older {
            self.more = page.more;
        }
        self.session = Some(page.thread);
        for event in page.events {
            self.events.insert(event.seq(), event);
        }
        while self.events.len() > MAX_EVENTS {
            self.events.pop_first();
            self.more = true;
        }
        self.note = None;
        true
    }
    /// Cycle through every held locked page; permanent corruption or a missing key cannot starve later pages.
    pub fn next_locked(&self, cursor: Option<i64>) -> Option<i64> {
        let locked = |event: &&Opened| matches!(event, Opened::NoKey { .. } | Opened::Unreadable { .. });
        self.events
            .values()
            .filter(locked)
            .map(Opened::seq)
            .find(|seq| cursor.is_none_or(|cursor| *seq >= cursor))
            .or_else(|| self.events.values().filter(locked).map(Opened::seq).next())
    }
    pub fn title(&self) -> Option<&Title> {
        self.events.values().rev().find_map(|event| match event {
            Opened::Open { thread, .. } => Some(thread),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page(owner: &str, private: bool, status: Status, seq: i64) -> Page {
        // The body is serialized from the production WbEvent; this is the actual shared reader response shape.
        let event = WbEvent::Text {
            id: "reply".into(),
            text: "Fixture response".into(),
            parent: None,
        };
        serde_json::from_value(json!({
            "thread": {"threadId":"fixture-thread","personId":owner,"agent":SessionAgent::Codex,"status":status,"private":private,"followers":["reader"]},
            "events":[{"state":"open","seq":seq,"at":1,"thread":{"title":"Fixture title","projectName":"Fixture project"},"event":event}],"more":false
        })).unwrap()
    }
    #[test]
    fn opens_shared_types_and_preserves_ended_session_history() {
        let mut model = Transcript::default();
        model.open("fixture-thread".into());
        let ticket = model.ticket().unwrap();
        assert!(model.apply(&ticket, page("owner", false, Status::Running, 10), "reader", false));
        assert!(model.apply(&ticket, page("owner", false, Status::Completed, 11), "reader", false));
        assert!(model.session.as_ref().unwrap().ended());
        assert_eq!(model.events.len(), 2);
        assert_eq!(model.title().unwrap().title.as_deref(), Some("Fixture title"));
    }
    #[test]
    fn privacy_change_erases_plaintext_and_invalidates_outstanding_reads() {
        let mut model = Transcript::default();
        model.open("fixture-thread".into());
        let ticket = model.ticket().unwrap();
        assert!(model.apply(&ticket, page("owner", false, Status::Running, 0), "reader", false));
        assert!(!model.apply(&ticket, page("owner", true, Status::Running, 1), "reader", false));
        assert!(model.events.is_empty());
        assert!(model.session.is_none());
        assert!(!model.current(&ticket));
        assert!(!model.apply(&ticket, page("owner", false, Status::Running, 1), "reader", false));
    }
    #[test]
    fn own_private_session_is_readable_but_other_owners_are_rejected() {
        assert!(page("reader", true, Status::Running, 0).thread.visible_to("reader"));
        assert!(!page("owner", true, Status::Running, 0).thread.visible_to("reader"));
    }
    #[test]
    fn rejects_old_account_even_after_reopening_the_same_thread() {
        let mut model = Transcript::default();
        model.open("fixture-thread".into());
        let old = model.ticket().unwrap();
        model = Transcript::default();
        model.open("fixture-thread".into());
        assert!(!model.apply(&old, page("owner", false, Status::Running, 0), "reader", false));
        assert!(model.events.is_empty());
    }
    #[test]
    fn membership_refusal_erases_content_and_late_reconnect_cannot_restore_it() {
        let mut model = Transcript::default();
        model.open("fixture-thread".into());
        let ticket = model.ticket().unwrap();
        model.apply(&ticket, page("owner", false, Status::Running, 0), "reader", false);
        model.deny();
        assert!(model.events.is_empty());
        assert!(!model.current(&ticket));
        assert_eq!(model.connection, Connection::Unavailable);
    }
    #[test]
    fn reconnect_deduplicates_and_refuses_gaps() {
        let mut model = Transcript::default();
        model.open("fixture-thread".into());
        let ticket = model.ticket().unwrap();
        model.apply(&ticket, page("owner", false, Status::Running, 10), "reader", false);
        model.connection = Connection::Reconnecting;
        assert!(!model.apply(&ticket, page("owner", false, Status::Running, 12), "reader", false));
        assert_eq!(model.events.len(), 1);
        assert!(model.apply(&ticket, page("owner", false, Status::Running, 11), "reader", false));
        assert!(model.apply(&ticket, page("owner", false, Status::Running, 11), "reader", false));
        assert_eq!(model.events.len(), 2);
    }
    #[test]
    fn locked_and_local_events_have_no_plaintext_and_incomplete_privacy_is_invalid() {
        for state in ["no-key", "local", "unreadable"] {
            let event: Opened = serde_json::from_value(json!({"state":state,"seq":0,"at":1})).unwrap();
            assert!(!matches!(event, Opened::Open { .. }));
        }
        assert!(serde_json::from_value::<Session>(json!({"threadId":"t","personId":"owner"})).is_err());
    }
    #[test]
    fn late_team_key_replaces_locked_events_without_changing_sequence() {
        let mut model = Transcript::default();
        model.open("fixture-thread".into());
        let ticket = model.ticket().unwrap();
        let mut locked = page("owner", false, Status::Running, 0);
        locked.events = vec![Opened::NoKey { seq: 0 }];
        assert!(model.apply(&ticket, locked, "reader", false));
        assert!(model.title().is_none());
        assert!(model.apply(&ticket, page("owner", false, Status::Running, 0), "reader", false));
        assert!(model.title().is_some());
        assert_eq!(model.events.len(), 1);
    }
    #[test]
    fn invalid_or_wrong_thread_pages_cannot_show_content() {
        let mut model = Transcript::default();
        model.open("fixture-thread".into());
        let ticket = model.ticket().unwrap();
        assert!(!model.apply(&ticket, page("owner", false, Status::Running, -1), "reader", false));
        assert!(model.events.is_empty());
        let mut wrong = page("owner", false, Status::Running, 0);
        wrong.thread.thread_id = "other".into();
        assert!(!model.apply(&ticket, wrong, "reader", false));
        assert!(model.events.is_empty());
        assert!(!page("owner", false, Status::Running, 0).thread.visible_to(""));
    }
    #[test]
    fn memory_is_bounded_while_live_updates_keep_the_newest_events() {
        let mut model = Transcript::default();
        model.open("fixture-thread".into());
        let ticket = model.ticket().unwrap();
        for seq in 0..=(MAX_EVENTS as i64) {
            assert!(model.apply(&ticket, page("owner", false, Status::Running, seq), "reader", false));
        }
        assert_eq!(model.events.len(), MAX_EVENTS);
        assert_eq!(model.events.first_key_value().unwrap().0, &1);
        assert_eq!(model.events.last_key_value().unwrap().0, &(MAX_EVENTS as i64));
        assert!(model.more);
    }
    #[test]
    fn corrupt_or_missing_early_events_cannot_starve_later_key_retries() {
        let mut model = Transcript::default();
        model.events.insert(0, Opened::Unreadable { seq: 0 });
        model.events.insert(100, Opened::NoKey { seq: 100 });
        model.events.insert(200, Opened::NoKey { seq: 200 });
        assert_eq!(model.next_locked(None), Some(0));
        assert_eq!(model.next_locked(Some(100)), Some(100));
        assert_eq!(model.next_locked(Some(200)), Some(200));
        assert_eq!(model.next_locked(Some(300)), Some(0));
        model.events.remove(&100);
        model.events.remove(&200);
        assert_eq!(model.next_locked(Some(300)), Some(0));
    }
}
