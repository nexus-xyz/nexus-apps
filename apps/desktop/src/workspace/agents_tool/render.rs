use super::*;
use crate::palette::Palette;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;

impl Workspace {
    pub(in crate::workspace) fn render_agents_tool(
        &self,
        p: Palette,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.side.agents.as_ref().is_some_and(|s| s.map.is_some()) {
            return self.render_crew_map(p, cx);
        }
        let Some(state) = &self.side.agents else {
            return div()
                .size_full()
                .p_4()
                .text_size(px(13.))
                .text_color(p.foreground)
                .child(super::loading::crew_loading(p, cx.reduce_motion()))
                .into_any_element();
        };
        if state.detail_in_pane {
            return self.render_agent_panel(p, cx);
        }
        let mut body = v_flex()
            .id("agents-tool")
            .size_full()
            .track_focus(&state.focus)
            .tab_index(0)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if let Some(state) = &this.side.agents {
                        state.focus.focus(window, cx);
                    }
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.side.agents.as_ref().is_some_and(|s| s.focus.is_focused(window))
                    && this.agent_key(&event.keystroke.key, cx)
                {
                    cx.stop_propagation();
                }
            }))
            .min_w(px(0.))
            .overflow_y_scroll()
            .track_scroll(&state.overview_scroll)
            .p_4()
            .gap_3()
            .text_size(px(13.))
            .text_color(p.foreground);
        if let Some(error) = &state.error {
            body = body.child(div().text_color(p.destructive).child(error.clone()));
        }
        if let Some(error) = &state.action_error {
            body = body.child(div().text_color(p.destructive).child(error.clone()));
        }
        if self.runs(&state.id).is_none() {
            body = body.child("Read-only while the Nexus app owns the store. Take over to control agents here.");
        }
        if let Some(error) = &self.side.side_chat_error {
            body = body.child(div().text_color(p.destructive).child(error.clone()));
        }

        let Some(crew) = &state.crew else {
            return body
                .child(super::loading::crew_loading(p, cx.reduce_motion()))
                .children(self.render_pull_section(p, cx))
                .into_any_element();
        };
        // No plan and no agents yet: the tools, as the Nexus app's `NothingOpen`.
        body = if crew.counts.total == 0 && !state.has_plan {
            body.child(self.render_nothing_open(p, cx))
        } else {
            self.render_crew_list(body, crew, state, p, width, cx)
        };
        body = body.child(self.render_side_chats(state, p, cx));
        body = self.render_agent_facts(body, p, cx);
        // The session's pull request, last, as the Nexus app's Overview has it (`pull_card.rs`).
        body.children(self.render_pull_section(p, cx)).into_any_element()
    }
}
