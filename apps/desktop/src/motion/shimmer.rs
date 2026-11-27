//! A line of text with a band of light sweeping across it, left to right, while something works ("Thinking…", a tool
//! that runs). Brand v5's text shimmer: GPUI has no gradient text, so the line is painted once per narrow strip,
//! each strip clipped to itself and in its own color, as Zeron's tool title shimmer does. Strips of the same color are painted together, so
//! a line costs a handful of paints a frame, at 30 frames a second.

use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, Bounds, ContentMask, Hsla, IntoElement, Pixels, Rgba, SharedString, TextAlign,
    TextRun, canvas, div, point, px, size,
};

/// One sweep of the band across the line (`motion.shimmer`, 2.4 s, linear, no pause).
const SWEEP: Duration = Duration::from_millis(2400);
/// The brand builds the sweep as `background-clip: text` over a 300 % gradient whose band runs from 35 % to 65 %: 30 %
/// of three line widths, so the band reaches 0.45 of the line each side of its middle, and its light falls off in a
/// straight line.
const HALF_WIDTH: f32 = 0.45;
/// The strip width in pixels, and how many color steps the band has.
const STRIP: f32 = 2.;
const STEPS: f32 = 8.;

/// How lit the point `x` (0 at the line's start, 1 at its end) is when the sweep is at `phase` (0 to 1). The gradient's
/// position runs from 100 % to 0 %, so the band's middle travels from half a line before the start to half a line past
/// the end, and the line is unlit only for a moment at each end of the sweep.
pub(crate) fn lit(x: f32, phase: f32) -> f32 {
    let middle = -0.5 + 2. * phase;
    (1. - (x - middle).abs() / HALF_WIDTH).clamp(0., 1.)
}

fn mix(rest: Hsla, peak: Hsla, t: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (rest.into(), peak.into());
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    }
    .into()
}

/// `text` in `rest`, lit to `peak` where the band passes. Under less motion it rests in `rest`.
pub fn shimmer(id: impl Into<SharedString>, text: impl Into<SharedString>, rest: Hsla, peak: Hsla) -> impl IntoElement {
    let text: SharedString = text.into();
    let id: SharedString = id.into();
    // The plain text lays the line out (and is what less motion shows); the canvas paints over it while it moves.
    div().relative().text_color(rest).child(text.clone()).with_animation(
        id,
        Animation::new(SWEEP).repeat().with_max_fps(super::LOOP_FPS),
        move |line, phase| {
            let text = text.clone();
            line.text_color(gpui_kit::transparent_black()).child(
                canvas(
                    |_, _, _| {},
                    move |bounds: Bounds<Pixels>, _, window, cx| {
                        let style = window.text_style();
                        let font_size = style.font_size.to_pixels(window.rem_size());
                        let line_height = style.line_height_in_pixels(window.rem_size());
                        let width = bounds.size.width.max(px(1.));
                        let mut x = px(0.);
                        while x < width {
                            // A run of strips that share a color step: one paint for all of them.
                            let step = |x: Pixels| (lit((x + px(STRIP / 2.)) / width, phase) * STEPS).round();
                            let level = step(x);
                            let mut end = x + px(STRIP);
                            while end < width && step(end) == level {
                                end += px(STRIP);
                            }
                            let run = TextRun {
                                len: text.len(),
                                font: style.font(),
                                color: mix(rest, peak, level / STEPS),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            let shaped = window.text_system().shape_line(text.clone(), font_size, &[run], None);
                            let strip = Bounds::new(
                                point(bounds.origin.x + x, bounds.origin.y),
                                size(end - x, bounds.size.height),
                            );
                            window.with_content_mask(Some(ContentMask { bounds: strip }), |window| {
                                let _ = shaped.paint(bounds.origin, line_height, TextAlign::Left, None, window, cx);
                            });
                            x = end;
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::lit;

    #[test]
    fn the_band_crosses_the_line_and_leaves_it_unlit_at_each_end() {
        // At either end of the sweep the band is off the line.
        for x in [0., 0.5, 1.] {
            assert_eq!(lit(x, 0.), 0.);
            assert_eq!(lit(x, 1.), 0.);
        }
        // Half way, the middle of the line is at the peak, and the ends at the band's edge.
        assert_eq!(lit(0.5, 0.5), 1.);
        assert!(lit(0., 0.5) < 0.0001 && lit(1., 0.5) < 0.0001);
        // The start lights before the end does.
        let first = |x: f32| (0..=100).position(|i| lit(x, i as f32 / 100.) > 0.5).unwrap();
        assert!(first(0.1) < first(0.9));
    }

    #[test]
    fn the_band_is_thirty_percent_of_the_gradient() {
        // 30 % of a 300 % gradient: 0.9 of the line, lit at all.
        let lit_width = (0..=1000).filter(|i| lit(*i as f32 / 1000., 0.5) > 0.).count() as f32 / 1000.;
        assert!((lit_width - 0.9).abs() < 0.01, "{lit_width}");
    }
}
