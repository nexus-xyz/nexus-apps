//! The side chat's Reply toolbar, reply box and stacked chips, drawn as the session's (`cite_selection.rs`,
//! `stacked_replies.rs`, `stacked_replies/chips.rs`).
use super::replies::LIFT;
use super::*;
use crate::motion;
use crate::palette::Palette;
use crate::workspace::cite_selection::bounds_probe;
use crate::workspace::code_dock::text_button;
use crate::workspace::glass::FloatingGround;
use crate::workspace::stacked_replies::{Reply, chip_fill, chip_label};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use nexus_workbench::cite;

/// Where a chip's reply sits under its passage: past the chip's 12px edge, its 15px icon and the 10px gap.
const INDENT: f32 = 37.;

impl SideChat {
    /// Over a selected passage of a side answer: Reply. A side chat does not start another side chat.
    pub(super) fn render_cite(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let selected = self.replies.selected.as_ref()?;
        let too_long = selected.encode_utf16().count() > cite::CITE_MAX;
        let item = h_flex()
            .id("side-cite-reply")
            .debug_selector(|| "side-cite-reply".into())
            .h(px(28.))
            .px(px(9.))
            .rounded(px(6.))
            .text_size(px(13.))
            .text_color(if too_long { p.faint } else { p.foreground })
            .when(!too_long, |item| {
                item.cursor_pointer()
                    .hover(|style| style.bg(p.menu_hover))
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(|this, _, window, cx| this.reply_to_selection(window, cx)))
            })
            .child(if too_long { "Selection too long" } else { "Reply" });
        let toolbar = h_flex()
            .relative()
            .p(px(2.))
            .rounded(px(8.))
            .border_1()
            .border_color(p.border)
            .shadow_md()
            .floating_ground(8., p.menu, true, &p)
            .child(item)
            .child(bounds_probe(self.replies.toolbar_bounds.clone()));
        Some(
            deferred(
                anchored()
                    .position(self.replies.pointer - point(px(0.), px(LIFT)))
                    .anchor(Anchor::BottomLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(toolbar),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }

    /// The reply box over the passage: the passage, then the reply and the add button.
    pub(super) fn render_reply_box(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let quote = self.replies.composing.as_ref()?;
        let card = v_flex()
            .debug_selector(|| "side-reply-box".into())
            .relative()
            .w(px(300.))
            .gap(px(10.))
            .pt(px(12.))
            .pb(px(10.))
            .pl(px(16.))
            .pr(px(10.))
            .rounded(px(14.))
            .border_1()
            .border_color(p.border)
            .shadow_lg()
            .floating_ground(14., p.menu, true, &p)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_reply(window, cx);
                }
            }))
            .child(
                h_flex()
                    .gap(px(8.))
                    .pr(px(6.))
                    .child(div().flex_none().w(px(2.)).h(px(18.)).rounded(px(1.)).bg(p.border))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(13.))
                            .text_color(p.muted)
                            .child(quote.split_whitespace().collect::<Vec<_>>().join(" ")),
                    ),
            )
            .child(
                h_flex()
                    .items_end()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(14.))
                            .child(Textarea::new(&self.replies.input).appearance(false)),
                    )
                    .child(
                        div()
                            .id("side-reply-add")
                            .flex_none()
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|style| style.bg(p.hover))
                            .child(Icon::new(Lucide::MessageCirclePlus).size(px(16.)).text_color(p.muted))
                            .tooltip(|window, cx| Tooltip::new("Add to message (Enter)").build(window, cx))
                            .on_click(cx.listener(|this, _, window, cx| this.add_reply(window, cx))),
                    ),
            )
            .child(bounds_probe(self.replies.box_bounds.clone()));
        Some(
            deferred(
                anchored()
                    .position(self.replies.pointer - point(px(0.), px(LIFT)))
                    .anchor(Anchor::BottomLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }

    /// The stacked replies at the composer's top.
    pub(super) fn render_reply_chips(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.replies.stack.is_empty() {
            return None;
        }
        let chips: Vec<AnyElement> = self
            .replies
            .stack
            .iter()
            .map(|reply| self.render_reply_chip(reply, p, cx))
            .collect();
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

    fn render_reply_chip(&self, reply: &Reply, p: Palette, cx: &mut Context<Self>) -> AnyElement {
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
                .when(on, |button| button.bg(p.raised))
                .hover(|style| style.bg(p.raised))
                .child(
                    Icon::new(icon)
                        .size(px(14.))
                        .text_color(if on { p.foreground } else { p.muted }),
                )
        };
        let pencil = icon_button("side-reply-edit", Lucide::Pencil, editing)
            .tooltip(move |window, cx| Tooltip::new(if editing { "Editing" } else { "Edit reply" }).build(window, cx))
            .on_click(cx.listener(move |this, _, window, cx| this.edit_reply(id, window, cx)));
        let head = h_flex()
            .h(px(36.))
            .pl(px(12.))
            .pr(px(8.))
            .gap(px(10.))
            .child(Icon::new(Lucide::Reply).size(px(15.)).text_color(p.faint))
            .child(div().flex_1().min_w(px(0.)).truncate().child(chip_label(&reply.quote)))
            .when(!reply.text.is_empty(), |head| {
                head.child(
                    div()
                        .flex_none()
                        .px(px(8.))
                        .py(px(1.))
                        .rounded_full()
                        .bg(p.raised)
                        .text_size(px(11.5))
                        .text_color(p.muted)
                        .child("With reply"),
                )
            })
            .when(open, |head| {
                head.child(motion::fade_in(pencil, ("side-reply-edit-fade", id)))
            })
            .child(
                icon_button("side-reply-remove", Lucide::X, false)
                    .tooltip(|window, cx| Tooltip::new("Remove reply").build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_reply(id, cx))),
            );
        let body = open.then(|| {
            if editing {
                self.render_reply_editor(id, p, cx)
            } else {
                let empty = reply.text.is_empty();
                div()
                    .id(("side-reply-text", id))
                    .pl(px(INDENT))
                    .pr(px(12.))
                    .pb(px(10.))
                    .line_height(px(20.))
                    .cursor_text()
                    .text_color(if empty { p.faint } else { p.foreground })
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
            .id(("side-stacked-reply", id))
            .debug_selector(|| "side-stacked-reply".into())
            .w_full()
            .rounded(px(10.))
            .border_1()
            .border_color(p.border)
            .bg(chip_fill(p))
            .text_size(px(13.5))
            .text_color(p.muted)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.reply_hovered(id, *hovered, cx)))
            .child(head)
            .child(motion::reveal(("side-stacked-reply-open", id), open, body))
            .into_any_element()
    }

    /// The rewrite in place of the reply: the field, then the keys' hint, Cancel and Save.
    fn render_reply_editor(&self, id: u64, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .id(("side-reply-editor", id))
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
                    .border_color(p.faint)
                    .bg(p.raised)
                    .text_color(p.foreground)
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
                            .text_color(p.faint)
                            .child("Enter saves \u{b7} Esc cancels"),
                    )
                    .child(
                        text_button(("side-reply-cancel", id), "Cancel", false, p)
                            .on_click(cx.listener(|this, _, window, cx| this.cancel_reply_edit(window, cx))),
                    )
                    .child(
                        text_button(("side-reply-save", id), "Save", true, p)
                            .on_click(cx.listener(|this, _, window, cx| this.save_reply_edit(window, cx))),
                    ),
            )
            .into_any_element()
    }
}
