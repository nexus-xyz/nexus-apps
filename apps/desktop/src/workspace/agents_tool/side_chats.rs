//! The Agents tab's "Side chats": the session's side chats as the sidebar's session rows (`thread_row`), each with its
//! state mark and age, and the one way to start a new one, the "+" at the end of the group's head.
use super::super::sidebar::{hint, small_button, state_word, thread_row};
use super::*;
use crate::code::{self, Status};
use crate::palette::Palette;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;

/// Side chats shown before "All side chats", as the sidebar's latest sessions.
const RECENT: usize = 8;

impl Workspace {
    pub(super) fn render_side_chats(&self, state: &AgentsTool, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let sides = &state.side_chats;
        let can_open = !self.side.side_chat_busy && self.runs(&state.id).is_some();
        let more = sides.len() > RECENT;
        let shown = if state.show_all_side_chats || !more {
            sides.len()
        } else {
            RECENT
        };
        let open = match &self.view {
            View::Code(view) if self.side.side_chat_visible => self.side.side_chat_for.get(&view.id).cloned(),
            _ => None,
        };
        let now = code::now_ms();
        let mut list = v_flex()
            .id("side-chats")
            .w_full()
            .child(self.side_chats_head(sides.len(), can_open, p, cx));
        if sides.is_empty() {
            list = list.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .text_color(p.faint)
                    .child("Ask about this session without adding to it."),
            );
        }
        for side in sides.iter().take(shown) {
            let retitled = state.side_chat_retitled.get(&side.id).copied();
            let open = open.as_deref() == Some(side.id.as_str());
            list = list.child(self.side_chat_row(side, open, retitled, now, p, cx));
        }
        if more {
            list = list.child(
                h_flex()
                    .id("all-side-chats")
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(8.))
                    .text_xs()
                    .text_color(p.faint)
                    .cursor_pointer()
                    .hover(|style| style.text_color(p.foreground))
                    .child(if state.show_all_side_chats {
                        "Fewer side chats"
                    } else {
                        "All side chats"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(state) = &mut this.side.agents {
                            state.show_all_side_chats = !state.show_all_side_chats;
                        }
                        cx.notify();
                    })),
            );
        }
        list.into_any_element()
    }

    /// "Side chats", its count, and a 22px "+" that opens a new one. While one opens, or while the session is read-only,
    /// the "+" dims and takes no click.
    fn side_chats_head(&self, count: usize, can_open: bool, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let tip = if self.side.side_chat_busy {
            "Opening side chat…"
        } else {
            "New side chat: ask about this session without adding to it"
        };
        let add = small_button("new-side-chat", Lucide::Plus, p)
            .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
            .when(!can_open, |button| button.opacity(0.4).cursor_default())
            .when(can_open, |button| {
                button.on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.create_side_chat(None, window, cx);
                }))
            });
        h_flex()
            .h(px(28.))
            .mt_2()
            .pl(px(8.))
            .pr(px(4.))
            .gap(px(8.))
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(p.faint)
            .child("Side chats")
            .when(count > 0, |head| {
                head.child(
                    div()
                        .font_family("monospace")
                        .text_size(px(11.))
                        .child(count.to_string()),
                )
            })
            .child(div().flex_1())
            .child(add)
    }

    /// One side chat: its state mark, its title, and its age, or what it is doing while it works or waits on the person.
    /// A secret its agent waits for needs the person. A title that just
    /// changed (`retitled`) rises into place.
    fn side_chat_row(
        &self,
        side: &ThreadSummary,
        open: bool,
        retitled: Option<std::time::Instant>,
        now: i64,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let secret = self
            .runner
            .as_ref()
            .is_some_and(|runner| runner.secret_request(&side.id).is_some());
        let mark = if secret {
            Status::NeedsYou
        } else {
            Status::of(side.status)
        };
        let tail = state_word(mark, p)
            .unwrap_or_else(|| hint(code::age(side.listed_at.unwrap_or(side.updated_at), now), p).into_any_element());
        let clicked = side.clone();
        thread_row(
            SharedString::from(format!("open-side-{}", side.id)),
            side.title.clone().into(),
            None,
            None,
            Some((side.id.as_str(), mark)),
            open,
            false,
            true,
            retitled,
            p,
        )
        .child(tail)
        .on_click(cx.listener(move |this, _, window, cx| this.show_side_chat(clicked.clone(), window, cx)))
    }
}
