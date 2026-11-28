//! Rewind's confirmation and the editor in a message's place, drawn as the session's (`code_view.rs`'s
//! `render_rewind_confirm` and `render_editor`).
use super::edit::{Editing, Rewinding};
use super::*;
use crate::palette::Palette;
use crate::workspace::code_dock::answer_button;
use crate::workspace::glass::FloatingGround;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;

impl SideChat {
    /// Rewind's confirmation under a message: what it does and what it leaves, then Cancel and Rewind.
    pub(super) fn render_rewind_confirm(
        &self,
        rewinding: &Rewinding,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = rewinding.busy;
        v_flex()
            .w(px(300.))
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(12.))
            .floating_ground(12., p.pop, false, &p)
            .shadow_md()
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Rewind to before this message?"),
            )
            .child(div().text_size(px(12.5)).line_height(px(18.)).text_color(p.muted).child(
                "The prompt returns to the composer. Later messages stay visible, dimmed. Files, past commands, \
                 connector changes, and pushes stay unchanged.",
            ))
            .when_some(rewinding.error.clone(), |card, error| {
                card.child(div().text_size(px(12.5)).text_color(p.destructive).child(error))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        answer_button("side-rewind-cancel", "Cancel", false, !busy, p)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_rewind(cx))),
                    )
                    .child(
                        answer_button(
                            "side-rewind-confirm",
                            if busy { "Rewinding…" } else { "Rewind" },
                            true,
                            !busy,
                            p,
                        )
                        .debug_selector(|| "side-rewind-confirm".into())
                        .on_click(cx.listener(|this, _, window, cx| this.confirm_rewind(window, cx))),
                    ),
            )
            .into_any_element()
    }

    /// The editor in a message's place: the text, what sending does, then Cancel and Save & send.
    pub(super) fn render_editor(&self, editing: &Editing, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let busy = editing.busy;
        let running = self.summary.status.busy();
        let empty = editing.empty(cx);
        v_flex()
            .w(relative(0.8))
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(12.))
            .bg(p.bubble)
            // Esc leaves the editor, as the session's.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.cancel_edit(window, cx);
                }
            }))
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .mx(px(-10.))
                    .child(Textarea::new(&editing.input).appearance(false)),
            )
            .child(div().text_size(px(12.5)).line_height(px(18.)).text_color(p.muted).child(
                "Sending replaces this message and later replies. Files, past commands, connector changes, and pushes \
                 stay unchanged.",
            ))
            .when_some(editing.error.clone(), |card, error| {
                card.child(div().text_size(px(12.5)).text_color(p.destructive).child(error))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        answer_button("side-edit-cancel", "Cancel", false, !busy, p)
                            .on_click(cx.listener(|this, _, window, cx| this.cancel_edit(window, cx))),
                    )
                    .child(
                        answer_button(
                            "side-edit-send",
                            if busy { "Sending…" } else { "Save & send" },
                            true,
                            !busy && !empty && !running,
                            p,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.send_edit(window, cx))),
                    ),
            )
            .into_any_element()
    }
}
