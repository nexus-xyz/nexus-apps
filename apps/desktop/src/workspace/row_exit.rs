//! A deleted Code session's row leaving the sidebar. The row starts to leave on the confirm click, before the delete
//! call answers: it tints red, fades and slides left (`motion::UI_EXIT`), then its height folds to nothing
//! (`motion::COLLAPSE`), so the rows under it glide up. A folded row keeps a slot of no height until the session list
//! drops it, so a slow call never shows a jump. When the call fails, the slot opens again and a short red ring marks
//! the row that stayed.
//!
//! Before this, the row stayed as it was until the list refreshed, then left in one frame, and the rows under it
//! jumped up by a row.
//!
//! Each frame is a plain function of the time since the row started to leave or to come back, so the row and the
//! fixed-height groups it sits in read the same height in the same frame.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::sidebar::ROW;
use crate::motion;
use crate::palette::Palette;

/// The fade and slide out.
const LEAVE: Duration = motion::UI_EXIT;
/// The height folding to nothing, after the fade.
const FOLD: Duration = motion::COLLAPSE;
/// The red ring around a row that came back.
const RING: Duration = Duration::from_millis(900);
/// How far left the row's content slides as it fades.
const SLIDE: f32 = 12.;
/// The red tint's strength under a leaving row.
const TINT: f32 = 0.12;
/// How long "Deleted …" stays at the foot of the window.
pub(super) const DELETED_STAY: Duration = Duration::from_millis(5_000);

/// What the notice says once session `title` is deleted.
pub(super) fn deleted_text(title: &str) -> String {
    format!("Deleted \u{201c}{title}\u{201d}")
}

/// What the notice says when session `title` could not be deleted.
pub(super) fn delete_failed_text(title: &str, error: &str) -> String {
    format!("Couldn\u{2019}t delete \u{201c}{title}\u{201d}: {error}")
}

/// How to draw a row now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Frame {
    /// The row's height, as a share of `ROW`.
    pub height: f32,
    /// The content's opacity.
    pub content: f32,
    /// The content's offset to the left, in pixels (negative moves it left).
    pub shift: f32,
    /// The red tint's strength under the row, 0 to 1.
    pub tint: f32,
    /// The red ring's strength around the row, 0 to 1.
    pub ring: f32,
    /// Whether it still moves: the row asks for the next frame.
    pub moving: bool,
    /// Whether the row is on its way out: it takes no pointer.
    pub leaving: bool,
}

fn share(elapsed: Duration, length: Duration) -> f32 {
    (elapsed.as_secs_f32() / length.as_secs_f32()).clamp(0., 1.)
}

/// The exit curve, `cubic-bezier(0.4, 0, 1, 1)`: what leaves speeds up as it goes.
fn exit_curve(t: f32) -> f32 {
    motion::cubic_bezier(0.4, 0., 1., 1., t)
}

/// A leaving row, `elapsed` after the click. Less motion folds it at once.
pub(super) fn leaving(elapsed: Duration, reduced: bool) -> Frame {
    if reduced {
        return Frame {
            height: 0.,
            content: 0.,
            shift: 0.,
            tint: 0.,
            ring: 0.,
            moving: false,
            leaving: true,
        };
    }
    let out = exit_curve(share(elapsed, LEAVE));
    let fold = motion::standard(share(elapsed.saturating_sub(LEAVE), FOLD));
    Frame {
        height: 1. - fold,
        content: 1. - out,
        shift: -SLIDE * out,
        tint: 1. - fold,
        ring: 0.,
        moving: elapsed < LEAVE + FOLD,
        leaving: true,
    }
}

/// A row coming back from height `from`, `elapsed` after its delete failed. Less motion shows it in place.
pub(super) fn returning(elapsed: Duration, from: f32, reduced: bool) -> Frame {
    if reduced {
        return Frame {
            height: 1.,
            content: 1.,
            shift: 0.,
            tint: 0.,
            ring: 0.,
            moving: false,
            leaving: false,
        };
    }
    let open = motion::standard(share(elapsed, FOLD));
    let fade = motion::ease_out(share(elapsed, FOLD + LEAVE));
    Frame {
        height: from + (1. - from) * open,
        content: fade,
        shift: -SLIDE * (1. - fade),
        tint: 0.,
        ring: 1. - share(elapsed, RING),
        moving: elapsed < RING,
        leaving: false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Exit {
    Leaving { since: Instant },
    Returning { since: Instant, from: f32 },
}

impl Exit {
    fn frame(self, now: Instant, reduced: bool) -> Frame {
        match self {
            Exit::Leaving { since } => leaving(now.saturating_duration_since(since), reduced),
            Exit::Returning { since, from } => returning(now.saturating_duration_since(since), from, reduced),
        }
    }
}

/// The rows leaving the sidebar, or coming back, by session id. One owner: `delete_session` starts a row, the delete's
/// answer brings it back, and the session list's refresh forgets the ones it dropped.
#[derive(Default)]
pub(super) struct RowExits {
    rows: HashMap<String, Exit>,
}

impl RowExits {
    /// Session `id`'s row starts to leave.
    pub fn start(&mut self, id: &str, now: Instant) {
        self.rows.insert(id.to_string(), Exit::Leaving { since: now });
    }

    /// Session `id`'s delete failed: its row comes back from wherever it is now.
    pub fn bring_back(&mut self, id: &str, now: Instant, reduced: bool) {
        let from = self.rows.get(id).map_or(1., |exit| exit.frame(now, reduced).height);
        self.rows.insert(id.to_string(), Exit::Returning { since: now, from });
    }

    /// Keep only the rows whose session is still `listed`, and drop the ones that have come back and rest.
    pub fn forget(&mut self, listed: impl Fn(&str) -> bool, now: Instant, reduced: bool) {
        self.rows.retain(|id, exit| {
            let frame = exit.frame(now, reduced);
            listed(id) && (frame.leaving || frame.moving)
        });
    }

    /// How to draw session `id`'s row now; `None` draws it as usual.
    pub fn frame(&self, id: &str, now: Instant, reduced: bool) -> Option<Frame> {
        let frame = self.rows.get(id)?.frame(now, reduced);
        (frame.leaving || frame.moving).then_some(frame)
    }

    /// The height of the rows of `ids`, in pixels: `ROW` each, less what the leaving ones have folded.
    pub fn height<'a>(&self, ids: impl IntoIterator<Item = &'a str>, now: Instant, reduced: bool) -> f32 {
        ids.into_iter()
            .map(|id| self.frame(id, now, reduced).map_or(1., |frame| frame.height) * ROW)
            .sum()
    }
}

/// Draw `row` as `frame`: its slot at `frame.height`, the tint and the ring behind and around it, its content faded
/// and shifted. A leaving row takes no pointer, so a second click cannot open it or its menu.
pub(super) fn draw(row: AnyElement, id: &str, frame: Frame, palette: Palette, window: &mut Window) -> AnyElement {
    if frame.moving {
        window.request_animation_frame();
    }
    div()
        .id(SharedString::from(format!("row-exit-{id}")))
        .relative()
        .flex_none()
        .w_full()
        .h(px(ROW * frame.height))
        .when(frame.height < 1., |slot| slot.overflow_hidden())
        .when(frame.tint > 0., |slot| {
            slot.child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(8.))
                    .bg(palette.destructive.opacity(TINT * frame.tint)),
            )
        })
        .child(div().relative().left(px(frame.shift)).opacity(frame.content).child(row))
        .when(frame.ring > 0., |slot| {
            slot.child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(palette.destructive.opacity(frame.ring)),
            )
        })
        .when(frame.leaving, |slot| {
            slot.child(div().id("row-exit-shield").absolute().inset_0().occlude())
        })
        .into_any_element()
}

#[cfg(test)]
#[path = "row_exit_tests.rs"]
mod tests;
