//! The person's gestures while recording, from samples of the pointer, Shift and the left button (src/screen.rs
//! polls them about 60 times a second; no permission needed): hold Shift and move to draw red ink, tap Shift twice to
//! drop a numbered pin, and every click is noted. Clicks still go to the app under the pointer: the ink overlay never
//! takes the mouse. Pure, so the rules are tested here.

use super::model::{Event, Point};

/// A pointer reading: when (seconds since the recording started), where (points on the recorded display), and what
/// was held.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub t: f64,
    pub at: Point,
    pub shift: bool,
    pub button: bool,
}

/// What the ink overlay draws.
#[derive(Debug, Clone, PartialEq)]
pub enum Draw {
    Begin {
        id: u32,
        at: Point,
    },
    Extend {
        id: u32,
        at: Point,
    },
    End {
        id: u32,
    },
    /// Too short to be ink: rubbed out.
    Discard {
        id: u32,
    },
    Pin {
        n: u32,
        at: Point,
    },
}

/// A point closer than this to the stroke's last one is skipped (points).
const STEP: f64 = 1.5;
/// A stroke shorter than this is a slip of the hand, not ink (points).
const MIN_STROKE: f64 = 6.0;
/// A Shift press shorter than this, with the pointer still, is a tap (seconds, points).
const TAP: f64 = 0.35;
const TAP_DRIFT: f64 = 6.0;
/// Two taps closer than this drop a pin (seconds).
const DOUBLE_TAP: f64 = 0.45;

#[derive(Debug)]
struct Stroke {
    id: u32,
    t: f64,
    points: Vec<Point>,
}

#[derive(Debug, Default)]
pub struct Gestures {
    last: Option<Sample>,
    /// When and where Shift went down, and whether the button was pressed since.
    shift_down: Option<(f64, Point, bool)>,
    last_tap: Option<f64>,
    stroke: Option<Stroke>,
    strokes: u32,
    pins: u32,
}

fn distance(a: Point, b: Point) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn length(points: &[Point]) -> f64 {
    points.windows(2).map(|pair| distance(pair[0], pair[1])).sum()
}

impl Gestures {
    /// Take one sample: what to draw, and any event it completed.
    pub fn feed(&mut self, sample: Sample) -> (Vec<Draw>, Vec<Event>) {
        let mut draws = Vec::new();
        let mut events = Vec::new();
        let previous = self.last.replace(sample);
        let was_shift = previous.is_some_and(|previous| previous.shift);
        let was_button = previous.is_some_and(|previous| previous.button);

        if sample.button && !was_button {
            events.push(Event::Click {
                t: sample.t,
                at: sample.at,
            });
            if let Some((_, _, pressed)) = self.shift_down.as_mut() {
                *pressed = true;
            }
        }

        if sample.shift && !was_shift {
            self.shift_down = Some((sample.t, sample.at, sample.button));
        }

        if sample.shift {
            let start = self.shift_down.map_or(sample.at, |(_, at, _)| at);
            match self.stroke.as_mut() {
                None if distance(start, sample.at) >= STEP => {
                    self.strokes += 1;
                    let id = self.strokes;
                    draws.push(Draw::Begin { id, at: start });
                    draws.push(Draw::Extend { id, at: sample.at });
                    self.stroke = Some(Stroke {
                        id,
                        t: sample.t,
                        points: vec![start, sample.at],
                    });
                }
                Some(stroke) => {
                    let last = stroke.points.last().copied().unwrap_or(sample.at);
                    if distance(last, sample.at) >= STEP {
                        stroke.points.push(sample.at);
                        draws.push(Draw::Extend {
                            id: stroke.id,
                            at: sample.at,
                        });
                    }
                }
                None => {}
            }
            return (draws, events);
        }

        if was_shift {
            // Shift came up: the stroke ends, or it was a tap.
            if let Some(stroke) = self.stroke.take() {
                if length(&stroke.points) >= MIN_STROKE {
                    draws.push(Draw::End { id: stroke.id });
                    events.push(Event::Ink {
                        t: stroke.t,
                        end: sample.t,
                        points: stroke.points,
                    });
                } else {
                    draws.push(Draw::Discard { id: stroke.id });
                }
                self.last_tap = None;
            } else if let Some((down, at, pressed)) = self.shift_down {
                let tap = sample.t - down < TAP && distance(at, sample.at) < TAP_DRIFT && !pressed;
                match (tap, self.last_tap) {
                    (true, Some(previous)) if sample.t - previous < DOUBLE_TAP => {
                        self.pins += 1;
                        draws.push(Draw::Pin {
                            n: self.pins,
                            at: sample.at,
                        });
                        events.push(Event::Pin {
                            t: sample.t,
                            n: self.pins,
                            at: sample.at,
                        });
                        self.last_tap = None;
                    }
                    (true, _) => self.last_tap = Some(sample.t),
                    (false, _) => self.last_tap = None,
                }
            }
            self.shift_down = None;
        }
        (draws, events)
    }

    /// The recording stopped with Shift still down: the stroke so far counts.
    pub fn finish(&mut self, t: f64) -> Option<Event> {
        let stroke = self.stroke.take()?;
        (length(&stroke.points) >= MIN_STROKE).then_some(Event::Ink {
            t: stroke.t,
            end: t,
            points: stroke.points,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t: f64, x: f64, y: f64, shift: bool, button: bool) -> Sample {
        Sample {
            t,
            at: [x, y],
            shift,
            button,
        }
    }

    fn run(samples: &[Sample]) -> (Vec<Draw>, Vec<Event>) {
        let mut gestures = Gestures::default();
        let mut all = (Vec::new(), Vec::new());
        for sample in samples {
            let (draws, events) = gestures.feed(*sample);
            all.0.extend(draws);
            all.1.extend(events);
        }
        all
    }

    #[test]
    fn holding_shift_and_moving_draws_one_stroke() {
        let (draws, events) = run(&[
            sample(0.0, 10.0, 10.0, false, false),
            sample(0.1, 10.0, 10.0, true, false),
            sample(0.2, 20.0, 10.0, true, false),
            sample(0.21, 20.5, 10.0, true, false),
            sample(0.3, 30.0, 20.0, true, false),
            sample(0.4, 30.0, 20.0, false, false),
        ]);
        assert_eq!(
            draws,
            vec![
                Draw::Begin {
                    id: 1,
                    at: [10.0, 10.0]
                },
                Draw::Extend {
                    id: 1,
                    at: [20.0, 10.0]
                },
                Draw::Extend {
                    id: 1,
                    at: [30.0, 20.0]
                },
                Draw::End { id: 1 },
            ]
        );
        assert_eq!(
            events,
            vec![Event::Ink {
                t: 0.2,
                end: 0.4,
                points: vec![[10.0, 10.0], [20.0, 10.0], [30.0, 20.0]],
            }]
        );
    }

    #[test]
    fn a_wobble_is_rubbed_out_and_a_click_is_noted() {
        let (draws, events) = run(&[
            sample(0.0, 10.0, 10.0, true, false),
            sample(0.1, 12.0, 10.0, true, false),
            sample(0.2, 12.0, 10.0, false, false),
            sample(1.0, 50.0, 60.0, false, true),
            sample(1.1, 50.0, 60.0, false, true),
            sample(1.2, 50.0, 60.0, false, false),
        ]);
        assert_eq!(
            draws,
            vec![
                Draw::Begin {
                    id: 1,
                    at: [10.0, 10.0]
                },
                Draw::Extend {
                    id: 1,
                    at: [12.0, 10.0]
                },
                Draw::Discard { id: 1 },
            ]
        );
        assert_eq!(
            events,
            vec![Event::Click {
                t: 1.0,
                at: [50.0, 60.0]
            }]
        );
    }

    #[test]
    fn two_quick_shift_taps_drop_a_pin_and_one_does_not() {
        let taps = [
            sample(0.0, 40.0, 40.0, true, false),
            sample(0.1, 40.0, 40.0, false, false),
            sample(0.3, 40.0, 40.0, true, false),
            sample(0.4, 41.0, 40.0, false, false),
        ];
        let (draws, events) = run(&taps);
        assert_eq!(draws, vec![Draw::Pin { n: 1, at: [41.0, 40.0] }]);
        assert_eq!(
            events,
            vec![Event::Pin {
                t: 0.4,
                n: 1,
                at: [41.0, 40.0]
            }]
        );
        // One tap, then a slow second one: no pin.
        let (draws, _) = run(&[
            taps[0],
            taps[1],
            sample(1.0, 40.0, 40.0, true, false),
            sample(1.1, 40.0, 40.0, false, false),
        ]);
        assert!(draws.is_empty());
        // Shift-click (selecting text) is not a tap.
        let (draws, events) = run(&[
            taps[0],
            sample(0.05, 40.0, 40.0, true, true),
            sample(0.1, 40.0, 40.0, false, false),
            taps[2],
            taps[3],
        ]);
        assert!(draws.is_empty());
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn a_stroke_still_drawn_at_the_stop_counts() {
        let mut gestures = Gestures::default();
        gestures.feed(sample(0.0, 0.0, 0.0, true, false));
        gestures.feed(sample(0.1, 20.0, 0.0, true, false));
        assert_eq!(
            gestures.finish(0.5),
            Some(Event::Ink {
                t: 0.1,
                end: 0.5,
                points: vec![[0.0, 0.0], [20.0, 0.0]],
            })
        );
        assert_eq!(gestures.finish(0.6), None);
    }
}
