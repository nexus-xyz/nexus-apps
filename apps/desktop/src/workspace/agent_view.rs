//! A sub-agent's own view: one sub-agent or background command of the open Code session, in the main area in place of
//! the conversation. Breadcrumbs lead back up to the conversation. Under them: its
//! task, its own work (`model::activity`), how it stands and how it ended. The Agents tab stays beside it with this
//! agent marked. Sub-agents take no messages, so the dock's composer writes to the lead. Approvals are answered only in
//! the conversation.
#[path = "agent_view/model.rs"]
mod view_model;
use super::super::code_view::{CodeView, agent_mark, tool_icon};
use super::super::{glyph, transcript};
use super::*;
use crate::palette::Palette;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use view_model::{Dock, Stop, Target};

/// A quiet action in the view's top bar, as the conversation's head actions.
fn head_action(id: impl Into<ElementId>, label: &'static str, p: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_none()
        .gap(px(5.))
        .px(px(8.))
        .py(px(3.))
        .rounded(px(6.))
        .text_size(px(12.5))
        .text_color(p.muted)
        .cursor_pointer()
        .hover(|s| s.bg(p.hover).text_color(p.foreground))
        .child(label)
}

fn card(heading: &'static str, p: Palette) -> Div {
    v_flex()
        .w_full()
        .min_w(px(0.))
        .gap_1()
        .p_3()
        .rounded(px(12.))
        .bg(p.raised)
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.faint)
                .child(heading),
        )
}

fn prose_block(view: &Entity<gpui_kit::component::text::TextViewState>, p: Palette) -> Div {
    div()
        .w_full()
        .min_w(px(0.))
        .text_size(px(14.))
        .line_height(px(22.))
        .child(transcript::prose(
            crate::external_link::text_view(view).selectable(true),
            p,
        ))
}

impl Workspace {
    fn viewed_agent(&self) -> Option<&AgentNode> {
        let state = self.side.agents.as_ref()?;
        state.crew.as_ref()?.resolve(state.selected.as_deref()?)
    }

    /// A message written in a sub-agent's view, as the lead reads it; any other message as written.
    pub(in crate::workspace) fn about_open_agent(&self, prompt: String) -> String {
        match self.viewed_agent() {
            Some(node) if self.agent_view_open() && node.kind == AgentKind::Subagent => {
                let about = view_model::about(&node.title, &prompt);
                if about.is_empty() { prompt } else { about }
            }
            _ => prompt,
        }
    }

    pub(in crate::workspace) fn render_agent_view(
        &self,
        view: &CodeView,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = v_flex()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .bg(p.background)
            .text_color(p.foreground);
        let (Some(state), Some(node)) = (self.side.agents.as_ref(), self.viewed_agent()) else {
            return page
                .child(div().p_6().text_color(p.muted).child("Reading the agent…"))
                .into_any_element();
        };
        let crew = state.crew.as_ref().unwrap();
        let controllable = self.runs(&state.id).is_some();
        let caps: Option<&AgentCapabilities> = state.summary.as_ref().and_then(|s| s.capabilities.as_deref());
        let controls = crew_fold::controls_of(node, caps);
        page.child(self.render_agent_bar(node, crew, &controls, controllable, p, cx))
            .child(
                div()
                    .id(SharedString::from(format!("agent-view-{}", node.id)))
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(self.column_width()))
                            .mx_auto()
                            .px(px(24.))
                            .py(px(20.))
                            .gap_3()
                            .text_size(px(13.))
                            .child(self.render_agent_head(node, &controls, controllable, p, cx))
                            .child(self.render_agent_work(node, crew, p, cx)),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(self.composer_width()))
                    .mx_auto()
                    .px(px(24.))
                    .pb(px(28.))
                    .child(self.render_agent_dock(view, &controls, controllable, p, cx)),
            )
            .into_any_element()
    }

    /// Breadcrumbs back up to the conversation, then Agent map, Show where it started, Background and Stop.
    fn render_agent_bar(
        &self,
        node: &AgentNode,
        crew: &Crew,
        controls: &crew_fold::Controls,
        controllable: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.side.agents.as_ref().unwrap();
        let mut crumbs = h_flex()
            .flex_1()
            .min_w(px(0.))
            .gap_1()
            .overflow_hidden()
            .text_size(px(12.5))
            .text_color(p.muted);
        let path = model::path_to(crew, node);
        for (ix, crumb) in view_model::crumbs(&self.title, &path).into_iter().enumerate() {
            if ix > 0 {
                crumbs = crumbs.child(glyph(Lucide::ChevronRight, p.faint).size(px(12.)));
            }
            let link = div()
                .id(("agent-crumb", ix))
                .min_w(px(0.))
                .truncate()
                .px_1()
                .rounded(px(6.))
                .child(crumb.label);
            crumbs = crumbs.child(match crumb.target {
                Target::Here => link.flex_shrink_0().text_color(p.foreground),
                Target::Conversation => link
                    .cursor_pointer()
                    .hover(|s| s.bg(p.hover))
                    .on_click(cx.listener(|this, _, _, cx| this.close_agent_view(cx))),
                Target::Agent(id) => link
                    .cursor_pointer()
                    .hover(|s| s.bg(p.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_agent(id.clone(), cx))),
            });
        }
        let selected = node.id.clone();
        let mut actions = h_flex()
            .flex_none()
            .gap_1()
            .child(
                head_action("agent-crew-map", "Agent map", p)
                    .on_click(cx.listener(|this, _, window, cx| this.show_crew_map(window, cx))),
            )
            .when(node.call_id.is_some(), |bar| {
                bar.child(
                    head_action("agent-show-start", "Show where it started", p)
                        .on_click(cx.listener(move |this, _, _, cx| this.show_agent_in_conversation(&selected, cx))),
                )
            });
        if let Some(call) = view_model::background(node, controls, controllable) {
            let args = json!({"threadId": state.id, "toolUseId": call});
            actions = actions.child(
                head_action("agent-background", "Background", p)
                    .when(state.busy, |b| b.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.agent_action("wb_task_background", args.clone(), false, cx)
                    })),
            );
        }
        if let Stop::Button(task) = view_model::stop(node, controls, controllable) {
            let args = json!({"threadId": state.id, "taskId": task});
            actions = actions.child(
                head_action("agent-stop", "Stop", p)
                    .text_color(p.destructive)
                    .when(state.busy, |b| b.opacity(0.5))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.agent_action("wb_task_stop", args.clone(), false, cx)),
                    ),
            );
        }
        h_flex()
            .w_full()
            .h(px(44.))
            .flex_none()
            .px(px(16.))
            .gap_3()
            .border_b_1()
            .border_color(p.border)
            .child(crumbs)
            .child(actions)
    }

    /// Its mark, name and facts; why Stop is not offered; a request waiting on you; its task and its output.
    pub(super) fn render_agent_head(
        &self,
        node: &AgentNode,
        controls: &crew_fold::Controls,
        controllable: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.side.agents.as_ref().unwrap();
        let now = crate::code::now_ms();
        let policy = crate::code::policy_of(node.status, node.reason.as_deref());
        let mut head = v_flex().w_full().gap_3().child(
            h_flex()
                .gap_2()
                .items_start()
                .child(div().pt(px(4.)).child(if policy.is_some() {
                    glyph(Lucide::ShieldOff, p.faint).into_any_element()
                } else {
                    agent_mark(node.started_at as usize, node.status, p)
                }))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::MEDIUM)
                                .when(node.kind == AgentKind::Shell, |s| s.font_family(crate::pages::ui::MONO))
                                .child(node.title.clone()),
                        )
                        .child(
                            div()
                                .text_color(p.muted)
                                .child(
                                    [
                                        Some(view_model::facts(node, status(node), now)),
                                        self.crew_project_note(node),
                                    ]
                                    .into_iter()
                                    .flatten()
                                    .collect::<Vec<_>>()
                                    .join(" · "),
                                )
                                .children(view_model::inferred(node).map(|note| div().text_color(p.faint).child(note))),
                        ),
                ),
        );
        if let Some(policy) = policy {
            head = head.child(div().text_color(p.muted).child(if policy.is_empty() {
                "Blocked by policy".to_string()
            } else {
                format!("Blocked by policy: {policy}")
            }));
        }
        if let Stop::Why(reason) = view_model::stop(node, controls, controllable) {
            head = head.child(
                div()
                    .text_color(p.faint)
                    .child(format!("Stop is not offered here: {reason}")),
            );
        }
        for error in [&state.error, &state.action_error].into_iter().flatten() {
            head = head.child(div().text_color(p.destructive).child(error.clone()));
        }
        if node.status == AgentStatus::Needs {
            head = head.child(
                h_flex()
                    .gap_3()
                    .p_3()
                    .rounded(px(12.))
                    .bg(p.raised)
                    .child(agent_mark(node.started_at as usize, AgentStatus::Needs, p))
                    .child(div().flex_1().child("Waiting on you, in the conversation"))
                    .child(
                        head_action("agent-go-to-request", "Go to the request", p)
                            .text_color(p.foreground)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_agent_view(cx);
                                this.focus_composer(window, cx);
                            })),
                    ),
            );
        }
        if let Some(task) = view_model::task(&state.events, node) {
            head = head.child(card("Its task", p).child(task));
        }
        if node.kind == AgentKind::Shell
            && let Some(output) = &state.output
        {
            let heading = if node.status.is_live() {
                "Its output, live"
            } else {
                "Its last output"
            };
            head = head.child(
                card(heading, p).child(
                    div()
                        .font_family(crate::pages::ui::MONO)
                        .text_size(px(12.))
                        .child(output.clone()),
                ),
            );
        }
        head.into_any_element()
    }

    /// Its own work: text as Markdown, tool calls with their literal output, its own sub-agents as rows that open them.
    pub(super) fn render_agent_work(
        &self,
        node: &AgentNode,
        crew: &Crew,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.side.agents.as_ref().unwrap();
        let mut body = v_flex().w_full().min_w(px(0.)).gap_3();
        if state.detail.len() > state.detail_limit {
            body = body.child(
                head_action("earlier-agent-activity", "Show earlier activity", p).on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Some(s) = &mut this.side.agents {
                            s.detail_limit += 200;
                            s.refresh_detail(cx);
                        }
                        cx.notify();
                    },
                )),
            );
        }
        // A command's work is its output, above.
        if state.detail.is_empty() && state.live.is_none() && node.kind != AgentKind::Shell {
            body = body.child(div().text_color(p.muted).child(if node.status.is_live() {
                "No steps yet."
            } else {
                "No steps recorded."
            }));
            if !node.recent.is_empty() {
                body = body.child(
                    div()
                        .text_color(p.faint)
                        .child("Its own steps are not in the session's log. What it reported doing, latest last:"),
                );
            }
            for step in &node.recent {
                body = body.child(
                    div()
                        .font_family(crate::pages::ui::MONO)
                        .text_size(px(12.))
                        .text_color(p.muted)
                        .child(step.text.clone()),
                );
            }
        }
        for row in state
            .detail
            .iter()
            .skip(state.detail.len().saturating_sub(state.detail_limit))
        {
            let child = row
                .call
                .as_deref()
                .and_then(|call| crew.node_of_call(call))
                .filter(|child| child.id != node.id);
            if let Some(child) = child {
                body = body.child(self.render_child_agent(child, p, cx));
            } else if let Some(title) = &row.title {
                body = body.child(self.render_agent_tool(row, title, p, cx));
            } else if let Some((_, text)) = state.markdown.get(&row.seq) {
                body = body.child(prose_block(text, p));
            }
        }
        if let Some((_, live)) = &state.live {
            body = body.child(prose_block(live, p));
        }
        if node.status.is_live() && node.status != AgentStatus::Needs {
            body = body.child(
                h_flex()
                    .gap_2()
                    .text_color(p.muted)
                    .child(agent_mark(node.started_at as usize, AgentStatus::Running, p))
                    .child("Working")
                    .child(
                        div()
                            .font_family(crate::pages::ui::MONO)
                            .text_size(px(12.))
                            .text_color(p.faint)
                            .child(view_model::ran(node, crate::code::now_ms())),
                    ),
            );
        }
        if let Some((heading, text)) = view_model::ending(node) {
            body = body.child(card(heading, p).child(text));
        }
        body.into_any_element()
    }

    /// One of its own sub-agents: a row that opens that agent's view.
    fn render_child_agent(&self, child: &AgentNode, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = child.id.clone();
        h_flex()
            .id(SharedString::from(format!("agent-child-{id}")))
            .w_full()
            .min_w(px(0.))
            .gap_2()
            .px_3()
            .py_2()
            .rounded(px(12.))
            .bg(p.raised)
            .cursor_pointer()
            .hover(|s| s.bg(p.hover))
            .child(agent_mark(child.started_at as usize, child.status, p))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(child.title.clone()),
            )
            .child(div().flex_none().text_color(p.muted).child(status(child)))
            .child(div().flex_none().text_color(p.muted).child("Open"))
            .child(glyph(Lucide::ChevronRight, p.faint))
            .on_click(cx.listener(move |this, _, _, cx| this.select_agent(id.clone(), cx)))
            .into_any_element()
    }

    /// A tool call: its first line, opening to its literal input and output.
    fn render_agent_tool(&self, row: &model::Activity, title: &str, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let state = self.side.agents.as_ref().unwrap();
        let seq = row.seq;
        let open = state.open_tools.contains(&seq);
        let mut tool = v_flex()
            .w_full()
            .min_w(px(0.))
            .gap_1()
            .p_3()
            .rounded(px(12.))
            .bg(p.raised)
            .child(
                h_flex()
                    .id(("agent-tool", seq as usize))
                    .gap_2()
                    .cursor_pointer()
                    .child(glyph(
                        if open {
                            Lucide::ChevronDown
                        } else {
                            Lucide::ChevronRight
                        },
                        p.icon,
                    ))
                    .child(glyph(tool_icon(title), p.icon))
                    .child(title.to_string())
                    .when(row.error, |s| s.child(glyph(Lucide::X, p.destructive)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(s) = &mut this.side.agents
                            && !s.open_tools.remove(&seq)
                        {
                            s.open_tools.insert(seq);
                        }
                        cx.notify();
                    })),
            );
        if open {
            if let Some((_, text)) = state.markdown.get(&seq) {
                tool = tool.child(transcript::prose(
                    crate::external_link::text_view(text).selectable(true),
                    p,
                ));
            }
        } else {
            tool = tool.child(
                div()
                    .font_family(crate::pages::ui::MONO)
                    .text_size(px(12.))
                    .truncate()
                    .text_color(if row.error { p.destructive } else { p.muted })
                    .child(row.preview.clone()),
            );
        }
        tool.into_any_element()
    }

    /// Under the view: where a message goes. A sub-agent's goes to the lead, through the session's own composer.
    fn render_agent_dock(
        &self,
        view: &CodeView,
        controls: &crew_fold::Controls,
        controllable: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let note = |text: SharedString| {
            div()
                .px(px(4.))
                .pb(px(6.))
                .text_size(px(12.))
                .text_color(p.faint)
                .child(text)
        };
        match view_model::dock(controls, controllable && self.runner.is_some()) {
            Dock::Lead => {
                let composer = v_flex()
                    .w_full()
                    .child(note(view_model::LEAD_NOTE.into()))
                    .child(self.render_code_composer(view, p, cx))
                    .into_any_element();
                self.render_owned_dock(false, composer, p, cx)
            }
            Dock::Note(text) => note(text.into()).into_any_element(),
            // Dimmed under who runs the session, with Take over (`read_only.rs`).
            Dock::ReadOnly => {
                let composer = self.render_code_composer(view, p, cx).into_any_element();
                self.render_owned_dock(true, composer, p, cx)
            }
        }
    }
}
