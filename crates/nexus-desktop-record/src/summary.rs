//! `summary.md`, what an agent reads first: what the recording is, which files to open, and a timeline with one
//! section per keyframe holding what was said, drawn and clicked around it, in that frame's pixels. And the text put
//! on the clipboard, which points an agent at the folder. Pure.

use std::path::Path;

use super::model::{Capture, Event, Frame, Phrase, TranscriptStatus};

/// `0:04`, `12:30`, `1:02:03`.
pub fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// `0:12`, `1:02:03` or `12.5` (seconds), as `--frames` takes them.
pub fn parse_time(text: &str) -> Option<f64> {
    let mut seconds = 0.0;
    for part in text.trim().split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok().filter(|value| *value >= 0.0)?;
    }
    Some(seconds)
}

/// The moment an event shows best, as src/keyframes.rs picks it.
fn anchor(event: &Event) -> f64 {
    match event {
        Event::Ink { end, .. } => end + 0.05,
        Event::Pin { t, .. } => t + 0.15,
        Event::Click { t, .. } | Event::App { t, .. } => t + 0.5,
    }
}

/// The keyframe nearest to `t`.
fn nearest(frames: &[Frame], t: f64) -> usize {
    (0..frames.len())
        .min_by(|&a, &b| (frames[a].t - t).abs().total_cmp(&(frames[b].t - t).abs()))
        .unwrap_or(0)
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The app in front at `t`, and its address: `Safari · http://localhost:5173/settings`.
fn app_at(events: &[Event], t: f64) -> Option<String> {
    events.iter().rev().find_map(|event| match event {
        Event::App { t: at, name, url, .. } if *at <= t + 0.6 => Some(match url {
            Some(url) => format!("{name} · {url}"),
            None => name.clone(),
        }),
        _ => None,
    })
}

fn line(capture: &Capture, frame: &Frame, event: &Event) -> Option<String> {
    let at = |point| {
        let (x, y) = capture.to_frame(frame, point);
        format!("({x}, {y})")
    };
    Some(match event {
        Event::Ink { t, points, .. } => {
            let (mut low, mut high) = ([f64::MAX; 2], [f64::MIN; 2]);
            for point in points {
                for axis in 0..2 {
                    low[axis] = low[axis].min(point[axis]);
                    high[axis] = high[axis].max(point[axis]);
                }
            }
            if points.is_empty() {
                return None;
            }
            format!("- {} red ink around {}–{}", clock(*t), at(low), at(high))
        }
        Event::Pin { t, n, at: point } => format!("- {} pin {n} at {}", clock(*t), at(*point)),
        Event::Click { t, at: point } => format!("- {} click at {}", clock(*t), at(*point)),
        Event::App { .. } => return None,
    })
}

fn transcript_note(capture: &Capture) -> Option<String> {
    let transcript = &capture.transcript;
    match transcript.status {
        TranscriptStatus::Ok => None,
        TranscriptStatus::Silent => Some("Narration was recorded, but nothing said could be made out.".into()),
        TranscriptStatus::NoAudio => Some("No narration: this was recorded without the microphone.".into()),
        TranscriptStatus::Unavailable | TranscriptStatus::Failed => Some(format!(
            "No transcript{}. The narration is in `recording.mov`.",
            transcript
                .note
                .as_deref()
                .map(|note| format!(": {note}"))
                .unwrap_or_default()
        )),
    }
}

pub fn render(capture: &Capture) -> String {
    let frames = &capture.frames;
    let mut out = String::from("# Screen recording for an agent\n\n");
    out.push_str(&format!(
        "{} recorded {}, display {} ({} × {} pt){}. Made with Nexus for Mac ({}).\n\n",
        clock(capture.duration),
        capture.created_at,
        capture.display.index,
        capture.display.width.round(),
        capture.display.height.round(),
        if capture.narration { ", narrated" } else { "" },
        capture.app,
    ));
    let talked = capture.narration && !capture.transcript.phrases.is_empty();
    out.push_str(&format!(
        "Someone recorded their screen to show you something{}. The red ink and the numbered pins are theirs: they \
         drew them on the screen to point at what they mean. Their clicks went through to the apps.\n\n",
        if talked { ", talking as they went" } else { "" },
    ));
    let size = frames
        .first()
        .map(|frame| format!(", {} × {} px", frame.width, frame.height))
        .unwrap_or_default();
    out.push_str(&format!(
        "- `contact-sheet.png`: the {} on one image, each labelled with its number and time.\n",
        plural(frames.len(), "keyframe", "keyframes")
    ));
    out.push_str(&format!(
        "- `frames/NN.png`: each keyframe{size}. Places below are pixels in these frames.\n"
    ));
    out.push_str("- `capture.json`: the same timeline as data, in the display's points.\n");
    out.push_str(&format!(
        "- `recording.mov`: the whole recording{}.\n",
        if capture.narration { ", with the narration" } else { "" }
    ));
    if let Some(note) = transcript_note(capture) {
        out.push_str(&format!("\n{note}\n"));
    }

    // Each thing said and done goes under the keyframe nearest to the moment it shows.
    let mut said: Vec<Vec<&Phrase>> = vec![Vec::new(); frames.len()];
    for phrase in &capture.transcript.phrases {
        if !frames.is_empty() {
            said[nearest(frames, phrase.end)].push(phrase);
        }
    }
    let mut done: Vec<Vec<&Event>> = vec![Vec::new(); frames.len()];
    for event in &capture.events {
        if !frames.is_empty() && !matches!(event, Event::App { .. }) {
            done[nearest(frames, anchor(event))].push(event);
        }
    }

    out.push_str("\n## Timeline\n");
    let mut previous_app = None;
    for (index, frame) in frames.iter().enumerate() {
        let app = app_at(&capture.events, frame.t);
        let heading = match (&app, &previous_app) {
            (Some(app), Some(previous)) if app == previous => String::new(),
            (Some(app), _) => format!(" · {app}"),
            (None, _) => String::new(),
        };
        previous_app = app.or(previous_app);
        out.push_str(&format!("\n### {:02} · {}{heading}\n\n", index + 1, clock(frame.t)));
        out.push_str(&format!("`{}`\n", frame.file));
        if !said[index].is_empty() {
            let words: Vec<&str> = said[index].iter().map(|phrase| phrase.text.trim()).collect();
            out.push_str(&format!("\n> {}\n", words.join(" ")));
        }
        let lines: Vec<String> = done[index]
            .iter()
            .filter_map(|event| line(capture, frame, event))
            .collect();
        if !lines.is_empty() {
            out.push('\n');
            out.push_str(&lines.join("\n"));
            out.push('\n');
        }
    }
    out
}

/// What goes on the clipboard beside the contact sheet's image: where the recording is and what to read first.
pub fn clipboard(capture: &Capture, dir: &Path) -> String {
    format!(
        "A screen recording for you to look at ({}, {}{}):\n{}\nRead summary.md in that folder first; it names the frames to open.\n",
        clock(capture.duration),
        plural(capture.frames.len(), "keyframe", "keyframes"),
        if capture.narration { ", narrated" } else { "" },
        dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Display, Transcript};

    fn capture() -> Capture {
        let frame = |n: u32, t: f64| Frame {
            file: format!("frames/{n:02}.png"),
            t,
            width: 800,
            height: 500,
            reasons: vec![],
        };
        Capture {
            duration: 12.4,
            narration: true,
            display: Display {
                index: 1,
                width: 1600.0,
                height: 1000.0,
            },
            frames: vec![frame(1, 0.3), frame(2, 4.05), frame(3, 12.3)],
            transcript: Transcript {
                status: TranscriptStatus::Ok,
                note: None,
                locale: Some("en-US".into()),
                phrases: vec![
                    Phrase {
                        start: 0.5,
                        end: 1.0,
                        text: "This is the settings page.".into(),
                    },
                    Phrase {
                        start: 2.0,
                        end: 3.8,
                        text: "This button is cut off.".into(),
                    },
                ],
            },
            events: vec![
                Event::App {
                    t: 0.0,
                    name: "Safari".into(),
                    bundle_id: Some("com.apple.Safari".into()),
                    url: Some("http://localhost:5173/settings".into()),
                },
                Event::Ink {
                    t: 3.0,
                    end: 4.0,
                    points: vec![[200.0, 100.0], [400.0, 300.0]],
                },
                Event::Click {
                    t: 11.0,
                    at: [800.0, 500.0],
                },
            ],
            ..Capture::new(
                "2026-10-19T14:03:22-0600".into(),
                "desktop/test".into(),
                Display::default(),
            )
        }
    }

    #[test]
    fn clocks_both_ways() {
        assert_eq!(clock(4.4), "0:04");
        assert_eq!(clock(750.0), "12:30");
        assert_eq!(clock(3723.0), "1:02:03");
        assert_eq!(parse_time("0:12"), Some(12.0));
        assert_eq!(parse_time("1:02:03"), Some(3723.0));
        assert_eq!(parse_time("12.5"), Some(12.5));
        assert_eq!(parse_time("soon"), None);
        assert_eq!(parse_time("-1"), None);
    }

    #[test]
    fn the_timeline_puts_speech_and_ink_under_their_keyframes() {
        let summary = render(&capture());
        assert!(summary.contains("0:12 recorded 2026-10-19T14:03:22-0600, display 1 (1600 × 1000 pt), narrated."));
        assert!(summary.contains("the 3 keyframes on one image"));
        assert!(summary.contains("each keyframe, 800 × 500 px"));
        assert!(summary.contains(
            "### 01 · 0:00 · Safari · http://localhost:5173/settings\n\n`frames/01.png`\n\n> This is the settings page.\n"
        ));
        assert!(summary.contains(
            "### 02 · 0:04\n\n`frames/02.png`\n\n> This button is cut off.\n\n- 0:03 red ink around (100, 50)–(200, 150)\n"
        ));
        assert!(summary.contains("### 03 · 0:12\n\n`frames/03.png`\n\n- 0:11 click at (400, 250)\n"));
        assert!(!summary.contains("No transcript"));
        assert!(summary.contains("to show you something, talking as they went."));
        assert!(summary.contains("- `recording.mov`: the whole recording, with the narration.\n"));
    }

    #[test]
    fn a_recording_without_the_microphone_says_nothing_about_talking() {
        let mut capture = capture();
        capture.narration = false;
        capture.transcript = Transcript {
            status: TranscriptStatus::NoAudio,
            ..Transcript::default()
        };
        let summary = render(&capture);
        assert!(summary.contains("display 1 (1600 × 1000 pt). Made with"));
        assert!(summary.contains("to show you something. The red ink"));
        assert!(summary.contains("- `recording.mov`: the whole recording.\n"));
        assert!(summary.contains("No narration: this was recorded without the microphone."));
        assert!(!summary.contains("talking"));
        assert_eq!(summary.matches("narrat").count(), 1);

        // The microphone was on, but nothing said was made out.
        capture.narration = true;
        capture.transcript.status = TranscriptStatus::Silent;
        let summary = render(&capture);
        assert!(!summary.contains("talking"));
        assert!(summary.contains("- `recording.mov`: the whole recording, with the narration.\n"));
    }

    #[test]
    fn a_missing_transcript_says_why() {
        let mut capture = capture();
        capture.transcript = Transcript {
            status: TranscriptStatus::Unavailable,
            note: Some("speech recognition is not allowed for Nexus".into()),
            ..Transcript::default()
        };
        assert!(render(&capture).contains(
            "No transcript: speech recognition is not allowed for Nexus. The narration is in `recording.mov`."
        ));
        let text = clipboard(&capture, Path::new("/tmp/rec"));
        assert_eq!(
            text,
            "A screen recording for you to look at (0:12, 3 keyframes, narrated):\n/tmp/rec\nRead summary.md in that folder first; it names the frames to open.\n"
        );
    }
}
