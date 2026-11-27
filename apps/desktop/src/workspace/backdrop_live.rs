//! The new session's background in motion (`backdrop.rs`): each art drawn live, dot by dot, from the fields that
//! `assets/backgrounds/make/gen.py` and `convergence.py` dither into the shipped images, with time in them.
//!
//! - Eclipse: the light slides along the planet's edge, the corona's rays turn, and the stars twinkle.
//! - Lattice: the sphere turns on its axis.
//! - Orbits: the planets go round the sun, the inner ones faster, and the sun breathes.
//! - Convergence: light runs along the threads into the beam, and down the beam.
//! - Moiré: one ring set ripples out and the other in, so their interference sweeps.
//! - Horizon: the floor's grid rolls toward the viewer, and the stars twinkle.
//!
//! GPUI puts an image on whole device pixels, so a sliding image of a dot matrix ticks. Here the dots stay put: each
//! cell is a whole number of device pixels, and each dot fades in and out with the field under it. A dot lights where
//! the field passes its ordered-dither threshold, as in `gen.py`, over a narrow soft band instead of a step, so a
//! shape moves smoothly through the dots.
//!
//! A frame is built off the UI thread. The parts of a field that do not move are kept per layout (`Still`), so a frame
//! only adds the moving parts. The frosted panels get a blurred copy of each frame.

use std::f32::consts::{PI, TAU};
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::*;
use image::RgbaImage;
use smallvec::SmallVec;

use super::backdrop::Art;

/// The art's size in its own pixels (`gen.py`'s `W` and `H`), and its dot cell.
const W: f32 = 2400.;
const H: f32 = 1500.;
const CELL: f32 = 4.;

/// How often a frame is built while the art moves.
pub(super) const FPS: f32 = 20.;

/// Where the art is drawn: the room it covers, and the window's scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Layout {
    pub room: Bounds<Pixels>,
    pub scale: f32,
}

/// The dot grid over a room.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Grid {
    /// The first cell's top left, on a device pixel.
    origin: Point<Pixels>,
    /// A cell's side and a dot's, in logical pixels: whole numbers of device pixels.
    cell: f32,
    dot: f32,
    cols: usize,
    rows: usize,
    /// The art pixel at the grid's origin, and art pixels per cell.
    art_x: f32,
    art_y: f32,
    step: f32,
}

impl Grid {
    fn of(layout: Layout) -> Grid {
        let room = layout.room;
        let (width, height) = (f32::from(room.size.width), f32::from(room.size.height));
        let scale = layout.scale.max(1.);
        // The art covers the room, as the still image does (`backdrop::cover`).
        let cover = (width / W).max(height / H).max(1e-3);
        let cell_px = (CELL * cover * scale).round().max(2.);
        // `gen.py`'s dot is three quarters of its cell, with a gap of at least one device pixel.
        let dot_px = (cell_px * 0.75).round().min(cell_px - 1.).max(1.);
        let cell = cell_px / scale;
        let snap = |value: f32| (value * scale).round() / scale;
        let origin = point(px(snap(f32::from(room.origin.x))), px(snap(f32::from(room.origin.y))));
        let cover_x = f32::from(room.origin.x) + (width - W * cover) / 2.;
        let cover_y = f32::from(room.origin.y) + (height - H * cover) / 2.;
        Grid {
            origin,
            cell,
            dot: dot_px / scale,
            cols: (width / cell).ceil() as usize + 1,
            rows: (height / cell).ceil() as usize + 1,
            art_x: (f32::from(origin.x) - cover_x) / cover,
            art_y: (f32::from(origin.y) - cover_y) / cover,
            step: cell / cover,
        }
    }

    fn len(&self) -> usize {
        self.cols * self.rows
    }

    /// The art pixel at the middle of cell (`col`, `row`).
    fn art(&self, col: usize, row: usize) -> (f32, f32) {
        (
            self.art_x + (col as f32 + 0.5) * self.step,
            self.art_y + (row as f32 + 0.5) * self.step,
        )
    }

    /// The cell under art pixel (`x`, `y`), if it is on the grid.
    fn cell_at(&self, x: f32, y: f32) -> Option<usize> {
        let col = ((x - self.art_x) / self.step).floor();
        let row = ((y - self.art_y) / self.step).floor();
        (col >= 0. && row >= 0. && (col as usize) < self.cols && (row as usize) < self.rows)
            .then(|| row as usize * self.cols + col as usize)
    }

    /// The columns and rows of the cells within `radius` art pixels of (`x`, `y`), as a box.
    fn around(&self, x: f32, y: f32, radius: f32) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
        let span = |at: f32, start: f32, count: usize| {
            let from = ((at - radius - start) / self.step).floor().max(0.) as usize;
            let to = (((at + radius - start) / self.step).ceil().max(0.) as usize).min(count);
            from.min(to)..to
        };
        (span(x, self.art_x, self.cols), span(y, self.art_y, self.rows))
    }

    /// `field` at cell (`col`, `row`), the mean of four points in it: a thin line does not fall between the middles.
    fn mean(&self, col: usize, row: usize, field: impl Fn(f32, f32) -> f32) -> f32 {
        let (x, y) = self.art(col, row);
        let q = self.step / 4.;
        (field(x - q, y - q) + field(x + q, y - q) + field(x - q, y + q) + field(x + q, y + q)) / 4.
    }
}

/// `gen.py`'s coordinates: across and down from the art's middle, in art heights.
fn uv(x: f32, y: f32) -> (f32, f32) {
    ((x - W / 2.) / H, (y - H / 2.) / H)
}

fn smoothstep(from: f32, to: f32, value: f32) -> f32 {
    let t = ((value - from) / (to - from)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// A number from 0 to 1 for `index`, the same every time.
fn unit(index: usize, salt: u32) -> f32 {
    let mut x = (index as u32).wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    x as f32 / u32::MAX as f32
}

/// `gen.py`'s grain: about a normal spread of 0.035, the same each frame.
fn grain(index: usize) -> f32 {
    let sum: f32 = (0..4).map(|salt| unit(index, salt + 11)).sum();
    (sum - 2.) * 3f32.sqrt() * 0.035
}

/// `gen.py`'s 8x8 ordered-dither thresholds.
fn bayer() -> &'static [f32; 64] {
    static BAYER: OnceLock<[f32; 64]> = OnceLock::new();
    BAYER.get_or_init(|| {
        let mut m = vec![vec![0u32, 2], vec![3, 1]];
        while m.len() < 8 {
            let n = m.len();
            let mut next = vec![vec![0; n * 2]; n * 2];
            for (y, row) in m.iter().enumerate() {
                for (x, &value) in row.iter().enumerate() {
                    next[y][x] = 4 * value;
                    next[y][x + n] = 4 * value + 2;
                    next[y + n][x] = 4 * value + 3;
                    next[y + n][x + n] = 4 * value + 1;
                }
            }
            m = next;
        }
        let mut out = [0.; 64];
        for (y, row) in m.iter().enumerate() {
            for (x, &value) in row.iter().enumerate() {
                out[y * 8 + x] = (value as f32 + 0.5) / 64.;
            }
        }
        out
    })
}

/// A seeded generator for `convergence.py`'s random threads.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, from: f32, to: f32) -> f32 {
        from + (to - from) * self.next()
    }

    fn normal(&mut self, spread: f32) -> f32 {
        let (a, b) = (self.next().max(1e-7), self.next());
        (-2. * a.ln()).sqrt() * (TAU * b).cos() * spread
    }
}

/// A star that twinkles: its cell, how bright it is at most, and where in its twinkle it starts.
struct Star {
    cell: usize,
    bright: f32,
    phase: f32,
}

/// Stars on the cells where `sky` is true, about one cell in `1 / chance`.
fn stars(grid: &Grid, chance: f32, sky: impl Fn(f32, f32) -> bool) -> Vec<Star> {
    let mut out = vec![];
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let cell = row * grid.cols + col;
            let (x, y) = grid.art(col, row);
            if unit(cell, 1) < chance && sky(x, y) {
                out.push(Star {
                    cell,
                    bright: 0.3 + 0.6 * unit(cell, 2),
                    phase: TAU * unit(cell, 3),
                });
            }
        }
    }
    out
}

/// One of `convergence.py`'s threads, for the light that runs along it.
#[derive(Clone, Copy)]
struct Thread {
    x0: f32,
    y0: f32,
    bend: f32,
    wobble: (f32, f32, f32),
    /// How long the light takes along it, and where it starts.
    period: f32,
    offset: f32,
}

/// `convergence.py`'s art cells across and down, and where the threads meet.
const GW: f32 = 600.;
const GH: f32 = 375.;
const FX: f32 = GW * 0.80;
const FY: f32 = GH * 0.50;

impl Thread {
    /// Where the thread is at `t` (0 its start, 1 the meeting point), in art pixels.
    fn at(&self, t: f32) -> (f32, f32) {
        let e = 1. - (1. - t).powf(self.bend);
        let (freq, phase, amp) = self.wobble;
        let wob = (t * PI * freq + phase).sin() * amp * GH * (1. - t);
        let x = self.x0 + (FX - self.x0) * t;
        let y = self.y0 + (FY - self.y0) * e + wob;
        (x * CELL + CELL / 2., y * CELL + CELL / 2.)
    }
}

/// The parts of an art that do not move, for one grid.
enum Fields {
    Eclipse {
        base: Vec<f32>,
        limb: Vec<f32>,
        corona: Vec<f32>,
        angle: Vec<f32>,
    },
    Lattice {
        base: Vec<f32>,
        lat_lines: Vec<f32>,
        lon: Vec<f32>,
        width: Vec<f32>,
        shade: Vec<f32>,
    },
    Orbits {
        base: Vec<f32>,
    },
    Convergence {
        base: Vec<f32>,
        threads: Vec<Thread>,
        /// The cells along the beam, with the beam's strength there and their place along it.
        beam: Vec<(usize, f32, f32)>,
    },
    Moire {
        cells: Vec<(usize, f32, f32, f32)>,
    },
    Horizon {
        base: Vec<f32>,
        floor: Vec<f32>,
        fade: Vec<f32>,
    },
}

/// What a frame starts from: an art's fixed parts on one grid.
struct Still {
    art: Art,
    grid: Grid,
    grain: Vec<f32>,
    fields: Fields,
    stars: Vec<Star>,
}

const ECLIPSE: (f32, f32, f32) = (0.55, 0.95, 0.95);
const LATTICE: (f32, f32, f32) = (0.42, 0.08, 0.62);
const ORBIT_SUN: (f32, f32) = (0.35, 0.25);
const ORBITS: [f32; 6] = [0.25, 0.42, 0.62, 0.86, 1.15, 1.5];
const HORIZON: f32 = H * 0.60;

fn per_cell(grid: &Grid, mut each: impl FnMut(usize, usize) -> f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(grid.len());
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            out.push(each(col, row));
        }
    }
    out
}

impl Still {
    fn of(art: Art, grid: Grid) -> Still {
        let grain = (0..grid.len()).map(grain).collect();
        let mut stars_of = vec![];
        let fields = match art {
            Art::Eclipse => {
                let (cx, cy, r0) = ECLIPSE;
                let radius = |x: f32, y: f32| {
                    let (u, v) = uv(x, y);
                    ((u - cx).powi(2) + (v - cy).powi(2)).sqrt()
                };
                stars_of = stars(&grid, 0.004, |x, y| radius(x, y) > r0);
                let base = per_cell(&grid, |col, row| {
                    let (x, y) = grid.art(col, row);
                    let r = radius(x, y);
                    let inside = if r < r0 {
                        (0.10 * (-(r0 - r) / 0.08).exp()).clamp(0., 1.)
                    } else {
                        0.
                    };
                    inside - if r < r0 - 0.01 { 0.2 } else { 0. }
                });
                let limb = per_cell(&grid, |col, row| {
                    grid.mean(col, row, |x, y| (-((radius(x, y) - r0) / 0.006).powi(2)).exp())
                });
                let corona = per_cell(&grid, |col, row| {
                    let (x, y) = grid.art(col, row);
                    let r = radius(x, y);
                    if r > r0 { (-(r - r0) / 0.22).exp() * 0.7 } else { 0. }
                });
                let angle = per_cell(&grid, |col, row| {
                    let (u, v) = uv(grid.art(col, row).0, grid.art(col, row).1);
                    (v - cy).atan2(u - cx)
                });
                Fields::Eclipse {
                    base,
                    limb,
                    corona,
                    angle,
                }
            }
            Art::Lattice => {
                let (cx, cy, r0) = LATTICE;
                // The sphere as `gen.py` turns and tilts it: latitude, longitude and light at an art pixel.
                let sphere = |x: f32, y: f32| {
                    let (u, v) = uv(x, y);
                    let (dx, dy) = ((u - cx) / r0, (v - cy) / r0);
                    let rr = dx * dx + dy * dy;
                    let z = (1. - rr).clamp(0., 1.).sqrt();
                    let (turn, tilt) = (0.5f32, 0.45f32);
                    let big_x = dx * turn.cos() + z * turn.sin();
                    let big_z = -dx * turn.sin() + z * turn.cos();
                    let big_y = dy * tilt.cos() - big_z * tilt.sin();
                    let big_z = dy * tilt.sin() + big_z * tilt.cos();
                    let lat = big_y.clamp(-1., 1.).asin();
                    let lon = big_x.atan2(big_z);
                    let shade = 0.25 + 0.75 * (-0.5 * dx - 0.6 * dy + 0.6 * z).clamp(0., 1.);
                    (rr, lat, lon, shade)
                };
                let base = per_cell(&grid, |col, row| {
                    let rim = grid.mean(col, row, |x, y| (-((sphere(x, y).0.sqrt() - 1.) / 0.01).powi(2)).exp());
                    let (x, y) = grid.art(col, row);
                    let (rr, _, _, shade) = sphere(x, y);
                    rim * 0.7 + if rr < 1. { 0.05 * shade } else { 0. }
                });
                let lat_lines = per_cell(&grid, |col, row| {
                    grid.mean(col, row, |x, y| {
                        let (rr, lat, _, _) = sphere(x, y);
                        if rr < 1. {
                            (-(lat * 9.).sin().powi(2) / 0.004).exp()
                        } else {
                            0.
                        }
                    })
                });
                let mut lon = Vec::with_capacity(grid.len());
                let mut width = Vec::with_capacity(grid.len());
                let mut shade = Vec::with_capacity(grid.len());
                for row in 0..grid.rows {
                    for col in 0..grid.cols {
                        let (x, y) = grid.art(col, row);
                        let (rr, lat, at, light) = sphere(x, y);
                        lon.push(at);
                        // A little wider than `gen.py`'s 0.004: a moving line between dot middles would flicker.
                        width.push(lat.cos().powi(2) / 0.006);
                        shade.push(if rr < 1. { light } else { 0. });
                    }
                }
                Fields::Lattice {
                    base,
                    lat_lines,
                    lon,
                    width,
                    shade,
                }
            }
            Art::Orbits => {
                let (cx, cy) = ORBIT_SUN;
                stars_of = stars(&grid, 0.003, |_, _| true);
                let base = per_cell(&grid, |col, row| {
                    let rings = grid.mean(col, row, |x, y| {
                        let (u, v) = uv(x, y);
                        ORBITS
                            .iter()
                            .enumerate()
                            .map(|(i, a)| {
                                let e = (((u - cx) / a).powi(2) + ((v - cy) / (a * 0.32)).powi(2)).sqrt();
                                (-((e - 1.) / (0.004 / a)).powi(2)).exp() * (0.9 - 0.1 * i as f32)
                            })
                            .sum()
                    });
                    let (u, v) = uv(grid.art(col, row).0, grid.art(col, row).1);
                    rings + (-((u - cx).powi(2) + (v - cy).powi(2)) / 0.05).exp() * 0.25
                });
                Fields::Orbits { base }
            }
            Art::Convergence => convergence(&grid),
            Art::Moire => {
                let mut cells = vec![];
                for row in 0..grid.rows {
                    for col in 0..grid.cols {
                        let (x, y) = grid.art(col, row);
                        let env = (-((x - W * 0.74).powi(2) + (y - H * 0.33).powi(2)) / (2. * (W * 0.33).powi(2)))
                            .exp()
                            * 1.1;
                        if env > 0.003 {
                            let k1 = ((x - W * 0.7).powi(2) + (y - H * 0.3).powi(2)).sqrt() / 9.;
                            let k2 = ((x - W * 0.78).powi(2) + (y - H * 0.36).powi(2)).sqrt() / 9.;
                            cells.push((row * grid.cols + col, k1, k2, env));
                        }
                    }
                }
                Fields::Moire { cells }
            }
            Art::Horizon => {
                let vx = W / 2.;
                stars_of = stars(&grid, 0.0035, |_, y| y < HORIZON);
                let base = per_cell(&grid, |col, row| {
                    grid.mean(col, row, |x, y| {
                        let off = (y - HORIZON).abs();
                        let glow = (-((y - HORIZON) / 3.5).powi(2)).exp()
                            + (-off / 90.).exp() * 0.3
                            + (-off / 300.).exp() * 0.12;
                        glow * (-((x - vx) / (W * 0.42)).powi(2)).exp()
                    })
                });
                // The floor's lines that run to the vanishing point (`gen.py`'s horizon grid): they stay put.
                let floor = per_cell(&grid, |col, row| {
                    grid.mean(col, row, |x, y| {
                        if y <= HORIZON {
                            return 0.;
                        }
                        let spacing = 4. + 296. * (y - HORIZON) / (H * 1.6 - HORIZON);
                        let k = ((x - vx) / spacing).round().clamp(-40., 40.);
                        let d = (x - vx - k * spacing).abs();
                        0.86 * (-(d / 2.4).powi(2)).exp()
                    })
                });
                let fade = per_cell(&grid, |col, row| {
                    let y = grid.art(col, row).1;
                    if y > HORIZON {
                        ((y - HORIZON) / (H * 0.25)).clamp(0., 1.).powf(1.2)
                    } else {
                        0.
                    }
                });
                Fields::Horizon { base, floor, fade }
            }
        };
        Still {
            art,
            grid,
            grain,
            fields,
            stars: stars_of,
        }
    }

    /// The field at `seconds`, one value per cell.
    fn field(&self, seconds: f32) -> Vec<f32> {
        let grid = &self.grid;
        let t = seconds;
        let mut field = match &self.fields {
            Fields::Eclipse {
                base,
                limb,
                corona,
                angle,
            } => {
                // The light behind the planet swings to and fro along its edge; the corona's rays turn slowly.
                let sway = 2.2 + 0.55 * (TAU * t / 22.).sin();
                let mut out = base.clone();
                for (i, value) in out.iter_mut().enumerate() {
                    if limb[i] < 1e-3 && corona[i] < 1e-3 {
                        continue;
                    }
                    let d = (angle[i] + sway).cos().clamp(0., 1.);
                    let dirf = d * d.sqrt();
                    let rays = 1. + 0.22 * (angle[i] * 11. - 0.35 * t).sin();
                    *value += limb[i] * dirf * 1.2 + corona[i] * (0.3 + 0.7 * dirf) * rays;
                }
                out
            }
            Fields::Lattice {
                base,
                lat_lines,
                lon,
                width,
                shade,
            } => {
                // One turn in two minutes: a line of longitude passes every 13 seconds.
                let turn = TAU * t / 120.;
                let mut out = base.clone();
                for (i, value) in out.iter_mut().enumerate() {
                    if shade[i] <= 0. {
                        continue;
                    }
                    let s = ((lon[i] - turn) * 9.).sin();
                    let lon_line = (-s * s * width[i]).exp();
                    *value += lat_lines[i].max(lon_line) * shade[i] * 0.95;
                }
                out
            }
            Fields::Orbits { base } => {
                let mut out = base.clone();
                let (cx, cy) = ORBIT_SUN;
                let mut blob = |u0: f32, v0: f32, spread: f32, bright: f32| {
                    let (x0, y0) = (W / 2. + u0 * H, H / 2. + v0 * H);
                    let (cols, rows) = grid.around(x0, y0, 3. * spread.sqrt() * H);
                    for row in rows {
                        for col in cols.clone() {
                            let (u, v) = uv(grid.art(col, row).0, grid.art(col, row).1);
                            out[row * grid.cols + col] +=
                                bright * (-((u - u0).powi(2) + (v - v0).powi(2)) / spread).exp();
                        }
                    }
                };
                blob(cx, cy, 0.004, 1. + 0.15 * (TAU * t / 6.).sin());
                for (i, a) in ORBITS.iter().enumerate() {
                    // Kepler's third law: the innermost planet goes round in 30 seconds.
                    let period = 30. * (a / ORBITS[0]).powf(1.5);
                    let th = i as f32 * 1.3 + 0.6 + TAU * t / period;
                    blob(
                        cx + a * th.cos(),
                        cy + a * 0.32 * th.sin(),
                        0.0004 + 0.0002 * i as f32,
                        1.2,
                    );
                }
                out
            }
            Fields::Convergence { base, threads, beam } => {
                let mut out = base.clone();
                let mut light = vec![0f32; grid.len()];
                for thread in threads {
                    let head = (t / thread.period + thread.offset).fract();
                    for j in 0..80 {
                        let at = head - j as f32 * 0.0014;
                        if at < 0. {
                            break;
                        }
                        let (x, y) = thread.at(at);
                        if let Some(cell) = grid.cell_at(x, y) {
                            // Brighter than the threads' own left fade, so the light shows from the start.
                            let fade = (0.45 + (x / CELL) / (GW * 0.45)).clamp(0., 1.);
                            let glow = (-(j as f32) / 22.).exp() * fade;
                            light[cell] = light[cell].max(glow);
                        }
                    }
                }
                for &(cell, strength, along) in beam {
                    let wave = ((along - 30. * t) / 5.).sin().max(0.).powi(6);
                    light[cell] = light[cell].max(0.35 * strength * wave);
                }
                for (value, light) in out.iter_mut().zip(light) {
                    *value = (*value + light).min(1.);
                }
                out
            }
            Fields::Moire { cells } => {
                let mut out = vec![0f32; grid.len()];
                for &(cell, k1, k2, env) in cells {
                    out[cell] = (0.5 + 0.5 * (k1 - 0.8 * t).cos()) * (0.5 + 0.5 * (k2 + 0.55 * t).cos()) * env;
                }
                out
            }
            Fields::Horizon { base, floor, fade } => {
                let mut out = base.clone();
                let breath = 1. + 0.08 * (TAU * t / 7.).sin();
                // The floor's cross lines come toward the viewer, one spacing every 2.5 seconds; a new one fades in at
                // the horizon.
                let shift = (t / 2.5).fract();
                for row in 0..grid.rows {
                    let y = grid.art(0, row).1;
                    let across = if y > HORIZON + 0.5 {
                        let depth = 660. / (y - HORIZON) - 0.1;
                        let slope = 660. / (y - HORIZON).powi(2);
                        let k = depth / 0.9 + shift;
                        let n = k.round();
                        let line = (n - shift) * 0.9;
                        let dy = (k - n).abs() * 0.9 / slope;
                        0.9 * (-(dy / 3.2).powi(2)).exp() * smoothstep(10.8, 9.6, line)
                    } else {
                        0.
                    };
                    for col in 0..grid.cols {
                        let i = row * grid.cols + col;
                        out[i] = out[i] * breath + floor[i].max(across) * fade[i] * 0.9;
                    }
                }
                out
            }
        };
        for star in &self.stars {
            field[star.cell] += star.bright * (0.55 + 0.45 * (1.3 * t + star.phase).sin());
        }
        field
    }
}

/// `convergence.py`'s threads, drawn on `grid`; a few of them carry light.
fn convergence(grid: &Grid) -> Fields {
    let mut seeded = Seeded(11);
    let mut acc = vec![0f32; grid.len()];
    let mut last = vec![u32::MAX; grid.len()];
    let mut threads = vec![];
    for i in 0..420u32 {
        let x0 = seeded.range(-0.10, 0.30) * GW;
        let y0 = FY + seeded.normal(0.55).clamp(-1.4, 1.4) * GH;
        let bend = seeded.range(2.4, 3.4);
        let wobble = (seeded.range(1., 3.), seeded.range(0., 6.3), seeded.normal(0.012));
        let thread = Thread {
            x0,
            y0,
            bend,
            wobble,
            period: 6. + 5. * seeded.next(),
            offset: seeded.next(),
        };
        for step in 0..2400 {
            let (x, y) = thread.at(step as f32 / 2399.);
            if let Some(cell) = grid.cell_at(x, y)
                && last[cell] != i
            {
                last[cell] = i;
                acc[cell] += 0.55 + 0.45 * ((x / CELL) / GW).clamp(0., 1.);
            }
        }
        if i % 4 == 0 {
            threads.push(thread);
        }
    }
    let mut beam = vec![];
    let base = per_cell(grid, |col, row| {
        let (x, y) = grid.art(col, row);
        let (xx, yy) = (x / CELL - 0.5, y / CELL - 0.5);
        let thr = 1. - (-acc[row * grid.cols + col] * 0.9).exp();
        let right = 1. / (1. + (-(xx - FX) / 3.).clamp(-50., 50.).exp());
        let line = if (yy - FY).abs() < 0.6 { right } else { 0. };
        let beam_glow = (-((yy - FY) / 4.).powi(2)).exp() * right * 0.35;
        let glow = (-(((xx - FX) / 55.).powi(2) + ((yy - FY) / 22.).powi(2))).exp() * 0.6;
        if right > 0.05 && (yy - FY).abs() < 3. {
            beam.push((row * grid.cols + col, (line + beam_glow).min(1.), xx));
        }
        (thr * 0.75 + line + beam_glow + glow).clamp(0., 1.) * (0.1 + xx / (GW * 0.45)).clamp(0., 1.)
    });
    Fields::Convergence { base, threads, beam }
}

/// One lit dot: its cell and how bright it is (0 the background, 255 the ink).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Dot {
    col: u16,
    row: u16,
    level: u8,
}

/// One frame of a moving art, ready to paint.
pub(super) struct Frame {
    pub art: Art,
    pub layout: Layout,
    /// The background and ink colors it was tinted with.
    pub tint: ([u8; 4], [u8; 4]),
    grid: Grid,
    /// Row by row, so a strip of rows is a slice.
    dots: Vec<Dot>,
    /// Each level's color, from the background to the ink.
    colors: Vec<Hsla>,
    /// The frame blurred, for the frosted panels, and where it sits.
    pub frost: Arc<RenderImage>,
    pub frost_bounds: Bounds<Pixels>,
}

/// The fixed parts of the art on screen now. A new layout or art builds them again.
fn still(art: Art, grid: Grid) -> Arc<Still> {
    static STILL: OnceLock<Mutex<Option<Arc<Still>>>> = OnceLock::new();
    let mut kept = STILL
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match kept.as_ref() {
        Some(still) if still.art == art && still.grid == grid => still.clone(),
        _ => {
            let still = Arc::new(Still::of(art, grid));
            *kept = Some(still.clone());
            still
        }
    }
}

/// Build the frame of `art` at `seconds` on its clock, tinted from `back` to `ink`. Run it off the UI thread.
pub(super) fn frame(art: Art, layout: Layout, seconds: f32, back: [u8; 4], ink: [u8; 4]) -> Frame {
    let grid = Grid::of(layout);
    let still = still(art, grid);
    let field = still.field(seconds);
    let thresholds = bayer();
    let (bw, bh) = (grid.cols.div_ceil(2), grid.rows.div_ceil(2));
    let mut blocks = vec![0f32; bw * bh];
    let mut dots = vec![];
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let i = row * grid.cols + col;
            let value = (field[i] + still.grain[i]).clamp(0., 1.);
            let threshold = thresholds[(row % 8) * 8 + col % 8];
            let level = smoothstep(threshold - 0.045, threshold + 0.045, value) * (0.35 + 0.65 * value);
            if level > 0.02 {
                dots.push(Dot {
                    col: col as u16,
                    row: row as u16,
                    level: (level * 255.).round() as u8,
                });
                blocks[(row / 2) * bw + col / 2] += level;
            }
        }
    }
    let area = (grid.dot / grid.cell).powi(2) / 4.;
    for block in &mut blocks {
        *block *= area;
    }
    blur(&mut blocks, bw, bh, 3);
    // A clear ground (a see-through window, `frost.rs`) leaves the art's ground clear (`backdrop::mix`).
    let mix = |level: f32| super::backdrop::mix(back, ink, level);
    let mut pixels = Vec::with_capacity(bw * bh * 4);
    for block in &blocks {
        pixels.extend_from_slice(&mix(block.clamp(0., 1.)));
    }
    let buffer = RgbaImage::from_raw(bw as u32, bh as u32, pixels).unwrap_or_else(|| RgbaImage::new(1, 1));
    let colors = (0..=255u8)
        .map(|level| {
            let [b, g, r, a] = mix(f32::from(level) / 255.).map(|channel| f32::from(channel) / 255.);
            Hsla::from(Rgba { r, g, b, a })
        })
        .collect();
    Frame {
        art,
        layout,
        tint: (back, ink),
        grid,
        dots,
        colors,
        frost: Arc::new(RenderImage::new(SmallVec::from_elem(image::Frame::new(buffer), 1))),
        frost_bounds: Bounds::new(
            grid.origin,
            size(px(bw as f32 * 2. * grid.cell), px(bh as f32 * 2. * grid.cell)),
        ),
    }
}

/// Three passes of a box blur `radius` wide each way: near a gaussian, as `backdrop::build`'s frosted copy.
fn blur(values: &mut [f32], width: usize, height: usize, radius: usize) {
    let mut scratch = vec![0f32; values.len()];
    for _ in 0..3 {
        for (line, out) in values.chunks(width).zip(scratch.chunks_mut(width)) {
            box_line(line, out, radius);
        }
        for col in 0..width {
            let line: Vec<f32> = (0..height).map(|row| scratch[row * width + col]).collect();
            let mut out = vec![0f32; height];
            box_line(&line, &mut out, radius);
            for (row, value) in out.into_iter().enumerate() {
                values[row * width + col] = value;
            }
        }
    }
}

fn box_line(line: &[f32], out: &mut [f32], radius: usize) {
    let len = line.len();
    for (i, slot) in out.iter_mut().enumerate() {
        let (from, to) = (i.saturating_sub(radius), (i + radius + 1).min(len));
        *slot = line[from..to].iter().sum::<f32>() / (to - from) as f32;
    }
}

impl Frame {
    /// Paint the dots between `top` and `bottom`, inside the current content mask.
    pub fn paint(&self, top: Pixels, bottom: Pixels, window: &mut Window) {
        let grid = &self.grid;
        let row_of = |y: Pixels| ((f32::from(y) - f32::from(grid.origin.y)) / grid.cell).floor().max(0.) as u16;
        let (first, last) = (row_of(top), row_of(bottom).saturating_add(1));
        let from = self.dots.partition_point(|dot| dot.row < first);
        let to = self.dots.partition_point(|dot| dot.row <= last);
        let side = size(px(grid.dot), px(grid.dot));
        for dot in &self.dots[from..to] {
            let at = point(
                grid.origin.x + px(f32::from(dot.col) * grid.cell),
                grid.origin.y + px(f32::from(dot.row) * grid.cell),
            );
            window.paint_quad(fill(Bounds::new(at, side), self.colors[usize::from(dot.level)]));
        }
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    fn layout() -> Layout {
        Layout {
            room: Bounds::new(point(px(220.), px(0.)), size(px(1060.), px(868.))),
            scale: 2.,
        }
    }

    #[test]
    fn a_cell_is_a_whole_number_of_device_pixels_and_the_grid_covers_the_room() {
        for scale in [1., 1.5, 2.] {
            let layout = Layout { scale, ..layout() };
            let grid = Grid::of(layout);
            assert_eq!((grid.cell * scale).fract(), 0.);
            assert_eq!((grid.dot * scale).fract(), 0.);
            assert!(grid.dot < grid.cell);
            assert!(grid.cols as f32 * grid.cell >= f32::from(layout.room.size.width));
            assert!(grid.rows as f32 * grid.cell >= f32::from(layout.room.size.height));
        }
    }

    #[test]
    fn each_art_draws_some_dots_and_moves() {
        let (back, ink) = ([24, 24, 26, 255], [236, 236, 236, 255]);
        for art in Art::ALL {
            let first = frame(art, layout(), 0., back, ink);
            let cells = first.grid.len();
            let lit = first.dots.len() as f32 / cells as f32;
            assert!((0.02..0.35).contains(&lit), "{art:?} lights {lit} of its cells");
            let later = frame(art, layout(), 4., back, ink);
            let changed =
                first.dots.iter().filter(|dot| !later.dots.contains(dot)).count() as f32 / first.dots.len() as f32;
            assert!(changed > 0.01, "{art:?} changed {changed} of its dots in 4 seconds");
            assert!(
                first
                    .dots
                    .windows(2)
                    .all(|pair| (pair[0].row, pair[0].col) < (pair[1].row, pair[1].col))
            );
        }
    }

    #[test]
    fn a_frame_is_the_same_each_time() {
        let (back, ink) = ([24, 24, 26, 255], [236, 236, 236, 255]);
        let a = frame(Art::Orbits, layout(), 7.5, back, ink);
        let b = frame(Art::Orbits, layout(), 7.5, back, ink);
        assert_eq!(a.dots, b.dots);
    }

    #[test]
    fn the_planets_go_round() {
        // The innermost planet is a bright spot on its ring; in a quarter of its 30-second year it moves a long way.
        let (back, ink) = ([24, 24, 26, 255], [236, 236, 236, 255]);
        let near = |seconds: f32| {
            let frame = frame(Art::Orbits, layout(), seconds, back, ink);
            let (cx, cy) = ORBIT_SUN;
            let th = 0.6 + TAU * seconds / 30.;
            let (x, y) = (
                W / 2. + (cx + 0.25 * th.cos()) * H,
                H / 2. + (cy + 0.25 * 0.32 * th.sin()) * H,
            );
            let cell = frame.grid.cell_at(x, y).expect("on screen");
            let (col, row) = ((cell % frame.grid.cols) as u16, (cell / frame.grid.cols) as u16);
            frame
                .dots
                .iter()
                .filter(|dot| dot.col.abs_diff(col) <= 3 && dot.row.abs_diff(row) <= 3)
                .count()
        };
        // Lit dots where the planet should be, at both times.
        assert!(near(0.) >= 8 && near(7.5) >= 8);
    }

    #[test]
    fn the_dither_thresholds_are_gen_pys() {
        let thresholds = bayer();
        let mut sorted: Vec<u32> = thresholds.iter().map(|t| (t * 64. - 0.5).round() as u32).collect();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..64).collect::<Vec<_>>());
        assert_eq!(thresholds[0], 0.5 / 64.);
        assert_eq!(thresholds[1], 32.5 / 64.);
    }
}
