//! A list row's actions, the macOS way (Finder, Mail): the row itself carries the main action on click, and its other
//! actions show only while the pointer is on the row, as small icon buttons in one cluster at the right edge. The
//! cluster floats over the row's end on the row's own hover fill, so it reserves no width: titles use the full row
//! until the pointer arrives.
//!
//! Use it on a `ui::row` that has `.relative()`, `.group(group)` and a hover fill of `palette.hover`.

use crate::workspace::FloatingGround;
use crate::workspace::menu_item_hover;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::ui;
use crate::palette::Palette;

/// The hover-revealed cluster at the row's right edge. `pinned` keeps it shown (e.g. right after "Copied").
pub fn actions(group: impl Into<SharedString>, pinned: bool, palette: Palette) -> Stateful<Div> {
    let group = group.into();
    // The fill matches the row's hover fill so the cluster reads as part of the row, and a short fade on its left
    // edge keeps a title running under it from being cut hard.
    let fill = row_fill(palette);
    h_flex()
        .id(SharedString::from(format!("row-actions-{group}")))
        .tab_index(0)
        .tab_stop(false)
        .absolute()
        .top_0()
        .bottom_0()
        .right_0()
        .pl(px(24.))
        .pr(px(ui::ROW_BLEED))
        .gap(px(2.))
        .items_center()
        .rounded_r(px(8.))
        .bg(linear_gradient(
            90.,
            linear_color_stop(fill.opacity(0.), 0.),
            linear_color_stop(fill, 0.2),
        ))
        .when(!pinned, |cluster| {
            cluster
                .opacity(0.)
                .group_hover(group, |style| style.opacity(1.))
                .in_focus(|style| style.opacity(1.))
        })
}

/// The row's hover fill, (nearly) opaque, so the cluster can cover the text under it.
fn row_fill(palette: Palette) -> Hsla {
    // A see-through window's page is clear: cover the text with the ground's cover tint instead (`frost::cover`).
    if palette.frosted {
        return crate::frost::cover(&palette).blend(palette.hover);
    }
    // `hover` is a translucent tint in both themes; laid over the page background it is this.
    let mut fill = palette.background.blend(palette.hover);
    fill.a = 1.;
    fill
}

/// A small icon button for the cluster, with its name as the tooltip.
pub fn icon_button(id: impl Into<ElementId>, icon: Lucide, tip: &'static str, palette: Palette) -> Stateful<Div> {
    ui::focusable(h_flex().id(id), palette)
        .flex_none()
        .size(px(ui::CONTROL_HEIGHT))
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|style| style.bg(ui::thumb(palette)))
        .child(ui::icon(icon, palette.icon))
        .aria_label(tip)
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
}

/// A quiet state in the cluster ("Copied"), an icon and a word.
pub fn status(icon: Lucide, text: &'static str, palette: Palette) -> Div {
    h_flex()
        .flex_none()
        .px(px(6.))
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(palette.muted)
        .child(ui::icon(icon, palette.muted))
        .child(text)
}

/// A filter or choice that opens a menu: "Team  Any ⌄", a soft fill like the chips.
pub fn select_button(
    id: impl Into<ElementId>,
    label: &'static str,
    value: impl Into<SharedString>,
    open: bool,
    palette: Palette,
) -> Stateful<Div> {
    ui::focusable(h_flex().id(id), palette)
        .flex_none()
        .h(px(ui::CONTROL_HEIGHT))
        .pl(px(10.))
        .pr(px(8.))
        .gap(px(6.))
        .rounded(px(8.))
        .bg(if open { palette.selected } else { palette.hover })
        .text_size(px(13.))
        .cursor_pointer()
        .hover(|style| style.bg(palette.selected))
        .when(!label.is_empty(), |button| {
            button.child(div().text_color(palette.muted).child(label))
        })
        .child(div().text_color(palette.strong).child(value.into()))
        .child(ui::icon(Lucide::ChevronDown, palette.icon))
}

/// The menu a [`select_button`] opens, under it. Put it in a `.relative()` wrapper around the button.
pub fn popover(body: impl IntoElement, palette: Palette) -> Div {
    div().absolute().left_0().top(px(ui::CONTROL_HEIGHT)).child(
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .offset(point(px(0.), px(4.)))
                .snap_to_window_with_margin(px(8.))
                .child(
                    v_flex()
                        .occlude()
                        .min_w(px(220.))
                        .p(px(4.))
                        .rounded(px(12.))
                        .floating_ground(12.0, palette.menu, false, &palette)
                        .shadow(ui::lift(palette))
                        .text_size(px(13.))
                        .child(body),
                ),
        )
        .with_priority(1),
    )
}

/// One choice in a [`popover`]: a check when picked, indented by `depth` for a tree.
pub fn menu_item(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    picked: bool,
    depth: usize,
    palette: Palette,
) -> Stateful<Div> {
    ui::focusable(h_flex().id(id), palette)
        .h(px(ui::CONTROL_HEIGHT))
        .pl(px(8. + 14. * depth as f32))
        .pr(px(8.))
        .gap(px(8.))
        .rounded(px(6.))
        .cursor_pointer()
        .text_color(if picked { palette.strong } else { palette.foreground })
        .hover(|style| menu_item_hover(style, palette))
        .child(div().flex_1().min_w_0().truncate().child(text.into()))
        .when(picked, |item| item.child(ui::icon(Lucide::Check, palette.icon)))
}
