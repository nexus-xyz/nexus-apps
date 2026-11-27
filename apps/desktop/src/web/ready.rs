//! A page opened ahead: blank, hidden, with a startup script in place (`client::prepare`). The next page with that
//! script takes it and only navigates, so an artifact's open skips the page's creation and its startup's round trips.
//! The page it took stays hidden until its own document loaded: no frame of the blank page it waited on shows.

use std::sync::Mutex;
use std::time::Duration;

use gpui_kit::{App, Window};

use super::{Startup, client, frame_rate, servo, startup_messages};

/// A taken page shows at most this long after it navigated, loaded or not: one that keeps a request open still draws.
pub(super) const HOLD_AT_MOST: Duration = Duration::from_millis(300);
/// Preparation follows recent intent. A person who leaves the card or list does not retain a renderer indefinitely.
pub(super) const KEEP_FOR: Duration = Duration::from_secs(30);

/// The size, scale and frame rate the last page was laid out at: a page opened ahead opens at them, so it shows with
/// no resize.
#[derive(Clone, Copy)]
pub(super) struct Placed {
    pub(super) size: (i32, i32),
    pub(super) scale: f32,
    pub(super) frame_rate: i32,
}

static PLACED: Mutex<Option<Placed>> = Mutex::new(None);

/// A page with a startup script was laid out at `placed`.
pub(super) fn placed(placed: Placed) {
    *PLACED.lock().expect("placed page") = Some(placed);
}

/// Prepare after an explicit request, such as entering the artifact list. Repeated intent refreshes its lease.
pub fn prepare(startup: Startup, window: &Window, cx: &App) {
    if servo::chosen() || cfg!(test) {
        return;
    }
    let placed = placement(window, cx);
    cx.background_executor()
        .spawn(async move { client::prepare(startup, startup_messages(startup), placed) })
        .detach();
}

/// Prepare when a pointer rests on an artifact, and prefetch its address in the same dwell.
pub fn prepare_on_hover_then(
    startup: Startup,
    hovered: bool,
    window: &Window,
    cx: &mut App,
    then: impl FnOnce() + 'static,
) {
    let placed = placement(window, cx);
    let executor = cx.background_executor().clone();
    super::dwell::after_dwell(hovered, cx, move || {
        if !servo::chosen() && !cfg!(test) {
            executor
                .spawn(async move { client::prepare(startup, startup_messages(startup), placed) })
                .detach();
        }
        then();
    });
}

fn placement(window: &Window, cx: &App) -> Placed {
    PLACED.lock().expect("placed page").unwrap_or(Placed {
        size: (800, 600),
        scale: window.scale_factor(),
        frame_rate: frame_rate(window.refresh_interval(cx)),
    })
}
