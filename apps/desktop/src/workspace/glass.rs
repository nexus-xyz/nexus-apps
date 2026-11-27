//! Frosted glass: a surface that blurs what is drawn under it, then tints it, as Zeron's composer, menus and toasts
//! are (`ui/src/glass.rs`, blur 16). The blur is GPUI's backdrop blur (`Window::paint_backdrop_blur`, ported from
//! zui into `vendor/gpui-pre`). A renderer that cannot blur skips it, and the tint alone shows, so the tint is strong
//! enough to read text over. How far it blurs is the Transparency setting's Blur (`frost::Frost::glass_px`): 16 px by
//! default, none at 0.
//!
//! The new session's art has its own, cheaper frost (`backdrop::frost`): the art does not move, so it is blurred once.

use crate::palette::Palette;
use gpui_kit::{
    BorderStyle, Corners, Hsla, InteractiveElement, IntoElement, ParentElement, Styled, canvas, div, fill, px, quad,
    transparent_black,
};

/// Glass under a surface with corners of `radius`: put it first in a `.relative()` box, in place of the box's `bg`.
/// `tint` is laid over the blur; `ring`, when given, is a 1px edge.
pub(super) fn glass(radius: f32, tint: Hsla, ring: Option<Hsla>) -> impl IntoElement {
    let blur = crate::frost::shown().glass_px();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let corners = Corners::all(px(radius));
            if blur > 0. {
                window.paint_backdrop_blur(bounds, corners, px(blur));
            }
            window.paint_quad(fill(bounds, tint).corner_radii(corners));
            if let Some(ring) = ring {
                window.paint_quad(quad(
                    bounds,
                    corners,
                    transparent_black(),
                    px(1.),
                    ring,
                    BorderStyle::Solid,
                ));
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

/// The ground of a box that floats over the window (a dialog, a popover, the reply box).
pub(crate) trait FloatingGround: ParentElement + Styled + Sized {
    /// Fills the box with `fill`. On a see-through window (`palette.frosted`) the floating tier lets what is under the
    /// box through, so the fill lies on glass: the text under the box blurs instead of reading through it. On an
    /// opaque window the fill is the box's own background, solid, and nothing blurs.
    ///
    /// Call it before the box's first child: the glass is a child, and it must paint under the others. `radius` is the
    /// box's outer corner. `bordered`: the box has a 1px border; the glass then covers it too, with the outer corners,
    /// so no sliver shows between the glass and the border's inner curve, and the border paints over its edge
    /// (`composer::composer_surface`). The box must not scroll: a scrolled box would move its glass with the content.
    fn floating_ground(self, radius: f32, fill: Hsla, bordered: bool, palette: &Palette) -> Self {
        if !palette.frosted {
            return self.bg(fill);
        }
        let inset = if bordered { -1. } else { 0. };
        self.child(
            div()
                .debug_selector(|| "floating-glass".into())
                .absolute()
                .inset(px(inset))
                .child(glass(radius, fill, None)),
        )
    }
}

impl<E: ParentElement + Styled> FloatingGround for E {}

#[cfg(test)]
mod tests {
    use super::FloatingGround;
    use crate::frost::Transparency;
    use crate::palette::Palette;
    use gpui_kit::prelude::*;
    use gpui_kit::{Context, IntoElement, Render, TestAppContext, Window, div, px};

    struct Card {
        palette: Palette,
        bordered: bool,
    }

    impl Render for Card {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let card = div()
                .debug_selector(|| "card".into())
                .w(px(200.))
                .h(px(80.))
                .rounded(px(12.))
                .when(self.bordered, |card| card.border_1());
            div()
                .p(px(20.))
                .child(card.floating_ground(12., self.palette.menu, self.bordered, &self.palette))
        }
    }

    fn draw(cx: &mut TestAppContext, palette: Palette, bordered: bool) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
        let (_, cx) = cx.add_window_view(|_, _| Card { palette, bordered });
        cx.run_until_parked();
        let card = cx.debug_bounds("card").expect("the card is drawn");
        let glass = cx.debug_bounds("floating-glass");
        if let Some(glass) = glass {
            assert_eq!(glass, card, "the glass covers the whole card, border included");
        }
        glass
    }

    #[gpui_kit::test]
    fn a_floating_box_lies_on_glass_only_on_a_see_through_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        for dark in [false, true] {
            for level in [Transparency::Frost, Transparency::Glass] {
                let palette = Palette::solid_for(dark).frosted(level);
                for bordered in [false, true] {
                    assert!(
                        draw(cx, palette, bordered).is_some(),
                        "{level:?}: no glass under the box"
                    );
                }
            }
            let solid = Palette::solid_for(dark);
            assert!(draw(cx, solid, true).is_none(), "an opaque window blurs nothing");
        }
    }
}
