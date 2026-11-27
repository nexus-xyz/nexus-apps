//! A rich block's corner: its kind (`latex` or `mermaid`) in faint Geist Mono, a button that shows the code or the
//! drawing (Retry instead, when the drawing failed), then Copy. The parts are the code block's (`code_block.rs`), so
//! math and diagrams act like the code around them.
//!
//! Over the drawing the corner shows only under the pointer. Prose has no head row, so it floats just above the
//! block's top right on the menu surface, never over the block's own words. Over the code it sits in the well's head,
//! as a code block's does, and stays.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::motion::{self, Tween};
use crate::palette::Palette;
use crate::workspace::code_block;

/// The corner's buttons and their gaps, as a code block's.
const BUTTON: f32 = 24.;
/// The floating corner: 2 in from its edge around 24 buttons, and 4 clear of the block below it.
const FLOAT_HEIGHT: f32 = 28.;
const FLOAT_GAP: f32 = 4.;
/// The corner's radius, the hover wash's and the buttons' (`DESIGN.md`, controls and rows).
pub(super) const RADIUS: f32 = 8.;

/// The words of the button that switches views: what it shows next. Never "Source": that word is the web's sources.
pub(super) fn toggle_tip(diagram: bool, showing_code: bool) -> &'static str {
    match (diagram, showing_code) {
        (false, false) => "Show LaTeX",
        (false, true) => "Show math",
        (true, false) => "Show code",
        (true, true) => "Show diagram",
    }
}

/// The corner's label: the code's language, as a code block's.
pub(super) fn kind(diagram: bool) -> &'static str {
    if diagram { "mermaid" } else { "latex" }
}

/// Where the corner sits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Place {
    /// Just above the drawing's top right, on the menu surface, under the pointer only.
    Float,
    /// In the code well's 32 head, always.
    Head,
}

/// What the corner's middle button does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Action {
    Toggle { showing_code: bool },
    Retry,
}

/// The fade of a floating corner: in over `motion::HOVER` on the settle curve, out over `motion::UI_EXIT`.
pub(super) fn fade(shown: bool) -> Tween {
    if shown {
        Tween::new(motion::HOVER, |t| motion::cubic_bezier(0., 0., 0.2, 1., t))
    } else {
        Tween::new(motion::UI_EXIT, motion::standard)
    }
}

/// The corner's row. `on_action` runs the middle button.
pub(super) fn corner(
    place: Place,
    diagram: bool,
    action: Action,
    code: SharedString,
    palette: Palette,
    on_action: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let middle = match action {
        Action::Toggle { showing_code } => code_block::icon_button(
            "rich-toggle",
            if showing_code { Lucide::Eye } else { Lucide::Code },
            toggle_tip(diagram, showing_code),
            palette,
        )
        .debug_selector(|| "rich-toggle".into()),
        Action::Retry => code_block::icon_button("rich-retry", Lucide::RotateCcw, "Retry", palette)
            .debug_selector(|| "rich-retry".into()),
    }
    .on_click(move |event, window, cx| {
        cx.stop_propagation();
        on_action(event, window, cx);
    });
    let row = h_flex()
        .id("rich-corner")
        .absolute()
        .h(px(BUTTON))
        .gap(px(4.))
        .font_family(".SystemUIFont")
        .debug_selector(|| "rich-corner".into())
        .child(
            div()
                .pr(px(4.))
                .font_family("Geist Mono")
                .text_size(px(12.))
                .line_height(px(BUTTON))
                .text_color(palette.faint)
                .child(kind(diagram)),
        )
        .child(middle)
        .child(code_block::copy_button(code, palette));
    match place {
        Place::Head => row.top(px(4.)).right(px(4.)),
        // Its own surface over whatever is above the block; it takes the pointer, so text under it is not selected.
        Place::Float => row
            .top(px(-(FLOAT_HEIGHT + FLOAT_GAP)))
            .right(px(-4.))
            .h(px(FLOAT_HEIGHT))
            .pl(px(6.))
            .pr(px(2.))
            .rounded(px(RADIUS))
            .bg(palette.menu)
            .border_1()
            .border_color(palette.border)
            .occlude(),
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn the_toggle_names_what_it_shows_next_and_never_says_source() {
        assert_eq!(toggle_tip(false, false), "Show LaTeX");
        assert_eq!(toggle_tip(false, true), "Show math");
        assert_eq!(toggle_tip(true, false), "Show code");
        assert_eq!(toggle_tip(true, true), "Show diagram");
        for diagram in [false, true] {
            for showing in [false, true] {
                assert!(!toggle_tip(diagram, showing).contains("Source"));
            }
            assert!(!kind(diagram).to_lowercase().contains("source"));
        }
    }
}
