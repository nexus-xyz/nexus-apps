//! The stacked replies at the composer's top.
//! A chip shows the passage, "With reply" and remove. Under the pointer it opens (`motion::reveal`, the composer's
//! own open and close) to show the reply in line with the passage, and a pencil fades in. The pencil, or a click on
//! the reply, rewrites it in place: Enter or Save keeps it, Esc or Cancel drops the change.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Reply, chip_fill, chip_label};
use crate::motion;
use crate::palette::Palette;
use crate::workspace::Workspace;
use crate::workspace::code_dock::text_button;

/// Where the reply sits under the passage: past the chip's 12px edge, its 15px icon and the 10px gap.
const INDENT: f32 = 37.;

impl Workspace {
    /// The pointer came onto chip `id`, or left it.
    fn reply_hovered(&mut self, id: u64, hovered: bool, cx: &mut Context<Self>) {
        let next = if hovered {
            Some(id)
        } else if self.replies.hovered == Some(id) {
            None
        } else {
            return;
        };
        if self.replies.hovered != next {
            self.replies.hovered = next;
            cx.notify();
        }
    }

    /// Rewrite chip `id`'s reply in place.
    pub(in crate::workspace) fn edit_reply(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self
            .pending_replies()
            .iter()
            .find(|reply| reply.id == id)
            .map(|reply| reply.text.clone())
        else {
            return;
        };
        self.replies.editing = Some(id);
        self.replies.edit.update(cx, |edit, cx| {
            edit.set_value(text, window, cx);
            edit.focus(window, cx);
        });
        cx.notify();
    }

    /// Keep the rewrite. An empty one leaves the passage without a reply.
    pub(in crate::workspace) fn save_reply_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.replies.editing.take() else {
            return;
        };
        let text = self.replies.edit.read(cx).value().trim().to_string();
        if let Some(reply) = self.replies.stacks.values_mut().flatten().find(|reply| reply.id == id) {
            reply.text = text;
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Drop the rewrite; the reply stays as it was.
    pub(in crate::workspace) fn cancel_reply_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.replies.editing.take().is_some() {
            self.focus_composer(window, cx);
            cx.notify();
        }
    }

    pub(in crate::workspace) fn remove_reply(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(thread) = self.reply_thread().map(str::to_string) else {
            return;
        };
        if let Some(stack) = self.replies.stacks.get_mut(&thread) {
            stack.retain(|reply| reply.id != id);
            if stack.is_empty() {
                self.replies.stacks.remove(&thread);
            }
        }
        if self.replies.editing == Some(id) {
            self.replies.editing = None;
        }
        if self.replies.hovered == Some(id) {
            self.replies.hovered = None;
        }
        cx.notify();
    }

    /// The stacked replies at the composer's top.
    pub(in crate::workspace) fn render_reply_chips(&self, palette: Palette, cx: &Context<Self>) -> Option<AnyElement> {
        let replies = self.pending_replies();
        if replies.is_empty() {
            return None;
        }
        let chips = replies.iter().map(|reply| self.render_reply_chip(reply, palette, cx));
        Some(
            v_flex()
                .w_full()
                .gap(px(4.))
                .pt(px(8.))
                .px(px(8.))
                .children(chips)
                .into_any_element(),
        )
    }

    fn render_reply_chip(&self, reply: &Reply, palette: Palette, cx: &Context<Self>) -> AnyElement {
        let id = reply.id;
        let editing = self.replies.editing == Some(id);
        let open = editing || self.replies.hovered == Some(id);
        let icon_button = |name: &'static str, icon: Lucide, on: bool| {
            div()
                .id((name, id))
                .flex_none()
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .cursor_pointer()
                .when(on, |button| button.bg(palette.raised))
                .hover(|style| style.bg(palette.raised))
                .child(
                    Icon::new(icon)
                        .size(px(14.))
                        .text_color(if on { palette.foreground } else { palette.muted }),
                )
        };
        let pencil = icon_button("stacked-reply-edit", Lucide::Pencil, editing)
            .debug_selector(|| "stacked-reply-edit".into())
            .tooltip(move |window, cx| Tooltip::new(if editing { "Editing" } else { "Edit reply" }).build(window, cx))
            .on_click(cx.listener(move |this, _, window, cx| this.edit_reply(id, window, cx)));
        let head = h_flex()
            .h(px(36.))
            .pl(px(12.))
            .pr(px(8.))
            .gap(px(10.))
            .child(Icon::new(Lucide::Reply).size(px(15.)).text_color(palette.faint))
            .child(div().flex_1().min_w(px(0.)).truncate().child(chip_label(&reply.quote)))
            .when(!reply.text.is_empty(), |head| {
                head.child(
                    div()
                        .flex_none()
                        .px(px(8.))
                        .py(px(1.))
                        .rounded_full()
                        .bg(palette.raised)
                        .text_size(px(11.5))
                        .text_color(palette.muted)
                        .child("With reply"),
                )
            })
            // The pencil fades in with the hover (`motion::fade_in`), beside remove.
            .when(open, |head| {
                head.child(motion::fade_in(pencil, ("stacked-reply-edit-fade", id)))
            })
            .child(
                icon_button("stacked-reply-remove", Lucide::X, false)
                    .tooltip(|window, cx| Tooltip::new("Remove reply").build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_reply(id, cx))),
            );
        let body = open.then(|| {
            if editing {
                self.render_reply_editor(id, palette, cx)
            } else {
                let empty = reply.text.is_empty();
                div()
                    .id(("stacked-reply-text", id))
                    .debug_selector(|| "stacked-reply-text".into())
                    .pl(px(INDENT))
                    .pr(px(12.))
                    .pb(px(10.))
                    .line_height(px(20.))
                    .cursor_text()
                    .text_color(if empty { palette.faint } else { palette.foreground })
                    .child(if empty {
                        "No reply yet. Add one with the pencil.".to_string()
                    } else {
                        reply.text.clone()
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.edit_reply(id, window, cx)))
                    .into_any_element()
            }
        });
        v_flex()
            .id(("stacked-reply", id))
            .debug_selector(|| "stacked-reply".into())
            .w_full()
            .rounded(px(10.))
            .border_1()
            .border_color(palette.border)
            .bg(chip_fill(palette))
            .text_size(px(13.5))
            .text_color(palette.muted)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.reply_hovered(id, *hovered, cx)))
            .child(head)
            // Opens and closes in height with its words fading, as the composer's other rows (`motion::reveal`).
            .child(motion::reveal(("stacked-reply-open", id), open, body))
            .into_any_element()
    }

    /// The rewrite in place of the reply: the field, then the keys' hint, Cancel and Save.
    fn render_reply_editor(&self, id: u64, palette: Palette, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .id(("stacked-reply-editor", id))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.cancel_reply_edit(window, cx);
                }
            }))
            .child(
                div()
                    .ml(px(INDENT - 13.))
                    .mr(px(8.))
                    .px(px(12.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(palette.faint)
                    .bg(palette.raised)
                    .text_color(palette.foreground)
                    .child(Textarea::new(&self.replies.edit).appearance(false)),
            )
            .child(
                h_flex()
                    .pl(px(INDENT))
                    .pr(px(8.))
                    .py(px(6.))
                    .gap(px(4.))
                    .text_size(px(13.))
                    .child(
                        div()
                            .flex_1()
                            .text_color(palette.faint)
                            .child("Enter saves \u{b7} Esc cancels"),
                    )
                    .child(
                        text_button(("stacked-reply-cancel", id), "Cancel", false, palette)
                            .on_click(cx.listener(|this, _, window, cx| this.cancel_reply_edit(window, cx))),
                    )
                    .child(
                        text_button(("stacked-reply-save", id), "Save", true, palette)
                            .on_click(cx.listener(|this, _, window, cx| this.save_reply_edit(window, cx))),
                    ),
            )
            .into_any_element()
    }
}
