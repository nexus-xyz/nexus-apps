//! A sent message shows in the side history at once, as a session's does. The side chat reads its store every two
//! seconds, so until a read brings the message's own event, its bubble draws from what was sent. A send that lands
//! wakes the read at once (`active`).
use super::*;
use crate::palette::Palette;
use crate::workspace::code_view;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;

/// Reads after its send landed before an echo goes even when no event matched its text: the store has it by then.
const READS_AFTER_LANDING: u8 = 2;

/// A message sent from the composer that no read has brought yet.
pub(super) struct Echo {
    text: String,
    /// The last event when it was sent: its own event comes after.
    after: i64,
    /// Once its send landed, the reads left before it goes.
    reads: Option<u8>,
}

impl Echo {
    /// A new side chat's first message, which landed with its creation: the first reads bring it.
    pub(super) fn landed(text: String) -> Self {
        Echo {
            text,
            after: 0,
            reads: Some(READS_AFTER_LANDING),
        }
    }
}

impl SideChat {
    /// `text` left the composer: its bubble shows now. A message queued behind a running turn waits over the composer
    /// as a Queued chip instead, so it has none.
    pub(super) fn echo_sent(&mut self, text: &str, queued: bool) {
        if text.trim().is_empty() || (queued && self.summary.status.busy()) {
            return;
        }
        let after = self.events.last().map_or(0, |event| event.seq);
        self.echo.push(Echo {
            text: text.to_string(),
            after,
            reads: None,
        });
    }

    /// The send of `text` landed, or it failed and went back to the composer.
    pub(super) fn echo_settled(&mut self, text: &str, landed: bool) {
        let Some(ix) = self
            .echo
            .iter()
            .position(|echo| echo.reads.is_none() && echo.text == text)
        else {
            return;
        };
        if landed {
            self.echo[ix].reads = Some(READS_AFTER_LANDING);
        } else {
            self.echo.remove(ix);
        }
    }

    /// A read landed: an echo goes once the read brings its own message, or a few reads after its send landed.
    pub(super) fn echo_read(&mut self) {
        let mut taken = HashSet::new();
        let blocks = &self.blocks;
        self.echo.retain_mut(|echo| {
            let own = blocks.iter().find_map(|block| match block.inner() {
                Block::User {
                    seq, text, from: None, ..
                } if *seq > echo.after && !taken.contains(seq) && text.trim() == echo.text.trim() => Some(*seq),
                _ => None,
            });
            if let Some(seq) = own {
                taken.insert(seq);
                return false;
            }
            match &mut echo.reads {
                Some(reads) => {
                    *reads = reads.saturating_sub(1);
                    *reads > 0
                }
                None => true,
            }
        });
    }

    /// The texts the history draws as sent: they change the tail's height.
    pub(super) fn echo_texts(&self) -> Vec<&str> {
        self.echo.iter().map(|echo| echo.text.as_str()).collect()
    }

    /// The sent messages' bubbles, as the person's own (`code_view::user_prompt`).
    pub(super) fn render_echo(&self, p: Palette) -> Vec<AnyElement> {
        self.echo
            .iter()
            .map(|echo| {
                v_flex()
                    .debug_selector(|| "side-echo".into())
                    .w_full()
                    .items_end()
                    .pt(px(10.))
                    .child(code_view::user_prompt(echo.text.clone(), vec![], None, p, None))
                    .into_any_element()
            })
            .collect()
    }
}
