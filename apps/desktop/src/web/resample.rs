//! Touchpad scrolling, resampled to the app's frames. A touchpad reports at its own rate, often 100 to 140 events a
//! second, so a 60Hz frame gets two of them, or three, and the page moves 10px one frame and 15px the next: the
//! scroll lurches. Chromium resamples its own windows' scrolling to its frames; a page drawn offscreen gets the
//! events as they come. So the app does it: each frame sends where the finger was [`lag`](Resampler::lag) before
//! it, between the two events around that time, and what it sent so far is taken off.
//!
//! When the finger lifts mid-stroke, the scroll goes on and slows down, as Chromium's own windows do it on Linux: by
//! Chromium's fling curve, from the stroke's speed at the end.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The lag stays between these: under the first, a slow touchpad's frames go past its last event and wait; over the
/// second, the scroll feels late.
const LAG_MIN: Duration = Duration::from_millis(4);
const LAG_MAX: Duration = Duration::from_millis(16);
/// No event for this long: the finger stopped or lifted, and the rest goes at once.
const SETTLE: Duration = Duration::from_millis(40);
/// Events kept, for the interpolation and the touchpad's rate.
const KEPT: usize = 8;
/// A lift starts a fling from the stroke's speed over this last part of it.
const FLING_WINDOW: Duration = Duration::from_millis(100);
/// Slower than this (pixels a second), a lift just stops: Chromium's least fling speed.
const FLING_MIN: f32 = 50.;

/// Chromium's fling curve (`ui/events/gestures/fling_curve.cc`): the distance `t` seconds in is
/// `ALPHA * e^(-GAMMA * t) - BETA * t - ALPHA`. A fling joins it where its speed matches, and stops where the speed
/// reaches nought.
const ALPHA: f32 = -5707.62;
const BETA: f32 = 172.;
const GAMMA: f32 = 3.7;

fn curve_at(t: f32) -> f32 {
    ALPHA * (-GAMMA * t).exp() - BETA * t - ALPHA
}

/// A fling under way: it started at `from`, at `offset` seconds into the curve. Its speed's larger part follows the
/// curve, and `ratio` scales it to each axis, as Chromium's does.
struct Fling {
    from: Instant,
    offset: f32,
    end: f32,
    ratio: (f32, f32),
}

impl Fling {
    fn new(from: Instant, velocity: (f32, f32)) -> Option<Self> {
        let speed = velocity.0.abs().max(velocity.1.abs());
        if speed < FLING_MIN {
            return None;
        }
        let fastest = -ALPHA * GAMMA - BETA;
        // Where the curve's speed, `-ALPHA * GAMMA * e^(-GAMMA * t) - BETA`, is this one's, and where it is nought.
        let offset = (-((speed.min(fastest) + BETA) / (-ALPHA * GAMMA)).ln() / GAMMA).max(0.);
        let end = (-ALPHA * GAMMA / BETA).ln() / GAMMA;
        Some(Self {
            from,
            offset,
            end,
            ratio: (velocity.0 / speed, velocity.1 / speed),
        })
    }

    /// How far the fling has gone at `at`, and whether it is over.
    fn at(&self, at: Instant) -> ((f32, f32), bool) {
        let t = (self.offset + at.saturating_duration_since(self.from).as_secs_f32()).min(self.end);
        let distance = curve_at(t) - curve_at(self.offset);
        ((distance * self.ratio.0, distance * self.ratio.1), t >= self.end)
    }
}

#[derive(Default)]
pub(super) struct Resampler {
    /// After each event: when it came, and where the scroll had got to in all.
    events: VecDeque<(Instant, (f32, f32))>,
    /// Where the scroll has got to, and how much of it went to the page (whole pixels).
    total: (f32, f32),
    sent: (f32, f32),
    /// When this stroke's first real event came (the one before it is made up).
    stroke: Option<Instant>,
    fling: Option<Fling>,
}

impl Resampler {
    /// A touchpad event at `at` moved the scroll by `delta` pixels.
    pub(super) fn push(&mut self, at: Instant, delta: (f32, f32)) {
        if let Some(fling) = self.fling.take() {
            // The fingers are back: the fling stops where it got to, and a new stroke starts there.
            let (gone, _) = fling.at(at.checked_sub(self.lag()).unwrap_or(at));
            self.total = (self.total.0 + gone.0, self.total.1 + gone.1);
            self.events.clear();
        }
        if self.events.is_empty() {
            self.stroke = Some(at);
            // A new stroke starts from where the last one ended, one touchpad interval back, so its first frame
            // moves part of the way and not all at once.
            self.events
                .push_back((at.checked_sub(LAG_MAX).unwrap_or(at), self.total));
        }
        self.total = (self.total.0 + delta.0, self.total.1 + delta.1);
        self.events.push_back((at, self.total));
        while self.events.len() > KEPT {
            self.events.pop_front();
        }
    }

    /// The finger lifted at `at`: the scroll goes on from the stroke's speed, and slows down to a stop. A finger that
    /// rested before it lifted has no speed, and the stroke ended already.
    pub(super) fn lift(&mut self, at: Instant) {
        let Some(&(last, position)) = self.events.back() else {
            return;
        };
        if self.fling.is_some() || at.saturating_duration_since(last) >= FLING_WINDOW {
            return;
        }
        let stroke = self.stroke.unwrap_or(last);
        let since = last.checked_sub(FLING_WINDOW).unwrap_or(last).max(stroke);
        let Some(&(first, start)) = self.events.iter().find(|event| event.0 >= since) else {
            return;
        };
        let span = last.saturating_duration_since(first).as_secs_f32();
        if span <= 0. {
            return;
        }
        let velocity = ((position.0 - start.0) / span, (position.1 - start.1) / span);
        // It goes on from the last event, where the resampled scroll gets to `lag` after it.
        self.fling = Fling::new(last, velocity);
    }

    /// Whether some of the scroll has not gone to the page yet.
    pub(super) fn pending(&self) -> bool {
        !self.events.is_empty() || self.fling.is_some()
    }

    /// The whole pixels to scroll the page by, for the frame drawn at `at`.
    pub(super) fn take(&mut self, at: Instant) -> (i32, i32) {
        let Some(&(last, _)) = self.events.back() else {
            return (0, 0);
        };
        let lagged = at.checked_sub(self.lag()).unwrap_or(at);
        let target = if let Some(fling) = &self.fling {
            if lagged <= last {
                self.at(lagged)
            } else {
                let (gone, over) = fling.at(lagged);
                let target = (self.total.0 + gone.0, self.total.1 + gone.1);
                if over {
                    self.fling = None;
                    self.events.clear();
                    self.total = target;
                }
                target
            }
        } else if at.saturating_duration_since(last) >= SETTLE {
            self.events.clear();
            self.total
        } else {
            self.at(lagged)
        };
        let whole = |want: f32, sent: f32| (want - sent).trunc();
        let step = (whole(target.0, self.sent.0), whole(target.1, self.sent.1));
        self.sent = (self.sent.0 + step.0, self.sent.1 + step.1);
        if self.events.is_empty() {
            // The stroke ended: what is left below a pixel goes with the next one.
            self.total = (self.total.0 - self.sent.0, self.total.1 - self.sent.1);
            self.sent = (0., 0.);
        }
        (step.0 as i32, step.1 as i32)
    }

    /// One touchpad interval: the median gap between its recent events, within [`LAG_MIN`] and [`LAG_MAX`].
    fn lag(&self) -> Duration {
        let mut gaps: Vec<Duration> = self
            .events
            .iter()
            .zip(self.events.iter().skip(1))
            .skip(1)
            .map(|(before, after)| after.0.saturating_duration_since(before.0))
            .collect();
        if gaps.is_empty() {
            return LAG_MAX;
        }
        gaps.sort();
        gaps[gaps.len() / 2].clamp(LAG_MIN, LAG_MAX)
    }

    /// Where the scroll was at `at`: between the events around it, or the last one's when it is past them.
    fn at(&self, at: Instant) -> (f32, f32) {
        let mut before = self.events.front().map_or(self.total, |event| event.1);
        let mut from = self.events.front().map_or(at, |event| event.0);
        for &(time, position) in &self.events {
            if time >= at {
                let span = time.saturating_duration_since(from).as_secs_f32();
                if span <= 0. {
                    return position;
                }
                let part = at.saturating_duration_since(from).as_secs_f32() / span;
                return (
                    before.0 + (position.0 - before.0) * part,
                    before.1 + (position.1 - before.1) * part,
                );
            }
            before = position;
            from = time;
        }
        before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A touchpad at `rate` events a second, `step` pixels each, for `seconds`, read by 60Hz frames.
    fn stroke(rate: f32, step: f32, seconds: f32) -> (Vec<i32>, f32) {
        let start = Instant::now();
        let mut resampler = Resampler::default();
        let every = Duration::from_secs_f32(1. / rate);
        let frame = Duration::from_secs_f64(1. / 60.);
        let end = start + Duration::from_secs_f32(seconds);
        let (mut next_event, mut next_frame) = (start, start + frame);
        let mut moved = Vec::new();
        let mut input = 0.;
        while next_frame < end + Duration::from_millis(200) {
            if next_event < end && next_event <= next_frame {
                resampler.push(next_event, (0., step));
                input += step;
                next_event += every;
            } else {
                moved.push(resampler.take(next_frame).1);
                next_frame += frame;
            }
        }
        (moved, input)
    }

    /// How far the frames of the stroke's steady middle (past its start and end ramps) stray from its mean, in
    /// pixels.
    fn stray(moved: &[i32]) -> f32 {
        let middle = &moved[10..moved.len() - 30];
        let mean = middle.iter().sum::<i32>() as f32 / middle.len() as f32;
        middle.iter().map(|&m| (m as f32 - mean).abs()).fold(0., f32::max)
    }

    #[test]
    fn a_steady_touchpad_scrolls_evenly_frame_to_frame() {
        for rate in [70., 100., 125., 140.] {
            let (moved, _) = stroke(rate, 5., 2.);
            // Only whole pixels go to the page, so a frame may be one off. Without resampling, 125 events a second
            // at 5px each move 10px a frame, and 15px every twelfth.
            assert!(stray(&moved) < 1., "{rate}/s: {moved:?}");
        }
    }

    #[test]
    fn a_stroke_moves_the_page_by_all_of_it() {
        let (moved, input) = stroke(125., 3.3, 1.);
        assert_eq!(moved.iter().sum::<i32>(), input.trunc() as i32);
    }

    #[test]
    fn one_event_goes_once_the_finger_stops() {
        let start = Instant::now();
        let mut resampler = Resampler::default();
        resampler.push(start, (0., -7.5));
        let mut moved = 0;
        for frame in 1..10 {
            moved += resampler.take(start + Duration::from_millis(frame * 16)).1;
        }
        assert_eq!(moved, -7);
        assert!(!resampler.pending());
        // The half pixel left goes with the next stroke.
        resampler.push(start + Duration::from_secs(1), (0., -0.5));
        assert_eq!(resampler.take(start + Duration::from_secs(2)).1, -1);
    }

    /// A stroke of `rate` events a second, `step` pixels each, lifted after `seconds`; the frames until it settles.
    fn flick(rate: f32, step: f32, seconds: f32) -> Vec<i32> {
        let start = Instant::now();
        let mut resampler = Resampler::default();
        let every = Duration::from_secs_f32(1. / rate);
        let frame = Duration::from_secs_f64(1. / 60.);
        let end = start + Duration::from_secs_f32(seconds);
        let (mut next_event, mut next_frame) = (start, start + frame);
        let mut last = start;
        let mut lifted = false;
        let mut moved = Vec::new();
        while next_frame < end + Duration::from_secs(3) {
            if next_event < end && next_event <= next_frame {
                resampler.push(next_event, (0., step));
                last = next_event;
                next_event += every;
            } else if !lifted && next_event >= end && last + Duration::from_millis(2) <= next_frame {
                resampler.lift(last + Duration::from_millis(2));
                lifted = true;
            } else {
                moved.push(resampler.take(next_frame).1);
                next_frame += frame;
            }
        }
        assert!(!resampler.pending());
        moved
    }

    #[test]
    fn a_lift_flings_on_and_slows_down_smoothly() {
        // 125 events a second at 5px: 625px a second, about 10px a frame.
        let moved = flick(125., 5., 0.5);
        let lift = moved.iter().rposition(|&m| m == 10).expect("the steady stroke");
        let fling: Vec<i32> = moved[lift..].iter().copied().take_while(|&m| m > 0).collect();
        // It goes on for a while, about Chromium's distance for the speed, and never speeds up or jumps.
        let gone: i32 = fling.iter().sum();
        let curve = Fling::new(Instant::now(), (0., 625.)).expect("fast enough");
        let want = curve_at(curve.end) - curve_at(curve.offset);
        assert!((gone as f32 - want).abs() < want * 0.1, "{gone} vs {want}: {moved:?}");
        assert!(fling.len() > 15, "{moved:?}");
        assert!(fling.windows(2).all(|pair| pair[1] <= pair[0] + 1), "{moved:?}");
    }

    #[test]
    fn a_finger_that_rests_before_it_lifts_does_not_fling() {
        let start = Instant::now();
        let mut resampler = Resampler::default();
        for event in 0..20 {
            resampler.push(start + Duration::from_millis(event * 8), (0., 5.));
        }
        resampler.lift(start + Duration::from_millis(152 + 300));
        let moved: i32 = (1..60)
            .map(|frame| resampler.take(start + Duration::from_millis(frame * 16)).1)
            .sum();
        assert_eq!(moved, 100);
        assert!(!resampler.pending());
    }

    #[test]
    fn fingers_back_on_the_touchpad_stop_a_fling() {
        let start = Instant::now();
        let mut resampler = Resampler::default();
        for event in 0..20 {
            resampler.push(start + Duration::from_millis(event * 8), (0., 5.));
        }
        resampler.lift(start + Duration::from_millis(154));
        let mut moved = 0;
        for frame in 1..20 {
            moved += resampler.take(start + Duration::from_millis(frame * 16)).1;
        }
        let back = start + Duration::from_millis(320);
        resampler.push(back, (0., 0.));
        for frame in 0..10 {
            moved += resampler.take(back + Duration::from_millis(frame * 16)).1;
        }
        let stopped = moved;
        for frame in 10..100 {
            moved += resampler.take(back + Duration::from_millis(frame * 16)).1;
        }
        assert!(moved > 100, "it flung: {moved}");
        assert_eq!(moved, stopped);
    }
}
