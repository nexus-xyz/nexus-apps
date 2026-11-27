//! The panel's geometry, with no window toolkit: Ask Nexus, the small chat window under the menu-bar item or tray icon.
//! Nexus GPUI (`panel.rs`) places and sizes it with these.
//!
//! The panel opens at the size the person left it at (`settings.json`'s `panel_size`), never under its first size. It
//! is fitted to the screen, centred under the item, and resized from its edges or its bottom-left grip, which keeps its
//! top-right corner still. Units are whatever the caller passes: points or physical pixels, the same for every argument.

/// The panel's first and smallest size, in points: 440 wide, a little taller for the composer.
pub const WIDTH: f64 = 440.0;
pub const HEIGHT: f64 = 600.0;
/// Between the menu bar (or the taskbar) and the panel, and from the screen's edges, in points.
pub const GAP: f64 = 8.0;

/// A rectangle: x, y, width, height.
pub type Frame = (f64, f64, f64, f64);

/// The size the panel opens at: the one the person left it at (`saved`), never under the first size.
pub fn size(saved: Option<[f64; 2]>) -> (f64, f64) {
    match saved {
        Some([width, height]) if width.is_finite() && height.is_finite() => (width.max(WIDTH), height.max(HEIGHT)),
        _ => (WIDTH, HEIGHT),
    }
}

/// The panel's size on a work area of `area` (width, height): `gap` from its sides, below the menu bar and above the
/// bottom, so the composer and the grip stay on the screen (a size kept from a larger display, say).
pub fn fit(size: (f64, f64), area: (f64, f64), gap: f64) -> (f64, f64) {
    (
        size.0.min((area.0 - 2.0 * gap).max(0.0)),
        size.1.min((area.1 - 2.0 * gap).max(0.0)),
    )
}

/// Where the panel goes for a menu-bar item or tray icon at `icon`: centred under it, `gap` below the menu bar (the
/// lower of the item's bottom edge and the work area's top, so a notched MacBook's taller menu bar and a hidden menu
/// bar both work) and `gap` from the work area's sides (Stage Manager and the Dock shrink it); above the icon when it
/// is in the lower half (the Windows taskbar), `gap` above the taskbar.
pub fn anchor(icon: Frame, panel: (f64, f64), area: Frame, gap: f64) -> (f64, f64) {
    let (ix, iy, iw, ih) = icon;
    let (pw, ph) = panel;
    let (ax, ay, aw, ah) = area;
    let x = (ix + iw / 2.0 - pw / 2.0).clamp(ax + gap, (ax + aw - pw - gap).max(ax + gap));
    let below = iy + ih / 2.0 < ay + ah / 2.0;
    let y = if below {
        (iy + ih).max(ay) + gap
    } else {
        iy.min(ay + ah) - ph - gap
    };
    (x, y.clamp(ay, (ay + ah - ph).max(ay)))
}

/// The panel's frame once its bottom-left grip has moved by `delta` from `start`: the top-right corner stays where it
/// was, the panel grows to at most `max` (the screen's edge) and is never smaller than `min`.
pub fn grip_frame(start: Frame, delta: (f64, f64), min: (f64, f64), max: (f64, f64)) -> Frame {
    let (x, y, width, height) = start;
    let new_width = (width - delta.0).min(max.0).max(min.0);
    let new_height = (height + delta.1).min(max.1).max(min.1);
    (x + width - new_width, y, new_width, new_height)
}

/// Whether the panel's size `now` differs from the one it opened at (`placed`), by more than rounding.
pub fn resized(placed: Option<(f64, f64)>, now: (f64, f64)) -> bool {
    placed.is_none_or(|(width, height)| (width - now.0).abs() >= 1.0 || (height - now.1).abs() >= 1.0)
}

/// What to keep in `panel_size` as the panel closes at `now`, after opening at `placed`, with `saved` kept so far: the
/// size, rounded and never under the first, when the person changed it and it is not the one kept already. A panel
/// fitted to a smaller screen and closed at that size keeps the size picked on a larger one.
pub fn to_keep(saved: Option<[f64; 2]>, placed: Option<(f64, f64)>, now: (f64, f64)) -> Option<[f64; 2]> {
    if !resized(placed, now) {
        return None;
    }
    let (width, height) = size(Some([now.0.round(), now.1.round()]));
    (size(saved) != (width, height)).then_some([width, height])
}

/// The panel window's own motion on macOS, where the window moves and the page does not (Nexus GPUI's
/// `panel_motion.rs`): it fades in and drops a few points into place as it
/// opens, and fades out as it closes. Windows and Linux move the page instead, and close at once.
pub mod motion {
    use std::time::Duration;

    /// How far the panel drops into place as it opens, in points.
    pub const DROP: f64 = 5.0;
    /// How long it opens and closes. These were Nexus UI's `--nx-dur` and `--nx-dur-fast`; nexus-ui.css no longer has
    /// those tokens, and the values stay.
    pub const OPEN: Duration = Duration::from_millis(200);
    pub const CLOSE: Duration = Duration::from_millis(150);
    /// The curve, `cubic-bezier(0.22, 1, 0.36, 1)`, quick to respond and slow to settle: Nexus UI's former
    /// `--nx-ease-out-soft`.
    pub const EASE_OUT_SOFT: (f32, f32, f32, f32) = (0.22, 1.0, 0.36, 1.0);

    /// The open motion: how long it takes and how far the window drops. With Reduce motion, only a short fade.
    pub fn open(reduce: bool) -> (Duration, f64) {
        if reduce { (CLOSE, 0.0) } else { (OPEN, DROP) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduce_motion_keeps_only_a_short_fade() {
        assert_eq!(motion::open(false), (motion::OPEN, motion::DROP));
        assert_eq!(motion::open(true), (motion::CLOSE, 0.0));
    }

    #[test]
    fn only_a_size_the_person_changed_is_kept() {
        // Fitted to a laptop on this open and closed at that size: not the person's, so not kept.
        assert!(!resized(Some((520.0, 831.0)), (520.0, 831.0)));
        assert!(!resized(Some((520.0, 831.0)), (520.4, 830.6)));
        // Made larger with the grip or an edge: kept.
        assert!(resized(Some((520.0, 831.0)), (600.0, 831.0)));
        // No size from this open: the current one is kept.
        assert!(resized(None, (520.0, 831.0)));
    }

    #[test]
    fn the_grip_grows_the_panel_left_and_down_with_its_top_right_corner_still() {
        let start = (1600.0, 80.0, 880.0, 1200.0);
        let min = (880.0, 1200.0);
        let max = (f64::INFINITY, f64::INFINITY);
        // Left 100 px and down 60 px: 100 px wider to the left, 60 px taller, the right edge at 2480 still.
        assert_eq!(
            grip_frame(start, (-100.0, 60.0), min, max),
            (1500.0, 80.0, 980.0, 1260.0)
        );
        // Right and up, past the smallest size: it stops at the minimum, the right edge still at 2480.
        assert_eq!(
            grip_frame(start, (300.0, -500.0), min, max),
            (1600.0, 80.0, 880.0, 1200.0)
        );
        // Past the screen's edge: it stops there, the right edge still at 2480.
        assert_eq!(
            grip_frame(start, (-5000.0, 5000.0), min, (2464.0, 1700.0)),
            (16.0, 80.0, 2464.0, 1700.0)
        );
    }

    #[test]
    fn a_panel_kept_from_a_larger_display_fits_a_smaller_one() {
        // 900 pt tall (1800 px) on a MacBook whose work area is 1662 px: 16 px from the top and the bottom.
        assert_eq!(fit((1040.0, 1800.0), (3024.0, 1662.0), 16.0), (1040.0, 1630.0));
        // A panel that fits is left as it is.
        assert_eq!(fit((880.0, 1200.0), (3024.0, 1662.0), 16.0), (880.0, 1200.0));
    }

    #[test]
    fn opens_at_the_size_it_was_left_at_never_smaller() {
        assert_eq!(size(None), (WIDTH, HEIGHT));
        assert_eq!(size(Some([520.0, 720.0])), (520.0, 720.0));
        assert_eq!(size(Some([300.0, 900.0])), (WIDTH, 900.0));
        assert_eq!(size(Some([f64::NAN, 700.0])), (WIDTH, HEIGHT));
    }

    #[test]
    fn a_larger_panel_stays_under_the_item_and_on_screen() {
        // Made 500 × 750 pt (1000 × 1500 px at 2×), under an item near the right edge: 8 pt from the edge, under the bar.
        let (x, y) = anchor(
            (2990.0, 0.0, 30.0, 48.0),
            (1000.0, 1500.0),
            (0.0, 48.0, 3024.0, 1842.0),
            16.0,
        );
        assert_eq!((x, y), (3024.0 - 1000.0 - 16.0, 64.0));
        // Taller than the work area (a smaller display): its top at the work area's top, never above it.
        let (_, y) = anchor(
            (2400.0, 0.0, 44.0, 48.0),
            (880.0, 2400.0),
            (0.0, 48.0, 3024.0, 1842.0),
            16.0,
        );
        assert_eq!(y, 48.0);
    }

    #[test]
    fn under_a_menu_bar_item_and_above_a_taskbar_icon() {
        // A Retina menu bar: the item at x 2400, 48 px tall; the panel 880 × 1200 under it, 8 pt (16 px) below.
        let (x, y) = anchor(
            (2400.0, 0.0, 44.0, 48.0),
            (880.0, 1200.0),
            (0.0, 48.0, 3024.0, 1842.0),
            16.0,
        );
        assert_eq!((x, y), (2400.0 + 22.0 - 440.0, 64.0));
        // Near the right edge: kept on screen, the gap from the edge too.
        let (x, _) = anchor(
            (2990.0, 0.0, 30.0, 48.0),
            (880.0, 1200.0),
            (0.0, 48.0, 3024.0, 1842.0),
            16.0,
        );
        assert_eq!(x, 3024.0 - 880.0 - 16.0);
        // A notched MacBook: the menu bar is 74 px; the item's rect reports it.
        let (_, y) = anchor(
            (2400.0, 0.0, 44.0, 74.0),
            (880.0, 1200.0),
            (0.0, 74.0, 3456.0, 2160.0),
            16.0,
        );
        assert_eq!(y, 90.0);
        // A second display to the left (negative x), its own menu bar at its top (y -2160).
        let (x, y) = anchor(
            (-200.0, -2160.0, 44.0, 48.0),
            (880.0, 1200.0),
            (-1920.0, -2112.0, 1920.0, 2112.0),
            16.0,
        );
        assert_eq!((x, y), (-880.0 - 16.0, -2112.0 + 16.0));
        // An item whose rect ends above the work area (the menu bar hidden and shown): still below the bar.
        let (_, y) = anchor(
            (2400.0, 0.0, 44.0, 40.0),
            (880.0, 1200.0),
            (0.0, 48.0, 3024.0, 1842.0),
            16.0,
        );
        assert_eq!(y, 64.0);
        // Windows: the icon in the taskbar at the bottom, the panel above the taskbar.
        let (_, y) = anchor(
            (1700.0, 1040.0, 24.0, 40.0),
            (440.0, 600.0),
            (0.0, 0.0, 1920.0, 1040.0),
            8.0,
        );
        assert_eq!(y, 1040.0 - 600.0 - 8.0);
    }

    #[test]
    fn a_kept_size_is_only_one_the_person_changed_and_not_kept_yet() {
        assert_eq!(to_keep(None, Some((440.0, 600.0)), (440.0, 600.0)), None);
        assert_eq!(
            to_keep(None, Some((440.0, 600.0)), (520.4, 700.6)),
            Some([520.0, 701.0])
        );
        assert_eq!(
            to_keep(Some([520.0, 701.0]), Some((440.0, 600.0)), (520.0, 701.0)),
            None
        );
        // Never under the first size: a window a little smaller than asked keeps nothing.
        assert_eq!(to_keep(None, None, (400.0, 560.0)), None);
    }
}
