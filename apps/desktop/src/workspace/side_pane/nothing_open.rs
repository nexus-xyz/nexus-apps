//! The Workbench with nothing open yet, as the Nexus web app's `NothingOpen`: in the
//! Background tasks tab of a session with no plan and no agents, a title, one sentence, and the six tools with their keys. A
//! tool this screen cannot open dims and takes no click. A click opens the tool, as its key does. The plan is
//! `nexus_workbench::plan_steps`, folded off the UI thread in `agents_tool.rs`.

use super::*;
impl Workspace {
    pub(in crate::workspace) fn render_nothing_open(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let tools: Vec<_> = Tool::ALL
            .into_iter()
            .map(|tool| self.render_nothing_open_tool(tool, p, cx))
            .collect();
        v_flex()
            .id("nothing-open")
            .debug_selector(|| "nothing-open".into())
            .w_full()
            .child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .pl(px(8.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.faint)
                    .child("Nothing open"),
            )
            .child(
                div()
                    .px(px(8.))
                    .pb(px(6.))
                    .text_size(px(12.))
                    .text_color(p.faint)
                    .child("Tools open here when the agent uses them, or when you pick one."),
            )
            .children(tools)
            .into_any_element()
    }

    /// One tool: its icon, its name and its key, as a row of the "+" menu.
    fn render_nothing_open_tool(&self, tool: Tool, p: Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let offered = self.can_pick_tool(tool);
        h_flex()
            .id(("nothing-open-tool", tool as usize))
            .debug_selector(move || format!("nothing-open-tool-{}", tool as usize))
            .h(px(32.))
            .pl(px(8.))
            .pr(px(6.))
            .gap(px(8.))
            .rounded(px(6.))
            .text_size(px(13.))
            .text_color(if offered { p.foreground } else { p.faint })
            .when(offered, |row| {
                row.cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick_tool(tool, window, cx)))
            })
            .child(
                Icon::new(tool.icon())
                    .size(px(15.))
                    .text_color(if offered { p.icon } else { p.faint }),
            )
            .child(div().flex_1().child(tool.label()))
            .when_some(crate::key_bindings::keystroke(tool.shortcut_id()), |row, keys| {
                row.child(key_hint(&keys, p))
            })
    }
}
