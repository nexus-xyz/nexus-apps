//! Opening a Code session at one of its events, as the Nexus app's message search does (`message-search.tsx`,
//! `timeline-jump.ts`): a sidebar "In messages" hit opens its session, reads older pages until the event is in the
//! transcript, scrolls to the block that holds it ([`crate::code::locate`]) and lights that block for a moment.
//! Another session opening drops the jump with its view.

use std::time::{Duration, Instant};

use gpui_kit::*;

use super::{View, Workspace};
use crate::code;

/// How long a block shown by a jump stays lit (the Nexus app's `FLASH_MS`).
const FLASH: Duration = Duration::from_millis(1600);
/// How many frames the block is brought into view again while the rows around it are measured.
const REVEAL_FRAMES: u8 = 3;

/// A jump asked for in the open session.
pub(super) struct Jump {
    seq: i64,
    /// The lit block, as an anchor that holds while older pages land above it (`CodeView::shifted`), and since when.
    shown: Option<(isize, Instant)>,
    reveal: u8,
}

impl Workspace {
    /// Open Code session `id` at event `seq`.
    pub(super) fn open_code_at(&mut self, id: &str, seq: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.open_code(id, window, cx);
        if let View::Code(view) = &mut self.view
            && view.id == id
        {
            view.jump = Some(Jump {
                seq,
                shown: None,
                reveal: 0,
            });
            cx.notify();
        }
    }

    /// The block lit by a jump, if `ix` is it.
    pub(super) fn jump_lit(&self, ix: usize) -> bool {
        let View::Code(view) = &self.view else {
            return false;
        };
        view.jump
            .as_ref()
            .and_then(|jump| jump.shown)
            .is_some_and(|(anchor, _)| anchor + view.shifted as isize == ix as isize)
    }

    /// Move a jump on: read the page before while the event is above the transcript, then scroll to its block and
    /// light it. Runs at the start of a frame.
    pub(super) fn refresh_code_jump(&mut self, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        let Some(jump) = view.jump.as_mut() else { return };
        if let Some((anchor, since)) = jump.shown {
            if since.elapsed() >= FLASH {
                view.jump = None;
                cx.notify();
            } else if jump.reveal > 0
                && let Ok(ix) = usize::try_from(anchor + view.shifted as isize)
            {
                jump.reveal -= 1;
                view.list.scroll_to_reveal_item(ix);
                cx.notify();
            }
            return;
        }
        if !view.loaded {
            return;
        }
        if view.more && view.start.is_some_and(|start| jump.seq < start) {
            if view.reading.is_none() && view.asked_before != view.start {
                let this = cx.entity().downgrade();
                cx.defer(move |cx| {
                    let _ = this.update(cx, |this, cx| this.read_older_code(cx));
                });
            }
            return;
        }
        let Some(ix) = code::locate(&view.starts, jump.seq) else {
            view.jump = None;
            return;
        };
        // A hit inside a completed sub-agent must open its ancestors before it can be revealed.
        if let Some(block) = view.blocks.get(ix) {
            for call in block.path() {
                view.open_agents.insert(call.clone(), true);
            }
        }
        view.list.remeasure_items(0..view.blocks.len());
        // A list following its end would snap back to it.
        view.list.pause_following_tail();
        view.list.scroll_to_reveal_item(ix);
        jump.shown = Some((ix as isize - view.shifted as isize, Instant::now()));
        jump.reveal = REVEAL_FRAMES;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FLASH).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }
}
