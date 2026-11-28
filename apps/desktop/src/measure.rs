//! Timings for `npm run measure` (with `--gpui`): the marks and their file come from
//! `nexus_desktop_core::metrics`. Marks are kept only with
//! `NEXUS_DESKTOP_MEASURE` set. The workspace runs the measured send (`workspace/measure_send.rs`).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::Window;
use nexus_desktop_core::metrics::{self, Metrics, now_ms};

static METRICS: OnceLock<Option<Metrics>> = OnceLock::new();

/// The process mark: first thing in `main`.
pub fn start() {
    METRICS.get_or_init(|| metrics::enabled().then(|| Metrics::new(now_ms())));
}

fn metrics() -> Option<&'static Metrics> {
    METRICS.get().and_then(Option::as_ref)
}

/// Measuring: the app runs the measured send and never touches the keychain (`gateway.rs`).
pub fn on() -> bool {
    metrics().is_some()
}

/// The API key measure signs in with, in memory only (`NEXUS_DESKTOP_MEASURE_KEY`).
pub fn key() -> Option<String> {
    on().then(|| std::env::var("NEXUS_DESKTOP_MEASURE_KEY").ok())
        .flatten()
        .filter(|key| !key.is_empty())
}

/// The two sessions measure seeded for the switches (`NEXUS_DESKTOP_MEASURE_SESSIONS`, `<first>,<second>`).
pub fn seeded_sessions() -> Option<(String, String)> {
    let ids = seeded("NEXUS_DESKTOP_MEASURE_SESSIONS")?;
    let (first, second) = ids.split_once(',')?;
    Some((first.to_string(), second.to_string()))
}

/// The page measure seeded to open (`NEXUS_DESKTOP_MEASURE_PAGE`).
pub fn seeded_page() -> Option<String> {
    seeded("NEXUS_DESKTOP_MEASURE_PAGE")
}

fn seeded(name: &str) -> Option<String> {
    on().then(|| std::env::var(name).ok())
        .flatten()
        .filter(|id| !id.is_empty())
}

static PAGE_ARMED: AtomicBool = AtomicBool::new(false);
static PAGE_LOADED: AtomicBool = AtomicBool::new(false);
static PAGE_FRAMED: AtomicBool = AtomicBool::new(false);

/// The measured page is opening: see [`page_event`].
pub fn arm_page() {
    PAGE_LOADED.store(false, Ordering::SeqCst);
    PAGE_FRAMED.store(false, Ordering::SeqCst);
    PAGE_ARMED.store(true, Ordering::SeqCst);
}

/// A web view's page finished loading (`loaded`) or it received a frame (`framed`), `web/mod.rs`. Once the armed page
/// has both, the frame after the next drawn one marks [`metrics::PAGE_DONE`], and the measure ends.
pub fn page_event(window: &mut Window, loaded: bool, framed: bool) {
    if !PAGE_ARMED.load(Ordering::SeqCst) {
        return;
    }
    if loaded {
        PAGE_LOADED.store(true, Ordering::SeqCst);
    }
    if framed {
        PAGE_FRAMED.store(true, Ordering::SeqCst);
    }
    if PAGE_LOADED.load(Ordering::SeqCst)
        && PAGE_FRAMED.load(Ordering::SeqCst)
        && PAGE_ARMED.swap(false, Ordering::SeqCst)
    {
        window.on_next_frame(|window, _| {
            window.on_next_frame(|_, _| {
                mark(metrics::PAGE_DONE);
                finish();
            })
        });
    }
}

pub fn mark(name: &str) {
    mark_at(name, now_ms());
}

pub fn mark_at(name: &str, at: f64) {
    if let Some(metrics) = metrics() {
        metrics.mark(name, at);
    }
}

/// Mark `name` unless it is marked already: the first window and the first frame. The compositor may show the first
/// frame after the measured send is over: the file is written again with it.
pub fn mark_once(name: &str) {
    if let Some(metrics) = metrics() {
        let marks = metrics.marks();
        if marks.iter().any(|mark| mark.name == name) {
            return;
        }
        metrics.mark(name, now_ms());
        if marks.iter().any(|mark| mark.name == metrics::MEASURE_DONE) {
            write(metrics);
        }
    }
}

fn write(metrics: &Metrics) {
    if let Err(error) = metrics.write() {
        log::warn!("measure: {error}");
    }
}

/// The measured page journey (`workspace/measure_send.rs`): `NEXUS_DESKTOP_MEASURE_PAGES` lists the pages to open, in
/// order, as `id@version` (`doc@1,heavy@1,doc@1,doc@2`). Empty when not measuring or not set.
pub fn pages() -> Vec<(String, u32)> {
    if !on() {
        return Vec::new();
    }
    std::env::var("NEXUS_DESKTOP_MEASURE_PAGES")
        .unwrap_or_default()
        .split(',')
        .filter_map(|step| {
            let (id, version) = step.trim().rsplit_once('@')?;
            Some((id.to_string(), version.parse().ok()?))
        })
        .collect()
}

/// How long the journey rests on each opened page before the next, in milliseconds
/// (`NEXUS_DESKTOP_MEASURE_PAGE_DWELL_MS`, 3000 by default: the page's picture is taken 1.5 s after it loads).
pub fn page_dwell() -> u64 {
    std::env::var("NEXUS_DESKTOP_MEASURE_PAGE_DWELL_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .unwrap_or(3000)
}

/// How long the pointer rests on a page's card before the click, in milliseconds
/// (`NEXUS_DESKTOP_MEASURE_PAGE_HOVER_MS`; 0, the default, clicks with no hover).
pub fn page_hover() -> u64 {
    std::env::var("NEXUS_DESKTOP_MEASURE_PAGE_HOVER_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .unwrap_or(0)
}

/// The journey's page under way, from 1; 0 between pages and when there is no journey.
static PAGE_STEP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn set_page_step(step: usize) {
    PAGE_STEP.store(step, std::sync::atomic::Ordering::Relaxed);
}

/// Mark `event` once for the journey's page under way, as `page-<step>-<event>`. Nothing between pages, or when not
/// measuring: the web view calls it on every frame.
pub fn page_mark(event: &str) {
    if !on() {
        return;
    }
    let step = PAGE_STEP.load(std::sync::atomic::Ordering::Relaxed);
    if step > 0 {
        mark_once(&format!("page-{step}-{event}"));
    }
}

/// Whether `name` is marked.
pub fn marked(name: &str) -> bool {
    metrics().is_some_and(|metrics| metrics.marks().iter().any(|mark| mark.name == name))
}

/// The measured send is over: mark it and write the file measure waits for.
pub fn finish() {
    if let Some(metrics) = metrics() {
        metrics.mark(metrics::MEASURE_DONE, now_ms());
        write(metrics);
    }
}
