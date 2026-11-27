//! The Agents tab's list: the header, the lead and the tree under it on
//! hairline branches, the commands left running, and the session's total with its split by route.
use super::super::code_dock::text_button;
use super::super::glyph;
use super::*;
use crate::crew_route::route_of;
use crate::palette::Palette;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use std::collections::HashSet;

/// Why a status may be a guess (`agents-section.tsx` `INFERRED`).
const INFERRED: &str = "Read from the session's tool calls: the agent did not report it.";
/// Below this pane width a row's meta (cost or time) drops; the status word stays (board L3).
pub(super) const NARROW: f32 = 300.;
/// The tree's hairline: from a row's left edge to its mark, and the tick's height (`.wb-crew-children`).
const BRANCH_INDENT: f32 = 14.;
const TICK_TOP: f32 = 16.;

/// Numbers that line up (`font-variant-numeric: tabular-nums`).
fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

fn usd(value: f64) -> String {
    format!("${value:.2}")
}

/// A node's state as a mark in a 14px box (Nexus UI `status-mark.tsx`): shape tells the state, hue tells the route.
pub(super) fn crew_mark(node: &AgentNode, route: usize, p: Palette) -> AnyElement {
    let hue = Palette::route(route);
    let dot = |color: Hsla| div().size(px(8.)).rounded_full().bg(color);
    let ring = |color: Hsla| div().size(px(9.)).rounded_full().border(px(1.5)).border_color(color);
    let mark = match status(node) {
        "Working" => dot(hue),
        "Needs you" => ring(hue),
        "Done" => dot(Palette::success()),
        "Failed" => dot(p.destructive),
        // Stopped by a person: half a dot, in no route's hue.
        "Stopped" => ring(p.muted).child(div().w(px(3.)).h_full().rounded_l_full().bg(p.muted)),
        // Blocked by policy: a hollow dashed steel ring, never red.
        _ => ring(p.steel()).border_dashed(),
    };
    div()
        .size(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(mark)
        .into_any_element()
}

impl Workspace {
    pub(super) fn render_crew_list(
        &self,
        mut body: Stateful<Div>,
        crew: &Crew,
        state: &AgentsTool,
        p: Palette,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let summary = model::summary(crew);
        body = body.child(
            h_flex()
                .w_full()
                .min_h(px(28.))
                .flex_wrap()
                .justify_end()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.faint)
                        .child(format!(
                            "Background tasks  {}",
                            crew.nodes.iter().filter(|node| node.kind != AgentKind::Shell).count()
                        )),
                )
                .when(!summary.is_empty(), |head| {
                    head.child(
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(12.))
                            .font_features(tabular())
                            .text_color(p.muted)
                            .child(summary),
                    )
                }),
        );
        if let Some(strip) = self.render_mini_map(crew, state, p, width, cx) {
            body = body.child(strip);
        }
        body = body.child(
            div()
                .text_size(px(12.))
                .text_color(p.faint)
                .child("↑ ↓ choose · ← → fold · Enter open · Space peek · S stop command"),
        );
        let rows = model::folded_rows(crew, state.show_finished, state.cursor.as_deref(), &state.collapsed);
        let visible: HashSet<&str> = rows.agents.iter().map(|node| node.id.as_str()).collect();
        let narrow = width < NARROW;
        let top: Vec<&AgentNode> = rows
            .agents
            .iter()
            .copied()
            .filter(|node| node.kind != AgentKind::Lead && crew.children_of(crew.lead()).any(|top| top.id == node.id))
            .collect();
        let mut tree = v_flex()
            .w_full()
            .child(self.crew_row(crew, crew.lead(), state, p, narrow, cx));
        if !top.is_empty() {
            tree = tree.child(self.crew_branch(crew, &top, &visible, state, p, narrow, cx));
        }
        body = body.child(tree);
        if rows.hidden > 0 || state.show_finished {
            let hidden = rows.hidden;
            body = body.child(
                div()
                    .id("show-finished-agents")
                    .ml(px(18.))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(8.))
                    .text_size(px(12.))
                    .text_color(p.muted)
                    .cursor_pointer()
                    .hover(|s| s.bg(p.hover).text_color(p.foreground))
                    .child(if state.show_finished {
                        "Show fewer finished".into()
                    } else {
                        format!("{hidden} more finished")
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(s) = &mut this.side.agents {
                            s.show_finished = !s.show_finished;
                        }
                        cx.notify();
                    })),
            );
        }
        if crew.counts.total == 0 {
            body = body.child(
                div()
                    .text_color(p.muted)
                    .child("No sub-agents yet. When the lead hands work to one, it shows here."),
            );
        }
        if !rows.commands.is_empty() {
            body = body.child(
                div()
                    .mt_2()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.faint)
                    .child("Left running"),
            );
            for node in rows.commands {
                body = body.child(self.command_row(node, state, p, cx));
            }
        }
        body.child(self.session_total(crew, state, p))
    }

    /// Children on a hairline branch: a 1px line down their left, a short tick to each row.
    #[allow(clippy::too_many_arguments)]
    fn crew_branch(
        &self,
        crew: &Crew,
        children: &[&AgentNode],
        visible: &HashSet<&str>,
        state: &AgentsTool,
        p: Palette,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut branch = v_flex().relative().w_full().ml(px(4.)).pl(px(BRANCH_INDENT)).child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom(px(BRANCH_INDENT))
                .w(px(1.))
                .bg(p.border),
        );
        for &node in children {
            let grandchildren: Vec<&AgentNode> = crew
                .children_of(node)
                .filter(|child| visible.contains(child.id.as_str()))
                .collect();
            let mut item = v_flex()
                .relative()
                .w_full()
                .child(
                    div()
                        .absolute()
                        .top(px(TICK_TOP))
                        .left(px(-BRANCH_INDENT))
                        .w(px(9.))
                        .h(px(1.))
                        .bg(p.border),
                )
                .child(if node.kind == AgentKind::Shell {
                    self.command_row(node, state, p, cx)
                } else {
                    self.crew_row(crew, node, state, p, narrow, cx)
                });
            if !grandchildren.is_empty() {
                item = item.child(self.crew_branch(crew, &grandchildren, visible, state, p, narrow, cx));
            }
            let item = if !cx.reduce_motion() && state.arriving.contains_key(&node.id) {
                item.overflow_hidden()
                    .with_animation(
                        SharedString::from(format!("crew-arrive-{}-{}", state.epoch, node.id)),
                        Animation::new(crate::motion::UI_LONG).with_easing(crate::motion::ease_out),
                        |item, progress| item.max_h(px(2000. * progress)).opacity(progress),
                    )
                    .into_any_element()
            } else {
                item.into_any_element()
            };
            branch = branch.child(item);
        }
        branch.into_any_element()
    }

    /// An agent's row: mark, name and status word; what it does and its meta under them. Needs you: the strong fill
    /// and a way to the request. Space: its last three steps under it.
    fn crew_row(
        &self,
        crew: &Crew,
        node: &AgentNode,
        state: &AgentsTool,
        p: Palette,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = node.id.clone();
        let lead = node.kind == AgentKind::Lead;
        let live = node.status.is_live();
        let needs = node.status == AgentStatus::Needs;
        let route = route_of(crew, &node.id);
        let policy = crate::code::policy_of(node.status, node.reason.as_deref());
        let line = if let Some(policy) = policy {
            Some(if policy.is_empty() {
                "Blocked by policy".to_string()
            } else {
                format!("Blocked by policy: {policy}")
            })
        } else if needs && !lead {
            Some("Waiting on you, in the conversation".into())
        } else if live {
            node.last.as_ref().map(|s| s.text.clone())
        } else if lead {
            None
        } else {
            node.summary.clone().or(node.last.as_ref().map(|s| s.text.clone()))
        };
        let meta = (!lead)
            .then(|| match node.cost_usd {
                Some(cost) => Some(usd(cost)),
                None if live => Some(crew_fold::elapsed(
                    (node.ended_at.unwrap_or(crate::code::now_ms()) - node.started_at).max(0),
                )),
                None if node.steps > 0 => Some(format!(
                    "{} {}",
                    node.steps,
                    if node.steps == 1 { "step" } else { "steps" }
                )),
                None => None,
            })
            .flatten()
            .filter(|_| !narrow);
        let name: AnyElement = if lead {
            let model = node
                .model
                .clone()
                .or_else(|| state.summary.as_ref().and_then(|s| s.model.clone()))
                .map(|m| crate::code::model_label(&m))
                .unwrap_or_else(|| crew_fold::agent_name(crew.provider).to_string());
            h_flex()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .child(div().flex_none().text_color(p.strong).child("Lead"))
                .child(
                    div()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(p.faint)
                        .child(format!(" · {model}")),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(p.strong)
                .child(if node.kind == AgentKind::Linked {
                    format!("{} · {}", node.title, crew_fold::agent_name(node.provider))
                } else {
                    node.title.clone()
                })
                .into_any_element()
        };
        let inferred = node.status_source == StatusSource::Inferred;
        let word = div()
            .id(SharedString::from(format!("crew-word-{id}")))
            .flex_none()
            .text_size(px(12.))
            .text_color(if needs { p.foreground } else { p.muted })
            .child(status(node))
            // GPUI draws no dotted underline: a dashed bottom edge stands in for it.
            .when(inferred, |word| {
                word.border_b_1()
                    .border_dashed()
                    .border_color(p.faint)
                    .tooltip(|window, cx| Tooltip::new(INFERRED).build(window, cx))
            });
        let cursor = state.cursor.as_deref() == Some(node.id.as_str());
        let peek = state.peek.as_deref() == Some(node.id.as_str());
        let mut row = v_flex()
            .id(SharedString::from(format!("crew-{id}")))
            .w_full()
            .min_w(px(0.))
            .p(px(8.))
            .gap(px(2.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|s| s.bg(p.hover))
            .when(cursor, |s| s.bg(p.selected))
            .when(needs, |s| s.rounded(px(12.)).bg(p.selected))
            .when(needs && cursor, |s| s.border_1().border_color(p.border))
            .child(
                h_flex()
                    .w_full()
                    .min_w(px(0.))
                    .gap(px(10.))
                    .text_size(px(13.))
                    .child(crew_mark(node, route, p))
                    .child(name)
                    .child(word)
                    .child(glyph(Lucide::ChevronRight, p.faint).size(px(14.))),
            );
        if line.is_some() || meta.is_some() {
            row = row.child(
                h_flex()
                    .w_full()
                    .min_w(px(0.))
                    .pl(px(24.))
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(p.faint)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .child(crew_fold::first_line(line.as_deref().unwrap_or(""), 180)),
                    )
                    .children(meta.map(|meta| div().flex_none().font_features(tabular()).child(meta))),
            );
        }
        if needs {
            let linked = (node.kind == AgentKind::Linked).then(|| node.id.clone());
            row = row.child(
                h_flex().pl(px(20.)).pt(px(4.)).child(
                    text_button(
                        SharedString::from(format!("crew-answer-{id}")),
                        "Answer in conversation",
                        true,
                        p,
                    )
                    .bg(p.background)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if let Some(id) = &linked {
                            this.select_agent(id.clone(), cx);
                        } else {
                            this.answer_in_conversation(window, cx);
                        }
                    })),
                ),
            );
        }
        if peek {
            let steps = model::peek(&state.events, node);
            row = row.child(
                v_flex()
                    .ml(px(24.))
                    .mt(px(4.))
                    .p(px(8.))
                    .gap(px(2.))
                    .rounded(px(8.))
                    .bg(p.background)
                    .font_family(crate::pages::ui::MONO)
                    .text_size(px(11.5))
                    .text_color(p.muted)
                    .when(steps.is_empty(), |b| b.child("No steps yet."))
                    .children(steps.into_iter().map(|step| div().truncate().child(step))),
            );
        }
        row.on_click(cx.listener(move |this, _, window, cx| {
            if let Some(state) = &this.side.agents {
                state.focus.focus(window, cx);
            }
            this.open_agent_detail(id.clone(), cx);
        }))
        .into_any_element()
    }

    /// A command left running: its command line, the port it serves and how long it ran, Stop, and Open in Browser
    /// when it printed a port.
    fn command_row(&self, node: &AgentNode, state: &AgentsTool, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = node.id.clone();
        let live = node.status.is_live();
        let port = model::port_of(node);
        let facts = [
            port.as_ref().map(|port| format!(":{port}")),
            Some(if live {
                format!(
                    "up {}",
                    crew_fold::elapsed((crate::code::now_ms() - node.started_at).max(0))
                )
            } else {
                status(node).to_string()
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let caps: Option<&AgentCapabilities> = state.summary.as_ref().and_then(|s| s.capabilities.as_deref());
        let stop = (crew_fold::controls_of(node, caps).stop.available && self.runs(&state.id).is_some())
            .then(|| node.task_id.clone())
            .flatten()
            .filter(|_| !state.busy);
        let thread = state.id.clone();
        let cursor = state.cursor.as_deref() == Some(node.id.as_str());
        h_flex()
            .id(SharedString::from(format!("crew-{id}")))
            .w_full()
            .min_w(px(0.))
            .px(px(8.))
            .py(px(6.))
            .gap(px(10.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|s| s.bg(p.hover))
            .when(cursor, |s| s.bg(p.selected))
            .child(
                div()
                    .size(px(14.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(glyph(Lucide::SquareTerminal, p.muted).size(px(13.))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_family(crate::pages::ui::MONO)
                    .text_size(px(12.))
                    .text_color(p.strong)
                    .child(node.title.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .font_features(tabular())
                    .text_color(p.muted)
                    .child(facts),
            )
            .when_some(port.filter(|_| live), |row, port| {
                let url = format!("http://localhost:{port}");
                row.child(
                    text_button(
                        SharedString::from(format!("crew-browse-{id}")),
                        "Open in Browser",
                        false,
                        p,
                    )
                    .tooltip({
                        let url = url.clone();
                        move |window, cx| Tooltip::new(format!("Open {url} in the Browser")).build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.show_in_side_browser(&url, window, cx);
                    })),
                )
            })
            .when_some(stop, |row, task| {
                let title = node.title.clone();
                row.child(
                    div()
                        .id(SharedString::from(format!("crew-stop-{id}")))
                        .flex_none()
                        .size(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(p.hover))
                        .child(div().size(px(8.)).rounded(px(1.5)).bg(p.muted))
                        .tooltip(move |window, cx| Tooltip::new(format!("Stop {title}")).build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.agent_action("wb_task_stop", json!({"threadId": thread, "taskId": task}), false, cx);
                        })),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(state) = &this.side.agents {
                    state.focus.focus(window, cx);
                }
                this.open_agent_detail(id.clone(), cx);
            }))
            .into_any_element()
    }

    /// The session's total, and a 4px bar under it split by who spent it, in route hues.
    fn session_total(&self, crew: &Crew, state: &AgentsTool, p: Palette) -> AnyElement {
        let total = state.summary.as_ref().map_or(0., |s| s.cost_usd);
        let shares = model::cost_shares(crew, total);
        let mut block = v_flex()
            .w_full()
            .mt_2()
            .pt_3()
            .gap(px(6.))
            .border_t_1()
            .border_color(p.border)
            .text_size(px(12.))
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .child(div().text_color(p.muted).child("Session total"))
                    // Nothing priced yet reads $0.00, as the top bar does.
                    .child(
                        div()
                            .font_features(tabular())
                            .text_color(p.foreground)
                            .child(usd(total)),
                    ),
            );
        if total > 0. && !shares.is_empty() {
            let named = shares
                .iter()
                .map(|share| format!("{} {}", share.label, usd(share.usd)))
                .collect::<Vec<_>>()
                .join(" · ");
            let mut bar = h_flex()
                .id("crew-cost-bar")
                .w_full()
                .h(px(4.))
                .gap(px(1.))
                .rounded_full()
                .overflow_hidden()
                .tooltip(move |window, cx| Tooltip::new(named.clone()).build(window, cx));
            for share in &shares {
                bar = bar.child(
                    div()
                        .h_full()
                        .w(relative((share.usd / total) as f32))
                        .bg(share.route.map_or(p.faint, Palette::route)),
                );
            }
            block = block.child(bar);
        }
        block.into_any_element()
    }

    /// How many sub-agents are live, when any are: the count the Agents tab shows.
    pub(in crate::workspace) fn agents_live_count(&self) -> Option<usize> {
        Some(model::live_count(self.crew()?)).filter(|count| *count > 0)
    }

    /// The Agents tab's label extras: how many sub-agents are live, and a ring while one needs you, in the waiting
    /// agent's route hue (amber when the lead itself asks). Shown while another tab is chosen too.
    pub(in crate::workspace) fn agents_tab_badge(&self, p: Palette) -> Option<AnyElement> {
        let crew = self.crew()?;
        let count = model::live_count(crew);
        let ring = model::waiting_hue(crew);
        if count == 0 && ring.is_none() {
            return None;
        }
        Some(
            div()
                .debug_selector(|| "side-tab-agents-count".into())
                .flex_none()
                .min_w(px(16.))
                .h(px(16.))
                .px(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(p.hover)
                .when_some(ring, |badge, hue| badge.border(px(1.5)).border_color(hue))
                .font_features(tabular())
                .text_size(px(10.5))
                .text_color(p.foreground)
                .child(count.to_string())
                .into_any_element(),
        )
    }
}
