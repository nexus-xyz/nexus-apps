//! The panel window's own motion on macOS: the window fades in and drops a few points into place as it opens, and
//! fades out before it closes, on `nexus_desktop_core::panel::motion`'s durations and curve. With Reduce motion on
//! (the system's or the app's), only the fade.
//! AppKit animates the window's alpha and frame (`NSAnimationContext`), so the panel's content draws once, at rest.
//!
//! On Windows and Linux the window does not move: the panel's content fades in and drops (`workspace/panel_view.rs`),
//! and it closes at once ([`WINDOW`]).

use std::time::Duration;

use gpui_kit::Window;

/// Whether the window itself moves (macOS). Elsewhere the content does, and only as it opens.
pub const WINDOW: bool = cfg!(target_os = "macos");

/// Right after the panel's window opened: start the open motion. `reduce`: Reduce motion is on.
pub fn open(window: &Window, reduce: bool) {
    #[cfg(target_os = "macos")]
    mac::open(window, reduce);
    #[cfg(not(target_os = "macos"))]
    let _ = (window, reduce);
}

/// Start the close motion; returns how long to wait before closing the window (zero: close it now).
pub fn close(window: &Window) -> Duration {
    #[cfg(target_os = "macos")]
    {
        mac::close(window);
        nexus_desktop_core::panel::motion::CLOSE
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Duration::ZERO
    }
}

/// A panel that was closing opens again: back to whole, with the open fade.
pub fn reopen(window: &Window, reduce: bool) {
    #[cfg(target_os = "macos")]
    mac::reopen(window, reduce);
    #[cfg(not(target_os = "macos"))]
    let _ = (window, reduce);
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use std::time::Duration;

    use gpui_kit::Window;
    use nexus_desktop_core::panel::motion::{self, EASE_OUT_SOFT};
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSAnimatablePropertyContainer as _, NSAnimationContext, NSView, NSWindow};
    use objc2_quartz_core::CAMediaTimingFunction;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// The panel's NSWindow, on the main thread only: GPUI hands out its content view.
    fn ns_window(window: &impl HasWindowHandle) -> Option<Retained<NSWindow>> {
        MainThreadMarker::new()?;
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        // SAFETY: GPUI's handle points at the window's live content view, an NSView; it is only used here, on the main
        // thread (checked above), while GPUI keeps the window open.
        let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        view.window()
    }

    fn animate(duration: Duration, changes: impl FnOnce()) {
        NSAnimationContext::beginGrouping();
        let context = NSAnimationContext::currentContext();
        context.setDuration(duration.as_secs_f64());
        let (x1, y1, x2, y2) = EASE_OUT_SOFT;
        context.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(x1, y1, x2, y2)));
        changes();
        NSAnimationContext::endGrouping();
    }

    /// Transparent and `drop` points above where it settles, then faded in and settled.
    pub fn open(window: &Window, reduce: bool) {
        let Some(ns) = ns_window(window) else {
            return;
        };
        let (duration, drop) = motion::open(reduce);
        let frame = ns.frame();
        let mut start = frame;
        // AppKit's y grows upwards: above is +y.
        start.origin.y += drop;
        ns.setAlphaValue(0.0);
        if drop > 0.0 {
            ns.setFrame_display(start, false);
        }
        animate(duration, || {
            let animator = ns.animator();
            animator.setAlphaValue(1.0);
            if drop > 0.0 {
                animator.setFrame_display(frame, true);
            }
        });
    }

    pub fn close(window: &Window) {
        let Some(ns) = ns_window(window) else {
            return;
        };
        animate(motion::CLOSE, || ns.animator().setAlphaValue(0.0));
    }

    /// From wherever the close fade got to, back to whole: the open fade, with no drop (the window has not moved).
    pub fn reopen(window: &Window, reduce: bool) {
        let Some(ns) = ns_window(window) else {
            return;
        };
        let (duration, _) = motion::open(reduce);
        animate(duration, || ns.animator().setAlphaValue(1.0));
    }
}
