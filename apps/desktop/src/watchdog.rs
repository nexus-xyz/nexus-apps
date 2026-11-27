//! The UI-thread stall log. GPUI draws every frame, spinners included, on one thread, so work there that
//! takes long freezes the window. A task on that thread wakes every [`TICK`]. When it wakes [`STALL`] or
//! more late, the thread was busy: it writes one line to `ui-stalls.log` in this build's data folder, with the labelled
//! work ([`span`]) that ran longest since the last wake.
//!
//! It runs in debug builds, or with `NEXUS_UI_WATCHDOG=1`. Off, [`span`] costs one atomic load and nothing wakes.

use std::cell::RefCell;
use std::io::Write as _;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::App;

/// How often the watchdog wakes: ten wakes a second, only while it runs.
const TICK: Duration = Duration::from_millis(100);
/// A wake this late or later is a stall: three frames at 60 Hz.
const STALL: Duration = Duration::from_millis(50);
/// The log moves to `ui-stalls.log.old` past this size, so it never grows without bound.
const MAX_LOG: u64 = 1024 * 1024;

static RUNNING: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The longest span that ended on this thread since the watchdog last woke.
    static LONGEST: RefCell<Option<(&'static str, Duration)>> = const { RefCell::new(None) };
}

/// Whether this build runs the watchdog: a debug build, or `NEXUS_UI_WATCHDOG=1`.
fn wanted() -> bool {
    static WANTED: OnceLock<bool> = OnceLock::new();
    *WANTED.get_or_init(|| cfg!(debug_assertions) || std::env::var("NEXUS_UI_WATCHDOG").is_ok_and(|value| value == "1"))
}

/// Labelled work on the UI thread: a stall names the longest one that ran during it. Hold it for the work's length.
pub struct Span {
    label: &'static str,
    start: Instant,
}

/// Starts a span named `label`. None while the watchdog is off.
pub fn span(label: &'static str) -> Option<Span> {
    RUNNING.load(Ordering::Relaxed).then(|| Span {
        label,
        start: Instant::now(),
    })
}

impl Drop for Span {
    fn drop(&mut self) {
        record(self.label, self.start.elapsed());
    }
}

fn record(label: &'static str, took: Duration) {
    LONGEST.with(|longest| {
        let mut longest = longest.borrow_mut();
        if longest.is_none_or(|(_, before)| took > before) {
            *longest = Some((label, took));
        }
    });
}

fn take_longest() -> Option<(&'static str, Duration)> {
    LONGEST.with(|longest| longest.borrow_mut().take())
}

/// The log line for a wake `late` past its time, None when that is no stall.
fn stall_line(late: Duration, longest: Option<(&'static str, Duration)>) -> Option<String> {
    if late < STALL {
        return None;
    }
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let during = match longest {
        Some((label, took)) => format!("longest span: {label} {} ms", took.as_millis()),
        None => "no span ran".to_string(),
    };
    Some(format!("{at} UI thread stalled {} ms; {during}", late.as_millis()))
}

fn write(line: &str) {
    eprintln!("nexus-desktop-gpui: {line}");
    let Some(dir) = crate::paths::own_data_dir() else {
        return;
    };
    let path = dir.join("ui-stalls.log");
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_LOG) {
        let _ = std::fs::rename(&path, dir.join("ui-stalls.log.old"));
    }
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
}

/// Starts the watchdog on the UI thread when this build wants it. Call it once, from the application's run closure.
pub fn start(cx: &mut App) {
    if !wanted() || RUNNING.swap(true, Ordering::Relaxed) {
        return;
    }
    cx.spawn(async move |cx| {
        loop {
            let asleep = Instant::now();
            cx.background_executor().timer(TICK).await;
            let late = asleep.elapsed().saturating_sub(TICK);
            if let Some(line) = stall_line(late, take_longest()) {
                write(&line);
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wake_on_time_is_no_stall() {
        assert_eq!(
            stall_line(Duration::from_millis(49), Some(("render", Duration::from_millis(40)))),
            None
        );
    }

    #[test]
    fn a_stall_names_the_longest_span() {
        std::thread::spawn(|| {
            record("render", Duration::from_millis(12));
            record("refresh_code", Duration::from_millis(410));
            record("render", Duration::from_millis(30));
            let line = stall_line(Duration::from_millis(420), take_longest()).unwrap();
            assert!(
                line.ends_with("UI thread stalled 420 ms; longest span: refresh_code 410 ms"),
                "{line}"
            );
            // Each wake starts over.
            assert_eq!(take_longest(), None);
            let line = stall_line(Duration::from_millis(60), take_longest()).unwrap();
            assert!(line.ends_with("UI thread stalled 60 ms; no span ran"), "{line}");
        })
        .join()
        .unwrap();
    }

    #[test]
    fn spans_cost_nothing_while_it_is_off() {
        // Tests never start the watchdog.
        assert!(span("render").is_none());
    }
}
