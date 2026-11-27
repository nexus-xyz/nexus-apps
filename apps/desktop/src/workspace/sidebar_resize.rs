//! The sidebar's right edge: drag, arrow keys, and double-click reset, as Nexus UI's `SidebarResize`.

use std::time::Duration;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;
use crate::i18n::tr;

// Nexus UI's `sidebar-script.ts` bounds and the shell's default width.
const MIN: f32 = 220.;
const MAX: f32 = 360.;
const DEFAULT: f32 = crate::tokens::SIDEBAR;

pub(super) struct SidebarSize {
    width: f32,
    focus: FocusHandle,
    _save: Option<Task<()>>,
}

impl SidebarSize {
    pub(super) fn new(cx: &mut Context<Workspace>) -> Self {
        Self {
            width: clamp(crate::paths::sidebar_width().unwrap_or(DEFAULT)),
            focus: cx.focus_handle(),
            _save: None,
        }
    }
}

fn clamp(width: f32) -> f32 {
    if width.is_finite() {
        width.round().clamp(MIN, MAX)
    } else {
        DEFAULT
    }
}

#[derive(Clone)]
struct Resize;

impl Render for Resize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

impl Workspace {
    pub(super) fn sidebar_width(&self) -> f32 {
        self.sidebar_size.width
    }

    /// What the open sidebar takes from the window's width: its card and the window's inset and gap around it.
    pub(super) fn sidebar_room(&self) -> f32 {
        self.sidebar_width() + crate::frost::GUTTER
    }

    fn set_sidebar_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let width = clamp(width);
        if width == self.sidebar_size.width {
            return;
        }
        self.sidebar_size.width = width;
        self.sidebar_size._save = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            crate::paths::save_sidebar_width(width);
        }));
        cx.notify();
    }

    pub(super) fn render_sidebar_resize(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar-resize")
            .absolute()
            .top_0()
            // In the gap between the sidebar card and the main pane (`frost::INSET`).
            .right(px(1.))
            .w(px(6.))
            .h_full()
            .cursor_col_resize()
            .track_focus(&self.sidebar_size.focus)
            .tab_index(0)
            .tooltip(|window, cx| Tooltip::new(tr("Drag to resize; double-click to reset")).build(window, cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.sidebar_size.focus, cx);
                    if event.click_count == 2 {
                        this.set_sidebar_width(DEFAULT, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let width = match event.keystroke.key.as_str() {
                    "left" => this.sidebar_width() - 16.,
                    "right" => this.sidebar_width() + 16.,
                    "home" => MIN,
                    "end" => MAX,
                    _ => return,
                };
                this.set_sidebar_width(width, cx);
                cx.stop_propagation();
            }))
            .on_drag(Resize, |_, _, _, cx| cx.new(|_| Resize))
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<Resize>, _, cx| {
                if this.sidebar_open {
                    // The card starts `frost::INSET` in, and the handle sits half the gap past its edge.
                    this.set_sidebar_width(f32::from(event.event.position.x) - 1.5 * crate::frost::INSET, cx);
                }
            }))
    }
}
