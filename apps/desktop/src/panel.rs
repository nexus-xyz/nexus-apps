//! The panel: Ask Nexus, the small chat window under the tray item or menu-bar item. A click on the item and the
//! global shortcut open it or close it; the tray's Ask Nexus opens it. The tray's other rows go to an app window
//! (`workspace/shell.rs`). Esc, Close or a click in another app closes it. "Open in the app window" (Ctrl+Enter,
//! Cmd+Enter on macOS) moves its chat to the main window and closes it.
//!
//! The window is 440 by 600 points, or the larger size the person left it at, kept in `settings.json` (`panel_size`).
//! It has no title bar. It resizes from its edges, never under its first size; on Linux its
//! bottom-left grip resizes it too, its top-right corner still. As it closes, a size the person changed is kept while
//! this build owns the store ([`hide`]). Its geometry is from `nexus_desktop_core::panel`. Where it
//! opens:
//!
//! - macOS and Windows: centred under the menu-bar item, or above the tray icon when the taskbar is at the bottom,
//!   8 points from the bar and the screen's edges ([`anchor`]). It floats over other windows and has no taskbar entry
//!   (`WindowKind::PopUp`).
//! - Linux: the tray reports no position. On X11 the panel is centred. On Wayland a client cannot place its windows:
//!   the compositor puts it where it puts any new window. On X11 it floats over other windows, on every desktop, with
//!   no taskbar or pager entry ([`x11::float`]). Wayland has no protocol for it.
//!
//! GPUI cannot hide a window, so closing the panel closes its window, and opening it makes a new one. What it showed
//! survives in [`PanelState`]: opened again within ten minutes, it opens the same chat; later, a new chat
//! ([`FRESH_AFTER`]). The window takes a side of the shared Gateway and the shared Code runner
//! (`windows.rs`), but is not in the routing order: the tray, notifications and links go to an app window.

use std::time::{Duration, Instant};

use gpui_kit::*;
use nexus_desktop_core::panel::{self as geometry, GAP, HEIGHT, WIDTH};

use crate::model::SessionSummary;
use crate::tray::Command;

/// Away this long, the panel opens on a new chat.
pub const FRESH_AFTER: Duration = Duration::from_secs(10 * 60);
/// A click on the tray item takes the focus from the panel before it arrives: a toggle this soon after losing the focus
/// closes, and does not open it again.
pub const BLUR_GRACE: Duration = Duration::from_millis(300);

/// A rectangle: x, y, width, height.
pub type Rect = geometry::Frame;

/// What a toggle does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Open,
    Close,
}

/// The panel's state across its windows: whether one is open, its focus, and the chat it left.
#[derive(Debug, Clone, Default)]
pub struct PanelState {
    open: bool,
    /// Whether the open panel has had the focus: only then does losing it close the panel. A window the compositor did
    /// not let take the focus would otherwise close as it opens.
    had_focus: bool,
    /// When the panel last closed for losing the focus.
    blurred_at: Option<Instant>,
    /// The chat the panel showed when it closed, and when.
    left: Option<(SessionSummary, Instant)>,
    /// The size the open panel was given, in points: only a size the person changed from it is kept.
    placed: Option<(f64, f64)>,
    /// The size the person left the panel at, for its next window: `settings.json` keeps it only while this build owns
    /// the store.
    kept: Option<[f64; 2]>,
    /// The close motion under way, by number: its window closes when it ends, unless the panel opened again since.
    closing: Option<u64>,
    /// The last close motion's number.
    closes: u64,
}

impl PanelState {
    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// What the tray item's click or the global shortcut does `now`.
    pub fn toggle(&self, now: Instant) -> Toggle {
        let just_blurred = self
            .blurred_at
            .is_some_and(|at| now.saturating_duration_since(at) < BLUR_GRACE);
        // A panel fading out counts as closed: a toggle opens it again.
        if (self.open && self.closing.is_none()) || just_blurred {
            Toggle::Close
        } else {
            Toggle::Open
        }
    }

    /// A panel window opened at `placed` (width, height in points).
    pub fn opened(&mut self, placed: (f64, f64)) {
        self.open = true;
        self.had_focus = false;
        self.placed = Some(placed);
        self.closing = None;
    }

    /// Whether the panel is fading out.
    pub fn is_closing(&self) -> bool {
        self.closing.is_some()
    }

    /// The close motion starts: its number, for [`PanelState::finish_closing`].
    pub fn start_closing(&mut self) -> u64 {
        self.closes += 1;
        self.closing = Some(self.closes);
        self.closes
    }

    /// The panel opens again while it fades out: answers whether it was fading out.
    pub fn cancel_closing(&mut self) -> bool {
        self.closing.take().is_some()
    }

    /// Close motion `number` ended: answers whether its window closes now (the panel did not open again since).
    pub fn finish_closing(&mut self, number: u64) -> bool {
        if self.closing != Some(number) {
            return false;
        }
        self.closing = None;
        true
    }

    /// The panel closes at `now` (width, height in points), with `saved` in `settings.json`: the size to write there,
    /// when the person changed it. The next window opens at it either way ([`PanelState::size`]).
    pub fn keep(&mut self, saved: Option<[f64; 2]>, now: (f64, f64)) -> Option<[f64; 2]> {
        let kept = geometry::to_keep(self.kept.or(saved), self.placed, now)?;
        self.kept = Some(kept);
        Some(kept)
    }

    /// The size the next window opens at, with `saved` in `settings.json`: the one the person left it at.
    pub fn size(&self, saved: Option<[f64; 2]>) -> (f64, f64) {
        geometry::size(self.kept.or(saved))
    }

    /// The panel's window gained or lost the focus `now`. Answers whether the panel closes.
    pub fn focus(&mut self, active: bool, now: Instant) -> bool {
        if active {
            self.had_focus = true;
            return false;
        }
        if !self.open || !std::mem::take(&mut self.had_focus) {
            return false;
        }
        self.blurred_at = Some(now);
        true
    }

    /// The panel's window closed `now`, on `chat` (`None`: a new chat).
    pub fn closed(&mut self, chat: Option<SessionSummary>, now: Instant) {
        self.open = false;
        self.had_focus = false;
        self.closing = None;
        self.left = chat.map(|chat| (chat, now));
    }

    /// The chat a panel opened `now` continues: the one it left less than [`FRESH_AFTER`] ago.
    pub fn resume(&self, now: Instant) -> Option<SessionSummary> {
        self.left
            .as_ref()
            .filter(|(_, at)| now.saturating_duration_since(*at) < FRESH_AFTER)
            .map(|(chat, _)| chat.clone())
    }
}

/// What "Open in the app window" asks of the app window: the panel's chat, or the window as it is for a new chat.
pub fn app_command(chat: Option<SessionSummary>) -> Command {
    match chat {
        Some(chat) => Command::Session {
            id: chat.id,
            title: chat.title,
        },
        None => Command::Open,
    }
}

/// The scales a display may have: Windows offers 100% to 400% in steps of 25%, macOS 1 and 2.
const SCALES: [f64; 13] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0, 3.25, 3.5, 3.75, 4.0];

fn contains((x, y, w, h): Rect, (px, py): (f64, f64)) -> bool {
    px >= x && py >= y && px < x + w && py < y + h
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

fn inside(inner: Rect, outer: Rect) -> bool {
    inner.0 >= outer.0
        && inner.1 >= outer.1
        && inner.0 + inner.2 <= outer.0 + outer.2
        && inner.1 + inner.3 <= outer.1 + outer.3
}

/// The display under the item and the item in that display's points. `icon` is in physical pixels, as `tray-icon`
/// reports it; each display is its bounds and its work area in points, as GPUI reports them. GPUI does not say a
/// display's scale, so each one is tried: at the right one, the item lies in a bar, on the display and clear of its
/// work area. Failing that, its centre is on a display.
pub fn locate(icon: Rect, displays: &[(Rect, Rect)]) -> Option<(usize, Rect)> {
    let (ix, iy, iw, ih) = icon;
    let scaled = |scale: f64| (ix / scale, iy / scale, iw / scale, ih / scale);
    let in_bar = SCALES.iter().find_map(|scale| {
        let at = scaled(*scale);
        displays
            .iter()
            .position(|(bounds, area)| inside(at, *bounds) && !overlaps(at, *area))
            .map(|display| (display, at))
    });
    in_bar.or_else(|| {
        SCALES.iter().find_map(|scale| {
            let at = scaled(*scale);
            let center = (at.0 + at.2 / 2.0, at.1 + at.3 / 2.0);
            displays
                .iter()
                .position(|(bounds, _)| contains(*bounds, center))
                .map(|display| (display, at))
        })
    })
}

fn rect_of(bounds: Bounds<Pixels>) -> Rect {
    (
        f64::from(f32::from(bounds.origin.x)),
        f64::from(f32::from(bounds.origin.y)),
        f64::from(f32::from(bounds.size.width)),
        f64::from(f32::from(bounds.size.height)),
    )
}

/// GPUI Kit's shadow around a window it frames itself (`SHADOW_SIZE` in gpui-component's `window_border.rs`).
const FRAME: f64 = 20.0;

/// Whether GPUI runs on Wayland here: it does when the session has a Wayland display.
fn wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty())
}

/// The client frame's size on each side, in points: GPUI Kit's shadow on Wayland, none elsewhere.
fn frame() -> f64 {
    if wayland() { FRAME } else { 0.0 }
}

/// The panel's window: where it goes and what it is on this platform, and the panel's size in it (width, height).
fn options(start: (f64, f64), cx: &App) -> (WindowOptions, (f64, f64)) {
    let (mut width, mut height) = start;
    let mut display_id = None;
    let mut bounds = None;
    if let Some(icon) = crate::tray::icon_rect(cx) {
        let displays = cx.displays();
        let rects: Vec<(Rect, Rect)> = displays
            .iter()
            .map(|display| (rect_of(display.bounds()), rect_of(display.visible_bounds())))
            .collect();
        if let Some((index, icon)) = locate(icon, &rects) {
            let (_, area) = rects[index];
            (width, height) = geometry::fit((width, height), (area.2, area.3), GAP);
            let (x, y) = geometry::anchor(icon, (width, height), area, GAP);
            display_id = Some(displays[index].id());
            bounds = Some(Bounds::new(
                point(px(x as f32), px(y as f32)),
                size(px(width as f32), px(height as f32)),
            ));
        }
    }
    let bounds = bounds.unwrap_or_else(|| {
        // Linux has no item position. On Wayland GPUI Kit's client frame (a shadow `FRAME` wide on each side) comes out
        // of the window's size: ask for that much more, so the panel itself is its full size.
        let frame = 2.0 * frame();
        Bounds::centered(None, size(px((width + frame) as f32), px((height + frame) as f32)), cx)
    });
    let frame = 2.0 * frame();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // No title bar: the panel's head moves it. macOS keeps a titled window's rounded corners and shadow.
        titlebar: None,
        focus: true,
        show: true,
        // macOS and Windows: over other windows, on every Space, with no taskbar entry. On Linux a pop-up is an X11
        // override-redirect window, which never takes the keyboard: a plain window.
        kind: if cfg!(target_os = "linux") {
            WindowKind::Normal
        } else {
            WindowKind::PopUp
        },
        // Resized from its edges (GPUI Kit's frame on Linux, the system's elsewhere), never under its first size.
        is_resizable: true,
        window_min_size: Some(size(px((WIDTH + frame) as f32), px((HEIGHT + frame) as f32))),
        is_minimizable: false,
        display_id,
        app_id: Some(crate::app_id::get()),
        // No server-side title bar or border on Linux.
        window_decorations: Some(WindowDecorations::Client),
        ..WindowOptions::default()
    };
    (options, (width, height))
}

/// Keep the activity window above other apps on X11. Wayland controls its own placement.
#[cfg(target_os = "linux")]
#[allow(dead_code)] // Unused while the activity pill is off.
pub(crate) fn float_activity(window: &Window) {
    x11::float(window);
}

/// The tray item's click or the global shortcut: open the panel, or close it.
pub fn toggle(cx: &mut App) {
    if !crate::windows::running(cx) {
        return;
    }
    match crate::windows::panel_state(cx).toggle(Instant::now()) {
        Toggle::Open => show(cx),
        Toggle::Close => hide(cx),
    }
}

/// The tray's Ask Nexus: open the panel, or bring it forward.
pub fn show(cx: &mut App) {
    if !crate::windows::running(cx) {
        return;
    }
    let reduce = cx.reduce_motion();
    if let Some((window, _)) = crate::windows::panel(cx) {
        // Fading out: it stays, and fades in again.
        let reopen = crate::windows::panel_state(cx).cancel_closing();
        let _ = window.update(cx, |_, window, _| {
            if reopen {
                crate::panel_motion::reopen(window, reduce);
            }
            window.activate_window();
        });
        cx.activate(true);
        return;
    }
    let resume = crate::windows::panel_state(cx).resume(Instant::now());
    let start = crate::windows::panel_state(cx).size(crate::desktop_settings::load().panel_size);
    let (options, placed) = options(start, cx);
    let Some((window, workspace)) = crate::windows::open_panel(options, cx) else {
        return;
    };
    crate::windows::panel_state(cx).opened(placed);
    let _ = window.update(cx, |_, window, cx| {
        crate::panel_motion::open(window, reduce);
        #[cfg(target_os = "linux")]
        x11::float(window);
        workspace.update(cx, |workspace, cx| workspace.panel_opened(resume, window, cx))
    });
    cx.activate(true);
}

/// Close the panel, if it is open, and keep its size when the person changed it. On macOS it fades out first
/// (`panel_motion.rs`); a second close while it fades does nothing.
pub fn hide(cx: &mut App) {
    let Some((window, _)) = crate::windows::panel(cx) else {
        return;
    };
    if crate::windows::panel_state(cx).is_closing() {
        return;
    }
    let closing = window
        .update(cx, |_, window, _| {
            // Without the client frame's shadow: the panel's own size.
            let size = window.inner_window_bounds().get_bounds().size;
            let wait = crate::panel_motion::close(window);
            if wait.is_zero() {
                window.remove_window();
            }
            (
                (f64::from(f32::from(size.width)), f64::from(f32::from(size.height))),
                wait,
            )
        })
        .ok();
    let Some((now, wait)) = closing else {
        return;
    };
    keep_size(now, cx);
    if wait.is_zero() {
        return;
    }
    let number = crate::windows::panel_state(cx).start_closing();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(wait).await;
        let _ = cx.update(|cx| {
            if !crate::windows::panel_state(cx).finish_closing(number) {
                return;
            }
            if let Some((window, _)) = crate::windows::panel(cx) {
                let _ = window.update(cx, |_, window, _| window.remove_window());
            }
        });
    })
    .detach();
}

/// Keep the panel's size `now` when the person changed it: for the next window,
/// and in `settings.json` off the UI thread. Only the store's owner writes the file (`desktop_settings::update`);
/// otherwise the size lasts until the app quits.
fn keep_size(now: (f64, f64), cx: &mut App) {
    let saved = crate::desktop_settings::load().panel_size;
    let Some(size) = crate::windows::panel_state(cx).keep(saved, now) else {
        return;
    };
    if !crate::desktop_settings::writable() {
        return;
    }
    cx.background_executor()
        .spawn(async move {
            let _ = crate::desktop_settings::update(|settings| {
                settings.panel_size = Some(size);
                Ok(())
            });
        })
        .detach();
}

/// A press on the grip at the panel's bottom-left corner: the compositor resizes the panel from that corner, its
/// top-right corner still. Linux only: GPUI has no interactive resize on macOS or Windows, where the system resizes the
/// panel from its edges.
pub fn grip(window: &Window) {
    window.start_window_resize(ResizeEdge::BottomLeft);
}

/// Whether the panel shows its grip: where [`grip`] resizes it.
pub const GRIP: bool = cfg!(target_os = "linux");

/// Close the panel once the window being updated now is done (Esc, Close).
pub fn hide_later(cx: &mut App) {
    cx.defer(hide);
}

/// The panel's window gained or lost the focus: on losing it, after having had it, the panel closes.
pub fn focus_changed(active: bool, cx: &mut App) {
    if crate::windows::running(cx) && crate::windows::panel_state(cx).focus(active, Instant::now()) {
        // The window is being updated now: close it once that is done.
        cx.defer(hide);
    }
}

/// "Open in the app window": the panel's `chat` continues in the app window focused last, and the panel closes.
pub fn open_in_app(chat: Option<SessionSummary>, cx: &mut App) {
    cx.defer(move |cx| {
        hide(cx);
        crate::windows::run(app_command(chat), cx);
    });
}

/// The panel's window states on X11: above other windows, on every desktop, and with no taskbar or pager entry.
/// GPUI's X11 windows have none of these, so the panel asks the window manager itself, as EWMH says a client changes
/// a mapped window's state: client messages to the root window.
/// The window manager drops a message for a window it does not manage yet, and GPUI's window is new (in a nested KWin,
/// about one opening in four lost them). So a thread of its own sends them until the window manager's `_NET_WM_STATE`
/// on the window says it is above, for up to [`MANAGED_WITHIN`].
#[cfg(target_os = "linux")]
mod x11 {
    use std::time::{Duration, Instant};

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask};

    /// How long the window manager has to take the panel's window and its state, and how often the panel asks.
    const MANAGED_WITHIN: Duration = Duration::from_secs(2);
    const RETRY: Duration = Duration::from_millis(50);

    /// `_NET_WM_STATE`'s action that adds the states it names.
    const ADD: u32 = 1;
    /// Who asks: a normal application (EWMH's source indication).
    const APPLICATION: u32 = 1;
    /// `_NET_WM_DESKTOP`'s "every desktop".
    const ALL_DESKTOPS: u32 = 0xFFFF_FFFF;

    /// The states the panel adds, two to a `_NET_WM_STATE` message.
    pub(super) const STATES: [[&str; 2]; 2] = [
        ["_NET_WM_STATE_ABOVE", "_NET_WM_STATE_STICKY"],
        ["_NET_WM_STATE_SKIP_TASKBAR", "_NET_WM_STATE_SKIP_PAGER"],
    ];

    /// Float the panel's `window` over other windows on X11. Nothing on Wayland, where the window has no X11 id.
    pub fn float(window: &gpui_kit::Window) {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::Xcb(handle) = handle.as_raw() else {
            return;
        };
        let window = handle.window.get();
        std::thread::spawn(move || {
            if let Err(error) = send(window) {
                log::warn!("the panel could not stay on top: {error}");
            }
        });
    }

    fn send(window: u32) -> Result<(), Box<dyn std::error::Error>> {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        let atom = |name: &str| -> Result<u32, Box<dyn std::error::Error>> {
            Ok(connection.intern_atom(false, name.as_bytes())?.reply()?.atom)
        };
        let state = atom("_NET_WM_STATE")?;
        let above = atom("_NET_WM_STATE_ABOVE")?;
        let mut messages = Vec::new();
        for [first, second] in STATES {
            messages.push((state, [ADD, atom(first)?, atom(second)?, APPLICATION, 0]));
        }
        messages.push((atom("_NET_WM_DESKTOP")?, [ALL_DESKTOPS, APPLICATION, 0, 0, 0]));
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        let start = Instant::now();
        // Until the window manager has taken the window, it drops the messages: send them again until the state the
        // window manager writes back says the panel is above.
        loop {
            for (kind, data) in &messages {
                connection.send_event(false, root, mask, ClientMessageEvent::new(32, window, *kind, *data))?;
            }
            connection.flush()?;
            std::thread::sleep(RETRY);
            let now = connection
                .get_property(false, window, state, AtomEnum::ATOM, 0, 32)?
                .reply()?;
            if now.value32().is_some_and(|mut states| states.any(|atom| atom == above)) {
                return Ok(());
            }
            if start.elapsed() > MANAGED_WITHIN {
                return Err("the window manager did not take the state".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // `gpui_kit::*` brings GPUI's own `test` attribute; these are plain tests.
    use core::prelude::v1::test;

    use super::*;

    fn chat(id: &str) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            title: format!("Chat {id}"),
            day: None,
            at_ms: None,
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn on_x11_the_panel_asks_to_float_on_every_desktop_without_entries() {
        let states: Vec<&str> = x11::STATES.iter().flatten().copied().collect();
        assert_eq!(
            states,
            [
                "_NET_WM_STATE_ABOVE",
                "_NET_WM_STATE_STICKY",
                "_NET_WM_STATE_SKIP_TASKBAR",
                "_NET_WM_STATE_SKIP_PAGER"
            ]
        );
    }

    #[test]
    fn a_panel_fading_out_counts_as_closed_and_can_open_again() {
        let now = Instant::now();
        let mut state = PanelState::default();
        state.opened((WIDTH, HEIGHT));
        let first = state.start_closing();
        assert!(state.is_closing());
        // A toggle while it fades opens it again.
        assert_eq!(state.toggle(now), Toggle::Open);
        assert!(state.cancel_closing());
        // The first fade ends after it opened again: its window stays.
        assert!(!state.finish_closing(first));
        assert_eq!(state.toggle(now), Toggle::Close);
        // A fade that runs to its end closes the window, once.
        let second = state.start_closing();
        assert_ne!(first, second);
        assert!(state.finish_closing(second));
        assert!(!state.finish_closing(second));
        assert!(!state.cancel_closing());
    }

    #[test]
    fn a_toggle_opens_a_closed_panel_and_closes_an_open_one() {
        let now = Instant::now();
        let mut state = PanelState::default();
        assert!(!state.is_open());
        assert_eq!(state.toggle(now), Toggle::Open);
        state.opened((WIDTH, HEIGHT));
        assert!(state.is_open());
        assert_eq!(state.toggle(now), Toggle::Close);
        state.closed(None, now);
        assert!(!state.is_open());
        assert_eq!(state.toggle(now), Toggle::Open);
    }

    #[test]
    fn losing_the_focus_closes_the_panel_only_after_it_had_it() {
        let now = Instant::now();
        let mut state = PanelState::default();
        state.opened((WIDTH, HEIGHT));
        // The compositor did not give it the focus: it stays.
        assert!(!state.focus(false, now));
        assert!(!state.focus(true, now));
        assert!(state.focus(false, now));
        // The same loss again, once it is closing, does nothing more.
        assert!(!state.focus(false, now));
        // A closed panel does not close again.
        state.closed(None, now);
        assert!(!state.focus(false, now));
    }

    #[test]
    fn a_click_on_the_item_that_took_the_focus_closes_and_does_not_reopen() {
        let now = Instant::now();
        let mut state = PanelState::default();
        state.opened((WIDTH, HEIGHT));
        state.focus(true, now);
        // A click on the tray item: the panel loses the focus first, and closes.
        assert!(state.focus(false, now));
        state.closed(None, now);
        // The click's toggle arrives just after: it stays closed.
        assert_eq!(state.toggle(now + Duration::from_millis(100)), Toggle::Close);
        // A later toggle opens it.
        assert_eq!(state.toggle(now + Duration::from_millis(400)), Toggle::Open);
    }

    #[test]
    fn the_panel_continues_its_chat_for_ten_minutes_then_starts_a_new_one() {
        let now = Instant::now();
        let mut state = PanelState::default();
        state.opened((WIDTH, HEIGHT));
        state.closed(Some(chat("a")), now);
        assert_eq!(state.resume(now + Duration::from_secs(60)), Some(chat("a")));
        assert_eq!(state.resume(now + FRESH_AFTER), None);
        // Closed on a new chat: a new chat again.
        state.opened((WIDTH, HEIGHT));
        state.closed(None, now);
        assert_eq!(state.resume(now), None);
    }

    #[test]
    fn open_in_the_app_window_takes_the_chat_or_the_window() {
        assert_eq!(
            app_command(Some(chat("a"))),
            Command::Session {
                id: "a".into(),
                title: "Chat a".into()
            }
        );
        assert_eq!(app_command(None), Command::Open);
    }

    #[test]
    fn a_size_the_person_changed_is_kept_for_the_next_window() {
        let mut state = PanelState::default();
        assert_eq!(state.size(None), (WIDTH, HEIGHT));
        assert_eq!(state.size(Some([520.0, 680.0])), (520.0, 680.0));
        // Closed at the size it opened at: nothing to keep.
        state.opened((WIDTH, HEIGHT));
        assert_eq!(state.keep(None, (WIDTH, HEIGHT)), None);
        // Made larger: kept, and the next window opens at it even when the file was not written.
        state.opened((WIDTH, HEIGHT));
        assert_eq!(state.keep(None, (520.4, 700.0)), Some([520.0, 700.0]));
        assert_eq!(state.size(None), (520.0, 700.0));
        // Opened at it and closed at it again: not written again.
        state.opened((520.0, 700.0));
        assert_eq!(state.keep(None, (520.0, 700.0)), None);
    }

    #[test]
    fn the_item_is_found_on_its_display_at_its_scale() {
        let laptop = ((0.0, 0.0, 1512.0, 982.0), (0.0, 37.0, 1512.0, 945.0));
        // A Retina menu bar: 2 pixels a point.
        assert_eq!(
            locate((2400.0, 0.0, 60.0, 74.0), &[laptop]),
            Some((0, (1200.0, 0.0, 30.0, 37.0)))
        );
        // Windows at 150%, beside a display at 100% on its left.
        let left = ((-1920.0, 0.0, 1920.0, 1080.0), (-1920.0, 0.0, 1920.0, 1032.0));
        let right = ((0.0, 0.0, 1707.0, 960.0), (0.0, 0.0, 1707.0, 912.0));
        assert_eq!(
            locate((2400.0, 1380.0, 36.0, 48.0), &[left, right]),
            Some((1, (1600.0, 920.0, 24.0, 32.0)))
        );
        // Nowhere on any display.
        assert_eq!(locate((-90000.0, 0.0, 10.0, 10.0), &[laptop]), None);
    }
}
