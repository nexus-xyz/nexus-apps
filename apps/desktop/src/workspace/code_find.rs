//! Find in a Code session (Ctrl+F, Cmd+F on macOS), as the Nexus app's (`find-bar.tsx`,
//! `find-logic.ts` and `find-marks.ts`): a bar over the transcript that finds text in the whole session, its older pages
//! too. It opens at the newest match. Enter and Shift+Enter (or the arrows) step through the matches, each scrolled to
//! and marked as the one shown. Esc closes it and gives the message box its focus back.
//!
//! It reads your prompts and the agent's answers, not a linked session's messages, thinking or tool rows. It counts in
//! the stored text, so a block the list has not drawn is found too, and marks what is drawn: an answer shows its
//! Markdown rendered, so where it draws fewer matches than it holds, its last drawn one stands in for the one shown.
//! The matching is the chat's (`find_bar.rs`'s `find_ranges`: any case, the text as typed).

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::text::{RangeHighlight, TextViewState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::code_view::CodeView;
use super::find_bar::{Bar, find_bar, find_ranges, mark_colors, reveal_bounds, step};
use super::{View, Workspace, glyph};
use crate::code::Block;
use crate::palette::Palette;

/// How many frames a step waits for its answer's text view to be made and laid out before it gives up marking it.
const REVEAL_TRIES: u8 = 8;

/// One place the text sits: the `occurrence`th (from 0) in block `block`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FindMatch {
    pub block: usize,
    pub occurrence: usize,
}

/// The text of a block find reads, or None for a block it skips (`findText`).
pub(super) fn find_text(block: &Block) -> Option<&str> {
    match block {
        Block::User { text, from: None, .. } => Some(text),
        Block::Text(text) => Some(text),
        _ => None,
    }
}

/// Every place `query` sits in the blocks find reads, oldest first (`findMatches`).
#[cfg(test)]
pub(super) fn find_matches(blocks: &[Block], query: &str) -> Vec<FindMatch> {
    matches_by(blocks, |_, text| find_ranges(text, query).len())
}

/// The matches, `count` saying how many block `block`, with text `text`, holds.
fn matches_by(blocks: &[Block], mut count: impl FnMut(usize, &str) -> usize) -> Vec<FindMatch> {
    let mut found = Vec::new();
    for (block, item) in blocks.iter().enumerate() {
        let Some(text) = find_text(item) else { continue };
        let count = count(block, text);
        found.extend((0..count).map(|occurrence| FindMatch { block, occurrence }));
    }
    found
}

/// The bar's count: "3 of 12", "No matches" for none, "Searching…" while the older pages are read. Empty with no text
/// (`countLabel`). `at` is the place of the match shown.
pub(super) fn count_label(query: &str, at: Option<usize>, count: usize, searching: bool) -> String {
    if query.is_empty() {
        String::new()
    } else if searching {
        "Searching…".into()
    } else if count == 0 {
        "No matches".into()
    } else {
        format!("{} of {}", at.map_or(0, |at| at + 1), count)
    }
}

/// The drawn place of the `occurrence`th match among `drawn`: the last one drawn stands in when it draws fewer
/// (`find-marks.ts`'s `placeOf`).
pub(super) fn place_of(drawn: &[Range<usize>], occurrence: usize) -> Option<Range<usize>> {
    drawn.get(occurrence.min(drawn.len().checked_sub(1)?)).cloned()
}

/// Find's state in the open Code session. A new session's view starts it closed and empty.
#[derive(Default)]
pub(super) struct CodeFind {
    pub open: bool,
    input: Option<(Entity<InputState>, Subscription)>,
    query: String,
    /// New with each query: a mark painted for an older one is painted again.
    version: u64,
    /// What `matches` was found for: the query and the transcript as read.
    stamp: u64,
    matches: Vec<FindMatch>,
    /// How many matches each block holds.
    per_block: HashMap<usize, usize>,
    /// The match shown: its block counted from the first block the view ever showed (blocks an older page puts above
    /// do not move it), and its occurrence.
    shown: Option<(isize, usize)>,
    /// Scroll to the match shown, for so many more frames, until its text view takes the request.
    reveal: u8,
    /// The block just scrolled to: the next frame moves it off the top edge, under the bar.
    settle: Option<usize>,
    /// Each answer's count, kept while its text and the query stay as they were: (text length, count).
    cache: HashMap<isize, (usize, usize)>,
    /// The answers' text views with marks painted, and what they were painted for.
    painted: HashMap<EntityId, (WeakEntity<TextViewState>, u64)>,
}

impl CodeFind {
    fn at(&self, shifted: usize) -> Option<usize> {
        let (anchor, occurrence) = self.shown?;
        let block = usize::try_from(anchor + shifted as isize).ok()?;
        self.matches
            .iter()
            .position(|found| *found == FindMatch { block, occurrence })
    }

    /// The occurrence shown in block `ix`, if the match shown is there.
    fn shown_in(&self, ix: usize, shifted: usize) -> Option<usize> {
        let (anchor, occurrence) = self.shown?;
        (anchor + shifted as isize == ix as isize).then_some(occurrence)
    }

    fn active(&self) -> bool {
        self.open && !self.query.is_empty()
    }
}

impl CodeView {
    /// The marks in your message at block `ix`, as its text draws them.
    pub(super) fn find_marks(&self, ix: usize, text: &str) -> Vec<(Range<usize>, HighlightStyle)> {
        let find = &self.find;
        if !find.active() || !find.per_block.contains_key(&ix) {
            return Vec::new();
        }
        let (plain, strong) = mark_colors();
        let drawn = find_ranges(text, &find.query);
        let shown = find
            .shown_in(ix, self.shifted)
            .and_then(|occurrence| place_of(&drawn, occurrence));
        drawn
            .into_iter()
            .map(|range| {
                let color = if shown.as_ref() == Some(&range) { strong } else { plain };
                let style = HighlightStyle {
                    background_color: Some(color),
                    ..Default::default()
                };
                (range, style)
            })
            .collect()
    }
}

impl Workspace {
    pub(super) fn open_code_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        let find = &mut view.find;
        if find.input.is_none() {
            let input =
                cx.new(|cx| InputState::new(window, cx).placeholder(format!("Find in the session ({})", key())));
            let subscription = cx.subscribe_in(&input, window, |this, state, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    let query = state.read(cx).value().to_string();
                    this.code_find_typed(query, cx);
                }
                InputEvent::PressEnter { shift, .. } => this.code_find_step(if *shift { -1 } else { 1 }, cx),
                _ => {}
            });
            find.input = Some((input, subscription));
        }
        if !find.open {
            // Opened again: the newest match, as a new search.
            find.open = true;
            find.stamp = 0;
            find.shown = None;
        }
        if let Some((input, _)) = &find.input {
            input.update(cx, |state, cx| {
                state.select_all(window, cx);
                state.focus(window, cx);
            });
        }
        cx.notify();
    }

    pub(super) fn close_code_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        let find = &mut view.find;
        if !find.open {
            return;
        }
        find.open = false;
        for (text, _) in std::mem::take(&mut find.painted).into_values() {
            if let Some(text) = text.upgrade() {
                text.update(cx, |state, cx| state.clear_range_highlights(cx));
            }
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Esc and the arrows in the find field, before the session sees them: Esc there closes find, not the agent's
    /// turn. True when it took the key.
    pub(super) fn code_find_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let View::Code(view) = &self.view else { return false };
        let Some((input, _)) = view.find.input.as_ref().filter(|_| view.find.open) else {
            return false;
        };
        if !input.focus_handle(cx).is_focused(window) {
            return false;
        }
        match key {
            "escape" => self.close_code_find(window, cx),
            "up" => self.code_find_step(-1, cx),
            "down" => self.code_find_step(1, cx),
            _ => return false,
        }
        true
    }

    fn code_find_typed(&mut self, query: String, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        let find = &mut view.find;
        if query == find.query {
            return;
        }
        find.query = query;
        find.version += 1;
        find.stamp = 0;
        find.shown = None;
        find.cache.clear();
        cx.notify();
    }

    pub(super) fn code_find_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        let shifted = view.shifted;
        let find = &mut view.find;
        if find.matches.is_empty() {
            return;
        }
        let at = step(find.at(shifted).unwrap_or(0), find.matches.len(), delta);
        let found = find.matches[at];
        find.shown = Some((found.block as isize - shifted as isize, found.occurrence));
        find.reveal = REVEAL_TRIES;
        cx.notify();
    }

    /// Bring the matches up to date with the query and the transcript, read the older pages while there is text to
    /// find, and scroll to the match shown. Runs at the start of a frame.
    pub(super) fn refresh_code_find(&mut self, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        if !view.find.active() {
            return;
        }
        // The whole session: the pages before the first one shown are read, one after another.
        if view.more && view.reading.is_none() && view.asked_before != view.start {
            let this = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = this.update(cx, |this, cx| this.read_older_code(cx));
            });
        }
        let mut hasher = DefaultHasher::new();
        (
            view.find.version,
            view.blocks.len(),
            view.last_seq,
            view.start,
            view.shifted,
        )
            .hash(&mut hasher);
        let stamp = hasher.finish();
        if stamp != view.find.stamp {
            view.find.stamp = stamp;
            update_matches(view);
            if view.find.at(view.shifted).is_none()
                && let Some(newest) = view.find.matches.last().copied()
            {
                view.find.shown = Some((newest.block as isize - view.shifted as isize, newest.occurrence));
                view.find.reveal = REVEAL_TRIES;
            }
        }
        if let Some(ix) = view.find.settle.take()
            && let Some(bounds) = view.list.bounds_for_item(ix)
            && bounds.size.height < view.list.viewport_bounds().size.height / 2.
        {
            reveal_bounds(&view.list, bounds);
        }
        if view.find.reveal > 0 {
            view.find.reveal -= 1;
            if reveal_shown(view, cx) {
                view.find.reveal = 0;
                view.find.settle = view.find.at(view.shifted).map(|at| view.find.matches[at].block);
            }
            cx.notify();
        }
    }

    /// Mark the matches in the answer at block `ix`, drawn now; take them away once find closes or moves on.
    pub(super) fn mark_code_text(&mut self, ix: usize, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        let Some(Some(text)) = view.texts.get(ix).cloned() else {
            return;
        };
        let find = &mut view.find;
        let id = text.entity_id();
        let active = find.active() && find.per_block.contains_key(&ix);
        if !active {
            if find.painted.remove(&id).is_some() {
                text.update(cx, |state, cx| state.clear_range_highlights(cx));
            }
            return;
        }
        let shown = find.shown_in(ix, view.shifted);
        let rendered = text.read(cx).rendered_text();
        let mut hasher = DefaultHasher::new();
        (find.version, shown, rendered.len()).hash(&mut hasher);
        let key = hasher.finish();
        if find.painted.get(&id).is_some_and(|(_, painted)| *painted == key) {
            return;
        }
        let drawn = find_ranges(rendered.as_str(), &find.query);
        let current = shown.and_then(|occurrence| place_of(&drawn, occurrence));
        let (plain, strong) = mark_colors();
        let highlights: Vec<RangeHighlight> = drawn
            .into_iter()
            .map(|range| {
                let color = if current.as_ref() == Some(&range) {
                    strong
                } else {
                    plain
                };
                RangeHighlight::new(range, color)
            })
            .collect();
        text.update(cx, |state, cx| {
            let _ = state.set_range_highlights(highlights, cx);
        });
        find.painted.insert(id, (text.downgrade(), key));
    }

    /// The bar over the transcript's top right, with the hand-off to the palette under it (`FindBar`).
    pub(super) fn render_code_find_bar(&self, view: &CodeView, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let find = &view.find;
        let searching = find.active() && view.more;
        let label = count_label(&find.query, find.at(view.shifted), find.matches.len(), searching);
        let handoff = h_flex()
            .id("find-handoff")
            .mx(px(6.))
            .mb(px(6.))
            .px(px(6.))
            .py(px(2.))
            .gap(px(6.))
            .items_center()
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(palette.faint)
            .cursor_pointer()
            .hover(|style| style.text_color(palette.muted))
            .on_click(cx.listener(|this, _, window, cx| this.search_all_sessions(window, cx)))
            .child(glyph(Lucide::Search, palette.faint).size(px(12.)))
            .child("Search all sessions…")
            .child(div().font_family("Geist Mono").text_size(px(11.)).child(palette_key()));
        find_bar(
            Bar {
                input: find.input.as_ref().map(|(input, _)| input.clone()),
                none: !find.query.is_empty() && !searching && find.matches.is_empty(),
                label,
                empty: find.matches.is_empty(),
                step: |this: &mut Workspace, delta, cx| this.code_find_step(delta, cx),
                close: |this, window, cx| this.close_code_find(window, cx),
                footer: Some(handoff.into_any_element()),
            },
            palette,
            cx,
        )
        .into_any_element()
    }

    /// The palette, searching for what find has (`openPalette(find.typed)`).
    fn search_all_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &self.view else { return };
        let query = view.find.query.clone();
        self.close_code_find(window, cx);
        self.open_palette(window, cx);
        self.palette_input
            .update(cx, |state, cx| state.set_value(query.clone(), window, cx));
        self.palette_typed(query, cx);
    }
}

/// Count the matches again, reusing each block's count while its text and the query are as they were.
fn update_matches(view: &mut CodeView) {
    let shifted = view.shifted as isize;
    let find = &mut view.find;
    let mut cache = HashMap::new();
    let matches = matches_by(&view.blocks, |block, text| {
        let anchor = block as isize - shifted;
        let count = match find.cache.get(&anchor) {
            Some((length, count)) if *length == text.len() => *count,
            _ => find_ranges(text, &find.query).len(),
        };
        cache.insert(anchor, (text.len(), count));
        count
    });
    let mut per_block = HashMap::new();
    for found in &matches {
        *per_block.entry(found.block).or_insert(0) += 1;
    }
    find.matches = matches;
    find.per_block = per_block;
    find.cache = cache;
}

/// Scroll the match shown into view. False while its answer's text view is not made or laid out yet.
fn reveal_shown(view: &mut CodeView, cx: &mut App) -> bool {
    let Some((anchor, occurrence)) = view.find.shown else {
        return true;
    };
    let Ok(ix) = usize::try_from(anchor + view.shifted as isize) else {
        return true;
    };
    // A list following its end would snap back to it: it stops there, and follows again once read to the end.
    view.list.pause_following_tail();
    view.list.scroll_to_reveal_item(ix);
    match view.blocks.get(ix) {
        Some(Block::Text(_)) => {
            let Some(Some(text)) = view.texts.get(ix) else {
                return false;
            };
            let rendered = text.read(cx).rendered_text();
            if rendered.is_empty() {
                return false;
            }
            let Some(range) = place_of(&find_ranges(rendered.as_str(), &view.find.query), occurrence) else {
                // Drawn without the text (a link's address): showing its block shows where it is.
                return true;
            };
            text.update(cx, |state, cx| {
                let _ = state.reveal_range(range, cx);
            });
            true
        }
        _ => true,
    }
}

/// Ctrl+F, or Cmd+F on macOS.
fn key() -> &'static str {
    if cfg!(target_os = "macos") { "⌘F" } else { "Ctrl+F" }
}

/// The palette's key.
fn palette_key() -> &'static str {
    if cfg!(target_os = "macos") { "⌘K" } else { "Ctrl+K" }
}

#[cfg(test)]
mod tests {
    use super::{FindMatch, count_label, find_matches, place_of};
    use crate::code::Block;

    fn user(seq: i64, text: &str, linked: bool) -> Block {
        let from = linked.then(|| nexus_workbench::event::Sender::Session {
            thread_id: "lead".into(),
            title: "Lead".into(),
        });
        Block::User {
            seq,
            at: 0,
            text: text.into(),
            from,
            rewound: false,
            images: 0,
            image_data: Vec::new(),
        }
    }

    fn found(pairs: &[(usize, usize)]) -> Vec<FindMatch> {
        pairs
            .iter()
            .map(|&(block, occurrence)| FindMatch { block, occurrence })
            .collect()
    }

    // `find-logic.test.ts`'s cases, over the transcript's blocks.
    #[test]
    fn finds_every_place_in_your_prompts_and_the_answers_in_any_case_oldest_first() {
        let blocks = vec![
            user(1, "Fix the login test", false),
            Block::Thinking("login".into(), None),
            Block::Text("The LOGIN test failed: login() threw.".into()),
            Block::Error("login".into(), None),
            user(7, "login again", true),
        ];
        assert_eq!(find_matches(&blocks, "login"), found(&[(0, 0), (2, 0), (2, 1)]));
    }

    #[test]
    fn skips_thinking_a_tool_an_error_and_a_linked_sessions_message() {
        let blocks = vec![
            Block::Thinking("login".into(), None),
            Block::Tool {
                call_id: "bash".into(),
                name: "Bash".into(),
                summary: "login".into(),
                error: None,
                edit: None,
                images: Vec::new(),
                call: Default::default(),
            },
            Block::Error("login".into(), Some("login".into())),
            user(7, "login again", true),
        ];
        assert_eq!(find_matches(&blocks, "login"), Vec::new());
        assert_eq!(find_matches(&blocks, "again"), Vec::new());
    }

    #[test]
    fn reads_the_text_as_typed() {
        let blocks = vec![
            user(1, "Fix the login test", false),
            Block::Text("The LOGIN test failed: login() threw.".into()),
        ];
        assert!(find_matches(&blocks, "t").len() > 3);
        assert_eq!(find_matches(&blocks, "login()"), found(&[(1, 0)]));
        assert_eq!(find_matches(&blocks, " test").len(), 2);
        assert_eq!(find_matches(&blocks, ""), Vec::new());
    }

    #[test]
    fn the_count_says_which_match_of_how_many_none_or_that_it_is_still_reading() {
        assert_eq!(count_label("login", Some(2), 12, false), "3 of 12");
        assert_eq!(count_label("login", None, 0, false), "No matches");
        assert_eq!(count_label("login", Some(0), 3, true), "Searching…");
        assert_eq!(count_label("", None, 0, false), "");
    }

    #[test]
    fn an_answer_drawing_fewer_matches_marks_its_last_one_drawn() {
        let drawn = vec![0..5, 10..15];
        assert_eq!(place_of(&drawn, 0), Some(0..5));
        assert_eq!(place_of(&drawn, 1), Some(10..15));
        assert_eq!(place_of(&drawn, 4), Some(10..15));
        assert_eq!(place_of(&[], 0), None);
    }
}
