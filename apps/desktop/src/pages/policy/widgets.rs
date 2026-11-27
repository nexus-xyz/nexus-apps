use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{h_flex, v_flex};

use super::super::ui;
use crate::palette::Palette;

use super::*;

/* ---------------------------------------------------------------- drawing */

/// An inline list to pick from, scrolled past ten rows.
pub(super) fn option_list(id: impl Into<ElementId>, palette: Palette) -> Stateful<Div> {
    v_flex()
        .id(id)
        .max_h(px(240.))
        .overflow_y_scroll()
        .mt(px(6.))
        .p(px(4.))
        .rounded(px(8.))
        .bg(palette.raised)
        .shadow(palette.pop_shadow(gpui_kit::transparent_black()))
}

pub(super) fn option_row(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    picked: bool,
    palette: Palette,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .gap(px(8.))
        .px(px(8.))
        .py(px(5.))
        .rounded(px(6.))
        .text_size(px(13.))
        .cursor_pointer()
        .when(picked, |row| row.bg(palette.selected))
        .hover(|style| style.bg(palette.hover))
        .child(div().text_color(palette.strong).child(title.into()))
        .child(
            div()
                .min_w(px(0.))
                .truncate()
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(detail.into()),
        )
}

/// A one-line text field in the app's soft style.
pub(super) fn field(input: &Entity<InputState>, palette: Palette) -> Div {
    field_in(input, palette, false)
}

/// The same for patterns and ids: monospace, 12px.
pub(super) fn code_field(input: &Entity<InputState>, palette: Palette) -> Div {
    field_in(input, palette, true)
}

pub(super) fn field_in(input: &Entity<InputState>, palette: Palette, mono: bool) -> Div {
    h_flex()
        .flex_1()
        .h(px(ui::CONTROL_HEIGHT))
        .px(px(10.))
        .rounded(px(8.))
        .bg(ui::inset(palette))
        .text_size(px(if mono { 12. } else { 13. }))
        .when(mono, |field| field.font_family(ui::MONO))
        .child(div().flex_1().min_w(px(0.)).child({
            let input = Input::new(input).appearance(false);
            if mono {
                input.font_family(ui::MONO).text_size(px(12.))
            } else {
                input
            }
        }))
}
