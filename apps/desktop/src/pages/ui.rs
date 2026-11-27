//! The pieces every page is drawn with, so the pages look like one app and like Settings' own sections: a place's
//! frame, section heads, rows, buttons, switches, chips, and the loading, empty and failed states. A list's rows part by
//! space and a hover fill, and buttons by a filled background. A settings card (`group`) is the one place with
//! hairlines: a ring around it and a short rule between its rows, so a section's settings read as one group.
//! Sizes follow `workspace/settings_view.rs` and `inbox_view.rs`.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::palette::Palette;

/// The content width for reading and simple settings forms, excluding gutters.
pub const READ_WIDTH: f32 = 720.;
/// The settings page column (canon `canon_settings.py` page()), excluding its side padding.
pub const SETTINGS_COLUMN: f32 = 680.;
/// The settings page's side padding on a wide window.
pub const SETTINGS_GUTTER: f32 = 56.;
/// A place's frame width (Overview, Usage), including both page gutters.
pub const PLACE_WIDTH: f32 = 1080.;
/// The horizontal gutter on a wide page.
pub const PAGE_GUTTER: f32 = 48.;
/// The monospace face, bundled with the app: ids, keys, patterns.
pub const MONO: &str = "Geist Mono";
/// How far a list row's fill reaches past the column on each side.
pub const ROW_BLEED: f32 = 8.;
/// The height of every button and text field on a page.
pub const CONTROL_HEIGHT: f32 = 28.;

/// The fill of a picked chip or segment: a step above `selected`, so it reads in dark where `hover` and `selected`
/// are a shade apart.
pub fn thumb(palette: Palette) -> Hsla {
    palette.thumb
}

/// A field's or chip's fill inside a card: darker than the card in dark (an inset well), the board's soft fill (black
/// at 5%) in light.
pub fn inset(palette: Palette) -> Hsla {
    if palette.dark {
        palette.background
    } else {
        palette.selected
    }
}

/// A soft drop under a filled surface in light; nothing in dark, where the fill alone lifts it.
pub fn lift(palette: Palette) -> Vec<BoxShadow> {
    if palette.dark {
        Vec::new()
    } else {
        vec![BoxShadow {
            color: black().opacity(0.06),
            offset: point(px(0.), px(1.)),
            blur_radius: px(2.),
            spread_radius: px(0.),
            inset: false,
        }]
    }
}

/// A 14px icon in a color.
pub fn icon(name: Lucide, color: Hsla) -> Icon {
    Icon::new(name).size(px(14.)).text_color(color)
}

/// A settings explanation that applies to a whole section, rather than one failed field.
pub fn settings_notice(title: impl Into<SharedString>, detail: impl Into<SharedString>, palette: Palette) -> Div {
    v_flex()
        .w_full()
        .min_w(px(0.))
        .gap(px(6.))
        .p(px(12.))
        .rounded(px(12.))
        .bg(palette.hover)
        .child(
            h_flex().gap(px(8.)).child(icon(Lucide::Info, palette.icon)).child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child(title.into()),
            ),
        )
        .child(setting_detail(detail, palette))
}

/// A place's frame: it scrolls, and holds a centred column with the title and the line under it.
pub fn place(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    palette: Palette,
) -> (Stateful<Div>, Div) {
    let outer = div().id(id).size_full().overflow_y_scroll();
    let column = v_flex()
        .w_full()
        .max_w(px(PLACE_WIDTH))
        // Centred in the main area, as the app's own pages.
        .mx_auto()
        .px(px(PAGE_GUTTER))
        .pt(px(28.))
        .pb(px(40.))
        .child(
            v_flex()
                .gap(px(4.))
                .mb(px(18.))
                .child(
                    div()
                        .text_size(px(28.))
                        .line_height(px(34.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.strong)
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(palette.faint)
                        .child(subtitle.into()),
                ),
        );
    (outer, column)
}

/// The line under a settings page's title: what the page is for.
pub fn lead(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .min_w(px(0.))
        .mb(px(14.))
        .text_size(px(14.))
        .text_color(palette.text_3())
        .child(text.into())
}

/// A card row's label (14px) over its help (12.5px, `text_3`); the help is left out when empty.
pub fn group_lines(label: impl Into<SharedString>, help: impl Into<SharedString>, palette: Palette) -> Div {
    setting_lines(label, help, palette).flex_1()
}

/// A heading inside a settings page, the one style every page and Settings' own sections use: 13px semibold
/// `muted`, 24px above and 8px below, so the page title stays the one large heading.
pub fn section_head(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .mt(px(24.))
        .mb(px(8.))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.muted)
        .child(text.into())
}

/// A small faint label, for column heads and field names.
pub fn label(text: impl Into<SharedString>, palette: Palette) -> Div {
    div().text_size(px(11.)).text_color(palette.faint).child(text.into())
}

/// Body text, faint.
pub fn note(text: impl Into<SharedString>, palette: Palette) -> Div {
    div().text_size(px(13.)).text_color(palette.faint).child(text.into())
}

/// A `note` a person can select and copy (Ctrl+C, Cmd+C on macOS, or the window's right-click Copy). A press on it
/// gives the focus to the nearest element that tracks it (the workspace), so a field or a shell that had the focus
/// does not take the copy keys.
pub fn selectable_note(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .text_size(px(13.))
        .text_color(palette.faint)
        .child(crate::workspace::message_text::MessageText::new(id, text.into()))
}

/// An error line.
pub fn error_line(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .text_size(px(13.))
        .text_color(palette.destructive)
        .child(text.into())
}

/// A row of a list, 44px tall at least, 2px from the next (no rule between rows). Rounded, so a clickable row's
/// caller can fill it on hover (`.hover(|style| style.bg(palette.hover))`). Its 8px inset sits outside the column
/// (`ROW_BLEED`), so the text lines up with the page's headings and only the fill reaches past them.
pub fn row(_palette: Palette) -> Div {
    h_flex()
        .min_h(px(44.))
        .mx(px(-ROW_BLEED))
        .gap(px(12.))
        .px(px(ROW_BLEED))
        .py(px(8.))
        .mb(px(2.))
        .rounded(px(8.))
}

/// A list's head row: the column labels, on the rows' 8px inset.
pub fn head_row(palette: Palette) -> Div {
    h_flex()
        .h(px(28.))
        .mx(px(-ROW_BLEED))
        .gap(px(12.))
        .px(px(ROW_BLEED))
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.faint)
}

/// A list row's title and the faint line under it, each cut to one line so rows keep one height. A setting's
/// explanation wraps instead: `setting_lines`.
pub fn two_lines(title: impl Into<SharedString>, detail: impl Into<SharedString>, palette: Palette) -> Div {
    v_flex()
        .min_w(px(0.))
        .gap(px(2.))
        .child(
            div()
                .text_size(px(13.))
                .text_color(palette.strong)
                .truncate()
                .child(title.into()),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(palette.faint)
                .truncate()
                .child(detail.into()),
        )
}

pub const REQUIRES_SIGN_IN: &str = "Requires sign-in";

/// A page's account-independent presentation for Settings opened without an account.
pub(super) fn locked_page(cx: &mut App, render: impl Fn(Palette) -> AnyElement + 'static) -> AnyView {
    struct Locked<F>(F);

    impl<F: Fn(Palette) -> AnyElement + 'static> Render for Locked<F> {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            (self.0)(super::palette(cx))
        }
    }

    cx.new(|_| Locked(render)).into()
}

/// The reason shown beside settings whose account context is absent.
pub fn requires_sign_in(palette: Palette) -> Div {
    setting_detail(REQUIRES_SIGN_IN, palette)
        .flex_none()
        .debug_selector(|| "requires-sign-in-label".into())
}

/// An account setting remains discoverable when its controls cannot be used.
pub fn requires_sign_in_row(label: impl Into<SharedString>, detail: impl Into<SharedString>, palette: Palette) -> Div {
    card_row()
        .opacity(0.65)
        .child(setting_lines(label, detail, palette).flex_1())
        .child(requires_sign_in(palette))
}

/// A setting’s complete explanation. It wraps at every text size; policy and privacy rules must never be clipped.
/// List summaries keep `two_lines`, which cuts.
pub fn setting_detail(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .min_w(px(0.))
        .text_size(px(12.5))
        .line_height(px(17.))
        .text_color(palette.text_3())
        .child(text.into())
}

/// A setting's name over its wrapping explanation (`setting_detail`).
pub fn setting_lines(title: impl Into<SharedString>, detail: impl Into<SharedString>, palette: Palette) -> Div {
    let detail = detail.into();
    v_flex()
        .min_w(px(0.))
        .gap(px(2.))
        .child(div().text_size(px(14.)).text_color(palette.strong).child(title.into()))
        .when(!detail.is_empty(), |column| {
            column.child(setting_detail(detail, palette))
        })
}

/// A card: a raised fill, no ring.
pub fn card(palette: Palette) -> Div {
    v_flex()
        .p(px(14.))
        .gap(px(6.))
        .rounded(px(12.))
        .bg(palette.raised)
        .shadow(lift(palette))
}

/// A figure on a card: a small label and a large number.
pub fn stat(label_text: impl Into<SharedString>, value: impl Into<SharedString>, palette: Palette) -> Div {
    card(palette)
        .flex_1()
        .min_w(px(160.))
        .child(label(label_text, palette))
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.strong)
                .child(value.into()),
        )
}

/// Keyboard focus for a page control. GPUI synthesizes clicks from Enter and Space for focused elements.
/// A ring outside the bounds keeps layout stable, including the 33px switch.
pub fn focusable(control: Stateful<Div>, palette: Palette) -> Stateful<Div> {
    control.tab_index(0).focus_visible(move |style| {
        style.shadow(vec![BoxShadow {
            color: palette.muted,
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(2.),
            inset: false,
        }])
    })
}

/// A pressed button's opacity: the dip that shows the press landed. A disabled button has no pressed state.
const PRESSED: f32 = 0.8;

/// A button's shape, 28px tall, with no fill or hover yet.
fn button_base(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_none()
        .h(px(CONTROL_HEIGHT))
        .px(px(12.))
        .justify_center()
        .rounded(px(crate::tokens::radius::CONTROL))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .child(text.into())
}

/// A secondary button: a soft fill that deepens on hover.
pub fn button(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    // `selected` in dark, so the fill still reads on a raised card; the soft grey in light.
    focusable(button_base(id, text), palette)
        .bg(if palette.dark { palette.selected } else { palette.hover })
        .text_color(palette.foreground)
        .hover(|style| style.bg(thumb(palette)))
        .active(|style| style.opacity(PRESSED))
}

/// A quiet button inside a row: text only until hovered. Set its text color after for a red one.
pub fn ghost_button(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    focusable(button_base(id, text), palette)
        .text_color(palette.muted)
        .hover(|style| style.bg(palette.hover))
        .active(|style| style.opacity(PRESSED))
}

/// The page's main action, filled.
pub fn primary_button(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    focusable(h_flex().id(id), palette)
        .flex_none()
        .h(px(CONTROL_HEIGHT))
        .px(px(12.))
        .justify_center()
        .rounded(px(crate::tokens::radius::CONTROL))
        .bg(palette.primary)
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.primary_foreground)
        .cursor_pointer()
        .hover(|style| style.opacity(0.9))
        .active(|style| style.opacity(PRESSED))
        .child(text.into())
}

/// Permission actions share one size and outline; only the affirmative action gets the primary fill.
pub fn permission_button(
    id: impl Into<ElementId>,
    text: &'static str,
    primary: bool,
    enabled: bool,
    palette: Palette,
) -> Stateful<Div> {
    button_base(id, text)
        .h(px(32.))
        .border_1()
        .border_color(if primary { palette.primary } else { palette.border })
        .bg(if primary { palette.primary } else { palette.background })
        .text_color(if primary {
            palette.primary_foreground
        } else {
            palette.foreground
        })
        .when(primary, |button| button.px(px(14.)))
        .map(|button| {
            if enabled {
                focusable(button, palette)
                    .hover(move |style| {
                        if primary {
                            style.opacity(0.88)
                        } else {
                            style.bg(palette.hover)
                        }
                    })
                    .active(|style| style.opacity(0.72))
            } else {
                button.cursor_default().opacity(0.5)
            }
        })
}

/// Keep the action group together. The scope note wraps above it in a narrow pane.
pub fn permission_footer(note: &'static str, actions: Div, palette: Palette) -> Div {
    h_flex()
        .w_full()
        .flex_wrap()
        .gap(px(12.))
        .pt(px(12.))
        .border_t_1()
        .border_color(palette.border)
        .child(
            div()
                .flex_1()
                .min_w(px(160.))
                .text_size(px(12.))
                .text_color(palette.muted)
                .child(note),
        )
        .child(actions.flex_none().ml_auto())
}

/// A button that removes something: red text. Ask twice before acting (`confirm` names the second press).
pub fn danger_button(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    button(id, text, palette).text_color(palette.destructive)
}

/// A button that is off: faint, no pointer. Give it no click handler.
pub fn disabled_button(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_none()
        .h(px(CONTROL_HEIGHT))
        .px(px(12.))
        .justify_center()
        .rounded(px(crate::tokens::radius::CONTROL))
        .bg(palette.hover)
        .opacity(0.6)
        .text_size(px(13.))
        .text_color(palette.faint)
        .child(text.into())
}

/// A choice among a few, as in the New Code session form: a soft fill, a deeper one when picked.
pub fn chip(id: impl Into<ElementId>, text: impl Into<SharedString>, picked: bool, palette: Palette) -> Stateful<Div> {
    focusable(h_flex().id(id), palette)
        .flex_none()
        .h(px(CONTROL_HEIGHT))
        .px(px(12.))
        .justify_center()
        .rounded(px(crate::tokens::radius::CONTROL))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(picked, |chip| chip.bg(thumb(palette)).text_color(palette.strong))
        .when(!picked, |chip| {
            chip.bg(palette.hover)
                .text_color(palette.muted)
                .hover(|style| style.bg(palette.selected).text_color(palette.strong))
        })
        .child(text.into())
}

/// Tabs in a soft track, as the Inbox's: a segmented control, the picked one on a raised thumb.
pub fn tab(id: impl Into<ElementId>, text: impl Into<SharedString>, picked: bool, palette: Palette) -> Stateful<Div> {
    focusable(h_flex().id(id), palette)
        .h(px(CONTROL_HEIGHT - 4.))
        .px(px(10.))
        .rounded(px(6.))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(picked, |tab| {
            tab.bg(if palette.dark { thumb(palette) } else { palette.raised })
                .shadow(lift(palette))
                .text_color(palette.strong)
        })
        .when(!picked, |tab| {
            tab.text_color(palette.muted)
                .hover(|style| style.text_color(palette.strong))
        })
        .child(text.into())
}

/// The track the tabs sit in.
pub fn tabs(palette: Palette) -> Div {
    h_flex().p(px(2.)).gap(px(2.)).rounded(px(8.)).bg(palette.hover)
}

/// A small rounded tag: a role, a status.
pub fn badge(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(crate::tokens::radius::CONTROL))
        .bg(palette.hover)
        .text_size(px(11.))
        .text_color(palette.muted)
        .child(text.into())
}

/// The app's one on/off switch, in Apple's proportions (a 51 by 31 track around a 27px knob, scaled to 33 by 20).
/// On is the inversion the primary button uses: a `strong` track (near white in dark, near black in light) with a
/// knob in the opposite color, so it reads as on at a glance in either theme. Off is a grey track with a white knob:
/// ink at 18%, so it keeps its contrast on a clear Glass window as well as on a card (a solid light grey all but
/// vanished there). A locked one is faint and does not move.
pub fn switch(id: impl Into<ElementId>, on: bool, locked: bool, palette: Palette) -> Stateful<Div> {
    let (track, knob): (Hsla, Hsla) = match (on, palette.dark) {
        (true, _) => (palette.primary, palette.primary_foreground),
        (false, true) => (white().opacity(0.2), white()),
        (false, false) => (black().opacity(0.18), white()),
    };
    h_flex()
        .id(id)
        .flex_none()
        .w(px(33.))
        .h(px(20.))
        .p(px(2.))
        .rounded(px(10.))
        .bg(track)
        .when(on, |track| track.justify_end())
        .when(locked, |track| track.opacity(0.5))
        .when(!locked, |track| focusable(track, palette).cursor_pointer())
        .child(
            div()
                .flex_none()
                .size(px(16.))
                .rounded(px(8.))
                .bg(knob)
                .shadow(vec![BoxShadow {
                    color: black().opacity(0.25),
                    offset: point(px(0.), px(1.)),
                    blur_radius: px(2.),
                    spread_radius: px(0.),
                    inset: false,
                }]),
        )
}

/// A setting: its name and what it does on the left, its control on the right.
pub fn setting(
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    control: impl IntoElement,
    palette: Palette,
) -> Div {
    h_flex()
        .flex_wrap()
        .justify_between()
        .gap(px(16.))
        .py(px(12.))
        .child(setting_lines(title, detail, palette).flex_1().min_w(px(200.)))
        .child(div().flex_none().child(control))
}

/// A raised card with a hairline ring, for one section's rows: a settings group, as on Notifications and This
/// computer. Fill it with [`group`], or with `card_row`s split by `card_rule`.
pub fn card_frame(palette: Palette) -> Div {
    v_flex()
        .w_full()
        .min_w(px(0.))
        .rounded(px(crate::tokens::GROUP_RADIUS))
        .border_1()
        .border_color(palette.border)
        .bg(palette.raised)
        .overflow_hidden()
}

/// A row inside a `card_frame`: its text on the left, its controls on the right.
pub fn card_row() -> Div {
    h_flex()
        .min_h(px(44.))
        .gap(px(16.))
        .px(px(16.))
        .py(px(12.))
        .items_center()
}

/// The hairline between two rows of a card. It starts at the rows' text, so the card's edge stays clean.
pub fn card_rule(palette: Palette) -> Div {
    div().h(px(1.)).ml(px(16.)).bg(palette.border)
}

/// A card of `rows`, a `card_rule` between each two. A `None` row (one a platform or a policy hides) leaves no line.
pub fn group(rows: impl IntoIterator<Item = Option<AnyElement>>, palette: Palette) -> Div {
    let mut card = card_frame(palette);
    for (ix, row) in rows.into_iter().flatten().enumerate() {
        if ix > 0 {
            card = card.child(card_rule(palette));
        }
        card = card.child(row);
    }
    card
}

/// A setting as a card row: its name over its wrapping explanation, and its control on the right.
pub fn setting_row(
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    control: impl IntoElement,
    palette: Palette,
) -> Div {
    card_row()
        .child(setting_lines(title, detail, palette).flex_1())
        .child(div().flex_none().child(control))
}

/// A card's heading, in `section_head`'s style. Add what sits at its right (a count, a `status`, a `scope_tag`, the
/// section's action) with `.child(..)`.
pub fn group_head(title: impl Into<SharedString>, palette: Palette) -> Div {
    h_flex().mt(px(24.)).mb(px(8.)).gap(px(12.)).child(
        div()
            .flex_1()
            .min_w(px(0.))
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(palette.muted)
            .child(title.into()),
    )
}

/// The explanation under a card: what applies to all its rows, so each row keeps a one-line detail.
pub fn footnote(text: impl Into<SharedString>, palette: Palette) -> Div {
    setting_detail(text, palette).mt(px(8.))
}

/// A state in a few words after a colored dot: "Up to date", "Checking…".
pub fn status(text: impl Into<SharedString>, dot: Hsla, palette: Palette) -> Div {
    h_flex()
        .flex_none()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(palette.muted)
        .child(div().flex_none().size(px(7.)).rounded(px(4.)).bg(dot))
        .child(text.into())
}

/// Where a card's settings are kept, at the right of its `group_head`: "This computer", "Synced to your account". A
/// page that mixes places names one per card, not one for the page.
pub fn scope_tag(icon_name: Lucide, text: impl Into<SharedString>, palette: Palette) -> Div {
    h_flex()
        .flex_none()
        .gap(px(6.))
        .text_size(px(12.))
        .font_weight(FontWeight::NORMAL)
        .text_color(palette.faint)
        .child(icon(icon_name, palette.faint).size(px(12.)))
        .child(text.into())
}

/// A small label on a soft fill, after a row's title or in its detail: a grant's scope ("Read only").
pub fn pill(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .flex_none()
        .px(px(8.))
        // 2 above and below: the glyphs of 11 px text fill a 16 px line, so with none they touch the fill.
        .py(px(2.))
        .rounded(px(10.))
        .bg(if palette.dark { palette.selected } else { palette.hover })
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(palette.foreground)
        .child(text.into())
}

/// A card row's icon on a rounded square, so a list of rows that each name a thing (a client, a page) scans by shape.
pub fn icon_tile(icon_name: Lucide, palette: Palette) -> Div {
    icon_tile_frame(palette).child(icon(icon_name, palette.muted))
}

/// A service's mark on `icon_tile`'s square: a connector's or an MCP server's logo (`assets::connector`), in its brand
/// colour, or in `strong` when the brand is black or white.
pub fn mark_tile(path: impl Into<SharedString>, color: Option<Hsla>, palette: Palette) -> Div {
    icon_tile_frame(palette).child(
        svg()
            .path(path.into())
            .size(px(18.))
            .flex_none()
            .text_color(color.unwrap_or(palette.strong)),
    )
}

/// `icon_tile`'s square with a name's first letter, for a service the app has no mark for.
pub fn letter_tile(name: &str, palette: Palette) -> Div {
    let letter = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    icon_tile_frame(palette)
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.muted)
        .child(letter)
}

fn icon_tile_frame(palette: Palette) -> Div {
    div()
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(crate::tokens::radius::CONTROL))
        .bg(if palette.dark { palette.selected } else { palette.hover })
}

/// A card row that opens another page: its icon, its name over one line of detail, and a chevron, or "Browser" and an
/// arrow when the page opens in the browser. The whole row is the target; hover fills it.
pub fn nav_row(
    id: impl Into<ElementId>,
    icon_name: Lucide,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    in_browser: bool,
    palette: Palette,
) -> Stateful<Div> {
    let end = if in_browser {
        h_flex()
            .flex_none()
            .gap(px(4.))
            .text_size(px(12.))
            .text_color(palette.faint)
            .child("Browser")
            .child(icon(Lucide::ArrowUpRight, palette.faint).size(px(13.)))
    } else {
        h_flex().flex_none().child(icon(Lucide::ChevronRight, palette.faint))
    };
    focusable(card_row().id(id), palette)
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .active(|style| style.opacity(PRESSED))
        .child(icon_tile(icon_name, palette))
        .child(setting_lines(title, detail, palette).flex_1())
        .child(end)
}

/// A keycap's height.
pub const KEYCAP: f32 = 20.;

/// A chord as one cap per key, as the menus and the palette draw it: `Ctrl` `Shift` `N`, `⌘` `K`. Takes the
/// labels as written in Settings and in `prefs.rs` (`Ctrl Shift N`, `Alt+Space`, `⌘K`); a trailing note after a comma
/// (`, not registered`) stays text.
pub fn keycaps(keys: &str, palette: Palette) -> Div {
    let (chord, note) = match keys.split_once(", ") {
        Some((chord, note)) => (chord, Some(note.to_string())),
        None => (keys, None),
    };
    h_flex()
        .flex_none()
        .gap(px(4.))
        .children(key_names(chord).into_iter().map(|key| {
            div()
                .min_w(px(KEYCAP))
                .h(px(KEYCAP))
                .px(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .bg(palette.hover)
                .border_1()
                .border_color(palette.border)
                .text_size(px(12.))
                .text_color(palette.muted)
                .child(key)
        }))
        .when_some(note, |caps, note| {
            caps.child(
                div()
                    .ml(px(4.))
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(note),
            )
        })
}

/// A chord's keys: split on spaces and `+`, and macOS's modifier glyphs each their own key (`⇧⌘N` is three).
pub fn key_names(chord: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for word in chord.split([' ', '+']).filter(|word| !word.is_empty()) {
        let mut rest = word;
        while let Some(first) = rest.chars().next().filter(|c| "⌘⇧⌥⌃".contains(*c)) {
            keys.push(first.to_string());
            rest = &rest[first.len_utf8()..];
        }
        if !rest.is_empty() {
            keys.push(rest.to_string());
        }
    }
    keys
}

/// A one-line text field in a soft box, as the forms of the settings pages draw it.
pub fn input_box(input: &Entity<InputState>, palette: Palette) -> Div {
    h_flex()
        .flex_1()
        .min_w(px(0.))
        .h(px(CONTROL_HEIGHT))
        .px(px(10.))
        .rounded(px(8.))
        .bg(palette.hover)
        .text_size(px(INPUT_TEXT))
        .child(div().flex_1().min_w(px(0.)).child(input_text(input)))
}

/// The settings pages' text size: field text, labels and row titles.
const INPUT_TEXT: f32 = 13.;

/// GPUI Kit's `Input` sets its own size (`text_sm`, 14 px) and ignores the size of the box around it. Set it on the
/// `Input`, so a field's text and placeholder match the 13 px rows and labels beside it.
fn input_text(input: &Entity<InputState>) -> Input {
    Input::new(input).appearance(false).text_size(px(INPUT_TEXT))
}

/// A form field: its label over its control.
pub fn field(label_text: impl Into<SharedString>, control: impl IntoElement, palette: Palette) -> Div {
    v_flex().gap(px(4.)).child(label(label_text, palette)).child(control)
}

/// An empty list or a page with nothing yet: a title and a line, centered.
pub fn empty(title: impl Into<SharedString>, detail: impl Into<SharedString>, palette: Palette) -> Div {
    v_flex()
        .items_center()
        .py(px(56.))
        .gap(px(6.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.strong)
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_size(px(13.))
                .text_color(palette.faint)
                .child(detail.into()),
        )
}

/// A page or list with nothing to show, or nothing this person may see: a 32px icon, a title and a line, centered
/// 64px under the page's head. The caller adds an action under it with `.child(..)`.
pub fn empty_state(
    name: Lucide,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    palette: Palette,
) -> Div {
    v_flex()
        .items_center()
        .pt(px(64.))
        .pb(px(32.))
        .gap(px(6.))
        .child(Icon::new(name).size(px(32.)).text_color(palette.icon).mb(px(8.)))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.strong)
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(420.))
                .mb(px(10.))
                .text_center()
                .text_size(px(13.))
                .text_color(palette.muted)
                .child(detail.into()),
        )
}

/// A failed read: what went wrong, and Try again when `retry` is given a handler by the caller.
pub fn failed(message: impl Into<SharedString>, palette: Palette) -> Div {
    v_flex()
        .items_center()
        .py(px(56.))
        .gap(px(6.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.strong)
                .child("This didn't load"),
        )
        .child(
            div()
                .max_w(px(480.))
                .text_center()
                .text_size(px(13.))
                .text_color(palette.faint)
                .child(message.into()),
        )
}

/// Rows of grey bars while a list loads.
pub fn skeleton(rows: usize, palette: Palette) -> Div {
    v_flex().children((0..rows).map(|ix| {
        h_flex().min_h(px(52.)).gap(px(12.)).mb(px(2.)).child(
            v_flex()
                .gap(px(6.))
                .child(
                    div()
                        .h(px(10.))
                        .w(px(180. + (ix % 3) as f32 * 60.))
                        .rounded(px(4.))
                        .bg(palette.hover),
                )
                .child(div().h(px(8.)).w(px(120.)).rounded(px(4.)).bg(palette.hover)),
        )
    }))
}

/// A link out to Nexus UI, for the parts of a page not drawn here: "Open in the browser ↗".
pub fn web_link(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    focusable(h_flex().id(id), palette)
        .gap(px(6.))
        .text_size(px(13.))
        .text_color(palette.muted)
        .cursor_pointer()
        .hover(|style| style.text_color(palette.strong))
        .child(text.into())
        .child(icon(Lucide::ExternalLink, palette.icon))
}

/// A date from an ISO time: `2026-11-05`.
pub fn day(at: &str) -> &str {
    at.get(..10).unwrap_or(at)
}

/// A dollar amount from Gateway's decimal string or number: `$12.30`, `$0.0042` for small ones.
pub fn dollars(value: &serde_json::Value) -> String {
    let amount = match value {
        serde_json::Value::String(text) => text.parse::<f64>().ok(),
        serde_json::Value::Number(number) => number.as_f64(),
        _ => None,
    };
    match amount {
        None => "–".into(),
        Some(amount) if amount != 0. && amount.abs() < 0.01 => format!("${amount:.4}"),
        Some(amount) => format!("${amount:.2}"),
    }
}

/// A count with thousands split: `12,340`.
pub fn count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A text field of a JSON object, or empty.
pub fn text<'a>(value: &'a serde_json::Value, key: &str) -> &'a str {
    value.get(key).and_then(serde_json::Value::as_str).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::{count, dollars, key_names};
    use serde_json::json;

    #[test]
    fn a_chord_splits_into_its_keys() {
        assert_eq!(key_names("Ctrl Shift N"), ["Ctrl", "Shift", "N"]);
        assert_eq!(key_names("Alt+Space"), ["Alt", "Space"]);
        assert_eq!(key_names("⇧⌘N"), ["⇧", "⌘", "N"]);
        assert_eq!(key_names("Ctrl K"), ["Ctrl", "K"]);
    }

    #[test]
    fn numbers_read_as_people_write_them() {
        assert_eq!(count(1234567), "1,234,567");
        assert_eq!(count(12), "12");
        assert_eq!(dollars(&json!("12.3")), "$12.30");
        assert_eq!(dollars(&json!(0.0042)), "$0.0042");
        assert_eq!(dollars(&json!(null)), "–");
    }
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod interaction_tests;
