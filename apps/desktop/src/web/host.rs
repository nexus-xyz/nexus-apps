//! The web host process: CEF and every page, outside the app. The app starts this same binary with
//! [`HOST_ARG`](super::protocol::HOST_ARG) on its first page, and closes its stdin a while after its last page closes
//! (`client.rs`). So the app's own process never loads Chromium's state, gets its memory back when no page is open,
//! and survives a crash here. CEF's message loop runs on this process's main thread, not the app's UI thread.
//!
//! The main thread owns CEF: it waits for a command from the app, a turn CEF asked for, or the fallback tick, and
//! handles each in order. A reader thread turns stdin's lines into commands. Events go out on the protocol pipe, a
//! copy of stdout taken at start; stdout itself then points at stderr, so nothing Chromium prints can break a line.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cef::{ImplBrowser, ImplBrowserHost, ImplDragData, ImplFrame};

use super::handlers::{self, StateRef};
use super::protocol::{
    Button, Command, Cursor, Event, FRAME_RATES, FileDragStep, KeyKind, MouseAction, POPUP_IDS, WebProfile,
};

/// CEF's message loop runs when CEF asks for it (`on_schedule_message_pump_work`). As in CEF's own sample app, it also
/// runs at least this often while a page is open, for work CEF does not announce.
const PUMP_FALLBACK: Duration = Duration::from_millis(33);
/// And this often when no page is open.
const PUMP_IDLE: Duration = Duration::from_secs(1);
/// A turn of CEF's work this long gets one more turn at once (`run`).
const BUSY_TURN: Duration = Duration::from_millis(1);
/// At most this many waiting messages go in one turn of the loop before CEF's work (`run`).
const BATCH: usize = 32;
/// How long the host waits for its pages to close at exit before it stops CEF anyway.
const CLOSE_WAIT: Duration = Duration::from_secs(2);
/// A hidden blank page the host opens at start and closes again. It starts Chromium's first page process, and
/// Chromium keeps a spare one ready after it, so the app's first page skips that start: about 150ms, measured.
/// On macOS `process_limits.rs` turns the spare off, so there the primer only starts the GPU and network
/// services before the first page. Its saving on macOS is not measured.
const PRIMER: u32 = u32::MAX;
/// The blank page closes after this long, or as soon as the app opens a page of its own.
const PRIMER_FOR: Duration = Duration::from_secs(1);
/// A paced page the app has not asked for a frame for this long gets one from the host, so a page that changes on
/// its own (a timer, a load) still shows it. Its first new frame then has the app pace it again.
const IDLE_FRAME: Duration = Duration::from_millis(33);

enum Message {
    Command(Command),
    /// CEF asks for a turn in this many milliseconds.
    Pump(i64),
    /// The app closed the pipe: time to exit.
    Closed,
    /// Paint the page again: its last frame was dropped (`frames/linux.rs`).
    #[cfg(target_os = "linux")]
    Repaint(u32),
}

static MAIL: OnceLock<Sender<Message>> = OnceLock::new();
static OUT: OnceLock<Mutex<Box<dyn Write + Send>>> = OnceLock::new();
/// Browsers created and not yet closed (`on_before_close`).
static OPEN: AtomicUsize = AtomicUsize::new(0);

pub(super) fn schedule_pump(delay_ms: i64) {
    if let Some(mail) = MAIL.get() {
        let _ = mail.send(Message::Pump(delay_ms));
    }
}

pub(super) fn browser_closed() {
    let _ = OPEN.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
}

/// Send `event` to the app.
pub(super) fn emit(event: &Event) {
    let Some(out) = OUT.get() else {
        return;
    };
    let Ok(line) = serde_json::to_string(event) else {
        return;
    };
    let mut out = out.lock().expect("protocol pipe");
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// The engine's cache folder (`Settings::root_cache_path`): every saved website profile is a folder under it.
static ROOT: OnceLock<PathBuf> = OnceLock::new();

// Popups the engine created and the main loop has not taken into its pages yet, and pages that closed themselves.
// CEF calls the handlers on this thread, so these need no lock.
thread_local! {
    static ADOPTED: RefCell<Vec<(u32, cef::Browser, StateRef)>> = const { RefCell::new(Vec::new()) };
    static CLOSED: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    /// Pages asked for and not created yet (a page in a website profile is created once its context is ready), and
    /// those of them the app closed already.
    static PENDING: RefCell<HashMap<u32, StateRef>> = RefCell::new(HashMap::new());
    static CLOSED_EARLY: RefCell<std::collections::HashSet<u32>> = RefCell::new(std::collections::HashSet::new());
    /// Keep a page alive until CEF exposes its native host and accepts its close request. Such a page stays in
    /// `pages` on purpose. It has no host yet, so `idle_frames` skips it. It counts as an explicit page for the
    /// primer, which only retires the primer earlier. The app sends nothing for a page after closing it (ids are
    /// never reused), so no other command reaches it.
    static CLOSING: RefCell<std::collections::HashSet<u32>> = RefCell::new(std::collections::HashSet::new());
    /// DevTools messages for pages not created yet, sent once they are, in order.
    static EARLY_CDP: RefCell<HashMap<u32, Vec<String>>> = RefCell::new(HashMap::new());
    /// Each website profile's request context, made on its first page and kept while the host runs.
    static CONTEXTS: RefCell<HashMap<WebProfile, cef::RequestContext>> = RefCell::new(HashMap::new());
    /// Each session's open popups, against `nexus_browser::MAX_TABS`.
    static POPUPS: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
}
static NEXT_POPUP: AtomicU32 = AtomicU32::new(POPUP_IDS);

/// A popup's id, from the host's own range.
pub(super) fn popup_id() -> u32 {
    NEXT_POPUP.fetch_add(1, Ordering::Relaxed)
}

/// Whether `session` may open another popup; counts it when it may.
pub(super) fn take_popup(session: &str) -> bool {
    POPUPS.with_borrow_mut(|open| {
        let count = open.entry(session.to_string()).or_default();
        if *count >= nexus_browser::MAX_TABS {
            return false;
        }
        *count += 1;
        true
    })
}

/// A popup of `session` closed, or never opened.
pub(super) fn drop_popup(session: &str) {
    POPUPS.with_borrow_mut(|open| {
        if let Some(count) = open.get_mut(session) {
            *count = count.saturating_sub(1);
        }
    });
}

/// The engine created popup `id`: the main loop takes it into its pages.
pub(super) fn adopt(id: u32, browser: cef::Browser, state: StateRef) {
    ADOPTED.with_borrow_mut(|adopted| adopted.push((id, browser, state)));
}

/// What the main loop did, for `NEXUS_WEB_TRACE`'s summary every 2s: commands, wheel events, begin frames sent (by
/// the app's ask and idle), message loop runs, pages' paints, and CPU frames sent with the pixels they wrote.
#[derive(Default)]
struct LoopStats {
    since: Option<Instant>,
    commands: u32,
    wheels: u32,
    begun: u32,
    idle_begun: u32,
    works: u32,
    /// The app's begin frames: when the last went, and how many came under 10ms or over 24ms after the one before.
    last_begun: Option<Instant>,
    close: u32,
    far: u32,
    paints: u32,
    /// From the last begin frame to the paint, the slowest, and how many took over 16ms; the slowest paint callback.
    slowest_paint: Duration,
    slow_paints: u32,
    longest_callback: Duration,
    /// The longest wait from CEF asking for work now to the work, the longest gap between works, and the longest work.
    pump_lag: Duration,
    work_gap: Duration,
    longest_work: Duration,
    longest_command: Duration,
    cpu_frames: u32,
    dirty_pixels: u64,
}

thread_local! {
    static STATS: RefCell<LoopStats> = RefCell::new(LoopStats::default());
}

/// A page painted; its paint callback started at `started` (on the main loop's thread).
pub(super) fn painted(started: Instant) {
    count(|stats| {
        stats.paints += 1;
        let now = Instant::now();
        stats.longest_callback = stats.longest_callback.max(now.duration_since(started));
        if let Some(gap) = stats.last_begun.map(|at| started.duration_since(at))
            && gap < Duration::from_millis(100)
        {
            stats.slowest_paint = stats.slowest_paint.max(gap);
            stats.slow_paints += u32::from(gap > Duration::from_millis(16));
        }
    });
}

fn count(change: impl FnOnce(&mut LoopStats)) {
    if super::trace() {
        STATS.with_borrow_mut(change);
    }
}

fn summarise_loop(pages: usize) {
    if !super::trace() || pages == 0 {
        return;
    }
    STATS.with_borrow_mut(|stats| {
        let since = *stats.since.get_or_insert_with(Instant::now);
        if since.elapsed() < Duration::from_secs(2) {
            return;
        }
        eprintln!(
            "nexus web trace: host loop: {} commands, {} wheels, {} begin frames ({} close, {} far) + {} idle, {} loop runs (pump lag max {:.1}ms, gap max {:.1}ms, work max {:.1}ms, command max {:.1}ms), {} paints (begin to paint max {:.1}ms, {} over 16ms; callback max {:.1}ms), {} CPU frames ({} dirty px/frame) in {:.1}s",
            stats.commands,
            stats.wheels,
            stats.begun,
            stats.close,
            stats.far,
            stats.idle_begun,
            stats.works,
            stats.pump_lag.as_secs_f32() * 1000.,
            stats.work_gap.as_secs_f32() * 1000.,
            stats.longest_work.as_secs_f32() * 1000.,
            stats.longest_command.as_secs_f32() * 1000.,
            stats.paints,
            stats.slowest_paint.as_secs_f32() * 1000.,
            stats.slow_paints,
            stats.longest_callback.as_secs_f32() * 1000.,
            stats.cpu_frames,
            stats.dirty_pixels / u64::from(stats.cpu_frames.max(1)),
            since.elapsed().as_secs_f32()
        );
        *stats = LoopStats { since: Some(Instant::now()), last_begun: stats.last_begun, ..Default::default() };
    });
}

/// Ask the main loop to paint page `id` again, from any thread.
#[cfg(target_os = "linux")]
pub(super) fn repaint(id: u32) {
    if let Some(mail) = MAIL.get() {
        let _ = mail.send(Message::Repaint(id));
    }
}

/// Page `id` closed itself: the main loop lets it go.
pub(super) fn closed(id: u32) {
    CLOSED.with_borrow_mut(|closed| closed.push(id));
}

/// Take adopted popups in, and closed pages out.
fn settle(pages: &mut HashMap<u32, Page>) {
    for (id, browser, state) in ADOPTED.with_borrow_mut(std::mem::take) {
        PENDING.with_borrow_mut(|pending| pending.remove(&id));
        let early = EARLY_CDP.with_borrow_mut(|early| early.remove(&id)).unwrap_or_default();
        let closed_early = CLOSED_EARLY.with_borrow_mut(|closed| closed.remove(&id));
        let mut page = Page {
            browser,
            state,
            dev_tools: None,
        };
        if closed_early {
            // Its native host may still be unavailable: retain it until the queued close can complete.
            pages.insert(id, page);
            request_close(id, pages);
            continue;
        }
        for message in early {
            send_cdp(id, &mut page, &message);
        }
        let late_frame_rate = page.state.lock().expect("page state").late_frame_rate.take();
        if let (Some(fps), Some(host)) = (late_frame_rate, page.host()) {
            host.set_windowless_frame_rate(fps);
        }
        pages.insert(id, page);
    }
    for id in CLOSED.with_borrow_mut(std::mem::take) {
        pages.remove(&id);
        super::page_drag::forget(id);
        #[cfg(target_os = "linux")]
        super::frames::forget(id);
    }
    close_pages(pages);
}

/// Ask each paced page the app has not asked lately for a frame.
fn idle_frames(pages: &HashMap<u32, Page>) {
    let now = Instant::now();
    for page in pages.values() {
        {
            let mut state = page.state.lock().expect("page state");
            if !state.paced || state.begun.is_some_and(|at| now.duration_since(at) < IDLE_FRAME) {
                continue;
            }
            state.begun = Some(now);
        }
        if let Some(host) = page.host() {
            count(|stats| stats.idle_begun += 1);
            host.send_external_begin_frame();
        }
    }
}

/// A page's state, shared with its handlers.
pub(super) struct PageState {
    /// The view's size in logical pixels, and the screen's scale.
    pub(super) size: (i32, i32),
    pub(super) scale: f32,
    pub(super) cursor: Cursor,
    /// The session whose browser tab this is (`Command::Open`'s `browser`), if it is one.
    pub(super) browser: Option<String>,
    /// A read-only document (`Command::Open`'s `document`).
    pub(super) document: bool,
    /// The address the page hands its result over by (`Command::Open`'s `handoff`).
    pub(super) handoff: Option<String>,
    /// Its CPU frames, through a memory file the app maps (`cpu.rs`).
    frames: super::cpu::Painter,
    /// A `<select>`'s open list, drawn over the frames (`popup.rs`).
    popup: super::popup::Popup,
    /// Whether the page paints only when asked (`Command::Open`'s `paced`), and when it was last asked.
    paced: bool,
    begun: Option<Instant>,
    /// A frame rate the app sent before the engine made the page (`Command::FrameRate`), set on it then.
    late_frame_rate: Option<i32>,
}

impl PageState {
    pub(super) fn new(id: u32, size: (i32, i32), scale: f32, browser: Option<String>) -> PageState {
        PageState {
            size,
            scale,
            cursor: Cursor::Arrow,
            browser,
            document: false,
            handoff: None,
            frames: super::cpu::Painter::new(id),
            popup: super::popup::Popup::default(),
            paced: false,
            begun: None,
            late_frame_rate: None,
        }
    }

    /// Chromium painted a frame, of which `dirty` changed (none: all of it): hand it to the app, or keep it until the
    /// app has read the last one.
    pub(super) fn paint(&mut self, id: u32, pixels: &[u8], width: u32, height: u32, dirty: &[[i32; 4]]) {
        if self.popup.shown()
            && let Some(composed) = self.popup.paint_page(pixels, width, height, dirty, self.scale)
        {
            self.send_composed(id, composed);
            return;
        }
        let sent = self.frames.paint(pixels, width, height, dirty);
        send_frame(id, sent);
    }

    /// Chromium painted the page's popup (a `<select>`'s open list).
    pub(super) fn paint_popup(&mut self, id: u32, pixels: &[u8], width: u32, height: u32) {
        if let Some(composed) = self.popup.paint_popup(pixels, width, height, self.scale) {
            self.send_composed(id, composed);
        }
    }

    pub(super) fn show_popup(&mut self, id: u32, shown: bool) {
        if shown {
            self.popup.show();
        } else if let Some(page) = self.popup.hide(self.scale) {
            self.send_composed(id, page);
        }
    }

    pub(super) fn place_popup(&mut self, id: u32, rect: [i32; 4]) {
        if let Some(composed) = self.popup.place(rect, self.scale) {
            self.send_composed(id, composed);
        }
    }

    fn send_composed(&mut self, id: u32, composed: super::popup::Composed) {
        let sent = self
            .frames
            .paint(&composed.pixels, composed.width, composed.height, &composed.dirty);
        send_frame(id, sent);
    }

    /// The page shows `cursor`: tell the app when it changed.
    pub(super) fn set_cursor(&mut self, id: u32, cursor: Cursor) {
        if std::mem::replace(&mut self.cursor, cursor) != cursor {
            emit(&Event::Cursor { id, cursor });
        }
    }

    /// The app read the last frame: send what was painted since, if anything was.
    fn ack(&mut self, id: u32) {
        let sent = self.frames.ack();
        send_frame(id, sent);
    }
}

fn send_frame(id: u32, sent: std::io::Result<Option<super::cpu::Sent>>) {
    match sent {
        Ok(Some(sent)) => {
            count(|stats| {
                stats.cpu_frames += 1;
                stats.dirty_pixels += sent.pixels;
            });
            emit(&Event::Frame {
                id,
                path: sent.path,
                width: sent.width,
                height: sent.height,
                seq: sent.seq,
                dirty: sent.dirty,
            });
        }
        Ok(None) => {}
        Err(error) => eprintln!("nexus web host: frame for page {id}: {error}"),
    }
}

struct Page {
    browser: cef::Browser,
    state: StateRef,
    /// Kept while the page's DevTools messages go to the app; dropping it stops them.
    dev_tools: Option<cef::Registration>,
}

impl Page {
    fn host(&self) -> Option<cef::BrowserHost> {
        self.browser.host()
    }
}

/// Run the host until the app closes the pipe. Returns the process's exit code.
pub(super) fn run() -> i32 {
    let _ = OUT.set(Mutex::new(protocol_out()));
    #[cfg(target_os = "linux")]
    super::frames::host_start();
    #[cfg(target_os = "linux")]
    super::cpu::sweep();
    let (mail, inbox) = mpsc::channel();
    let _ = MAIL.set(mail.clone());
    std::thread::Builder::new()
        .name("web-host-commands".into())
        .spawn(move || read_commands(mail))
        .expect("the command reader starts");
    // On macOS, CEF's framework loads from the app bundle and stays loaded while the host runs.
    #[cfg(target_os = "macos")]
    let _framework = {
        let framework = std::env::current_exe()
            .ok()
            .and_then(|exe| super::mac::load_framework(&exe, false));
        if framework.is_none() {
            emit(&Event::Ready { ok: false });
            return 1;
        }
        super::mac::install_application();
        framework
    };
    if !start_engine() {
        emit(&Event::Ready { ok: false });
        return 1;
    }
    emit(&Event::Ready { ok: true });
    let mut pages: HashMap<u32, Page> = HashMap::new();
    let mut primed = prime(&mut pages);
    let mut due: Option<Instant> = Some(Instant::now());
    let mut worked = Instant::now();
    let mut asked_now: Option<Instant> = None;
    // The last turn was the extra turn after a long one (below).
    let mut extra = false;
    'run: loop {
        settle(&mut pages);
        if primed.is_some_and(|at| at.elapsed() >= PRIMER_FOR || pages.keys().any(|id| *id != PRIMER)) {
            primed = None;
            request_close(PRIMER, &mut pages);
        }
        idle_frames(&pages);
        // CEF's work runs when it asks for it, and at least every `fallback` from the last run. The deadline is fixed
        // in time, not renewed with each command: a steady stream of them (a touchpad scroll's wheel events and the
        // app's begin frames) would otherwise put CEF's work off for as long as it lasts, and the page would freeze.
        let fallback = if pages.is_empty() { PUMP_IDLE } else { PUMP_FALLBACK };
        let deadline = due.map_or(worked + fallback, |at| at.min(worked + fallback));
        // The messages already waiting go in one turn, up to `BATCH`, before CEF's work: a page's open, its startup
        // and its first address then reach the engine together, not one per turn of CEF's work.
        let mut next = Some(inbox.recv_timeout(deadline.saturating_duration_since(Instant::now())));
        let mut taken = 0;
        while let Some(message) = next.take() {
            match message {
                Ok(Message::Command(command)) => {
                    let started = Instant::now();
                    settle(&mut pages);
                    handle(command, &mut pages);
                    settle(&mut pages);
                    count(|stats| {
                        stats.commands += 1;
                        stats.longest_command = stats.longest_command.max(started.elapsed());
                    });
                }
                Ok(Message::Pump(delay_ms)) => {
                    if delay_ms <= 0 && asked_now.is_none() {
                        asked_now = Some(Instant::now());
                    }
                    let at = Instant::now() + Duration::from_millis(delay_ms.max(0) as u64);
                    due = Some(due.map_or(at, |due| due.min(at)));
                }
                #[cfg(target_os = "linux")]
                Ok(Message::Repaint(id)) => {
                    if let Some(host) = pages.get(&id).and_then(Page::host) {
                        host.invalidate(cef::PaintElementType::VIEW);
                        if pages[&id].state.lock().expect("page state").paced {
                            host.send_external_begin_frame();
                        }
                    }
                }
                Ok(Message::Closed) | Err(RecvTimeoutError::Disconnected) => break 'run,
                Err(RecvTimeoutError::Timeout) => break,
            }
            taken += 1;
            if taken < BATCH {
                next = inbox.try_recv().ok().map(Ok);
            }
        }
        let now = Instant::now();
        if now < due.map_or(worked + fallback, |at| at.min(worked + fallback)) {
            continue;
        }
        due = None;
        count(|stats| {
            stats.works += 1;
            stats.work_gap = stats.work_gap.max(now.duration_since(worked));
            if let Some(asked) = asked_now.take() {
                stats.pump_lag = stats.pump_lag.max(now.duration_since(asked));
            }
        });
        worked = now;
        cef::do_message_loop_work();
        settle(&mut pages);
        let took = now.elapsed();
        count(|stats| stats.longest_work = stats.longest_work.max(took));
        // CEF ends a turn when no work is left, or when its 10ms time slice (`MessagePumpExternal`) is over. Then work
        // can still be due with no ask for it, and it waits for the fallback: a begin frame for the display then comes
        // a frame late, and the page skips one. So a turn that did real work (1ms or more: a frame copy) gets one
        // extra turn at once. This rule is ours: cefclient runs again at once only after a re-entered turn. The extra
        // turn gets no extra turn of its own, so a page that keeps every turn long (an animation, a video) waits for
        // CEF's asks and the fallback. The loop does not spin.
        extra = !extra && took >= BUSY_TURN;
        if extra {
            due = Some(Instant::now());
        }
        summarise_loop(pages.len());
    }
    // Close every page, let CEF finish closing them, then stop it.
    let mut ids = pages.keys().copied().collect::<Vec<_>>();
    ids.extend(PENDING.with_borrow(|pending| pending.keys().copied().collect::<Vec<_>>()));
    for id in ids {
        request_close(id, &mut pages);
    }
    let deadline = Instant::now() + CLOSE_WAIT;
    while OPEN.load(Ordering::Relaxed) > 0 && Instant::now() < deadline {
        cef::do_message_loop_work();
        settle(&mut pages);
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(pages);
    cef::shutdown();
    0
}

/// Close through one path, including a page that is not created yet or whose native host is not ready.
fn request_close(id: u32, pages: &mut HashMap<u32, Page>) {
    if PENDING.with_borrow_mut(|pending| pending.remove(&id)).is_some() {
        CLOSED_EARLY.with_borrow_mut(|closed| closed.insert(id));
    }
    CLOSING.with_borrow_mut(|closing| closing.insert(id));
    close_pages(pages);
}

fn close_pages(pages: &mut HashMap<u32, Page>) {
    CLOSING.with_borrow_mut(|closing| {
        settle_closes(pages, closing, |page| {
            let Some(host) = page.host() else { return false };
            host.close_browser(1);
            true
        })
    });
}

/// A native resource stays owned while its close cannot run. A later turn retries the same request.
fn settle_closes<T>(
    pages: &mut HashMap<u32, T>,
    closing: &mut std::collections::HashSet<u32>,
    mut close: impl FnMut(&T) -> bool,
) {
    closing.retain(|id| {
        if pages.get(id).is_none_or(&mut close) {
            pages.remove(id);
            false
        } else {
            true
        }
    });
}

/// Open the [`PRIMER`] page. Returns when it opened.
fn prime(pages: &mut HashMap<u32, Page>) -> Option<Instant> {
    let Some(Opening::Now(page)) = open(
        PRIMER,
        "about:blank",
        (16, 16),
        1.0,
        1,
        false,
        false,
        None,
        None,
        false,
        None,
        false,
    ) else {
        return None;
    };
    if let Some(host) = page.host() {
        host.was_hidden(1);
    }
    pages.insert(PRIMER, page);
    Some(Instant::now())
}

fn read_commands(mail: Sender<Message>) {
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        match serde_json::from_str::<Command>(&line) {
            Ok(command) => {
                if mail.send(Message::Command(command)).is_err() {
                    return;
                }
            }
            Err(error) => eprintln!("nexus web host: bad command ({error}): {line}"),
        }
    }
    let _ = mail.send(Message::Closed);
}

fn handle(command: Command, pages: &mut HashMap<u32, Page>) {
    match command {
        Command::Open {
            id,
            url,
            width,
            height,
            scale,
            frame_rate,
            gpu,
            paced,
            browser,
            profile,
            document,
            handoff,
            clear,
        } => match open(
            id,
            &url,
            (width, height),
            scale,
            frame_rate,
            gpu,
            paced,
            browser,
            profile.as_ref(),
            document,
            handoff,
            clear,
        ) {
            Some(Opening::Now(page)) => {
                pages.insert(id, page);
            }
            Some(Opening::Later(state)) => {
                PENDING.with_borrow_mut(|pending| pending.insert(id, state));
            }
            None => emit(&Event::Failed { id }),
        },
        Command::Close { id } => {
            request_close(id, pages);
            super::page_drag::forget(id);
            #[cfg(target_os = "linux")]
            super::frames::forget(id);
        }
        Command::FileDrag {
            id,
            x,
            y,
            modifiers,
            step,
        } => {
            let Some(page) = pages.get(&id) else { return };
            // A read-only document takes no files.
            if page.state.lock().expect("page state").document {
                return;
            }
            if let Some(host) = page.host() {
                file_drag(&host, cef::MouseEvent { x, y, modifiers }, step);
            }
        }
        Command::Navigate { id, url } => {
            // Only a web address, in a session's browser: the same rule as its own navigations.
            let Some(page) = pages.get(&id) else { return };
            let state = page.state.lock().expect("page state");
            if state.document || (state.browser.is_some() && nexus_browser::preview::browser_url(&url).is_err()) {
                return;
            }
            drop(state);
            if let Some(frame) = page.browser.main_frame() {
                frame.load_url(Some(&url.as_str().into()));
            }
        }
        Command::History { id, step } => {
            let Some(page) = pages.get(&id) else { return };
            match step {
                ..0 => page.browser.go_back(),
                0 => page.browser.reload(),
                _ => page.browser.go_forward(),
            }
        }
        Command::Resize {
            id,
            width,
            height,
            scale,
        } => {
            let Some(page) = pages.get(&id) else {
                // Not created yet: it starts at the new size.
                if let Some(state) = PENDING.with_borrow(|pending| pending.get(&id).cloned()) {
                    let mut state = state.lock().expect("page state");
                    state.size = (width, height);
                    state.scale = scale;
                }
                return;
            };
            {
                let mut state = page.state.lock().expect("page state");
                state.size = (width, height);
                state.scale = scale;
            }
            if let Some(host) = page.host() {
                host.notify_screen_info_changed();
                host.was_resized();
            }
        }
        Command::BeginFrame { id } => {
            let Some(page) = pages.get(&id) else { return };
            {
                let mut state = page.state.lock().expect("page state");
                if !state.paced {
                    return;
                }
                state.begun = Some(Instant::now());
            }
            if let Some(host) = page.host() {
                count(|stats| {
                    stats.begun += 1;
                    let now = Instant::now();
                    if let Some(gap) = stats.last_begun.map(|at| now.duration_since(at)) {
                        stats.close += u32::from(gap < Duration::from_millis(10));
                        stats.far += u32::from(gap > Duration::from_millis(24) && gap < Duration::from_millis(100));
                    }
                    stats.last_begun = Some(now);
                });
                host.send_external_begin_frame();
            }
        }
        Command::FrameRate { id, fps } => {
            let fps = fps.clamp(*FRAME_RATES.start(), *FRAME_RATES.end());
            match pages.get(&id) {
                // Chromium's begin frame interval and the most frames a second it hands over, paced or not.
                Some(page) => {
                    if let Some(host) = page.host() {
                        host.set_windowless_frame_rate(fps);
                    }
                }
                // Not created yet: it gets the rate when it is (`settle`).
                None => {
                    if let Some(state) = PENDING.with_borrow(|pending| pending.get(&id).cloned()) {
                        state.lock().expect("page state").late_frame_rate = Some(fps);
                    }
                }
            }
        }
        Command::Hidden { id, hidden } => {
            if let Some(host) = pages.get(&id).and_then(Page::host) {
                host.was_hidden(hidden as i32);
            }
        }
        Command::Focus { id, focused } => {
            if let Some(host) = pages.get(&id).and_then(Page::host) {
                host.set_focus(focused as i32);
            }
        }
        Command::Mouse {
            id,
            x,
            y,
            modifiers,
            action,
        } => {
            let Some(host) = pages.get(&id).and_then(Page::host) else {
                return;
            };
            let event = cef::MouseEvent { x, y, modifiers };
            // A drag inside the page takes the mouse while it lasts (`page_drag.rs`).
            if let Some(handled) = super::page_drag::mouse(id, &host, &event, action) {
                if let Some(cursor) = handled.restore
                    && let Some(page) = pages.get(&id)
                {
                    page.state.lock().expect("page state").set_cursor(id, cursor);
                }
                if !handled.pass {
                    return;
                }
            }
            match action {
                MouseAction::Move => host.send_mouse_move_event(Some(&event), 0),
                MouseAction::Leave => host.send_mouse_move_event(Some(&event), 1),
                MouseAction::Down { button, clicks } | MouseAction::Up { button, clicks } => {
                    let up = matches!(action, MouseAction::Up { .. });
                    let kind = match button {
                        Button::Left => cef::MouseButtonType::LEFT,
                        Button::Middle => cef::MouseButtonType::MIDDLE,
                        Button::Right => cef::MouseButtonType::RIGHT,
                    };
                    host.send_mouse_click_event(Some(&event), kind, up as i32, clicks.clamp(1, 3));
                }
            }
        }
        Command::Wheel {
            id,
            x,
            y,
            modifiers,
            dx,
            dy,
        } => {
            count(|stats| stats.wheels += 1);
            if let Some(host) = pages.get(&id).and_then(Page::host) {
                host.send_mouse_wheel_event(Some(&cef::MouseEvent { x, y, modifiers }), dx, dy);
            }
        }
        Command::Key { id, key } => {
            let Some(host) = pages.get(&id).and_then(Page::host) else {
                return;
            };
            // Escape cancels a drag inside the page, as in a browser.
            if key.kind == KeyKind::RawDown
                && key.code == ESCAPE
                && let Some(cursor) = super::page_drag::cancel(id, &host)
                && let Some(page) = pages.get(&id)
            {
                page.state.lock().expect("page state").set_cursor(id, cursor);
            }
            host.send_key_event(Some(&cef::KeyEvent {
                size: std::mem::size_of::<cef::sys::cef_key_event_t>(),
                type_: match key.kind {
                    KeyKind::RawDown => cef::KeyEventType::RAWKEYDOWN,
                    KeyKind::Char => cef::KeyEventType::CHAR,
                    KeyKind::Up => cef::KeyEventType::KEYUP,
                },
                modifiers: key.modifiers,
                windows_key_code: key.code,
                character: key.character,
                unmodified_character: key.character,
                ..Default::default()
            }));
        }
        Command::Ack { id } => {
            if let Some(page) = pages.get(&id) {
                page.state.lock().expect("page state").ack(id);
            }
        }
        Command::Cdp { id, message } => match pages.get_mut(&id) {
            Some(page) => send_cdp(id, page, &message),
            // A page in a website profile is created a moment later: its messages wait for it.
            None if PENDING.with_borrow(|pending| pending.contains_key(&id)) => {
                EARLY_CDP.with_borrow_mut(|early| early.entry(id).or_default().push(message));
            }
            None => {}
        },
    }
}

/// Escape's Windows virtual-key code (`keys.rs`).
const ESCAPE: i32 = 0x1B;

/// The operations a file from the system allows: copy, link or move, as a file manager offers them.
const FILE_DRAG_OPERATIONS: u32 = 1 | 2 | 16;

/// A step of a system file drag over a page (`file_drag.rs`), as an HTML drag and drop with the files.
fn file_drag(host: &cef::BrowserHost, event: cef::MouseEvent, step: FileDragStep) {
    let allowed = || super::page_drag::mask(FILE_DRAG_OPERATIONS);
    match step {
        FileDragStep::Enter { paths } => {
            let Some(mut data) = cef::drag_data_create() else {
                return;
            };
            for path in &paths {
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                data.add_file(Some(&path.as_str().into()), Some(&name.as_str().into()));
            }
            host.drag_target_drag_enter(Some(&mut data), Some(&event), allowed());
        }
        FileDragStep::Over => host.drag_target_drag_over(Some(&event), allowed()),
        FileDragStep::Leave => host.drag_target_drag_leave(),
        FileDragStep::Drop => host.drag_target_drop(Some(&event)),
    }
}

/// Send page `id` a DevTools message. The first one turns the replies on: until then, nothing listens.
fn send_cdp(id: u32, page: &mut Page, message: &str) {
    let Some(host) = page.host() else { return };
    if page.dev_tools.is_none() {
        page.dev_tools = host.add_dev_tools_message_observer(Some(&mut handlers::dev_tools(id)));
    }
    if host.send_dev_tools_message(Some(message.as_bytes())) == 0 {
        eprintln!("nexus web host: page {id} refused a DevTools message");
    }
}

/// The engine's background (ARGB) under what a page paints: opaque white until the page paints, as a browser tab; or
/// none for a clear page (`Command::Open`'s `clear`), so the app's surface shows where the page leaves it unpainted.
fn background_color(clear: bool) -> u32 {
    if clear { 0x0000_0000 } else { 0xFFFF_FFFF }
}

fn open(
    id: u32,
    url: &str,
    size: (i32, i32),
    scale: f32,
    frame_rate: i32,
    gpu: bool,
    paced: bool,
    browser: Option<String>,
    profile: Option<&WebProfile>,
    document: bool,
    handoff: Option<String>,
    clear: bool,
) -> Option<Opening> {
    // A session browser opens only in its own profile: never in another's, or the default one, if that fails.
    let mut context = match profile {
        Some(profile) => Some(context(profile)?),
        None => None,
    };
    let mut page_state = PageState::new(id, size, scale, browser);
    page_state.document = document;
    page_state.handoff = handoff;
    page_state.paced = paced;
    let state: StateRef = Arc::new(Mutex::new(page_state));
    #[cfg(target_os = "linux")]
    let gpu = gpu && super::frames::host_ready();
    #[cfg(not(target_os = "linux"))]
    let gpu = {
        let _ = gpu;
        false
    };
    if super::trace() {
        eprintln!("nexus web trace: page {id} opens, GPU frames {gpu}, paced {paced}");
    }
    let window_info = cef::WindowInfo {
        windowless_rendering_enabled: 1,
        // Frames in GPU buffers (`on_accelerated_paint`). Chromium then never calls `on_paint`: where it can't make the
        // buffers, the page paints nothing, which `frames::wanted` and the app's first-frame wait guard against.
        shared_texture_enabled: gpu as i32,
        // Paced: Chromium paints when the app draws (`Command::BeginFrame`), so its frames land on the screen's
        // refresh. On its own timer they drift against it, and motion judders: some refreshes get two frames, some none.
        external_begin_frame_enabled: paced as i32,
        ..Default::default()
    };
    let settings = cef::BrowserSettings {
        windowless_frame_rate: frame_rate.clamp(*FRAME_RATES.start(), *FRAME_RATES.end()),
        background_color: background_color(clear),
        // A read-only document runs no script, as the Nexus app's `sandbox=""` frame.
        javascript: if document {
            cef::State::DISABLED
        } else {
            Default::default()
        },
        ..Default::default()
    };
    if let Some(context) = context.as_mut() {
        // A new request context gets ready asynchronously, so the page is created when it is (`on_after_created`).
        let asked = cef::browser_host_create_browser(
            Some(&window_info),
            Some(&mut handlers::later_client(id, &state)),
            Some(&url.into()),
            Some(&settings),
            None,
            Some(context),
        );
        if asked != 1 {
            return None;
        }
        OPEN.fetch_add(1, Ordering::Relaxed);
        return Some(Opening::Later(state));
    }
    let browser = cef::browser_host_create_browser_sync(
        Some(&window_info),
        Some(&mut handlers::client(id, &state)),
        Some(&url.into()),
        Some(&settings),
        None,
        None,
    )?;
    OPEN.fetch_add(1, Ordering::Relaxed);
    Some(Opening::Now(Page {
        browser,
        state,
        dev_tools: None,
    }))
}

/// A page asked for: made now, or (in a website profile) once the engine has made it.
enum Opening {
    Now(Page),
    Later(StateRef),
}

fn start_engine() -> bool {
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let resources = super::cef_resources(&exe);
    let cache = crate::paths::cache_dir().join("cef");
    let _ = ROOT.set(cache.clone());
    // On macOS, CEF finds the helper apps, its resources and its locales in the bundle by itself.
    let (subprocess, resources_dir, locales_dir) = if cfg!(target_os = "macos") {
        Default::default()
    } else {
        (
            exe.to_string_lossy().into_owned(),
            resources.to_string_lossy().into_owned(),
            resources.join("locales").to_string_lossy().into_owned(),
        )
    };
    let settings = cef::Settings {
        windowless_rendering_enabled: 1,
        external_message_pump: 1,
        // The helper processes are this binary; `run_subprocess` handles them.
        browser_subprocess_path: subprocess.as_str().into(),
        resources_dir_path: resources_dir.as_str().into(),
        locales_dir_path: locales_dir.as_str().into(),
        // A shipped build carries only the English locale files (`packaging/`): Chromium's own strings (a context
        // menu, an error page) are all they translate, and the other languages cost 50 MB. Pages see `en-US` as the
        // browser's language.
        locale: "en-US".into(),
        root_cache_path: cache.to_string_lossy().as_ref().into(),
        // The pages' profile (cookies, storage) on disk; in memory when the organization keeps nothing on this computer
        // (`AllowLocalCache` off).
        cache_path: profile_path(&cache, crate::managed::local_cache(crate::managed::policy()))
            .as_str()
            .into(),
        log_severity: cef::LogSeverity::WARNING,
        no_sandbox: (!sandbox_on()).into(),
        ..Default::default()
    };
    let args = cef::args::Args::new();
    #[cfg(target_os = "linux")]
    let gpu = super::frames::host_ready();
    #[cfg(not(target_os = "linux"))]
    let gpu = false;
    // Chromium shares GPU frames only through X11 (`frames::wanted`, `frames/mod.rs`).
    let ozone = if gpu { Some("x11".to_string()) } else { super::ozone() };
    let mut app = handlers::NexusApp::new(ozone, gpu);
    let started = cef::initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) == 1;
    if !started {
        eprintln!(
            "nexus web host: CEF did not start (resources at {})",
            resources.display()
        );
    }
    started
}

/// Whether Chromium's processes run in its sandbox. Off unless `NEXUS_CEF_SANDBOX=1`: it is opt-in while people try it,
/// and a later change turns it on by default.
///
/// When asked for on Linux, it is on if the kernel lets an unprivileged process make the namespaces Chromium's sandbox
/// uses ([`namespace_sandbox_works`]). Without them, Chromium wants `chrome-sandbox` installed setuid root, which a
/// package unpacked by the person cannot be, and it aborts rather than run unsandboxed: no page would open. So there the
/// pages run without it, and the log says so. macOS and Windows keep it off: there each helper process must link CEF's
/// sandbox library first.
fn sandbox_on() -> bool {
    if !std::env::var("NEXUS_CEF_SANDBOX").is_ok_and(|value| value == "1") {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let works = namespace_sandbox_works();
        if !works {
            eprintln!(
                "nexus web host: Chromium's sandbox cannot start here (no usable user namespaces), so pages run \
                 without it"
            );
        }
        works
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("nexus web host: NEXUS_CEF_SANDBOX=1 has no effect on this platform yet");
        false
    }
}

/// Whether Chromium's namespace sandbox can start here. A child process makes the checks Chromium's zygote host makes
/// before it picks that sandbox (`Credentials::CanCreateProcessInNewUserNS`, `sandbox/linux/services/credentials.cc`),
/// then exits; this process's own namespaces never change. In a new user namespace (here with the PID and network
/// namespaces the sandbox also uses), it maps its user and group, drops every capability, and makes a nested user
/// namespace. Any step that fails means Chromium would find no usable sandbox. That covers unprivileged user
/// namespaces turned off (`kernel.unprivileged_userns_clone=0`, an exhausted `user.max_user_namespaces`, a container
/// that forbids them) and a namespace with no rights inside it (Ubuntu's `apparmor_restrict_unprivileged_userns`).
#[cfg(target_os = "linux")]
fn namespace_sandbox_works() -> bool {
    // Built before `fork`: the child must not allocate.
    // SAFETY: plain reads of this process's ids.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let uid_map = format!("{uid} {uid} 1");
    let gid_map = format!("{gid} {gid} 1");
    // SAFETY: between `fork` and `_exit` the child calls only system calls (`unshare`, `open`, `write`, `close`,
    // `capset`) on memory made before the fork, so forking while other threads run is sound. The parent waits for that
    // one child.
    unsafe {
        let child = libc::fork();
        if child == 0 {
            let works = libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWPID | libc::CLONE_NEWNET) == 0
                && write_proc(c"/proc/self/setgroups", b"deny")
                && write_proc(c"/proc/self/uid_map", uid_map.as_bytes())
                && write_proc(c"/proc/self/gid_map", gid_map.as_bytes())
                && drop_capabilities()
                && libc::unshare(libc::CLONE_NEWUSER) == 0;
            libc::_exit(if works { 0 } else { 1 });
        }
        if child < 0 {
            return false;
        }
        let mut status = 0;
        loop {
            let done = libc::waitpid(child, &mut status, 0);
            if done == child {
                return libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;
            }
            if done < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return false;
            }
        }
    }
}

/// Write `data` to a `/proc` file in one `write`, as the kernel wants for a namespace's maps. For the child of
/// [`namespace_sandbox_works`] only: it uses nothing but system calls.
#[cfg(target_os = "linux")]
unsafe fn write_proc(path: &std::ffi::CStr, data: &[u8]) -> bool {
    // SAFETY: `path` is a valid C string and `data` a valid buffer; the descriptor is closed before returning.
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
        if fd < 0 {
            return false;
        }
        let written = libc::write(fd, data.as_ptr().cast(), data.len());
        libc::close(fd);
        written == data.len() as isize
    }
}

/// Drop every capability of this process, as Chromium's check does before its nested namespace. For the child of
/// [`namespace_sandbox_works`] only.
#[cfg(target_os = "linux")]
unsafe fn drop_capabilities() -> bool {
    #[repr(C)]
    struct Header {
        version: u32,
        pid: libc::c_int,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Data {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    // `_LINUX_CAPABILITY_VERSION_3`: two `Data` words, all zero for no capabilities.
    let mut header = Header {
        version: 0x2008_0522,
        pid: 0,
    };
    let data = [Data {
        effective: 0,
        permitted: 0,
        inheritable: 0,
    }; 2];
    // SAFETY: `capset` reads the header and two data words, both valid for the call.
    unsafe { libc::syscall(libc::SYS_capset, &mut header as *mut Header, data.as_ptr()) == 0 }
}

/// The protocol pipe: the stdout the app reads. On Unix, stdout is then pointed at stderr, so a stray print from CEF,
/// Chromium or a helper process lands in the log, not in the middle of an event.
#[cfg(unix)]
fn protocol_out() -> Box<dyn Write + Send> {
    use std::os::fd::FromRawFd;
    // SAFETY: plain descriptor calls on this process's own stdout and stderr, before any other thread writes.
    unsafe {
        let pipe = libc::fcntl(libc::STDOUT_FILENO, libc::F_DUPFD_CLOEXEC, 3);
        if pipe < 0 || libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) < 0 {
            return Box::new(std::io::stdout());
        }
        Box::new(std::fs::File::from_raw_fd(pipe))
    }
}

#[cfg(not(unix))]
fn protocol_out() -> Box<dyn Write + Send> {
    Box::new(std::io::stdout())
}

/// The request context of a session browser's website profile: made once, then shared by all of its pages, so its
/// cookies and storage reach that sign-in's browsers and no other's. Popups inherit their opener's.
fn context(profile: &WebProfile) -> Option<cef::RequestContext> {
    if let Some(context) = CONTEXTS.with_borrow(|contexts| contexts.get(profile).cloned()) {
        return Some(context);
    }
    let root = ROOT.get()?;
    let path = profile_folder(root, profile)?;
    let settings = cef::RequestContextSettings {
        cache_path: path.as_str().into(),
        persist_session_cookies: profile.persistent as i32,
        ..Default::default()
    };
    let context = cef::request_context_create_context(Some(&settings), None)?;
    CONTEXTS.with_borrow_mut(|contexts| contexts.insert(profile.clone(), context.clone()));
    Some(context)
}

/// A website profile's folder under the engine's cache (`browser-<id>`: Chromium makes a profile only directly under
/// the cache folder), or none (in memory) when it is not saved.
/// `None` for a folder name that is not a plain id.
fn profile_folder(root: &std::path::Path, profile: &WebProfile) -> Option<String> {
    if profile.folder.is_empty()
        || !profile
            .folder
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return None;
    }
    Some(if profile.persistent {
        root.join(format!("browser-{}", profile.folder))
            .to_string_lossy()
            .into_owned()
    } else {
        String::new()
    })
}

/// The CEF profile's folder under `cache`, or none (CEF keeps the profile in memory) when nothing may be kept here.
fn profile_path(cache: &std::path::Path, local_cache: bool) -> String {
    if local_cache {
        cache.join("default").to_string_lossy().into_owned()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn retiring_the_primer_before_its_native_host_exists_keeps_it_owned_until_close() {
        use std::cell::Cell;
        let ready = Cell::new(false);
        let closes = Cell::new(0);
        let mut pages = HashMap::from([(PRIMER, &ready), (7, &ready)]);
        let mut closing = std::collections::HashSet::from([PRIMER]);
        let close = |ready: &&Cell<bool>| {
            if !ready.get() {
                return false;
            }
            closes.set(closes.get() + 1);
            true
        };
        settle_closes(&mut pages, &mut closing, close);
        assert!(
            pages.contains_key(&PRIMER),
            "an unavailable host must not orphan its browser"
        );
        assert!(closing.contains(&PRIMER));
        assert_eq!(closes.get(), 0);
        ready.set(true);
        settle_closes(&mut pages, &mut closing, close);
        assert!(!pages.contains_key(&PRIMER));
        assert!(pages.contains_key(&7), "the real page stays alive");
        assert!(closing.is_empty());
        settle_closes(&mut pages, &mut closing, close);
        assert_eq!(closes.get(), 1, "close exactly once when its host becomes ready");
    }

    #[test]
    fn a_profile_is_saved_in_its_own_folder_or_kept_in_memory() {
        let root = std::path::Path::new("/cache/cef");
        let saved = WebProfile {
            folder: "0e9a4c1b-1111-4222-8333-944455556666".into(),
            persistent: true,
        };
        let folder = root.join("browser-0e9a4c1b-1111-4222-8333-944455556666");
        assert_eq!(
            profile_folder(root, &saved),
            Some(folder.to_string_lossy().into_owned())
        );
        let memory = WebProfile {
            persistent: false,
            ..saved.clone()
        };
        assert_eq!(profile_folder(root, &memory).as_deref(), Some(""));
        for folder in ["", "../default", "a/b", "default"] {
            let wrong = WebProfile {
                folder: folder.into(),
                persistent: true,
            };
            assert_eq!(profile_folder(root, &wrong), None, "{folder}");
        }
    }

    #[test]
    fn a_clear_page_has_no_engine_background_and_a_tab_stays_white() {
        assert_eq!(
            background_color(true) >> 24,
            0,
            "no alpha: the app's surface shows through"
        );
        assert_eq!(background_color(false), 0xFFFF_FFFF);
    }
}
