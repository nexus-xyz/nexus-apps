//! A double click on the app window's title bar does what the desktop's setting says, as a system title bar does.
//!
//! - Linux: KWin's setting on KDE Plasma, GNOME's elsewhere (`nexus_desktop_core::title_bar::linux`), read at each double
//!   click, so a change in System Settings applies at once. Maximize or restore, minimize, the window menu, or nothing.
//!   GPUI Kit's title bar would always maximize; the workspace's bar takes the double click first.
//! - macOS: GPUI's own handler (`titlebar_double_click`, in `vendor/gpui-pre-macos`) reads `AppleActionOnDoubleClick`.
//! - Windows: the bar is the window's caption area, and Windows maximizes or restores it, as the Nexus app does.

use gpui_kit::*;
use nexus_desktop_core::title_bar::DoubleClick;

/// Whether this build handles the double click itself (Linux). Elsewhere GPUI and the OS do.
pub const HANDLED_HERE: bool = cfg!(target_os = "linux");

/// The title bar was double-clicked at `position`: read the setting off the main thread, then act on the window.
pub fn double_click<T: 'static>(position: Point<Pixels>, window: &mut Window, cx: &mut Context<T>) {
    cx.spawn_in(window, async move |_, cx| {
        let action = cx
            .background_executor()
            .spawn(async { nexus_desktop_core::title_bar::linux() })
            .await;
        let _ = cx.update(|window, _| apply(action, position, window));
    })
    .detach();
}

fn apply(action: DoubleClick, position: Point<Pixels>, window: &mut Window) {
    match action {
        // GPUI's zoom maximizes the window, or restores it when it is maximized.
        DoubleClick::Fill => window.zoom_window(),
        DoubleClick::Minimize => window.minimize_window(),
        DoubleClick::Menu => window.show_window_menu(position),
        DoubleClick::Nothing => {}
    }
}
