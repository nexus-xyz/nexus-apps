//! The prompt stash: composer drafts parked with Mod+S so the person can write something else first, then brought back
//! from the stash menu or with Mod+S again. One stash for every session, newest first, at most [`MAX_STASH`].
//!
//! The previous desktop app kept the stash in its webview's localStorage (key [`STORAGE_KEY`]). The app stores
//! [`Stash::to_json`] on its own, in the same shape. Only the text lasts across a restart. The
//! attachments stay in memory, as the drafts' do: a screenshot's bytes would fill the store at once.
//!
//! No I/O and no screen code here: the app reads and writes the stored text and draws the menu.

use serde::{Deserialize, Serialize};

/// Where the previous desktop app kept the stash in localStorage.
pub const STORAGE_KEY: &str = "nexus.workbench.prompt-stash";
/// The most drafts kept; the oldest falls off.
pub const MAX_STASH: usize = 20;
/// The most of a draft's text stored, in UTF-16 units (JavaScript's `length`).
pub const MAX_TEXT: usize = 20_000;

/// A draft the stash can hold: the composer's text and what is attached to it.
pub trait StashDraft: Clone {
    fn text(&self) -> &str;
    /// The draft as it comes back after a restart: its text alone.
    fn from_text(text: String) -> Self;
    fn images(&self) -> usize {
        0
    }
    fn files(&self) -> usize {
        0
    }
    /// Comments on diff lines, sent ahead of the text.
    fn comments(&self) -> usize {
        0
    }
    /// Nothing written and nothing attached (`composer.tsx`'s `isEmpty`).
    fn is_empty(&self) -> bool {
        self.text().trim().is_empty() && self.images() == 0 && self.files() == 0 && self.comments() == 0
    }
}

/// A text-only draft: what a stored entry holds.
impl StashDraft for String {
    fn text(&self) -> &str {
        self
    }
    fn from_text(text: String) -> Self {
        text
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry<D> {
    pub id: String,
    /// When it was stashed (ms since the epoch).
    pub at: i64,
    pub draft: D,
}

/// An entry as stored: its text, never its attachments.
#[derive(Serialize, Deserialize)]
struct Stored {
    id: String,
    at: i64,
    text: String,
}

/// What Mod+S does (`onStashKey`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// Something is in the composer: stash it.
    Stash,
    /// The composer is empty and one draft is stashed: bring it back.
    Restore(String),
    /// The composer is empty and several are stashed: open the menu to choose.
    Menu,
    /// Nothing written and nothing stashed.
    None,
}

/// What Mod+S does with `draft` in the composer and `list` in the stash.
pub fn on_stash_key<D: StashDraft>(draft: &D, list: &[Entry<D>]) -> Key {
    if !draft.is_empty() {
        return Key::Stash;
    }
    match list {
        [] => Key::None,
        [one] => Key::Restore(one.id.clone()),
        _ => Key::Menu,
    }
}

/// The stash, newest first.
#[derive(Debug, Clone)]
pub struct Stash<D> {
    entries: Vec<Entry<D>>,
    counter: u64,
}

impl<D> Default for Stash<D> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            counter: 0,
        }
    }
}

impl<D: StashDraft> Stash<D> {
    /// The stash as stored (`load`). What cannot be read is left out, and so is an entry whose text is empty: a draft
    /// that was only attachments comes back empty after a restart.
    pub fn from_json(text: &str) -> Self {
        let parsed: Vec<serde_json::Value> = serde_json::from_str(text).unwrap_or_default();
        let entries = parsed
            .into_iter()
            .filter_map(|raw| serde_json::from_value::<Stored>(raw).ok())
            .filter(|stored| !stored.text.trim().is_empty())
            .take(MAX_STASH)
            .map(|stored| Entry {
                id: stored.id,
                at: stored.at,
                draft: D::from_text(stored.text),
            })
            .collect();
        Self { entries, counter: 0 }
    }

    /// The stash to store (`save`): each entry's text, cut at [`MAX_TEXT`]; entries with no text are left out.
    pub fn to_json(&self) -> String {
        let kept: Vec<Stored> = self
            .entries
            .iter()
            .filter(|entry| !entry.draft.text().trim().is_empty())
            .map(|entry| Stored {
                id: entry.id.clone(),
                at: entry.at,
                text: clip(entry.draft.text(), MAX_TEXT).to_string(),
            })
            .collect();
        serde_json::to_string(&kept).unwrap_or_else(|_| "[]".into())
    }

    pub fn entries(&self) -> &[Entry<D>] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn new_id(&mut self, now: i64) -> String {
        let id = format!("s{}{}", base36(now.max(0) as u64), base36(self.counter));
        self.counter += 1;
        id
    }

    /// Parks `draft` at the top (the oldest falls off past [`MAX_STASH`]). Nothing for an empty draft.
    pub fn stash(&mut self, draft: D, now: i64) -> Option<&Entry<D>> {
        if draft.is_empty() {
            return None;
        }
        let id = self.new_id(now);
        self.entries.insert(0, Entry { id, at: now, draft });
        self.entries.truncate(MAX_STASH);
        self.entries.first()
    }

    /// Takes an entry out, to put it back in the composer.
    pub fn take(&mut self, id: &str) -> Option<Entry<D>> {
        let at = self.entries.iter().position(|entry| entry.id == id)?;
        Some(self.entries.remove(at))
    }

    /// Deletes an entry.
    pub fn drop_entry(&mut self, id: &str) {
        self.entries.retain(|entry| entry.id != id);
    }

    /// Brings `id` back. A draft already in the composer is not lost: it takes the restored one's place in the
    /// stash. Returns the draft to show, or None when the entry is gone.
    pub fn restore(&mut self, id: &str, current: D, now: i64) -> Option<D> {
        let entry = self.take(id)?;
        self.stash(current, now);
        Some(entry.draft)
    }
}

/// The first line, squeezed to fit a menu row (`snippet`). `max` counts UTF-16 units.
pub fn snippet<D: StashDraft>(draft: &D, max: usize) -> String {
    let text = draft.text().split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() && draft.files() > 0 {
        return format!("({} files)", draft.files());
    }
    if text.is_empty() && draft.images() == 0 && draft.comments() > 0 {
        return if draft.comments() == 1 {
            "(1 comment)".into()
        } else {
            format!("({} comments)", draft.comments())
        };
    }
    if text.is_empty() {
        return if draft.images() == 1 {
            "(1 image)".into()
        } else {
            format!("({} images)", draft.images())
        };
    }
    if utf16_len(&text) > max {
        format!("{}…", clip(&text, max))
    } else {
        text
    }
}

/// "now", "5m", "3h", "2d": how long ago, as the menu shows it.
pub fn ago(at: i64, now: i64) -> String {
    let minutes = (now - at).div_euclid(60_000);
    if minutes < 1 {
        return "now".into();
    }
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        format!("{hours}h")
    } else {
        format!("{}d", hours / 24)
    }
}

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// `text` cut to at most `max` UTF-16 units without splitting a character (`clip.ts`): a character the cut would
/// halve is left out whole.
pub fn clip(text: &str, max: usize) -> &str {
    let mut units = 0;
    for (at, ch) in text.char_indices() {
        units += ch.len_utf16();
        if units > max {
            return &text[..at];
        }
    }
    text
}

fn base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A draft with images and diff comments, as the composers' are.
    #[derive(Debug, Clone, PartialEq, Default)]
    struct Draft {
        text: String,
        images: usize,
        comments: usize,
    }

    impl StashDraft for Draft {
        fn text(&self) -> &str {
            &self.text
        }
        fn from_text(text: String) -> Self {
            Self {
                text,
                ..Self::default()
            }
        }
        fn images(&self) -> usize {
            self.images
        }
        fn comments(&self) -> usize {
            self.comments
        }
    }

    fn text(t: &str) -> Draft {
        Draft::from_text(t.into())
    }

    fn image(t: &str, images: usize) -> Draft {
        Draft {
            text: t.into(),
            images,
            comments: 0,
        }
    }

    fn texts(stash: &Stash<Draft>) -> Vec<&str> {
        stash.entries().iter().map(|entry| entry.draft.text.as_str()).collect()
    }

    #[test]
    fn parks_a_draft_at_the_top_and_leaves_an_empty_one_alone() {
        let mut stash = Stash::default();
        stash.stash(text("first"), 1);
        stash.stash(text("second"), 2);
        assert!(stash.stash(text("   "), 3).is_none());
        assert_eq!(texts(&stash), ["second", "first"]);
    }

    #[test]
    fn keeps_at_most_max_stash_dropping_the_oldest() {
        let mut stash = Stash::default();
        for i in 0..MAX_STASH + 3 {
            stash.stash(text(&format!("d{i}")), i as i64);
        }
        assert_eq!(stash.len(), MAX_STASH);
        assert_eq!(stash.entries()[MAX_STASH - 1].draft.text, "d3");
    }

    #[test]
    fn keeps_the_text_across_a_restart_but_not_the_images() {
        let mut stash = Stash::default();
        stash.stash(image("with a screenshot", 1), 1);
        stash.stash(image("", 1), 2);
        assert_eq!(stash.len(), 2);
        let back: Stash<Draft> = Stash::from_json(&stash.to_json());
        let drafts: Vec<_> = back.entries().iter().map(|entry| entry.draft.clone()).collect();
        assert_eq!(drafts, [text("with a screenshot")]);
    }

    #[test]
    fn stores_the_shared_shape_and_cuts_long_text() {
        let mut stash = Stash::default();
        let long = "x".repeat(MAX_TEXT + 5);
        stash.stash(text(&long), 7);
        let stored: serde_json::Value = serde_json::from_str(&stash.to_json()).expect("stored JSON");
        let entry = &stored[0];
        assert_eq!(entry["at"], 7);
        assert!(entry["id"].as_str().expect("a string").starts_with('s'));
        assert_eq!(entry["text"].as_str().expect("a string").len(), MAX_TEXT);
    }

    #[test]
    fn says_what_mod_s_does() {
        let mut stash = Stash::default();
        assert_eq!(on_stash_key(&text("hi"), stash.entries()), Key::Stash);
        assert_eq!(on_stash_key(&text(""), stash.entries()), Key::None);
        let one = stash.stash(text("a"), 1).expect("stashed").id.clone();
        assert_eq!(on_stash_key(&text(""), stash.entries()), Key::Restore(one));
        stash.stash(text("b"), 2);
        assert_eq!(on_stash_key(&text(""), stash.entries()), Key::Menu);
        assert_eq!(on_stash_key(&image("", 1), stash.entries()), Key::Stash);
    }

    #[test]
    fn brings_a_draft_back_and_stashes_what_was_in_the_composer() {
        let mut stash = Stash::default();
        let a = stash.stash(text("a"), 1).expect("stashed").id.clone();
        assert_eq!(stash.restore(&a, text(""), 2), Some(text("a")));
        assert!(stash.is_empty());
        let b = stash.stash(text("b"), 3).expect("stashed").id.clone();
        assert_eq!(stash.restore(&b, text("half written"), 4), Some(text("b")));
        assert_eq!(texts(&stash), ["half written"]);
        assert_eq!(stash.restore("gone", text("x"), 5), None);
    }

    #[test]
    fn deletes_an_entry() {
        let mut stash = Stash::default();
        let a = stash.stash(text("a"), 1).expect("stashed").id.clone();
        stash.stash(text("b"), 2);
        stash.drop_entry(&a);
        assert_eq!(texts(&stash), ["b"]);
    }

    #[test]
    fn ids_differ_within_one_millisecond() {
        let mut stash = Stash::default();
        let a = stash.stash(text("a"), 5).expect("stashed").id.clone();
        let b = stash.stash(text("b"), 5).expect("stashed").id.clone();
        assert_ne!(a, b);
    }

    #[test]
    fn ignores_what_it_cannot_read() {
        let stored = r#"[{"id":1},{"id":"ok","at":5,"text":"kept"},"junk"]"#;
        assert_eq!(texts(&Stash::from_json(stored)), ["kept"]);
        assert!(Stash::<Draft>::from_json("not json").is_empty());
    }

    #[test]
    fn shows_a_short_line_and_how_long_ago() {
        assert_eq!(snippet(&text("  fix\n the   tests "), 80), "fix the tests");
        assert_eq!(snippet(&text(&"x".repeat(100)), 10), format!("{}…", "x".repeat(10)));
        assert_eq!(snippet(&image("", 2), 80), "(2 images)");
        assert_eq!(
            snippet(&text(&format!("{}😀", "x".repeat(9))), 10),
            format!("{}…", "x".repeat(9))
        );
        let comment = Draft {
            comments: 1,
            ..Draft::default()
        };
        assert_eq!(snippet(&comment, 80), "(1 comment)");
        assert_eq!(ago(0, 30_000), "now");
        assert_eq!(ago(0, 5 * 60_000), "5m");
        assert_eq!(ago(0, 3 * 3_600_000), "3h");
        assert_eq!(ago(0, 49 * 3_600_000), "2d");
    }
}
