//! Timings for the measure script: wall-clock marks in milliseconds since the epoch, from
//! the process's start to the first frame, the app being interactive, and a chat answer's first token. With
//! `NEXUS_DESKTOP_MEASURE_OUT` set, the app writes them there as JSON when its measured send is done
//! ([`MEASURE_DONE`]). The app records them here, so measure reads one file.
//!
//! The file: `{"pid": <pid>, "marks": [{"name": "process", "at": <ms>}, …]}`, in the order they were marked.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// The process started: the first mark, made by [`Metrics::new`].
pub const PROCESS: &str = "process";
/// The main window exists (Nexus GPUI).
pub const WINDOW: &str = "window";
/// The first frame is on screen: GPUI's first presented frame.
pub const FIRST_FRAME: &str = "first-frame";
/// Signed in, with the organization and its recent sessions shown.
pub const INTERACTIVE: &str = "interactive";
/// The measured chat message is sent, its session already made.
pub const SEND: &str = "send";
/// Its answer's first token reached the UI.
pub const FIRST_TOKEN: &str = "first-token";
/// Its answer is complete.
pub const ANSWER_DONE: &str = "answer-done";
/// A second message, on the warm connection, and its first token.
pub const SEND_2: &str = "send-2";
pub const FIRST_TOKEN_2: &str = "first-token-2";
/// The core's own view of a send and of its first token: the request started, and its first token read off the wire.
pub const SEND_CORE: &str = "send-core";
pub const FIRST_TOKEN_CORE: &str = "first-token-core";
/// After the sends, measure switches to a session it seeded (never opened in this launch, so read from the Gateway)
/// and marks the frame that shows its messages; then back to the measured session.
pub const SWITCH: &str = "switch";
pub const SWITCH_DONE: &str = "switch-done";
pub const SWITCH_BACK: &str = "switch-back";
pub const SWITCH_BACK_DONE: &str = "switch-back-done";
/// Then it opens a page it seeded, as the Library's Open does, and marks the frame drawn once the page has loaded
/// and shows its content.
pub const PAGE_OPEN: &str = "page-open";
pub const PAGE_DONE: &str = "page-done";
/// The measured send is over: the app writes the marks.
pub const MEASURE_DONE: &str = "measure-done";

/// `NEXUS_DESKTOP_MEASURE` is set: the app runs one measured chat send by itself and writes its marks.
pub fn enabled() -> bool {
    std::env::var("NEXUS_DESKTOP_MEASURE").is_ok_and(|value| !value.is_empty())
}

pub fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64() * 1000.0)
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
pub struct Mark {
    pub name: String,
    /// Milliseconds since the epoch.
    pub at: f64,
}

#[derive(Debug)]
pub struct Metrics {
    pub started: f64,
    marks: Mutex<Vec<Mark>>,
}

impl Metrics {
    pub fn new(started: f64) -> Metrics {
        Metrics {
            started,
            marks: Mutex::new(vec![Mark {
                name: PROCESS.into(),
                at: started,
            }]),
        }
    }

    pub fn mark(&self, name: &str, at: f64) {
        if let Ok(mut marks) = self.marks.lock() {
            marks.push(Mark { name: name.into(), at });
        }
        // `measure.mjs --log`: every mark as it happens, so a run that never ends shows where it stopped.
        if let Ok(path) = std::env::var("NEXUS_DESKTOP_MEASURE_TRACE") {
            let _ = std::fs::write(&path, self.json().to_string());
        }
    }

    pub fn marks(&self) -> Vec<Mark> {
        self.marks.lock().map(|marks| marks.clone()).unwrap_or_default()
    }

    /// Write the marks to `NEXUS_DESKTOP_MEASURE_OUT`, if set.
    pub fn write(&self) -> Result<(), String> {
        let Ok(path) = std::env::var("NEXUS_DESKTOP_MEASURE_OUT") else {
            return Ok(());
        };
        std::fs::write(&path, self.json().to_string()).map_err(|error| format!("{path}: {error}"))
    }

    /// The file's body.
    fn json(&self) -> serde_json::Value {
        serde_json::json!({ "pid": std::process::id(), "marks": self.marks() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape measure.mjs reads: the process mark first, then the others in order.
    #[test]
    fn writes_the_marks_in_order_after_the_process() {
        let metrics = Metrics::new(1000.0);
        metrics.mark(FIRST_FRAME, 1100.5);
        metrics.mark(INTERACTIVE, 1200.0);
        let json = metrics.json();
        assert_eq!(json["pid"], std::process::id());
        assert_eq!(
            json["marks"],
            serde_json::json!([
                { "name": "process", "at": 1000.0 },
                { "name": "first-frame", "at": 1100.5 },
                { "name": "interactive", "at": 1200.0 },
            ])
        );
    }
}
