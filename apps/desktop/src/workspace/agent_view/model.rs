//! What a sub-agent's own view shows (`agent_view.rs`): the way back up to the conversation, the facts line, and which controls it offers. Pure: no GPUI.
use nexus_workbench::crew_fold::{self, AgentKind, AgentNode, AgentStatus, Controls, MessageTo, StatusSource};
use nexus_workbench::event::{StoredEvent, WbEvent};

/// Where a breadcrumb goes.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::workspace) enum Target {
    /// The session and the lead: back to the conversation.
    Conversation,
    /// An agent between the lead and this one.
    Agent(String),
    /// This agent: not a link.
    Here,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::workspace) struct Crumb {
    pub label: String,
    pub target: Target,
}

/// The session's title, then the lead, then each agent down to this one. `path` is the lead first.
pub(in crate::workspace) fn crumbs(session: &str, path: &[&AgentNode]) -> Vec<Crumb> {
    let session = if session.trim().is_empty() {
        "Your session"
    } else {
        session
    };
    let mut crumbs = vec![Crumb {
        label: session.to_string(),
        target: Target::Conversation,
    }];
    let last = path.len().saturating_sub(1);
    for (ix, node) in path.iter().enumerate() {
        let (label, target) = if node.kind == AgentKind::Lead {
            ("Lead".to_string(), Target::Conversation)
        } else if ix == last {
            (node.title.clone(), Target::Here)
        } else {
            (node.title.clone(), Target::Agent(node.id.clone()))
        };
        crumbs.push(Crumb { label, target });
    }
    crumbs
}

/// How long it has run: its own figure once it ended, else up to now.
pub(in crate::workspace) fn ran(node: &AgentNode, now: i64) -> String {
    let ms = match (node.status.is_live(), node.duration_ms) {
        (false, Some(ms)) => ms as i64,
        _ => node.ended_at.unwrap_or(now.max(node.started_at)) - node.started_at,
    };
    crew_fold::elapsed(ms)
}

/// "Working · 14 steps · $0.19 · 1m 12s · opus · Explore".
pub(in crate::workspace) fn facts(node: &AgentNode, status: &str, now: i64) -> String {
    let steps = format!("{} {}", node.steps, if node.steps == 1 { "step" } else { "steps" });
    [
        Some(status.to_string()),
        Some(steps),
        node.cost_usd.map(|cost| format!("${cost:.2}")),
        Some(ran(node, now)),
        node.model.clone(),
        node.role.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

/// The note after the facts when the fold read the status from the log, not from the agent's reports.
pub(in crate::workspace) fn inferred(node: &AgentNode) -> Option<&'static str> {
    (node.status_source == StatusSource::Inferred).then_some("(read from the session's tool calls)")
}

/// What the top bar offers for Stop.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::workspace) enum Stop {
    /// The button, with the task it stops.
    Button(String),
    /// A quiet line: "Stop is not offered here: …".
    Why(String),
    /// Nothing: it ended, or the session is read-only here.
    None,
}

pub(in crate::workspace) fn stop(node: &AgentNode, controls: &Controls, controllable: bool) -> Stop {
    if !controllable || !node.status.is_live() {
        return Stop::None;
    }
    match (&node.task_id, controls.stop.available) {
        (Some(task), true) => Stop::Button(task.clone()),
        (None, true) => Stop::Why("its task id is not known yet.".into()),
        (_, false) => Stop::Why(
            controls
                .stop
                .reason
                .unwrap_or("this agent can't be stopped.")
                .to_string(),
        ),
    }
}

/// The foreground call that Background moves, when it can.
pub(in crate::workspace) fn background(node: &AgentNode, controls: &Controls, controllable: bool) -> Option<String> {
    (controllable && node.status.is_live() && controls.background.available)
        .then(|| node.call_id.clone())
        .flatten()
}

/// What the dock under the view holds.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::workspace) enum Dock {
    /// The session's composer: what is written goes to the lead.
    Lead,
    /// Only a note: this agent takes no message.
    Note(&'static str),
    /// The session is read-only here.
    ReadOnly,
}

pub(in crate::workspace) const LEAD_NOTE: &str =
    "Sub-agents take no messages. Add this to the lead’s draft, then send it from the conversation.";

pub(in crate::workspace) fn dock(controls: &Controls, controllable: bool) -> Dock {
    match controls.message_to {
        Some(MessageTo::Lead) if controllable => Dock::Lead,
        Some(MessageTo::Lead) => Dock::ReadOnly,
        _ => Dock::Note(controls.message.reason.unwrap_or("This agent takes no messages here.")),
    }
}

/// The task the lead gave it: an agent's prompt, else its description; a command's line. None when it only repeats
/// the title.
pub(in crate::workspace) fn task(events: &[StoredEvent], node: &AgentNode) -> Option<String> {
    let call = node.call_id.as_deref()?;
    let input = events.iter().find_map(|stored| match &stored.event {
        WbEvent::Tool { id, input, .. } if id == call => Some(input),
        _ => None,
    })?;
    let keys: &[&str] = if node.kind == AgentKind::Shell {
        &["command"]
    } else {
        &["prompt", "description"]
    };
    keys.iter()
        .find_map(|key| input[*key].as_str().map(str::trim).filter(|text| !text.is_empty()))
        .filter(|text| *text != node.title)
        .map(str::to_string)
}

/// "How it ended", once it ended: its own words, else why it stopped.
pub(in crate::workspace) fn ending(node: &AgentNode) -> Option<(&'static str, String)> {
    if node.status.is_live() {
        return None;
    }
    let text = node.summary.clone().or_else(|| node.reason.clone())?;
    let heading = match node.status {
        AgentStatus::Failed => "How it ended: it failed",
        AgentStatus::Stopped => "How it ended: it stopped",
        _ => "How it ended",
    };
    Some((heading, text))
}

/// What the lead reads when a message is written in a sub-agent's view.
pub(in crate::workspace) fn about(title: &str, text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    format!("About the sub-agent \"{title}\": {text}")
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
