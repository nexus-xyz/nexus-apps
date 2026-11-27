//! Nexus: a mouse wheel's notch glides over the next frames instead of jumping the whole notch (60px in a list) in one
//! frame. Upstream GPUI applies every scroll at once.
//!
//! The glide is Chromium's wheel animation (`cc::ScrollOffsetAnimationCurve` for `kMouseWheel`): an ease-in-out,
//! cubic-bezier(0.42, 0, 0.58, 1), over 200ms, shortening to 100ms for long moves. A notch that comes while the glide
//! moves pushes the target on and starts a new leg at the glide's current speed (`UpdateTarget`), so a fast spin is
//! one continuous glide, with no lurch at each notch. A notch the other way drops what was left on its axis and starts
//! from rest. Chromium times a leg by its length in its pixels, 40 to a wheel line. A notch here is 3 lines of the
//! container's line height (20px in a list), so a leg is timed on its length in lines times 40: the same click takes
//! as long as in Chromium.
//!
//! Only whole wheel notches ([`crate::ScrollDelta::Lines`]) glide. Pixel deltas come from touchpads and other precise
//! devices that already move with the fingers, and pass through as before. Outside macOS, fractional lines do too:
//! X11 and Wayland high-resolution wheels and legacy Windows touchpads stream small fractions that are smooth already.
//! On X11 every scroll is in lines, so the platform also flags a touchpad's ([`scroll_event_continuous`]). A notch with
//! Ctrl, Alt, Cmd or Fn held stays a step (zoom and other gestures), and so does every notch under reduced motion
//! ([`crate::App::reduce_motion`]).
//!
//! The two scroll containers (`List` and a `div` with overflow scroll) hold one of these, step it once per frame from
//! `Window::on_next_frame` (which runs before that frame's draw), and ask for the next frame only while it moves, so a
//! window at rest draws nothing. A scroll the code makes (to an item, to the end, the scrollbar), a scroll that does
//! not glide, and a mouse press all end the glide.

use crate::{Modifiers, Pixels, Point, ScrollDelta, ScrollEvent, ScrollSource, point, px};
use std::time::Instant;

/// Chromium's wheel curve, cubic-bezier(0.42, 0, 0.58, 1) (`kEaseInOutControlPoints`).
const EASE_IN_OUT: Curve = Curve::new(0.42, 0., 0.58, 1.);
/// Chromium's pixels in a wheel line (`kScrollbarPixelsPerCocoaTick`), the unit it times legs in.
const CHROMIUM_LINE: f64 = 40.;
/// Chromium's `kEpsilon`: 0.01 of its pixels, for distances and speeds.
const CHROMIUM_EPSILON: f64 = 0.01;
/// Steps are multiples of this many pixels, so sums of steps are exact in f32 and a glide's steps add up to exactly
/// its notches. A 256th of a pixel is far below what a screen shows.
const GRID: f32 = 1. / 256.;

thread_local! {
    static SCROLL_EVENT_CONTINUOUS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Tests set the clock, as their frames come microseconds apart.
    #[cfg(test)]
    static CLOCK: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Nexus: whether the scroll event being dispatched came from a device that scrolls continuously (a touchpad), though
/// its delta is in lines. X11 reports every scroll in lines; its client sets this for touchpads (`x11/client.rs`).
pub fn scroll_event_continuous() -> bool {
    SCROLL_EVENT_CONTINUOUS.get()
}

/// Nexus: for platforms, around dispatching a scroll event.
#[doc(hidden)]
pub fn set_scroll_event_continuous(continuous: bool) {
    SCROLL_EVENT_CONTINUOUS.set(continuous);
}

/// The time of a notch or a frame.
pub(crate) fn now() -> Instant {
    #[cfg(test)]
    if let Some(now) = CLOCK.get() {
        return now;
    }
    Instant::now()
}

#[cfg(test)]
pub(crate) fn set_test_clock(now: Option<Instant>) {
    CLOCK.set(now);
}

/// Whether this scroll should glide: a wheel's notch, with motion allowed.
pub(crate) fn animates(event: &ScrollEvent, reduce_motion: bool) -> bool {
    !reduce_motion
        && event.source == ScrollSource::Pointer
        && match event.delta {
            // macOS flags touchpads precise, so any line delta there is a wheel (slow notches from some mice come as
            // fractions). Elsewhere a notch is whole lines, and fractions are high-resolution wheels and touchpads.
            ScrollDelta::Lines(lines) => {
                cfg!(target_os = "macos") || (lines.x.fract() == 0. && lines.y.fract() == 0.)
            }
            ScrollDelta::Pixels(_) => false,
        }
        && !commanded(&event.modifiers)
        && !scroll_event_continuous()
}

/// Whether this scroll ends a glide in flight: any that does not glide, but an empty notch.
pub(crate) fn ends_glide(event: &ScrollEvent, reduce_motion: bool) -> bool {
    !animates(event, reduce_motion)
        && !matches!(event.delta, ScrollDelta::Lines(lines) if lines == Point::default())
}

/// A modifier that makes a wheel gesture something other than a scroll. Shift only picks the axis.
fn commanded(modifiers: &Modifiers) -> bool {
    modifiers.control || modifiers.alt || modifiers.platform || modifiers.function
}

/// The wheel glide of one scroll container.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WheelScroll {
    /// Pixels the glide still owes the container, per axis.
    owed: Point<f32>,
    /// The leg in flight; `None` at rest.
    leg: Option<Leg>,
    /// Chromium's pixels in one of this container's pixels: 40 over its line height.
    scale: f64,
    /// A frame callback is already waiting to step this, so another is not needed.
    frame_scheduled: bool,
}

impl Default for WheelScroll {
    fn default() -> Self {
        Self {
            owed: Point::default(),
            leg: None,
            scale: CHROMIUM_LINE / 20.,
            frame_scheduled: false,
        }
    }
}

impl WheelScroll {
    /// Whether a notch is still moving.
    pub(crate) fn is_active(&self) -> bool {
        self.leg.is_some()
    }

    /// Adds a notch of `delta` pixels at `now`, in a container whose wheel line is `line` pixels. The target moves on
    /// by it, and a glide in flight keeps its speed into the new leg. Along an axis the notch reverses, what was left
    /// is dropped and that axis starts from rest.
    pub(crate) fn add(&mut self, delta: Point<Pixels>, line: Pixels, now: Instant) {
        if line.0 > 0. {
            self.scale = CHROMIUM_LINE / f64::from(line.0);
        }
        let delta = point(delta.x.0, delta.y.0);
        let Some(leg) = self.leg.take() else {
            if delta != Point::default() {
                self.owed = delta;
                self.leg = Some(Leg::from_rest(delta, now, self.scale));
            }
            return;
        };
        let left = leg.left(now);
        let reverses = |heading: f32, delta: f32| heading * delta < 0.;
        let reversed = (
            reverses(leg.travel.x, delta.x),
            reverses(leg.travel.y, delta.y),
        );
        let axis = |reversed: bool, owed: &mut f32, left: f32, delta: f32| {
            if reversed {
                *owed = delta;
                delta
            } else {
                *owed += delta;
                left + delta
            }
        };
        let travel = point(
            axis(reversed.0, &mut self.owed.x, left.x, delta.x),
            axis(reversed.1, &mut self.owed.y, left.y, delta.y),
        );
        // Chromium nets a notch against the glide into a target clamped to the scroller's bounds. This can't see the
        // bounds, and netted, the notch could be spent on distance past an end, so it turns from rest instead.
        self.leg = Some(if reversed.0 || reversed.1 {
            Leg::from_rest(travel, now, self.scale)
        } else {
            leg.retarget(travel, now, self.scale)
        });
    }

    /// The distance to scroll by in the frame at `now`: where the leg is, less what was given out, on the grid. Once
    /// the leg is over, all that is owed, which ends it.
    pub(crate) fn step(&mut self, now: Instant) -> Point<Pixels> {
        let Some(leg) = &self.leg else {
            return Point::default();
        };
        let step = if leg.progress(now) >= 1. {
            let owed = self.owed;
            self.cancel();
            owed
        } else {
            let left = leg.left(now);
            let snap = |pixels: f32| (pixels / GRID).round() * GRID;
            let step = point(snap(self.owed.x - left.x), snap(self.owed.y - left.y));
            self.owed = point(self.owed.x - step.x, self.owed.y - step.y);
            step
        };
        point(px(step.x), px(step.y))
    }

    /// Drops what is left: something else scrolled the container.
    pub(crate) fn cancel(&mut self) {
        self.owed = Point::default();
        self.leg = None;
    }

    /// Whether the caller should ask for a frame to step this: it moves and no frame is asked for yet. Marks one as
    /// asked for.
    pub(crate) fn needs_frame(&mut self) -> bool {
        if self.leg.is_some() && !self.frame_scheduled {
            self.frame_scheduled = true;
            true
        } else {
            false
        }
    }

    /// The frame asked for came.
    pub(crate) fn frame_came(&mut self) {
        self.frame_scheduled = false;
    }
}

/// The larger axis, with its sign (Chromium's `MaximumDimension`).
fn longer_axis(travel: Point<f32>) -> f64 {
    f64::from(if travel.x.abs() > travel.y.abs() {
        travel.x
    } else {
        travel.y
    })
}

/// One leg of a glide: `travel` pixels from where it started at `start`, in `seconds`, along `curve`.
#[derive(Clone, Copy, Debug)]
struct Leg {
    travel: Point<f32>,
    start: Instant,
    seconds: f64,
    curve: Curve,
}

impl Leg {
    /// A leg from rest: Chromium's `kInverseDelta` time, 12 frames at 60Hz (200ms) up to 120 of its pixels, ramping
    /// down to 6 frames (100ms) at 480, so a fast spin keeps up.
    fn from_rest(travel: Point<f32>, start: Instant, scale: f64) -> Self {
        let chromium_pixels = longer_axis(travel).abs() * scale;
        let frames = 12. - (chromium_pixels - 120.) / (480. - 120.) * 6.;
        Self {
            travel,
            start,
            seconds: frames.clamp(6., 12.) / 60.,
            curve: EASE_IN_OUT,
        }
    }

    /// Where along its time the leg is at `now`, from 0 to 1.
    fn progress(&self, now: Instant) -> f64 {
        if self.seconds <= 0. {
            return 1.;
        }
        (now.saturating_duration_since(self.start).as_secs_f64() / self.seconds).min(1.)
    }

    /// The pixels the leg still has to travel after `now`.
    fn left(&self, now: Instant) -> Point<f32> {
        let share = 1. - self.curve.y_at(self.progress(now));
        point(
            (f64::from(self.travel.x) * share) as f32,
            (f64::from(self.travel.y) * share) as f32,
        )
    }

    /// Pixels a second along the larger axis at `now` (Chromium's `CalculateVelocity`).
    fn speed(&self, now: Instant) -> f64 {
        if self.seconds <= 0. {
            return 0.;
        }
        self.curve.slope_at(self.progress(now)) * longer_axis(self.travel) / self.seconds
    }

    /// The leg that takes over from this one at `now` to travel `travel` (Chromium's `UpdateTarget`). It starts at this
    /// one's speed, and is cut short of a leg from rest's time when that speed would carry it there sooner.
    fn retarget(&self, travel: Point<f32>, now: Instant, scale: f64) -> Self {
        self.retarget_onto(Self::from_rest(travel, now, scale), scale)
    }

    /// [`Self::retarget`], onto `next`: the leg from rest that would travel the same, for its time and its curve.
    fn retarget_onto(&self, mut next: Self, scale: f64) -> Self {
        let distance = longer_axis(next.travel);
        let speed = self.speed(next.start);
        // `VelocityBasedDurationBound`: the time at this speed, with room for the ease-out. None when still or heading
        // away from the target.
        let bound = distance / speed * 2.5;
        if (speed * scale).abs() >= CHROMIUM_EPSILON && bound >= 0. {
            next.seconds = next.seconds.min(bound);
        }
        if (distance * scale).abs() < CHROMIUM_EPSILON || next.seconds < 0.01 {
            // There already, or nearly: the rest goes in the next frame.
            next.seconds = 0.;
            return next;
        }
        // `EaseInOutWithInitialSlope`: y1 set so the curve leaves 0 at this speed over the leg's average.
        let slope = (speed * next.seconds / distance).clamp(-1000., 1000.);
        next.curve.y1 = next.curve.x1 * slope;
        next
    }
}

/// A CSS cubic-bezier timing curve, from (0, 0) to (1, 1) through control points (x1, y1) and (x2, y2): y of x.
#[derive(Clone, Copy, Debug)]
struct Curve {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

impl Curve {
    const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Self { x1, y1, x2, y2 }
    }

    /// The Bernstein form of one coordinate at parameter `t`, with control values `a` and `b`.
    fn coordinate(a: f64, b: f64, t: f64) -> f64 {
        let u = 1. - t;
        3. * u * u * t * a + 3. * u * t * t * b + t * t * t
    }

    /// Its derivative in `t`.
    fn coordinate_rate(a: f64, b: f64, t: f64) -> f64 {
        let u = 1. - t;
        3. * u * u * a + 6. * u * t * (b - a) + 3. * t * t * (1. - b)
    }

    /// The parameter where the curve's x is `x`: Newton's method from `x`, else halving [0, 1].
    fn parameter_at(&self, x: f64) -> f64 {
        const TOLERANCE: f64 = 1e-7;
        let mut t = x;
        for _ in 0..8 {
            let error = Self::coordinate(self.x1, self.x2, t) - x;
            if error.abs() < TOLERANCE {
                return t;
            }
            let rate = Self::coordinate_rate(self.x1, self.x2, t);
            if rate.abs() < TOLERANCE {
                break;
            }
            t -= error / rate;
        }
        let (mut low, mut high) = (0., 1.);
        t = x;
        for _ in 0..64 {
            let error = Self::coordinate(self.x1, self.x2, t) - x;
            if error.abs() < TOLERANCE {
                break;
            }
            if error < 0. {
                low = t;
            } else {
                high = t;
            }
            t = (low + high) / 2.;
        }
        t
    }

    fn y_at(&self, x: f64) -> f64 {
        Self::coordinate(self.y1, self.y2, self.parameter_at(x.clamp(0., 1.)))
    }

    /// dy/dx at `x`.
    fn slope_at(&self, x: f64) -> f64 {
        let t = self.parameter_at(x.clamp(0., 1.));
        let dx = Self::coordinate_rate(self.x1, self.x2, t);
        let dy = Self::coordinate_rate(self.y1, self.y2, t);
        if dx == 0. && dy == 0. { 0. } else { dy / dx }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 120Hz.
    const FRAME: Duration = Duration::from_millis(8);
    /// A list's wheel line.
    const LINE: Pixels = px(20.);

    fn down(pixels: f32) -> Point<Pixels> {
        point(px(0.), px(-pixels))
    }

    fn ms(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    /// Every step of a glide to rest, one `frame` apart.
    fn land(wheel: &mut WheelScroll, now: &mut Instant, frame: Duration) -> Vec<Point<Pixels>> {
        let mut steps = Vec::new();
        while wheel.is_active() {
            *now += frame;
            steps.push(wheel.step(*now));
            assert!(steps.len() < 500, "the glide never landed");
        }
        steps
    }

    fn total(steps: &[Point<Pixels>]) -> Point<Pixels> {
        steps.iter().fold(Point::default(), |sum, step| sum + *step)
    }

    #[test]
    fn a_notch_eases_in_and_out_and_adds_up_to_its_distance() {
        let start = Instant::now();
        let mut now = start;
        let mut wheel = WheelScroll::default();
        wheel.add(down(60.), LINE, now);
        // The curve is symmetric: halfway through its 200ms, halfway there.
        let half = wheel.leg.as_ref().unwrap().left(ms(start, 100));
        assert!((half.y + 30.).abs() < 1e-3, "{half:?}");

        let steps = land(&mut wheel, &mut now, FRAME);
        assert_eq!(total(&steps), down(60.));
        assert_eq!(steps.len(), 25, "lands on the first frame from 200ms");
        // Slow, fast, slow: steps grow to the middle frame, then shrink (give or take the grid).
        let speeds: Vec<f32> = steps.iter().map(|step| -step.y.0).collect();
        let peak = (0..speeds.len())
            .max_by(|a, b| speeds[*a].total_cmp(&speeds[*b]))
            .unwrap();
        assert!((11..=13).contains(&peak), "fastest at frame {peak}");
        assert!(
            speeds[..=peak]
                .windows(2)
                .all(|pair| pair[1] >= pair[0] - GRID)
        );
        assert!(
            speeds[peak..]
                .windows(2)
                .all(|pair| pair[1] <= pair[0] + GRID)
        );
        assert!(speeds[0] > 0. && speeds[0] < speeds[peak] / 10.);
        assert!(!wheel.is_active());
        assert_eq!(wheel.step(now + FRAME), Point::default());
    }

    #[test]
    fn longer_moves_take_less_time_as_in_chromium() {
        // In lines: a notch is 3, Chromium's 120px. Its time does not depend on the container's line height.
        for (lines, millis) in [
            (1., 200.),
            (3., 200.),
            (7.5, 150.),
            (12., 100.),
            (40., 100.),
        ] {
            for line in [20., 17.5] {
                let leg = Leg::from_rest(
                    point(0., -lines * line),
                    Instant::now(),
                    CHROMIUM_LINE / line as f64,
                );
                let took = leg.seconds * 1000.;
                assert!(
                    (took - millis).abs() < 1e-9,
                    "{lines} lines of {line}px took {took}ms"
                );
            }
        }
    }

    #[test]
    fn a_retarget_matches_chromiums_own_numbers() {
        // cc's `EaseInOutUpdateTarget` test, on the curve it uses (desktop programmatic scroll, (0.4, 0, 0, 1)) and
        // its constant 150ms legs, so in Chromium's pixels and with the time from rest set to 150ms.
        let start = Instant::now();
        let curve = Curve::new(0.4, 0., 0., 1.);
        let first = Leg {
            travel: point(0., 3600.),
            start,
            seconds: 0.15,
            curve,
        };
        let left = first.left(ms(start, 75));
        assert!((3600. - left.y - 3101.4792).abs() < 0.002, "{left:?}");

        let next = Leg {
            travel: point(0., left.y + 6300.),
            start: ms(start, 75),
            ..first
        };
        let second = first.retarget_onto(next, 1.);
        assert!((second.seconds - 0.15).abs() < 0.0002);
        let position = |millis| 9900. - second.left(ms(start, millis)).y;
        assert!((position(150) - 9097.8).abs() < 0.1, "{}", position(150));
        assert!((position(225) - 9900.).abs() < 0.0002);
    }

    #[test]
    fn a_notch_mid_glide_keeps_its_speed() {
        let mut now = Instant::now();
        let mut wheel = WheelScroll::default();
        wheel.add(down(60.), LINE, now);
        let mut before = 0.;
        for _ in 0..6 {
            now += FRAME;
            before = wheel.step(now).y.0;
        }
        let speed = wheel.leg.as_ref().unwrap().speed(now);
        wheel.add(down(60.), LINE, now);
        let new_speed = wheel.leg.as_ref().unwrap().speed(now);
        assert!(
            (new_speed - speed).abs() < speed.abs() * 1e-6,
            "{new_speed} after {speed}"
        );
        now += FRAME;
        let after = wheel.step(now).y.0;
        // From rest it would slow to a fifth; an ease-out restarted on the rest jumped about 3 times.
        assert!(
            after / before > 0.8 && after / before < 1.5,
            "{after} after {before}"
        );
    }

    #[test]
    fn a_fast_spin_is_one_glide_with_no_lurch() {
        // A notch every 40ms, ten of them, at 120Hz.
        let start = Instant::now();
        let mut now = start;
        let mut wheel = WheelScroll::default();
        let mut steps = Vec::new();
        for notch in 0..10 {
            wheel.add(down(60.), LINE, now);
            let next = ms(start, 40 * (notch + 1));
            while now + FRAME <= next {
                now += FRAME;
                steps.push(wheel.step(now));
            }
        }
        steps.extend(land(&mut wheel, &mut now, FRAME));
        assert_eq!(total(&steps), down(600.));
        // Once it gets going, no frame moves more than half again the one before it.
        let speeds: Vec<f32> = steps.iter().map(|step| -step.y.0).collect();
        for pair in speeds[3..].windows(2) {
            assert!(pair[1] <= pair[0] * 1.5 + GRID, "lurch: {speeds:?}");
        }
    }

    #[test]
    fn notches_in_flight_add_up_exactly() {
        let mut now = Instant::now();
        let mut wheel = WheelScroll::default();
        // A div's notch: three of a fractional line height.
        let line = px(21.333_334);
        let notch = down(3. * line.0);
        wheel.add(notch, line, now);
        let mut delivered = Point::default();
        for _ in 0..3 {
            now += FRAME;
            delivered = delivered + wheel.step(now);
        }
        wheel.add(notch, line, now);
        delivered = delivered + total(&land(&mut wheel, &mut now, FRAME));
        assert_eq!(delivered, notch + notch);
    }

    #[test]
    fn a_notch_the_other_way_starts_from_rest() {
        let mut now = Instant::now();
        let mut wheel = WheelScroll::default();
        wheel.add(point(px(-20.), px(-60.)), LINE, now);
        for _ in 0..12 {
            now += FRAME;
            wheel.step(now);
        }
        let fast = -wheel.leg.as_ref().unwrap().speed(now);
        wheel.add(point(px(0.), px(60.)), LINE, now);
        assert_eq!(wheel.owed.y, 60.);
        assert_eq!(
            wheel.leg.as_ref().unwrap().speed(now),
            0.,
            "turns from rest"
        );
        now += FRAME;
        let first = wheel.step(now);
        assert!(first.y > px(0.) && f64::from(first.y.0) < fast * FRAME.as_secs_f64() / 4.);
        let rest = first + total(&land(&mut wheel, &mut now, FRAME));
        assert_eq!(rest.y, px(60.));
        assert!(rest.x < px(0.), "x keeps gliding left");
    }

    #[test]
    fn the_curve_does_not_depend_on_frame_rate() {
        let position = |frame: Duration, frames: u32| {
            let mut now = Instant::now();
            let mut wheel = WheelScroll::default();
            wheel.add(down(60.), LINE, now);
            (0..frames)
                .map(|_| {
                    now += frame;
                    wheel.step(now).y.0
                })
                .sum::<f32>()
        };
        let at_60hz = position(Duration::from_millis(16), 3);
        let at_120hz = position(FRAME, 6);
        assert!((at_60hz - at_120hz).abs() < 0.02, "{at_60hz} vs {at_120hz}");
    }

    /// A reference glide's position, in lines, at each 8ms frame of a spin of 5 notches of 3 lines down at 0, 37,
    /// 74, 111 and 148ms (a notch is taken before a frame due at the same time). Computed by a separate
    /// implementation of Chromium's wheel curve, timed on lines at 40px a line.
    #[rustfmt::skip]
    const REFERENCE_SPIN: &[f32] = &[
        -0.009277344, -0.037597656, -0.0859375, -0.15478516, -0.24633789, -0.37451172, -0.5449219,
        -0.75927734, -1.0180664, -1.3244629, -1.6877441, -2.1101074, -2.5915527, -3.130127, -3.71875,
        -4.3552246, -5.037842, -5.762207, -6.518799, -7.2946777, -8.085449, -8.885254, -9.686035,
        -10.476318, -11.241211, -11.963623, -12.625732, -13.2126465, -13.713623, -14.125, -14.447998,
        -14.688232, -14.854004, -14.954102, -14.99707, -15.0,
    ];

    #[test]
    fn a_fast_spin_matches_the_reference_glide() {
        let start = Instant::now();
        let notches = [0, 37, 74, 111, 148];
        let mut wheel = WheelScroll::default();
        let mut position = 0.;
        let mut next = 0;
        for (frame, expected) in REFERENCE_SPIN.iter().enumerate() {
            let now = ms(start, 8 * (frame as u64 + 1));
            while next < notches.len() && ms(start, notches[next]) < now {
                wheel.add(down(60.), LINE, ms(start, notches[next]));
                next += 1;
            }
            position += wheel.step(now).y.0;
            let expected = expected * LINE.0;
            assert!(
                (position - expected).abs() < 0.01,
                "frame {frame}: {position} vs {expected}"
            );
        }
        assert!(!wheel.is_active());
        assert_eq!(position, -300.);
    }

    #[test]
    fn cancel_drops_the_rest_and_frames_are_asked_for_once() {
        let start = Instant::now();
        let mut wheel = WheelScroll::default();
        assert!(!wheel.needs_frame(), "nothing moves, no frame");
        wheel.add(Point::default(), LINE, start);
        assert!(!wheel.needs_frame(), "an empty notch moves nothing");
        wheel.add(down(60.), LINE, start);
        assert!(wheel.needs_frame());
        assert!(!wheel.needs_frame(), "one frame is already asked for");
        wheel.frame_came();
        wheel.cancel();
        assert!(!wheel.is_active());
        assert!(!wheel.needs_frame());
        assert_eq!(wheel.step(start + FRAME), Point::default());
    }

    #[test]
    fn only_plain_whole_wheel_lines_animate() {
        let event = |delta| ScrollEvent {
            delta,
            ..Default::default()
        };
        let lines = event(ScrollDelta::Lines(point(0., -3.)));
        assert!(animates(&lines, false));
        assert!(
            !animates(&lines, true),
            "reduced motion applies a notch at once"
        );
        assert!(!animates(
            &event(ScrollDelta::Pixels(point(px(0.), px(-30.)))),
            false
        ));
        let touch = ScrollEvent {
            source: ScrollSource::Touch,
            ..lines.clone()
        };
        assert!(!animates(&touch, false));
        for modifiers in [Modifiers::control(), Modifiers::alt(), Modifiers::command()] {
            let held = ScrollEvent {
                modifiers,
                ..lines.clone()
            };
            assert!(
                !animates(&held, false),
                "{modifiers:?} keeps the notch a step"
            );
        }
        assert!(animates(
            &ScrollEvent {
                modifiers: Modifiers::shift(),
                ..lines.clone()
            },
            false
        ));
        #[cfg(not(target_os = "macos"))]
        assert!(
            !animates(&event(ScrollDelta::Lines(point(0., -0.37))), false),
            "a high-resolution wheel's fraction passes through"
        );
        set_scroll_event_continuous(true);
        assert!(
            !animates(&lines, false),
            "an X11 touchpad's lines pass through"
        );
        set_scroll_event_continuous(false);
    }
}
