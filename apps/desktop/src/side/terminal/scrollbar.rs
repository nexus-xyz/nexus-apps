//! The scrollback bar: a thin thumb on the terminal's right edge that shows where the view is in the scrollback. It
//! shows while the wheel moves the view and while the pointer is over the terminal, and fades out
//! `motion::SCROLLBAR_LINGER` after the last wheel turn (`motion::fade_to`). Nothing moves while the terminal is idle.
//!
//! The thumb is worked out in the handlers that change it (a wheel turn, the pointer, new output), never in `render`:
//! the reader thread holds the terminal's lock while it parses output.

use std::time::Instant;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Term;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{LINE_HEIGHT, Listener, PADDING, Terminal};
use crate::motion::{SCROLLBAR_LINGER, fade_to};
use crate::palette::Palette;

/// The thumb's width, its inset from the right edge, and its shortest length.
const WIDTH: f32 = 4.;
const INSET: f32 = 2.;
const SHORTEST: f32 = 16.;
/// The thumb's opacity while it shows: a hint, under the text.
const SHOWN: f32 = 0.45;

#[derive(Default)]
pub(super) struct Scrollbar {
    /// Top and length as fractions of the track; None while there is no scrollback.
    thumb: Option<(f32, f32)>,
    scrolled_at: Option<Instant>,
    hovered: bool,
    _linger: Option<Task<()>>,
}

impl Scrollbar {
    /// Read the thumb from the grid as it is now.
    pub(super) fn refresh(&mut self, term: &Term<Listener>) {
        self.thumb = thumb(
            term.grid().history_size(),
            term.screen_lines(),
            term.grid().display_offset(),
        );
    }

    /// The wheel moved the view: show the bar, and draw again once it has lingered, so it fades out.
    pub(super) fn scrolled(&mut self, cx: &mut Context<Terminal>) {
        self.scrolled_at = Some(Instant::now());
        self._linger = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SCROLLBAR_LINGER).await;
            // The view may have closed meanwhile: then there is nothing to fade.
            this.update(cx, |_, cx| cx.notify()).ok();
        }));
    }

    /// The pointer came over the terminal or left it. True when that changes what shows.
    pub(super) fn hover(&mut self, hovered: bool) -> bool {
        std::mem::replace(&mut self.hovered, hovered) != hovered
    }

    fn shown(&self) -> bool {
        self.hovered || self.scrolled_at.is_some_and(|at| at.elapsed() < SCROLLBAR_LINGER)
    }

    /// The thumb over a grid of `rows` lines, or nothing while there is no scrollback.
    pub(super) fn render(&self, rows: u16, palette: Palette) -> Option<AnyElement> {
        let (top, length) = self.thumb?;
        let track = f32::from(rows) * LINE_HEIGHT;
        let (y, height) = place(top, length, track);
        let bar = div()
            .absolute()
            .right(px(INSET))
            .top(px(PADDING + y))
            .w(px(WIDTH))
            .h(px(height))
            .rounded(px(WIDTH / 2.))
            .bg(palette.icon);
        Some(fade_to(bar, "terminal-scrollbar", if self.shown() { SHOWN } else { 0. }).into_any_element())
    }
}

/// Where the view is: the thumb's top and length as fractions of the track, for `history` lines above a screen of
/// `screen` lines, viewed `offset` lines up from the bottom. None while there is no scrollback.
fn thumb(history: usize, screen: usize, offset: usize) -> Option<(f32, f32)> {
    if history == 0 || screen == 0 {
        return None;
    }
    let total = (history + screen) as f32;
    Some(((history - offset.min(history)) as f32 / total, screen as f32 / total))
}

/// The thumb's top and height in pixels on a `track` px track. A long scrollback still leaves a thumb of `SHORTEST`,
/// and the rest of the track maps the place.
fn place(top: f32, length: f32, track: f32) -> (f32, f32) {
    let height = (length * track).clamp(SHORTEST.min(track), track);
    let room = 1. - length;
    let y = if room > 0. { top / room * (track - height) } else { 0. };
    (y, height)
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn the_thumb_shows_where_the_view_is_in_the_scrollback() {
        // No scrollback: no thumb.
        assert_eq!(thumb(0, 24, 0), None);
        // At the bottom of 72 lines above a screen of 24: the last quarter.
        assert_eq!(thumb(72, 24, 0), Some((0.75, 0.25)));
        // At the top.
        assert_eq!(thumb(72, 24, 72), Some((0., 0.25)));
        // Halfway up.
        assert_eq!(thumb(72, 24, 36), Some((0.375, 0.25)));
    }

    #[test]
    fn a_long_scrollback_keeps_a_thumb_to_grab_and_maps_the_rest() {
        // 10,000 lines above 24: the thumb would be under a pixel, so it is 16px, and the bottom still ends the track.
        let (top, length) = thumb(10_000, 24, 0).unwrap();
        let (y, height) = place(top, length, 432.);
        assert_eq!(height, SHORTEST);
        assert!((y + height - 432.).abs() < 0.01, "{y} + {height}");
        assert_eq!(place(0., length, 432.).0, 0.);
    }
}
