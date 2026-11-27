//! Side chat context, empty state, and the explicit answer handoff.
use super::*;
use crate::motion;
use crate::palette::Palette;
use crate::workspace::MenuTrigger;
use crate::workspace::{answer_footer, code_dock, code_view, glyph};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;

impl SideChat {
    pub(super) fn is_empty(&self) -> bool {
        !self.events.iter().any(|stored| {
            stored.at >= self.summary.created_at
                && matches!(stored.event, nexus_workbench::event::WbEvent::User { from: None, .. })
        })
    }

    pub(super) fn has_answer(&self) -> bool {
        nexus_workbench::send_back::latest_answer(&self.events, self.summary.created_at).is_some()
    }

    pub(in crate::workspace) fn focus_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.focus_handle(cx).focus(window, cx);
    }

    pub(in crate::workspace) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.deleting = false;
        self.command(
            "wb_side_chat_close",
            json!({"threadId":self.summary.id}),
            true,
            None,
            window,
            cx,
        );
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.deleting = false;
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// The side chat's menu, as the session menu (`session_menu.rs`): Rename, Close, then Delete, whose confirmation takes
    /// the menu's place.
    fn render_menu(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.can_command();
        let item = |id: &'static str, icon: Option<Lucide>, label: &'static str, destructive: bool| {
            h_flex()
                .id(id)
                .debug_selector(move || id.into())
                .h(px(30.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .when(destructive, |row| row.text_color(p.destructive))
                .when(!enabled, |row| row.opacity(0.4))
                .when(enabled, |row| {
                    row.cursor_pointer().hover(|style| code_dock::menu_item_hover(style, p))
                })
                .children(icon.map(|icon| {
                    crate::workspace::glyph(icon, if destructive { p.destructive } else { p.icon }).size(px(16.))
                }))
                .child(div().flex_1().truncate().child(label))
        };
        let menu = code_dock::menu_box("side-chat-menu", 240., p)
            .text_size(px(13.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu_open = false;
                this.deleting = false;
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_menu(window, cx);
                }
            }));
        if self.deleting {
            return menu
                .child(
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_color(p.foreground)
                        .child("Delete this side chat and its messages?"),
                )
                .child(
                    h_flex()
                        .px(px(4.))
                        .pb(px(4.))
                        .gap(px(6.))
                        .justify_end()
                        .child(
                            item("keep-side-chat", None, "Keep", false)
                                .on_click(cx.listener(|this, _, window, cx| this.close_menu(window, cx))),
                        )
                        .child(item("confirm-side-delete", None, "Delete", true).when(enabled, |row| {
                            row.on_click(cx.listener(|this, _, window, cx| {
                                this.menu_open = false;
                                this.deleting = false;
                                this.command(
                                    "wb_thread_delete",
                                    json!({"threadId":this.summary.id}),
                                    true,
                                    None,
                                    window,
                                    cx,
                                );
                            }))
                        })),
                );
        }
        menu.child(
            item("side-menu-rename", Some(Lucide::Pencil), "Rename side chat", false).when(enabled, |row| {
                row.on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.start_rename(window, cx);
                }))
            }),
        )
        .child(
            item(
                "side-menu-close",
                Some(Lucide::PanelRightClose),
                "Close side chat",
                false,
            )
            .when(enabled, |row| {
                row.on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
            }),
        )
        .child(div().h(px(8.)))
        .child(
            item("side-menu-delete", Some(Lucide::Trash), "Delete side chat…", true).when(enabled, |row| {
                row.on_click(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.deleting = true;
                    cx.notify();
                }))
            }),
        )
    }

    pub(super) fn render_head(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let open = self.menu_open;
        v_flex()
            .flex_none()
            .px_4()
            .py_3()
            .gap_1()
            .border_b_1()
            .border_color(p.border)
            .child(
                h_flex()
                    .gap_2()
                    .min_w(px(0.))
                    .child(self.render_title(p, cx))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(p.muted)
                            .child(super::super::code_dock::agent_name(self.summary.agent)),
                    )
                    .child(
                        div()
                            .relative()
                            .debug_selector(|| "side-chat-options".into())
                            .menu_trigger(
                                open,
                                cx,
                                |this| this.menu_open,
                                |this, _, cx| {
                                    this.menu_open = !this.menu_open;
                                    this.deleting = false;
                                    cx.notify();
                                },
                            )
                            .child(
                                Button::new("side-chat-options")
                                    .ghost()
                                    .icon(Lucide::Ellipsis)
                                    .accessibility_label("Side chat options")
                                    .tooltip("Side chat options")
                                    .on_click(cx.listener(|this, event, _, cx| {
                                        if !matches!(event, ClickEvent::Mouse(_)) {
                                            this.menu_open = !this.menu_open;
                                            this.deleting = false;
                                            cx.notify();
                                        }
                                    })),
                            )
                            .when_some(motion::presence("side-chat-menu", open, cx), |container, phase| {
                                let menu = phase.menu("side-chat-menu", -4., self.render_menu(p, cx));
                                container.child(
                                    div().absolute().right_0().top(px(28.)).child(
                                        deferred(
                                            anchored()
                                                .anchor(Anchor::TopRight)
                                                .offset(point(px(0.), px(4.)))
                                                .snap_to_window_with_margin(px(8.))
                                                .child(menu),
                                        )
                                        .with_priority(2),
                                    ),
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .min_w(px(0.))
                    .text_size(px(12.))
                    .text_color(p.muted)
                    .child(Icon::new(Lucide::GitBranch).size(px(13.)))
                    .child("From")
                    .child(
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .text_color(p.foreground)
                            .child(self.parent_title.clone()),
                    ),
            )
    }

    pub(super) fn render_welcome(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        v_flex()
            .flex_1()
            .w_full()
            .min_h(px(220.))
            .items_center()
            .justify_center()
            .px_3()
            .gap_3()
            .child(
                div()
                    .size(px(44.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(p.border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.muted)
                    .child(Icon::new(Lucide::MessageSquare).size(px(22.))),
            )
            .child(div().text_size(px(20.)).child("A little room to think."))
            .child(
                div()
                    .text_center()
                    .text_size(px(13.))
                    .text_color(p.muted)
                    .child("Ask a question about this session. The main agent keeps working."),
            )
            .child(
                h_flex().gap_2().flex_wrap().justify_center().children(
                    ["Summarize the approach", "Explain the latest change"]
                        .into_iter()
                        .enumerate()
                        .map(|(ix, text)| {
                            div().debug_selector(move || format!("side-suggestion-{ix}")).child(
                                Button::new(("side-chat-suggestion", ix))
                                    .outline()
                                    .label(text)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.input.update(cx, |input, cx| {
                                            input.set_value(text, window, cx);
                                            input.focus(window, cx);
                                        });
                                    })),
                            )
                        }),
                ),
            )
    }

    /// The turn whose row carries Bring back: the last one with an answer, since Bring back sends the latest answer.
    pub(super) fn bring_back_turn(&self) -> Option<usize> {
        if !self.has_answer() {
            return None;
        }
        self.blocks.iter().enumerate().rev().find_map(|(ix, block)| {
            (matches!(block.inner(), Block::Turn { .. })
                && code_view::turn_answer(&self.blocks, ix).is_some_and(|text| !text.trim().is_empty()))
            .then_some(ix)
        })
    }

    /// Bring back, in the latest answer's actions as the transcript's Copy and Ask again: an icon whose tooltip says
    /// where the answer goes, then "Queued in …" once it is sent.
    pub(super) fn bring_back_action(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.brought_back {
            return h_flex()
                .debug_selector(|| "side-brought-back".into())
                .h(px(answer_footer::HOVER_BUTTON))
                .min_w(px(0.))
                .max_w(px(220.))
                .pl(px(4.))
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(p.muted)
                .child(glyph(Lucide::Check, p.faint))
                .child(
                    div()
                        .min_w(px(0.))
                        .truncate()
                        .child(format!("Queued in “{}”", self.parent_title)),
                )
                .into_any_element();
        }
        let enabled = self.can_command() && !self.summary.status.busy();
        let tip = format!("Bring back to “{}”, after its current work", self.parent_title);
        answer_footer::hover_button("side-bring-back", Lucide::CornerUpLeft, tip, p)
            .debug_selector(|| "side-bring-back".into())
            .when(!enabled, |button| button.opacity(0.4).cursor_default())
            .when(enabled, |button| {
                button.on_click(cx.listener(|this, _, window, cx| {
                    if !this.summary.status.busy() && this.has_answer() {
                        let note = format!("Queued in “{}”.", this.parent_title);
                        this.command(
                            "wb_send_back",
                            json!({"threadId":this.summary.id}),
                            false,
                            Some(&note),
                            window,
                            cx,
                        );
                    }
                }))
            })
            .into_any_element()
    }
}
