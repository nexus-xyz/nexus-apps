//! The Agents tab while its first read runs: the list's own shape in grey, the header, the lead and three agents
//! on the hairline branch, so the tab does not jump when the crew arrives. The shapes breathe, and a band of light
//! sweeps across "Reading agents…". Under less motion both rest.
use super::*;
use crate::motion;
use crate::palette::Palette;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;

/// The rows' line widths, as parts of the row: the lead, then each agent's name and its line under it.
const LEAD: f32 = 0.42;
const AGENTS: [(f32, f32); 3] = [(0.58, 0.82), (0.46, 0.7), (0.52, 0.76)];

/// A grey bar `height` tall and `width` of its row wide.
fn bar(height: f32, width: f32, p: Palette) -> Div {
    div().h(px(height)).w(relative(width)).rounded(px(4.)).bg(p.selected)
}

/// A row as `rows.rs` lays one out: the 14px mark, the name and the status word; under them, its line.
fn row(name: f32, line: Option<f32>, p: Palette) -> Div {
    let head = h_flex()
        .w_full()
        .h(px(18.))
        .gap(px(10.))
        .child(
            div()
                .size(px(14.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(px(8.)).rounded_full().bg(p.selected)),
        )
        .child(h_flex().flex_1().min_w(px(0.)).child(bar(10., name, p)))
        .child(div().flex_none().h(px(8.)).w(px(40.)).rounded(px(4.)).bg(p.selected));
    v_flex()
        .w_full()
        .p(px(8.))
        .gap(px(6.))
        .child(head)
        .when_some(line, |row, line| {
            row.child(div().w_full().pl(px(24.)).child(bar(8., line, p)))
        })
}

/// The skeleton under the tab's header. `reduce_motion` stills the pulse and the shimmer.
pub(super) fn crew_loading(p: Palette, reduce_motion: bool) -> AnyElement {
    let label: AnyElement = if reduce_motion {
        div().text_color(p.muted).child("Reading agents…").into_any_element()
    } else {
        motion::shimmer("agents-loading-label", "Reading agents…", p.muted, p.strong).into_any_element()
    };
    let header = h_flex()
        .w_full()
        .min_h(px(28.))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .child(div().flex_1().text_color(p.faint).child("Background tasks"))
        .child(label);
    // The branch as `crew_branch` draws it: a 1px line down the left, a short tick to each row.
    let mut branch = v_flex()
        .relative()
        .w_full()
        .ml(px(4.))
        .pl(px(14.))
        .child(div().absolute().left_0().top_0().bottom(px(14.)).w(px(1.)).bg(p.border));
    for (name, line) in AGENTS {
        branch = branch.child(
            div()
                .relative()
                .w_full()
                .child(
                    div()
                        .absolute()
                        .top(px(16.))
                        .left(px(-14.))
                        .w(px(9.))
                        .h(px(1.))
                        .bg(p.border),
                )
                .child(row(name, Some(line), p)),
        );
    }
    let shapes = v_flex().w_full().child(row(LEAD, None, p)).child(branch);
    let shapes: AnyElement = if reduce_motion {
        shapes.opacity(0.7).into_any_element()
    } else {
        shapes
            .with_animation(
                "agents-loading",
                Animation::new(motion::PULSE)
                    .repeat()
                    .with_max_fps(crate::motion::LOOP_FPS)
                    .with_easing(pulsating_between(0.4, 0.9)),
                |shapes, delta| shapes.opacity(delta),
            )
            .into_any_element()
    };
    v_flex().w_full().gap_3().child(header).child(shapes).into_any_element()
}
