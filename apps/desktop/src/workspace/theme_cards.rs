//! Appearance's theme picker (`ui/src/components/nexus-ui/settings/you/preferences.tsx`, `you.css`): System, Light
//! and Dark as small preview cards, each a window in miniature (a sidebar and a page) in that theme's own colors.
//! System shows light and dark side by side. No hairlines: the cards are filled, and the picked one is filled
//! stronger with its disc on, as Nexus UI's v5 cards.
//!
//! The transparency picker below it (`frost.rs`) has the same cards: Opaque, Frost and Glass, each a window in
//! miniature over a desktop, at that level.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{WindowAppearance, *};

use super::super::Workspace;
use crate::frost::{Look, Transparency};
use crate::palette::{Palette, ThemeChoice};

/// The preview's height, as `.you-preview`.
const PREVIEW_HEIGHT: f32 = 96.;

/// The three cards in a row, in Nexus UI's order. Settings › Appearance and onboarding's Look show them.
pub(in crate::workspace) fn render(
    picked: ThemeChoice,
    palette: Palette,
    fields: &crate::settings_fields::SettingsFields,
    cx: &mut Context<Workspace>,
) -> Div {
    h_flex().w_full().max_w(px(720.)).gap(px(12.)).children(
        [
            (ThemeChoice::System, "System"),
            (ThemeChoice::Light, "Light"),
            (ThemeChoice::Dark, "Dark"),
        ]
        .into_iter()
        .enumerate()
        .map(|(ix, (choice, label))| card(ix, choice, label, picked == choice, palette, fields, cx)),
    )
}

fn card(
    ix: usize,
    choice: ThemeChoice,
    label: &'static str,
    picked: bool,
    palette: Palette,
    fields: &crate::settings_fields::SettingsFields,
    cx: &mut Context<Workspace>,
) -> Stateful<Div> {
    let light = Palette::for_choice(ThemeChoice::Light, WindowAppearance::Light).solid();
    let dark = Palette::for_choice(ThemeChoice::Dark, WindowAppearance::Dark).solid();
    let preview = match choice {
        ThemeChoice::System => h_flex()
            .size_full()
            .child(window(light).w_1_2().rounded_r(px(0.)))
            .child(window(dark).w_1_2().rounded_l(px(0.))),
        ThemeChoice::Light => h_flex().size_full().child(window(light).w_full()),
        ThemeChoice::Dark => h_flex().size_full().child(window(dark).w_full()),
    };
    let ids = ["theme-system", "theme-light", "theme-dark"];
    let focus = fields.focus(ids[ix], cx);
    shell(("settings-theme", ix), &focus, preview, label, picked, palette)
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            let next = match event.keystroke.key.as_str() {
                "left" | "up" => (ix + 2) % 3,
                "right" | "down" => (ix + 1) % 3,
                _ => return,
            };
            let choice = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark][next];
            this.set_theme(choice, cx);
            this.settings.shell.fields.focus(ids[next], cx).focus(window, cx);
            cx.stop_propagation();
        }))
        .on_click(cx.listener(move |this, _, _, cx| this.set_theme(choice, cx)))
}

/// Opaque, Frost and Glass in a row, each in the theme on screen. None is picked when the choice is Custom.
pub(super) fn render_transparency(
    picked: Option<Transparency>,
    palette: Palette,
    fields: &crate::settings_fields::SettingsFields,
    cx: &mut Context<Workspace>,
) -> Div {
    h_flex().w_full().max_w(px(720.)).gap(px(12.)).children(
        Transparency::ALL
            .into_iter()
            .enumerate()
            .map(|(ix, level)| transparency_card(ix, level, picked == Some(level), palette, fields, cx)),
    )
}

fn transparency_card(
    ix: usize,
    level: Transparency,
    picked: bool,
    palette: Palette,
    fields: &crate::settings_fields::SettingsFields,
    cx: &mut Context<Workspace>,
) -> Stateful<Div> {
    let ids = ["transparency-opaque", "transparency-frost", "transparency-glass"];
    let focus = fields.focus(ids[ix], cx);
    let preview = over_desktop(Look::at(level, &palette), palette);
    shell(
        ("settings-transparency", ix),
        &focus,
        preview,
        level.label(),
        picked,
        palette,
    )
    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
        let next = match event.keystroke.key.as_str() {
            "left" | "up" => (ix + 2) % 3,
            "right" | "down" => (ix + 1) % 3,
            _ => return,
        };
        this.set_transparency(Transparency::ALL[next], cx);
        this.settings.shell.fields.focus(ids[next], cx).focus(window, cx);
        cx.stop_propagation();
    }))
    .on_click(cx.listener(move |this, _, _, cx| this.set_transparency(level, cx)))
}

/// A card: its preview in a ring slot, then its disc and label. Keyboard focus shows as a ring.
fn shell(
    id: (&'static str, usize),
    focus: &FocusHandle,
    preview: Div,
    label: &'static str,
    picked: bool,
    palette: Palette,
) -> Stateful<Div> {
    v_flex()
        .id(id)
        .track_focus(focus)
        .tab_index(0)
        .flex_1()
        .min_w(px(0.))
        .p(px(6.))
        .gap(px(4.))
        .rounded(px(12.))
        // A ring for the keyboard only: transparent until the card has visible focus, so nothing shifts.
        .border_2()
        .border_color(transparent_black())
        .focus_visible(|style| style.border_color(palette.muted))
        .bg(if picked { palette.selected } else { palette.hover })
        .cursor_pointer()
        .when(!picked, |card| card.hover(|style| style.bg(palette.selected)))
        .active(|style| style.opacity(0.85))
        // The picked preview wears a 2px strong ring at a 2px offset; the slot is always there, so nothing shifts.
        .child(
            div()
                .w_full()
                .p(px(2.))
                .rounded(px(10.))
                .border_2()
                .border_color(if picked { palette.strong } else { transparent_black() })
                .child(
                    div()
                        .h(px(PREVIEW_HEIGHT))
                        .w_full()
                        // The window inside rounds to 6; this clip box matches it, so the corners agree.
                        .rounded(px(6.))
                        .overflow_hidden()
                        .child(preview),
                ),
        )
        .child(
            h_flex()
                .h(px(32.))
                .px(px(6.))
                .gap(px(8.))
                .child(radio(picked, palette))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(if picked { palette.strong } else { palette.foreground })
                        .when(picked, |text| text.font_weight(FontWeight::MEDIUM))
                        .child(label),
                ),
        )
}

/// A window in miniature at `look`'s level, over a desktop in the theme's tone: the wash and the pane's veil let the
/// desktop through around the sidebar card, and the card lets more through.
fn over_desktop(look: Look, theme: Palette) -> Div {
    let (from, to, blob) = if theme.dark {
        (0x2B3A4E, 0x101820, 0x6B4E3A)
    } else {
        (0xBCC5D6, 0x8E9AA6, 0xE9C9AB)
    };
    let line = |width: Length, color: Hsla| div().h(px(4.)).w(width).flex_none().rounded(px(2.)).bg(color);
    div()
        .relative()
        .size_full()
        .bg(linear_gradient(
            155.,
            linear_color_stop(rgb(from), 0.),
            linear_color_stop(rgb(to), 1.),
        ))
        .child(
            div()
                .absolute()
                .top(px(-18.))
                .right(px(-10.))
                .size(px(84.))
                .rounded_full()
                .bg(rgb(blob)),
        )
        .child(
            h_flex()
                .absolute()
                .inset_0()
                .items_start()
                .p(px(5.))
                .gap(px(8.))
                // As the window draws them in two layers (`frost.rs`), each as one fill.
                .bg(Hsla {
                    a: look.pane_cover(),
                    ..look.veil
                })
                .child(
                    v_flex()
                        .w(px(34.))
                        .h_full()
                        .flex_none()
                        .p(px(5.))
                        .pt(px(8.))
                        .gap(px(5.))
                        .rounded(px(5.))
                        .bg(Hsla {
                            a: look.sidebar_cover(),
                            ..look.panel
                        })
                        .shadow(look.panel_shadow())
                        .child(line(px(18.).into(), theme.faint))
                        .child(line(relative(1.).into(), theme.selected))
                        .child(line(relative(1.).into(), theme.selected))
                        .child(line(px(16.).into(), theme.selected)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .pt(px(10.))
                        .gap(px(6.))
                        .child(div().h(px(5.)).w(relative(0.5)).rounded(px(2.)).bg(theme.muted))
                        .child(line(relative(0.8).into(), theme.raised))
                        .child(line(relative(0.65).into(), theme.raised)),
                ),
        )
}

/// A window in miniature, in `theme`'s colors: the sidebar's lines, and a page with a heading and two lines.
fn window(theme: Palette) -> Div {
    let line = |width: Length, color: Hsla| div().h(px(4.)).w(width).flex_none().rounded(px(2.)).bg(color);
    h_flex()
        .h_full()
        .items_start()
        .gap(px(6.))
        .pt(px(8.))
        .pl(px(8.))
        // Concentric with the card's ring: 10, less its 2 border and 2 padding.
        .rounded(px(6.))
        .overflow_hidden()
        .bg(theme.sidebar)
        .child(
            v_flex()
                .w(px(28.))
                .flex_none()
                .pt(px(4.))
                .gap(px(5.))
                .child(line(px(22.).into(), theme.faint))
                .child(line(relative(1.).into(), theme.selected))
                .child(line(relative(1.).into(), theme.selected))
                .child(line(px(18.).into(), theme.selected)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .p(px(10.))
                .gap(px(6.))
                .rounded_tl(px(6.))
                .bg(theme.background)
                .child(div().h(px(5.)).w(relative(0.5)).rounded(px(2.)).bg(theme.muted))
                .child(line(relative(0.8).into(), theme.selected))
                .child(line(relative(0.65).into(), theme.selected)),
        )
}

/// The card's disc: a faint 1.5px ring when off, a filled disc around the card's fill when on.
fn radio(on: bool, palette: Palette) -> Div {
    div()
        .size(px(14.))
        .flex_none()
        .rounded_full()
        .when(on, |disc| {
            disc.bg(palette.strong)
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(px(6.)).rounded_full().bg(palette.selected))
        })
        .when(!on, |disc| disc.border(px(1.5)).border_color(palette.faint))
}
