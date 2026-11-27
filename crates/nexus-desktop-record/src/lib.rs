//! Screen recordings for agents, for Nexus GPUI: show an agent a problem
//! instead of describing it. The shortcut (⌥⇧S) or the menu-bar item's "Record for an Agent" records the display
//! under the pointer with the narration. Hold Shift and move to draw red ink on the screen; tap Shift twice for a
//! numbered pin. The shortcut again, or a click on the menu-bar item (which counts the time), stops it. Nexus then
//! makes a folder an agent can read (keyframes, a contact sheet, a transcript made on this Mac, a timeline in
//! `summary.md` and `capture.json`) and puts the contact sheet and the folder's path on the clipboard, to paste into
//! Claude Code, Codex or a terminal. Nothing is sent anywhere. macOS only; the organization turns it off with
//! `ScreenCaptureEnabled` (nexus-desktop-core's policy).
//!
//! A shell gives the recorder what only it has through [`Shell`]: the menu-bar item's text, its main thread, the
//! panel and the policy. The shell keeps one [`Recorder`] and calls [`Recorder::toggle`] and [`Recorder::cancel`].
//!
//! Pure rules (tested anywhere): gesture.rs, keyframes.rs, transcript.rs, sheet.rs, summary.rs, model.rs. macOS:
//! screencapture.rs (the video), screen.rs (the display and the pointer), overlay.rs (the ink), system.rs (the app in
//! front, the browser's address, the clipboard), media.rs (frames and speech), process.rs (the folder) and
//! session.rs (a recording from start to folder).

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod gesture;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod keyframes;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod model;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod sheet;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod summary;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod transcript;

#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods, reason = "macOS only: no console window to hide")]
mod media;
#[cfg(target_os = "macos")]
mod overlay;
#[cfg(target_os = "macos")]
mod process;
#[cfg(target_os = "macos")]
mod screen;
#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods, reason = "macOS only: no console window to hide")]
mod screencapture;
#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods, reason = "macOS only: no console window to hide")]
mod session;
#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods, reason = "macOS only: no console window to hide")]
mod system;

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
pub use session::{Recorder, cli};

/// The recordings' folder in an app's data folder: `recordings/`, the newest also as `recordings/latest`. The app
/// uses the same data folder as an older Nexus install, so `nexus recordings` finds every recording.
pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join("recordings")
}

/// Whether this computer records: macOS, and the organization has not turned it off (`allowed`).
pub fn available(allowed: bool) -> bool {
    cfg!(target_os = "macos") && allowed
}

/// What a shell does for a recording. Called from the recorder's own threads.
pub trait Shell: Send + Sync + 'static {
    /// Whether the organization allows screen recordings now (`ScreenCaptureEnabled`).
    fn allowed(&self) -> bool;
    /// Show `title` beside the menu-bar item (the recording's time, then what became of it), or nothing, and
    /// rebuild its menu: it has Finish and Cancel while recording.
    fn status(&self, title: Option<&str>);
    /// Run `work` on the main thread, where AppKit's windows and the clipboard live.
    fn on_main(&self, work: Box<dyn FnOnce() + Send>);
    /// Close the panel, so the recording shows what the person sees under it.
    fn hide_panel(&self);
}

/// Where a shell keeps its recordings and how it names itself in them.
#[derive(Debug, Clone)]
pub struct Config {
    /// The recordings' folder ([`folder`]); None when the app has no data folder.
    pub folder: Option<PathBuf>,
    /// `capture.json`'s `app`: `desktop/<version>` or `desktop-gpui/<version>`.
    pub app: String,
    /// The prefix of its log lines: `nexus-desktop`.
    pub log: &'static str,
}

/// Off macOS: a recorder that never records, so a shell needs no `cfg` of its own.
#[cfg(not(target_os = "macos"))]
#[derive(Clone)]
pub struct Recorder;

#[cfg(not(target_os = "macos"))]
impl Recorder {
    pub fn new(_shell: std::sync::Arc<dyn Shell>, _config: Config) -> Recorder {
        Recorder
    }

    pub fn is_recording(&self) -> bool {
        false
    }

    pub fn toggle(&self) {}

    pub fn cancel(&self) {}
}

/// Off macOS: `--process-recording` and `--frames` say they need macOS and exit 1, so `nexus recordings frames` never
/// starts the app instead. `None` when neither was asked for.
#[cfg(not(target_os = "macos"))]
pub fn cli(_app: &str, log: &str) -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    let asked = args.iter().any(|arg| arg == "--process-recording" || arg == "--frames");
    asked.then(|| {
        eprintln!("{log}: screen recordings for agents need macOS");
        1
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The API a shell calls, the same on every target: `cargo clippy --target aarch64-apple-darwin --all-targets`
    /// type-checks this against the macOS recorder, which no Linux run can start.
    #[allow(dead_code)]
    fn a_shell_calls_the_same_api_on_every_target(shell: std::sync::Arc<dyn Shell>) -> Option<i32> {
        let recorder: Recorder = Recorder::new(
            shell,
            Config {
                folder: Some(folder(Path::new("/data"))),
                app: "desktop/test".into(),
                log: "test",
            },
        );
        let copy = recorder.clone();
        if !copy.is_recording() {
            copy.toggle();
        }
        recorder.cancel();
        cli("desktop/test", "test")
    }

    #[test]
    fn recordings_live_in_the_data_folder_and_only_on_macos() {
        assert_eq!(
            folder(Path::new("/data/xyz.nexus.desktop")),
            PathBuf::from("/data/xyz.nexus.desktop/recordings")
        );
        assert!(!available(false));
        assert_eq!(available(true), cfg!(target_os = "macos"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn off_macos_the_recorder_never_records() {
        struct Never;
        impl Shell for Never {
            fn allowed(&self) -> bool {
                true
            }
            fn status(&self, _title: Option<&str>) {
                panic!("no status off macOS");
            }
            fn on_main(&self, _work: Box<dyn FnOnce() + Send>) {
                panic!("nothing to run off macOS");
            }
            fn hide_panel(&self) {
                panic!("no panel to hide off macOS");
            }
        }
        let recorder = Recorder::new(
            std::sync::Arc::new(Never),
            Config {
                folder: None,
                app: "desktop/test".into(),
                log: "test",
            },
        );
        recorder.toggle();
        assert!(!recorder.is_recording());
        recorder.cancel();
        assert_eq!(cli("desktop/test", "test"), None);
    }
}
