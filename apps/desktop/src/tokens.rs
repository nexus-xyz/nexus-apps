//! The shared sizes of the 2026-11 revamp (the shell spec's global scale), one name per value, so every screen draws
//! the same corners, heads and menus. Colors live in `palette.rs`.
//!
//! Areas move onto these as they are redrawn; a value not named here is not on the scale.

/// Corner radii. The scale is 4 / 6 / 10 / 12 / 16 / pill and nothing else. Inner radius = outer radius - inset, so
/// a control inset 2 inside a 6 box is 4, and a control inside the composer's 10 padding is 16 - 10 = 6.
pub mod radius {
    /// Tiny inline things: inline code, checkboxes, find highlights, a segment inset 2 inside a 6 track.
    #[allow(dead_code)] // waits for the screen that first uses it (redraw in progress)
    pub const INLINE: f32 = 4.;
    /// Controls, rows, text buttons, fields, chips (`--radius-panel`).
    pub const CONTROL: f32 = 6.;
    /// Menus, popovers, toasts, content cards (`--radius-card`).
    pub const CARD: f32 = 10.;
    /// Modal cards (the crash and outside-access prompts) and the menu box (canon `.nx-menu`). Canon's window panel
    /// (`--nx-radius-frame`) is 12 too: the shell does have a rounded panel, drawn by the window code, not by a token.
    #[allow(dead_code)] // waits for the screen that first uses it (redraw in progress)
    pub const MODAL: f32 = 12.;
    /// The composer box and the cards that take its place, nothing else (`--radius-composer`).
    pub const COMPOSER: f32 = 16.;
    /// Avatars, dots, switches, round buttons.
    #[allow(dead_code)] // waits for the screen that first uses it (redraw in progress)
    pub const PILL: f32 = 9999.;
}

/// Canon row radius (`--nx-radius-row`): sidebar, settings and menu rows. Wider than `radius::CONTROL`; the 6 scale
/// above stays for controls and fields.
#[allow(dead_code)] // waits for the screen that first uses it (canon build)
pub const ROW_RADIUS: f32 = 8.;

/// Canon row height (`--nx-row`): sidebar places and sessions, settings nav rows.
#[allow(dead_code)] // waits for the screen that first uses it (canon build)
pub const ROW_HEIGHT: f32 = 32.;

/// Canon group radius (`--nx-radius-md`): a settings card (`.nx-set-group`).
#[allow(dead_code)] // waits for the screen that first uses it (canon build)
pub const GROUP_RADIUS: f32 = 14.;

/// Canon menu box radius (`.nx-menu`); same value as `radius::MODAL`.
#[allow(dead_code)] // waits for the screen that first uses it (canon build)
pub const MENU_RADIUS: f32 = 12.;

/// The keyboard focus ring's width (`--nx-border-focus`); colour is `Palette::focus_ring`.
pub const FOCUS_RING: f32 = 2.;

/// The composer box's padding: the scale's one named exception, so the 6 radius controls inside sit concentric with
/// its 16 corners.
#[allow(dead_code)] // waits for the screen that first uses it (redraw in progress)
pub const COMPOSER_PADDING: f32 = 10.;

/// The height of the heads that use it: the sidebar's and the main pane's top bar. Icons center on 22.
pub const HEAD: f32 = 44.;

/// The sidebar pane's width.
pub const SIDEBAR: f32 = 256.;

/// The macOS traffic-light slot, after the head's 12 px leading inset. Native buttons end near x 80;
/// the first toolbar button starts at x 100, leaving space around both hit targets.
pub const TRAFFIC_LIGHTS: f32 = 88.;

/// The native buttons' top-left. macOS draws 14 pt controls, centered on the 44 pt head at y 22.
/// Every main window uses this position; GPUI reapplies it after resize and fullscreen transitions.
pub const TRAFFIC_LIGHT_ORIGIN: (f32, f32) = (20., 15.);

/// Menus size to their longest row, never narrower than a sidebar row.
pub mod menu {
    /// The least width of a menu: the sidebar row's width, so a menu opened from a row lines up with it.
    pub const MIN_WIDTH: f32 = 240.;
    /// The Snooze submenu and the Custom time page that replaces it ("Until tomorrow morning" + its wake time).
    pub const SNOOZE_WIDTH: f32 = 304.;
    /// The person menu and the Team popover.
    #[allow(dead_code)] // waits for the screen that first uses it (redraw in progress)
    pub const PERSON_WIDTH: f32 = 288.;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canon sizes (`product-web/ds/tokens.css`) pinned so a drift shows here.
    #[test]
    fn canon_sizes_match_tokens_css() {
        assert_eq!((ROW_RADIUS, ROW_HEIGHT, GROUP_RADIUS, MENU_RADIUS), (8., 32., 14., 12.));
        assert_eq!(FOCUS_RING, 2.);
        assert_eq!(MENU_RADIUS, radius::MODAL);
    }
}
