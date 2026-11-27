//! The agent's activity inside the Agents panel. Back restores the overview's scroll and cursor.
use super::*;
use crate::pages::ui;
use crate::palette::Palette;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;

impl Workspace {
    pub(super) fn render_agent_panel(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let state = self.side.agents.as_ref().unwrap();
        let node = state
            .selected
            .as_deref()
            .and_then(|id| state.crew.as_ref()?.resolve(id));
        let mut page = v_flex()
            .id("agent-panel")
            .size_full()
            .min_w(px(0.))
            .track_focus(&state.focus)
            .text_size(px(13.))
            .text_color(p.foreground)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && !event.keystroke.modifiers.modified() {
                    this.back_to_agents(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                h_flex().h(px(48.)).flex_none().px_3().child(
                    Button::new("agent-panel-back")
                        .ghost()
                        .icon(Lucide::ArrowLeft)
                        .label("Background tasks")
                        .tooltip("Back to Background tasks (Esc)")
                        .h(px(32.))
                        .on_click(cx.listener(|this, _, window, cx| this.back_to_agents(window, cx))),
                ),
            );
        let Some(node) = node else {
            return page
                .child(div().p_4().text_color(p.muted).child("Agent is no longer available."))
                .into_any_element();
        };
        let crew = state.crew.as_ref().unwrap();
        let caps = state.summary.as_ref().and_then(|s| s.capabilities.as_deref());
        let controls = crew_fold::controls_of(node, caps);
        let controllable = self.runs(&state.id).is_some();
        let content = v_flex()
            .id(SharedString::from(format!("agent-panel-activity-{}", node.id)))
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .overflow_y_scroll()
            .px_4()
            .pb_4()
            .gap_3()
            .child(self.render_agent_head(node, &controls, controllable, p, cx))
            .child(self.render_agent_work(node, crew, p, cx));
        page = page.child(content).child(
            h_flex()
                .flex_none()
                .flex_wrap()
                .gap_2()
                .p_4()
                .border_t_1()
                .border_color(p.border)
                .child(
                    ui::primary_button("agent-panel-conversation", "Open conversation", p).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.open_agent_conversation(window, cx);
                        },
                    )),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(p.faint)
                        .child("Esc Back to Background tasks"),
                ),
        );
        page.into_any_element()
    }
}
