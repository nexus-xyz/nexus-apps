//! From a stopped recording to what an agent reads: the events moved onto the video's clock, the narration
//! transcribed, the keyframes chosen and decoded (`frames/NN.png`), the contact sheet, `summary.md` and `capture.json`.
//! Also `nexus-desktop --process-recording <folder>`, which does it again for a folder (after a change to these rules,
//! or when the app quit before it finished), and `--frames`, which pulls more frames out for a closer look. macOS only.

use std::path::{Path, PathBuf};

use super::keyframes;
use super::media::{FRAME_SIZE, Video, transcribe};
use super::model::{Capture, Frame, Transcript, TranscriptStatus};
use super::sheet::{Rgb, contact_sheet};
use super::summary::{clock, render};

pub const VIDEO: &str = "recording.mov";
pub const MANIFEST: &str = "capture.json";
pub const SUMMARY: &str = "summary.md";
pub const SHEET: &str = "contact-sheet.png";
pub const FRAMES: &str = "frames";

pub fn read(dir: &Path) -> Result<Capture, String> {
    let text = std::fs::read_to_string(dir.join(MANIFEST))
        .map_err(|error| format!("{}: {error}", dir.join(MANIFEST).display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", dir.join(MANIFEST).display()))
}

pub fn write(dir: &Path, capture: &Capture) -> Result<(), String> {
    let text = serde_json::to_string_pretty(capture).map_err(|error| error.to_string())?;
    std::fs::write(dir.join(MANIFEST), format!("{text}\n")).map_err(|error| error.to_string())
}

/// Make everything from the video and `capture`'s events; `ask` lets the transcriber ask for permission (the app).
/// Returns the contact sheet's PNG, for the clipboard.
pub fn process(dir: &Path, capture: &mut Capture, ask: bool) -> Result<Vec<u8>, String> {
    let video_path = dir.join(VIDEO);
    let video = Video::open(&video_path, FRAME_SIZE)?;
    let duration = video.duration();
    if duration <= 0.0 {
        return Err("the recording is empty".into());
    }
    capture.duration = duration;
    capture.move_to_video_clock(duration);
    capture.video = Some(super::model::Video { file: VIDEO.into() });
    capture.transcript = if capture.narration {
        transcribe(&video_path, dir, ask)
    } else {
        Transcript {
            status: TranscriptStatus::NoAudio,
            ..Transcript::default()
        }
    };

    let picks = keyframes::choose(duration, &capture.events, &capture.transcript.phrases);
    let frames_dir = dir.join(FRAMES);
    let _ = std::fs::remove_dir_all(&frames_dir);
    std::fs::create_dir_all(&frames_dir).map_err(|error| error.to_string())?;
    let mut images: Vec<Rgb> = Vec::new();
    capture.frames.clear();
    for pick in picks {
        let image = match video.frame(pick.t) {
            Ok(image) => image,
            Err(error) => {
                eprintln!("nexus-desktop: recording: {error}");
                continue;
            }
        };
        let file = format!("{FRAMES}/{:02}.png", capture.frames.len() + 1);
        std::fs::write(dir.join(&file), image.png()?).map_err(|error| error.to_string())?;
        capture.frames.push(Frame {
            file,
            t: pick.t,
            width: image.width as u32,
            height: image.height as u32,
            reasons: pick.reasons,
        });
        images.push(image);
    }
    if images.is_empty() {
        return Err("no frame could be read from the recording".into());
    }

    let labelled: Vec<(&Rgb, String)> = images
        .iter()
        .zip(&capture.frames)
        .enumerate()
        .map(|(index, (image, frame))| (image, format!("{:02}  {}", index + 1, clock(frame.t))))
        .collect();
    let sheet = contact_sheet(&labelled).png()?;
    std::fs::write(dir.join(SHEET), &sheet).map_err(|error| error.to_string())?;
    std::fs::write(dir.join(SUMMARY), render(capture)).map_err(|error| error.to_string())?;
    write(dir, capture)?;
    Ok(sheet)
}

/// `count` frames from `from` to `to` (seconds) at full resolution, into `<dir>/closer/`; returns their paths.
pub fn frames_between(dir: &Path, from: f64, to: f64, count: usize) -> Result<Vec<PathBuf>, String> {
    let video = Video::open(&dir.join(VIDEO), 0.0)?;
    let end = video.duration();
    let (from, to) = (from.clamp(0.0, end), to.clamp(0.0, end));
    let count = count.clamp(1, 60);
    let out = dir.join("closer");
    std::fs::create_dir_all(&out).map_err(|error| error.to_string())?;
    let mut paths = Vec::new();
    for index in 0..count {
        let t = if count == 1 {
            from
        } else {
            from + (to - from) * index as f64 / (count - 1) as f64
        };
        let image = video.frame(t)?;
        let path = out.join(format!("{t:07.2}s.png"));
        std::fs::write(&path, image.png()?).map_err(|error| error.to_string())?;
        paths.push(path);
    }
    Ok(paths)
}
