//! Up and Down in a Code session's empty composer walk back through what the person sent in that session, like a
//! shell. The rules port the Nexus app's `prompt-history.ts` (`ui/src/components/nexus-ui/chat/`), with one change:
//! a walk starts only from an empty composer, so Up never replaces what the person is typing.

use std::collections::HashSet;
use std::ops::Range;

use gpui_kit::{Context, Window};

use super::{View, Workspace};
use crate::code::Block;

/// Where a walk is: an index into the entries (oldest first), and the text the last step put in the composer. A
/// different text means the person edited it, and the walk ends.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Walk {
    index: usize,
    shown: String,
}

/// Where the caret goes after a step: Up puts it at the start, Down at the end, as the Nexus app does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Caret {
    Start,
    End,
}

/// One Up or Down: the text to show, where the caret goes, and the walk for the next step (`None` once Down passes
/// the newest message and the composer is empty again).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Step {
    pub walk: Option<Walk>,
    pub text: String,
    pub caret: Caret,
}

/// What the person sent in a session, oldest first: its `User` blocks, without the ones a linked session wrote and
/// without `skip` (the Continue bar's message, which the person did not type). `sentPrompts` in `prompt-history.ts`.
pub(super) fn sent_prompts(blocks: &[Block], skip: &str) -> Vec<String> {
    let sent = blocks.iter().filter_map(|block| match block {
        Block::User { text, from: None, .. } if text != skip => Some(text.as_str()),
        _ => None,
    });
    entries(sent)
}

/// The walk's entries, oldest first: blanks dropped, and a message sent twice kept once, in its newest place.
fn entries<'a>(sent: impl DoubleEndedIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out: Vec<String> = sent
        .rev()
        .map(str::trim)
        .filter(|text| !text.is_empty() && seen.insert(*text))
        .map(str::to_string)
        .collect();
    out.reverse();
    out
}

fn caret_on_first_line(text: &str, caret: usize) -> bool {
    !text.get(..caret).unwrap_or(text).contains('\n')
}

fn caret_on_last_line(text: &str, caret: usize) -> bool {
    !text.get(caret..).unwrap_or("").contains('\n')
}

/// What Up (`down` false) or Down does in a composer that shows `text` with the selection `selected`. `None` when the
/// key is not a step, and the text moves the caret as usual: a selection; text the person typed or edited; the caret
/// below the first line (Up) or above the last (Down) of a recalled message; nothing older to recall; Down with no
/// walk.
pub(super) fn recall(
    down: bool,
    text: &str,
    selected: Range<usize>,
    entries: &[String],
    walk: Option<&Walk>,
) -> Option<Step> {
    if !selected.is_empty() {
        return None;
    }
    let walking = walk.filter(|walk| walk.shown == text);
    if !down {
        // A walk starts only from an empty composer; an unedited recalled message goes on with it.
        if walking.is_none() && !text.is_empty() {
            return None;
        }
        if !caret_on_first_line(text, selected.start) {
            return None;
        }
        let index = match walking {
            Some(walk) => walk.index.checked_sub(1)?,
            None => entries.len().checked_sub(1)?,
        };
        return show(entries, index, Caret::Start);
    }
    let walk = walking?;
    if !caret_on_last_line(text, selected.start) {
        return None;
    }
    let index = walk.index + 1;
    if index >= entries.len() {
        // Past the newest message: the composer is empty again.
        let text = String::new();
        return Some(Step {
            walk: None,
            text,
            caret: Caret::End,
        });
    }
    show(entries, index, Caret::End)
}

/// The entry at `index`, shown; `None` when the session's messages changed under the walk and it is gone.
fn show(entries: &[String], index: usize, caret: Caret) -> Option<Step> {
    let shown = entries.get(index)?.clone();
    let text = shown.clone();
    Some(Step {
        walk: Some(Walk { index, shown }),
        text,
        caret,
    })
}

impl Workspace {
    /// Up (`down` false) or Down in the composer of an open Code session: one step through the messages the person
    /// sent in it. False when the key is not a step, so the text moves the caret as usual.
    pub(super) fn recall_sent(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let View::Code(view) = &self.view else {
            return false;
        };
        if view.pending.is_some() {
            return false;
        }
        let composer = self.composer.read(cx);
        let text = composer.value().to_string();
        let selected = composer.selected_range();
        let entries = sent_prompts(&view.blocks, nexus_workbench::CONTINUE);
        let Some(step) = recall(down, &text, selected, &entries, view.recall.as_ref()) else {
            return false;
        };
        if let View::Code(view) = &mut self.view {
            view.recall = step.walk;
        }
        let at = match step.caret {
            Caret::Start => 0,
            Caret::End => step.text.len(),
        };
        self.composer_empty = step.text.trim().is_empty();
        self.composer.update(cx, |state, cx| {
            if state.value().as_ref() != step.text {
                state.set_value(step.text, window, cx);
            }
            state.set_selected_range(at..at, cx);
        });
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(entries: &[String], steps: &[(bool, &str, usize)]) -> Vec<Option<Step>> {
        let mut walk = None;
        steps
            .iter()
            .map(|&(down, text, caret)| {
                let step = recall(down, text, caret..caret, entries, walk.as_ref());
                if let Some(step) = &step {
                    walk = step.walk.clone();
                }
                step
            })
            .collect()
    }

    fn texts(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|text| text.to_string()).collect()
    }

    fn user(text: &str, from: Option<nexus_workbench::event::Sender>) -> Block {
        Block::User {
            seq: 0,
            at: 0,
            text: text.into(),
            from,
            rewound: false,
            images: 0,
            image_data: Vec::new(),
        }
    }

    #[test]
    fn up_in_an_empty_composer_walks_back_from_the_newest_and_down_returns_to_empty() {
        let entries = texts(&["one", "two", "three"]);
        let steps = walk(
            &entries,
            &[
                (false, "", 0),
                (false, "three", 0),
                (false, "two", 0),
                (true, "one", 3),
                (true, "two", 3),
                (true, "three", 5),
            ],
        );
        let shown: Vec<_> = steps
            .iter()
            .map(|step| step.as_ref().map(|step| step.text.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                Some("three"),
                Some("two"),
                Some("one"),
                Some("two"),
                Some("three"),
                Some("")
            ]
        );
        assert_eq!(steps[0].as_ref().unwrap().caret, Caret::Start);
        assert_eq!(steps[3].as_ref().unwrap().caret, Caret::End);
        // Past the newest, the walk is over: a second Down does nothing.
        assert_eq!(steps[5].as_ref().unwrap().walk, None);
        assert_eq!(recall(true, "", 0..0, &entries, None), None);
    }

    #[test]
    fn up_at_the_oldest_message_leaves_the_key_to_the_text() {
        let entries = texts(&["one", "two"]);
        let steps = walk(&entries, &[(false, "", 0), (false, "two", 0), (false, "one", 0)]);
        assert_eq!(steps[2], None);
    }

    #[test]
    fn text_the_person_typed_is_never_replaced() {
        let entries = texts(&["one"]);
        assert_eq!(recall(false, "draft", 0..0, &entries, None), None);
        assert_eq!(recall(true, "draft", 5..5, &entries, None), None);
    }

    #[test]
    fn an_edited_recalled_message_ends_the_walk() {
        let entries = texts(&["one", "two"]);
        let up = recall(false, "", 0..0, &entries, None).unwrap();
        assert_eq!(recall(false, "two!", 0..0, &entries, up.walk.as_ref()), None);
        assert_eq!(recall(true, "two!", 4..4, &entries, up.walk.as_ref()), None);
    }

    #[test]
    fn nothing_sent_means_nothing_to_recall() {
        assert_eq!(recall(false, "", 0..0, &[], None), None);
    }

    #[test]
    fn a_multi_line_message_moves_the_caret_until_its_first_or_last_line() {
        let entries = texts(&["a\nb", "c"]);
        let walk = Walk {
            index: 0,
            shown: "a\nb".into(),
        };
        // Up below the first line, and Down above the last, move the caret.
        assert_eq!(recall(false, "a\nb", 3..3, &entries, Some(&walk)), None);
        assert_eq!(recall(true, "a\nb", 0..0, &entries, Some(&walk)), None);
        // On the last line, Down steps on.
        assert_eq!(recall(true, "a\nb", 3..3, &entries, Some(&walk)).unwrap().text, "c");
        // On the first line, Up would step back, but this is the oldest message.
        let newer = Walk {
            index: 1,
            shown: "c".into(),
        };
        assert_eq!(recall(false, "c", 0..0, &entries, Some(&newer)).unwrap().text, "a\nb");
    }

    #[test]
    fn a_selection_leaves_the_key_to_the_text() {
        let entries = texts(&["one", "abc"]);
        let walk = Walk {
            index: 1,
            shown: "abc".into(),
        };
        assert_eq!(recall(false, "abc", 0..2, &entries, Some(&walk)), None);
        assert_eq!(recall(true, "abc", 0..2, &entries, Some(&walk)), None);
    }

    #[test]
    fn the_history_is_what_the_person_sent_in_the_session_once_each_newest_place() {
        let linked = nexus_workbench::event::Sender::Session {
            thread_id: "parent".into(),
            title: "Parent".into(),
        };
        let blocks = vec![
            user("one", None),
            Block::Text("answer".into()),
            user("  two  ", None),
            user(nexus_workbench::CONTINUE, None),
            user("from a linked session", Some(linked)),
            user("   ", None),
            user("one", None),
        ];
        assert_eq!(sent_prompts(&blocks, nexus_workbench::CONTINUE), ["two", "one"]);
    }
}

#[cfg(test)]
#[path = "prompt_history_tests.rs"]
mod workspace_tests;
