//! Renaming a side chat in its head, as a session's title renames (`session_menu.rs`): its menu's Rename or a
//! double-click on the title opens a field. Enter keeps a non-empty title; Escape or a click outside drops it.
use super::*;
use crate::palette::Palette;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;

/// The title field while the person renames the side chat.
pub(super) struct Renaming {
    pub(super) input: Entity<InputState>,
    _subscription: Subscription,
}

impl SideChat {
    /// The title the head shows: the agent's name for it, or "New side chat" before the first message.
    pub(super) fn shown_title(&self) -> String {
        if self.is_empty() && self.summary.title == nexus_workbench::side_chat::TITLE {
            "New side chat".to_string()
        } else {
            self.summary.title.clone()
        }
    }

    pub(super) fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.deleting = false;
        let title = self.shown_title();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(title, window, cx);
            state
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let title = state.read(cx).value().trim().to_string();
                    this.end_rename(window, cx);
                    this.rename(title, window, cx);
                }
                InputEvent::Blur => {
                    this.renaming = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        input.update(cx, |state, cx| state.focus(window, cx));
        self.renaming = Some(Renaming {
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Drops the field and gives the composer its focus back.
    pub(super) fn end_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming = None;
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Saves `title` (`wb_thread_update`), which also stops the agent naming the side chat.
    pub(super) fn rename(&mut self, title: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_command() || title.is_empty() || title == self.summary.title {
            return;
        }
        self.summary.title = title.clone();
        self.retitled = Some(std::time::Instant::now());
        let args = json!({"threadId":self.summary.id,"title":title});
        self.command("wb_thread_update", args, false, None, window, cx);
    }

    /// The head's title: the field while renaming, else the title, which a double-click renames.
    pub(super) fn render_title(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        if let Some(renaming) = &self.renaming {
            return div()
                .flex_1()
                .min_w(px(0.))
                // The field pads its text by 10px: the box reaches back so the title doesn't move.
                .ml(px(-10.))
                .rounded(px(6.))
                .bg(p.selected)
                .text_color(p.foreground)
                .font_weight(FontWeight::MEDIUM)
                .debug_selector(|| "side-chat-title-field".into())
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.renaming = None;
                    cx.notify();
                }))
                .child(Input::new(&renaming.input).appearance(false))
                .into_any_element();
        }
        let shown = self.shown_title();
        let title = div()
            .id("side-chat-title")
            .debug_selector(|| "side-chat-title".into())
            .flex_1()
            .min_w(px(0.))
            .truncate()
            .font_weight(FontWeight::MEDIUM)
            .child(shown.clone())
            .when(self.can_command(), |title| {
                title.on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                    if event.click_count() == 2 {
                        cx.stop_propagation();
                        this.start_rename(window, cx);
                    }
                }))
            });
        // A new title, from its agent or a rename, rises into place.
        crate::motion::retitle(title, &shown, self.retitled)
    }
}
