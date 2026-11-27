//! The pieces the redesigned Usage page is drawn with: its frame and head, the underlined tabs, a bar
//! that fills its column, a figure row, a person's initials, a status dot, and a chart day's tooltip.

use super::*;
use crate::workspace::FloatingGround;

/// The green of a call that went through (brand v5 `--nx-live`).
pub(super) fn live() -> Hsla {
    rgb(0x30D158).into()
}

/// The page's frame: it scrolls, and holds the centred column. The head goes in the column, so the range and Export
/// sit beside the title.
pub(super) fn frame(palette: Palette) -> (Stateful<Div>, Div) {
    let outer = div()
        .id("usage-page")
        .size_full()
        .overflow_y_scroll()
        .bg(palette.background);
    let column = v_flex()
        .w_full()
        .max_w(px(ui::PLACE_WIDTH))
        .mx_auto()
        .px(px(ui::PAGE_GUTTER))
        .pt(px(28.))
        .pb(px(40.));
    (outer, column)
}

/// The title over its line: "Usage", "Acme · Nov 1 – Nov 18, UTC".
pub(super) fn title(subtitle: impl Into<SharedString>, palette: Palette) -> Div {
    v_flex()
        .gap(px(4.))
        .child(
            div()
                .text_size(px(28.))
                .line_height(px(34.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.strong)
                .child("Usage"),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(palette.faint)
                .child(subtitle.into()),
        )
}

/// The row the underlined tabs sit on: a hairline under them all.
pub(super) fn tab_row(palette: Palette) -> Div {
    h_flex()
        .gap(px(24.))
        .mt(px(20.))
        .mb(px(20.))
        .border_b_1()
        .border_color(palette.border)
}

/// One underlined tab: the picked one in `strong` over a 2px line that covers the row's hairline.
pub(super) fn tab(id: impl Into<ElementId>, text: &'static str, picked: bool, palette: Palette) -> Stateful<Div> {
    ui::focusable(div().id(id), palette)
        .pt(px(4.))
        .pb(px(10.))
        .mb(px(-1.))
        .text_size(px(14.))
        .cursor_pointer()
        .border_b_2()
        .border_color(if picked { palette.strong } else { transparent_black() })
        .text_color(if picked { palette.strong } else { palette.muted })
        .when(!picked, |tab| tab.hover(|style| style.text_color(palette.strong)))
        .child(text)
}

/// A bar that fills its column: `filled` of it in `color`, then `held` of it lighter, and a tick at `tick` when given.
/// Each share is 0 to 1.
pub(super) fn bar(
    height: f32,
    filled: f64,
    held: f64,
    tick: Option<(f64, Hsla)>,
    color: Hsla,
    palette: Palette,
) -> Div {
    let filled = filled.clamp(0., 1.) as f32;
    let held = (held.clamp(0., 1.) as f32).min(1. - filled);
    div()
        .relative()
        .w_full()
        .h(px(height))
        .rounded(px(height / 2.))
        .bg(palette.hover)
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .h_full()
                .w(relative(filled))
                .rounded(px(height / 2.))
                .bg(color),
        )
        .when(held > 0., |track| {
            track.child(
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .left(relative(filled))
                    .w(relative(held))
                    .bg(color.opacity(0.35)),
            )
        })
        .when_some(tick, |track, (at, tick_color)| {
            track.child(
                div()
                    .absolute()
                    .top(px(-4.))
                    .left(relative(at.clamp(0., 1.) as f32))
                    .ml(px(-1.))
                    .w(px(2.))
                    .h(px(height + 8.))
                    .rounded(px(1.))
                    .bg(tick_color),
            )
        })
}

/// A figure beside the budget: its name, its value, and a short note after it.
pub(super) fn figure(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    note: impl Into<SharedString>,
    first: bool,
    palette: Palette,
) -> Div {
    h_flex()
        .items_baseline()
        .gap(px(8.))
        .px(px(20.))
        .py(px(12.))
        .when(!first, |row| row.border_t_1().border_color(palette.border))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_size(px(13.))
                .text_color(palette.muted)
                .child(label.into()),
        )
        .child(div().text_size(px(18.)).text_color(palette.strong).child(value.into()))
        .child(
            div()
                .w(px(64.))
                .flex_none()
                .text_right()
                .truncate()
                .text_size(px(11.))
                .text_color(palette.faint)
                .child(note.into()),
        )
}

/// A section's head on the page: its title, a faint line after it, and whatever goes at its end.
pub(super) fn section(title: impl Into<SharedString>, note: impl Into<SharedString>, palette: Palette) -> Div {
    h_flex()
        .gap(px(12.))
        .mb(px(8.))
        .child(
            div()
                .flex_none()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.strong)
                .child(title.into()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(note.into()),
        )
}

/// A link-like action at a section's end: "Open Breakdown →".
pub(super) fn action(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    ui::focusable(div().id(id), palette)
        .flex_none()
        .text_size(px(13.))
        .text_color(palette.muted)
        .cursor_pointer()
        .hover(|style| style.text_color(palette.strong))
        .child(text.into())
}

/// A person's initials on a small round tile: "Ana Ruiz" is "AR", "ana@x.io" is "A".
pub(super) fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == '@')
        .filter(|word| !word.is_empty())
        .collect();
    let letters: String = match words.as_slice() {
        [] => String::new(),
        [one] => one.chars().take(1).collect(),
        [first, second, ..] if !name.contains('@') => first.chars().take(1).chain(second.chars().take(1)).collect(),
        [first, ..] => first.chars().take(1).collect(),
    };
    letters.to_uppercase()
}

pub(super) fn avatar(name: &str, palette: Palette) -> Div {
    div()
        .flex_none()
        .size(px(24.))
        .rounded(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .bg(if palette.dark { palette.selected } else { palette.hover })
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.muted)
        .child(initials(name))
}

/// A small coloured dot before a state.
pub(super) fn dot(color: Hsla) -> Div {
    div().flex_none().size(px(7.)).rounded(px(4.)).bg(color)
}

/// A call's state colour: green when it went through, amber when a limit stopped it, red when it failed.
pub(super) fn call_color(status: &str, palette: Palette) -> Hsla {
    match status {
        "settled" => live(),
        "refused" => palette.warning,
        "failed" => palette.destructive,
        _ => palette.faint,
    }
}

/// A chart day's tooltip: the day and its total, each series' spend that day, and its calls.
pub(super) struct DayTip {
    pub(super) title: String,
    pub(super) rows: Vec<(Hsla, String, String)>,
    pub(super) foot: String,
    pub(super) palette: Palette,
}

impl Render for DayTip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        v_flex()
            .w(px(220.))
            .p(px(12.))
            .gap(px(4.))
            .rounded(px(10.))
            .border_1()
            .border_color(palette.border)
            .floating_ground(10.0, palette.menu, true, &palette)
            .shadow_lg()
            .text_size(px(12.))
            .child(
                div()
                    .mb(px(4.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child(self.title.clone()),
            )
            .children(self.rows.iter().map(|(color, label, value)| {
                h_flex()
                    .gap(px(8.))
                    .child(div().flex_none().size(px(8.)).rounded(px(2.)).bg(*color))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_color(palette.muted)
                            .child(label.clone()),
                    )
                    .child(div().text_color(palette.foreground).child(value.clone()))
            }))
            .child(
                div()
                    .mt(px(4.))
                    .pt(px(6.))
                    .border_t_1()
                    .border_color(palette.border)
                    .text_color(palette.faint)
                    .child(self.foot.clone()),
            )
    }
}
