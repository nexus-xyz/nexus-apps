//! Composer drafts: each Code session, each chat and each New session target keeps its own unsent prompt, across
//! switches and restarts.
//!
//! A session's draft is keyed by its id; New session's by [`new_session_key`], so each organization,
//! machine, project and window has its own. A draft never expires and has no cap: it lasts until it is sent or emptied. Sending takes the sent text and attachments out, so an empty draft is dropped.
//!
//! No I/O or screen code lives here. Native storage writes full snapshots; legacy JSON helpers cover text only.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Typing is saved this long (ms) after the last key (`SAVE_DELAY`).
pub const SAVE_DELAY_MS: u64 = 300;
/// And at most this long (ms) after the first unsaved key, so steady typing still saves (`SAVE_MAX_WAIT`).
pub const SAVE_MAX_WAIT_MS: u64 = 1_000;
/// New session's machine when it starts on Nexus Cloud (`new-session.tsx`'s `CLOUD`).
pub const CLOUD: &str = "cloud";
/// New session's project when No project is picked: a plain chat (`new-session.tsx`'s `PLAIN_DRAFT`).
pub const PLAIN_DRAFT: &str = ":plain";
/// The app's main window: its New session keys leave the window out (`newSessionDraftKey`'s default).
pub const MAIN_WINDOW: &str = "main";

/// New session's draft key: one for each organization, machine, project and app window (`newSessionDraftKey`).
/// `machine` is `None` for this computer.
pub fn new_session_key(org: &str, machine: Option<&str>, project: Option<&str>, window: &str) -> String {
    let parts = if window == MAIN_WINDOW {
        vec![Some(org), machine, project]
    } else {
        vec![Some(org), machine, project, Some(window)]
    };
    format!("new:{}", serde_json::to_string(&parts).unwrap_or_default())
}

/// How long (ms) to wait before saving typing that started `pending_since` and changed again at `now` (ms): the
/// usual delay, cut so no edit waits more than [`SAVE_MAX_WAIT_MS`] (`setDraft`'s timer).
pub fn save_delay_ms(pending_since: u64, now: u64) -> u64 {
    SAVE_DELAY_MS.min((pending_since + SAVE_MAX_WAIT_MS).saturating_sub(now))
}

/// Remove only the accepted prefix. Text edited inside it stays for the person to resolve (`afterSend`).
pub fn after_send_text(current: &str, submitted: &str) -> String {
    if submitted.is_empty() {
        return current.to_string();
    }
    current
        .strip_prefix(submitted)
        .map_or_else(|| current.to_string(), |left| left.trim_start().to_string())
}

/// Native persistence belongs to one Gateway, organization and sign-in identity.
/// An unknown identity uses memory only. The on-disk name never contains the identity marker.
pub fn storage_scope(gateway: &str, org: &str, marker: Option<&str>) -> Option<String> {
    let marker = marker.filter(|marker| !marker.is_empty())?;
    if org.is_empty() {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(b"nexus-composer-drafts-v1\0");
    for part in [gateway, org, marker] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    Some(hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

/// A draft the store can hold: the composer's text and, in memory only, what is attached to it.
pub trait ComposerDraft: Default {
    fn text(&self) -> &str;
    fn set_text(&mut self, text: String);
    /// Attachments or anything else besides the text.
    fn has_attachments(&self) -> bool {
        false
    }
    /// The retained copy while the draft is on screen. Native byte stores override this with cheap Arc clones.
    fn snapshot(&self) -> Self {
        let mut draft = Self::default();
        draft.set_text(self.text().to_string());
        draft
    }
    /// Nothing written and nothing attached.
    fn is_empty(&self) -> bool {
        self.text().trim().is_empty() && !self.has_attachments()
    }
}

/// A text-only draft: what is stored.
impl ComposerDraft for String {
    fn text(&self) -> &str {
        self
    }
    fn set_text(&mut self, text: String) {
        *self = text;
    }
}

/// Every draft not on screen, by key, and the text of the one on screen.
#[derive(Debug, Clone)]
pub struct Drafts<D> {
    entries: HashMap<String, D>,
}

impl<D> Default for Drafts<D> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl<D: ComposerDraft> Drafts<D> {
    /// The drafts as stored: `{key: text}`, or the previous desktop app's `{key: {text, ...}}`. What cannot be read is left out,
    /// and so is a draft with no text: its attachments did not last.
    pub fn from_json(text: &str) -> Self {
        let parsed: serde_json::Map<String, Value> = serde_json::from_str(text).unwrap_or_default();
        let entries = parsed
            .into_iter()
            .filter_map(|(key, value)| {
                let text = match value {
                    Value::String(text) => text,
                    Value::Object(mut entry) => match entry.remove("text") {
                        Some(Value::String(text)) => text,
                        _ => return None,
                    },
                    _ => return None,
                };
                (!text.trim().is_empty()).then(|| {
                    let mut draft = D::default();
                    draft.set_text(text);
                    (key, draft)
                })
            })
            .collect();
        Self { entries }
    }

    /// The drafts to store, `{key: text}` in key order: the text of each one `keep` takes. Attachments are left out.
    pub fn to_json(&self, keep: impl Fn(&str) -> bool) -> String {
        let kept: BTreeMap<&str, &str> = self
            .entries
            .iter()
            .filter(|(key, draft)| keep(key) && !draft.text().trim().is_empty())
            .map(|(key, draft)| (key.as_str(), draft.text()))
            .collect();
        serde_json::to_string(&kept).unwrap_or_else(|_| "{}".into())
    }

    pub fn get(&self, key: &str) -> Option<&D> {
        self.entries.get(key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut D> {
        self.entries.get_mut(key)
    }

    /// Keep `draft` under `key`; an empty one is dropped.
    pub fn put(&mut self, key: &str, draft: D) {
        if draft.is_empty() {
            self.entries.remove(key);
        } else {
            self.entries.insert(key.to_string(), draft);
        }
    }

    /// Take `key`'s draft out to show it; an empty draft when there is none. Its text stays, so it is still stored
    /// while it is on screen; [`Drafts::set_text`] follows the edits.
    pub fn open(&mut self, key: &str) -> D {
        let draft = self.entries.remove(key).unwrap_or_default();
        if !draft.is_empty() {
            self.entries.insert(key.to_string(), draft.snapshot());
        }
        draft
    }

    /// The text on screen under `key` changed. Returns whether what is stored changed.
    pub fn set_text(&mut self, key: &str, text: &str) -> bool {
        match self.entries.get_mut(key) {
            Some(draft) if draft.text() == text => false,
            Some(draft) => {
                draft.set_text(text.to_string());
                if draft.is_empty() {
                    self.entries.remove(key);
                }
                true
            }
            None if text.trim().is_empty() => false,
            None => {
                let mut draft = D::default();
                draft.set_text(text.to_string());
                self.entries.insert(key.to_string(), draft);
                true
            }
        }
    }

    /// A draft's key changed (a new session got its id): its draft goes with it.
    pub fn rename(&mut self, from: &str, to: &str) {
        if let Some(draft) = self.entries.remove(from) {
            self.entries.insert(to.to_string(), draft);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, Clone, PartialEq)]
    struct Draft {
        text: String,
        images: usize,
    }

    impl ComposerDraft for Draft {
        fn text(&self) -> &str {
            &self.text
        }
        fn set_text(&mut self, text: String) {
            self.text = text;
        }
        fn has_attachments(&self) -> bool {
            self.images > 0
        }
    }

    fn draft(text: &str, images: usize) -> Draft {
        Draft {
            text: text.into(),
            images,
        }
    }

    #[test]
    fn native_storage_isolates_gateway_organization_and_identity() {
        let a = storage_scope("https://one.test", "org", Some("member-a")).expect("known identity");
        for (gateway, org, marker) in [
            ("https://two.test", "org", "member-a"),
            ("https://one.test", "other", "member-a"),
            ("https://one.test", "org", "member-b"),
        ] {
            assert_ne!(Some(a.clone()), storage_scope(gateway, org, Some(marker)));
        }
        assert_eq!(a.len(), 64);
        assert!(!a.contains("member"));
        assert_eq!(storage_scope("https://one.test", "org", None), None);
        assert_eq!(storage_scope("https://one.test", "", Some("member")), None);
    }

    #[test]
    fn accepting_a_send_preserves_later_or_changed_text() {
        assert_eq!(after_send_text("sent\n\nnext", "sent"), "next");
        assert_eq!(after_send_text("changed inside sent", "sent"), "changed inside sent");
        assert_eq!(after_send_text("  only images sent", ""), "  only images sent");
        assert_eq!(after_send_text("sent", "sent"), "");
    }

    // draft-store.test.ts: "separates new-session drafts by org, machine, project and window".
    #[test]
    fn separates_new_session_drafts_by_org_machine_project_and_window() {
        assert_eq!(new_session_key("o", None, None, MAIN_WINDOW), r#"new:["o",null,null]"#);
        assert_eq!(
            new_session_key("o", Some("m"), Some("p"), MAIN_WINDOW),
            r#"new:["o","m","p"]"#
        );
        assert_eq!(
            new_session_key("o", Some("m"), Some("p"), "w2"),
            r#"new:["o","m","p","w2"]"#
        );
        let keys = [
            new_session_key("o", None, Some("p"), MAIN_WINDOW),
            new_session_key("o2", None, Some("p"), MAIN_WINDOW),
            new_session_key("o", Some(CLOUD), Some("p"), MAIN_WINDOW),
            new_session_key("o", None, Some("p2"), MAIN_WINDOW),
            new_session_key("o", None, Some(PLAIN_DRAFT), MAIN_WINDOW),
        ];
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());
    }

    // draft-store.test.ts: "batches typing but saves within SAVE_MAX_WAIT".
    #[test]
    fn batches_typing_but_saves_within_the_max_wait() {
        assert_eq!(save_delay_ms(0, 0), SAVE_DELAY_MS);
        assert_eq!(save_delay_ms(0, 800), 200);
        assert_eq!(save_delay_ms(0, SAVE_MAX_WAIT_MS), 0);
        assert_eq!(save_delay_ms(0, SAVE_MAX_WAIT_MS + 500), 0);
    }

    #[test]
    fn each_key_keeps_its_own_draft_across_switches() {
        let mut drafts = Drafts::<Draft>::default();
        drafts.put("a", draft("for a", 1));
        drafts.put("b", draft("for b", 0));
        assert_eq!(drafts.open("a"), draft("for a", 1));
        // The text stays stored while it is on screen; the attachments are on screen only.
        assert_eq!(drafts.get("a"), Some(&draft("for a", 0)));
        assert_eq!(drafts.open("b"), draft("for b", 0));
        assert_eq!(drafts.open("c"), Draft::default());
    }

    #[test]
    fn an_empty_draft_is_dropped_but_attachments_alone_are_kept() {
        let mut drafts = Drafts::<Draft>::default();
        drafts.put("a", draft("  ", 0));
        assert!(drafts.is_empty());
        drafts.put("a", draft("", 2));
        assert_eq!(drafts.get("a"), Some(&draft("", 2)));
    }

    #[test]
    fn edits_follow_the_text_on_screen_and_sending_clears_it() {
        let mut drafts = Drafts::<Draft>::default();
        assert!(!drafts.set_text("a", ""));
        assert!(drafts.set_text("a", "hel"));
        assert!(!drafts.set_text("a", "hel"));
        assert!(drafts.set_text("a", "hello"));
        assert_eq!(drafts.get("a").map(Draft::text), Some("hello"));
        // Sent: the composer is empty, so nothing comes back after a restart.
        assert!(drafts.set_text("a", ""));
        assert!(drafts.get("a").is_none());
    }

    #[test]
    fn a_new_session_takes_its_draft_with_its_id() {
        let mut drafts = Drafts::<Draft>::default();
        drafts.set_text("draft-1", "typed while it started");
        drafts.rename("draft-1", "t_1");
        assert!(drafts.get("draft-1").is_none());
        assert_eq!(drafts.get("t_1").map(Draft::text), Some("typed while it started"));
    }

    // draft-store.test.ts: "restores text from the fallback but not the images".
    #[test]
    fn keeps_the_text_across_a_restart_but_not_the_attachments() {
        let mut drafts = Drafts::<Draft>::default();
        drafts.put("a", draft("text", 2));
        drafts.put("b", draft("", 1));
        drafts.put("draft-3", draft("starting", 0));
        let stored = drafts.to_json(|key| !key.starts_with("draft-"));
        assert_eq!(stored, r#"{"a":"text"}"#);
        let back = Drafts::<Draft>::from_json(&stored);
        assert_eq!(back.len(), 1);
        assert_eq!(back.get("a"), Some(&draft("text", 0)));
    }

    #[test]
    fn reads_the_old_apps_shape_and_skips_what_it_cannot_read() {
        let back = Drafts::<String>::from_json(
            r#"{"a":{"text":"old draft","images":[],"revision":3},"b":"plain","c":{"images":[]},"d":4,"e":"  "}"#,
        );
        assert_eq!(back.len(), 2);
        assert_eq!(back.get("a").map(String::as_str), Some("old draft"));
        assert_eq!(back.get("b").map(String::as_str), Some("plain"));
        assert!(Drafts::<String>::from_json("not json").is_empty());
    }
}
