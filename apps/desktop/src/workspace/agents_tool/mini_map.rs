//! The mini crew map docked at the top of the Agents tab (design board L5b): the full map's tidy tree, shrunk to
//! status marks on route-hued curves. A click opens that agent's activity and controls in the panel.
use super::map::model::{self, Shape};
use super::rows::{NARROW, crew_mark};
use super::*;
use crate::crew_route::route_of;
use crate::palette::Palette;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use std::collections::HashSet;

/// The strip's height.
pub(super) const STRIP: f32 = 150.;
/// Room between the strip's edge and a mark's centre: a mark's half and its outline fit inside.
const PAD: f32 = 16.;
/// The closest two marks may sit, row to row and column to column. A crew that needs closer folds or scrolls.
const MIN_ROW: f32 = 18.;
const MIN_COL: f32 = 28.;
/// The farthest apart: a small crew stays a compact tree, not spread over the whole strip.
const MAX_ROW: f32 = 28.;
const MAX_COL: f32 = 72.;
/// A mark's box (`crew_mark`) and the selected outline around its 8px dot.
const MARK: f32 = 14.;
const OUTLINE: f32 = 8. + 2. * 3. + 2. * 1.5;
/// The dot grid's pitch.
const GRID: f32 = 12.;

/// A placed mark: its centre in the strip's content, in px.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Mark {
    pub id: String,
    /// The index of its parent in `Mini::marks`.
    pub parent: Option<usize>,
    pub x: f32,
    pub y: f32,
    /// A gathered group of finished agents: how many it holds.
    pub group: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Mini {
    pub marks: Vec<Mark>,
    /// The content's size: the strip's own when the tree fits, more when it has to scroll.
    pub width: f32,
    pub height: f32,
}

/// How many rows fit `height` at the closest spacing.
fn capacity(height: f32) -> usize {
    ((height - 2. * PAD).max(0.) / MIN_ROW).floor() as usize + 1
}

/// Spacing between `count` marks over `room` px, between `least` and `most`.
fn step(count: usize, room: f32, least: f32, most: f32) -> f32 {
    if count < 2 {
        return 0.;
    }
    (room / (count - 1) as f32).clamp(least, most)
}

/// Lays the crew's agents (no commands) into a `width` by `height` strip. Every subtree stays open while it fits; a
/// crew taller than the strip folds its finished subtrees as the full map does, and past that keeps the closest
/// spacing and scrolls.
pub(super) fn mini_layout(crew: &Crew, width: f32, height: f32) -> Mini {
    let open = HashSet::new();
    let mut layout = model::layout_with(
        crew,
        "",
        &open,
        Shape {
            shells: false,
            folding: false,
        },
    );
    if layout.rows > capacity(height) {
        layout = model::layout_with(
            crew,
            "",
            &open,
            Shape {
                shells: false,
                folding: true,
            },
        );
    }
    let rows = layout.rows.max(1);
    let columns = layout.cells.iter().map(|c| c.depth).max().unwrap_or(0) + 1;
    let row = step(rows, height - 2. * PAD, MIN_ROW, MAX_ROW);
    let column = step(columns, width - 2. * PAD, MIN_COL, MAX_COL);
    let tree_width = (columns - 1) as f32 * column;
    let tree_height = (rows - 1) as f32 * row;
    // Centred while it fits; from the top left corner once it scrolls.
    let left = PAD + ((width - 2. * PAD - tree_width) / 2.).max(0.);
    let top = PAD + ((height - 2. * PAD - tree_height) / 2.).max(0.);
    let marks = layout
        .cells
        .iter()
        .map(|cell| Mark {
            id: cell.id.clone(),
            parent: cell
                .parent
                .as_ref()
                .and_then(|id| layout.cells.iter().position(|c| &c.id == id)),
            x: left + cell.depth as f32 * column,
            y: top + cell.y / model::ROW * row,
            group: cell.group.then_some(cell.hidden),
        })
        .collect();
    Mini {
        marks,
        width: width.max(tree_width + 2. * PAD),
        height: height.max(tree_height + 2. * PAD),
    }
}

/// Whether the strip shows: the crew has a sub-agent and the pane (`width`) is wide enough.
pub(super) fn shows(crew: &Crew, width: f32) -> bool {
    width >= NARROW
        && crew
            .nodes
            .iter()
            .any(|node| matches!(node.kind, AgentKind::Subagent | AgentKind::Linked))
}

/// A route from the parent's mark to the child's: an S curve, as the full map's edges.
fn curve(from: Point<Pixels>, to: Point<Pixels>, dashed: bool) -> Option<Path<Pixels>> {
    let half = MARK / 2.;
    let start = point(from.x + px(half), from.y);
    let end = point(to.x - px(half), to.y);
    let middle = (start.x + end.x) / 2.;
    let mut builder = PathBuilder::stroke(px(1.5));
    if dashed {
        builder = builder.dash_array(&[px(3.), px(3.)]);
    }
    builder.move_to(start);
    builder.cubic_bezier_to(end, point(middle, start.y), point(middle, end.y));
    builder.build().ok()
}

impl Workspace {
    /// Opens the agent under the mark and takes the keys so Escape returns to the overview.
    fn mini_pick(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.side.agents {
            state.cursor = Some(id.clone());
            state.focus.focus(window, cx);
        }
        self.open_agent_detail(id, cx);
    }

    /// The strip, or nothing when it hides. `width` is the pane's.
    pub(super) fn render_mini_map(
        &self,
        crew: &Crew,
        state: &AgentsTool,
        p: Palette,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !shows(crew, width) {
            return None;
        }
        // Inside the tab's padding (`p_4`) and the strip's 1px border.
        let mini = mini_layout(crew, width - 32. - 2., STRIP - 2.);
        let edges: Vec<(Point<Pixels>, Point<Pixels>, Hsla, bool)> = mini
            .marks
            .iter()
            .filter_map(|mark| {
                let parent = &mini.marks[mark.parent?];
                let node = crew.resolve(&mark.id);
                let blocked = node.is_some_and(|n| status(n) == "Blocked");
                let color = if blocked {
                    p.steel()
                } else if mark.group.is_some() {
                    p.border
                } else {
                    Palette::route(route_of(crew, &mark.id))
                };
                Some((
                    point(px(parent.x), px(parent.y)),
                    point(px(mark.x), px(mark.y)),
                    color,
                    blocked,
                ))
            })
            .collect();
        let (content_width, content_height) = (mini.width, mini.height);
        let dot = p.border;
        let drawing = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let origin = bounds.origin;
                let mut y = GRID / 2.;
                while y < content_height {
                    let mut x = GRID / 2.;
                    while x < content_width {
                        window.paint_quad(fill(
                            Bounds::new(origin + point(px(x - 0.5), px(y - 0.5)), size(px(1.), px(1.))),
                            dot,
                        ));
                        x += GRID;
                    }
                    y += GRID;
                }
                for (from, to, color, dashed) in &edges {
                    if let Some(path) = curve(origin + *from, origin + *to, *dashed) {
                        window.paint_path(path, *color);
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .w(px(content_width))
        .h(px(content_height));
        let mut content = div()
            .relative()
            .w(px(content_width))
            .h(px(content_height))
            .child(drawing);
        let cursor = state.cursor.as_deref();
        for mark in &mini.marks {
            let id = mark.id.clone();
            if let Some(hidden) = mark.group {
                content = content.child(
                    div()
                        .id(SharedString::from(format!("mini-{id}")))
                        .absolute()
                        .left(px(mark.x - MARK / 2.))
                        .top(px(mark.y - 8.))
                        .h(px(16.))
                        .px(px(4.))
                        .rounded(px(8.))
                        .bg(p.sidebar)
                        .border_1()
                        .border_color(p.border)
                        .text_size(px(10.))
                        .text_color(p.muted)
                        .cursor_pointer()
                        .child(format!("+{hidden}"))
                        .tooltip(move |window, cx| {
                            Tooltip::new(format!("{hidden} finished · show in the list")).build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(state) = &mut this.side.agents {
                                state.show_finished = true;
                            }
                            cx.notify();
                        })),
                );
                continue;
            }
            let Some(node) = crew.resolve(&id) else {
                continue;
            };
            let name = if node.kind == AgentKind::Lead {
                "Lead".to_string()
            } else {
                crew_fold::first_line(&node.title, 60)
            };
            let tip = format!("{name} · {}", status(node));
            let selected = cursor == Some(id.as_str());
            content = content.child(
                div()
                    .id(SharedString::from(format!("mini-{id}")))
                    .absolute()
                    .left(px(mark.x - MARK / 2.))
                    .top(px(mark.y - MARK / 2.))
                    .size(px(MARK))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(selected, |mark| {
                        mark.child(
                            div()
                                .absolute()
                                .left(px((MARK - OUTLINE) / 2.))
                                .top(px((MARK - OUTLINE) / 2.))
                                .size(px(OUTLINE))
                                .rounded_full()
                                .border(px(1.5))
                                .border_color(p.foreground),
                        )
                    })
                    .child(crew_mark(node, route_of(crew, &id), p))
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| this.mini_pick(id.clone(), window, cx))),
            );
        }
        Some(
            div()
                .id("agents-mini-map")
                .flex_none()
                .w_full()
                .h(px(STRIP))
                .rounded(px(8.))
                .border_1()
                .border_color(p.border)
                .overflow_scroll()
                .track_scroll(&state.mini_scroll)
                .child(content)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_COL, MIN_ROW, Mini, NARROW, PAD, STRIP, mini_layout, shows};
    use nexus_workbench::crew_fold::fold_crew;
    use nexus_workbench::event::{SessionAgent, StoredEvent, WbEvent};
    use serde_json::json;

    fn tool(seq: i64, id: &str, name: &str, input: serde_json::Value, parent: Option<&str>) -> StoredEvent {
        StoredEvent {
            thread_id: "t".into(),
            seq,
            at: seq * 1000,
            event: WbEvent::Tool {
                id: id.into(),
                name: name.into(),
                input,
                parent: parent.map(Into::into),
            },
        }
    }

    fn done(seq: i64, id: &str) -> StoredEvent {
        StoredEvent {
            thread_id: "t".into(),
            seq,
            at: seq * 1000,
            event: WbEvent::ToolResult {
                id: id.into(),
                output: "done".into(),
                images: vec![],
                is_error: false,
                truncated: false,
                parent: None,
            },
        }
    }

    fn agent(seq: i64, id: &str, parent: Option<&str>) -> StoredEvent {
        tool(seq, id, "Agent", json!({"description": format!("Task {id}")}), parent)
    }

    fn inside(mini: &Mini, width: f32, height: f32) -> bool {
        mini.marks
            .iter()
            .all(|m| m.x >= PAD && m.x <= width - PAD && m.y >= PAD && m.y <= height - PAD)
    }

    #[test]
    fn a_small_crew_fits_the_strip_without_commands() {
        let events = vec![
            agent(1, "a", None),
            agent(2, "b", None),
            agent(3, "c", Some("a")),
            tool(
                4,
                "sh",
                "Bash",
                json!({"command": "pnpm dev", "run_in_background": true}),
                None,
            ),
        ];
        let crew = fold_crew(&events, SessionAgent::Claude);
        let mini = mini_layout(&crew, 320., STRIP);
        let ids: Vec<&str> = mini.marks.iter().map(|m| m.id.as_str()).collect();
        assert!(!ids.contains(&"sh"), "commands are not drawn: {ids:?}");
        assert_eq!(ids.len(), 4);
        assert_eq!((mini.width, mini.height), (320., STRIP));
        assert!(inside(&mini, 320., STRIP));
        let at = |id: &str| mini.marks.iter().find(|m| m.id == id).unwrap();
        // Left to right by depth, a parent level with the middle of its children.
        assert!(at("lead").x < at("a").x && at("a").x < at("c").x);
        assert_eq!(at("a").x, at("b").x);
        assert!(at("a").y < at("b").y);
        assert_eq!(at("a").parent, ids.iter().position(|id| *id == "lead"));
        assert_eq!(at("c").parent, ids.iter().position(|id| *id == "a"));
        assert!((at("lead").y - (at("c").y + at("b").y) / 2.).abs() < 0.01);
    }

    #[test]
    fn spacing_stays_between_its_bounds() {
        let crew = fold_crew(&[agent(1, "a", None)], SessionAgent::Claude);
        let mini = mini_layout(&crew, 1200., STRIP);
        let (lead, a) = (&mini.marks[0], &mini.marks[1]);
        assert_eq!(a.x - lead.x, MAX_COL);
        // One row: centred in the strip.
        assert_eq!(lead.y, STRIP / 2.);
        assert!(inside(&mini, 1200., STRIP));
    }

    #[test]
    fn a_tall_finished_crew_folds_and_a_tall_live_one_scrolls() {
        let mut events = vec![];
        for i in 0..30 {
            events.push(agent(i * 2 + 1, &format!("a{i}"), None));
            events.push(done(i * 2 + 2, &format!("a{i}")));
        }
        let finished = fold_crew(&events, SessionAgent::Claude);
        let mini = mini_layout(&finished, 320., STRIP);
        assert_eq!(mini.marks.len(), 2);
        assert_eq!(mini.marks[1].group, Some(30));
        assert!(inside(&mini, 320., STRIP));

        let live: Vec<_> = (0..30).map(|i| agent(i + 1, &format!("a{i}"), None)).collect();
        let live = fold_crew(&live, SessionAgent::Claude);
        let mini = mini_layout(&live, 320., STRIP);
        assert_eq!(mini.marks.len(), 31);
        assert!(mini.height > STRIP);
        let column: Vec<f32> = mini.marks[1..].iter().map(|m| m.y).collect();
        assert!(column.windows(2).all(|w| (w[1] - w[0] - MIN_ROW).abs() < 0.01));
    }

    #[test]
    fn the_strip_hides_without_sub_agents_or_in_a_narrow_pane() {
        let shell = tool(
            1,
            "sh",
            "Bash",
            json!({"command": "pnpm dev", "run_in_background": true}),
            None,
        );
        let only_commands = fold_crew(&[shell], SessionAgent::Claude);
        assert!(!shows(&only_commands, 400.));
        let crew = fold_crew(&[agent(1, "a", None)], SessionAgent::Claude);
        assert!(shows(&crew, 400.));
        assert!(!shows(&crew, NARROW - 1.));
    }
}
