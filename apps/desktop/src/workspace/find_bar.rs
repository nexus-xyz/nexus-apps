//! Find in the open chat (Ctrl+F, Cmd+F on macOS), as the Nexus app's find bar (`core/ui/src/components/nexus-ui/chat/
//! find-bar.tsx` and `find-logic.ts`): a bar over the chat, the matches marked in the transcript, "3 of 12", Enter and
//! Shift+Enter step, Esc closes.
//!
//! Unlike the Nexus app, which counts messages, this counts every occurrence. Matches are found in the text as drawn
//! (an answer's rendered Markdown), only when the query or the transcript changes: a per-message cache keeps a
//! streaming answer from rescanning the rest of the chat.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::text::RangeHighlight;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Workspace, glyph};
use crate::palette::Palette;

/// One match: the entry (by its id) and where it sits in that entry's text.
#[derive(Debug, Clone, PartialEq)]
struct Hit {
    entry: usize,
    range: Range<usize>,
}

#[derive(Default)]
pub(super) struct Find {
    pub open: bool,
    input: Option<(Entity<InputState>, Subscription)>,
    query: String,
    /// Every match, in transcript order.
    hits: Vec<Hit>,
    /// The match stepped to, a place in `hits`.
    current: usize,
    /// What `hits` was found for: the query and each message's text length.
    stamp: u64,
    /// What the transcript shows of `hits`: set when the stamp or the current match changes.
    painted: Option<(u64, usize)>,
    /// A new query opens at the newest match, as the Nexus app does.
    fresh: bool,
    /// Scroll to the current match at the next frame.
    reveal: bool,
    /// Matches of one message, kept while its text and the query stay as they were: (text length, query version).
    cache: HashMap<usize, (usize, u64, Vec<Range<usize>>)>,
    version: u64,
    /// Marks in a question's text (a plain element, unlike an answer's text view).
    marks: HashMap<usize, Vec<Range<usize>>>,
}

impl Find {
    /// The marks in a question, with the current one's range if it is in this question.
    pub fn marks_in(&self, entry: usize) -> (&[Range<usize>], Option<Range<usize>>) {
        let current = self
            .hits
            .get(self.current)
            .filter(|hit| hit.entry == entry)
            .map(|hit| hit.range.clone());
        (self.marks.get(&entry).map(Vec::as_slice).unwrap_or(&[]), current)
    }

    fn label(&self) -> String {
        if self.hits.is_empty() {
            if self.query.is_empty() {
                String::new()
            } else {
                "No matches".into()
            }
        } else {
            format!("{} of {}", self.current + 1, self.hits.len())
        }
    }
}

/// Where `query` sits in `text`, ignoring case: the rule both apps' find bars share.
pub(crate) use nexus_workbench::find::text_ranges as find_ranges;

/// The match after `at` (`delta` 1) or before it (-1), going round at the ends.
pub(crate) fn step(at: usize, count: usize, delta: isize) -> usize {
    if count == 0 {
        0
    } else {
        (at as isize + delta).rem_euclid(count as isize) as usize
    }
}

const MATCH: Hsla = Hsla {
    h: 0.13,
    s: 0.95,
    l: 0.55,
    a: 0.32,
};
const CURRENT: Hsla = Hsla {
    h: 0.08,
    s: 0.95,
    l: 0.55,
    a: 0.7,
};

/// The colour of a match, and of the one stepped to.
pub(crate) fn mark_colors() -> (Hsla, Hsla) {
    (MATCH, CURRENT)
}

/// Scroll the chat so `bounds` (window coordinates) shows, a third of the way down when it was off screen.
pub(super) fn reveal_bounds(list: &ListState, bounds: gpui_kit::Bounds<Pixels>) {
    let view = list.viewport_bounds();
    let margin = px(56.);
    if bounds.top() >= view.top() + margin && bounds.bottom() <= view.bottom() - margin {
        return;
    }
    let want = view.top() + view.size.height / 3.;
    list.scroll_by(bounds.top() - want);
}

impl Workspace {
    pub(super) fn on_find_in_chat(&mut self, _: &super::FindInChat, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(window, cx);
    }

    pub(super) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // In Settings the key finds a setting: it goes to the search field.
        if matches!(self.view, super::View::Settings(_)) {
            return self.focus_settings_search(window, cx);
        }
        // Find searches the conversation. Reveal it before opening its search bar.
        if self.agent_view_open() {
            self.close_agent_view(cx);
        }
        // A Code session has its own find (`code_find.rs`).
        if matches!(self.view, super::View::Code(_)) {
            return self.open_code_find(window, cx);
        }
        if !matches!(self.view, super::View::Chat) || self.entries.is_empty() {
            return;
        }
        if self.find.input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in this chat"));
            let subscription = cx.subscribe_in(&input, window, |this, state, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    let query = state.read(cx).value().to_string();
                    this.find_typed(query, cx);
                }
                InputEvent::PressEnter { shift, .. } => this.find_step(if *shift { -1 } else { 1 }, cx),
                _ => {}
            });
            self.find.input = Some((input, subscription));
        }
        self.find.open = true;
        if let Some((input, _)) = &self.find.input {
            input.update(cx, |state, cx| {
                state.select_all(window, cx);
                state.focus(window, cx);
            });
        }
        cx.notify();
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find.open {
            return;
        }
        self.find.open = false;
        self.find.query.clear();
        self.find.hits.clear();
        self.find.marks.clear();
        self.find.cache.clear();
        self.find.painted = None;
        self.find.stamp = 0;
        if let Some((input, _)) = &self.find.input {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        for entry in &self.entries {
            if let Some(answer) = &entry.answer {
                answer.update(cx, |state, cx| state.clear_range_highlights(cx));
            }
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn find_typed(&mut self, query: String, cx: &mut Context<Self>) {
        if query == self.find.query {
            return;
        }
        self.find.query = query;
        self.find.version += 1;
        self.find.fresh = true;
        self.find.stamp = 0;
        cx.notify();
    }

    pub(super) fn find_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.find.current = step(self.find.current, self.find.hits.len(), delta);
        self.find.reveal = true;
        cx.notify();
    }

    /// Bring the matches up to date, once per change of the query or the transcript, and mark them. Runs at the start
    /// of a frame, so what draws is current.
    pub(super) fn refresh_find(&mut self, cx: &mut Context<Self>) {
        if !self.find.open {
            return;
        }
        let mut hasher = DefaultHasher::new();
        self.find.version.hash(&mut hasher);
        let lengths: Vec<usize> = self
            .entries
            .iter()
            .map(|entry| match &entry.answer {
                Some(answer) => answer.read(cx).rendered_text().len(),
                None => entry.text.len(),
            })
            .collect();
        for (entry, length) in self.entries.iter().zip(&lengths) {
            (entry.id, length).hash(&mut hasher);
        }
        let stamp = hasher.finish();
        if stamp != self.find.stamp {
            self.find.stamp = stamp;
            let (query, version) = (self.find.query.clone(), self.find.version);
            let mut hits = Vec::new();
            let mut marks: HashMap<usize, Vec<Range<usize>>> = HashMap::new();
            for (entry, length) in self.entries.iter().zip(&lengths) {
                let cached = self
                    .find
                    .cache
                    .get(&entry.id)
                    .filter(|(len, v, _)| len == length && *v == version)
                    .map(|(_, _, ranges)| ranges.clone());
                let ranges = cached.unwrap_or_else(|| {
                    let ranges = match &entry.answer {
                        Some(answer) => find_ranges(answer.read(cx).rendered_text().as_str(), &query),
                        None => find_ranges(&entry.text, &query),
                    };
                    self.find.cache.insert(entry.id, (*length, version, ranges.clone()));
                    ranges
                });
                if entry.answer.is_none() && !ranges.is_empty() {
                    marks.insert(entry.id, ranges.clone());
                }
                hits.extend(ranges.into_iter().map(|range| Hit { entry: entry.id, range }));
            }
            self.find.hits = hits;
            self.find.marks = marks;
            if self.find.fresh {
                self.find.fresh = false;
                self.find.current = self.find.hits.len().saturating_sub(1);
                self.find.reveal = !self.find.hits.is_empty();
            }
            self.find.current = self.find.current.min(self.find.hits.len().saturating_sub(1));
        }
        if self.find.painted != Some((self.find.stamp, self.find.current)) {
            self.find.painted = Some((self.find.stamp, self.find.current));
            self.paint_find(cx);
        }
        if self.find.reveal {
            self.find.reveal = false;
            self.reveal_find(cx);
        }
    }

    /// Mark every match in the answers' text views; a question's marks are drawn with its text.
    fn paint_find(&mut self, cx: &mut Context<Self>) {
        let (plain, strong) = mark_colors();
        let current = self.find.current;
        let mut by_entry: HashMap<usize, Vec<RangeHighlight>> = HashMap::new();
        for (ix, hit) in self.find.hits.iter().enumerate() {
            let color = if ix == current { strong } else { plain };
            by_entry
                .entry(hit.entry)
                .or_default()
                .push(RangeHighlight::new(hit.range.clone(), color));
        }
        for entry in &self.entries {
            if let Some(answer) = &entry.answer {
                let highlights = by_entry.remove(&entry.id).unwrap_or_default();
                answer.update(cx, |state, cx| {
                    if highlights.is_empty() {
                        state.clear_range_highlights(cx);
                    } else {
                        let _ = state.set_range_highlights(highlights, cx);
                    }
                });
            }
        }
        cx.notify();
    }

    /// Scroll the current match into view.
    fn reveal_find(&mut self, cx: &mut Context<Self>) {
        let Some(hit) = self.find.hits.get(self.find.current).cloned() else {
            return;
        };
        let Some((ix, entry)) = self.entries.iter().enumerate().find(|(_, entry)| entry.id == hit.entry) else {
            return;
        };
        // The message may not be laid out (the list lays out only what is near the screen): bring it in first.
        self.chat_list.list.scroll_to_reveal_item(ix);
        match &entry.answer {
            // The text view finds the line and tells the transcript (`on_reveal`, in `transcript.rs`).
            Some(answer) => {
                answer.update(cx, |state, cx| {
                    let _ = state.reveal_range(hit.range, cx);
                });
            }
            // A question is one bubble: showing its message shows the match.
            None => {}
        }
        cx.notify();
    }

    /// The bar over the chat's top right.
    pub(super) fn render_find_bar(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        find_bar(
            Bar {
                input: self.find.input.as_ref().map(|(input, _)| input.clone()),
                label: self.find.label(),
                none: false,
                empty: self.find.hits.is_empty(),
                step: |this: &mut Workspace, delta, cx| this.find_step(delta, cx),
                close: |this: &mut Workspace, window, cx| this.close_find(window, cx),
                footer: None,
            },
            palette,
            cx,
        )
    }
}

/// What a find bar shows and does: the chat's (above), a Code session's (`code_find.rs`) and the Changes tool's
/// (`side/changes.rs`). `V` is the view that owns it.
pub(crate) struct Bar<V: 'static = Workspace> {
    pub input: Option<Entity<InputState>>,
    /// "3 of 12", "No matches", or nothing.
    pub label: String,
    /// The label says there is no match: it shows in the danger colour.
    pub none: bool,
    /// No match to step to: the arrows are off.
    pub empty: bool,
    pub step: fn(&mut V, isize, &mut Context<V>),
    pub close: fn(&mut V, &mut Window, &mut Context<V>),
    /// A row under the field.
    pub footer: Option<AnyElement>,
}

/// A find bar over the top right of a transcript: the field, the count, Previous, Next and Close.
pub(crate) fn find_bar<V: 'static>(bar: Bar<V>, palette: Palette, cx: &mut Context<V>) -> impl IntoElement {
    let Bar {
        input,
        label,
        none,
        empty,
        step,
        close,
        footer,
    } = bar;
    let button = |id: &'static str, icon: Lucide, tip: &'static str, enabled: bool| {
        div()
            .id(id)
            .size(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover))
                    .active(|style| style.opacity(0.8))
            })
            .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
            .child(glyph(icon, if enabled { palette.icon } else { palette.faint }).size(px(16.)))
    };
    let row = h_flex()
        .h(px(40.))
        .pl(px(12.))
        .pr(px(6.))
        .gap(px(6.))
        .items_center()
        .child(glyph(Lucide::Search, palette.muted))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(14.))
                .children(input.map(|input| Input::new(&input).appearance(false))),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(12.))
                .font_family("Geist Mono")
                .text_color(if none { palette.destructive } else { palette.faint })
                .child(label),
        )
        .child(
            button("find-prev", Lucide::ChevronUp, "Previous match (Shift+Enter)", !empty)
                .on_click(cx.listener(move |this, _, _, cx| step(this, -1, cx))),
        )
        .child(
            button("find-next", Lucide::ChevronDown, "Next match (Enter)", !empty)
                .on_click(cx.listener(move |this, _, _, cx| step(this, 1, cx))),
        )
        .child(
            button("find-close", Lucide::X, "Close (Esc)", true)
                .on_click(cx.listener(move |this, _, window, cx| close(this, window, cx))),
        );
    motion_bar(
        v_flex()
            .id("find-bar")
            .absolute()
            .top(px(12.))
            .right(px(24.))
            .w(px(380.))
            .rounded(px(12.))
            .bg(palette.pop)
            .shadow_md()
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    close(this, window, cx);
                }
            }))
            .child(row)
            .children(footer),
    )
}

fn motion_bar(bar: Stateful<Div>) -> impl IntoElement {
    crate::motion::fade_in(bar, "find-bar-in")
}

#[cfg(test)]
mod tests {
    use super::step;

    #[test]
    fn steps_round_at_the_ends() {
        assert_eq!(step(0, 3, 1), 1);
        assert_eq!(step(2, 3, 1), 0);
        assert_eq!(step(0, 3, -1), 2);
        assert_eq!(step(0, 0, 1), 0);
    }
}
