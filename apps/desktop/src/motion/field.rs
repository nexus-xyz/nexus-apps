//! The field behind the sign-in's mark: the F5 mark's recursion carried
//! outward. Each layer of the mark is the one outside it turned 10.18° and shrunk to 0.9205; the field's ring `k` is
//! the rim grown back `k` times. Its rings are 1 px hairlines, faint near the mark, faintest far out, and gone before
//! the window's edge. The field turns once in 140 s against the mark, on the app's shared clock at 30 frames a second,
//! so it never restarts when the screen is drawn anew. Under less motion it stands still.

use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;
use std::time::Duration;

use gpui_kit::{
    Animation, AnimationExt as _, Bounds, Hsla, IntoElement, ParentElement as _, PathBuilder, Pixels, Styled as _,
    canvas, div, point, px,
};

/// One turn of the field.
pub const FIELD_TURN: Duration = Duration::from_secs(140);
/// How many rings: the last ones are past the fade at any window size.
const RINGS: usize = 46;
/// The mark's rim (`app-mark.svg`'s first path): its radius in the path's units, its first corner's angle in degrees,
/// and how far its sides bow out at their middle.
const RIM: f32 = 88.37;
const RIM_ANGLE: f32 = -79.82;
const RIM_BOW: f32 = 0.035;
/// From one layer to the one inside it: the size and the bow it keeps. The angle step is the unfurl's `STEP`.
const RATIO: f32 = 0.9205;
const BOW_RATIO: f32 = 0.697;
/// The rim's radius against the mark's side: `app-mark.svg` draws the paths at 0.8853 in a 168-unit box.
const RIM_OF_SIDE: f32 = RIM * 0.8853 / 168.;
/// Points along each side of a ring, as in the mark.
const STEPS: usize = 16;

/// Ring `k`'s points, in rims (the rim's radius is 1): six bowed sides, from its first corner.
fn ring(k: usize) -> Vec<[f32; 2]> {
    let grow = RATIO.powi(-(k as i32));
    let angle = RIM_ANGLE - k as f32 * super::unfurl::STEP;
    let bow = (RIM_BOW / BOW_RATIO.powi(k as i32)).min(0.08);
    let corner = |j: usize| {
        let a = (angle + 60. * j as f32).to_radians();
        [grow * a.cos(), grow * a.sin()]
    };
    let mut points = Vec::with_capacity(6 * STEPS);
    for side in 0..6 {
        let (from, to) = (corner(side), corner(side + 1));
        for step in 0..STEPS {
            let t = step as f32 / STEPS as f32;
            let out = 1. + bow * (PI * t).sin();
            points.push([
                (from[0] + (to[0] - from[0]) * t) * out,
                (from[1] + (to[1] - from[1]) * t) * out,
            ]);
        }
    }
    points
}

fn rings() -> &'static [Vec<[f32; 2]>] {
    static RINGS_CACHE: OnceLock<Vec<Vec<[f32; 2]>>> = OnceLock::new();
    RINGS_CACHE.get_or_init(|| (1..=RINGS).map(ring).collect())
}

/// How strongly ring `k` shows at `radius` pixels from the mark whose side is `side`: lighter near the mark, so the
/// mark stays crisp, fading out with the ring's number, and with the distance: whole to 1.7 sides, half at 4.5, gone at
/// 9.6.
pub(crate) fn ring_light(k: usize, radius: f32, side: f32) -> f32 {
    let own = (1. - k as f32 / 48.).max(0.).powf(1.2) * (k as f32 / 3.).min(1.);
    let r = radius / side.max(1.);
    let fade = if r <= 1.7 {
        1.
    } else if r <= 4.5 {
        1. - 0.5 * (r - 1.7) / 2.8
    } else {
        (0.5 * (1. - (r - 4.5) / 5.1)).max(0.)
    };
    own * fade
}

/// The field around a mark `side` pixels wide at the middle of its parent, in `color` at `strength` (its strongest
/// ring's opacity). Put it first in a `relative` parent that fills the area, and the mark over its middle.
pub fn field(id: &'static str, side: f32, color: Hsla, strength: f32) -> impl IntoElement {
    div().absolute().inset_0().with_animation(
        id,
        Animation::new(FIELD_TURN).repeat_synced().with_max_fps(30.),
        move |frame, phase| {
            frame.child(
                canvas(
                    |_, _, _| {},
                    move |bounds: Bounds<Pixels>, _, window, cx| {
                        // Against the mark: back, a turn in 140 s. Under less motion it stands as drawn.
                        let turn = if cx.reduce_motion() { 0. } else { -phase * TAU };
                        let (sin, cos) = turn.sin_cos();
                        let center = bounds.center();
                        let rim = side * RIM_OF_SIDE;
                        for (index, points) in rings().iter().enumerate() {
                            let k = index + 1;
                            let radius = rim * RATIO.powi(-(k as i32));
                            let light = strength * ring_light(k, radius, side);
                            if light < 0.002 {
                                continue;
                            }
                            let placed: Vec<_> = points
                                .iter()
                                .map(|&[x, y]| {
                                    let (x, y) = (x * cos - y * sin, x * sin + y * cos);
                                    point(center.x + px(x * rim), center.y + px(y * rim))
                                })
                                .collect();
                            let mut path = PathBuilder::stroke(px(1.));
                            path.add_polygon(&placed, true);
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color.opacity(light));
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
    fn the_first_ring_is_the_rim_grown_once_and_turned_back_a_step() {
        let first = ring(1);
        let [x, y] = first[0];
        assert!(((x * x + y * y).sqrt() - 1. / RATIO).abs() < 1e-3);
        let angle = y.atan2(x).to_degrees();
        assert!(
            (angle - (RIM_ANGLE - super::super::unfurl::STEP)).abs() < 1e-2,
            "{angle}"
        );
        assert_eq!(first.len(), 6 * STEPS);
    }

    #[test]
    fn the_field_is_faint_by_the_mark_and_gone_far_out() {
        assert!(ring_light(1, 40., 64.) < ring_light(3, 40., 64.));
        assert_eq!(ring_light(5, 64. * 10., 64.), 0.);
        assert!(ring_light(5, 64. * 3., 64.) < ring_light(5, 64., 64.));
        assert_eq!(ring_light(48, 10., 64.), 0.);
    }
}
