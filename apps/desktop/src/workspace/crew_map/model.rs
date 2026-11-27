//! Native canvas geometry and navigation. Agent state always comes from the shared crew fold. The full crew map and
//! the Agents tab's mini map share the tree layout.
use nexus_workbench::crew_fold::{AgentKind, AgentNode, AgentStatus, Crew};
use std::collections::HashSet;

pub const WIDTH: f32 = 240.;
pub const HEIGHT: f32 = 84.;
pub const COL: f32 = 300.;
pub const ROW: f32 = 108.;

#[derive(Debug, Clone)]
pub struct Cell {
    pub id: String,
    pub parent: Option<String>,
    pub depth: usize,
    pub y: f32,
    pub hidden: usize,
    pub group: bool,
    pub matched: bool,
}
#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub cells: Vec<Cell>,
    pub width: f32,
    pub height: f32,
    /// How many leaf rows the tree takes; a cell's row is `y / ROW`.
    pub rows: usize,
}

/// What a layout draws: whether background commands are nodes, and whether finished subtrees fold.
#[derive(Debug, Clone, Copy)]
pub struct Shape {
    pub shells: bool,
    pub folding: bool,
}

fn kids<'a>(crew: &'a Crew, node: &'a AgentNode, shape: Shape) -> impl Iterator<Item = &'a AgentNode> {
    crew.children_of(node)
        .filter(move |child| shape.shells || child.kind != AgentKind::Shell)
}

fn matches(node: &AgentNode, query: &str) -> bool {
    [
        node.title.as_str(),
        node.name.as_deref().unwrap_or(""),
        node.role.as_deref().unwrap_or(""),
        node.model.as_deref().unwrap_or(""),
    ]
    .iter()
    .any(|s| s.to_lowercase().contains(query))
}

fn subtree(crew: &Crew, node: &AgentNode, query: &str, shape: Shape) -> (usize, bool, bool) {
    let mut size = 1;
    let mut live = node.status.is_live() || node.status == AgentStatus::Idle;
    let mut hit = matches(node, query);
    for child in kids(crew, node, shape) {
        let (n, working, matched) = subtree(crew, child, query, shape);
        size += n;
        live |= working;
        hit |= matched;
    }
    (size, live, hit)
}

/// The full map's layout: every node, commands included, folding finished subtrees past 24 nodes.
pub fn layout(crew: &Crew, query: &str, open: &HashSet<String>) -> Layout {
    let shape = Shape {
        shells: true,
        folding: crew.counts.total > 24,
    };
    layout_with(crew, query, open, shape)
}

pub fn layout_with(crew: &Crew, query: &str, open: &HashSet<String>, shape: Shape) -> Layout {
    let query = query.trim().to_lowercase();
    struct Walk<'a> {
        crew: &'a Crew,
        query: &'a str,
        open: &'a HashSet<String>,
        shape: Shape,
        row: f32,
        cells: Vec<Cell>,
    }
    impl Walk<'_> {
        fn node(&mut self, node: &AgentNode, parent: Option<String>, depth: usize) -> f32 {
            let index = self.cells.len();
            let (size, live, inside) = subtree(self.crew, node, self.query, self.shape);
            let fold = self.shape.folding
                && depth > 0
                && size > 1
                && !live
                && !self.open.contains(&node.id)
                && (self.query.is_empty() || !inside);
            self.cells.push(Cell {
                id: node.id.clone(),
                parent,
                depth,
                y: 0.,
                hidden: if fold { size - 1 } else { 0 },
                group: false,
                matched: !self.query.is_empty() && matches(node, self.query),
            });
            let children: Vec<_> = if fold {
                vec![]
            } else {
                kids(self.crew, node, self.shape).collect()
            };
            let group_id = format!("finished:{}", node.id);
            let done: Vec<_> = children
                .iter()
                .filter(|child| {
                    let (_, live, hit) = subtree(self.crew, child, self.query, self.shape);
                    !live && (self.query.is_empty() || !hit)
                })
                .map(|n| n.id.as_str())
                .collect();
            let gather = self.shape.folding && done.len() >= 4 && !self.open.contains(&group_id);
            let mut positions = vec![];
            for child in &children {
                if gather && done.contains(&child.id.as_str()) {
                    continue;
                }
                positions.push(self.node(child, Some(node.id.clone()), depth + 1));
            }
            if gather {
                let hidden = children
                    .iter()
                    .filter(|c| done.contains(&c.id.as_str()))
                    .map(|c| subtree(self.crew, c, self.query, self.shape).0)
                    .sum();
                self.cells.push(Cell {
                    id: group_id,
                    parent: Some(node.id.clone()),
                    depth: depth + 1,
                    y: self.row,
                    hidden,
                    group: true,
                    matched: false,
                });
                positions.push(self.row);
                self.row += ROW;
            }
            let y = if positions.is_empty() {
                let y = self.row;
                self.row += ROW;
                y
            } else {
                (positions[0] + positions[positions.len() - 1]) / 2.
            };
            self.cells[index].y = y;
            y
        }
    }
    let mut walk = Walk {
        crew,
        query: &query,
        open,
        shape,
        row: 0.,
        cells: vec![],
    };
    walk.node(crew.lead(), None, 0);
    let width = walk.cells.iter().map(|c| c.depth).max().unwrap_or(0) as f32 * COL + WIDTH + 48.;
    let height = walk.row.max(ROW) + 48.;
    Layout {
        cells: walk.cells,
        width,
        height,
        rows: (walk.row / ROW).round() as usize,
    }
}

pub fn step(layout: &Layout, id: &str, key: &str) -> String {
    let Some(here) = layout.cells.iter().find(|c| c.id == id) else {
        return layout.cells.first().map(|c| c.id.clone()).unwrap_or_default();
    };
    let target = match key {
        "home" => layout.cells.first(),
        "end" => layout.cells.last(),
        "left" => here
            .parent
            .as_ref()
            .and_then(|id| layout.cells.iter().find(|c| &c.id == id)),
        "right" => layout.cells.iter().find(|c| c.parent.as_deref() == Some(id)),
        "up" => layout
            .cells
            .iter()
            .filter(|c| c.depth == here.depth && c.y < here.y)
            .max_by(|a, b| a.y.total_cmp(&b.y)),
        "down" => layout
            .cells
            .iter()
            .filter(|c| c.depth == here.depth && c.y > here.y)
            .min_by(|a, b| a.y.total_cmp(&b.y)),
        _ => None,
    };
    target.unwrap_or(here).id.clone()
}

pub fn zoom(value: f32) -> f32 {
    if value.is_finite() { value.clamp(0.25, 2.) } else { 1. }
}
pub fn fit(layout: &Layout, width: f32, height: f32) -> f32 {
    zoom((width / layout.width).min(height / layout.height).min(1.))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_workbench::crew_fold::fold_crew;
    use nexus_workbench::event::{SessionAgent, StoredEvent, WbEvent};
    use serde_json::json;
    fn crew(count: usize) -> Crew {
        let mut events = vec![];
        for i in 0..count {
            let id = format!("a{i}");
            events.push(StoredEvent {
                thread_id: "t".into(),
                seq: (i * 2 + 1) as i64,
                at: 0,
                event: WbEvent::Tool {
                    id: id.clone(),
                    name: "Agent".into(),
                    input: json!({"description":format!("Task {i}")}),
                    parent: None,
                },
            });
            events.push(StoredEvent {
                thread_id: "t".into(),
                seq: (i * 2 + 2) as i64,
                at: 1,
                event: WbEvent::ToolResult {
                    id,
                    output: "done".into(),
                    images: vec![],
                    is_error: false,
                    truncated: false,
                    parent: None,
                },
            });
        }
        fold_crew(&events, SessionAgent::Claude)
    }
    #[test]
    fn small_crews_keep_shape_and_keys_follow_the_tree() {
        let layout = layout(&crew(3), "", &HashSet::new());
        assert_eq!(layout.cells.len(), 4);
        assert_eq!(step(&layout, "lead", "right"), "a0");
        assert_eq!(step(&layout, "a0", "down"), "a1");
        assert_eq!(step(&layout, "a1", "left"), "lead");
        assert!(layout.cells[1].y < layout.cells[2].y);
        assert_eq!(step(&layout, "a0", "up"), "a0");
    }
    #[test]
    fn large_finished_crews_gather_and_search_reveals_matches() {
        let crew = crew(30);
        let closed = layout(&crew, "", &HashSet::new());
        assert_eq!(closed.cells.len(), 2);
        assert_eq!(closed.cells[1].hidden, 30);
        let found = layout(&crew, "Task 12", &HashSet::new());
        assert!(found.cells.iter().any(|c| c.id == "a12" && c.matched));
        let opened = layout(&crew, "", &HashSet::from(["finished:lead".into()]));
        assert_eq!(opened.cells.len(), 31);
        assert!(
            opened
                .cells
                .windows(2)
                .filter(|w| w[0].depth == w[1].depth)
                .all(|w| w[0].y < w[1].y)
        );
    }
    #[test]
    fn zoom_is_bounded_and_fit_does_not_enlarge_small_crews() {
        assert_eq!(zoom(0.), 0.25);
        assert_eq!(zoom(8.), 2.);
        assert_eq!(zoom(f32::NAN), 1.);
        let layout = layout(&crew(1), "", &HashSet::new());
        assert_eq!(fit(&layout, 4000., 2000.), 1.);
        assert!(fit(&layout, 200., 100.) <= 0.5);
    }
}
