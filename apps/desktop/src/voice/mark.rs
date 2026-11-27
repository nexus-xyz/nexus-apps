//! The mark is the voice: the app mark in one ink, its light carrying the state, and a quiet ring of 64 level ticks
//! while Nexus speaks and while it hears you. `motion.rs` holds the maths, as Nexus UI's `voice-mark.tsx` draws it.
//!
//! A view of its own, so its frames redraw only the mark (AGENTS.md, render rules 4 to 6). It moves at most 30 times a
//! second, and only while it has a reason to: a still state (needs you, muted, can't hear you, a poor connection, ended)
//! stops once its 240 ms cross-fade and the turn's ease to rest are done. Under Reduce motion the geometry is still and
//! only the light follows the voice, smoothed over 1 s; Ready and Thinking stand still.
//!
//! The layers' hairlines are capped at 1.6 px, so the mark at 120 px is lighter than the app mark.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::call::VoiceState;
use super::motion::{
    self, FRAME_MS, LAYERS, RING_ROOM, THINK_TURN, TICKS, Tick, cross_fade, fade, follow, layer_opacity, layer_scale,
    mark_opacity, mix, rest_angle, ring_opacity, ring_ticks,
};

/// The mark's size in the desktop voice pane.
pub const SIZE: f32 = 120.;
/// The ring's units: the mark is 100 across, so the ring's radius is 85 (`voice-mark.tsx`).
const RADIUS: f32 = 85.;
/// The ring's hairline, in its units.
const RING_STROKE: f32 = 1.2;
/// After Thinking, the mark eases forward to rest over 600 ms.
const REST_MS: f32 = 600.;

/// The app mark's layers with their hairlines capped for a mark `SIZE` px across, made once.
fn layers() -> &'static [Vec<u8>] {
    static LAYERS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    LAYERS.get_or_init(|| {
        crate::assets::mark_layers()
            .iter()
            .map(|layer| capped(std::str::from_utf8(layer).unwrap_or_default(), SIZE).into_bytes())
            .collect()
    })
}

/// One layer's SVG with each `stroke-width` capped at 1.6 px when drawn `size` px across: its viewBox's width and the
/// `scale()` of its group give the pixels per unit.
fn capped(svg: &str, size: f32) -> String {
    let number = |pattern: &str| {
        regex::Regex::new(pattern)
            .ok()
            .and_then(|re| re.captures(svg)?.get(1)?.as_str().parse::<f32>().ok())
    };
    let width = number(r#"viewBox="[-\d.]+ [-\d.]+ ([\d.]+)"#).unwrap_or(168.);
    let scale = number(r"scale\(([\d.]+)\)").unwrap_or(1.);
    let px_per_unit = size / width * scale;
    let Ok(stroke) = regex::Regex::new(r#"stroke-width="([\d.]+)""#) else {
        return svg.to_string();
    };
    stroke
        .replace_all(svg, |found: &regex::Captures| {
            let width: f32 = found[1].parse().unwrap_or(1.);
            format!("stroke-width=\"{:.3}\"", motion::stroke_width(width, px_per_unit))
        })
        .into_owned()
}

/// Whether a state's mark comes to rest: still once it has cross-faded. Under Reduce motion Ready and Thinking too.
fn rests(state: VoiceState, reduce: bool) -> bool {
    match state {
        VoiceState::NeedsYou
        | VoiceState::Muted
        | VoiceState::CantHear
        | VoiceState::PoorConnection
        | VoiceState::Ended => true,
        VoiceState::Ready | VoiceState::Thinking => reduce,
        VoiceState::Listening | VoiceState::Speaking | VoiceState::Interrupted => false,
    }
}

/// What one frame draws.
#[derive(Clone, Copy)]
struct Frame {
    /// Each layer's opacity and scale, rim first.
    layers: [(f32, f32); LAYERS],
    ring: f32,
    ticks: [Tick; TICKS],
    mark: f32,
    /// Degrees.
    angle: f32,
}

pub struct VoiceMark {
    /// The voice's level, 0 to 1, as the pane last set it. A spoken word's pulse fades here.
    level: Rc<Cell<f32>>,
    state: VoiceState,
    previous: VoiceState,
    /// The clock when the state changed, for the cross-fade.
    changed: f32,
    /// Ms the mark has moved.
    clock: f32,
    smooth: f32,
    angle: f32,
    resting: Option<(f32, f32, f32)>,
    last: Option<Instant>,
    frame: Frame,
    running: bool,
    _frames: Option<Task<()>>,
}

impl VoiceMark {
    pub fn new(level: Rc<Cell<f32>>, cx: &mut Context<Self>) -> VoiceMark {
        let mut mark = VoiceMark {
            level,
            state: VoiceState::Ready,
            previous: VoiceState::Ready,
            changed: 0.,
            clock: 0.,
            smooth: 0.,
            angle: 0.,
            resting: None,
            last: None,
            frame: Frame {
                layers: [(1., 1.); LAYERS],
                ring: 0.,
                ticks: [[(0., 0.); 2]; TICKS],
                mark: 1.,
                angle: 0.,
            },
            running: false,
            _frames: None,
        };
        mark.tick(cx.background_executor().now(), cx.reduce_motion());
        mark.wake(cx);
        mark
    }

    /// The call moved to `state`: the mark cross-fades to it.
    pub fn set_state(&mut self, state: VoiceState, cx: &mut Context<Self>) {
        if state == self.state {
            return;
        }
        self.previous = self.state;
        self.state = state;
        self.changed = self.clock;
        self.wake(cx);
        cx.notify();
    }

    /// Start the frames, unless they run.
    fn wake(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.running = true;
        self.last = Some(cx.background_executor().now());
        let interval = Duration::from_secs_f32(FRAME_MS / 1000.);
        self._frames = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(interval).await;
                let go = this.update(cx, |mark, cx| {
                    let moving = mark.tick(cx.background_executor().now(), cx.reduce_motion());
                    cx.notify();
                    mark.running = moving;
                    moving
                });
                if !go.unwrap_or(false) {
                    break;
                }
            }
        }));
    }

    /// Move the clock to `now` and work out the frame. Whether the mark still moves.
    fn tick(&mut self, now: Instant, reduce: bool) -> bool {
        let dt = self.last.map_or(FRAME_MS, |last| {
            now.saturating_duration_since(last).as_secs_f32() * 1000.
        });
        self.last = Some(now);
        self.clock += dt;
        let target = self.level.get();
        self.smooth = follow(self.smooth, target, dt, reduce);
        if self.state == VoiceState::Speaking && target > 0. {
            self.level.set(fade(target, dt));
        }
        let since = self.clock - self.changed;
        let t = cross_fade(since);
        let (state, previous, smooth, clock) = (self.state, self.previous, self.smooth, self.clock);
        let at = |f: &dyn Fn(VoiceState) -> f32| {
            if t >= 1. {
                f(state)
            } else {
                mix(f(previous), f(state), t)
            }
        };
        let layers = std::array::from_fn(|i| {
            (
                at(&|s| layer_opacity(s, i, smooth, since, clock, reduce)),
                at(&|s| layer_scale(s, i, smooth, since, clock, reduce)),
            )
        });
        let ring = at(&ring_opacity);
        // Only Thinking turns it, once in 14 s; after, it eases forward to the next sixth of a turn.
        if !reduce {
            if state == VoiceState::Thinking {
                self.resting = None;
                self.angle = (self.angle + 360. * dt / THINK_TURN) % 360.;
            } else if self.angle % 60. != 0. {
                let (from, to, at) = *self
                    .resting
                    .get_or_insert((self.angle, rest_angle(self.angle), self.clock));
                let k = ((self.clock - at) / REST_MS).min(1.);
                self.angle = if k >= 1. {
                    to % 360.
                } else {
                    mix(from, to, 1. - (1. - k).powi(3))
                };
                if k >= 1. {
                    self.resting = None;
                }
            }
        }
        self.frame = Frame {
            layers,
            ring,
            ticks: if ring > 0.001 {
                ring_ticks(RADIUS, smooth, clock, reduce)
            } else {
                self.frame.ticks
            },
            mark: at(&mark_opacity),
            angle: self.angle,
        };
        !(rests(state, reduce) && t >= 1. && self.angle % 60. == 0.)
    }

    /// Whether frames are running, for the tests.
    #[cfg(test)]
    pub fn moving(&self) -> bool {
        self.running
    }
}

impl Render for VoiceMark {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::pages::palette(cx);
        let room = SIZE * RING_ROOM;
        let inset = (room - SIZE) / 2.;
        let frame = self.frame;
        let ink = palette.strong;
        let ring_ink = palette.faint.opacity(frame.ring);
        let turn = radians(frame.angle.to_radians());
        div()
            .id("voice-mark")
            .relative()
            .flex_none()
            .size(px(room))
            .when(frame.ring > 0.001, |mark| {
                mark.child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let unit = f32::from(bounds.size.width) / (2. * RADIUS);
                            let center = bounds.center();
                            let mut path = PathBuilder::stroke(px(RING_STROKE * unit));
                            for [(ox, oy), (ix, iy)] in frame.ticks {
                                path.move_to(center + point(px(ox * unit), px(oy * unit)));
                                path.line_to(center + point(px(ix * unit), px(iy * unit)));
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, ring_ink);
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                )
            })
            .children(layers().iter().zip(frame.layers).map(|(layer, (opacity, scale))| {
                svg()
                    .data(layer)
                    .absolute()
                    .top(px(inset))
                    .left(px(inset))
                    .size(px(SIZE))
                    .text_color(ink.opacity(opacity * frame.mark))
                    .with_transformation(Transformation::rotate(turn).with_scaling(size(scale, scale)))
            }))
            // Needs you: a dot 6 px under the mark.
            .when(self.state == VoiceState::NeedsYou, |mark| {
                mark.child(
                    div()
                        .id("voice-mark-dot")
                        .debug_selector(|| "voice-mark-dot".into())
                        .absolute()
                        .top(px(room / 2. + 66.))
                        .left(px(room / 2. - 3.))
                        .size(px(6.))
                        .rounded_full()
                        .bg(ink),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Cell, Duration, LAYERS, Rc, VoiceMark, VoiceState, capped, layers};
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};

    #[test]
    fn the_hairlines_are_capped_at_1_6_px_at_120_px() {
        let svg = r#"<svg viewBox="-84 -84 168 168"><g transform="scale(0.8853)"><path stroke-width="3.83"/><path stroke-width="0.39"/></g></svg>"#;
        let px_per_unit = 120. / 168. * 0.8853;
        let out = capped(svg, 120.);
        assert!(
            out.contains(&format!("stroke-width=\"{:.3}\"", 1.6 / px_per_unit)),
            "{out}"
        );
        assert!(out.contains("stroke-width=\"0.390\""), "{out}");
        assert_eq!(layers().len(), LAYERS, "the app mark has eleven layers");
    }

    /// A window with one mark, and a count of the times it asked to be drawn again.
    fn mark(cx: &mut TestAppContext) -> (Entity<VoiceMark>, Rc<Cell<usize>>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
                crate::palette::ThemeChoice::Dark,
                gpui_kit::WindowAppearance::Dark,
            )));
        });
        let (mark, cx) = cx.add_window_view(|_, cx| VoiceMark::new(Rc::new(Cell::new(0.)), cx));
        let asks = Rc::new(Cell::new(0));
        let count = asks.clone();
        cx.update(|_, cx| cx.observe(&mark, move |_, _| count.set(count.get() + 1)).detach());
        (mark, asks, cx)
    }

    fn frames_over(asks: &Cell<usize>, span: Duration, cx: &mut VisualTestContext) -> usize {
        let before = asks.get();
        let step = Duration::from_millis(1);
        let mut elapsed = Duration::ZERO;
        while elapsed < span {
            cx.executor().advance_clock(step);
            cx.run_until_parked();
            elapsed += step;
        }
        asks.get() - before
    }

    #[gpui_kit::test]
    fn a_listening_mark_draws_thirty_frames_a_second_and_a_still_one_none(cx: &mut TestAppContext) {
        let (mark, asks, cx) = mark(cx);
        mark.update(cx, |mark, cx| mark.set_state(VoiceState::Listening, cx));
        let listening = frames_over(&asks, Duration::from_secs(1), cx);
        assert!((27..=31).contains(&listening), "{listening} frames in a second");

        // Needs you is still: once the cross-fade ends, nothing asks for a frame.
        mark.update(cx, |mark, cx| mark.set_state(VoiceState::NeedsYou, cx));
        frames_over(&asks, Duration::from_millis(400), cx);
        assert!(!mark.read_with(cx, |mark, _| mark.moving()));
        assert_eq!(frames_over(&asks, Duration::from_secs(1), cx), 0);
    }

    #[gpui_kit::test]
    fn under_reduce_motion_thinking_stands_still(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (mark, asks, cx) = mark(cx);
        mark.update(cx, |mark, cx| mark.set_state(VoiceState::Thinking, cx));
        frames_over(&asks, Duration::from_millis(400), cx);
        assert_eq!(frames_over(&asks, Duration::from_secs(1), cx), 0);
        let frame = mark.read_with(cx, |mark, _| mark.frame);
        assert!(
            frame
                .layers
                .iter()
                .all(|(opacity, scale)| (*opacity - 0.6).abs() < 1e-3 && *scale == 1.)
        );
    }
}
