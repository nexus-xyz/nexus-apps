//! A session's agent tree, read from its log: the session's own agent (the lead), its sub-agents (Claude Code's Task /
//! Agent calls, Codex's sub-agent threads), their sub-agents, and the commands they left running in the background.
//! The Nexus app folds it in TypeScript; this is the same fold for Nexus GPUI, with
//! crew.ts's rules and tests (`crew_fold/tests.rs`). The Workbench's Agents tool and the crew map read it.
//!
//! The agents' own `task` reports are the truth when there are any; without them a status is read from the log's shape
//! (a call's result, a cut turn, Claude Code's list of what still runs, a process-end notice) and says so
//! ([`StatusSource::Inferred`]). [`CrewFolder`] folds one event at a time and folds only what is new.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::Value;

use crate::event::{
    AgentCapabilities, BackgroundTask, SessionAgent, StoredEvent, TaskDetail, TaskStatus, TurnStatus, WbEvent,
};

use crate::harness::harness;

/// How an agent stands. `Needs`: it holds at least one open approval; it wins over every other live state. `Waiting`:
/// the lead between turns while its agents work. `Done`, `Failed`, `Stopped`: it ended. `Idle`: a linked session is
/// between turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentStatus {
    Running,
    Needs,
    Waiting,
    Idle,
    Done,
    Failed,
    Stopped,
}

impl AgentStatus {
    /// Running, needs or waiting.
    pub fn is_live(self) -> bool {
        matches!(self, AgentStatus::Running | AgentStatus::Needs | AgentStatus::Waiting)
    }
}

/// `Linked`: a full session another session started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Lead,
    Subagent,
    Shell,
    Linked,
}

/// `Reported`: a `task` event said so. `Inferred`: the fold read it from the log's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusSource {
    Reported,
    Inferred,
}

/// One action in one line, with when (the event's `at`, in ms).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub text: String,
    pub at: i64,
}

/// One open approval an agent holds: enough for a "Needs you" row with Allow and Deny.
#[derive(Debug, Clone, PartialEq)]
pub struct NeedRef {
    pub request_id: String,
    /// The tool call it asks about. A hidden agent's call is not in the timeline.
    pub tool_use_id: Option<String>,
    pub tool: String,
    pub input: Value,
    pub description: Option<String>,
    pub at: i64,
}

/// One node of the tree, as a snapshot hands it out.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentNode {
    /// Stable for the node's life: its tool call's id when the log showed one, else its task id; [`LEAD_ID`] for the
    /// lead.
    pub id: String,
    pub task_id: Option<String>,
    /// The Agent/Task (or background Bash) call that started it.
    pub call_id: Option<String>,
    pub kind: AgentKind,
    pub provider: SessionAgent,
    /// What it does, in the agent's words, or a shell's command line. The lead's: the agent's name.
    pub title: String,
    /// The agent's own name for it (a Codex nickname).
    pub name: Option<String>,
    /// Its type: Claude Code's `subagent_type` ("Explore"), Codex's role.
    pub role: Option<String>,
    /// The agent's task type from its reports ("local_agent", "local_bash", "local_workflow").
    pub task_type: Option<String>,
    pub model: Option<String>,
    pub status: AgentStatus,
    pub status_source: StatusSource,
    /// Why it ended, when said: "worker_restart", "process_ended" or "app_closed".
    pub reason: Option<String>,
    /// It runs past the call that started it. None: not said.
    pub backgrounded: Option<bool>,
    pub started_at: i64,
    /// The first end seen in its current run.
    pub ended_at: Option<i64>,
    /// Its own tool calls: the larger of the log's count and the agent's report.
    pub tool_count: u64,
    /// Tool calls in its whole subtree, `tool_count` included: the "N steps" every view shows.
    pub steps: u64,
    /// Tokens its own model calls used (Codex sub-agents).
    pub tokens: Option<u64>,
    /// The size of its latest model call. Not a sum: never add it across agents.
    pub context_tokens: Option<u64>,
    /// What its own priced calls cost (Codex sub-agents).
    pub cost_usd: Option<f64>,
    /// Its run time as the agent reported it; None: work it out from `started_at` and `ended_at` (or now).
    pub duration_ms: Option<u64>,
    pub last: Option<Activity>,
    /// Its latest actions, at most 4, oldest first.
    pub recent: Vec<Activity>,
    /// How it went, in its own words, once it ended.
    pub summary: Option<String>,
    /// Open approvals it holds, oldest first.
    pub needs: Vec<NeedRef>,
    /// 0 for the lead, 1 for its sub-agents and shells, one more per level.
    pub depth: u32,
    pub parent_id: Option<String>,
    /// Ids of the nodes it started, in the order the log showed them start.
    pub children: Vec<String>,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CrewCounts {
    /// Sub-agents alive (running, needing the person, or waiting); never the lead.
    pub agents_running: u64,
    /// Background commands still running.
    pub shells_running: u64,
    /// Nodes whose status is `Needs`, the lead included.
    pub needs: u64,
    pub done: u64,
    pub failed: u64,
    pub stopped: u64,
    /// Every node but the lead, ended ones included.
    pub total: u64,
}

/// A session's agent tree, read from its log.
#[derive(Debug, Clone, PartialEq)]
pub struct Crew {
    pub provider: SessionAgent,
    /// Every node: the lead first, then in the order the log showed them start.
    pub nodes: Vec<AgentNode>,
    /// Ids of nodes whose status is `Needs`, the one waiting longest first.
    pub needs: Vec<String>,
    /// Each open approval's request id, with the id of the node that holds it, in the order they opened.
    pub approvals: Vec<(String, String)>,
    pub counts: CrewCounts,
    /// Any id a node had (its id, its call id, its task id, its id before a merge), to its place in `nodes`.
    names: HashMap<String, usize>,
    /// A tool call's id, to the place in `nodes` of the node it started.
    calls: HashMap<String, usize>,
}

impl Crew {
    pub fn lead(&self) -> &AgentNode {
        &self.nodes[0]
    }

    /// Ids of every node but the lead, in the order the log showed them start.
    pub fn order(&self) -> impl Iterator<Item = &str> {
        self.nodes[1..].iter().map(|node| node.id.as_str())
    }

    /// A node by any id it had: its id, its call id, its task id, or its id before a merge.
    pub fn resolve(&self, id: &str) -> Option<&AgentNode> {
        self.names.get(id).map(|&index| &self.nodes[index])
    }

    /// The node a tool call started (an Agent/Task call, a background Bash call), if it started one.
    pub fn node_of_call(&self, call_id: &str) -> Option<&AgentNode> {
        self.calls.get(call_id).map(|&index| &self.nodes[index])
    }

    /// `id`'s children, in start order.
    pub fn children_of<'a>(&'a self, node: &'a AgentNode) -> impl Iterator<Item = &'a AgentNode> {
        node.children.iter().filter_map(|id| self.resolve(id))
    }
}

/// Why a control is or is not offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    pub available: bool,
    /// Why not, in words a person reads; None when available.
    pub reason: Option<&'static str>,
}

/// Where a composer on a node sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageTo {
    /// This agent (the lead, a linked session).
    Own,
    /// Offer "Message the lead" with the text kept (every sub-agent).
    Lead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controls {
    pub stop: Control,
    pub background: Control,
    pub output: Control,
    pub message: Control,
    /// None: no composer (a shell).
    pub message_to: Option<MessageTo>,
}

pub const LEAD_ID: &str = "lead";

const ON: Control = Control {
    available: true,
    reason: None,
};

const fn off(reason: &'static str) -> Control {
    Control {
        available: false,
        reason: Some(reason),
    }
}

/// The agent's name as people see it (`AGENT_NAMES`).
pub fn agent_name(agent: SessionAgent) -> &'static str {
    crate::driver::label(agent)
}

/// "2 agents running", "1 agent and 1 command running", "1 command running", else "3 tasks".
pub fn pill_label(counts: &CrewCounts) -> String {
    let part = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let (agents, shells) = (counts.agents_running, counts.shells_running);
    if agents > 0 && shells > 0 {
        format!(
            "{} and {} running",
            part(agents, "agent", "agents"),
            part(shells, "command", "commands")
        )
    } else if agents > 0 {
        format!("{} running", part(agents, "agent", "agents"))
    } else if shells > 0 {
        format!("{} running", part(shells, "command", "commands"))
    } else {
        part(counts.total, "task", "tasks")
    }
}

/// "12s", "3m 04s", "1h 05m".
pub fn elapsed(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {:02}s", seconds % 60);
    }
    format!("{}h {:02}m", minutes / 60, minutes % 60)
}

/// The id Claude Code gives a background command ("Command running in background with ID: b546").
pub fn shell_id(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut from = 0;
    while let Some(found) = output[from..].to_ascii_lowercase().find("id:") {
        let start = from + found;
        from = start + 3;
        if start > 0 && word(bytes[start - 1]) {
            continue;
        }
        let rest = output[from..].trim_start();
        let id: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if !id.is_empty() {
            return Some(id);
        }
    }
    None
}

/// The controls a node offers now, with a reason for each it does not, in this order: no capabilities (an older core,
/// or no live process), the lead (its own controls are the session's), an ended node, then what the agent accepts.
pub fn controls_of(node: &AgentNode, capabilities: Option<&AgentCapabilities>) -> Controls {
    let (message, message_to) = match node.kind {
        AgentKind::Lead | AgentKind::Linked => (ON, Some(MessageTo::Own)),
        AgentKind::Shell => (off("A command takes no messages."), None),
        AgentKind::Subagent => (
            off("Sub-agents can't take messages. Message the lead instead."),
            Some(MessageTo::Lead),
        ),
    };
    let all = |control: Control| Controls {
        stop: control.clone(),
        background: control.clone(),
        output: control,
        message: message.clone(),
        message_to,
    };
    let Some(capabilities) = capabilities else {
        return all(off("This Nexus can't control agents yet."));
    };
    if matches!(node.kind, AgentKind::Lead | AgentKind::Linked) {
        return all(off("Use the session's Interrupt or Stop."));
    }
    if !node.status.is_live() {
        return all(off(if node.kind == AgentKind::Shell {
            "This command has finished."
        } else {
            "This agent has finished."
        }));
    }
    // The kind as the core names it (agents.rs): a task type other than an agent's or a shell's is its own kind.
    let kind = match (node.kind, node.task_type.as_deref()) {
        (AgentKind::Shell, _) => "shell",
        (_, None | Some("") | Some("local_agent")) => "agent",
        (_, Some(other)) => other,
    };
    let can = |kinds: &[String]| kinds.iter().any(|named| named == kind);
    let hints = harness(node.provider).task_control_hints();
    Controls {
        stop: if can(&capabilities.stop) {
            ON
        } else if kind == "agent" || kind == "shell" {
            off(hints.update_to_stop)
        } else {
            off("Nexus can't stop this kind of task.")
        },
        background: if node.backgrounded == Some(true) {
            off("It already runs in the background.")
        } else if can(&capabilities.background) {
            ON
        } else {
            off(hints.cannot_background)
        },
        output: if can(&capabilities.output) {
            ON
        } else {
            off("Only a background command has output to read.")
        },
        message,
        message_to,
    }
}

/// The core's notices for an agent process that ended without a crash. Its crashes are `error` events.
const PROCESS_ENDED: [&str; 4] = [
    "Stopped when Nexus quit.",
    "Nexus closed while this session was working.",
    "What the agent left running in the background stopped with it.",
    "Agent restarted. Your next message resumes the conversation in a new process, which reads MCP servers, skills and settings again.",
];

/// The tool calls that read a background command's output, and those that stop it, by name across Claude Code builds.
const OUTPUT_TOOLS: [&str; 2] = ["BashOutput", "TaskOutput"];
const STOP_TOOLS: [&str; 3] = ["KillShell", "KillBash", "TaskStop"];
const RECENT: usize = 4;

fn text<'a>(input: &'a Value, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or("")
}

fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

/// A non-empty string, as JavaScript's truthiness reads one.
fn some(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

/// The command a read or a stop names: `bash_id` / `shell_id` on older builds, `task_id` on newer ones.
fn target_id(input: &Value) -> &str {
    or(
        or(text(input, "bash_id"), text(input, "shell_id")),
        text(input, "task_id"),
    )
}

/// The first line of `value`, trimmed, at most `max` characters (the last one an ellipsis when cut).
pub fn first_line(value: &str, max: usize) -> String {
    let line = value.trim().split('\n').next().unwrap_or("");
    if line.chars().count() > max {
        let mut cut: String = line.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        line.to_string()
    }
}

/// How a tool call reads in one line (the Nexus web app): a verb and the thing it acts on, and, for an Agent call,
/// its type. An MCP tool reads as its server and tool.
struct ToolLine {
    agent: bool,
    verb: String,
    detail: String,
    extra: Option<String>,
}

fn tool_line(name: &str, input: &Value) -> ToolLine {
    let line = |verb: &str, detail: String| ToolLine {
        agent: false,
        verb: verb.to_string(),
        detail,
        extra: None,
    };
    let path = |key: &str| or(or(text(input, key), text(input, "path")), text(input, "notebook_path")).to_string();
    match name {
        "Bash" => line("Run", first_line(text(input, "command"), 160)),
        "BashOutput" => line("Read output", text(input, "bash_id").into()),
        "KillShell" | "KillBash" => line("Stop shell", or(text(input, "shell_id"), text(input, "bash_id")).into()),
        "Read" | "Write" | "Edit" | "MultiEdit" => line(
            if name == "Read" {
                "Read"
            } else if name == "Write" {
                "Write"
            } else {
                "Edit"
            },
            path("file_path"),
        ),
        "NotebookEdit" => line("Edit notebook", path("notebook_path")),
        "Grep" => line("Search", text(input, "pattern").into()),
        "Glob" => line("Find files", text(input, "pattern").into()),
        "LS" => line("List", path("path")),
        "WebFetch" => line("Fetch", short_url(text(input, "url"))),
        "WebSearch" => line("Search the web", text(input, "query").into()),
        "Task" | "Agent" => ToolLine {
            agent: true,
            verb: "Agent".into(),
            detail: match text(input, "description") {
                "" => first_line(text(input, "prompt"), 80),
                description => description.to_string(),
            },
            extra: Some(text(input, "subagent_type").to_string()).filter(|kind| !kind.is_empty()),
        },
        "TodoWrite" => {
            let todos = input.get("todos").and_then(Value::as_array);
            let total = todos.map_or(0, Vec::len);
            let done = todos.map_or(0, |todos| {
                todos
                    .iter()
                    .filter(|todo| todo.get("status").and_then(Value::as_str) == Some("completed"))
                    .count()
            });
            line("Update the plan", format!("{done} of {total} done"))
        }
        "ExitPlanMode" => {
            let plan = text(input, "plan");
            let plan = plan.trim_start_matches('#').trim_start();
            line("Propose a plan", first_line(plan, 100))
        }
        _ => {
            let detail = first_argument(input);
            match name.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")) {
                Some((server, tool)) if !server.is_empty() && !tool.is_empty() => {
                    line(&format!("{server} · {}", tool.replace('_', " ")), detail)
                }
                _ => line(name, detail),
            }
        }
    }
}

fn first_argument(input: &Value) -> String {
    input
        .as_object()
        .and_then(|object| {
            object
                .values()
                .filter_map(Value::as_str)
                .find(|value| !value.trim().is_empty())
        })
        .map(|value| first_line(value, 100))
        .unwrap_or_default()
}

fn short_url(url: &str) -> String {
    let Some((_, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let (host, path) = match rest.find(['/', '?', '#']) {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    if host.is_empty() {
        return url.to_string();
    }
    if path.is_empty() || path == "/" {
        host.to_string()
    } else {
        format!("{host}{path}")
    }
}

/// The status a BashOutput result reports ("<status>completed</status>"), when it does.
fn output_status(output: &str) -> Option<AgentStatus> {
    let lower = output.to_ascii_lowercase();
    let start = lower.find("<status>")? + "<status>".len();
    let end = start + lower[start..].find("</status>")?;
    match lower[start..end].trim() {
        "running" => Some(AgentStatus::Running),
        "completed" => Some(AgentStatus::Done),
        "failed" => Some(AgentStatus::Failed),
        "killed" => Some(AgentStatus::Stopped),
        _ => None,
    }
}

/// A tool result's text without its tags (`<` up to the next `>`, at least one character between), on one line.
fn untagged(output: &str) -> String {
    let mut plain = String::with_capacity(output.len());
    let mut rest = output;
    while let Some(open) = rest.find('<') {
        plain.push_str(&rest[..open]);
        match rest[open + 1..].find('>') {
            Some(close) if close > 0 => {
                plain.push(' ');
                rest = &rest[open + 1 + close + 1..];
            }
            _ => {
                plain.push('<');
                rest = &rest[open + 1..];
            }
        }
    }
    plain.push_str(rest);
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A node as the fold keeps it.
#[derive(Debug, Clone)]
struct Node {
    id: String,
    task_id: Option<String>,
    task_type: Option<String>,
    call_id: Option<String>,
    kind: AgentKind,
    title: String,
    name: Option<String>,
    role: Option<String>,
    model: Option<String>,
    /// How it stands before open approvals count.
    base: AgentStatus,
    status: AgentStatus,
    status_source: StatusSource,
    reason: Option<String>,
    backgrounded: Option<bool>,
    started_at: i64,
    ended_at: Option<i64>,
    /// The tool calls the log shows it making, and the count its reports gave.
    logged: i64,
    reported: i64,
    steps: i64,
    tokens: Option<u64>,
    context_tokens: Option<u64>,
    cost_usd: Option<f64>,
    duration_ms: Option<u64>,
    recent: Vec<Activity>,
    summary: Option<String>,
    needs: Vec<NeedRef>,
    depth: u32,
    parent: Option<usize>,
    children: Vec<usize>,
    run_id: Option<String>,
    /// The agent has reported on it (a `task` event).
    announced: bool,
    /// Its call asked to run in the background (`run_in_background`).
    background_call: bool,
    /// Its call's `description`: what Claude Code's task list names it, before a report links the two.
    call_description: Option<String>,
    /// Merged into another node: out of the tree.
    gone: bool,
}

/// What a `task` event says.
struct Report<'a> {
    task_id: &'a str,
    tool_use_id: Option<&'a str>,
    task_type: Option<&'a str>,
    description: &'a str,
}

/// The fold of one log.
struct FoldState {
    provider: SessionAgent,
    /// Every node ever made; the lead is 0. Merged ones stay, marked `gone`.
    arena: Vec<Node>,
    /// Live nodes in start order, the lead first.
    order: Vec<usize>,
    by_id: HashMap<String, usize>,
    by_call: HashMap<String, usize>,
    by_task: HashMap<String, usize>,
    /// Ids merged away, to the node they became.
    aliases: HashMap<String, usize>,
    /// Every tool call's parent call (the agent that made it).
    call_parent: HashMap<String, Option<String>>,
    /// Reads of a background command's output, by call id, for their results.
    output_calls: HashMap<String, Value>,
    /// Foreground Bash calls, by call id, with their command and when they were made.
    foreground_calls: HashMap<String, (String, i64)>,
    /// Foreground commands Claude Code reported, by task id: their call, and what the report called them.
    foreground: HashMap<String, (String, String)>,
    by_shell: HashMap<String, usize>,
    /// Task ids on Claude Code's last list of what still runs.
    listed: HashSet<String>,
    /// Each open approval, to the node that holds it, in the order they opened.
    pending: Vec<(String, usize)>,
    /// Nodes alive now, the lead apart.
    live: BTreeSet<usize>,
    turn_open: bool,
    last_end: Option<AgentStatus>,
    counts: CrewCounts,
}

const LEAD: usize = 0;

impl FoldState {
    fn new(provider: SessionAgent) -> FoldState {
        let mut state = FoldState {
            provider,
            arena: Vec::new(),
            order: Vec::new(),
            by_id: HashMap::new(),
            by_call: HashMap::new(),
            by_task: HashMap::new(),
            aliases: HashMap::new(),
            call_parent: HashMap::new(),
            output_calls: HashMap::new(),
            foreground_calls: HashMap::new(),
            foreground: HashMap::new(),
            by_shell: HashMap::new(),
            listed: HashSet::new(),
            pending: Vec::new(),
            live: BTreeSet::new(),
            turn_open: false,
            last_end: None,
            counts: CrewCounts::default(),
        };
        let mut lead = state.make(LEAD_ID, AgentKind::Lead, agent_name(provider), None, 0);
        lead.base = AgentStatus::Done;
        lead.status = AgentStatus::Done;
        state.arena.push(lead);
        state.order.push(LEAD);
        state.by_id.insert(LEAD_ID.into(), LEAD);
        state
    }

    fn make(&self, id: &str, kind: AgentKind, title: &str, parent: Option<usize>, at: i64) -> Node {
        Node {
            id: id.to_string(),
            task_id: None,
            task_type: None,
            call_id: None,
            kind,
            title: title.to_string(),
            name: None,
            role: None,
            model: None,
            base: AgentStatus::Running,
            status: AgentStatus::Running,
            status_source: StatusSource::Inferred,
            reason: None,
            backgrounded: None,
            started_at: at,
            ended_at: None,
            logged: 0,
            reported: 0,
            steps: 0,
            tokens: None,
            context_tokens: None,
            cost_usd: None,
            duration_ms: None,
            recent: Vec::new(),
            summary: None,
            needs: Vec::new(),
            depth: parent.map_or(0, |parent| self.arena[parent].depth + 1),
            parent,
            children: Vec::new(),
            run_id: None,
            announced: false,
            background_call: false,
            call_description: None,
            gone: false,
        }
    }

    /// A new node under `parent`, running: counted, listed, and live.
    fn add(&mut self, id: &str, kind: AgentKind, title: &str, parent: usize, at: i64) -> usize {
        let node = self.make(id, kind, title, Some(parent), at);
        let index = self.arena.len();
        self.arena.push(node);
        self.by_id.insert(id.to_string(), index);
        self.order.push(index);
        self.arena[parent].children.push(index);
        self.counts.total += 1;
        self.count(index, 1);
        self.live.insert(index);
        self.refresh_lead();
        index
    }

    /// `node`'s share of the counters, added (`sign` 1) or taken away (-1).
    fn count(&mut self, node: usize, sign: i64) {
        let (kind, status) = (self.arena[node].kind, self.arena[node].status);
        let add = |value: &mut u64| *value = value.saturating_add_signed(sign);
        let counts = &mut self.counts;
        if status == AgentStatus::Needs {
            add(&mut counts.needs);
        }
        if kind == AgentKind::Lead {
            return;
        }
        if status.is_live() {
            if kind == AgentKind::Shell {
                add(&mut counts.shells_running);
            } else {
                add(&mut counts.agents_running);
            }
        } else {
            match status {
                AgentStatus::Done => add(&mut counts.done),
                AgentStatus::Failed => add(&mut counts.failed),
                AgentStatus::Stopped => add(&mut counts.stopped),
                _ => {}
            }
        }
    }

    /// Works `node.status` out again from how it stands and the approvals it holds.
    fn refresh(&mut self, node: usize) {
        let n = &self.arena[node];
        let status = if !n.needs.is_empty() && n.base.is_live() {
            AgentStatus::Needs
        } else {
            n.base
        };
        if status == n.status {
            return;
        }
        self.count(node, -1);
        self.arena[node].status = status;
        self.count(node, 1);
    }

    /// The lead's status: needs with an open approval of its own, running in a turn, waiting while its sub-agents work
    /// (a background command alone is no reason to wait).
    fn refresh_lead(&mut self) {
        let base = if self.turn_open {
            AgentStatus::Running
        } else if self.counts.agents_running > 0 {
            AgentStatus::Waiting
        } else {
            self.last_end.unwrap_or(AgentStatus::Done)
        };
        self.arena[LEAD].base = base;
        self.refresh(LEAD);
    }

    /// Sets how `node` stands (`at`: when, for an end). An end keeps the first time seen in the run.
    fn settle(&mut self, node: usize, base: AgentStatus, source: StatusSource, at: i64) {
        let n = &mut self.arena[node];
        if n.base == base && n.status_source == source {
            return;
        }
        let was_live = n.base.is_live();
        n.base = base;
        n.status_source = source;
        if base.is_live() {
            n.ended_at = None;
            self.live.insert(node);
        } else {
            if was_live || n.ended_at.is_none() {
                n.ended_at = Some(at);
            }
            self.live.remove(&node);
        }
        self.refresh(node);
        if was_live != base.is_live() {
            self.refresh_lead();
        }
    }

    fn ended(&mut self, node: usize, base: AgentStatus, source: StatusSource, at: i64) {
        if self.arena[node].base.is_live() {
            self.settle(node, base, source, at);
        }
    }

    fn activity(&mut self, node: usize, text: String, at: i64) {
        let recent = &mut self.arena[node].recent;
        if recent.len() >= RECENT {
            recent.drain(..=recent.len() - RECENT);
        }
        recent.push(Activity { text, at });
    }

    /// The node `id` names, by any id it had.
    fn find(&self, id: Option<&str>) -> Option<usize> {
        let id = id.filter(|id| !id.is_empty())?;
        self.by_id
            .get(id)
            .or_else(|| self.by_call.get(id))
            .or_else(|| self.aliases.get(id))
            .or_else(|| self.by_task.get(id))
            .copied()
    }

    /// Runs in the background: its call asked to, the agent said so, or Claude Code lists it.
    fn background(&self, node: usize) -> bool {
        let n = &self.arena[node];
        n.background_call
            || n.backgrounded == Some(true)
            || n.task_id.as_ref().is_some_and(|task| self.listed.contains(task))
    }

    /// Every live node stops: its process is gone.
    fn stop_all(&mut self, at: i64) {
        let live: Vec<usize> = self.live.iter().copied().collect();
        for node in live {
            self.settle(node, AgentStatus::Stopped, StatusSource::Inferred, at);
        }
        self.listed.clear();
    }

    /// Folds `merged` (known only from the task list) into `into` (its tool call's node).
    fn merge(&mut self, merged: usize, into: usize) {
        let m = self.arena[merged].clone();
        if !m.base.is_live() && self.arena[into].base.is_live() {
            let at = m.ended_at.unwrap_or(self.arena[into].started_at);
            self.settle(into, m.base, m.status_source, at);
        }
        self.aliases.insert(m.id.clone(), into);
        if m.backgrounded == Some(true) && self.arena[into].backgrounded != Some(true) {
            self.arena[into].backgrounded = Some(true);
        }
        for need in &m.needs {
            if let Some(entry) = self.pending.iter_mut().find(|(id, _)| *id == need.request_id) {
                entry.1 = into;
            }
        }
        self.arena[into].needs.extend(m.needs.iter().cloned());
        let parent = m.parent.unwrap_or(LEAD);
        self.arena[parent].children.retain(|&child| child != merged);
        if m.steps != 0 {
            self.bump(parent, -m.steps);
        }
        for &child in &m.children {
            self.arena[child].parent = Some(into);
            self.arena[into].children.push(child);
            let steps = self.arena[child].steps;
            if steps != 0 {
                self.bump(into, steps);
            }
        }
        if m.logged != 0 {
            self.arena[into].logged += m.logged;
            self.bump(into, m.logged);
        }
        if m.reported > self.arena[into].reported {
            self.report(into, m.reported);
        }
        if m.base.is_live() {
            self.live.remove(&merged);
        }
        self.count(merged, -1);
        self.counts.total -= 1;
        self.by_id.remove(&m.id);
        self.order.retain(|&node| node != merged);
        self.arena[merged].gone = true;
        self.refresh(into);
        self.refresh_lead();
    }

    /// The tool calls `node` reported making beyond those the log shows (a hidden agent's).
    fn hidden(&self, node: usize) -> i64 {
        (self.arena[node].reported - self.arena[node].logged).max(0)
    }

    /// Adds `delta` steps to `node` and every node above it.
    fn bump(&mut self, node: usize, delta: i64) {
        if delta == 0 {
            return;
        }
        let mut next = Some(node);
        while let Some(at) = next {
            self.arena[at].steps += delta;
            next = self.arena[at].parent;
        }
    }

    /// Moves `node` and its subtree under `parent`: steps leave the old ancestors and join the new ones; depths follow.
    fn move_under(&mut self, node: usize, parent: usize) {
        let Some(old) = self.arena[node].parent else {
            return;
        };
        if old == parent {
            return;
        }
        self.arena[old].children.retain(|&child| child != node);
        let steps = self.arena[node].steps;
        if steps != 0 {
            self.bump(old, -steps);
        }
        self.arena[node].parent = Some(parent);
        self.arena[parent].children.push(node);
        if steps != 0 {
            self.bump(parent, steps);
        }
        let mut stack = vec![node];
        while let Some(at) = stack.pop() {
            self.arena[at].depth = self.arena[at].parent.map_or(0, |parent| self.arena[parent].depth + 1);
            stack.extend(self.arena[at].children.iter().copied());
        }
    }

    /// `node` reported `count` tool calls: its own count, and its subtree's steps where the log shows fewer.
    fn report(&mut self, node: usize, count: i64) {
        if count <= self.arena[node].reported {
            return;
        }
        let before = self.hidden(node);
        self.arena[node].reported = count;
        let delta = self.hidden(node) - before;
        self.bump(node, delta);
    }

    /// A tool call `owner` made: its counts, its latest action, and its ancestors' steps.
    fn made(&mut self, owner: usize, name: &str, input: &Value, at: i64) {
        let before = self.hidden(owner);
        self.arena[owner].logged += 1;
        let line = tool_line(name, input);
        // A sub-agent call reads as its description, not "Agent <description>".
        let words: Vec<&str> = if line.agent {
            vec![line.detail.as_str()]
        } else {
            vec![line.verb.as_str(), line.detail.as_str()]
        };
        let text = words
            .into_iter()
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        self.activity(owner, text, at);
        let delta = 1 + self.hidden(owner) - before;
        self.bump(owner, delta);
    }

    /// The node a report is about, made or linked.
    fn reported_node(&mut self, event: &Report, at: i64, under: usize) -> usize {
        let via_call = event.tool_use_id.and_then(|id| self.by_call.get(id).copied());
        let by_task = self.by_task.get(event.task_id).copied();
        if let (Some(via_call), Some(by_task)) = (via_call, by_task)
            && via_call != by_task
        {
            self.merge(by_task, via_call);
        }
        let node = match via_call.or(by_task) {
            Some(node) => {
                if let Some(call) = event.tool_use_id
                    && self.arena[node].call_id.is_none()
                {
                    self.arena[node].call_id = Some(call.to_string());
                    self.by_call.insert(call.to_string(), node);
                    // A foreground command Claude Code listed before the core logged its start: it started when its
                    // call was made.
                    if let Some((_, made_at)) = self.foreground_calls.get(call)
                        && *made_at < self.arena[node].started_at
                    {
                        self.arena[node].started_at = *made_at;
                    }
                }
                node
            }
            None => {
                // Not one the tool calls showed (a command Claude Code moved to the background by itself, say).
                let kind = if event.task_type == Some("local_bash") {
                    AgentKind::Shell
                } else {
                    AgentKind::Subagent
                };
                let made = event
                    .tool_use_id
                    .and_then(|call| self.foreground_calls.get(call).cloned());
                let source = made
                    .as_ref()
                    .map(|(command, _)| command.as_str())
                    .filter(|command| !command.is_empty())
                    .unwrap_or(event.description);
                let title = match first_line(source, 120) {
                    title if title.is_empty() => "Task".to_string(),
                    title => title,
                };
                let id = event.tool_use_id.unwrap_or(event.task_id);
                let node = self.add(id, kind, &title, under, made.map_or(at, |(_, made_at)| made_at));
                if let Some(call) = event.tool_use_id {
                    self.arena[node].call_id = Some(call.to_string());
                    self.by_call.insert(call.to_string(), node);
                }
                node
            }
        };
        self.arena[node].task_id = Some(event.task_id.to_string());
        if let Some(task_type) = event.task_type.filter(|kind| !kind.is_empty()) {
            self.arena[node].task_type = Some(task_type.to_string());
        }
        self.by_task.insert(event.task_id.to_string(), node);
        self.arena[node].announced = true;
        if self.listed.contains(event.task_id) && self.arena[node].backgrounded != Some(true) {
            self.arena[node].backgrounded = Some(true);
        }
        node
    }

    /// A shell node for a foreground command once Claude Code says it runs in the background.
    fn promote(&mut self, task_id: &str, call: &str, description: &str, at: i64) -> usize {
        self.foreground.remove(task_id);
        let made = self.foreground_calls.get(call).cloned();
        let source = made
            .as_ref()
            .map(|(command, _)| command.as_str())
            .filter(|command| !command.is_empty())
            .unwrap_or(description);
        let title = match first_line(source, 120) {
            title if title.is_empty() => "Command".to_string(),
            title => title,
        };
        let owner = self.owner_of(Some(call));
        let node = self.add(
            call,
            AgentKind::Shell,
            &title,
            owner,
            made.map_or(at, |(_, made_at)| made_at),
        );
        self.arena[node].call_id = Some(call.to_string());
        self.arena[node].backgrounded = Some(true);
        self.by_call.insert(call.to_string(), node);
        node
    }

    /// The node a tool call's parent names (the agent that made it), else the lead.
    fn owner_of(&self, call: Option<&str>) -> usize {
        call.and_then(|call| self.call_parent.get(call))
            .and_then(|parent| self.find(parent.as_deref()))
            .unwrap_or(LEAD)
    }

    #[allow(clippy::too_many_arguments)]
    fn task(
        &mut self,
        task_id: &str,
        tool_use_id: Option<&str>,
        task_type: Option<&str>,
        agent_type: Option<&str>,
        status: TaskStatus,
        description: &str,
        tool_count: Option<u64>,
        total_tokens: Option<u64>,
        duration_ms: Option<u64>,
        detail: Option<&TaskDetail>,
        at: i64,
    ) {
        // An update of a running task: it never creates, revives or renames a node.
        if let Some(detail) = detail
            && detail.progress == Some(true)
        {
            self.progress(task_id, tool_count, total_tokens, detail, at);
            return;
        }
        // A long foreground command reports a task of its own, and so does one of a sub-agent whose frames Claude Code
        // does not forward: a step, not a node. A background command is always listed, started by a background call,
        // or says it runs in the background.
        let known = self.foreground.get(task_id).cloned();
        let command = (task_type == Some("local_bash") || known.is_some())
            && detail.and_then(|detail| detail.backgrounded) != Some(true)
            && !self.by_task.contains_key(task_id)
            && !self.listed.contains(task_id)
            && !tool_use_id.is_some_and(|call| self.by_call.contains_key(call));
        if command {
            if status == TaskStatus::Running {
                let call = tool_use_id
                    .map(str::to_string)
                    .or_else(|| known.as_ref().map(|(call, _)| call.clone()))
                    .unwrap_or_else(|| task_id.to_string());
                let words = if description.is_empty() {
                    known.map(|(_, words)| words).unwrap_or_default()
                } else {
                    description.to_string()
                };
                self.foreground.insert(task_id.to_string(), (call, words));
            } else {
                self.foreground.remove(task_id);
            }
            return;
        }
        // Where it sits: the task that started it, else the agent whose call started it, else the lead.
        let parent_task = detail.and_then(|detail| self.find(some(&detail.parent_task_id)));
        let under = parent_task.unwrap_or_else(|| self.owner_of(tool_use_id));
        let report = Report {
            task_id,
            tool_use_id,
            task_type,
            description,
        };
        let node = self.reported_node(&report, at, under);
        let run_id = detail.and_then(|detail| some(&detail.run_id));
        if status != TaskStatus::Running
            && let (Some(run), Some(held)) = (run_id, some(&self.arena[node].run_id))
            && run != held
        {
            // An older run's end.
            return;
        }
        if status == TaskStatus::Running {
            let n = &self.arena[node];
            // Same run, already ended: a repeat, which never revives it.
            if run_id.is_some()
                && run_id == n.run_id.as_deref()
                && !n.base.is_live()
                && n.status_source == StatusSource::Reported
            {
                return;
            }
            // A start; after an end, a resume. A new run says nothing yet of how it goes.
            if !n.base.is_live() {
                let n = &mut self.arena[node];
                n.duration_ms = None;
                n.summary = None;
                n.reason = None;
            }
            // The log placed it under the lead before it knew better: the task it names wins.
            if let Some(parent) = parent_task
                && parent != node
                && self.arena[node].parent == Some(LEAD)
            {
                self.move_under(node, parent);
            }
            if let Some(run) = run_id
                && Some(run) != self.arena[node].run_id.as_deref()
            {
                self.arena[node].run_id = Some(run.to_string());
            }
            self.settle(node, AgentStatus::Running, StatusSource::Reported, at);
            if !description.is_empty() {
                self.arena[node].title = first_line(description, 120);
            }
            if let Some(role) = agent_type.filter(|role| !role.is_empty())
                && self.arena[node].role.as_deref().is_none_or(str::is_empty)
            {
                self.arena[node].role = Some(role.to_string());
            }
        } else {
            let base = match status {
                TaskStatus::Completed => AgentStatus::Done,
                TaskStatus::Failed => AgentStatus::Failed,
                TaskStatus::Stopped | TaskStatus::Running => AgentStatus::Stopped,
            };
            // A reported end stands until a start; a guessed one yields to it.
            let n = &self.arena[node];
            if n.base.is_live() || n.status_source == StatusSource::Inferred {
                self.settle(node, base, StatusSource::Reported, at);
            }
            if !description.is_empty() {
                self.arena[node].summary = Some(description.to_string());
                self.activity(node, first_line(description, 100), at);
            }
            if let Some(reason) = detail.and_then(|detail| some(&detail.reason)) {
                self.arena[node].reason = Some(reason.to_string());
            }
        }
        self.described(node, detail);
        if let Some(count) = tool_count {
            self.report(node, count as i64);
        }
        if let Some(tokens) = total_tokens {
            self.arena[node].context_tokens = Some(tokens);
        }
        if let Some(duration) = duration_ms {
            self.arena[node].duration_ms = Some(duration);
        }
    }

    /// What a report's detail says of the node at any point: its model, its name, whether it runs in the background.
    fn described(&mut self, node: usize, detail: Option<&TaskDetail>) {
        let Some(detail) = detail else {
            return;
        };
        let n = &mut self.arena[node];
        if let Some(model) = some(&detail.model) {
            n.model = Some(model.to_string());
        }
        if let Some(name) = some(&detail.name) {
            n.name = Some(name.to_string());
        }
        if let Some(backgrounded) = detail.backgrounded {
            n.backgrounded = Some(backgrounded);
        }
    }

    /// A running task's update: its counts, context size, model and, while its own calls are hidden, its latest action.
    fn progress(
        &mut self,
        task_id: &str,
        tool_count: Option<u64>,
        total_tokens: Option<u64>,
        detail: &TaskDetail,
        at: i64,
    ) {
        let Some(&node) = self.by_task.get(task_id) else {
            return;
        };
        if !self.arena[node].base.is_live() {
            return;
        }
        if let Some(count) = tool_count {
            self.report(node, count as i64);
        }
        if let Some(tokens) = total_tokens {
            self.arena[node].context_tokens = Some(tokens);
        }
        self.described(node, Some(detail));
        if let Some(activity) = some(&detail.activity)
            && self.hidden(node) > 0
        {
            self.activity(node, first_line(activity, 100), at);
        }
    }

    /// A model call a sub-agent made (Codex): its tokens and cost add up; its size is the node's context now.
    fn usage(&mut self, parent: Option<&str>, model: &str, size: u64, cost_usd: f64) {
        let Some(node) = self.find(parent) else {
            return;
        };
        let n = &mut self.arena[node];
        if n.kind == AgentKind::Lead {
            return;
        }
        n.tokens = Some(n.tokens.unwrap_or(0) + size);
        n.cost_usd = Some(n.cost_usd.unwrap_or(0.) + cost_usd);
        n.context_tokens = Some(size);
        if !model.is_empty() {
            n.model = Some(model.to_string());
        }
    }

    fn list(&mut self, tasks: &[BackgroundTask], at: i64) {
        let now: HashSet<String> = tasks.iter().map(|entry| entry.task_id.clone()).collect();
        for entry in tasks {
            let node = match self.by_task.get(&entry.task_id).copied() {
                Some(node) => node,
                None => match self.foreground.get(&entry.task_id).cloned() {
                    Some((call, description)) => self.promote(&entry.task_id, &call, &description, at),
                    None => match self.call_of(entry) {
                        Some(node) => node,
                        None => {
                            let report = Report {
                                task_id: &entry.task_id,
                                tool_use_id: None,
                                task_type: entry.task_type.as_deref(),
                                description: &entry.description,
                            };
                            self.reported_node(&report, at, LEAD)
                        }
                    },
                },
            };
            if self.arena[node].task_id.is_none() {
                self.arena[node].task_id = Some(entry.task_id.clone());
                self.by_task.insert(entry.task_id.clone(), node);
            }
            // Listed: it runs in the background. A reported end stands; a guessed one yields.
            self.arena[node].backgrounded = Some(true);
            let n = &self.arena[node];
            if !n.base.is_live() && n.status_source == StatusSource::Inferred {
                self.settle(node, AgentStatus::Running, StatusSource::Inferred, at);
            }
        }
        // Off the list: it has ended. Its own report, which comes just after, says how.
        let before: Vec<String> = self.listed.iter().cloned().collect();
        for task_id in before {
            if !now.contains(&task_id)
                && let Some(&node) = self.by_task.get(&task_id)
            {
                self.ended(node, AgentStatus::Done, StatusSource::Inferred, at);
            }
        }
        self.listed = now;
    }

    /// The call node a listed task is, before its start report links them: the one live node of its kind with no task
    /// id whose call has the list's description. None when not exactly one.
    fn call_of(&mut self, entry: &BackgroundTask) -> Option<usize> {
        let kind = if entry.task_type.as_deref() == Some("local_bash") {
            AgentKind::Shell
        } else {
            AgentKind::Subagent
        };
        let mut found = None;
        for &node in &self.live {
            let n = &self.arena[node];
            if n.kind != kind
                || n.task_id.is_some()
                || n.call_id.is_none()
                || n.call_description.as_deref() != Some(entry.description.as_str())
            {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some(node);
        }
        let node = found?;
        self.arena[node].task_id = Some(entry.task_id.clone());
        self.arena[node].announced = true;
        self.by_task.insert(entry.task_id.clone(), node);
        Some(node)
    }

    fn approval(&mut self, need: NeedRef, parent: Option<&str>) {
        // The agent the approval names, else the one that made the tool call it asks about.
        let owner = self
            .find(parent)
            .unwrap_or_else(|| self.owner_of(need.tool_use_id.as_deref()));
        if self.pending.iter().any(|(id, _)| *id == need.request_id) {
            self.resolve_request(&need.request_id);
        }
        self.pending.push((need.request_id.clone(), owner));
        self.arena[owner].needs.push(need);
        self.refresh(owner);
    }

    fn resolve_request(&mut self, request_id: &str) {
        let Some(at) = self.pending.iter().position(|(id, _)| id == request_id) else {
            return;
        };
        let (_, owner) = self.pending.remove(at);
        self.arena[owner].needs.retain(|need| need.request_id != request_id);
        self.refresh(owner);
    }

    fn tool(&mut self, id: &str, name: &str, input: &Value, parent: Option<&str>, at: i64) {
        self.call_parent.insert(id.to_string(), parent.map(str::to_string));
        let owner = self.find(parent).unwrap_or(LEAD);
        if !self.by_call.contains_key(id) {
            if name == "Task" || name == "Agent" {
                let line = tool_line(name, input);
                let title = if line.detail.is_empty() {
                    "Sub-agent"
                } else {
                    &line.detail
                };
                let node = self.add(id, AgentKind::Subagent, title, owner, at);
                let n = &mut self.arena[node];
                n.call_id = Some(id.to_string());
                n.role = line.extra;
                n.call_description = Some(text(input, "description").to_string()).filter(|d| !d.is_empty());
                if input.get("run_in_background") == Some(&Value::Bool(true)) {
                    n.background_call = true;
                    n.backgrounded = Some(true);
                }
                self.by_call.insert(id.to_string(), node);
            } else if name == "Bash" && input.get("run_in_background") == Some(&Value::Bool(true)) {
                let command = text(input, "command");
                let node = self.add(id, AgentKind::Shell, &first_line(command, 120), owner, at);
                let n = &mut self.arena[node];
                n.call_id = Some(id.to_string());
                // Claude Code lists a background command by its description, else by its command line.
                n.call_description =
                    Some(or(text(input, "description"), command).to_string()).filter(|d| !d.is_empty());
                n.background_call = true;
                n.backgrounded = Some(true);
                self.by_call.insert(id.to_string(), node);
            } else if name == "Bash" {
                self.foreground_calls
                    .insert(id.to_string(), (text(input, "command").to_string(), at));
            }
        }
        if STOP_TOOLS.contains(&name)
            && let Some(&target) = self.by_shell.get(target_id(input))
            && !self.arena[target].announced
        {
            self.ended(target, AgentStatus::Stopped, StatusSource::Inferred, at);
        }
        if OUTPUT_TOOLS.contains(&name) {
            self.output_calls.insert(id.to_string(), input.clone());
        }
        self.made(owner, name, input, at);
    }

    fn result(&mut self, id: &str, output: &str, is_error: bool, at: i64) {
        let node = self.by_call.get(id).copied();
        match node.map(|node| (node, self.arena[node].kind)) {
            Some((node, AgentKind::Subagent)) => {
                // A background agent hands back at once ("Async agent launched"): its reports say how it goes.
                let n = &self.arena[node];
                if n.announced && (self.background(node) || !n.base.is_live()) {
                    return;
                }
                if self.background(node) && !is_error {
                    return;
                }
                if n.base.is_live() {
                    self.arena[node].summary = Some(output.to_string());
                    let base = if is_error {
                        AgentStatus::Failed
                    } else {
                        AgentStatus::Done
                    };
                    self.settle(node, base, StatusSource::Inferred, at);
                }
            }
            Some((node, AgentKind::Shell)) => {
                // A background command answers with "ID: <id>"; a Monitor's answer names no ID, and its task id stands
                // in. An error answer, or no ID at all, means it never started.
                let shell = shell_id(output).or_else(|| self.arena[node].task_id.clone());
                match shell {
                    Some(shell) if !is_error => {
                        self.by_shell.insert(shell, node);
                    }
                    _ => self.ended(node, AgentStatus::Failed, StatusSource::Inferred, at),
                }
            }
            _ => {
                let Some(target) = self
                    .output_calls
                    .get(id)
                    .and_then(|input| self.by_shell.get(target_id(input)).copied())
                else {
                    return;
                };
                let line = first_line(&untagged(output), 100);
                if !line.is_empty() {
                    self.activity(target, line, at);
                }
                if let Some(status) = output_status(output)
                    && status != AgentStatus::Running
                    && !self.arena[target].announced
                {
                    self.ended(target, status, StatusSource::Inferred, at);
                }
            }
        }
    }

    fn apply(&mut self, at: i64, event: &WbEvent) {
        if self.arena[LEAD].started_at == 0 {
            self.arena[LEAD].started_at = at;
        }
        match event {
            WbEvent::Session { model, .. } => {
                // A new agent process: what ran in the old one is gone.
                self.stop_all(at);
                if let Some(model) = model.as_ref().filter(|model| !model.is_empty()) {
                    self.arena[LEAD].model = Some(model.clone());
                }
            }
            WbEvent::TurnStart { .. } => {
                self.turn_open = true;
                self.refresh_lead();
            }
            WbEvent::TurnEnd { status, .. } => {
                self.turn_open = false;
                self.last_end = Some(match status {
                    TurnStatus::Completed => AgentStatus::Done,
                    TurnStatus::Error => AgentStatus::Failed,
                    TurnStatus::Interrupted => AgentStatus::Stopped,
                });
                // A turn cut short leaves its foreground sub-agents unfinished: they are not coming back.
                if *status != TurnStatus::Completed {
                    let live: Vec<usize> = self.live.iter().copied().collect();
                    for node in live {
                        if self.arena[node].kind == AgentKind::Subagent
                            && !self.background(node)
                            && !self.arena[node].announced
                        {
                            self.settle(node, AgentStatus::Stopped, StatusSource::Inferred, at);
                        }
                    }
                }
                self.refresh_lead();
            }
            WbEvent::Tool {
                id,
                name,
                input,
                parent,
            } => self.tool(id, name, input, parent.as_deref(), at),
            WbEvent::ToolResult {
                id, output, is_error, ..
            } => self.result(id, output, *is_error, at),
            WbEvent::Task {
                task_id,
                tool_use_id,
                task_type,
                agent_type,
                status,
                description,
                tool_count,
                total_tokens,
                duration_ms,
                detail,
            } => self.task(
                task_id,
                tool_use_id.as_deref(),
                task_type.as_deref(),
                agent_type.as_deref(),
                *status,
                description,
                *tool_count,
                *total_tokens,
                *duration_ms,
                detail.as_deref(),
                at,
            ),
            WbEvent::BackgroundTasks { tasks } => self.list(tasks, at),
            WbEvent::Approval {
                request_id,
                tool_use_id,
                tool,
                input,
                description,
                parent,
                ..
            } => self.approval(
                NeedRef {
                    request_id: request_id.clone(),
                    tool_use_id: tool_use_id.clone(),
                    tool: tool.clone(),
                    input: input.clone(),
                    description: description.clone(),
                    at,
                },
                parent.as_deref(),
            ),
            WbEvent::ApprovalResolved { request_id, .. } => self.resolve_request(request_id),
            WbEvent::Usage {
                model,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                cost_usd,
                parent,
                ..
            } => self.usage(
                parent.as_deref(),
                model,
                input_tokens + cache_read_tokens + cache_write_tokens + output_tokens,
                *cost_usd,
            ),
            // The agent process failed or crashed: its background work went with it.
            WbEvent::Error { .. } => self.stop_all(at),
            WbEvent::Notice { text } if PROCESS_ENDED.contains(&text.as_str()) => self.stop_all(at),
            _ => {}
        }
    }

    fn node_out(&self, node: usize) -> AgentNode {
        let n = &self.arena[node];
        AgentNode {
            id: n.id.clone(),
            task_id: n.task_id.clone(),
            call_id: n.call_id.clone(),
            kind: n.kind,
            provider: self.provider,
            title: n.title.clone(),
            name: n.name.clone(),
            role: n.role.clone(),
            task_type: n.task_type.clone(),
            model: n.model.clone(),
            status: n.status,
            status_source: n.status_source,
            reason: n.reason.clone(),
            backgrounded: n.backgrounded,
            started_at: n.started_at,
            ended_at: n.ended_at,
            tool_count: n.logged.max(n.reported).max(0) as u64,
            steps: n.steps.max(0) as u64,
            tokens: n.tokens,
            context_tokens: n.context_tokens,
            cost_usd: n.cost_usd,
            duration_ms: n.duration_ms,
            last: n.recent.last().cloned(),
            recent: n.recent.clone(),
            summary: n.summary.clone(),
            needs: n.needs.clone(),
            depth: n.depth,
            parent_id: n.parent.map(|parent| self.arena[parent].id.clone()),
            children: n.children.iter().map(|&child| self.arena[child].id.clone()).collect(),
            run_id: n.run_id.clone(),
        }
    }

    fn snapshot(&self) -> Crew {
        let place: HashMap<usize, usize> = self.order.iter().enumerate().map(|(at, &node)| (node, at)).collect();
        let nodes: Vec<AgentNode> = self.order.iter().map(|&node| self.node_out(node)).collect();
        // The lowest priority first, so `find`'s order wins: its id, its call, an id merged away, its task.
        let mut names = HashMap::new();
        for map in [&self.by_task, &self.aliases, &self.by_call, &self.by_id] {
            for (name, node) in map {
                if let Some(&at) = place.get(node) {
                    names.insert(name.clone(), at);
                }
            }
        }
        let calls = self
            .by_call
            .iter()
            .filter_map(|(call, node)| place.get(node).map(|&at| (call.clone(), at)))
            .collect();
        let mut needing: Vec<&AgentNode> = nodes.iter().filter(|node| node.status == AgentStatus::Needs).collect();
        needing.sort_by_key(|node| node.needs.first().map_or(0, |need| need.at));
        Crew {
            provider: self.provider,
            needs: needing.into_iter().map(|node| node.id.clone()).collect(),
            approvals: self
                .pending
                .iter()
                .map(|(request, node)| (request.clone(), self.arena[*node].id.clone()))
                .collect(),
            counts: self.counts,
            nodes,
            names,
            calls,
        }
    }
}

/// Folds a log as it grows: only the events past those folded before, and from the start again on a log that does
/// not continue them (its first new event's number is not the next one).
pub struct CrewFolder {
    provider: SessionAgent,
    state: FoldState,
    /// The last event folded: its number, to tell a log that continues from one that starts over.
    last_seq: Option<i64>,
    cached: Option<Crew>,
    /// Events folded, and fresh starts, since construction (tests).
    pub applied: usize,
    pub resets: usize,
}

impl CrewFolder {
    pub fn new(provider: SessionAgent) -> CrewFolder {
        CrewFolder {
            provider,
            state: FoldState::new(provider),
            last_seq: None,
            cached: None,
            applied: 0,
            resets: 0,
        }
    }

    pub fn provider(&self) -> SessionAgent {
        self.provider
    }

    /// The last event folded's number.
    pub fn last_seq(&self) -> Option<i64> {
        self.last_seq
    }

    /// Folds `events` that follow those folded before (a later page of the same log). Events at or before the last
    /// folded one are skipped; a gap starts the fold over from these events, so a reader that missed some events hands
    /// the whole log again.
    pub fn push<'a>(&mut self, events: impl IntoIterator<Item = &'a StoredEvent>) {
        for stored in events {
            if let Some(last) = self.last_seq {
                if stored.seq <= last {
                    continue;
                }
                if stored.seq != last + 1 {
                    self.reset();
                }
            }
            self.state.apply(stored.at, &stored.event);
            self.last_seq = Some(stored.seq);
            self.applied += 1;
            self.cached = None;
        }
    }

    /// Starts over, as on a log that does not continue the one folded.
    pub fn reset(&mut self) {
        self.state = FoldState::new(self.provider);
        self.last_seq = None;
        self.cached = None;
        self.resets += 1;
    }

    /// The crew as folded so far. The same value while nothing new was folded.
    pub fn crew(&mut self) -> &Crew {
        if self.cached.is_none() {
            self.cached = Some(self.state.snapshot());
        }
        self.cached.as_ref().expect("just set")
    }

    /// Folds `events` as a whole log: continues when it extends the one folded, else starts over.
    pub fn sync(&mut self, events: &[StoredEvent]) -> &Crew {
        let continues = match (self.last_seq, events.first()) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(last), Some(first)) => first.seq <= last + 1 && events.iter().any(|stored| stored.seq == last),
        };
        if !continues {
            self.reset();
        }
        self.push(events);
        self.crew()
    }
}

/// A whole log at once.
pub fn fold_crew(events: &[StoredEvent], provider: SessionAgent) -> Crew {
    let mut state = FoldState::new(provider);
    for stored in events {
        state.apply(stored.at, &stored.event);
    }
    state.snapshot()
}

#[cfg(test)]
mod tests;

#[path = "crew_fold/linked.rs"]
mod linked;
