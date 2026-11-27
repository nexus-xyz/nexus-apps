//! A 3D icon for an agent at work while the model generates, one of three:
//!
//! - Depth (T1): the F5 mark's 11 layers stand apart in depth, rim in front and core behind, so the mark becomes the
//!   tunnel it draws (brand v5's mark: each layer is the last one turned and shrunk). The stack
//!   precesses, its axis circling 22° off the line of sight every 6 s, while the layers keep their 14 s turn.
//! - Globe (T2): the new-session backdrop's Lattice (`workspace/backdrop_live.rs`) as an icon: a wire sphere on a
//!   tilted axis, turning once in 8 s.
//! - Gyro (T3): three of the mark's layers, each turning on its own axis.
//!
//! Each is wire lines in 3D, turned and drawn in perspective, nearer lines brighter: GPUI paints them as paths, a few
//! per frame (segments of one light share a path), at 30 frames a second. A turn picks one at random ([`Solid::pick`])
//! and keeps it. Under less motion the icon stands still, facing front.

use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, Bounds, Hsla, IntoElement, PathBuilder, Pixels, SharedString, canvas, div, point, px,
};

/// The three icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Solid {
    Depth,
    Globe,
    Gyro,
}

impl Solid {
    pub const ALL: [Solid; 3] = [Solid::Depth, Solid::Globe, Solid::Gyro];

    /// One of the three for `seed`, spread evenly: give a turn's identity, so the icon holds for the turn and changes
    /// with the next.
    pub fn pick(seed: u64) -> Solid {
        // SplitMix64's finaliser: neighbouring seeds (turn 4, turn 5) land far apart.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self::ALL[(z % 3) as usize]
    }
}

/// The loop every motion fits in a whole number of times (6, 14, 8, 4 and 7 s all divide it), so it never jumps.
const LOOP: Duration = Duration::from_secs(168);
/// The eye, this many radii in front of the centre: nearer is larger.
const EYE: f32 = 3.2;
/// The radius of the drawing, as a part of the icon's size.
const REACH: f32 = 0.44;
/// Lights a frame's segments are rounded to, so segments of one light share one path.
const LIGHTS: f32 = 8.;

type P3 = [f32; 3];

/// A wire line's piece: where it starts and ends in 3D (the radius is 1), how wide it is at 120 px, and how lit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Segment {
    pub from: P3,
    pub to: P3,
    pub width: f32,
    pub light: f32,
}

fn rz([x, y, z]: P3, a: f32) -> P3 {
    [x * a.cos() - y * a.sin(), x * a.sin() + y * a.cos(), z]
}
fn rx([x, y, z]: P3, a: f32) -> P3 {
    [x, y * a.cos() - z * a.sin(), y * a.sin() + z * a.cos()]
}
fn ry([x, y, z]: P3, a: f32) -> P3 {
    [x * a.cos() + z * a.sin(), y, -x * a.sin() + z * a.cos()]
}

/// Where a point lands in an icon `size` wide, from its top left, and how much nearness scales it.
pub(crate) fn project([x, y, z]: P3, size: f32) -> (f32, f32, f32) {
    let k = EYE / (EYE - z);
    (size / 2. + x * k * size * REACH, size / 2. + y * k * size * REACH, k)
}

/// The app mark's layers, rim first, as closed outlines in a unit radius, and their stroke widths in the mark's units.
fn layers() -> &'static [(f32, Vec<[f32; 2]>)] {
    static LAYERS: OnceLock<Vec<(f32, Vec<[f32; 2]>)>> = OnceLock::new();
    LAYERS.get_or_init(|| {
        crate::assets::mark_layers()
            .iter()
            .map(|svg| {
                let svg = std::str::from_utf8(svg).expect("the mark is UTF-8");
                let attr = |name: &str| {
                    let from = svg.find(&format!("{name}=\"")).expect("the layer has the attribute") + name.len() + 2;
                    &svg[from..from + svg[from..].find('"').expect("the attribute closes")]
                };
                let width: f32 = attr("stroke-width").parse().expect("a stroke width");
                // The mark's frame: a 168-unit box, the paths scaled 0.8853 (`app-mark.svg`).
                let points = attr("d")
                    .split_whitespace()
                    .filter_map(|step| {
                        let (x, y) = step.trim_start_matches(['M', 'L']).split_once(',')?;
                        Some([
                            x.parse::<f32>().ok()? * 0.8853 / 84.,
                            y.parse::<f32>().ok()? * 0.8853 / 84.,
                        ])
                    })
                    .collect();
                (width, points)
            })
            .collect()
    })
}

/// A closed outline's segments, each lit by `light` from its middle's depth.
fn outline(points: &[P3], width: f32, light: impl Fn(f32) -> f32, out: &mut Vec<Segment>) {
    for (i, &from) in points.iter().enumerate() {
        let to = points[(i + 1) % points.len()];
        out.push(Segment {
            from,
            to,
            width,
            light: light((from[2] + to[2]) / 2.).clamp(0., 1.),
        });
    }
}

/// The icon's segments `t` seconds into its motion; `still` stands it facing front.
pub(crate) fn segments(solid: Solid, t: f32, still: bool) -> Vec<Segment> {
    let mut out = Vec::new();
    match solid {
        Solid::Depth => {
            let (tilt, depth) = (if still { 0. } else { 22f32.to_radians() }, 0.09);
            let (around, spin) = (t * TAU / 6., if still { 0. } else { t * TAU / 14. });
            for (i, (width, points)) in layers().iter().enumerate() {
                let z = (5. - i as f32) * depth;
                let placed: Vec<P3> = points
                    .iter()
                    .map(|&[x, y]| {
                        let [x, y, _] = rz([x, y, 0.], spin);
                        ry(rx([x, y, z], tilt * around.cos()), tilt * around.sin())
                    })
                    .collect();
                outline(
                    &placed,
                    (width * 1.15).max(0.35),
                    |z| 0.38 + 0.62 * (z / (5. * depth) * 0.5 + 0.5),
                    &mut out,
                );
            }
        }
        Solid::Globe => {
            let (turn, radius) = (if still { 0.5 } else { t * TAU / 8. }, 0.92);
            let place = |p: P3| rx(ry(p, turn), 0.45);
            let light = |z: f32| if z > 0. { 0.35 + 0.65 * z / radius } else { 0.16 };
            let ring =
                |at: &dyn Fn(f32) -> P3| -> Vec<P3> { (0..48).map(|k| place(at(k as f32 / 48. * TAU))).collect() };
            for latitude in [-60f32, -30., 0., 30., 60.] {
                let la = latitude.to_radians();
                let points = ring(&|lo| {
                    [
                        radius * la.cos() * lo.sin(),
                        -radius * la.sin(),
                        radius * la.cos() * lo.cos(),
                    ]
                });
                outline(&points, 1.4, light, &mut out);
            }
            for meridian in 0..6 {
                let lo = meridian as f32 * PI / 6.;
                let points = ring(&|la| {
                    [
                        radius * la.cos() * lo.sin(),
                        radius * la.sin(),
                        radius * la.cos() * lo.cos(),
                    ]
                });
                outline(&points, 1.4, light, &mut out);
            }
        }
        Solid::Gyro => {
            // The rim on Y every 4 s, a middle layer on X every 6 s, the core on a tilted Z every 7 s.
            for (layer, size, axis, period) in [(0, 1., 0, 4.), (4, 0.82, 1, 6.), (9, 0.62, 2, 7.)] {
                let (width, points) = &layers()[layer];
                let a = if still { 0. } else { t * TAU / period };
                let placed: Vec<P3> = points
                    .iter()
                    .map(|&[x, y]| {
                        let q = [x * size / 0.93, y * size / 0.93, 0.];
                        let q = match axis {
                            0 => ry(q, a),
                            1 => rx(q, a),
                            _ => rx(rz(q, a), 0.6),
                        };
                        rx(ry(q, 0.35), -0.25)
                    })
                    .collect();
                outline(&placed, width.max(1.6), |z| 0.4 + 0.6 * (z * 0.5 + 0.5), &mut out);
            }
        }
    }
    out
}

/// The 3D icon `solid`, `size` pixels square, in `color`. Its motion reads the app's shared clock, not the element's
/// state, so it runs on as one motion when the line around it is drawn anew (a new summary, a new call).
pub fn solid(id: impl Into<SharedString>, solid: Solid, size: f32, color: Hsla) -> impl IntoElement {
    let id: SharedString = id.into();
    div().size(px(size)).flex_none().with_animation(
        id,
        Animation::new(LOOP)
            .repeat_synced()
            .with_max_fps(crate::motion::LOOP_FPS),
        move |frame, phase| {
            frame.child(
                canvas(
                    |_, _, _| {},
                    move |bounds: Bounds<Pixels>, _, window, cx| {
                        let still = cx.reduce_motion();
                        let side = f32::from(bounds.size.width);
                        let scale = side / 120.;
                        // One path for each light and width (a quarter pixel): those segments share a stroke and a
                        // colour, so a frame paints a few dozen paths, not a thousand.
                        let mut paths = std::collections::BTreeMap::<(usize, u32), PathBuilder>::new();
                        for segment in segments(solid, phase * LOOP.as_secs_f32(), still) {
                            let (ax, ay, ak) = project(segment.from, side);
                            let (bx, by, bk) = project(segment.to, side);
                            let level = (segment.light * LIGHTS).round() as usize;
                            let quarters = ((segment.width * scale * (ak + bk) / 2.).max(0.5) * 4.).round() as u32;
                            let builder = paths
                                .entry((level, quarters))
                                .or_insert_with(|| PathBuilder::stroke(px(quarters as f32 / 4.)));
                            builder.move_to(point(bounds.origin.x + px(ax), bounds.origin.y + px(ay)));
                            builder.line_to(point(bounds.origin.x + px(bx), bounds.origin.y + px(by)));
                        }
                        for ((level, _), builder) in paths {
                            if let Ok(path) = builder.build() {
                                window.paint_path(path, color.opacity(level as f32 / LIGHTS));
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
    fn a_turn_picks_one_of_the_three_and_the_three_come_up_evenly() {
        let mut counts = [0usize; 3];
        for seed in 0..3000u64 {
            let at = Solid::ALL.iter().position(|solid| *solid == Solid::pick(seed)).unwrap();
            counts[at] += 1;
        }
        assert!(counts.iter().all(|&n| (850..=1150).contains(&n)), "{counts:?}");
        assert_eq!(Solid::pick(42), Solid::pick(42), "a turn keeps its icon");
    }

    #[test]
    fn the_marks_layers_are_read_rim_first_inside_a_unit_radius() {
        let layers = layers();
        assert_eq!(layers.len(), 11);
        assert!(layers.iter().all(|(_, points)| points.len() == 96));
        let reach = |points: &[[f32; 2]]| points.iter().map(|[x, y]| f32::hypot(*x, *y)).fold(0., f32::max);
        assert!(
            reach(&layers[0].1) <= 1. && reach(&layers[0].1) > 0.9,
            "the rim nearly fills the radius"
        );
        assert!(
            reach(&layers[10].1) < reach(&layers[0].1) / 2.,
            "the core sits well inside"
        );
    }

    #[test]
    fn every_icon_stays_in_its_box_and_lights_within_bounds() {
        for solid in Solid::ALL {
            for t in [0., 1.7, 13.3, 84., 167.9] {
                let parts = segments(solid, t, false);
                assert!(!parts.is_empty());
                for segment in parts {
                    assert!((0. ..=1.).contains(&segment.light));
                    for end in [segment.from, segment.to] {
                        let (x, y, _) = project(end, 120.);
                        assert!(
                            (-2. ..=122.).contains(&x) && (-2. ..=122.).contains(&y),
                            "{solid:?} at {t} s: {x}, {y}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn depth_puts_the_rim_in_front_and_lights_it_brighter() {
        let parts = segments(Solid::Depth, 0., true);
        let (rim, core) = (parts.first().unwrap(), parts.last().unwrap());
        assert!(rim.from[2] > core.from[2]);
        assert!(rim.light > core.light);
    }

    #[test]
    fn the_globes_far_side_is_faint() {
        let parts = segments(Solid::Globe, 0., false);
        let back = parts
            .iter()
            .filter(|s| s.from[2] < 0. && s.to[2] < 0.)
            .map(|s| s.light)
            .fold(0., f32::max);
        let front = parts.iter().map(|s| s.light).fold(0., f32::max);
        assert!(back < 0.2 && front > 0.9, "back {back}, front {front}");
    }

    #[test]
    fn each_motion_loops_without_a_jump() {
        for solid in Solid::ALL {
            let (start, end) = (segments(solid, 0., false), segments(solid, LOOP.as_secs_f32(), false));
            for (a, b) in start.iter().zip(&end) {
                for i in 0..3 {
                    assert!((a.from[i] - b.from[i]).abs() < 1e-2, "{solid:?}");
                }
            }
        }
    }

    #[test]
    fn standing_still_faces_front() {
        for (a, b) in segments(Solid::Depth, 0., true)
            .iter()
            .zip(&segments(Solid::Depth, 50., true))
        {
            assert_eq!(a, b, "a still icon does not move with time");
        }
    }
}
