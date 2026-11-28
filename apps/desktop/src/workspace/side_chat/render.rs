use super::*;
use crate::palette::Palette;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;

impl SideChat {
    pub(in crate::workspace) fn focused(&self, window: &Window, cx: &App) -> bool {
        self.model_input.focus_handle(cx).contains_focused(window, cx)
            || self
                .rewrite
                .as_ref()
                .is_some_and(|edit| edit.input.focus_handle(cx).contains_focused(window, cx))
            || self.input.focus_handle(cx).contains_focused(window, cx)
            || self
                .renaming
                .as_ref()
                .is_some_and(|renaming| renaming.input.focus_handle(cx).contains_focused(window, cx))
            || self.secret.focus_handle(cx).contains_focused(window, cx)
            || self
                .answers
                .iter()
                .any(|i| i.focus_handle(cx).contains_focused(window, cx))
    }
}

impl Render for SideChat {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_choice(super::super::settings_view::saved_theme(), window.appearance());
        // The now line's clock runs from when the side agent started working.
        match (self.summary.status.busy(), self.working_since) {
            (true, None) => self.working_since = Some(std::time::Instant::now()),
            (false, Some(_)) => self.working_since = None,
            _ => {}
        }
        self.sync_history(p, cx);
        let body = div().id("side-chat-log").flex_1().min_h(px(0.)).bg(p.background).child(
            list(
                self.scroll.list.clone(),
                cx.processor(|this: &mut Self, ix: usize, window, cx| this.history_row(ix, window, cx)),
            )
            .size_full(),
        );
        let head = self.render_head(p, cx);
        let dock = self.render_dock(window, p, cx);
        let jump = h_flex()
            .absolute()
            .left_0()
            .right_0()
            .bottom(self.dock_height.room() + px(12.))
            .justify_center()
            .child(
                super::super::code_view::jump_circle("side-jump-latest", p).on_click(cx.listener(|this, _, _, cx| {
                    this.scroll.to_end();
                    cx.notify();
                })),
            );
        let dock =
            super::super::floating_dock::floating_dock(dock, self.dock_height.clone(), p, cx, |_, cx| cx.notify());
        v_flex()
            .size_full()
            .min_h(px(0.))
            .bg(p.sidebar)
            .text_color(p.foreground)
            .text_size(px(13.))
            // A passage selected in a side answer offers Reply; a press outside the reply box closes it (`replies.rs`).
            .capture_any_mouse_down(cx.listener(|this, event, window, cx| this.cite_pointer_down(event, window, cx)))
            .capture_any_mouse_up(cx.listener(|this, event, window, cx| this.cite_pointer_up(event, window, cx)))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.paste_as_text(&event.keystroke, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.as_str() == "escape" && this.dismiss_cite(cx) {
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key.as_str() == "escape" && this.renaming.is_some() {
                    this.end_rename(window, cx);
                    cx.stop_propagation();
                    return;
                }
                // As in the session, Esc closes what is open and never reaches a running turn's Stop: Stop is its
                // button, or Mod+. (`session_keys.rs`).
                if event.keystroke.key.as_str() == "escape"
                    && (this.menu_open
                        || this.deleting
                        || this.rewrite.is_some()
                        || this.settings.is_some()
                        || this.attach_open)
                {
                    this.menu_open = false;
                    this.deleting = false;
                    this.rewrite = None;
                    this.settings = None;
                    this.attach_open = false;
                    this.input.focus_handle(cx).focus(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(head)
            .children(self.render_cite(p, cx))
            .children(self.render_reply_box(p, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .flex()
                    .child(body)
                    .child(super::super::transcript::top_fade(p))
                    .child(dock)
                    .when(!self.scroll.at_end(), |pane| pane.child(jump)),
            )
    }
}
