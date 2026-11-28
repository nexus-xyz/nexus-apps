//! A queued message's row opens under the pointer, as a stacked reply's chip does (`stacked_replies/chips.rs`): its
//! whole text eases in under the one-line head (`motion::reveal`, the composer's own open and close), so a long
//! message reads in full without opening the editor. A message that fits on its line has nothing more to show.

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;
use crate::motion;
use crate::palette::Palette;

impl Workspace {
    /// The pointer came onto queued row `seq` of `surface`, or left it.
    pub(super) fn queue_hovered(&mut self, surface: &'static str, seq: u64, hovered: bool, cx: &mut Context<Self>) {
        if self.queue_hover.set(surface, seq, hovered) {
            cx.notify();
        }
    }
}

/// Past this many characters, or with a second line, a queued message's head cuts it short.
const ONE_LINE: usize = 56;

/// Whether `text` says more than its one-line head shows.
pub(super) fn opens(text: &str) -> bool {
    let text = text.trim();
    text.contains('\n') || text.chars().count() > ONE_LINE
}

/// The whole of a queued message under its head, while `open`. `indent`: where the head's text starts, so the
/// message lines up under its own first words.
pub(super) fn body(
    id: impl Into<ElementId>,
    open: bool,
    text: &str,
    indent: f32,
    palette: Palette,
) -> impl IntoElement {
    let content = (open && opens(text)).then(|| {
        div()
            .pl(px(indent))
            .pr(px(12.))
            .pb(px(8.))
            .line_height(px(20.))
            .text_color(palette.foreground)
            .whitespace_normal()
            .child(text.trim().to_string())
            .into_any_element()
    });
    motion::reveal(id, open && content.is_some(), content)
}

/// A queued chip's hairline box: its head, and under the pointer its whole message ([`body`]), inside one border.
pub(super) fn frame(id: impl Into<ElementId>, fill: Hsla, palette: Palette) -> Stateful<Div> {
    gpui_kit::component::v_flex()
        .id(id)
        .w_full()
        .rounded(px(10.))
        .border_1()
        .border_color(palette.border)
        .bg(fill)
        .text_size(px(13.))
}

/// A queued chip (`code_dock::queued_chip`) as a frame's head: the frame draws the box, so the chip draws none.
pub(super) fn in_frame(chip: Stateful<Div>) -> Stateful<Div> {
    chip.border_0().rounded(px(0.)).bg(transparent_black())
}

/// Where a chip's text starts: past its 10px edge, its place number and the 8px gap; and past a grip.
pub(super) fn chip_indent(grip: bool) -> f32 {
    if grip { 50. } else { 26. }
}

/// Which queued row is under the pointer, by surface and message: one at a time opens.
#[derive(Default)]
pub(super) struct Hovered(Option<(&'static str, u64)>);

impl Hovered {
    pub(super) fn is(&self, surface: &'static str, seq: u64) -> bool {
        self.0 == Some((surface, seq))
    }

    /// The pointer came onto row `seq` of `surface`, or left it. True when that changed what shows.
    pub(super) fn set(&mut self, surface: &'static str, seq: u64, hovered: bool) -> bool {
        let next = if hovered {
            Some((surface, seq))
        } else if self.is(surface, seq) {
            None
        } else {
            return false;
        };
        let changed = self.0 != next;
        self.0 = next;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::{Hovered, opens};

    #[test]
    fn only_a_message_longer_than_its_line_opens() {
        assert!(!opens("Run the tests"));
        assert!(opens("First line\nsecond line"));
        assert!(opens(&"word ".repeat(20)));
        assert!(!opens("  short, with spaces around  \n"));
    }

    #[test]
    fn one_row_opens_at_a_time_and_a_late_leave_does_not_close_the_next() {
        let mut hovered = Hovered::default();
        assert!(hovered.set("code", 2, true));
        assert!(hovered.set("code", 3, true));
        assert!(
            !hovered.set("code", 2, false),
            "leaving 2 after entering 3 leaves 3 open"
        );
        assert!(hovered.is("code", 3));
        assert!(hovered.set("code", 3, false));
        assert!(!hovered.is("code", 3));
    }
}
