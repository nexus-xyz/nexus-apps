//! A Code session's runs of work, folded as the Nexus app's timeline folds them (`work-run.tsx`). The rule is
//! `nexus_workbench::work_folds`: the calls between two answers share a card headed by what they did ("Explored 6
//! files, ran 2 commands"). Cards start closed until the person opens them, including while work runs.
//! Beside a step that matters (an edit, a sub-agent, a plan), four or more routine steps fold into
//! one row. A card past 40 parts offers "Show N earlier steps". A thought reads "Thought for 14 s" and opens to its
//! text.
//!
//! The transcript is a list with one item per block, so a card's header and a fold's row draw on their first block's
//! item, and a closed card's other blocks draw nothing.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::work_folds::{self, Card, Segment, Step};

use super::code_view::{CodeView, hidden_tool};
use super::{View, Workspace, glyph};
use crate::code::Block;
use crate::palette::Palette;

/// Cards and nested folds can start on the same block, but keep separate disclosure states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Card(isize),
    Fold(isize),
}

impl Key {
    pub(super) fn anchor(self) -> isize {
        match self {
            Self::Card(anchor) | Self::Fold(anchor) => anchor,
        }
    }
}

/// A card's header or a fold's row, drawn on its first block.
#[derive(Debug, Clone, PartialEq)]
pub struct Head {
    /// Its open state's key: its first block, counted from the session's first block read.
    pub key: Key,
    pub label: String,
    /// "6 steps · 1 failed" on a fold; "1 failed" on a card.
    pub facts: Option<String>,
    pub open: bool,
}

/// How block `ix` draws inside its run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Place {
    pub header: Option<Head>,
    /// "Show N earlier steps": the card's key and the steps it hides.
    pub earlier: Option<(isize, usize)>,
    pub fold: Option<Head>,
    /// The block's own row is hidden: its card or its fold is closed, or it is among the earlier steps.
    pub hidden: bool,
    /// The row sits in an open card (one step in), or in an open fold inside it (two).
    pub depth: u8,
}

/// Blocks a run is made of: tool calls and thinking.
fn in_run(block: &Block) -> bool {
    matches!(block, Block::Tool { .. } | Block::Thinking(..))
}

fn step_of(block: &Block, running: bool) -> Step {
    match block {
        Block::Tool {
            name, error, images, ..
        } => Step::Call {
            name: name.clone(),
            running,
            failed: error.is_some(),
            marked: !images.is_empty(),
        },
        _ => Step::Thinking,
    }
}

/// Each block's place. `open`: the cards and folds the person opened or closed, by key; `all`: the cards whose
/// earlier steps they asked to see. `shifted`: how many blocks older pages put above the first ones read.
pub fn layout(blocks: &[Block], open: &HashMap<Key, bool>, all: &HashSet<isize>, shifted: usize) -> Vec<Place> {
    let mut places = vec![Place::default(); blocks.len()];
    let key = |ix: usize| ix as isize - shifted as isize;
    // The last block of the agent's work: a run that reaches it is the turn's work in progress.
    let tail = crate::code::work_tail(blocks);
    let mut ix = 0;
    while ix < blocks.len() {
        if !in_run(&blocks[ix]) {
            ix += 1;
            continue;
        }
        let mut members = Vec::new();
        let mut end = ix;
        while end < blocks.len() && in_run(&blocks[end]) {
            if !matches!(&blocks[end], Block::Tool { name, .. } if hidden_tool(name)) {
                members.push(end);
            }
            end += 1;
        }
        ix = end;
        let live = tail.is_some_and(|tail| tail < end);
        let last = members.last().copied();
        let steps: Vec<Step> = members
            .iter()
            .map(|&at| step_of(&blocks[at], live && Some(at) == last))
            .collect();
        for card in work_folds::cards(&steps, live) {
            let Card::Tools {
                steps: range, parts, ..
            } = card
            else {
                continue;
            };
            if range.len() < 2 {
                continue;
            }
            let run = &steps[range.clone()];
            let first = members[range.start];
            let card_key = key(first);
            let is_open = open.get(&Key::Card(card_key)).copied().unwrap_or(false);
            let failed = work_folds::fold_failures(run);
            places[first].header = Some(Head {
                key: Key::Card(card_key),
                label: work_folds::fold_summary(run),
                facts: (failed > 0).then(|| format!("{failed} failed")),
                open: is_open,
            });
            let rows: Vec<usize> = range.clone().map(|step| members[step]).collect();
            if !is_open {
                for at in rows {
                    places[at].hidden = true;
                }
                continue;
            }
            for &at in &rows {
                places[at].depth = 1;
            }
            let hidden = if all.contains(&card_key) {
                0
            } else {
                parts.len().saturating_sub(work_folds::RUN_TAIL)
            };
            if hidden > 0 {
                let mut count = 0;
                for part in &parts[..hidden] {
                    let steps_of: Range<usize> = match part {
                        Segment::Step(at) => *at..*at + 1,
                        Segment::Fold(fold) => fold.clone(),
                    };
                    count += steps_of.len();
                    for step in steps_of {
                        places[members[step]].hidden = true;
                    }
                }
                places[first].earlier = Some((card_key, count));
            }
            for part in &parts[hidden..] {
                let Segment::Fold(fold) = part else {
                    continue;
                };
                let fold_first = members[fold.start];
                let fold_key = key(fold_first);
                let fold_open = open.get(&Key::Fold(fold_key)).copied().unwrap_or(false);
                places[fold_first].fold = Some(Head {
                    key: Key::Fold(fold_key),
                    label: work_folds::fold_summary(&steps[fold.clone()]),
                    facts: Some(work_folds::fold_facts(&steps[fold.clone()])),
                    open: fold_open,
                });
                for step in fold.clone() {
                    let place = &mut places[members[step]];
                    place.hidden = !fold_open;
                    place.depth = 2;
                }
            }
        }
    }
    places
}

impl CodeView {
    /// Lay the runs out again, and have the list measure the blocks whose place changed.
    pub fn relayout(&mut self) {
        let places = layout(&self.blocks, &self.fold_open, &self.fold_all, self.shifted);
        for (ix, place) in places.iter().enumerate() {
            if self.folds.get(ix) != Some(place) {
                self.list.remeasure_items(ix..ix + 1);
            }
        }
        self.folds = places;
    }
}

impl Workspace {
    /// Open or close the card or fold `key`, now `open` or closed.
    fn toggle_fold(&mut self, key: Key, open: bool, cx: &mut Context<Self>) {
        if let View::Code(view) = &mut self.view {
            view.fold_open.insert(key, !open);
            view.relayout();
            cx.notify();
        }
    }

    fn show_earlier(&mut self, key: isize, cx: &mut Context<Self>) {
        if let View::Code(view) = &mut self.view {
            view.fold_all.insert(key);
            view.relayout();
            cx.notify();
        }
    }

    fn toggle_thought(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let View::Code(view) = &mut self.view {
            let key = ix as isize - view.shifted as isize;
            if !view.thoughts_open.remove(&key) {
                view.thoughts_open.insert(key);
            }
            view.list.remeasure_items(ix..ix + 1);
            cx.notify();
        }
    }

    /// What block `ix` draws above its own row: its card's header, "Show N earlier steps", its fold's row.
    pub(super) fn fold_lead(&self, place: &Place, palette: Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut lead = Vec::new();
        if let Some(head) = &place.header {
            lead.push(self.head_row("card", head, 0, palette, cx));
        }
        if let Some((key, steps)) = place.earlier {
            lead.push(
                earlier_row("earlier", key, steps, palette)
                    .on_click(cx.listener(move |this, _, _, cx| this.show_earlier(key, cx)))
                    .into_any_element(),
            );
        }
        if let Some(head) = &place.fold {
            lead.push(self.head_row("fold", head, 1, palette, cx));
        }
        lead
    }

    /// A card's header or a fold's row: a chevron, what its steps did, its facts. A click opens or closes it.
    fn head_row(
        &self,
        kind: &'static str,
        head: &Head,
        depth: u8,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (key, open) = (head.key, head.open);
        head_row(kind, head, depth, palette)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_fold(key, open, cx)))
            .into_any_element()
    }

    /// A thought (`ThinkingRow`): "Thought for 14 s", opening to its text.
    pub(super) fn thought_row(
        &self,
        ix: usize,
        text: &str,
        ms: Option<u64>,
        open: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        gpui_kit::component::v_flex()
            .w_full()
            .child(
                thought_head("thought", ix, ms, open, palette)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_thought(ix, cx))),
            )
            .when(open, |row| row.child(thought_body(text, palette)))
            .into_any_element()
    }
}

/// A card's header or a fold's row: a static chevron (never a loader: only the now line spins) and one plain
/// sentence, which goes to the strongest ink under the pointer; then its facts. Its owner opens it.
pub(super) fn head_row(kind: &'static str, head: &Head, depth: u8, palette: Palette) -> Stateful<Div> {
    let anchor = head.key.anchor();
    let group: SharedString = format!("{kind}-head-{anchor}").into();
    h_flex()
        .id((kind, anchor as usize))
        .group(group.clone())
        .w_full()
        .min_h(px(28.))
        .pl(px(INDENT * depth as f32))
        .gap(px(8.))
        .text_size(px(14.))
        .cursor_pointer()
        .child(glyph(
            if head.open {
                Lucide::ChevronDown
            } else {
                Lucide::ChevronRight
            },
            palette.faint,
        ))
        .child(
            div()
                .flex_none()
                .text_color(palette.muted)
                .group_hover(group, |style| style.text_color(palette.foreground))
                .child(head.label.clone()),
        )
        .when_some(head.facts.clone(), |row, facts| {
            row.child(
                div()
                    .flex_none()
                    .text_size(px(13.))
                    .text_color(palette.faint)
                    .child(facts),
            )
        })
}

/// "Show N earlier steps", in an open card.
pub(super) fn earlier_row(kind: &'static str, key: isize, steps: usize, palette: Palette) -> Stateful<Div> {
    div()
        .id((kind, key as usize))
        .pl(px(INDENT))
        .py(px(4.))
        .text_size(px(13.))
        .text_color(palette.muted)
        .cursor_pointer()
        .hover(|style| style.text_color(palette.foreground))
        .child(work_folds::earlier_label(steps))
}

/// A thought's row (`ThinkingRow`): its chevron and "Thought for 14 s". Its owner opens it.
pub(super) fn thought_head(
    kind: &'static str,
    ix: usize,
    ms: Option<u64>,
    open: bool,
    palette: Palette,
) -> Stateful<Div> {
    let group: SharedString = format!("{kind}-{ix}").into();
    h_flex()
        .id((kind, ix))
        .group(group.clone())
        .w_full()
        .min_h(px(28.))
        .gap(px(8.))
        .text_size(px(14.))
        .cursor_pointer()
        .child(glyph(
            if open {
                Lucide::ChevronDown
            } else {
                Lucide::ChevronRight
            },
            palette.faint,
        ))
        .child(
            div()
                .text_color(palette.muted)
                .group_hover(group, |style| style.text_color(palette.foreground))
                .child(work_folds::thinking_label(ms)),
        )
}

/// An open thought's text, on the rail under its row.
pub(super) fn thought_body(text: &str, palette: Palette) -> Div {
    div()
        .ml(px(6.5))
        .pl(px(15.))
        .py(px(4.))
        .border_l_1()
        .border_color(palette.border)
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(palette.faint)
        .child(text.to_string())
}

/// How far a row in an open card, or a fold in it, sits in.
pub(super) const INDENT: f32 = 22.;

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::{Key, Place, layout};
    use crate::code::Block;

    fn tool(name: &str) -> Block {
        Block::Tool {
            call_id: String::new(),
            name: name.into(),
            summary: String::new(),
            error: None,
            edit: None,
            images: Vec::new(),
            call: Default::default(),
        }
    }

    fn turn() -> Block {
        Block::Turn {
            seq: 0,
            at: 0,
            status: nexus_workbench::event::TurnStatus::Completed,
            duration_ms: 0,
            cost_usd: None,
            tokens: 0,
            model: None,
            reverted: false,
        }
    }

    fn no() -> (HashMap<Key, bool>, HashSet<isize>) {
        (HashMap::new(), HashSet::new())
    }

    #[test]
    fn an_open_screenshot_card_keeps_routine_steps_folded() {
        let mut screenshot = tool("Read");
        let Block::Tool { images, .. } = &mut screenshot else {
            unreachable!()
        };
        images.push(crate::code_media::ToolImage {
            name: "screenshot.png".into(),
            media_type: "image/png",
            image: std::sync::Arc::new(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, Vec::new())),
        });
        let blocks = [
            screenshot,
            tool("Read"),
            tool("Read"),
            tool("Read"),
            tool("Read"),
            Block::Text("Done".into()),
            turn(),
        ];
        let (mut open, all) = no();
        let places = layout(&blocks, &open, &all, 0);
        assert!(!places[0].header.as_ref().expect("screenshot card").open);
        assert!(places[..5].iter().all(|place| place.hidden));
        open.insert(Key::Card(0), true);
        let places = layout(&blocks, &open, &all, 0);
        assert!(places[0].header.as_ref().expect("screenshot card").open);
        assert!(
            !places[0].hidden,
            "a returned screenshot stays visible after the turn ends"
        );
        assert!(places[1..5].iter().all(|place| place.hidden));
        assert!(
            places[1].fold.is_some(),
            "routine work still folds beside the screenshot"
        );
    }

    #[test]
    fn a_finished_run_of_routine_steps_closes_to_its_header() {
        let blocks = [
            tool("Read"),
            tool("Bash"),
            tool("Grep"),
            Block::Text("Done".into()),
            turn(),
        ];
        let (open, all) = no();
        let places = layout(&blocks, &open, &all, 0);
        let head = places[0].header.as_ref().expect("a header");
        assert_eq!(head.label, "Explored a file, ran a command");
        assert!(!head.open);
        assert!(places[..3].iter().all(|place| place.hidden));
        assert!(!places[3].hidden);
    }

    #[test]
    fn the_run_still_working_starts_closed_and_preserves_the_persons_choice() {
        let mut blocks = vec![tool("Read"), tool("Bash")];
        let (mut open, all) = no();
        let places = layout(&blocks, &open, &all, 0);
        assert!(!places[0].header.as_ref().expect("a header").open);
        assert!(places.iter().all(|place| place.hidden));

        open.insert(Key::Card(0), true);
        blocks.push(tool("Bash"));
        let places = layout(&blocks, &open, &all, 0);
        assert!(places[0].header.as_ref().expect("a header").open);
        assert!(places.iter().all(|place| !place.hidden && place.depth == 1));

        open.insert(Key::Card(0), false);
        blocks.push(tool("Bash"));
        assert!(layout(&blocks, &open, &all, 0).iter().all(|place| place.hidden));
        blocks.push(turn());
        assert!(layout(&blocks, &open, &all, 0)[..4].iter().all(|place| place.hidden));
    }

    #[test]
    fn an_edit_starts_closed_and_four_reads_beside_it_fold_when_opened() {
        let blocks = [
            tool("Read"),
            tool("Read"),
            tool("Read"),
            tool("Read"),
            tool("Edit"),
            turn(),
        ];
        let (mut open, all) = no();
        let places = layout(&blocks, &open, &all, 0);
        assert!(!places[0].header.as_ref().expect("a header").open);
        assert!(places[..5].iter().all(|place| place.hidden));
        open.insert(Key::Card(0), true);
        let places = layout(&blocks, &open, &all, 0);
        assert!(places[0].header.as_ref().expect("a header").open);
        let fold = places[0].fold.as_ref().expect("a fold");
        assert_eq!(fold.label, "Explored 4 files");
        assert_eq!(fold.facts.as_deref(), Some("4 steps"));
        assert!(places[..4].iter().all(|place| place.hidden));
        assert!(!places[4].hidden);

        open.insert(Key::Fold(0), true);
        assert!(layout(&blocks, &open, &all, 0)[..5].iter().all(|place| !place.hidden));
        open.insert(Key::Card(0), false);
        assert!(layout(&blocks, &open, &all, 0)[..5].iter().all(|place| place.hidden));
        open.insert(Key::Card(0), true);
        assert!(layout(&blocks, &open, &all, 0)[..5].iter().all(|place| !place.hidden));
    }

    #[test]
    fn what_the_person_opened_stays_open_by_its_key() {
        let blocks = [tool("Read"), tool("Bash"), turn()];
        let (mut open, all) = no();
        open.insert(Key::Card(0), true);
        assert!(
            layout(&blocks, &open, &all, 0)
                .iter()
                .take(2)
                .all(|place| !place.hidden)
        );
        // An older page put two blocks above: the same card is now at 2, with the same key.
        let older = [Block::Text("a".into()), turn(), tool("Read"), tool("Bash"), turn()];
        assert!(!layout(&older, &open, &all, 2)[2].hidden);
    }

    #[test]
    fn a_lone_step_and_the_plumbing_draw_as_before() {
        let blocks = [
            tool("Bash"),
            Block::Text("x".into()),
            tool("ToolSearch"),
            tool("Read"),
            turn(),
        ];
        let (open, all) = no();
        assert!(
            layout(&blocks, &open, &all, 0)
                .iter()
                .all(|place| *place == Place::default())
        );
    }

    #[test]
    fn a_long_card_offers_its_earlier_steps() {
        let mut blocks: Vec<Block> = (0..45).map(|_| tool("Edit")).collect();
        blocks.push(turn());
        let (mut open, mut all) = no();
        open.insert(Key::Card(0), true);
        let places = layout(&blocks, &open, &all, 0);
        assert_eq!(places[0].earlier, Some((0, 5)));
        assert!(places[..5].iter().all(|place| place.hidden));
        all.insert(0);
        assert!(layout(&blocks, &open, &all, 0).iter().all(|place| !place.hidden));
    }
    #[test]
    fn nested_agent_work_keeps_its_own_visibility_outside_routine_cards() {
        let nested = Block::Inside {
            path: vec!["agent".into()],
            block: Box::new(tool("Read")),
        };
        let blocks = [tool("Read"), tool("Bash"), nested, tool("Read"), tool("Bash"), turn()];
        let (open, all) = no();
        let places = layout(&blocks, &open, &all, 0);
        assert!(places[0].header.is_some());
        assert!(places[0].hidden && places[1].hidden);
        assert_eq!(places[2], Place::default(), "agent rows use their own fold state");
        assert!(places[3].header.is_some(), "routine cards never cross an agent body");
        assert!(places[3].hidden && places[4].hidden);
    }

    #[test]
    fn a_message_sent_into_the_run_leaves_its_layout_as_it_was() {
        let message = Block::User {
            seq: 3,
            at: 0,
            text: "also check the tests".into(),
            from: None,
            rewound: false,
            images: 0,
            image_data: Vec::new(),
        };
        let working = [tool("Read"), tool("Bash")];
        let sent = [tool("Read"), tool("Bash"), message];
        let (open, all) = no();
        assert_eq!(layout(&sent, &open, &all, 0)[..2], layout(&working, &open, &all, 0)[..]);
    }
}
