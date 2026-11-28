//! Web pages inside the app: Nexus artifacts and Pages now, and the design canvas later. They render in CEF (Chromium's
//! embedding framework) off screen, in a separate web host process. Chromium paints each frame into a pixel buffer,
//! and GPUI draws that buffer as an image. So a page scrolls, clips and sits under the app's own menus like any other
//! element, the same on every platform, Wayland included, with no second native window to keep in place.
//!
//! The pieces:
//! - [`run_subprocess`] and [`run_host`]: this binary is also the web host process and Chromium's helper processes
//!   (renderer, GPU, network), started again with an argument. `main` calls both first.
//! - `host.rs` and `handlers.rs`: the host process, which owns CEF and every page, and what CEF calls back into there.
//! - `client.rs`: the app's side of the host. The first page starts it; it exits a while after the last page closes.
//! - `protocol.rs`: what the two say to each other.
//! - `cpu.rs`: frames through a memory file both map, of which each frame rewrites only what changed.
//! - `frames/`: GPU frames. `mod.rs` holds the switches every system shares; `linux.rs`, on Linux, frames as GPU
//!   buffers the app draws as textures, with no copy through the CPU.
//! - [`WebView`]: one page, an entity the UI renders.
//! - Input a browser window gets from the system, which the app passes on: `gestures.rs` (the back and forward
//!   buttons, the cursor), `page_drag.rs` (a drag inside the page), `file_drag.rs` (files dragged onto a
//!   session browser's page) and `popup.rs` (a `<select>`'s open list).
//!
//! Prototype limits: Chromium's sandbox opt-in and Linux only (`host::sandbox_on`), frames copied through the CPU and a
//! memory file on macOS and Windows (GPU frames are Linux only), a `<select>`'s open list is drawn on CPU frames only,
//! no context menu, no input method (composed text), and the page's `window.nexus` bridge is not wired yet.

mod client;
mod cpu;
#[cfg(windows)]
mod d3d11_selftest;
#[cfg(target_os = "linux")]
mod dmabuf;
mod dwell;
mod file_drag;
#[cfg(target_os = "linux")]
mod frames;
mod gestures;
mod handlers;
mod host;
mod keys;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(test)]
mod mouse_tests;
mod page_drag;
mod popup;
#[cfg(target_os = "macos")]
mod process_limits;
mod protocol;
mod ready;
#[cfg(test)]
mod ready_tests;
mod resample;
mod retention;
#[cfg(target_os = "linux")]
mod ring;
#[cfg(target_os = "macos")]
mod selftest;
#[cfg(any(target_os = "macos", windows, test))]
mod selftest_pattern;
mod servo;
mod snapshot;
#[cfg(test)]
mod veil_tests;
pub(crate) use retention::RetainedPage;
#[cfg(test)]
pub(crate) use retention::fixture_page;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};

use gpui_kit::prelude::*;
use gpui_kit::*;
use smallvec::SmallVec;

pub(crate) use self::protocol::Cursor;
use self::protocol::{Button, Command, FRAME_RATES, MouseAction};

/// A page whose new frame the app has not drawn for this long is not on screen (a hidden or minimised window, where
/// the compositor stops asking for frames): Chromium is told it is hidden, and stops painting and animating, until the
/// app draws it at the screen's pace again: two frames closer than [`SHOWN_GAP`]. A compositor still sends a hidden
/// window the odd frame, which alone must not wake the page.
const HIDDEN_AFTER: Duration = Duration::from_millis(500);
const SHOWN_GAP: Duration = Duration::from_millis(100);

/// How long after its last new frame or input a paced page keeps getting a frame each time the app draws. Past it,
/// the host asks for one now and then (`host.rs`'s `IDLE_FRAME`), and a new frame starts the pace again.
const LIVELY_FOR: Duration = Duration::from_millis(300);

/// A lively paced page whose next frame the app has not drawn by this share of a frame past its time gets its frame
/// anyway (`WebView::fill_frame`): when the compositor skips a refresh, Chromium still paints for it, with that time's
/// share of a touchpad scroll, as it does on its own clock in a browser. The next refresh then shows where the page is
/// at its time. At most [`FILLS`] in a row.
const FILL_SLACK: f32 = 0.1;
const FILLS: u8 = 2;
/// The app's frame gaps that give its pace, and how many it takes to know it.
const PACE_GAPS: usize = 15;
const PACE_KNOWN: usize = 8;

/// Send `token` to `wake` at `at`, from a thread of its own: GPUI's timers fire up to 10ms late on a busy machine
/// (measured 2026-11-08), most of a frame.
fn alarm(at: Instant, token: u64, wake: UnboundedSender<u64>) {
    static ALARMS: std::sync::OnceLock<
        Option<std::sync::Mutex<std::sync::mpsc::Sender<(Instant, u64, UnboundedSender<u64>)>>>,
    > = std::sync::OnceLock::new();
    let alarms = ALARMS.get_or_init(|| {
        let (set, alarms) = std::sync::mpsc::channel::<(Instant, u64, UnboundedSender<u64>)>();
        std::thread::Builder::new()
            .name("web-fill-alarms".into())
            .spawn(move || {
                let mut set_for: Vec<(Instant, u64, UnboundedSender<u64>)> = Vec::new();
                loop {
                    let next = set_for.iter().map(|(at, ..)| *at).min();
                    let wait = next.map_or(Duration::from_secs(3600), |at| {
                        at.saturating_duration_since(Instant::now())
                    });
                    match alarms.recv_timeout(wait) {
                        Ok(alarm) => set_for.push(alarm),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                    let now = Instant::now();
                    set_for.retain(|(at, token, wake)| {
                        if *at > now {
                            return true;
                        }
                        let _ = wake.unbounded_send(*token);
                        false
                    });
                }
            })
            .ok()
            .map(|_| std::sync::Mutex::new(set))
    });
    if let Some(alarms) = alarms {
        let _ = alarms.lock().expect("web fill alarms").send((at, token, wake));
    }
}

/// The pace of the frames the app draws for a lively paced page, and the frames filled in where one came late.
#[derive(Default)]
struct Pace {
    /// The display's refresh interval: the pace until enough gaps are measured.
    prior: Option<Duration>,
    gaps: std::collections::VecDeque<Duration>,
    drawn: Option<Instant>,
    /// When the last frame was filled in, and how many were since the app drew.
    filled_at: Option<Instant>,
    filled: u8,
}

impl Pace {
    /// The app's frame interval: the median gap, once there are enough, and the display's interval until then. A
    /// skipped refresh's gap is above it.
    fn interval(&self) -> Option<Duration> {
        if self.gaps.len() < PACE_KNOWN {
            return self.prior;
        }
        let mut gaps: Vec<Duration> = self.gaps.iter().copied().collect();
        gaps.sort_unstable();
        Some(gaps[gaps.len() / 2])
    }

    /// The app draws now. Whether a frame was filled in less than half a frame ago: Chromium paints for this refresh
    /// already.
    fn draw(&mut self, now: Instant) -> bool {
        if let Some(gap) = self.drawn.map(|at| now.duration_since(at))
            && gap < Duration::from_millis(100)
        {
            if self.gaps.len() == PACE_GAPS {
                self.gaps.pop_front();
            }
            self.gaps.push_back(gap);
        }
        self.drawn = Some(now);
        self.filled = 0;
        self.interval()
            .zip(self.filled_at)
            .is_some_and(|(interval, at)| now.duration_since(at) < interval / 2)
    }
}

/// Whether to log what makes a page's frames late: `NEXUS_WEB_TRACE=1`. For finding a stutter.
pub(super) fn trace() -> bool {
    static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("NEXUS_WEB_TRACE").is_ok_and(|value| value == "1"))
}

/// `NEXUS_WEB_TRACE`'s summary of a page's frames: every few seconds while it animates, how evenly they came. A gap
/// over [`FrameStats::PAUSE`] is the page having nothing new to paint, not a stutter, and is left out.
#[derive(Default)]
struct FrameStats {
    /// The gaps, in microseconds, since `since`.
    gaps: Vec<u32>,
    since: Option<Instant>,
    /// This page's frames filled in since `since` (`WebView::fill_frame`).
    filled: u32,
    /// This page's CPU frames written into the image on screen since `since` (`WebView::write_changes`).
    in_place: u32,
}

impl FrameStats {
    const EVERY: Duration = Duration::from_secs(2);
    const PAUSE: Duration = Duration::from_millis(100);

    fn frame(&mut self, gap: Duration, label: &str) {
        let now = Instant::now();
        let since = *self.since.get_or_insert(now);
        if gap < Self::PAUSE {
            self.gaps.push(gap.as_micros() as u32);
        }
        if now.duration_since(since) < Self::EVERY {
            return;
        }
        if let Some(summary) = Self::summary(&mut self.gaps) {
            eprintln!(
                "nexus web trace: {label}: {} ({} filled in, {} in place) in {:.1}s",
                summary,
                std::mem::take(&mut self.filled),
                std::mem::take(&mut self.in_place),
                now.duration_since(since).as_secs_f32()
            );
        }
        self.gaps.clear();
        self.filled = 0;
        self.in_place = 0;
        self.since = None;
    }

    /// The gaps' median, 95th percentile and longest, and how many were late: over 1.5 times the median.
    fn summary(gaps: &mut [u32]) -> Option<String> {
        if gaps.len() < 10 {
            return None;
        }
        gaps.sort_unstable();
        let at = |fraction: f32| gaps[((gaps.len() - 1) as f32 * fraction) as usize] as f32 / 1000.0;
        let median = at(0.5);
        let late = gaps.iter().filter(|gap| **gap as f32 / 1000.0 > median * 1.5).count();
        Some(format!(
            "{} frames, gap median {median:.1}ms, p95 {:.1}ms, max {:.1}ms, {late} late",
            gaps.len(),
            at(0.95),
            at(1.0)
        ))
    }
}

/// Whether pages paint at the app's pace, one frame per frame the app draws (`Command::BeginFrame`).
/// On by default only when the page asks for GPU frames on Linux, the one case measured. Linux CPU frames stay
/// unpaced: paced, they scroll less evenly. macOS and Windows keep Chromium's own timer until a run
/// on each measures them.
/// `NEXUS_WEB_PACED=1` turns it on anywhere, `=0` turns it off: Chromium then paints on its own timer, at
/// [`frame_rate`]. `client::HostPage::open` turns it off again for a page that gets no GPU frames, unless forced.
fn paced() -> bool {
    match std::env::var("NEXUS_WEB_PACED").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => default_paced(),
    }
}

/// `NEXUS_WEB_PACED=1`: pages are paced even with CPU frames.
fn paced_forced() -> bool {
    std::env::var("NEXUS_WEB_PACED").is_ok_and(|paced| paced == "1")
}

#[cfg(target_os = "linux")]
fn default_paced() -> bool {
    frames::wanted()
}

#[cfg(not(target_os = "linux"))]
fn default_paced() -> bool {
    false
}

/// Whether touchpad scrolling goes to paced pages once a frame, resampled (`resample.rs`). `NEXUS_WEB_RESAMPLE=0`
/// sends each event as it comes, as before.
fn resampled() -> bool {
    !std::env::var("NEXUS_WEB_RESAMPLE").is_ok_and(|value| value == "0")
}

/// Frames a second a page paints at most, paced or not: the display's rate (`refresh` is its interval), so a 120 Hz
/// screen gets 120. `NEXUS_WEB_FPS` overrides it. Paced, it is Chromium's begin frame interval; on its own timer, the
/// timer's rate. Either way, Chromium hands over no frames closer together than it.
fn frame_rate(refresh: Option<Duration>) -> i32 {
    pick_frame_rate(std::env::var("NEXUS_WEB_FPS").ok().as_deref(), refresh)
}

/// [`frame_rate`] from `NEXUS_WEB_FPS`'s value and the display's interval: 60 when neither says, and always within
/// what CEF takes.
fn pick_frame_rate(fps: Option<&str>, refresh: Option<Duration>) -> i32 {
    let display = refresh
        .filter(|interval| !interval.is_zero())
        .map(|interval| (1. / interval.as_secs_f64()).round().min(f64::from(i32::MAX)) as i32);
    fps.and_then(|fps| fps.trim().parse::<i32>().ok())
        .or(display)
        .unwrap_or(60)
        .clamp(*FRAME_RATES.start(), *FRAME_RATES.end())
}

/// Where [`frame_rate`] came from, for `NEXUS_WEB_TRACE`.
fn frame_rate_source(refresh: Option<Duration>) -> String {
    match refresh {
        _ if std::env::var("NEXUS_WEB_FPS").is_ok_and(|fps| fps.trim().parse::<i32>().is_ok()) => {
            "NEXUS_WEB_FPS".into()
        }
        Some(interval) if !interval.is_zero() => format!("display {:.2} Hz", 1. / interval.as_secs_f64()),
        _ => "display rate unknown".into(),
    }
}

/// Start the web engine now, when a page is likely to open soon: it takes about half a second to start, which the
/// page then skips. It exits again if no page opens within a few minutes. Call it when the person shows they will
/// open a page (a pointer on an artifact's card, the Artifacts list), not merely when one could open.
pub fn warm() {
    if !servo::chosen() {
        client::warm();
    }
}

pub use dwell::warm_on_hover;
pub use ready::{prepare, prepare_on_hover_then};

/// A window of the app took or lost the focus: in the background, an idle web engine exits sooner.
pub fn app_focused(focused: bool) {
    client::focused(focused);
}

/// Run a Chromium helper process, if this is one. Returns its exit code then; `None` otherwise.
pub fn run_subprocess() -> Option<i32> {
    // Read from the plain arguments: the app's own process must not load CEF.
    if !std::env::args().any(|arg| arg.starts_with("--type=")) {
        return None;
    }
    // On macOS, CEF's framework loads first, from the helper's bundle; it stays loaded until the process exits.
    #[cfg(target_os = "macos")]
    let _framework = match std::env::current_exe()
        .ok()
        .and_then(|exe| mac::load_framework(&exe, true))
    {
        Some(loader) => loader,
        None => return Some(1),
    };
    // Before any other CEF call: it picks the API version this binary was built against.
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let args = cef::args::Args::new();
    let mut app = handlers::NexusApp::new(ozone(), false);
    Some(cef::execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    ))
}

/// Run the web host process, if this is it. Returns its exit code then; `None` in the app's own process.
pub fn run_host() -> Option<i32> {
    std::env::args().any(|arg| arg == protocol::HOST_ARG).then(host::run)
}

/// Which Ozone platform (Chromium's display backend) its processes use. Off screen Chromium draws no window, so on
/// Linux it needs no display connection: `headless`. Chromium's `wayland` backend also drops the scale the app asks
/// for and paints at 1x, which GPUI then stretches into a blur at 1.25. Headless has no system clipboard, so copy and
/// paste go through the app (not wired yet). `NEXUS_CEF_OZONE` overrides (`x11`, `wayland`, `headless`).
fn ozone() -> Option<String> {
    if let Ok(ozone) = std::env::var("NEXUS_CEF_OZONE") {
        return Some(ozone).filter(|ozone| !ozone.is_empty());
    }
    cfg!(target_os = "linux").then(|| "headless".into())
}

/// Where CEF's files are: beside the binary in a shipped build, else the development copy `build.rs` found. Not used
/// on macOS, where the framework holds them.
fn cef_resources(exe: &std::path::Path) -> PathBuf {
    let beside = exe.parent().map(|dir| dir.to_path_buf()).unwrap_or_default();
    if beside.join("icudtl.dat").exists() {
        return beside;
    }
    option_env!("NEXUS_CEF_RESOURCES").map(PathBuf::from).unwrap_or(beside)
}

/// A frame the engine painted: BGRA pixels, `width` by `height` device pixels, and its number (`cpu.rs`; 0 for none).
#[derive(Default)]
pub(crate) struct Frame {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    seq: u64,
    /// The frame this one follows and the rects that changed since: frame `seq - 1`'s view can write only those into
    /// its image (`Window::update_image`). None: unknown, all of it.
    changed: Option<(u64, Vec<cpu::Rect>)>,
}

/// What the engine reported for a page, read by its view on the next look.
#[derive(Default)]
pub(crate) struct Shared {
    /// The view's size in logical pixels, and the screen's scale.
    size: (i32, i32),
    scale: f32,
    frame: Option<Frame>,
    /// A frame on the GPU, instead (`frames/linux.rs`).
    #[cfg(target_os = "linux")]
    texture: Option<Arc<gpui_wgpu::SharedTexture>>,
    /// The app imported a GPU frame for the page.
    #[cfg(target_os = "linux")]
    gpu_framed: bool,
    /// When the page last finished loading, for the first GPU frame's wait (`frames::watch`).
    #[cfg(target_os = "linux")]
    loaded: Option<Instant>,
    /// The app could not import a GPU frame, or none came: the page opens again with CPU frames.
    gpu_failed: bool,
    title: Option<String>,
    cursor: Cursor,
    /// Links the page asked to open in a new window.
    popups: Vec<String>,
    /// Why the page stopped (its renderer crashed, or the engine did), when it did.
    stopped: Option<String>,
    /// DevTools Protocol messages from the page.
    cdp: Vec<String>,
    /// Where the page's main frame is, its loading state, popups it opened (a session browser's) and whether it closed
    /// itself.
    address: Option<String>,
    loading: Option<Loading>,
    load_failure: Option<Option<u16>>,
    opened: Vec<Opened>,
    closed: bool,
    /// Addresses the page handed its result over by (`protocol::Command::Open`'s `handoff`), with their fragments.
    handoffs: Vec<String>,
    /// Something changed since the view last looked.
    dirty: bool,
    /// Wakes the view to look.
    wake: Option<UnboundedSender<()>>,
    /// No view consumes transient events while the primary window is closed.
    parked: bool,
}

pub(crate) type SharedRef = Arc<Mutex<Shared>>;

/// A page's loading state, and whether its history has a page behind or ahead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Loading {
    pub loading: bool,
    pub back: bool,
    pub forward: bool,
}

/// A popup a session browser's page opened, for the app to show with [`WebView::adopt`].
#[derive(Clone)]
pub struct Opened {
    /// Only native user input may foreground a popup. Tool completion timing is not a focus signal.
    pub(crate) user_gesture: bool,
    id: u32,
    shared: SharedRef,
}

/// The DevTools ids of [`WebView::call`]s start here, apart from any other's (32-bit, as Chromium takes them), and
/// the first id of a [`Startup`]'s messages (the page and runtime domains, the binding, the script, the first address).
const CALL_IDS: u64 = 1 << 28;
const SCRIPT_ID: u64 = 3;

/// The id and answer of a DevTools reply: its `result`, or its error's message.
fn answer_of(message: &str) -> Option<(u64, Result<serde_json::Value, String>)> {
    let mut message: serde_json::Value = serde_json::from_str(message).ok()?;
    let id = message.get("id")?.as_u64()?;
    if let Some(error) = message.get("error") {
        let text = error["message"]
            .as_str()
            .unwrap_or("The page refused the call.")
            .to_string();
        return Some((id, Err(text)));
    }
    Some((id, Ok(message["result"].take())))
}

/// A session browser's popup opens at this size, as the Nexus app's "Browser sign-in" window (`handlers.rs` has the
/// engine's side).
pub const POPUP_SIZE: (f32, f32) = (520., 680.);

/// The rects to write into an image that shows frame `image_seq` at `image_size`, to make it show `frame`. None: a
/// new image instead.
fn changes_onto(image_seq: u64, image_size: (i32, i32), frame: &Frame) -> Option<&[cpu::Rect]> {
    let (since, rects) = frame.changed.as_ref()?;
    let same_size = (frame.width as i64, frame.height as i64) == (image_size.0 as i64, image_size.1 as i64);
    (image_seq != 0 && *since == image_seq && same_size).then_some(rects.as_slice())
}

impl Shared {
    /// Mark a change and wake the view.
    pub(crate) fn changed(&mut self) {
        self.dirty = true;
        if let Some(wake) = &self.wake {
            let _ = wake.unbounded_send(());
        }
    }
}

/// Something a page did that its owner may want to know.
pub enum WebEvent {
    TitleChanged(String),
    /// A DevTools Protocol message from the page, as JSON text: a reply to [`WebView::send_cdp`], or an event. For the
    /// agent's browser tools.
    Cdp(#[allow(dead_code)] String),
    /// The page's main frame went to this address.
    Address(String),
    Loading(Loading),
    /// The main document failed; `Some(status)` is an HTTP response, `None` a network error.
    LoadFailed(Option<u16>),
    /// A session browser's page opened a popup: show it with [`WebView::adopt`].
    Popup(Opened),
    /// The page closed itself (a popup's `window.close()`).
    Closed,
    /// A veiled view ([`WebView::veil`]) shows its page now: once, when its own document drew.
    Shown,
    /// A challenge page ([`WebView::challenge`]) went to its handoff address, here with its fragment. The page stayed.
    Handoff(String),
}

impl EventEmitter<WebEvent> for WebView {}

/// Whether a view draws the page's frames, and whether it draws the white engine background under them. A veiled view
/// draws no frame until its page shows, and never the white: its owner draws its own background under it.
fn veil_state(veiled: bool, revealed: bool) -> (bool, bool) {
    (!veiled || revealed, !veiled)
}

/// The engine a page runs in.
enum Engine {
    Cef(client::HostPage),
    /// Servo instead, when `NEXUS_WEB_ENGINE=servo` (`servo.rs`).
    Servo(servo::ServoPage),
}

/// One web page, drawn as an element.
pub struct WebView {
    url: String,
    /// The session whose browser tab this is (`protocol::Command::Open`'s `browser`), if it is one, and its sign-in's
    /// website profile.
    browser: Option<String>,
    profile: Option<protocol::WebProfile>,
    /// A popup the host opened, which this view shows instead of opening a page.
    adopted: Option<u32>,
    /// The script each of its documents runs first, if any.
    startup: Option<Startup>,
    /// A read-only document ([`WebView::document`]).
    document: bool,
    /// No background under the page ([`WebView::clear_with_startup`]).
    clear: bool,
    /// A challenge page's handoff address ([`WebView::challenge`]).
    handoff: Option<String>,
    engine: Option<Engine>,
    shared: SharedRef,
    image: Option<Arc<RenderImage>>,
    /// The frame `image`'s tile in the atlas shows: newer than the pixels `image` was made with once the view wrote
    /// changes into it (`write_changes`). 0: none.
    image_seq: u64,
    /// The newest GPU frame, drawn instead of `image` when there is one.
    #[cfg(target_os = "linux")]
    texture: Option<Arc<gpui_wgpu::SharedTexture>>,
    focus: FocusHandle,
    focused: bool,
    /// Where the page is in the window, from the last layout.
    bounds: Bounds<Pixels>,
    /// The mouse buttons held, for the move events' flags.
    buttons: u32,
    /// What `mouse` sent, with its flags: tests open no engine.
    #[cfg(test)]
    sent: Vec<(MouseAction, u32)>,
    /// Why there is no page, when there is none.
    problem: Option<SharedString>,
    /// [`WebView::call`]s waiting for their answer, by DevTools id.
    calls: std::collections::HashMap<u64, futures::channel::oneshot::Sender<Result<serde_json::Value, String>>>,
    next_call: u64,
    /// The title and address last told to the owner.
    title: Option<String>,
    address_told: Option<String>,
    /// When the app last drew the page, since when a frame waits to be drawn, and whether Chromium was told the page
    /// is hidden.
    drawn: Instant,
    waiting: Option<Instant>,
    hidden: bool,
    /// Whether the page paints at the app's pace, and until when the app asks it for a frame each time it draws.
    paced: bool,
    lively: Instant,
    /// When the last new frame came, and the gaps between frames lately, for `NEXUS_WEB_TRACE`.
    last_frame: Instant,
    frame_stats: FrameStats,
    /// What a scroll left over below a whole pixel, sent with the next one.
    wheel_rest: Point<f32>,
    /// Files dragged from the system over a session browser's page.
    file_drag: file_drag::FileDrag,
    /// Whether a touchpad pinch reaches the page ([`WebView::set_pinch_zoom`]), and what it left below a whole pixel.
    pinch_zoom: bool,
    pinch_rest: f32,
    /// A paced page's touchpad scrolling, sent once a frame (`resample.rs`), and where the pointer was for it.
    resampler: resample::Resampler,
    touchpad_at: (i32, i32, u32),
    /// The display's refresh interval, the frame rate the page was last given ([`frame_rate`]), and the watch on the
    /// window's display that keeps it.
    refresh: Option<Duration>,
    frame_rate: i32,
    _display: Subscription,
    /// The app's pace for a lively paced page, and the timer that fills in a frame it skips.
    pace: Pace,
    /// The current fill-in's number (a newer one replaces it), and how its alarm wakes the view.
    fill_token: u64,
    fill_wake: Option<UnboundedSender<u64>>,
    _fills: Option<Task<()>>,
    /// The page as it last looked (`snapshot.rs`), drawn until the page drew itself or until [`snapshot::SHOW_AT_MOST`]
    /// passed; and the file that keeps the page's next picture, until it is taken.
    placeholder: Option<(Arc<RenderImage>, Instant)>,
    snapshot: Option<PathBuf>,
    /// Since the page went to its own address (past the blank startup document): whether it drew, and whether it
    /// finished loading.
    drew: bool,
    loaded: bool,
    /// Draws no frame, and no white, until the page shows ([`WebView::veil`]); and whether it shows now.
    veiled: bool,
    revealed: bool,
    /// A page opened ahead (`ready.rs`), kept hidden until its own document loaded.
    held: bool,
    _tick: Task<()>,
}

/// A script a page runs before its own, on each of its documents, and the function it may call to tell the app
/// something (`Runtime.addBinding`). Each call comes to the owner as a `Runtime.bindingCalled` [`WebEvent::Cdp`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Startup {
    pub source: &'static str,
    pub binding: Option<&'static str>,
}

/// The DevTools messages that put `startup` in place on a blank page, from id [`SCRIPT_ID`]: the page domain first
/// (without it, the engine keeps the script but runs it on no document), the binding, then the script.
fn startup_messages(startup: Startup) -> Vec<String> {
    let mut messages = vec![("Page.enable", serde_json::json!({}))];
    if let Some(name) = startup.binding {
        messages.push(("Runtime.enable", serde_json::json!({})));
        messages.push(("Runtime.addBinding", serde_json::json!({ "name": name })));
    }
    messages.push((
        "Page.addScriptToEvaluateOnNewDocument",
        serde_json::json!({ "source": startup.source }),
    ));
    (SCRIPT_ID..)
        .zip(messages)
        .map(|(id, (method, params))| serde_json::json!({ "id": id, "method": method, "params": params }).to_string())
        .collect()
}

impl WebView {
    /// A page with `startup` in place before its first document loads. On Servo, which has no DevTools Protocol here,
    /// the page opens without it.
    pub fn with_startup(url: &str, startup: Startup, window: &mut Window, cx: &mut Context<Self>) -> WebView {
        Self::build(url, None, None, Some(startup), None, false, None, window, cx)
    }

    /// [`WebView::with_startup`], see-through: the engine has no background, and the view draws none under the page
    /// and nothing of it until its own document drew ([`WebView::veil`]). What the page leaves unpainted shows the
    /// owner's surface: an answer's math sits on the transcript as its text does (`rich_text`).
    pub fn clear_with_startup(url: &str, startup: Startup, window: &mut Window, cx: &mut Context<Self>) -> WebView {
        Self::attach(
            url,
            None,
            None,
            Some(startup),
            None,
            false,
            None,
            None,
            true,
            window,
            cx,
        )
    }

    /// Whether the page has no background of its own ([`WebView::clear_with_startup`]).
    #[cfg(test)]
    pub fn is_clear(&self) -> bool {
        self.clear
    }

    /// A read-only document at `url` (a `data:` page or a file): no JavaScript, popups or downloads, and it never
    /// leaves `url`. The Files tool's HTML and video previews, as the Nexus app's `sandbox=""` frame and player.
    pub fn document(url: &str, window: &mut Window, cx: &mut Context<Self>) -> WebView {
        Self::build(url, None, None, None, None, true, None, window, cx)
    }

    /// A page that hands its result over by going to `handoff` (guest chat's challenge page, `crate::guest`): the
    /// engine cancels that navigation and the view emits [`WebEvent::Handoff`] with the address and its fragment.
    pub fn challenge(url: &str, handoff: &str, window: &mut Window, cx: &mut Context<Self>) -> WebView {
        Self::build(
            url,
            None,
            None,
            None,
            None,
            false,
            Some(handoff.to_string()),
            window,
            cx,
        )
    }

    /// A Code session's browser tab: only web addresses, and its sign-in popups open in a window of the app's
    /// ([`WebEvent::Popup`]), as in the Nexus app.
    pub fn browser(
        url: &str,
        session: &str,
        profile: nexus_browser::profile::Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> WebView {
        let startup = Startup {
            source: nexus_browser::PAGE_SCRIPT,
            binding: None,
        };
        Self::build(
            url,
            Some(session.to_string()),
            Some(profile.into()),
            Some(startup),
            None,
            false,
            None,
            window,
            cx,
        )
    }

    /// The popup a page opened ([`WebEvent::Popup`]).
    pub fn adopt(opened: Opened, window: &mut Window, cx: &mut Context<Self>) -> WebView {
        let url = opened
            .shared
            .lock()
            .expect("web view state")
            .address
            .clone()
            .unwrap_or_default();
        Self::build(&url, None, None, None, Some(opened), false, None, window, cx)
    }

    fn build(
        url: &str,
        browser: Option<String>,
        profile: Option<protocol::WebProfile>,
        startup: Option<Startup>,
        opened: Option<Opened>,
        document: bool,
        handoff: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> WebView {
        Self::attach(
            url, browser, profile, startup, opened, document, handoff, None, false, window, cx,
        )
    }

    fn attach(
        url: &str,
        browser: Option<String>,
        profile: Option<protocol::WebProfile>,
        startup: Option<Startup>,
        opened: Option<Opened>,
        document: bool,
        handoff: Option<String>,
        retained: Option<RetainedPage>,
        clear: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> WebView {
        let (wake, mut woken) = unbounded::<()>();
        let adopted = opened.as_ref().map(|opened| opened.id);
        // A page with a startup script takes the one opened ahead for it, if one waits: it only navigates. Pages are
        // opened ahead opaque, so a clear one never takes one.
        let ready = match (&retained, &opened, startup) {
            (None, None, Some(startup))
                if browser.is_none() && profile.is_none() && !document && !clear && !cfg!(test) =>
            {
                client::take_ready(startup)
            }
            _ => None,
        };
        let held = ready.is_some();
        let (engine, retained_shared, next_call) = match (retained, ready) {
            (Some(saved), _) => (Some(Engine::Cef(saved.page)), Some(saved.shared), saved.next_call),
            (None, Some((page, shared))) => (Some(Engine::Cef(page)), Some(shared), CALL_IDS),
            (None, None) => (None, None, CALL_IDS),
        };
        let shared: SharedRef = if let Some(shared) = retained_shared {
            {
                let mut state = shared.lock().expect("web view state");
                state.parked = false;
                state.wake = Some(wake);
                state.changed();
            }
            shared
        } else {
            match opened {
                Some(opened) => {
                    {
                        // What came before the view did is taken on its first look.
                        let mut shared = opened.shared.lock().expect("web view state");
                        shared.wake = Some(wake);
                        shared.changed();
                    }
                    opened.shared
                }
                None => Arc::new(Mutex::new(Shared {
                    size: (800, 600),
                    scale: window.scale_factor(),
                    wake: Some(wake),
                    ..Default::default()
                })),
            }
        };
        // The engine wakes this when something changed; several wakes at once are one look.
        let tick = cx.spawn_in(window, async move |this, cx| {
            while woken.next().await.is_some() {
                while woken.try_recv().is_ok() {}
                if this
                    .update_in(cx, |this, window, cx| this.take_changes(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        // The window moved to another display, or the display's rate changed: the page follows.
        let display = cx.observe_window_display(window, |this, window, cx| {
            this.display_changed(window.refresh_interval(cx));
        });
        let refresh = window.refresh_interval(cx);
        let mut view = WebView {
            url: url.to_string(),
            browser,
            profile,
            adopted,
            startup,
            document,
            clear,
            handoff,
            engine,
            shared,
            image: None,
            image_seq: 0,
            #[cfg(target_os = "linux")]
            texture: None,
            focus: cx.focus_handle(),
            focused: false,
            bounds: Bounds::default(),
            buttons: 0,
            #[cfg(test)]
            sent: Vec::new(),
            problem: None,
            title: None,
            address_told: None,
            calls: std::collections::HashMap::new(),
            next_call,
            drawn: Instant::now(),
            waiting: None,
            hidden: false,
            paced: false,
            lively: Instant::now(),
            last_frame: Instant::now(),
            frame_stats: FrameStats::default(),
            wheel_rest: Point::default(),
            file_drag: file_drag::FileDrag::default(),
            pinch_zoom: false,
            pinch_rest: 0.,
            resampler: Default::default(),
            touchpad_at: (0, 0, 0),
            refresh,
            frame_rate: frame_rate(refresh),
            _display: display,
            pace: Pace {
                prior: refresh,
                ..Default::default()
            },
            fill_token: 0,
            fill_wake: None,
            _fills: None,
            placeholder: None,
            snapshot: None,
            drew: false,
            loaded: false,
            // A clear page draws no white under it, and nothing until its own document drew.
            veiled: clear,
            revealed: false,
            held,
            _tick: tick,
        };
        if view.engine.is_none() {
            view.start();
        } else if let Some(Engine::Cef(page)) = &view.engine {
            view.paced = page.paced();
            page.send(Command::FrameRate {
                id: page.id(),
                fps: view.frame_rate,
            });
            // The next draw unhides the same page. Never rerun startup or navigate it on reattachment.
            view.hidden = true;
        }
        if let Some(startup) = startup.filter(|_| held) {
            crate::measure::page_mark("engine-open");
            view.navigate_startup(SCRIPT_ID + startup_messages(startup).len() as u64);
            // Hidden until its document loaded (`take_changes`), and at most `HOLD_AT_MOST`.
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(ready::HOLD_AT_MOST).await;
                if let Err(error) = this.update(cx, |view, _| view.release_held()) {
                    eprintln!("nexus-desktop-gpui: release prepared page: {error}");
                }
            })
            .detach();
        }
        if trace() {
            fake_touchpad(window, cx);
        }
        view
    }

    /// The page's name in `NEXUS_WEB_TRACE`'s lines.
    fn trace_label(&self) -> String {
        match &self.engine {
            Some(Engine::Cef(page)) => format!("page {}", page.id()),
            _ => "page".into(),
        }
    }

    /// `NEXUS_WEB_TRACE`'s line for the frame rate the page was given, and why.
    fn trace_frame_rate(&self, when: &str) {
        if trace() {
            eprintln!(
                "nexus web trace: {}: frame rate {} ({}), {when}",
                self.trace_label(),
                self.frame_rate,
                frame_rate_source(self.refresh)
            );
        }
    }

    /// The window moved to another display, or its display's rate changed: the page paints at the new rate, and the
    /// pace starts from it.
    fn display_changed(&mut self, refresh: Option<Duration>) {
        if refresh != self.refresh {
            // The gaps measured were the old display's.
            self.pace.gaps.clear();
        }
        self.refresh = refresh;
        self.pace.prior = refresh;
        let fps = frame_rate(refresh);
        if fps == self.frame_rate {
            return;
        }
        self.frame_rate = fps;
        if let Some(Engine::Cef(page)) = &self.engine {
            page.send(Command::FrameRate { id: page.id(), fps });
            self.trace_frame_rate("display changed");
        }
    }

    /// Open the page in its engine, at the size and scale last laid out.
    fn start(&mut self) {
        // Tests drive the view without an engine (`mouse` records what it would send).
        if cfg!(test) {
            return;
        }
        // `NEXUS_WEB_SELFTEST=surface`: stripes instead of a page (`selftest.rs`).
        #[cfg(target_os = "macos")]
        if selftest::on() {
            return;
        }
        let (size, scale) = {
            let shared = self.shared.lock().expect("web view state");
            (shared.size, shared.scale)
        };
        #[cfg(windows)]
        if d3d11_selftest::on() {
            return;
        }
        // A popup paints on its own timer (`handlers.rs`), as Servo does.
        self.paced = self.adopted.is_none() && !servo::chosen() && paced();
        self.frame_rate = frame_rate(self.refresh);
        if let Some(id) = self.adopted {
            match client::HostPage::adopt(id) {
                Ok(page) => {
                    // A popup starts at its opener's rate; this window's display may have another.
                    page.send(Command::FrameRate {
                        id,
                        fps: self.frame_rate,
                    });
                    self.engine = Some(Engine::Cef(page));
                    self.problem = None;
                    self.trace_frame_rate("on open");
                }
                Err(problem) => self.problem = Some(problem.into()),
            }
            return;
        }
        let started = if servo::chosen() {
            servo::ServoPage::start(&self.url, size, scale, self.shared.clone())
                .map(Engine::Servo)
                .map_err(|error| format!("Servo did not start: {error}"))
        } else {
            // A page with a startup script opens blank: the script is in place before its first document loads.
            client::HostPage::open(
                if self.startup.is_some() {
                    "about:blank"
                } else {
                    &self.url
                },
                size,
                scale,
                self.frame_rate,
                self.paced,
                self.shared.clone(),
                self.browser.clone(),
                self.profile.clone(),
                self.document,
                self.handoff.clone(),
                self.clear,
            )
            .map(Engine::Cef)
            .map_err(|error| {
                eprintln!("nexus-desktop-gpui: web host: {error}");
                "Pages can't show here: the web engine did not start.".to_string()
            })
        };
        crate::measure::page_mark("engine-open");
        match started {
            Ok(engine) => {
                // `HostPage::open` keeps pacing only for a page that gets GPU frames.
                if let Engine::Cef(page) = &engine {
                    self.paced = page.paced();
                }
                self.engine = Some(engine);
                self.problem = None;
                if matches!(self.engine, Some(Engine::Cef(_))) {
                    self.trace_frame_rate("on open");
                }
                // A session browser's page keeps its console and the agent's tools from its first document on, and a
                // Nexus page its canvas's messages. The page domain is on first: without it, the engine keeps the
                // script but runs it on no document.
                if let Some(startup) = self.startup {
                    self.run_startup(startup);
                }
            }
            Err(problem) => self.problem = Some(problem.into()),
        }
    }

    /// Put `startup` in place, then go to the page's address.
    fn run_startup(&self, startup: Startup) {
        let messages = startup_messages(startup);
        let navigate = SCRIPT_ID + messages.len() as u64;
        for message in messages {
            self.send_cdp(message);
        }
        self.navigate_startup(navigate);
    }

    /// Go to the page's address, past the blank document its startup script went in on. `id` is the DevTools id after
    /// the startup's own.
    fn navigate_startup(&self, id: u64) {
        let message = serde_json::json!({ "id": id, "method": "Page.navigate", "params": { "url": self.url } });
        self.send_cdp(message.to_string());
        crate::measure::page_mark("navigate-sent");
    }

    /// Write `frame`'s changed rects into the image on screen, in place, so the GPU gets only those pixels and the image
    /// keeps its tile. False: nothing to update (another size, a frame that does not follow the one shown, no image),
    /// or the renderer said no; then the view makes a new image from the whole frame.
    fn write_changes(&self, frame: &Frame, window: &mut Window) -> bool {
        let Some(image) = &self.image else {
            return false;
        };
        let size = image.size(0);
        let Some(rects) = changes_onto(self.image_seq, (size.width.0, size.height.0), frame) else {
            return false;
        };
        let mut bytes = Vec::new();
        rects.iter().all(|&rect| {
            let [x, y, width, height] = rect;
            let pixels = if rect == [0, 0, frame.width, frame.height] {
                // A full repaint is already tightly packed; do not copy it into the rect buffer.
                frame.pixels.as_slice()
            } else {
                if !cpu::rect_rows(&frame.pixels, frame.width, rect, &mut bytes) {
                    return false;
                }
                bytes.as_slice()
            };
            let rect = Bounds {
                origin: point(DevicePixels(x as i32), DevicePixels(y as i32)),
                size: gpui_kit::size(DevicePixels(width as i32), DevicePixels(height as i32)),
            };
            matches!(window.update_image(image, 0, rect, pixels), Ok(true))
        })
    }

    /// Open the page again, after it stopped.
    fn reload(&mut self, cx: &mut Context<Self>) {
        // The old page closes first, so a dead engine is not asked for it again. A popup opens again at its address,
        // as a page of its own. Its calls will not be answered.
        self.engine = None;
        self.calls.clear();
        // The new page counts its frames from 1 again: no change it sends is onto the old page's image.
        self.image_seq = 0;
        if self.adopted.take().is_some()
            && let Some(address) = self.shared.lock().expect("web view state").address.clone()
        {
            self.url = address;
        }
        {
            let mut shared = self.shared.lock().expect("web view state");
            shared.stopped = None;
            shared.frame = None;
            shared.gpu_failed = false;
            #[cfg(target_os = "linux")]
            {
                shared.texture = None;
            }
        }
        #[cfg(target_os = "linux")]
        {
            self.texture = None;
        }
        self.hidden = false;
        self.waiting = None;
        self.start();
        cx.notify();
    }

    /// Send the page a DevTools Protocol message (JSON text); its replies come back as [`WebEvent::Cdp`]. For the agent
    /// browser: Chromium's own automation, with no debugging port open to the rest of the computer.
    #[allow(dead_code)]
    pub fn send_cdp(&self, message: impl Into<String>) {
        if let Some(Engine::Cef(page)) = &self.engine {
            page.send(Command::Cdp {
                id: page.id(),
                message: message.into(),
            });
        }
    }

    /// Go to `url`, in this page's history.
    pub fn navigate(&self, url: &str) {
        if let Some(Engine::Cef(page)) = &self.engine {
            page.send(Command::Navigate {
                id: page.id(),
                url: url.to_string(),
            });
        }
    }

    /// Back (-1) or forward (1) in the page's history, or load it again (0). A page that stopped opens again.
    pub fn go(&mut self, step: i32, cx: &mut Context<Self>) {
        if self.problem.is_some() && step == 0 {
            self.reload(cx);
            return;
        }
        if let Some(Engine::Cef(page)) = &self.engine {
            page.send(Command::History { id: page.id(), step });
        }
    }

    /// Ask the page `method` with `params` over its DevTools Protocol, and get its answer: the `result`, or the error's
    /// message. The answer is dropped (the receiver says it was cancelled) when the page stops or opens again.
    pub fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> futures::channel::oneshot::Receiver<Result<serde_json::Value, String>> {
        let (reply, answer) = futures::channel::oneshot::channel();
        self.next_call += 1;
        let id = self.next_call;
        self.calls.insert(id, reply);
        self.send_cdp(serde_json::json!({ "id": id, "method": method, "params": params }).to_string());
        answer
    }

    /// Show the picture of `key` (one page at one version) while the page opens, and take a new one once it loaded.
    /// Only a Chromium page has the DevTools Protocol that takes it.
    pub fn keep_snapshot(&mut self, key: &str, cx: &mut Context<Self>) {
        if !matches!(self.engine, Some(Engine::Cef(_))) {
            return;
        }
        let file = snapshot::file_for(key);
        self.snapshot = Some(file.clone());
        // Read here, not in the background: the view draws its first frame right after this, and a picture that
        // arrives a frame late leaves that frame blank. It is one small JPEG, a few milliseconds.
        crate::measure::page_mark("snapshot-read-start");
        if let Some(image) = snapshot::load(&file) {
            self.placeholder = Some((image, Instant::now()));
            crate::measure::page_mark("snapshot-read");
        }
        // The picture's time is up: the next render drops it.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(snapshot::SHOW_AT_MOST).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// Draw nothing of the page until its own document drew and loaded: no white engine background, no blank startup
    /// document, no half-drawn page. The view is see-through meanwhile, so its owner's loading state shows under it,
    /// and only the page's last picture ([`WebView::keep_snapshot`]) or a problem note draws. [`WebEvent::Shown`]
    /// says when the page shows.
    pub fn veil(&mut self) {
        self.veiled = true;
    }

    /// Whether the page's last picture shows while it opens.
    pub fn has_placeholder(&self) -> bool {
        self.placeholder.is_some()
    }

    /// Show a veiled page that drew but never said it finished loading (a page that keeps a request open): it is
    /// better than a loading state that never ends.
    pub fn reveal_if_drawn(&mut self, cx: &mut Context<Self>) {
        if self.drew {
            self.reveal(cx);
        }
    }

    fn reveal(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.revealed, true) {
            return;
        }
        if self.veiled {
            cx.emit(WebEvent::Shown);
            cx.notify();
        }
    }

    /// Take the page's picture, a while after it loaded, and keep it in its file.
    fn take_snapshot(&mut self, cx: &mut Context<Self>) {
        let Some(file) = self.snapshot.take() else {
            return;
        };
        let opened = self.url.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(snapshot::TAKE_AFTER).await;
            // The picture is of this page: not of what a click, a sign-in redirect or an error page made of it since.
            let Ok(Some(status)) = this.update(cx, |view, _| {
                snapshot::at_address(view.address().as_deref(), &opened).then(|| {
                    view.call(
                        "Runtime.evaluate",
                        serde_json::json!({ "expression": snapshot::STATUS, "returnByValue": true }),
                    )
                })
            }) else {
                return;
            };
            let Ok(Ok(status)) = status.await else {
                return;
            };
            if !snapshot::loaded_ok(&status) {
                return;
            }
            // The page can move while the status comes back: look again right before the picture.
            let Ok(Some(answer)) = this.update(cx, |view, _| {
                snapshot::at_address(view.address().as_deref(), &opened).then(|| {
                    view.call(
                        "Page.captureScreenshot",
                        serde_json::json!({ "format": "jpeg", "quality": snapshot::QUALITY }),
                    )
                })
            }) else {
                return;
            };
            let Ok(Ok(result)) = answer.await else {
                return;
            };
            let Some(data) = result["data"].as_str().map(String::from) else {
                return;
            };
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = snapshot::save(&file, &data) {
                        eprintln!("nexus-desktop-gpui: page snapshot: {error}");
                    }
                })
                .await;
            crate::measure::page_mark("snapshot-saved");
        })
        .detach();
    }

    /// Show a page opened ahead, which stayed hidden while it navigated.
    fn release_held(&mut self) {
        if std::mem::take(&mut self.held) {
            self.set_hidden(false);
        }
    }

    fn drop_placeholder(&mut self, window: &mut Window) {
        if let Some((old, _)) = self.placeholder.take() {
            let _ = window.drop_image(old);
        }
    }

    /// Remove every page picture: they show one sign-in's pages. Call it when the person signs out or switches organization.
    pub fn clear_snapshots() {
        snapshot::clear();
    }

    /// Where the page's main frame is, once the engine said.
    pub fn address(&self) -> Option<String> {
        self.shared.lock().expect("web view state").address.clone()
    }

    /// Bring in what the engine reported: a new frame, title, cursor, links to open, or that the page stopped.
    fn take_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (frame, title, popups, stopped, cdp, gpu_failed, address, loading, load_failure, opened, closed, handoffs) = {
            let mut shared = self.shared.lock().expect("web view state");
            if !shared.dirty {
                return;
            }
            shared.dirty = false;
            (
                shared.frame.take(),
                shared.title.clone(),
                std::mem::take(&mut shared.popups),
                shared.stopped.clone(),
                std::mem::take(&mut shared.cdp),
                shared.gpu_failed,
                shared.address.clone(),
                shared.loading.take(),
                shared.load_failure.take(),
                std::mem::take(&mut shared.opened),
                std::mem::take(&mut shared.closed),
                std::mem::take(&mut shared.handoffs),
            )
        };
        // A prepared page may still report its old blank paint after adoption. Ignore all paints until its
        // document loads and the host is unhidden, so the first accepted paint belongs to the visible page.
        let frame = if self.held { None } else { frame };
        if gpu_failed {
            // GPU frames are off now (`frames::turn_off`): the page opens again, with CPU frames.
            self.reload(cx);
            return;
        }
        #[cfg(target_os = "linux")]
        let texture = self.shared.lock().expect("web view state").texture.take();
        #[cfg(not(target_os = "linux"))]
        let texture: Option<()> = None;
        let fresh = frame.is_some() || texture.is_some();
        crate::measure::page_event(
            window,
            loading.as_ref().is_some_and(|state| !state.loading)
                && address.as_deref().is_some_and(|address| address != "about:blank"),
            fresh,
        );
        if fresh {
            let now = Instant::now();
            if trace() {
                let label = self.trace_label();
                self.frame_stats.frame(now.duration_since(self.last_frame), &label);
            }
            self.last_frame = now;
            self.lively = now + LIVELY_FOR;
            let waiting = *self.waiting.get_or_insert_with(Instant::now);
            if !self.hidden && waiting.elapsed() > HIDDEN_AFTER {
                self.set_hidden(true);
            }
        }
        #[cfg(target_os = "linux")]
        if let Some(texture) = texture {
            self.texture = Some(texture);
            if let Some(old) = self.image.take() {
                let _ = window.drop_image(old);
            }
        }
        if let Some(frame) = frame {
            if self.write_changes(&frame, window) {
                // The frame's buffer goes: the view keeps no copy of the page, so it costs no more memory than before.
                self.image_seq = frame.seq;
                if trace() {
                    self.frame_stats.in_place += 1;
                }
            } else if let Some(buffer) = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels) {
                // GPUI's images are BGRA already, as CEF paints them: no conversion.
                let image = Arc::new(RenderImage::new(SmallVec::from_elem(image::Frame::new(buffer), 1)));
                self.image_seq = frame.seq;
                #[cfg(target_os = "linux")]
                {
                    self.texture = None;
                }
                if let Some(old) = self.image.replace(image) {
                    // The old frame's texture leaves the atlas, or every frame would stay in GPU memory.
                    let _ = window.drop_image(old);
                }
            }
        }
        // The page's own document, past the blank one its startup script goes in on: once it loaded and drew, it
        // covers its picture, and its own picture is taken.
        if address.as_deref().is_some_and(|address| address != "about:blank") {
            crate::measure::page_mark("address");
            self.drew |= fresh;
            self.loaded |= loading.is_some_and(|loading| !loading.loading);
            if fresh {
                crate::measure::page_mark("first-frame");
            }
            if self.loaded {
                crate::measure::page_mark("loaded");
            }
            if self.drew && self.loaded {
                crate::measure::page_mark("usable");
                self.drop_placeholder(window);
                self.take_snapshot(cx);
                self.reveal(cx);
            }
            // A page opened ahead shows once its own document loaded: its first frame is of it.
            if self.loaded {
                self.release_held();
            }
        }
        if let Some(stopped) = stopped {
            self.calls.clear();
            self.problem = Some(stopped.into());
            if let Some(old) = self.image.take() {
                let _ = window.drop_image(old);
            }
            #[cfg(target_os = "linux")]
            {
                self.texture = None;
            }
        }
        // Only a new title: each emit redraws the page's owner, and a frame must not redraw the whole window.
        if let Some(title) = title
            && self.title.as_ref() != Some(&title)
        {
            if trace() {
                eprintln!("nexus web trace: {}: title {title}", self.trace_label());
            }
            self.title = Some(title.clone());
            cx.emit(WebEvent::TitleChanged(title));
        }
        for message in cdp {
            // An answer to [`WebView::call`] goes to its caller; anything else to the owner.
            if let Some((id, answer)) = answer_of(&message)
                && let Some(reply) = self.calls.remove(&id)
            {
                let _ = reply.send(answer);
                continue;
            }
            cx.emit(WebEvent::Cdp(message));
        }
        for url in popups {
            crate::external_link::open_popup(&url, cx);
        }
        if let Some(address) = address
            && self.address_told.as_ref() != Some(&address)
        {
            self.address_told = Some(address.clone());
            cx.emit(WebEvent::Address(address));
        }
        if let Some(loading) = loading {
            cx.emit(WebEvent::Loading(loading));
        }
        if let Some(status) = load_failure {
            cx.emit(WebEvent::LoadFailed(status));
        }
        for opened in opened {
            cx.emit(WebEvent::Popup(opened));
        }
        if closed {
            cx.emit(WebEvent::Closed);
        }
        for url in handoffs {
            cx.emit(WebEvent::Handoff(url));
        }
        cx.notify();
    }

    /// Send a lively paced page this frame's share of a touchpad scroll, and ask it for its next frame (`begin`).
    fn begin_frame(&mut self, now: Instant, begin: bool) {
        if let Some(Engine::Cef(page)) = &self.engine {
            let (dx, dy) = self.resampler.take(now);
            if dx != 0 || dy != 0 {
                let (x, y, modifiers) = self.touchpad_at;
                page.send(Command::Wheel {
                    id: page.id(),
                    x,
                    y,
                    modifiers,
                    dx,
                    dy,
                });
            }
            if begin {
                page.send(Command::BeginFrame { id: page.id() });
            }
        }
        if self.resampler.pending() {
            self.lively = now + LIVELY_FOR;
        }
    }

    /// Fill in the next frame if the app has not drawn it `frames` intervals from now. A later call replaces it.
    fn arm_fill(&mut self, frames: f32, cx: &mut Context<Self>) {
        self.fill_token += 1;
        let Some(interval) = self.pace.interval() else {
            return;
        };
        let wake = match &self.fill_wake {
            Some(wake) => wake.clone(),
            None => {
                let (wake, mut woken) = unbounded::<u64>();
                self.fill_wake = Some(wake.clone());
                self._fills = Some(cx.spawn(async move |this, cx| {
                    while let Some(token) = woken.next().await {
                        let fill = |this: &mut WebView, cx: &mut Context<WebView>| {
                            if token == this.fill_token {
                                this.fill_frame(cx);
                            }
                        };
                        if this.update(cx, fill).is_err() {
                            break;
                        }
                    }
                }));
                wake
            }
        };
        alarm(Instant::now() + interval.mul_f32(frames), self.fill_token, wake);
    }

    /// The app did not draw a lively paced page's frame in time (the compositor skipped a refresh): ask for it anyway,
    /// so the page paints at its own pace, as in a browser.
    fn fill_frame(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        if !self.paced || self.hidden || now >= self.lively || self.pace.filled >= FILLS {
            return;
        }
        self.pace.filled += 1;
        self.pace.filled_at = Some(now);
        if trace() {
            self.frame_stats.filled += 1;
        }
        self.begin_frame(now, true);
        self.arm_fill(1., cx);
    }

    /// Input came in: keep the page painting at the app's pace, since it is likely to change.
    fn stir(&mut self, cx: &mut Context<Self>) {
        if !self.paced {
            return;
        }
        let now = Instant::now();
        let was_lively = now < self.lively;
        self.lively = now + LIVELY_FOR;
        if !was_lively {
            cx.notify();
        }
    }

    /// The fingers left the touchpad. On Linux, Chromium's own windows fling the page on from the stroke's speed;
    /// elsewhere the system sends the fling as more scrolling.
    fn lift(&mut self, at: Instant, cx: &mut Context<Self>) {
        if cfg!(target_os = "linux") && self.resampler.pending() {
            self.resampler.lift(at);
            self.stir(cx);
        }
    }

    /// A wheel or touchpad scroll over the page, sensed at `at`.
    fn wheel(
        &mut self,
        at: Instant,
        position: Point<Pixels>,
        delta: ScrollDelta,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) {
        // Chromium's wheel deltas are pixels; a notch is 120 in its terms, about three lines.
        let pixels = delta.pixel_delta(px(40.));
        if pixels.x == px(0.) && pixels.y == px(0.) {
            return;
        }
        let (x, y, mut modifiers) = self.mouse_at(position, modifiers);
        // On a paced page, a touchpad's deltas are exact: Chromium follows them as they come, and animates only a
        // wheel's notches. Other pages keep Chromium's animation for both, as before pacing. CPU frame pages are not
        // paced by default: paced, they scroll less evenly on Linux. macOS and Windows wait for a
        // measured run.
        if delta.precise() && self.paced {
            modifiers |= keys::PRECISE_SCROLL;
            if resampled() && matches!(self.engine, Some(Engine::Cef(_))) {
                // Sent with the next frame, resampled (`render`).
                self.resampler.push(at, (f32::from(pixels.x), f32::from(pixels.y)));
                self.touchpad_at = (x, y, modifiers);
                self.stir(cx);
                return;
            }
        }
        // Chromium takes whole pixels: what is below one waits for the next event, or slow scrolls stall and fast
        // ones lurch.
        let wanted = point(f32::from(pixels.x), f32::from(pixels.y)) + self.wheel_rest;
        let (dx, dy) = (wanted.x.trunc(), wanted.y.trunc());
        self.wheel_rest = point(wanted.x - dx, wanted.y - dy);
        self.stir(cx);
        match &self.engine {
            Some(Engine::Cef(_)) if dx == 0. && dy == 0. => {}
            Some(Engine::Cef(page)) => page.send(Command::Wheel {
                id: page.id(),
                x,
                y,
                modifiers,
                dx: dx as i32,
                dy: dy as i32,
            }),
            Some(Engine::Servo(page)) => page.send(serde_json::json!({
                "cmd": "wheel", "dx": f32::from(pixels.x), "dy": f32::from(pixels.y), "x": x, "y": y,
            })),
            None => {}
        }
    }

    /// The mouse's back or forward button: a step through the page's history.
    fn on_history_button(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(step) = gestures::history_step(event.button) {
            self.go(step, cx);
        }
    }

    /// Send the page the steps of a system file drag over it.
    fn send_file_drag(&self, position: Point<Pixels>, modifiers: &Modifiers, steps: Vec<protocol::FileDragStep>) {
        let Some(Engine::Cef(page)) = &self.engine else {
            return;
        };
        let (x, y, modifiers) = self.mouse_at(position, modifiers);
        for step in steps {
            page.send(Command::FileDrag {
                id: page.id(),
                x,
                y,
                modifiers,
                step,
            });
        }
    }

    /// Whether files from the system may be dropped on the page: a session browser's website, as in a browser. The
    /// app's own pages leave a drop to the window, which attaches the files to the composer.
    fn takes_files(&self) -> bool {
        (self.browser.is_some() || self.adopted.is_some()) && !self.document
    }

    /// Send the page touchpad pinches ([`WebView::pinch`]). Only a page that zooms with them asks: on a page that does
    /// not, Chromium would scroll it by the pinch's wheel events.
    pub(crate) fn set_pinch_zoom(&mut self, on: bool) {
        self.pinch_zoom = on;
    }

    /// A touchpad pinch, as Chrome gives one to a page: a wheel event with Ctrl held. CEF takes no pinch,
    /// and without this the page never heard of one: only Cmd with a two-finger scroll zoomed a design.
    fn pinch(&mut self, event: &PinchEvent, cx: &mut Context<Self>) {
        let Some(Engine::Cef(page)) = &self.engine else {
            return;
        };
        if !self.pinch_zoom {
            return;
        }
        let wanted = pinch_wheel_delta(event.delta) + self.pinch_rest;
        let dy = wanted.trunc();
        self.pinch_rest = wanted - dy;
        if dy == 0. {
            return;
        }
        let (x, y, modifiers) = self.mouse_at(event.position, &event.modifiers);
        page.send(Command::Wheel {
            id: page.id(),
            x,
            y,
            modifiers: modifiers | keys::CONTROL | keys::PRECISE_SCROLL,
            dx: 0,
            dy: dy as i32,
        });
        self.stir(cx);
    }

    fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
        if let Some(Engine::Cef(page)) = &self.engine {
            page.send(Command::Hidden { id: page.id(), hidden });
        }
    }

    /// The layout placed the page at `bounds`: tell the engine when its size or the screen's scale changed.
    fn place(&mut self, bounds: Bounds<Pixels>, scale: f32) {
        self.bounds = bounds;
        let size = (
            f32::from(bounds.size.width).round() as i32,
            f32::from(bounds.size.height).round() as i32,
        );
        let changed = {
            let mut shared = self.shared.lock().expect("web view state");
            let changed = shared.size != size || shared.scale != scale;
            shared.size = size;
            shared.scale = scale;
            changed
        };
        if !changed {
            return;
        }
        if self.startup.is_some() && self.browser.is_none() && size.0 > 0 && size.1 > 0 {
            ready::placed(ready::Placed {
                size,
                scale,
                frame_rate: self.frame_rate,
            });
        }
        match &self.engine {
            Some(Engine::Cef(page)) => page.send(Command::Resize {
                id: page.id(),
                width: size.0,
                height: size.1,
                scale,
            }),
            Some(Engine::Servo(page)) => page.resize(size, scale),
            None => {}
        }
    }

    /// Where `position` is on the page, and the event flags with the held buttons.
    fn mouse_at(&self, position: Point<Pixels>, modifiers: &Modifiers) -> (i32, i32, u32) {
        let local = position - self.bounds.origin;
        (
            f32::from(local.x).round() as i32,
            f32::from(local.y).round() as i32,
            keys::flags(modifiers) | self.buttons,
        )
    }

    fn mouse(&mut self, position: Point<Pixels>, modifiers: &Modifiers, action: MouseAction) {
        let (x, y, flags) = self.mouse_at(position, modifiers);
        #[cfg(test)]
        self.sent.push((action, flags));
        match &self.engine {
            Some(Engine::Cef(page)) => page.send(Command::Mouse {
                id: page.id(),
                x,
                y,
                modifiers: flags,
                action,
            }),
            Some(Engine::Servo(page)) => match action {
                MouseAction::Move => page.send(serde_json::json!({ "cmd": "move", "x": x, "y": y })),
                MouseAction::Leave => page.send(serde_json::json!({ "cmd": "leave" })),
                MouseAction::Down { button, .. } | MouseAction::Up { button, .. } => {
                    let button = match button {
                        Button::Left => 0,
                        Button::Middle => 1,
                        Button::Right => 2,
                    };
                    let down = matches!(action, MouseAction::Down { .. });
                    page.send(serde_json::json!({ "cmd": "button", "down": down, "button": button, "x": x, "y": y }));
                }
            },
            None => {}
        }
    }

    fn mouse_button(
        &mut self,
        button: MouseButton,
        up: bool,
        position: Point<Pixels>,
        modifiers: &Modifiers,
        clicks: usize,
    ) {
        let Some((button, flag)) = button_and_flag(button) else {
            return;
        };
        if !up && self.buttons & flag != 0 {
            // Pressed again with no release between: the release went where the view did not see it. The page gets
            // it first, or it holds the button through both presses.
            self.release(button, position, modifiers);
        }
        // The flags name the buttons held during the event: the pressed one too, the released one no more.
        if up {
            self.buttons &= !flag;
        } else {
            self.buttons |= flag;
        }
        let clicks = clicks.clamp(1, 3) as i32;
        let action = if up {
            MouseAction::Up { button, clicks }
        } else {
            MouseAction::Down { button, clicks }
        };
        self.mouse(position, modifiers, action);
    }

    /// The page lets go of `button`.
    fn release(&mut self, button: Button, position: Point<Pixels>, modifiers: &Modifiers) {
        self.buttons &= !flag_of(button);
        self.mouse(position, modifiers, MouseAction::Up { button, clicks: 1 });
    }

    /// A move with no button pressed: the page lets go of the buttons it still holds. Their release went where the
    /// view did not see it: a release the window never got (Windows once capture is lost, a compositor's window
    /// move). Chromium ends a scrollbar drag or a text selection only on a release; without one, the next hover drags.
    /// A move names one pressed button at most, so a move with one releases nothing.
    fn release_missed(&mut self, pressed: Option<MouseButton>, position: Point<Pixels>, modifiers: &Modifiers) {
        if pressed.is_some() {
            return;
        }
        for button in [Button::Left, Button::Middle, Button::Right] {
            if self.buttons & flag_of(button) != 0 {
                self.release(button, position, modifiers);
            }
        }
    }

    /// A release off the page (over the sidebar, or on another display: the window still gets it) of a button
    /// pressed on it. The page's own `on_mouse_up` hears only releases over it.
    fn on_mouse_up_out(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((button, flag)) = button_and_flag(event.button) else {
            return;
        };
        if self.buttons & flag != 0 {
            self.release(button, event.position, &event.modifiers);
            self.stir(cx);
        }
    }

    fn sync_focus(&mut self, window: &Window) {
        let focused = self.focus.is_focused(window);
        if focused == self.focused {
            return;
        }
        self.focused = focused;
        match &self.engine {
            Some(Engine::Cef(page)) => page.send(Command::Focus { id: page.id(), focused }),
            Some(Engine::Servo(page)) => page.send(serde_json::json!({ "cmd": "focus", "focused": focused })),
            None => {}
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.mouse_button(event.button, true, event.position, &event.modifiers, event.click_count);
        self.stir(cx);
    }

    /// The note shown instead of the page, with Reload: a page that stopped, or an engine that did not start, gets
    /// another try (a new host, when the old one is gone).
    /// In the app's palette, as every other note: a white box in the dark theme reads as a flash.
    fn render_problem(&self, problem: SharedString, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx
            .try_global::<crate::pages::Theme>()
            .map(|theme| theme.0)
            .unwrap_or_else(|| {
                crate::palette::Palette::for_choice(crate::palette::ThemeChoice::Light, WindowAppearance::Light)
            });
        div()
            .absolute()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .bg(palette.background)
            .child(div().text_size(px(13.)).text_color(palette.muted).child(problem))
            .child(
                div()
                    .id("web-reload")
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(crate::tokens::radius::CONTROL))
                    .border_1()
                    .border_color(palette.border)
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .cursor_pointer()
                    .hover(move |button| button.bg(palette.hover))
                    .child("Reload")
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
    }
}

impl Focusable for WebView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for WebView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        if let Some(stripes) = selftest::element(cx.entity_id(), window) {
            return stripes;
        }
        let gap = self.drawn.elapsed();
        self.drawn = Instant::now();
        self.waiting = None;
        if self.hidden && !self.held {
            if gap < SHOWN_GAP {
                self.set_hidden(false);
            } else {
                // Is the window back? The next frame's pace says.
                window.request_animation_frame();
            }
        }
        // A lively paced page paints its next frame now, in time for the app's next one: first with this frame's
        // share of a touchpad scroll.
        if self.paced && !self.hidden && Instant::now() < self.lively {
            let now = Instant::now();
            let filled = self.pace.draw(now);
            self.begin_frame(now, !filled);
            window.request_animation_frame();
            self.arm_fill(FILL_SLACK + 1., cx);
        }
        self.sync_focus(window);
        let cursor = gestures::cursor_style(self.shared.lock().expect("web view state").cursor);
        let takes_files = self.takes_files();
        let problem = self
            .problem
            .clone()
            .map(|problem| self.render_problem(problem, cx).into_any_element());
        let this = cx.entity();
        if self
            .placeholder
            .as_ref()
            .is_some_and(|(_, since)| since.elapsed() >= snapshot::SHOW_AT_MOST)
        {
            self.drop_placeholder(window);
        }
        let placeholder = self.placeholder.as_ref().map(|(picture, _)| picture.clone());
        let (frames, backdrop) = veil_state(self.veiled, self.revealed);
        let image = self.image.clone().filter(|_| frames);
        #[cfg(target_os = "linux")]
        let texture = self.texture.clone().filter(|_| frames);
        // Measures where the layout put the page, then draws its frame there pixel for pixel: from a whole device
        // pixel, at the frame's own size. Stretched or placed between pixels (as at a 1.25 scale), it would blur.
        let page = canvas(
            move |bounds, window, cx| {
                let scale = window.scale_factor();
                this.update(cx, |view, _| view.place(bounds, scale));
            },
            move |bounds, _, window, _| {
                let scale = window.scale_factor();
                let snap = |value: Pixels| px((f32::from(value) * scale).round() / scale);
                let place = |width: u32, height: u32| Bounds {
                    origin: point(snap(bounds.origin.x), snap(bounds.origin.y)),
                    size: gpui_kit::size(px(width as f32 / scale), px(height as f32 / scale)),
                };
                #[cfg(windows)]
                if d3d11_selftest::paint(bounds, &place, window) {
                    return;
                }
                // The page's picture, until the page drew itself: at the width it was taken at only, or the page's own
                // layout would jump when it shows.
                if let Some(picture) = placeholder {
                    let size = picture.size(0);
                    if snapshot::fits(size.width.0 as f32, f32::from(bounds.size.width) * scale) {
                        let frame = place(size.width.0 as u32, size.height.0 as u32);
                        paint_frame(bounds, frame, picture, window);
                        return;
                    }
                }
                #[cfg(target_os = "linux")]
                if let Some(texture) = texture {
                    let (width, height) = texture.size();
                    let frame = place(width, height);
                    // A surface is not clipped to the canvas as an image is: clip it here.
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        window.paint_surface(frame, texture);
                    });
                    return;
                }
                let Some(image) = image else {
                    return;
                };
                let size = image.size(0);
                let frame = place(size.width.0 as u32, size.height.0 as u32);
                paint_frame(bounds, frame, image, window);
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div()
            .id("web-view")
            .relative()
            .size_full()
            .overflow_hidden()
            .when(backdrop, |view| view.bg(gpui_kit::white()))
            .track_focus(&self.focus)
            .key_context("WebView")
            .cursor(cursor)
            .child(page)
            .children(problem)
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                // Files from the system over the page: the page gets drag moves, not hovers (`on_drag_move`).
                if this.file_drag.active() {
                    return;
                }
                this.release_missed(event.pressed_button, event.position, &event.modifiers);
                this.mouse(event.position, &event.modifiers, MouseAction::Move);
                this.stir(cx);
            }))
            .on_hover(cx.listener(|this, hovered: &bool, window, _| {
                if !*hovered {
                    let steps = this.file_drag.left();
                    this.send_file_drag(window.mouse_position(), &Modifiers::default(), steps);
                    this.mouse(window.mouse_position(), &Modifiers::default(), MouseAction::Leave);
                }
            }))
            // The mouse's back and forward buttons go back and forward in the page's history, as in a browser.
            .on_mouse_up(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(Self::on_history_button),
            )
            .on_mouse_up(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(Self::on_history_button),
            )
            .when(takes_files, |view| {
                view.on_drag_move(cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                    let paths = event.drag(cx).paths().to_vec();
                    let inside = event.bounds.contains(&event.event.position);
                    let steps = this.file_drag.moved(&paths, inside);
                    this.send_file_drag(event.event.position, &event.event.modifiers, steps);
                    this.stir(cx);
                }))
                .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                    let steps = this.file_drag.dropped(paths.paths());
                    this.send_file_drag(window.mouse_position(), &window.modifiers(), steps);
                    this.stir(cx);
                }))
            })
            .on_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.sync_focus(window);
                this.mouse_button(event.button, false, event.position, &event.modifiers, event.click_count);
                this.stir(cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up_out))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::on_mouse_up_out))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::on_mouse_up_out))
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                // When the touchpad sensed it, where the platform says (`interactive.rs`, a Nexus patch). A time far
                // back is on some other clock.
                let at = scroll_event_time()
                    .filter(|at| at.elapsed() < Duration::from_millis(250))
                    .unwrap_or_else(Instant::now);
                if event.touch_phase == TouchPhase::Ended {
                    this.lift(at, cx);
                }
                this.wheel(at, event.position, event.delta, &event.modifiers, cx);
            }))
            .on_pinch(cx.listener(|this, event: &PinchEvent, _, cx| this.pinch(event, cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.stir(cx);
                match &this.engine {
                    Some(Engine::Cef(page)) => {
                        for key in keys::key_down(&event.keystroke) {
                            page.send(Command::Key { id: page.id(), key });
                        }
                    }
                    Some(Engine::Servo(page)) => page.send(keys::servo_key(true, &event.keystroke)),
                    None => return,
                }
                cx.stop_propagation();
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, _| match &this.engine {
                Some(Engine::Cef(page)) => page.send(Command::Key {
                    id: page.id(),
                    key: keys::key_up(&event.keystroke),
                }),
                Some(Engine::Servo(page)) => page.send(keys::servo_key(false, &event.keystroke)),
                None => {}
            }))
    }
}

/// The wheel delta Chromium gives a page for a pinch step that scales by `1 + magnification`: `100·ln(scale)` (its
/// `TouchpadPinchEventQueue`). The page reads `deltaY`, its negative, so `Math.exp(-deltaY / 100)` is the step, as
/// pages-host's canvas does (`wheelZoomFactor`). Positive zooms in.
fn pinch_wheel_delta(magnification: f32) -> f32 {
    100. * (1. + magnification).max(0.01).ln()
}

/// For measuring scroll smoothness: `NEXUS_WEB_FAKE_SCROLL=<events a second>,<pixels each>[,<jitter ms>[,<lift>]]`
/// (with `NEXUS_WEB_TRACE`) scrolls the page as a touchpad would, through the same path as one, from 3s after it
/// opens: 3s down, 3s up, over and over. Each event comes up to the jitter early or late, as a real touchpad's do.
/// With `<lift>` at 1, each stroke lasts 1s and the finger lifts: the page flings, and the next stroke waits 2s.
fn fake_touchpad(window: &mut Window, cx: &mut Context<WebView>) {
    let Some((rate, step, jitter, lift)) = std::env::var("NEXUS_WEB_FAKE_SCROLL").ok().and_then(|value| {
        let mut parts = value.split(',').map(|part| part.parse::<f32>().ok());
        Some((
            parts.next()??,
            parts.next()??,
            parts.next().flatten().unwrap_or(0.),
            parts.next().flatten().unwrap_or(0.) > 0.,
        ))
    }) else {
        return;
    };
    cx.spawn_in(window, async move |this, cx| {
        let every = Duration::from_secs_f32(1. / rate.max(1.));
        cx.background_executor().timer(Duration::from_secs(3)).await;
        let start = Instant::now();
        let mut next = Instant::now();
        let mut seed = 0x2545_f491_u32;
        let mut stroke = 0;
        loop {
            next += every;
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let off = ((seed >> 16) as f32 / 32_768. - 1.) * jitter;
            // The touchpad senses it at `next`, and it reaches the app up to the jitter later or earlier.
            let sensed = next;
            let at = if off >= 0. {
                next + Duration::from_secs_f32(off / 1000.)
            } else {
                next.checked_sub(Duration::from_secs_f32(-off / 1000.)).unwrap_or(next)
            };
            cx.background_executor()
                .timer(at.saturating_duration_since(Instant::now()))
                .await;
            let (now_stroke, touching, down) = if lift {
                let second = start.elapsed().as_secs();
                (second / 3, second % 3 == 0, (second / 3) % 2 == 0)
            } else {
                (0, true, (start.elapsed().as_secs() / 3) % 2 == 0)
            };
            let lifted = lift && !touching && stroke == now_stroke;
            if lift && touching {
                stroke = now_stroke;
            } else if lift && !lifted {
                continue;
            }
            let delta = ScrollDelta::Pixels(point(px(0.), px(if down { -step } else { step })));
            let alive = this.update(cx, |this, cx| {
                let center = this.bounds.center();
                if lifted {
                    this.lift(sensed, cx);
                } else {
                    this.wheel(sensed, center, delta, &Modifiers::default(), cx);
                }
            });
            if lifted {
                stroke = u64::MAX;
            }
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
}

/// Chromium's button, and its event flag, for a GPUI button. `None`: one Chromium has no button for (back, forward).
fn button_and_flag(button: MouseButton) -> Option<(Button, u32)> {
    let button = match button {
        MouseButton::Left => Button::Left,
        MouseButton::Middle => Button::Middle,
        MouseButton::Right => Button::Right,
        _ => return None,
    };
    Some((button, flag_of(button)))
}

fn flag_of(button: Button) -> u32 {
    match button {
        Button::Left => keys::LEFT_BUTTON,
        Button::Middle => keys::MIDDLE_BUTTON,
        Button::Right => keys::RIGHT_BUTTON,
    }
}

/// Draw a page's CPU frame `frame` (whole device pixels, at the frame's own size) clipped to the view's `bounds`.
/// The clip is a content mask, not `paint_image`'s own `bounds`: GPUI cuts the image to `bounds ∩ frame`, rounds that
/// to device pixels half toward zero and the texture's part to the nearest. When a view edge falls on exactly half a
/// device pixel (a quarter point at 2x, a half at 1x), the two disagree by one pixel and the frame is stretched or
/// shifted by one: blurred.
/// The content mask rounds outward (floor left and top, ceil right and bottom), so when the frame is larger than
/// `bounds`, one extra device pixel row or column of the frame can draw past the view edge. The GPU path
/// (`paint_surface`) does the same; that row is not a bug.
fn paint_frame(bounds: Bounds<Pixels>, frame: Bounds<Pixels>, image: Arc<RenderImage>, window: &mut Window) {
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        let _ = window.paint_image(frame, frame, Corners::default(), image, 0, false);
    });
}
#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn a_pinch_is_the_wheel_delta_whose_exp_is_its_scale() {
        // The canvas zooms by exp(-deltaY / 100), and the page's deltaY is the negative of Chromium's delta.
        for magnification in [0.01_f32, 0.1, -0.1, -0.5] {
            let step = (pinch_wheel_delta(magnification) / 100.).exp();
            assert!((step - (1. + magnification)).abs() < 1e-4, "{magnification}: {step}");
        }
        assert!(pinch_wheel_delta(0.05) > 0.);
        assert!(pinch_wheel_delta(-0.05) < 0.);
        assert_eq!(pinch_wheel_delta(0.), 0.);
        // A pinch past nothing stays finite.
        assert!(pinch_wheel_delta(-1.).is_finite());
    }

    #[test]
    fn a_frame_updates_only_the_image_of_the_frame_it_follows() {
        let frame = |seq, changed| Frame {
            pixels: vec![0; 4 * 4 * 4],
            width: 4,
            height: 4,
            seq,
            changed,
        };
        let rects = vec![[1, 1, 2, 2]];
        let next = frame(3, Some((2, rects.clone())));
        assert_eq!(changes_onto(2, (4, 4), &next), Some(rects.as_slice()));
        // Another frame on screen, none, or another size: a new image.
        assert_eq!(changes_onto(1, (4, 4), &next), None);
        assert_eq!(changes_onto(0, (4, 4), &frame(1, Some((0, rects.clone())))), None);
        assert_eq!(changes_onto(2, (4, 5), &next), None);
        // A frame that does not say what changed.
        assert_eq!(changes_onto(2, (4, 4), &frame(3, None)), None);
    }

    #[test]
    fn a_frame_summary_counts_late_frames() {
        let mut gaps: Vec<u32> = std::iter::repeat_n(7_000, 18).chain([7_200, 21_000]).collect();
        let summary = FrameStats::summary(&mut gaps).expect("enough frames");
        assert_eq!(summary, "20 frames, gap median 7.0ms, p95 7.2ms, max 21.0ms, 1 late");
        assert_eq!(FrameStats::summary(&mut [7_000; 3]), None);
    }

    #[test]
    fn the_pace_is_the_apps_usual_gap_and_a_skipped_refresh_does_not_move_it() {
        let start = Instant::now();
        let mut pace = Pace::default();
        let mut at = start;
        for frame in 0..20 {
            // Every fifth frame, the compositor skips a refresh.
            at += Duration::from_micros(if frame % 5 == 4 { 33_333 } else { 16_667 });
            assert!(!pace.draw(at));
        }
        assert_eq!(pace.interval(), Some(Duration::from_micros(16_667)));
    }

    #[test]
    fn the_app_drawing_just_after_a_filled_frame_does_not_ask_again() {
        let start = Instant::now();
        let mut pace = Pace::default();
        assert_eq!(pace.interval(), None);
        let mut at = start;
        for _ in 0..PACE_KNOWN + 1 {
            at += Duration::from_micros(16_667);
            pace.draw(at);
        }
        pace.filled_at = Some(at + Duration::from_millis(18));
        pace.filled = 1;
        // The late frame lands 3ms after the filled one: Chromium paints for it already.
        assert!(pace.draw(at + Duration::from_millis(21)));
        assert_eq!(pace.filled, 0);
        // The next comes a whole frame after it: asked as usual.
        assert!(!pace.draw(at + Duration::from_millis(38)));
    }

    #[test]
    fn a_page_paints_at_the_displays_rate_unless_told_otherwise() {
        let hz = |hertz: f64| Some(Duration::from_secs_f64(1. / hertz));
        assert_eq!(pick_frame_rate(None, hz(120.)), 120);
        assert_eq!(pick_frame_rate(None, hz(143.856)), 144);
        assert_eq!(pick_frame_rate(None, hz(59.94)), 60);
        // `NEXUS_WEB_FPS` wins; a value that is not a number does not.
        assert_eq!(pick_frame_rate(Some("90"), hz(144.)), 90);
        assert_eq!(pick_frame_rate(Some(" 30 "), None), 30);
        assert_eq!(pick_frame_rate(Some("fast"), hz(144.)), 144);
        // No rate from the display (X11 without RandR, say): 60, as before.
        assert_eq!(pick_frame_rate(None, None), 60);
        assert_eq!(pick_frame_rate(None, Some(Duration::ZERO)), 60);
        // Within what CEF takes.
        assert_eq!(pick_frame_rate(None, hz(360.)), 240);
        assert_eq!(pick_frame_rate(Some("1000"), None), 240);
        assert_eq!(pick_frame_rate(Some("0"), hz(60.)), 1);
        assert_eq!(pick_frame_rate(None, Some(Duration::from_secs(5))), 1);
    }

    #[test]
    fn the_pace_starts_at_the_displays_interval_until_it_is_measured() {
        let display = Duration::from_micros(8_333);
        let mut pace = Pace {
            prior: Some(display),
            ..Default::default()
        };
        // From the first frame, before any gap.
        assert_eq!(pace.interval(), Some(display));
        let mut at = Instant::now();
        // The app draws at 100 Hz instead: the measured gaps take over once there are enough.
        for frame in 0..PACE_KNOWN {
            assert_eq!(pace.interval(), Some(display), "frame {frame}");
            at += Duration::from_millis(10);
            pace.draw(at);
        }
        // `PACE_KNOWN` draws give one gap fewer: one more.
        assert_eq!(pace.interval(), Some(display));
        at += Duration::from_millis(10);
        pace.draw(at);
        assert_eq!(pace.interval(), Some(Duration::from_millis(10)));
    }

    #[test]
    fn a_frame_filled_in_before_the_pace_is_measured_still_counts() {
        let mut pace = Pace {
            prior: Some(Duration::from_micros(16_667)),
            ..Default::default()
        };
        let at = Instant::now();
        pace.draw(at);
        pace.filled_at = Some(at + Duration::from_millis(18));
        // The app draws 3ms after the filled frame, its second frame: Chromium paints for it already.
        assert!(pace.draw(at + Duration::from_millis(21)));
    }

    #[test]
    fn a_reply_goes_to_its_call_with_its_result_or_error() {
        let (id, answer) = answer_of(r#"{"id":268435457,"result":{"data":"x"}}"#).expect("a reply");
        assert_eq!(id, CALL_IDS + 1);
        assert_eq!(answer, Ok(serde_json::json!({"data":"x"})));
        let (_, answer) = answer_of(r#"{"id":5,"error":{"code":-32000,"message":"No node"}}"#).expect("a reply");
        assert_eq!(answer, Err("No node".into()));
        assert!(answer_of(r#"{"method":"Page.loadEventFired","params":{}}"#).is_none());
        assert!(CALL_IDS + 1_000_000 < i32::MAX as u64);
    }
}
