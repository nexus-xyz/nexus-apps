//! A session's pull requests, drawn the same way wherever more than one shows: each stack is one column with a rail
//! through its pull requests (top first) that ends at the branch they merge into, and each pull request that stands
//! alone has a row of its own. The top bar's card and the Workbench's section let the person pick one; the sidebar's
//! card only lists them. The chip's strip ([`pull_strip`]) is the same picture in one line of dots, bottom first.
//!
//! The grouping is `nexus_pull_requests::stack`'s. Nothing here moves: a person scans these lists many times a day.

use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_pull_requests::badge::Badge;
use nexus_pull_requests::chip::{self, Tone};
use nexus_pull_requests::pull_request::PullRequestStatus;
use nexus_pull_requests::stack::{self, Group};

use crate::palette::Palette;
use crate::workspace::code_dock::menu_item_hover;
use crate::workspace::glyph;
use crate::workspace::session_menu::pull_color;

/// What a row does when the person clicks it: show that pull request.
pub(super) type Pick = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// How many dots the chip's strip draws before it says "+N" instead.
const STRIP_MAX: usize = 6;

/// The list's size: rows in a card, or the smaller rows of the sidebar's card.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Density {
    Card,
    Compact,
}

impl Density {
    fn row(self) -> f32 {
        match self {
            Density::Card => 30.,
            Density::Compact => 24.,
        }
    }

    fn text(self) -> f32 {
        match self {
            Density::Card => 12.,
            Density::Compact => 11.5,
        }
    }
}

/// The chip's word's hue: a failed check or a conflict red, asked changes amber, the rest quiet.
pub(super) fn word_color(tone: Tone, palette: Palette) -> Hsla {
    match tone {
        Tone::Failed => palette.destructive,
        Tone::Changes => pull_color(Badge::Changes, palette),
        Tone::Quiet | Tone::Done => palette.muted,
    }
}

/// The rail's column: a dot in the state's hue, with the line to the row above and below when it joins them.
fn rail(color: Hsla, above: bool, below: bool, height: f32, palette: Palette) -> Div {
    let half = height / 2.;
    let line = |top: f32, h: f32| {
        div()
            .absolute()
            .left(px(7.5))
            .top(px(top))
            .w(px(1.))
            .h(px(h))
            .bg(palette.border)
    };
    div()
        .relative()
        .flex_none()
        .w(px(16.))
        .h(px(height))
        .flex()
        .items_center()
        .justify_center()
        .when(above, |rail| rail.child(line(0., half)))
        .when(below, |rail| rail.child(line(half, half)))
        .child(div().size(px(8.)).rounded_full().bg(color))
}

/// The session's pull requests: each stack under a quiet "Stack of N", top first and ending at its trunk; the ones
/// that stand alone each on a row. `chosen` sits on a fill. With `pick`, a click shows that one.
pub(super) fn pull_list(
    pulls: &[PullRequestStatus],
    chosen: Option<&str>,
    density: Density,
    palette: Palette,
    pick: Option<Pick>,
) -> Div {
    let height = density.row();
    let mut list = v_flex().gap(px(6.));
    let mut ix = 0usize;
    for group in stack::groups(pulls) {
        let mut column = v_flex();
        if group.stacked() {
            column = column.child(group_head(&group, density, palette));
        }
        let last = group.rows.len() - 1;
        let trunk = group.stacked() && !group.trunk.is_empty();
        for (at, row) in group.rows.iter().enumerate() {
            let pull = row.pull;
            let badge = Badge::of(pull);
            let (word, tone) = chip::word(pull);
            let above = group.stacked() && at > 0;
            let below = group.stacked() && (at < last || trunk);
            let picked = chosen == Some(pull.url.as_str());
            let mut line = h_flex()
                .id(("pull-list-row", ix))
                .h(px(height))
                .px(px(6.))
                .gap(px(8.))
                .rounded(px(6.))
                .text_size(px(density.text()))
                .when(picked, |line| line.bg(palette.menu_hover))
                .child(rail(pull_color(badge, palette), above, below, height, palette))
                .child(
                    div()
                        .flex_none()
                        .text_color(palette.muted)
                        .child(pull.host.reference(pull.number)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(if pull.state == "OPEN" {
                            palette.foreground
                        } else {
                            palette.muted
                        })
                        .child(pull.title.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .max_w(px(if density == Density::Card { 132. } else { 96. }))
                        .truncate()
                        .text_color(word_color(tone, palette))
                        .child(word),
                );
            if let Some(pick) = pick.clone() {
                let url = pull.url.clone();
                line = line
                    .cursor_pointer()
                    .hover(|style| menu_item_hover(style, palette))
                    .active(|style| style.opacity(0.72))
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        pick(url.clone(), window, cx);
                    });
            }
            column = column.child(line);
            ix += 1;
        }
        if trunk {
            column = column.child(trunk_row(group.trunk, density, palette));
        }
        list = list.child(column);
    }
    list
}

/// "Stack of 3" over a stack, in the list's quiet voice.
fn group_head(group: &Group, density: Density, palette: Palette) -> Div {
    h_flex()
        .h(px(20.))
        .px(px(6.))
        .gap(px(8.))
        .text_size(px(11.))
        .text_color(palette.faint)
        .child(
            div()
                .w(px(16.))
                .flex()
                .justify_center()
                .child(glyph(Lucide::Layers, palette.faint).size(px(11.))),
        )
        .child(stack::group_label(group))
        .when(density == Density::Card, |head| head.font_weight(FontWeight::MEDIUM))
}

/// The branch a stack merges into, under its bottom pull request: where the rail ends.
fn trunk_row(trunk: &str, density: Density, palette: Palette) -> Div {
    let height = if density == Density::Card { 22. } else { 20. };
    h_flex()
        .h(px(height))
        .px(px(6.))
        .gap(px(8.))
        .text_size(px(11.))
        .text_color(palette.faint)
        .child(
            div()
                .relative()
                .flex_none()
                .w(px(16.))
                .h(px(height))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .absolute()
                        .left(px(7.5))
                        .top_0()
                        .w(px(1.))
                        .h(px(height / 2.))
                        .bg(palette.border),
                )
                .child(div().size(px(7.)).rounded_full().border_1().border_color(palette.faint)),
        )
        .child(div().font_family("monospace").child(trunk.to_string()))
}

/// The chip's picture of the session's pull requests: a dot for each in its state's hue, the dots of a stack joined
/// bottom first, the oldest group on the left. The shown one has a ring. None for one, and past [`STRIP_MAX`].
pub(super) fn pull_strip(pulls: &[PullRequestStatus], shown: &str, palette: Palette) -> Option<Div> {
    if pulls.len() < 2 || pulls.len() > STRIP_MAX {
        return None;
    }
    let mut strip = h_flex().gap(px(5.));
    for group in stack::groups(pulls).iter().rev() {
        let mut run = h_flex();
        for (at, row) in group.rows.iter().rev().enumerate() {
            if at > 0 {
                run = run.child(div().w(px(3.)).h(px(1.)).bg(palette.faint));
            }
            let color = pull_color(Badge::of(row.pull), palette);
            let dot = div().size(px(10.)).flex().items_center().justify_center().flex_none();
            run = run.child(if row.pull.url == shown {
                dot.child(ringed_dot(color))
            } else {
                dot.child(div().size(px(6.)).rounded_full().bg(color))
            });
        }
        strip = strip.child(run);
    }
    Some(strip)
}

/// The shown pull request's mark in the strip: a 1 px ring with a dot inside it, both in the state's hue.
///
/// GPUI snaps each box's edges to device pixels on its own, so a 4 px child centred by flex in a 10 px ring lands a
/// device pixel off centre at a fractional scale (125 %, 150 %). This paints both from the ring's snapped bounds, with
/// the dot's inset a whole number of device pixels on every side.
fn ringed_dot(color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let scale = window.scale_factor();
            let outer = f32::from(bounds.size.width) * scale;
            let (inset, inner) = centered_inset(outer, 4. * scale);
            window.paint_quad(
                outline(bounds, color, BorderStyle::default()).corner_radii(Corners::all(bounds.size.width / 2.)),
            );
            let origin = bounds.origin + point(px(inset / scale), px(inset / scale));
            let side = px(inner / scale);
            window.paint_quad(fill(Bounds::new(origin, size(side, side)), color).corner_radii(Corners::all(side / 2.)));
        },
    )
    .size(px(10.))
    .flex_none()
}

/// The inset and side, in device pixels, of a dot of about `inner` centred in a square of `outer`: the side is
/// rounded, then grown by one when needed so the space left over splits evenly on both sides.
fn centered_inset(outer: f32, inner: f32) -> (f32, f32) {
    let outer = outer.round();
    let mut inner = inner.round().clamp(1., outer);
    if (outer - inner) % 2. != 0. {
        inner += 1.;
    }
    ((outer - inner) / 2., inner)
}

/// What must merge first under a stacked pull request (`stack::stack_note`), on an amber fill: the person acts on it,
/// but nothing failed.
pub(super) fn stack_callout(note: String, palette: Palette) -> Div {
    let warn = pull_color(Badge::Changes, palette);
    h_flex()
        .items_start()
        .gap(px(8.))
        .p(px(8.))
        .rounded(px(8.))
        .bg(warn.opacity(0.1))
        .text_size(px(12.))
        .text_color(palette.foreground)
        .child(glyph(Lucide::Layers, warn).size(px(13.)).mt(px(1.)))
        .child(div().flex_1().min_w(px(0.)).child(note))
}

#[cfg(test)]
mod tests {
    use super::centered_inset;

    #[test]
    fn the_ringed_dot_sits_in_the_middle_at_every_scale() {
        for scale in [1., 1.25, 1.5, 1.75, 2., 2.25, 3.] {
            let outer = (10. * scale as f32).round();
            let (inset, inner) = centered_inset(10. * scale, 4. * scale);
            assert_eq!(inset.fract(), 0., "scale {scale}: the inset is whole device pixels");
            assert_eq!(
                inset * 2. + inner,
                outer,
                "scale {scale}: both sides leave the same space"
            );
            assert!(
                (inner - 4. * scale).abs() <= 1.5,
                "scale {scale}: the dot keeps about its size"
            );
        }
    }
}
