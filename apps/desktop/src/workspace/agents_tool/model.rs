//! Presentation rules for the crew trees and agent detail ownership.
use super::*;

pub(super) struct Rows<'a> {
    pub agents: Vec<&'a AgentNode>,
    pub commands: Vec<&'a AgentNode>,
    pub hidden: usize,
}

/// Fold whole finished branches, never individual parents. Keep the selected branch visible.
pub(super) fn visible_rows<'a>(crew: &'a Crew, all: bool, selected: Option<&str>) -> Rows<'a> {
    fn live(crew: &Crew, node: &AgentNode) -> bool {
        node.status.is_live() || crew.children_of(node).any(|child| live(crew, child))
    }
    fn walk<'a>(crew: &'a Crew, node: &'a AgentNode, rows: &mut Vec<&'a AgentNode>) {
        rows.push(node);
        for child in crew.children_of(node) {
            walk(crew, child, rows);
        }
    }
    fn contains(crew: &Crew, node: &AgentNode, selected: Option<&str>) -> bool {
        selected == Some(node.id.as_str()) || crew.children_of(node).any(|child| contains(crew, child, selected))
    }
    let mut active = vec![];
    let mut ended = vec![];
    let mut commands = vec![];
    for node in crew.children_of(crew.lead()) {
        if live(crew, node) {
            if node.kind == AgentKind::Shell {
                commands.push(node);
            } else {
                active.push(node);
            }
        } else {
            ended.push(node);
        }
    }
    ended.sort_by_key(|node| node.ended_at.unwrap_or(node.started_at));
    let cut = if all { 0 } else { ended.len().saturating_sub(4) };
    let mut agents = vec![crew.lead()];
    for node in active {
        walk(crew, node, &mut agents);
    }
    let mut hidden = 0;
    for (ix, node) in ended.into_iter().enumerate() {
        if ix < cut && !contains(crew, node, selected) {
            hidden += 1;
        } else {
            walk(crew, node, &mut agents);
        }
    }
    Rows {
        agents,
        commands,
        hidden,
    }
}

pub(super) fn owns(node: &AgentNode, parent: Option<&str>) -> bool {
    if node.kind == AgentKind::Lead {
        parent.is_none()
    } else {
        parent.is_some_and(|parent| Some(parent) == node.call_id.as_deref() || Some(parent) == node.task_id.as_deref())
    }
}

/// Rich text stays Markdown; tool inputs and results keep their literal formatting.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Activity {
    pub seq: i64,
    /// The tool call's id: a call that started a sub-agent shows as that agent's row.
    pub call: Option<String>,
    pub title: Option<String>,
    pub text: String,
    pub preview: String,
    pub error: bool,
}

pub(super) fn activity(events: &[StoredEvent], node: &AgentNode) -> Vec<Activity> {
    let mut rows = vec![];
    let mut calls = std::collections::HashMap::new();
    for stored in events {
        match &stored.event {
            WbEvent::Text { text, parent, .. } if owns(node, parent.as_deref()) => rows.push(Activity {
                seq: stored.seq,
                call: None,
                title: None,
                text: text.clone(),
                preview: String::new(),
                error: false,
            }),
            WbEvent::Tool {
                id,
                name,
                input,
                parent,
            } if owns(node, parent.as_deref()) => {
                calls.insert(id, rows.len());
                rows.push(Activity {
                    seq: stored.seq,
                    call: Some(id.clone()),
                    title: Some(name.clone()),
                    preview: crew_fold::first_line(
                        ["command", "file_path", "path", "description", "query", "pattern"]
                            .into_iter()
                            .find_map(|key| input[key].as_str())
                            .unwrap_or(&input.to_string()),
                        180,
                    ),
                    text: serde_json::to_string_pretty(input).unwrap_or_default(),
                    error: false,
                });
            }
            // Some providers omit parent on results. Ownership comes from the call, never the result.
            WbEvent::ToolResult {
                id,
                output,
                is_error,
                truncated,
                ..
            } => {
                if let Some(&ix) = calls.get(id) {
                    let row = &mut rows[ix];
                    row.text.push_str("\n\n");
                    if *truncated {
                        row.text.push_str("…\n");
                    }
                    row.text.push_str(output);
                    row.error = *is_error;
                    if *is_error {
                        row.preview = crew_fold::first_line(output, 180);
                    }
                }
            }
            _ => {}
        }
    }
    rows
}

pub(super) fn path_to<'a>(crew: &'a Crew, node: &'a AgentNode) -> Vec<&'a AgentNode> {
    let mut path = vec![node];
    let mut current = node;
    while let Some(parent) = current.parent_id.as_deref().and_then(|id| crew.resolve(id)) {
        if path.iter().any(|step| step.id == parent.id) {
            break;
        }
        path.push(parent);
        current = parent;
    }
    path.reverse();
    path
}

/// Tool output is literal, even when it contains Markdown fences of its own.
pub(super) fn literal(text: &str) -> String {
    let longest = text.split(|c| c != '~').map(str::len).max().unwrap_or(0);
    let fence = "~".repeat(longest.max(2) + 1);
    format!("{fence}text\n{text}\n{fence}")
}

pub(super) fn summary(crew: &Crew) -> String {
    let agents: Vec<_> = crew
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, AgentKind::Subagent | AgentKind::Linked))
        .collect();
    let total = agents.len();
    if total == 0 {
        return String::new();
    }
    let running = agents
        .iter()
        .filter(|n| n.status.is_live() && n.status != AgentStatus::Needs)
        .count();
    let needs = agents.iter().filter(|n| n.status == AgentStatus::Needs).count();
    let failed = agents.iter().filter(|n| n.status == AgentStatus::Failed).count();
    let mut parts = vec![format!("{total} {}", if total == 1 { "agent" } else { "agents" })];
    if running > 0 {
        parts.push(format!("{running} running"));
    }
    if needs > 0 {
        parts.push(format!("{needs} {} you", if needs == 1 { "needs" } else { "need" }));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    parts.join(" · ")
}

pub(super) fn step(ids: &[&str], current: Option<&str>, key: &str) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let at = current.and_then(|id| ids.iter().position(|item| *item == id));
    let index = match key {
        "home" => 0,
        "end" => ids.len() - 1,
        "up" => at.unwrap_or(0).saturating_sub(1),
        _ => at.map_or(0, |at| (at + 1).min(ids.len() - 1)),
    };
    Some(ids[index].to_string())
}

/// The port a command serves, when its output, summary or command line names one:
/// `localhost:3000`, `127.0.0.1:8080`, `0.0.0.0:5173`, `[::]:4000`, `--port 8000`, `PORT=3001`. Newest output first.
pub(super) fn port_of(node: &AgentNode) -> Option<String> {
    use std::sync::LazyLock;
    static PATTERNS: LazyLock<[regex::Regex; 3]> = LazyLock::new(|| {
        [
            regex::Regex::new(r"(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::\]):(\d{2,5})\b").unwrap(),
            regex::Regex::new(r"--port[ =](\d{2,5})\b").unwrap(),
            regex::Regex::new(r"\bPORT=(\d{2,5})\b").unwrap(),
        ]
    });
    node.recent
        .iter()
        .rev()
        .map(|activity| activity.text.as_str())
        .chain([node.summary.as_deref().unwrap_or(""), node.title.as_str()])
        .find_map(|text| {
            PATTERNS
                .iter()
                .find_map(|pattern| pattern.captures(text).map(|found| found[1].to_string()))
        })
}

/// One slice of the session total's bar: who spent it, the route hue it wears, and how much.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Share {
    pub label: String,
    /// The route (`crew_route::route_of`); None for the grouped rest, drawn faint.
    pub route: Option<usize>,
    pub usd: f64,
}

/// Sub-agents shown on their own in the bar; the rest share one "Other agents" slice.
const SHARES: usize = 4;

/// The session total split by who spent it. Each priced sub-agent is a slice in its route hue, the biggest first;
/// past [`SHARES`] they group. The lead takes what the sub-agents did not, so the slices add up to the total.
pub(super) fn cost_shares(crew: &Crew, total: f64) -> Vec<Share> {
    let mut agents: Vec<_> = crew
        .nodes
        .iter()
        .filter(|node| !matches!(node.kind, AgentKind::Lead | AgentKind::Shell))
        .filter_map(|node| node.cost_usd.filter(|usd| *usd > 0.).map(|usd| (node, usd)))
        .collect();
    agents.sort_by(|a, b| b.1.total_cmp(&a.1));
    let spent: f64 = agents.iter().map(|(_, usd)| usd).sum();
    let mut shares = vec![];
    let lead = (total - spent).max(0.);
    if lead > 0. {
        shares.push(Share {
            label: "Lead".into(),
            route: Some(0),
            usd: lead,
        });
    }
    let rest: f64 = agents.iter().skip(SHARES).map(|(_, usd)| usd).sum();
    for (node, usd) in agents.into_iter().take(SHARES) {
        shares.push(Share {
            label: node.title.clone(),
            route: Some(crate::crew_route::route_of(crew, &node.id)),
            usd,
        });
    }
    if rest > 0. {
        shares.push(Share {
            label: "Other agents".into(),
            route: None,
            usd: rest,
        });
    }
    shares
}

/// A row's peek: its last three steps, one line each.
pub(super) fn peek(events: &[StoredEvent], node: &AgentNode) -> Vec<String> {
    let rows = activity(events, node);
    let lines: Vec<String> = rows
        .iter()
        .rev()
        .take(3)
        .rev()
        .map(|row| match &row.title {
            Some(title) => format!("{title} {}", row.preview).trim_end().to_string(),
            None => crew_fold::first_line(&row.text, 180),
        })
        .collect();
    if lines.is_empty() {
        node.recent
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(|step| step.text.clone())
            .collect()
    } else {
        lines
    }
}

/// The waiting agent's route, for the tab's ring: the one waiting longest. None when nobody needs you.
pub(super) fn waiting_route(crew: &Crew) -> Option<usize> {
    let id = crew.needs.first().cloned().or_else(|| {
        crew.nodes
            .iter()
            .find(|n| n.status == AgentStatus::Needs)
            .map(|n| n.id.clone())
    })?;
    Some(crate::crew_route::route_of(crew, &id))
}

/// The ring's hue while an agent needs you: the waiting agent's route, amber (route 2) when the lead itself asks.
/// The tab's badge and the top bar's pill draw it.
pub(super) fn waiting_hue(crew: &Crew) -> Option<Hsla> {
    waiting_route(crew).map(|route| crate::palette::Palette::route(if route == 0 { 2 } else { route }))
}

/// Sub-agents and subsessions still at work or waiting on you, never commands.
fn live_subagents(crew: &Crew) -> impl Iterator<Item = &AgentNode> {
    crew.nodes
        .iter()
        .filter(|n| matches!(n.kind, AgentKind::Subagent | AgentKind::Linked) && n.status.is_live())
}

/// How many sub-agents and subsessions are live: the tab's badge.
pub(super) fn live_count(crew: &Crew) -> usize {
    live_subagents(crew).count()
}

/// How many live ones are the harness's sub-agents, and how many are subsessions: the line under the conversation
/// names each.
pub(super) fn live_split(crew: &Crew) -> (usize, usize) {
    live_subagents(crew).fold((0, 0), |(agents, sessions), n| match n.kind {
        AgentKind::Linked => (agents, sessions + 1),
        _ => (agents + 1, sessions),
    })
}

/// When the earliest live sub-agent started, in ms: the clock of the line under the conversation.
pub(super) fn live_since(crew: &Crew) -> Option<i64> {
    live_subagents(crew).map(|n| n.started_at).min()
}

/// Remove descendants of folded branches while keeping each branch's own row.
pub(super) fn folded_rows<'a>(
    crew: &'a Crew,
    all: bool,
    selected: Option<&str>,
    collapsed: &std::collections::HashSet<String>,
) -> Rows<'a> {
    let mut rows = visible_rows(crew, all, selected);
    let visible = |node: &&AgentNode| {
        let mut parent = node.parent_id.as_deref();
        while let Some(id) = parent {
            if collapsed.contains(id) {
                return false;
            }
            parent = crew.resolve(id).and_then(|node| node.parent_id.as_deref());
        }
        true
    };
    rows.agents.retain(visible);
    rows.commands.retain(visible);
    rows
}
