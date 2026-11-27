//! A nonactivating floating window sized to the pill, so invisible space never blocks other apps.
use gpui_kit::*;

pub const COMPACT: f32 = 242.;
pub const EXPANDED: f32 = 352.;
pub const HEADER: f32 = 38.;

pub fn options(owner: Option<AnyWindowHandle>, cx: &mut App) -> WindowOptions {
    let zoom = gpui_kit::ui_zoom();
    let width = COMPACT * zoom;
    let height = HEADER * zoom;
    let display = owner
        .and_then(|owner| owner.update(cx, |_, window, cx| window.display(cx)).ok().flatten())
        .or_else(|| cx.primary_display());
    let bounds = display
        .as_ref()
        .map(|display| {
            let area = display.visible_bounds();
            Bounds::new(
                point(
                    area.origin.x + (area.size.width - px(width)) / 2.,
                    area.origin.y + px(10.),
                ),
                size(px(width), px(height)),
            )
        })
        .unwrap_or_else(|| Bounds::centered(None, size(px(width), px(height)), cx));
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: false,
        show: true,
        kind: window_kind(cx.compositor_name()),
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(WindowDecorations::Client),
        display_id: display.map(|display| display.id()),
        app_id: Some(crate::app_id::get()),
        ..WindowOptions::default()
    }
}

/// A regular Wayland toplevel can be tiled across the workspace, regardless of its requested size.
/// Layer-shell keeps the indicator above other apps without reserving desktop space or taking focus.
fn window_kind(compositor: &str) -> WindowKind {
    #[cfg(target_os = "linux")]
    {
        if compositor == "Wayland" {
            use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
            return WindowKind::LayerShell(LayerShellOptions {
                namespace: "nexus-browser-activity".into(),
                layer: Layer::Overlay,
                anchor: Anchor::TOP,
                exclusive_zone: Some(px(0.)),
                margin: Some((px(10.), px(0.), px(0.), px(0.))),
                keyboard_interactivity: KeyboardInteractivity::OnDemand,
                ..Default::default()
            });
        }
        WindowKind::Normal
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = compositor;
        WindowKind::PopUp
    }
}

pub fn resize(window: &mut Window, width: f32, height: f32, reduce: bool) {
    let width = width * gpui_kit::ui_zoom();
    let height = height * gpui_kit::ui_zoom();
    #[cfg(target_os = "macos")]
    if mac::resize(window, width, height, reduce) {
        return;
    }
    let _ = reduce;
    window.resize(size(px(width), px(height)));
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use gpui_kit::Window;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAnimatablePropertyContainer as _, NSAnimationContext, NSView};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    pub fn resize(window: &Window, width: f32, height: f32, reduce: bool) -> bool {
        let Some(_) = MainThreadMarker::new() else { return false };
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return false;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return false;
        };
        // SAFETY: GPUI owns this live content view. This function runs only on the main thread.
        let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let Some(ns) = view.window() else { return false };
        let mut frame = ns.frame();
        frame.origin.x += (frame.size.width - f64::from(width)) / 2.;
        frame.origin.y += frame.size.height - f64::from(height);
        frame.size.width = f64::from(width);
        frame.size.height = f64::from(height);
        if reduce {
            ns.setFrame_display(frame, true);
        } else {
            NSAnimationContext::beginGrouping();
            NSAnimationContext::currentContext().setDuration(0.18);
            ns.animator().setFrame_display(frame, true);
            NSAnimationContext::endGrouping();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[cfg(target_os = "linux")]
    #[test]
    fn wayland_indicator_is_a_nonexclusive_keyboard_accessible_overlay() {
        use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer};
        let WindowKind::LayerShell(options) = window_kind("Wayland") else {
            panic!("the activity indicator must not be a tiled Wayland toplevel");
        };
        assert_eq!(options.layer, Layer::Overlay);
        assert_eq!(options.anchor, Anchor::TOP);
        assert_eq!(options.exclusive_zone, Some(px(0.)));
        assert_eq!(options.margin, Some((px(10.), px(0.), px(0.), px(0.))));
        assert_eq!(options.keyboard_interactivity, KeyboardInteractivity::OnDemand);
        assert_eq!(options.namespace, "nexus-browser-activity");
        assert_eq!(window_kind("X11"), WindowKind::Normal);
    }

    #[gpui_kit::test]
    fn indicator_opens_at_its_compact_size_without_activating(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let options = options(None, cx);
            assert_eq!(
                options.window_bounds.unwrap().get_bounds().size,
                size(px(COMPACT), px(HEADER))
            );
            assert!(!options.focus);
            assert!(!options.is_resizable);
        });
    }
}
