//! The system's light or dark, remembered across launches (`lastAppearance` in this build's settings file).
//!
//! On Linux, GPUI learns the desktop's color scheme from the desktop portal, which answers a moment after the app
//! starts. The vendored Linux layer waits for it briefly (`vendor/README.md`). When the portal is slower than that, or
//! missing, windows open in the appearance the app saw last instead of Light, so a dark desktop does not flash light.
//! The portal's answer still wins when it comes. Other platforms know their appearance at once; the value is saved
//! there too, and unused.

use std::sync::atomic::{AtomicU8, Ordering};

use gpui_kit::WindowAppearance;

/// What was saved or read this launch: 0 unknown, 1 light, 2 dark. Saves only a change.
static KNOWN: AtomicU8 = AtomicU8::new(0);

fn code(dark: bool) -> u8 {
    if dark { 2 } else { 1 }
}

/// Before the platform starts: the appearance windows open in until the portal answers.
pub fn start() {
    let Some(dark) = crate::paths::last_appearance_dark() else {
        return;
    };
    KNOWN.store(code(dark), Ordering::Relaxed);
    #[cfg(target_os = "linux")]
    gpui_linux::set_fallback_appearance(if dark {
        WindowAppearance::Dark
    } else {
        WindowAppearance::Light
    });
}

/// The system's appearance now, as a window sees it: kept for the next launch when it changed.
pub fn seen(appearance: WindowAppearance) {
    let dark = matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark);
    if KNOWN.swap(code(dark), Ordering::Relaxed) != code(dark) && !cfg!(test) {
        crate::paths::save_last_appearance_dark(dark);
    }
}
