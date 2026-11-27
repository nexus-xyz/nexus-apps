//! The Nexus AI mark above an empty session: F5 (`brand/v5.0/nexus-ai/mark/MARK.md`) as an iris of lit bars, drawn in
//! Braille dots. F5 is eleven six-part layers, each the last one turned and shrunk to sit inside it, 112° from edge to
//! core; the turn is what shrinks them. So the twist works as an iris does: at 0° every layer lies on the rim, open;
//! as it grows, the blades swing in and stop the opening down. The rim stays put and the blades turn inside it. The
//! mark never turns as a whole.
//!
//! The loop (10 s): F5 as drawn; it stops down to 240° and opens back; it opens wide to the rim and closes to F5
//! again. A session starts wide open, so the iris closes into the mark first. While the person types it stands as
//! drawn, dimmed, and goes on from there once the composer is empty again; it leaves with the first prompt (`app.rs`).
//!
//! The depth: each layer is lit as a solid bar would be by a key light from the top left, bright where its outer side
//! faces the light and dim where it faces away. It faces front: no tilt and no body behind it, because at this size a
//! dot is on or off, and any thickness fills the gaps between the layers.
//!
//! Each layer is drawn as a crisp line of dots, one dot wide (two for the band's heaviest bars), never a soft edge: a
//! partly covered dot lit dimmer reads as blur at this size. Weight shows as strength: F5's profile, thin at the edge
//! and the core and heavy in the turning band, sets each layer's strength. One ink, the text colour: no hue on the
//! mark. The geometry is `mark/source/round11.py`'s `soft()`, at the brand's weight.

use std::f64::consts::PI;

use crate::style::{Depth, ansi256};

/// One loop of the iris.
pub const LOOP_MS: u64 = 10_000;
/// Where a session's mark starts in the loop: wide open, about to close.
const START_MS: u64 = 7_900;
/// F5 as drawn: where it stands while the person types.
pub const FRONT_MS: u64 = LOOP_MS - START_MS;
/// How long the mark takes to dim or brighten, and how dim it gets while the person types.
pub const FADE_MS: u64 = 400;
pub const DIM: f64 = 0.18;
/// F5's twist from edge to core (degrees).
const F5_TWIST: f64 = 112.0;

/// The loop's key poses: seconds into it, and the twist from edge to core (degrees). Between two, the pose eases
/// (smootherstep).
const KEYS: [(f64, f64); 9] = [
    (0.0, F5_TWIST),
    (2.6, F5_TWIST),
    (3.8, 240.0),
    (4.6, 240.0),
    (5.8, F5_TWIST),
    (6.8, F5_TWIST),
    (7.9, 0.0),
    (8.5, 0.0),
    (10.0, F5_TWIST),
];

/// The key light's direction across the mark, from the top left (a unit vector; y runs down).
const KEY: (f64, f64) = (-0.588, -0.809);
/// How much of a bar's strength the key light sets.
const LIT: f64 = 0.6;

/// F5's layers and profile (`round15.py`): a line is `WFAC` of the gap to the next layer, its weight rising from
/// `EDGE` to full at `PEAK` and falling to `CORE`; sides bow toward a circle by `BULGE`, less on each layer in.
const LAYERS: usize = 11;
const WFAC: f64 = 0.7;
const EDGE: f64 = 0.12;
const PEAK: f64 = 0.3;
const CORE: f64 = 0.12;
const BULGE: f64 = 0.32;
const DECAY: f64 = 0.7;
const R0: f64 = 96.0;
/// The thinnest stroke, in mark units: wide open, the layers' gaps (and so their weights) are zero.
const MIN_WEIGHT: f64 = 0.9;
/// Points a side, as the layer's curve is stroked.
const STEPS: usize = 10;
/// The mark's units across the stage's height: the outer layer (radius 88), its weight, a little air.
const SPAN: f64 = 196.0;
/// A layer's strength: `FLOOR` of the ink for a hairline, rising with its weight to full for the heaviest bar.
const FLOOR: f64 = 0.4;
/// A stroke this many dots wide or more is drawn two dots wide.
const THICK: f64 = 1.7;

/// The largest stage's rows, and the smallest worth drawing.
const MAX_ROWS: usize = 22;
const MIN_ROWS: usize = 10;

/// The Braille bit of each dot, row by row: left, right.
const DOT_BITS: [u8; 8] = [1, 8, 2, 16, 4, 32, 64, 128];

/// When the terminal does not say its cell size: a cell a little over twice as tall as it is wide.
pub const ASPECT: f64 = 1.05;

/// The iris's twist (degrees) `elapsed_ms` into a session's mark.
fn twist(elapsed_ms: u64) -> f64 {
    let at = ((elapsed_ms + START_MS) % LOOP_MS) as f64 / 1000.0;
    let next = KEYS.iter().position(|key| key.0 > at).unwrap_or(KEYS.len() - 1);
    let (from, to) = (KEYS[next.saturating_sub(1)], KEYS[next]);
    let t = if to.0 > from.0 {
        smooth((at - from.0) / (to.0 - from.0))
    } else {
        1.0
    };
    from.1 + (to.1 - from.1) * t
}

/// One layer of the mark at a twist (mark units).
struct Layer {
    /// Its curve's points.
    points: Vec<(f64, f64)>,
    stroke: f64,
}

/// The layers at a twist, from the rim in.
fn layers(twist: f64) -> Vec<Layer> {
    let count = LAYERS;
    let half = PI / 6.0;
    let turn = (twist / count as f64).to_radians();
    // The rim keeps F5's size and angle at every twist; the blades inside it turn and shrink.
    let rim = (F5_TWIST / count as f64).to_radians();
    let ratio = half.cos() / (half - turn).cos();
    let radius = |i: usize| R0 * half.cos() / (half - rim).cos() * ratio.powi(i as i32 - 1);
    let ease = |x: f64| {
        let x = x.clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    };
    (1..=count)
        .map(|i| {
            let u = (i - 1) as f64 / (count - 1) as f64;
            let profile = if u < PEAK {
                EDGE + (1.0 - EDGE) * ease(u / PEAK)
            } else {
                1.0 - (1.0 - CORE) * ease((u - PEAK) / (1.0 - PEAK))
            };
            let j = i.min(count - 1);
            let stroke = ((radius(j) - radius(j + 1)) * WFAC * profile).max(MIN_WEIGHT);
            let (r, a) = (radius(i), -PI / 2.0 + rim + turn * (i - 1) as f64);
            let bulge = (BULGE * DECAY.powi(i as i32)).min(1.0);
            let corner = |k: usize| {
                let angle = a + k as f64 * PI / 3.0;
                (r * angle.cos(), r * angle.sin())
            };
            // `round11.py`'s `curved()`: each side from corner to corner, bowed toward the circle through them.
            let points = (0..6 * STEPS)
                .map(|step| {
                    let (side, u) = (step / STEPS, (step % STEPS) as f64 / STEPS as f64);
                    let (from, to) = (corner(side), corner(side + 1));
                    let (cx, cy) = (from.0 + (to.0 - from.0) * u, from.1 + (to.1 - from.1) * u);
                    let angle = a + (side as f64 + u) * PI / 3.0;
                    let (ox, oy) = (r * angle.cos(), r * angle.sin());
                    (cx + (ox - cx) * bulge, cy + (oy - cy) * bulge)
                })
                .collect();
            Layer { points, stroke }
        })
        .collect()
}

/// The stage the mark gets on a terminal `width` wide with `rows_free` rows to spare, its cells `aspect` (a dot's
/// height over its width); `None` when it would be too small to read.
pub fn stage(width: usize, rows_free: usize, aspect: f64) -> Option<(usize, usize)> {
    // The mark is round: as many dots across as down, corrected for the dots' shape, and a cell to spare each side.
    let columns_for = |rows: usize| ((rows * 4) as f64 * aspect / 2.0).ceil() as usize + 2;
    let mut rows = rows_free.min(MAX_ROWS);
    while rows >= MIN_ROWS && columns_for(rows) > width.saturating_sub(4) {
        rows -= 1;
    }
    (rows >= MIN_ROWS).then(|| (columns_for(rows), rows))
}

/// How the mark prints: the terminal's background (the ink is the text colour for it, and fades into it), its depth,
/// and its cells' shape. Without colour, or with 16 colours, the dots print in the text colour.
#[derive(Debug, Clone, Copy)]
pub struct Paint {
    pub background: Option<(u8, u8, u8)>,
    pub depth: Option<Depth>,
    /// A dot's height over its width (`ASPECT` when the terminal does not say).
    pub aspect: f64,
}

/// The mark on a `columns` × `rows` stage, `elapsed_ms` into its loop, at `opacity` (1: full, `DIM`: faded).
pub fn lines(columns: usize, rows: usize, elapsed_ms: u64, opacity: f64, paint: Paint) -> Vec<String> {
    // The app's window grey stands in when the background is unknown.
    let background = paint.background.unwrap_or((0x16, 0x16, 0x17));
    // The brand's app `text`: #ECECEC on dark, #1C1C1E on light.
    let ink = if is_light(background) {
        (0x1c, 0x1c, 0x1e)
    } else {
        (0xec, 0xec, 0xec)
    };
    let (width, height) = (columns * 2, rows * 4);
    let scale = height as f64 / SPAN;
    let aspect = paint.aspect.clamp(0.5, 2.0);
    let layers = layers(twist(elapsed_ms));
    let heaviest = layers.iter().map(|layer| layer.stroke).fold(MIN_WEIGHT, f64::max);
    // Each dot's strength: the strongest layer through it; 0 is no dot.
    let mut strength = vec![0.0f64; width * height];
    let mut light = |i: f64, j: f64, value: f64| {
        if i >= 0.0 && j >= 0.0 && (i as usize) < width && (j as usize) < height {
            let dot = &mut strength[j as usize * width + i as usize];
            *dot = dot.max(value);
        }
    };
    // Dot space: a unit is a dot, and x runs `aspect` dots to a unit of y, so the mark stays round.
    let at = |(x, y): (f64, f64)| (x * scale * aspect + width as f64 / 2.0, y * scale + height as f64 / 2.0);
    for Layer { points, stroke } in &layers {
        let base = FLOOR + (1.0 - FLOOR) * stroke / heaviest;
        let thick = stroke * scale >= THICK;
        for (k, &point) in points.iter().enumerate() {
            let next = points[(k + 1) % points.len()];
            // How far the bar's outer side here faces the key light: 1 toward it, -1 away.
            let (mx, my) = ((point.0 + next.0) / 2.0, (point.1 + next.1) / 2.0);
            let facing = (mx * KEY.0 + my * KEY.1) / mx.hypot(my).max(1e-9);
            let value = base * ((1.0 - LIT) + LIT * (0.5 + 0.5 * facing));
            let ((x0, y0), (x1, y1)) = (at(point), at(next));
            let (dx, dy) = (x1 - x0, y1 - y0);
            // One dot a step along the longer axis, at the line's centre there; a thick stroke adds the nearer neighbour.
            if dx.abs() >= dy.abs() {
                for i in (x0.min(x1).round() as i64)..(x0.max(x1).round() as i64) {
                    let y = y0 + dy * (i as f64 + 0.5 - x0) / dx;
                    light(i as f64, y.floor(), value);
                    if thick {
                        light(
                            i as f64,
                            if y.fract() < 0.5 {
                                y.floor() - 1.0
                            } else {
                                y.floor() + 1.0
                            },
                            value,
                        );
                    }
                }
            } else {
                for j in (y0.min(y1).round() as i64)..(y0.max(y1).round() as i64) {
                    let x = x0 + dx * (j as f64 + 0.5 - y0) / dy;
                    light(x.floor(), j as f64, value);
                    if thick {
                        light(
                            if x.fract() < 0.5 {
                                x.floor() - 1.0
                            } else {
                                x.floor() + 1.0
                            },
                            j as f64,
                            value,
                        );
                    }
                }
            }
        }
    }
    (0..rows)
        .map(|row| {
            let mut line = String::new();
            let mut current = String::new();
            for column in 0..columns {
                let (mut dots, mut strongest) = (0u8, 0.0f64);
                for (point, bit) in DOT_BITS.into_iter().enumerate() {
                    let value = strength[(row * 4 + point / 2) * width + column * 2 + point % 2];
                    if value > 0.0 {
                        dots |= bit;
                        strongest = strongest.max(value);
                    }
                }
                if dots == 0 {
                    line.push(' ');
                    continue;
                }
                let strength = strongest * opacity;
                let mix = |ink: u8, ground: u8| {
                    (f64::from(ground) + (f64::from(ink) - f64::from(ground)) * strength).round() as u8
                };
                let rgb = (
                    mix(ink.0, background.0),
                    mix(ink.1, background.1),
                    mix(ink.2, background.2),
                );
                let code = match paint.depth {
                    Some(Depth::True) => format!("\x1b[38;2;{};{};{}m", rgb.0, rgb.1, rgb.2),
                    Some(Depth::C256) => {
                        format!(
                            "\x1b[38;5;{}m",
                            ansi256(&format!("{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2))
                        )
                    }
                    Some(Depth::C16) | None => String::new(),
                };
                if code != current {
                    line.push_str(&code);
                    current = code;
                }
                line.push(char::from_u32(0x2800 + u32::from(dots)).unwrap_or(' '));
            }
            let mut line = line.trim_end().to_string();
            if !current.is_empty() {
                line.push_str("\x1b[39m");
            }
            line
        })
        .collect()
}

/// Smootherstep: slow at both ends.
pub fn smooth(value: f64) -> f64 {
    let t = value.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

pub fn is_light((r, g, b): (u8, u8, u8)) -> bool {
    0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b) > 128.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: Paint = Paint {
        background: None,
        depth: None,
        aspect: ASPECT,
    };

    fn dots(lines: &[String]) -> u32 {
        lines
            .iter()
            .flat_map(|line| line.chars())
            .map(|c| (c as u32).saturating_sub(0x2800).count_ones())
            .sum()
    }

    /// The cells of the stage a frame inks: its first and last row, and its leftmost and rightmost column.
    fn bounds(drawn: &[String]) -> (usize, usize, usize, usize) {
        let top = drawn.iter().position(|line| !line.is_empty()).unwrap_or(0);
        let bottom = drawn.iter().rposition(|line| !line.is_empty()).unwrap_or(0);
        let left = drawn
            .iter()
            .filter_map(|line| line.find(|c| c != ' '))
            .min()
            .unwrap_or(0);
        let right = drawn.iter().map(|line| line.chars().count()).max().unwrap_or(0);
        (top, bottom, left, right)
    }

    /// The iris's opening: the widest run of blank cells inside the ink on any row.
    fn opening(drawn: &[String]) -> usize {
        drawn
            .iter()
            .map(|line| {
                let cells: Vec<char> = line.chars().collect();
                let (mut widest, mut run, mut inside) = (0, 0, false);
                for c in cells {
                    if c == ' ' {
                        run += 1;
                    } else {
                        if inside {
                            widest = widest.max(run);
                        }
                        (run, inside) = (0, true);
                    }
                }
                widest
            })
            .max()
            .unwrap_or(0)
    }

    /// Where in a session's loop the iris is at `seconds` into the loop's key poses.
    fn at_key(seconds: f64) -> u64 {
        ((seconds * 1000.0) as u64 + LOOP_MS - START_MS) % LOOP_MS
    }

    #[test]
    fn it_closes_from_wide_open_then_loops() {
        let (columns, rows) = stage(120, 40, ASPECT).expect("room for the mark");
        let at = |ms: u64| lines(columns, rows, ms, 1.0, PLAIN);
        let drawn = at(FRONT_MS);
        assert_eq!(drawn.len(), rows);
        assert!(drawn.iter().all(|line| line.chars().count() <= columns));
        assert_eq!(at(FRONT_MS + LOOP_MS * 3), drawn, "it loops");
        assert_ne!(at(at_key(3.2)), drawn, "it stops down");
        // The session starts wide open: the rim alone, far fewer dots than the eleven layers.
        assert!(dots(&at(0)) * 3 < dots(&drawn), "{} and {}", dots(&at(0)), dots(&drawn));
        // As much room on the left as on the right, to a cell, and above as below.
        let (top, bottom, left, right) = bounds(&drawn);
        assert!(
            left.abs_diff(columns - right) <= 1,
            "{left} and {} free: {drawn:#?}",
            columns - right
        );
        assert!(
            top.abs_diff(rows - 1 - bottom) <= 1,
            "{top} rows above, {} below",
            rows - 1 - bottom
        );
    }

    #[test]
    fn the_rim_stays_put_and_the_blades_stop_it_down() {
        let (columns, rows) = stage(120, 40, ASPECT).expect("room for the mark");
        let at = |ms: u64| lines(columns, rows, ms, 1.0, PLAIN);
        // The mark never turns as a whole: the rim holds the same cells at every pose.
        let rim = bounds(&at(FRONT_MS));
        for ms in (0..LOOP_MS).step_by(250) {
            assert_eq!(bounds(&at(ms)), rim, "at {ms} ms");
        }
        let (f5, stopped, open) = (
            opening(&at(FRONT_MS)),
            opening(&at(at_key(4.2))),
            opening(&at(at_key(8.2))),
        );
        assert!(f5 >= 8, "F5's opening: {f5}");
        assert!(
            stopped < f5 && f5 < open,
            "stopped down {stopped}, F5 {f5}, wide open {open}"
        );
    }

    #[test]
    fn the_key_light_comes_from_the_top_left() {
        let paint = Paint {
            background: Some((0, 0, 0)),
            depth: Some(Depth::True),
            aspect: ASPECT,
        };
        let (columns, rows) = stage(120, 40, ASPECT).expect("room for the mark");
        // The mean grey of the dots in the top-left and the bottom-right quarter of the stage.
        let (mut top_left, mut bottom_right) = ((0.0, 0.0), (0.0, 0.0));
        for (row, line) in lines(columns, rows, FRONT_MS, 1.0, paint).iter().enumerate() {
            let (mut column, mut grey) = (0, 0.0);
            for part in line.replace("\x1b[39m", "").split("\x1b[38;2;") {
                if let Some((rgb, cells)) = part.split_once('m') {
                    grey = rgb
                        .split(';')
                        .next()
                        .and_then(|r| r.parse::<f64>().ok())
                        .unwrap_or(grey);
                    for c in cells.chars() {
                        let quarter = match (row < rows / 2, column < columns / 2) {
                            (true, true) => Some(&mut top_left),
                            (false, false) => Some(&mut bottom_right),
                            _ => None,
                        };
                        if let (Some(sum), true) = (quarter, c != ' ') {
                            *sum = (sum.0 + grey, sum.1 + 1.0);
                        }
                        column += 1;
                    }
                } else {
                    column += part.chars().count();
                }
            }
        }
        let (lit, shaded) = (top_left.0 / top_left.1, bottom_right.0 / bottom_right.1);
        assert!(lit > shaded * 1.2, "top left {lit:.0}, bottom right {shaded:.0}");
    }

    #[test]
    fn one_ink_follows_the_depth_and_fades() {
        let paint = Paint {
            background: Some((0, 0, 0)),
            depth: Some(Depth::True),
            aspect: ASPECT,
        };
        let full = lines(40, 20, FRONT_MS, 1.0, paint).concat();
        let dim = lines(40, 20, FRONT_MS, DIM, paint).concat();
        assert!(full.contains("\x1b[38;2;") && full.ends_with("\x1b[39m"));
        assert_ne!(full, dim);
        // Every colour is a neutral grey: one ink, never a hue.
        for code in full.split("\x1b[38;2;").skip(1) {
            let rgb: Vec<&str> = code.split('m').next().unwrap_or("").split(';').collect();
            assert!(rgb.len() == 3 && rgb[0] == rgb[1] && rgb[1] == rgb[2], "{rgb:?}");
        }
        let sixteen = Paint {
            depth: Some(Depth::C16),
            ..PLAIN
        };
        assert!(!lines(40, 20, FRONT_MS, 1.0, sixteen).concat().contains('\x1b'));
    }

    #[test]
    fn the_stage_fits_the_terminal_and_its_cells() {
        assert_eq!(stage(120, 40, 1.0), Some((46, 22)));
        assert_eq!(stage(120, 14, 1.0), Some((30, 14)));
        // Wide cells (Konsole's 8 × 15 px) need fewer columns for the same rows.
        assert_eq!(stage(120, 40, 0.95), Some((44, 22)));
        assert_eq!(stage(40, 40, 1.0), Some((36, 17)));
        assert_eq!(stage(120, 8, 1.0), None);
        assert_eq!(stage(20, 40, 1.0), None);
    }
}
