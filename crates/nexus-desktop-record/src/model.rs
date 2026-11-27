//! What a screen recording for an agent holds, as its `capture.json` writes it (README.md, "Screen recordings for
//! agents"). Times are seconds from the start of `recording.mov`; places are points on the recorded display, from its
//! top-left corner (x to the right, y down), as the person saw them.

use serde::{Deserialize, Serialize};

/// `capture.json`'s `kind`, so an agent can tell the file apart from others.
pub const KIND: &str = "nexus.screen-recording";
/// `capture.json`'s `version`: bumped when a field changes meaning.
pub const VERSION: u32 = 1;

/// A place on the recorded display, in points from its top-left corner.
pub type Point = [f64; 2];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Display {
    /// `screencapture -D`'s number: 1 is the main display.
    pub index: u32,
    /// Its size in points.
    pub width: f64,
    pub height: f64,
}

/// What the person did while recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Event {
    /// A red stroke drawn with Shift held down, from `t` to `end`.
    Ink { t: f64, end: f64, points: Vec<Point> },
    /// A numbered pin, dropped by tapping Shift twice.
    Pin { t: f64, n: u32, at: Point },
    /// A click (the left button), which went through to the app under it.
    Click { t: f64, at: Point },
    /// The app in front, when the recording starts and whenever it (or its browser tab's address) changes.
    #[serde(rename_all = "camelCase")]
    App {
        t: f64,
        name: String,
        bundle_id: Option<String>,
        url: Option<String>,
    },
}

impl Event {
    pub fn t(&self) -> f64 {
        match self {
            Event::Ink { t, .. } | Event::Pin { t, .. } | Event::Click { t, .. } | Event::App { t, .. } => *t,
        }
    }

    pub fn shift(&mut self, by: f64) {
        match self {
            Event::Ink { t, end, .. } => {
                *t += by;
                *end += by;
            }
            Event::Pin { t, .. } | Event::Click { t, .. } | Event::App { t, .. } => *t += by,
        }
    }
}

/// One thing said, as the transcriber split it at a pause or the end of a sentence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Phrase {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscriptStatus {
    /// Transcribed on this Mac.
    Ok,
    /// Narration was recorded but nothing was said that could be made out.
    Silent,
    /// No narration: recorded without the microphone.
    #[default]
    NoAudio,
    /// Not transcribed: speech recognition is not allowed, or not available on this Mac for this language.
    Unavailable,
    /// The transcriber failed.
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub status: TranscriptStatus,
    /// Why there is no transcript, in a sentence (for the summary).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default)]
    pub phrases: Vec<Phrase>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// Relative to the recording's folder: `frames/01.png`.
    pub file: String,
    pub t: f64,
    /// Pixels.
    pub width: u32,
    pub height: u32,
    /// Why this moment was kept: `start`, `ink`, `pin`, `click`, `app`, `speech`, `gap`, `end`.
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Video {
    pub file: String,
}

/// `capture.json`. Written once when the recording stops (no frames or transcript yet), and again when they are made,
/// so `nexus-desktop --process-recording <folder>` can make them again from the video and the events.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capture {
    pub kind: String,
    pub version: u32,
    /// Local time with its offset: `2026-10-19T14:03:22-0600`.
    pub created_at: String,
    /// The app that made it: `desktop/<version>`.
    pub app: String,
    pub display: Display,
    /// Seconds.
    pub duration: f64,
    /// Whether the microphone was recorded with the screen.
    pub narration: bool,
    /// Until the events are on the video's clock: when the recording was stopped, in seconds on the events' clock
    /// (from when it was asked for). Gone once they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<Video>,
    #[serde(default)]
    pub frames: Vec<Frame>,
    #[serde(default)]
    pub transcript: Transcript,
    #[serde(default)]
    pub events: Vec<Event>,
}

impl Capture {
    pub fn new(created_at: String, app: String, display: Display) -> Capture {
        Capture {
            kind: KIND.into(),
            version: VERSION,
            created_at,
            app,
            display,
            ..Capture::default()
        }
    }

    /// The events were timed from when recording was asked for, and the video starts a little later. It ends when it
    /// was told to stop (`stopped_at`), so it started `stopped_at - duration` in: move every event back by that, and
    /// drop what happened before the video began. Once only: `stopped_at` is cleared.
    pub fn move_to_video_clock(&mut self, duration: f64) {
        let Some(stopped_at) = self.stopped_at.take() else {
            return;
        };
        let lead = (stopped_at - duration).max(0.0);
        for event in self.events.iter_mut() {
            event.shift(-lead);
        }
        self.events.retain(|event| match event {
            Event::Ink { end, .. } => *end > 0.0,
            other => other.t() >= 0.0,
        });
        for event in self.events.iter_mut() {
            if let Event::Ink { t, .. } = event {
                *t = t.max(0.0);
            }
        }
        self.events.sort_by(|a, b| a.t().total_cmp(&b.t()));
    }

    /// The display's points to a frame's pixels (frames keep the display's aspect ratio).
    pub fn to_frame(&self, frame: &Frame, at: Point) -> (u32, u32) {
        let scale = if self.display.width > 0.0 {
            f64::from(frame.width) / self.display.width
        } else {
            1.0
        };
        let clamp = |value: f64, max: u32| (value * scale).round().clamp(0.0, f64::from(max.saturating_sub(1))) as u32;
        (clamp(at[0], frame.width), clamp(at[1], frame.height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialise_with_their_kind_in_camel_case() {
        let app = Event::App {
            t: 1.5,
            name: "Safari".into(),
            bundle_id: Some("com.apple.Safari".into()),
            url: None,
        };
        let json = serde_json::to_value(&app).expect("json");
        assert_eq!(json["kind"], "app");
        assert_eq!(json["bundleId"], "com.apple.Safari");
        let back: Event = serde_json::from_value(json).expect("back");
        assert_eq!(back, app);
    }

    #[test]
    fn events_move_onto_the_videos_clock_once() {
        let mut capture = Capture {
            stopped_at: Some(10.0),
            events: vec![
                Event::Click { t: 5.0, at: [1.0, 1.0] },
                Event::Click { t: 0.5, at: [0.0, 0.0] },
                Event::Ink {
                    t: 0.8,
                    end: 2.0,
                    points: vec![],
                },
            ],
            ..Capture::default()
        };
        // Stopped 10 s after it was asked for; the video is 9 s long: it started 1 s in.
        capture.move_to_video_clock(9.0);
        let moved = vec![
            Event::Ink {
                t: 0.0,
                end: 1.0,
                points: vec![],
            },
            Event::Click { t: 4.0, at: [1.0, 1.0] },
        ];
        assert_eq!(capture.events, moved);
        assert_eq!(capture.stopped_at, None);
        capture.move_to_video_clock(9.0);
        assert_eq!(capture.events, moved);
    }

    #[test]
    fn display_points_map_to_frame_pixels() {
        let capture = Capture::new(
            "now".into(),
            "desktop/test".into(),
            Display {
                index: 1,
                width: 1600.0,
                height: 1000.0,
            },
        );
        let frame = Frame {
            file: "frames/01.png".into(),
            t: 0.0,
            width: 800,
            height: 500,
            reasons: vec![],
        };
        assert_eq!(capture.to_frame(&frame, [400.0, 200.0]), (200, 100));
        // Off the edge: kept inside the frame.
        assert_eq!(capture.to_frame(&frame, [5000.0, -3.0]), (799, 0));
    }
}
