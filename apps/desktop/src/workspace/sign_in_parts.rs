//! The sign-in's parts (`sign_in.rs`): its type, its buttons, the code's tiles and countdown, the approval link and the
//! light well. Brand v5's product UI: DM Sans Regular for the heading, one white pill as the primary, a `selected`
//! pill as the secondary, and quiet text as the ghost.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use image::RgbaImage;
use smallvec::SmallVec;

use crate::motion;
use crate::palette::Palette;

/// The heading: 28 px, Regular, the strongest ink.
pub(super) fn heading(text: &'static str, palette: Palette) -> AnyElement {
    div()
        .text_size(px(28.))
        .line_height(px(32.))
        .text_color(palette.strong)
        .text_center()
        .child(text)
        .into_any_element()
}

/// The line under the heading.
pub(super) fn lede(text: &'static str, palette: Palette) -> AnyElement {
    div()
        .mt(px(10.))
        .max_w(px(330.))
        .text_size(px(14.))
        .line_height(px(21.))
        .text_color(palette.muted)
        .text_center()
        .child(text)
        .into_any_element()
}

/// An error in the lede's place: the red ✕, the only colour on the screen, and the words in the text's ink. The words
/// wrap in the column: a flex child is as wide as its text unless it may shrink (`min_w(0)`), and a long sentence then
/// ran past the window's edge.
pub(super) fn error_line(error: SharedString, palette: Palette) -> AnyElement {
    h_flex()
        .mt(px(12.))
        .w_full()
        .max_w(px(340.))
        .gap(px(8.))
        .items_start()
        .text_size(px(14.))
        .line_height(px(20.))
        .text_color(palette.foreground)
        .child(
            div()
                .h(px(20.))
                .flex()
                .items_center()
                .child(Icon::new(Lucide::X).size(px(14.)).text_color(palette.destructive)),
        )
        .child(
            div()
                .debug_selector(|| "sign-in-error-text".into())
                .flex_1()
                .min_w(px(0.))
                .child(error),
        )
        .into_any_element()
}

/// A labelled field: the Gateway or the API key.
pub(super) fn field(label: &'static str, state: &Entity<InputState>, palette: Palette) -> AnyElement {
    v_flex()
        .w_full()
        .mt(px(24.))
        .gap(px(8.))
        .child(div().text_size(px(12.)).text_color(palette.faint).child(label))
        .child(
            div()
                .h(px(44.))
                .px(px(6.))
                .flex()
                .items_center()
                .rounded(px(12.))
                .bg(palette.raised)
                .border_1()
                .border_color(palette.border)
                .text_size(px(14.))
                .child(Input::new(state).appearance(false)),
        )
        .into_any_element()
}

/// A sign-in button's kind.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Button {
    /// The white pill, one per screen.
    Primary,
    /// A `selected` pill.
    Secondary,
    /// Quiet text.
    Ghost,
}

/// A sign-in button. A pill fills the column; a ghost fits its words.
pub(super) fn button(
    id: &'static str,
    kind: Button,
    label: &'static str,
    icon: Option<Lucide>,
    palette: Palette,
) -> Stateful<Div> {
    let base = h_flex()
        .id(id)
        .justify_center()
        .gap(px(10.))
        .cursor_pointer()
        .children(icon.map(|icon| Icon::new(icon).size(px(16.))))
        .child(label);
    match kind {
        Button::Primary => base
            .w_full()
            .h(px(44.))
            .rounded_full()
            .bg(palette.primary)
            .text_color(palette.primary_foreground)
            .text_size(px(15.))
            .font_weight(FontWeight::MEDIUM)
            .hover(|style| style.opacity(0.9)),
        Button::Secondary => base
            .w_full()
            .h(px(40.))
            .rounded_full()
            .bg(palette.selected)
            .text_color(palette.foreground)
            .text_size(px(14.5))
            .font_weight(FontWeight::MEDIUM)
            .hover(|style| style.bg(palette.hover)),
        Button::Ghost => base
            .h(px(32.))
            .px(px(12.))
            .rounded(px(8.))
            .text_color(palette.muted)
            .text_size(px(13.5))
            .hover(|style| style.bg(palette.hover).text_color(palette.foreground)),
    }
}

/// The one-time code as tiles, arriving 40 ms apart. An expired code dims and stays put.
pub(super) fn code_tiles(code: &str, expired: bool, palette: Palette) -> AnyElement {
    let tiles = code.chars().enumerate().map(|(index, ch)| {
        let dash = ch == '-';
        let tile = div()
            .w(px(if dash { 14. } else { 38. }))
            .h(px(50.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(10.))
            .when(!dash, |tile| tile.bg(palette.raised))
            .font_family(crate::pages::ui::MONO)
            .text_size(px(26.))
            .text_color(if dash { palette.faint } else { palette.strong })
            .child(if dash { "–".to_string() } else { ch.to_string() });
        if expired {
            tile.into_any_element()
        } else {
            motion::rise(
                tile,
                ElementId::Name(format!("sign-in-tile-{code}-{index}").into()),
                Duration::from_millis(120 + 40 * index as u64),
            )
            .into_any_element()
        }
    });
    h_flex()
        .mt(px(28.))
        .gap(px(6.))
        .justify_center()
        .when(expired, |row| row.opacity(0.4))
        .children(tiles)
        .into_any_element()
}

/// The code's time left as a 2 px hairline: `share` of it is left.
pub(super) fn countdown_bar(share: f32, palette: Palette) -> AnyElement {
    div()
        .flex_1()
        .h(px(2.))
        .rounded(px(1.))
        .bg(palette.selected)
        .child(div().h_full().w(relative(share)).rounded(px(1.)).bg(palette.faint))
        .into_any_element()
}

/// The sign-in's approval link, for when the browser did not open: a hint, then the link on one line with Copy. A
/// long link ends in an ellipsis; hover shows all of it.
pub(super) fn approval_link(link: SharedString, palette: Palette) -> AnyElement {
    v_flex()
        .w_full()
        .mt(px(24.))
        .gap(px(6.))
        .child(
            div()
                .text_xs()
                .text_color(palette.faint)
                .child("Browser didn't open? Copy this link into it."),
        )
        .child(
            h_flex()
                .gap(px(4.))
                .py(px(4.))
                .pl(px(12.))
                .pr(px(4.))
                .rounded(px(10.))
                .bg(palette.raised)
                .border_1()
                .border_color(palette.border)
                .child(
                    div()
                        .id("sign-in-link")
                        .flex_1()
                        .min_w(px(0.))
                        .font_family(crate::pages::ui::MONO)
                        .text_xs()
                        .text_color(palette.muted)
                        .truncate()
                        .tooltip({
                            let link = link.clone();
                            move |window, cx| Tooltip::new(link.clone()).build(window, cx)
                        })
                        .child(link.clone()),
                )
                .child(
                    Clipboard::new("sign-in-copy-link")
                        .value(link)
                        .tooltip("Copy link")
                        .accessibility_label("Copy link"),
                ),
        )
        .into_any_element()
}

/// The light well behind the mark on dark: a soft white oval, at most 4.5 % at its middle. GPUI has no radial
/// gradient, so it is a small image, made once and drawn large.
pub(super) fn light_well() -> AnyElement {
    static WELL: OnceLock<Arc<RenderImage>> = OnceLock::new();
    let image = WELL
        .get_or_init(|| {
            const SIDE: u32 = 96;
            let mut buffer = RgbaImage::new(SIDE, SIDE);
            for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                let at = |v: u32| (v as f32 + 0.5) / SIDE as f32 * 2. - 1.;
                let r = (at(x).powi(2) + at(y).powi(2)).sqrt().min(1.);
                let fade = 1. - r * r * (3. - 2. * r);
                *pixel = image::Rgba([255, 255, 255, (0.045 * fade * 255.).round() as u8]);
            }
            Arc::new(RenderImage::new(SmallVec::from_elem(image::Frame::new(buffer), 1)))
        })
        .clone();
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(img(ImageSource::Render(image)).w(px(1100.)).h(px(860.)))
        .into_any_element()
}

#[cfg(test)]
#[path = "sign_in_parts_tests.rs"]
mod tests;
