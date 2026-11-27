//! Text size and reduce motion, as the Nexus app's Settings › Appearance.
//!
//! Text size zooms the whole window, as the Nexus app zooms its webview: from 50% to 200% in steps of 10%, shown as
//! the body text's size (14px at 100%). Ctrl+= or Ctrl++ zooms in, Ctrl+- out and Ctrl+0 resets (Cmd on macOS). Ctrl
//! or Cmd with the wheel does not zoom the app: it reaches the element under the pointer, so a canvas zooms with it.
//! GPUI draws it (`vendor/gpui-pre`, `ui_zoom`): every window lays out smaller and draws at a larger scale. Reduce motion
//! is GPUI's own switch: every animation shows its end, and a repeating one holds still. It is on while the app's
//! switch is on or the system asks for less motion (`os_motion.rs`), as the Nexus app's webview follows
//! `prefers-reduced-motion`. The system's setting is read again every few seconds, so a change there applies at once.
//!
//! Both are kept in this build's settings file (`zoom`, `reduceMotion`), as the Nexus app keeps them per computer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::*;

actions!(nexus, [ZoomIn, ZoomOut, ZoomReset]);

pub const MIN: f32 = 0.5;
pub const MAX: f32 = 2.;
const STEP: f32 = 0.1;
/// The body text at 100%, for the label: "14 px".
const BASE_TEXT: f32 = 14.;
/// How often the system's reduced-motion setting is read again.
const SYSTEM_EVERY: Duration = Duration::from_secs(5);

/// The system asks for less motion, as last read.
static SYSTEM: AtomicBool = AtomicBool::new(false);

/// A zoom on the 10% steps, from 50% to 200% (`clampZoom`).
pub fn clamp(zoom: f32) -> f32 {
    if !zoom.is_finite() {
        return 1.;
    }
    ((zoom.clamp(MIN, MAX) * 100.).round() / 100. * 10.).round() / 10.
}

/// The text size a zoom gives: "14 px" at 100%.
pub fn label(zoom: f32) -> String {
    format!("{} px", (BASE_TEXT * zoom).round() as i64)
}

/// Apply the saved zoom and motion, and bind the zoom keys. Before the first window opens.
pub fn start(cx: &mut App) {
    gpui::set_ui_zoom(clamp(crate::paths::zoom()));
    SYSTEM.store(crate::os_motion::reduces_motion(), Ordering::Relaxed);
    apply_motion(cx);
    follow_system(cx);
    cx.bind_keys(bindings());
    cx.on_action(|_: &ZoomIn, cx| set(gpui::ui_zoom() + STEP, cx));
    cx.on_action(|_: &ZoomOut, cx| set(gpui::ui_zoom() - STEP, cx));
    cx.on_action(|_: &ZoomReset, cx| set(1., cx));
}

/// Zoom every window to `zoom` and keep it.
pub fn set(zoom: f32, cx: &mut App) {
    let zoom = clamp(zoom);
    if zoom == gpui::ui_zoom() {
        return;
    }
    crate::paths::save_zoom(zoom);
    gpui::set_ui_zoom(zoom);
    // After the current event: the window it came from is busy until then.
    cx.defer(|cx| {
        for window in cx.windows() {
            let _ = window.update(cx, |_, window, cx| window.bounds_changed(cx));
        }
    });
}

/// Turn the app's reduce motion switch on or off, and keep it. Motion stays reduced while the system asks for it. A
/// failed save changes nothing: the switch and the motion keep the saved value.
pub fn set_reduce_motion(on: bool, cx: &mut App) -> Result<(), String> {
    crate::paths::save_reduce_motion(on)?;
    apply_motion(cx);
    Ok(())
}

/// Whether the system asks for less motion, as last read.
pub fn system_reduces_motion() -> bool {
    SYSTEM.load(Ordering::Relaxed)
}

/// The app's switch, or the system's setting.
fn apply_motion(cx: &mut App) {
    cx.set_reduce_motion(crate::paths::reduce_motion() || system_reduces_motion());
}

/// Read the system's setting again every few seconds, off the UI thread; a change applies to every window.
fn follow_system(cx: &mut App) {
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        loop {
            executor.timer(SYSTEM_EVERY).await;
            let now = executor.spawn(async { crate::os_motion::reduces_motion() }).await;
            if SYSTEM.swap(now, Ordering::Relaxed) != now {
                cx.update(apply_motion);
            }
        }
    })
    .detach();
}

fn bindings() -> Vec<KeyBinding> {
    let (zoom_in, zoom_in_plus, zoom_in_shift, zoom_out, reset) = if cfg!(target_os = "macos") {
        ("cmd-=", "cmd-+", "cmd-shift-=", "cmd--", "cmd-0")
    } else {
        ("ctrl-=", "ctrl-+", "ctrl-shift-=", "ctrl--", "ctrl-0")
    };
    vec![
        KeyBinding::new(zoom_in, ZoomIn, None),
        KeyBinding::new(zoom_in_plus, ZoomIn, None),
        KeyBinding::new(zoom_in_shift, ZoomIn, None),
        KeyBinding::new(zoom_out, ZoomOut, None),
        KeyBinding::new(reset, ZoomReset, None),
    ]
}

#[cfg(test)]
mod tests {
    use super::{clamp, label};

    #[test]
    fn steps_by_ten_percent_within_bounds() {
        assert_eq!(clamp(1.1000001), 1.1);
        assert_eq!(clamp(0.3), 0.5);
        assert_eq!(clamp(9.), 2.);
        assert_eq!(clamp(f32::NAN), 1.);
        assert_eq!(label(1.), "14 px");
        assert_eq!(label(1.5), "21 px");
    }
}
