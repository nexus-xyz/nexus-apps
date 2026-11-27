//! The crew canvas: the shared tree with native scrolling, zoom, filtering and keyboard selection.
#[path = "crew_map/model.rs"]
pub(super) mod model;
use super::*;
use crate::palette::Palette;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashSet;

pub(super) struct MapState {
    input: Entity<InputState>,
    query: String,
    open: HashSet<String>,
    zoom: f32,
    selected: String,
    focus: FocusHandle,
    scroll: ScrollHandle,
    drag: Option<Point<Pixels>>,
    _subscription: Subscription,
}

impl Workspace {
    pub(in crate::workspace) fn show_crew_map(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &mut self.side.agents else {
            return;
        };
        if state.map.is_none() {
            let epoch = state.epoch;
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find an agent, role or model"));
            let subscription = cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event
                    && let Some(map) = this.side.agents.as_mut().and_then(|s| s.map.as_mut())
                {
                    map.query = input.read(cx).value().to_string();
                    cx.notify();
                }
            });
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            state.map = Some(MapState {
                input,
                query: String::new(),
                open: HashSet::new(),
                zoom: 1.,
                selected: state.selected.clone().unwrap_or_else(|| "lead".into()),
                focus,
                scroll: ScrollHandle::new(),
                drag: None,
                _subscription: subscription,
            });
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let active = this
                        .update(cx, |this, cx| {
                            let active = this.side.open
                                && this
                                    .side
                                    .agents
                                    .as_ref()
                                    .is_some_and(|state| state.epoch == epoch && state.map.is_some());
                            if active {
                                cx.notify();
                            }
                            active
                        })
                        .unwrap_or(false);
                    if !active {
                        break;
                    }
                }
            })
            .detach();
        }
        self.side.maximized = true;
        cx.notify();
    }

    pub(in crate::workspace) fn crew_map_focused(&self, window: &Window, cx: &App) -> bool {
        self.side
            .agents
            .as_ref()
            .and_then(|s| s.map.as_ref())
            .is_some_and(|m| m.focus.contains_focused(window, cx))
    }

    fn map_zoom(&mut self, amount: f32, cx: &mut Context<Self>) {
        if let Some(map) = self.side.agents.as_mut().and_then(|s| s.map.as_mut()) {
            map.zoom = model::zoom(map.zoom + amount);
        }
        cx.notify();
    }

    fn map_close(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.side.agents {
            state.map = None;
        }
        self.side.maximized = false;
        cx.notify();
    }

    fn map_toggle(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(map) = self.side.agents.as_mut().and_then(|s| s.map.as_mut())
            && !map.open.remove(&id)
        {
            map.open.insert(id);
        }
        cx.notify();
    }

    pub(in crate::workspace) fn render_crew_map(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = &self.side.agents else {
            return div().into_any_element();
        };
        let (Some(map), Some(crew)) = (&state.map, &state.crew) else {
            return div()
                .p_4()
                .text_size(px(13.))
                .child(super::loading::crew_loading(p, cx.reduce_motion()))
                .into_any_element();
        };
        let layout = model::layout(crew, &map.query, &map.open);
        let z = map.zoom;
        let key_layout = layout.clone();
        let canvas_focus = map.focus.clone();
        let mut canvas = div().relative().w(px(layout.width * z)).h(px(layout.height * z));
        for cell in &layout.cells {
            let x = 24. + cell.depth as f32 * model::COL;
            let y = 24. + cell.y;
            if let Some(parent) = cell
                .parent
                .as_ref()
                .and_then(|id| layout.cells.iter().find(|c| &c.id == id))
            {
                let px0 = 24. + parent.depth as f32 * model::COL + model::WIDTH;
                let py0 = 24. + parent.y + model::HEIGHT / 2.;
                let py1 = y + model::HEIGHT / 2.;
                let middle = (px0 + x) / 2.;
                for (left, top, width, height) in [
                    (px0, py0, middle - px0, 1.),
                    (middle, py0.min(py1), 1., (py1 - py0).abs().max(1.)),
                    (middle, py1, x - middle, 1.),
                ] {
                    canvas = canvas.child(
                        div()
                            .absolute()
                            .left(px(left * z))
                            .top(px(top * z))
                            .w(px(width * z))
                            .h(px(height * z))
                            .bg(p.border),
                    );
                }
            }
            let id = cell.id.clone();
            let selected = map.selected == id;
            let mut card = v_flex()
                .id(SharedString::from(format!("map-{}", id)))
                .absolute()
                .left(px(x * z))
                .top(px(y * z))
                .w(px(model::WIDTH * z))
                .h(px(model::HEIGHT * z))
                .p(px(10. * z))
                .gap(px(4. * z))
                .rounded_md()
                .overflow_hidden()
                .bg(if selected { p.selected } else { p.raised })
                .text_size(px(12. * z))
                .cursor_pointer()
                .when(cell.matched, |s| s.border_2().border_color(p.strong))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            if cell.group {
                let group = id.clone();
                card = card
                    .child(format!("{} finished", cell.hidden))
                    .child("Click to expand")
                    .on_click(cx.listener(move |this, _, _, cx| this.map_toggle(group.clone(), cx)));
            } else if let Some(node) = crew.resolve(&id) {
                card = card
                    .child(crew_fold::first_line(
                        if node.kind == AgentKind::Lead {
                            "Your session"
                        } else {
                            &node.title
                        },
                        60,
                    ))
                    .child(div().text_color(p.muted).child(format!(
                        "{}{} · {} steps",
                        status(node),
                        if node.status_source == StatusSource::Inferred {
                            " · inferred"
                        } else {
                            ""
                        },
                        node.steps
                    )));
                if cell.hidden > 0 {
                    card = card.child(format!("{} hidden · Enter to expand", cell.hidden));
                }
                let pick = id.clone();
                card = card.on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(map) = this.side.agents.as_mut().and_then(|s| s.map.as_mut()) {
                        map.selected = pick.clone();
                        map.focus.focus(window, cx);
                    }
                    cx.notify();
                }));
            }
            canvas = canvas.child(card);
        }
        let selected = map.selected.clone();
        let controls =
            h_flex()
                .gap_3()
                .p_3()
                .child(
                    div()
                        .id("map-back")
                        .cursor_pointer()
                        .child("Back to Agents")
                        .on_click(cx.listener(|this, _, _, cx| this.map_close(cx))),
                )
                .child(div().flex_1().child(Input::new(&map.input)))
                .child(
                    div()
                        .id("map-zoom-out")
                        .cursor_pointer()
                        .child("−")
                        .on_click(cx.listener(|this, _, _, cx| this.map_zoom(-0.1, cx))),
                )
                .child(format!("{}%", (z * 100.).round() as u32))
                .child(
                    div()
                        .id("map-zoom-in")
                        .cursor_pointer()
                        .child("+")
                        .on_click(cx.listener(|this, _, _, cx| this.map_zoom(0.1, cx))),
                )
                .child(div().id("map-fit").cursor_pointer().child("Fit").on_click(cx.listener(
                    |this, _, window, cx| {
                        if let Some(state) = &mut this.side.agents
                            && let (Some(map), Some(crew)) = (&mut state.map, &state.crew)
                        {
                            let layout = model::layout(crew, &map.query, &map.open);
                            let size = window.viewport_size();
                            map.zoom = model::fit(&layout, f32::from(size.width) - 80., f32::from(size.height) - 220.);
                            map.scroll.set_offset(point(px(0.), px(0.)));
                        }
                        cx.notify();
                    },
                )))
                .child(
                    div()
                        .id("inspect-map-agent")
                        .cursor_pointer()
                        .child("Activity and controls")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_agent(selected.clone(), cx);
                            this.map_close(cx);
                        })),
                );
        v_flex()
            .size_full()
            .track_focus(&map.focus)
            .text_color(p.foreground)
            .bg(p.sidebar)
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if !this
                    .side
                    .agents
                    .as_ref()
                    .and_then(|s| s.map.as_ref())
                    .is_some_and(|m| m.focus.is_focused(window))
                {
                    return;
                }
                let key = event.keystroke.key.as_str();
                if key == "escape" {
                    this.map_close(cx);
                    cx.stop_propagation();
                    return;
                }
                if let Some(map) = this.side.agents.as_mut().and_then(|s| s.map.as_mut()) {
                    if key == "enter" {
                        let id = map.selected.clone();
                        this.map_toggle(id, cx);
                    } else if ["left", "right", "up", "down", "home", "end"].contains(&key) {
                        map.selected = model::step(&key_layout, &map.selected, key);
                        if let Some(cell) = key_layout.cells.iter().find(|c| c.id == map.selected) {
                            map.scroll.set_offset(point(
                                px(-(cell.depth as f32 * model::COL * z - 80.).max(0.)),
                                px(-(cell.y * z - 120.).max(0.)),
                            ));
                        }
                    } else {
                        return;
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(controls)
            .child(
                h_flex()
                    .flex_1()
                    .min_h(px(0.))
                    .items_stretch()
                    .child(
                        div()
                            .id("crew-map-scroll")
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_scroll()
                            .track_scroll(&map.scroll)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    canvas_focus.focus(window, cx);
                                    if let Some(map) = this.side.agents.as_mut().and_then(|s| s.map.as_mut()) {
                                        map.drag = Some(event.position);
                                    }
                                }),
                            )
                            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                                if let Some(map) = this.side.agents.as_mut().and_then(|s| s.map.as_mut())
                                    && let Some(previous) = map.drag
                                {
                                    if event.pressed_button == Some(MouseButton::Left) {
                                        map.scroll.set_offset(map.scroll.offset() + event.position - previous);
                                        map.drag = Some(event.position);
                                        cx.notify();
                                    } else {
                                        map.drag = None;
                                    }
                                }
                            }))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, _| {
                                    if let Some(map) = this.side.agents.as_mut().and_then(|s| s.map.as_mut()) {
                                        map.drag = None;
                                    }
                                }),
                            )
                            .child(canvas),
                    )
                    .child(self.render_map_inspector(
                        crew.resolve(&map.selected).unwrap_or_else(|| crew.lead()),
                        p,
                        cx,
                    )),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(p.muted)
                    .child("Drag or scroll to pan · Arrow keys select · Enter expands · Esc returns"),
            )
            .into_any_element()
    }
}

impl Workspace {
    fn render_map_inspector(&self, node: &AgentNode, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let state = self.side.agents.as_ref().unwrap();
        let now = crate::code::now_ms();
        let elapsed = if !node.status.is_live() {
            node.duration_ms
                .map(|ms| ms as i64)
                .unwrap_or_else(|| node.ended_at.unwrap_or(now) - node.started_at)
        } else {
            now - node.started_at
        };
        let selected = node.id.clone();
        let mut panel = v_flex()
            .id("crew-map-inspector")
            .w(px(300.))
            .flex_none()
            .min_h(px(0.))
            .overflow_y_scroll()
            .p_4()
            .gap_3()
            .border_l_1()
            .border_color(p.border)
            .bg(p.background)
            .text_size(px(12.))
            .text_color(p.foreground)
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(if node.kind == AgentKind::Lead {
                        "Lead".to_string()
                    } else {
                        node.name.clone().unwrap_or_else(|| node.title.clone())
                    }),
            )
            .child(div().text_color(p.muted).child(status(node)))
            .child(format!(
                "{} steps · {} · {}",
                node.steps,
                crew_fold::elapsed(elapsed.max(0)),
                node.cost_usd
                    .map(|cost| format!("${cost:.2}"))
                    .unwrap_or_else(|| "—".into())
            ))
            .when(node.kind != AgentKind::Lead, |panel| {
                panel.child(div().text_color(p.muted).child(node.title.clone()))
            })
            .child(div().font_weight(FontWeight::MEDIUM).child("Latest"));
        if node.recent.is_empty() {
            panel = panel.child(
                div()
                    .text_color(p.muted)
                    .child(node.summary.clone().unwrap_or_else(|| "Nothing yet.".into())),
            );
        }
        for step in node.recent.iter().rev() {
            panel = panel.child(
                v_flex()
                    .gap_1()
                    .child(div().font_family(crate::pages::ui::MONO).child(step.text.clone()))
                    .child(
                        div()
                            .text_color(p.faint)
                            .child(format!("{} ago", crew_fold::elapsed((now - step.at).max(0)))),
                    ),
            );
        }
        if node.status == AgentStatus::Needs {
            let linked = (node.kind == AgentKind::Linked).then(|| node.id.clone());
            panel = panel.child(
                div()
                    .id("map-answer")
                    .cursor_pointer()
                    .text_color(p.strong)
                    .child("Answer in conversation →")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.map_close(cx);
                        if let Some(id) = &linked {
                            this.select_agent(id.clone(), cx);
                        } else {
                            this.answer_in_conversation(window, cx);
                        }
                    })),
            );
        }
        panel = panel.child(
            div()
                .id("map-open-timeline")
                .cursor_pointer()
                .text_color(p.strong)
                .child(if node.kind == AgentKind::Lead {
                    "Open the conversation"
                } else {
                    "Open its timeline"
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_agent(selected.clone(), cx);
                    this.map_close(cx);
                })),
        );
        let caps = state
            .summary
            .as_ref()
            .and_then(|summary| summary.capabilities.as_deref());
        if !state.busy
            && self.runs(&state.id).is_some()
            && crew_fold::controls_of(node, caps).stop.available
            && let Some(task) = &node.task_id
        {
            let args = json!({"threadId": state.id, "taskId": task});
            panel = panel.child(
                div()
                    .id("map-stop")
                    .cursor_pointer()
                    .text_color(p.destructive)
                    .child("Stop")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.agent_action("wb_task_stop", args.clone(), false, cx)),
                    ),
            );
        }
        panel.into_any_element()
    }
}
