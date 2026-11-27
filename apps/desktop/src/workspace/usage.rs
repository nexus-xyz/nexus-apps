//! The activity heartbeat's input (`nexus_desktop_core::signals`): which windows have the focus, what
//! each shows, and when the person presses a key, clicks or scrolls. The core decides when a heartbeat goes and sends
//! it off the UI thread. Nothing here shows.

use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::signals::{self, Surface};

use super::{View, Workspace};

/// The core's name for a window.
pub(super) fn window_key(window: &Window) -> String {
    window.window_handle().window_id().as_u64().to_string()
}

impl Workspace {
    /// What this window shows, as the contract names it: the panel is the chat.
    pub(super) fn usage_surface(&self) -> Surface {
        if self.panel {
            return Surface::Chat;
        }
        match &self.view {
            View::Chat => Surface::Chat,
            View::Code(_) | View::NewCode | View::Pull(_) => Surface::Code,
            View::Library => Surface::Pages,
            View::Inbox
            | View::TeamSessions
            | View::Docs
            | View::Issues
            | View::Settings(_)
            | View::Models
            | View::Page(_) => Surface::Other,
        }
    }

    /// Each frame: this window's surface, and a listener for its clicks and scrolls (they last one frame).
    pub(super) fn usage_listener(&self, window: &mut Window) -> impl IntoElement {
        let key = window_key(window);
        signals::view(&key, self.usage_surface());
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let down = key.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, _| {
                    if phase == DispatchPhase::Capture {
                        signals::input(&down);
                    }
                });
                let scroll = key.clone();
                window.on_mouse_event(move |_: &ScrollEvent, phase, _, _| {
                    if phase == DispatchPhase::Capture {
                        signals::input(&scroll);
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }
}
