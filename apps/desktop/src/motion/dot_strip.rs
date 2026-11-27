//! A strip of dots under the now line while a session starts, in the
//! language of the new-session backdrop (`workspace/backdrop_live.rs`): a dot lights where a moving field passes its
//! ordered-dither threshold. Here the field is a ripple that a soft window carries left to right. It is a strip, never a
//! ground (brand v5: no particle fields behind content). Under less motion it is not drawn.

use std::f32::consts::PI;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, Bounds, Corners, Hsla, IntoElement, Pixels, SharedString, canvas, div, fill, point,
    px, size,
};

/// One pass of the window over the strip: π seconds, so the ripple (two waves a pass) loops with it.
const PASS: Duration = Duration::from_nanos(3_141_592_654);
/// The dot grid's cell, and a dot's diameter in it.
const CELL: f32 = 4.;
const DOT: f32 = 2.2;
/// The 4 × 4 Bayer matrix the backdrop dithers with.
const BAYER: [f32; 16] = [0., 8., 2., 10., 12., 4., 14., 6., 3., 11., 1., 9., 15., 7., 13., 5.];

/// How lit the dot in column `i` of `columns`, row `j`, is when the pass is at `phase` (0 to 1).
pub(crate) fn lit(i: usize, j: usize, columns: usize, phase: f32) -> f32 {
    let t = phase * PASS.as_secs_f32();
    let x = i as f32 / columns.max(1) as f32;
    let ripple = (i as f32 * 0.35 - t * 4.).sin() * 0.5 + 0.5;
    // The window runs from a fifth of the strip before it to a fifth past it, and fades in and out with the pass, so the
    // next pass starts from a dark strip instead of jumping back.
    let centre = phase * 1.4 - 0.2;
    let field = ripple * (-((x - centre) * 3.).powi(2)).exp() * (PI * phase).sin();
    let threshold = (BAYER[(j % 4) * 4 + i % 4] + 0.5) / 16.;
    ((field - threshold) / 0.08).clamp(0., 1.)
}

/// The strip, `width` by `height` pixels, in `color`. Its pass reads the app's shared clock, not the element's state, so
/// it runs on when the now line is drawn anew (the next step's words).
pub fn dot_strip(id: impl Into<SharedString>, width: f32, height: f32, color: Hsla) -> impl IntoElement {
    let id: SharedString = id.into();
    div().w(px(width)).h(px(height)).flex_none().with_animation(
        id,
        Animation::new(PASS)
            .repeat_synced()
            .with_max_fps(crate::motion::LOOP_FPS),
        move |strip, phase| {
            strip.child(
                canvas(
                    |_, _, _| {},
                    move |bounds: Bounds<Pixels>, _, window, cx| {
                        if cx.reduce_motion() {
                            return;
                        }
                        let columns = (f32::from(bounds.size.width) / CELL) as usize;
                        let rows = (f32::from(bounds.size.height) / CELL) as usize;
                        let r = px(DOT / 2.);
                        for j in 0..rows {
                            for i in 0..columns {
                                let light = lit(i, j, columns, phase);
                                if light <= 0.02 {
                                    continue;
                                }
                                let at = point(
                                    bounds.origin.x + px(i as f32 * CELL + (CELL - DOT) / 2.),
                                    bounds.origin.y + px(j as f32 * CELL + (CELL - DOT) / 2.),
                                );
                                window.paint_quad(
                                    fill(Bounds::new(at, size(r * 2., r * 2.)), color.opacity(light))
                                        .corner_radii(Corners::all(r)),
                                );
                            }
                        }
                    },
                )
                .size_full(),
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_crosses_the_strip_left_to_right_and_leaves_it_dark_between_passes() {
        let columns = 60;
        let lit_columns = |phase: f32| -> Vec<usize> {
            (0..columns)
                .filter(|&i| (0..2).any(|j| lit(i, j, columns, phase) > 0.))
                .collect()
        };
        // At the start of a pass the window is before the strip: nothing on its right half lights.
        assert!(lit_columns(0.).iter().all(|&i| i < columns / 2));
        // Half way, it lights the middle; near the end, the right.
        let middle = lit_columns(0.5);
        assert!(middle.iter().any(|&i| (25..35).contains(&i)));
        let late = lit_columns(0.85);
        assert!(late.iter().all(|&i| i > columns / 3), "{late:?}");
    }

    #[test]
    fn a_pass_loops_without_a_jump() {
        for i in 0..60 {
            for j in 0..2 {
                assert!(
                    (lit(i, j, 60, 0.) - lit(i, j, 60, 1.)).abs() < 0.05,
                    "column {i} row {j}"
                );
            }
        }
    }
}
