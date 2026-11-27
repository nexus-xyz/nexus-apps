//! How a Code session's run of work folds, as the Nexus app's timeline shows it (Nexus UI's
//! `session-timeline/work-folds.ts`). The calls between two pieces of prose share one card; sub-agents side by
//! side share another; a plan keeps its own. Routine steps (reads, searches, commands, thinking) fold into one line
//! that says what they did ("Explored 6 files, ran 2 commands"); the steps that matter stay in view. This is the same
//! rule for the apps written in Rust, and its tests are the TypeScript tests, ported. Pure: the app draws it.

use std::ops::Range;

/// What a call does, by its tool's name (`summarize`'s `kind` in `tools.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Command,
    Read,
    Write,
    Edit,
    Search,
    Find,
    Web,
    Agent,
    Todo,
    Plan,
    Mcp,
    Other,
}

/// The kind of call tool `name` makes.
pub fn kind(name: &str) -> Kind {
    match name {
        "Bash" | "BashOutput" | "KillShell" | "KillBash" => Kind::Command,
        "Read" => Kind::Read,
        "Write" => Kind::Write,
        "Edit" | "MultiEdit" | "NotebookEdit" => Kind::Edit,
        "Grep" => Kind::Search,
        "Glob" | "LS" => Kind::Find,
        "WebFetch" | "WebSearch" => Kind::Web,
        "Task" | "Agent" => Kind::Agent,
        "TodoWrite" => Kind::Todo,
        "ExitPlanMode" => Kind::Plan,
        name if name.starts_with("mcp__") => Kind::Mcp,
        _ => Kind::Other,
    }
}

/// One step of a run: a call or thinking, with what the fold needs to know of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Thinking,
    Call {
        /// The tool's name, as the agent called it.
        name: String,
        /// Its result is not in yet.
        running: bool,
        /// Its result was an error.
        failed: bool,
        /// It waits for the person's answer, or they denied it, or its result holds images: it stays in view.
        marked: bool,
    },
}

impl Step {
    /// A finished call that did not fail, with no mark.
    pub fn call(name: impl Into<String>) -> Step {
        Step::Call {
            name: name.into(),
            running: false,
            failed: false,
            marked: false,
        }
    }

    fn kind(&self) -> Option<Kind> {
        match self {
            Step::Thinking => None,
            Step::Call { name, .. } => Some(kind(name)),
        }
    }

    fn running(&self) -> bool {
        matches!(self, Step::Call { running: true, .. })
    }
}

/// A step that shows as its own row, never folded away (`important`).
pub fn important(step: &Step) -> bool {
    match step {
        Step::Thinking => false,
        Step::Call { name, marked, .. } => {
            *marked || matches!(kind(name), Kind::Plan | Kind::Edit | Kind::Write | Kind::Agent)
        }
    }
}

/// A part of a card: one step, or routine steps folded into one row. Indices are into the card's steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Step(usize),
    Fold(Range<usize>),
}

/// The steps in order: each important step on its own, the routine stretches between them folded (`segments`). A
/// stretch shorter than `min` shows its steps as themselves. `live`: the run is at the tail of a working session, so
/// its last step, still running, shows on its own.
pub fn segments(steps: &[Step], live: bool, min: usize) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut routine: Option<Range<usize>> = None;
    let flush = |routine: &mut Option<Range<usize>>, out: &mut Vec<Segment>| {
        if let Some(range) = routine.take() {
            if range.len() >= min {
                out.push(Segment::Fold(range));
            } else {
                out.extend(range.map(Segment::Step));
            }
        }
    };
    for (index, step) in steps.iter().enumerate() {
        let running = live && index + 1 == steps.len() && step.running();
        if important(step) || running {
            flush(&mut routine, &mut out);
            out.push(Segment::Step(index));
        } else {
            match &mut routine {
                Some(range) => range.end = index + 1,
                None => routine = Some(index..index + 1),
            }
        }
    }
    flush(&mut routine, &mut out);
    out
}

/// A stretch of routine steps shorter than this, beside an important step, shows its steps as rows of their own.
pub const FOLD_MIN: usize = 4;

/// How many parts an open card shows before "Show N earlier steps" (`RUN_TAIL` in `work-run.tsx`).
pub const RUN_TAIL: usize = 40;

/// A run of work as cards (`blocks`). Ranges are into the run's steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Card {
    /// Consecutive calls. A card of routine steps only (`important` false) is itself the fold: each step is a row,
    /// and it closes once its work is done. Next to an important step, a routine stretch of [`FOLD_MIN`] or more
    /// folds into one row. `parts` index the run's steps.
    Tools {
        steps: Range<usize>,
        parts: Vec<Segment>,
        important: bool,
    },
    /// Sub-agents started side by side.
    Agents(Range<usize>),
    /// The plan.
    Plan(usize),
}

/// The run's steps as cards. `live`: the run is at the tail of a working session.
pub fn cards(steps: &[Step], live: bool) -> Vec<Card> {
    let mut out = Vec::new();
    let mut tools: Option<Range<usize>> = None;
    let mut agents: Option<Range<usize>> = None;
    let flush_tools = |tools: &mut Option<Range<usize>>, out: &mut Vec<Card>| {
        let Some(range) = tools.take() else {
            return;
        };
        let run = &steps[range.clone()];
        // Only the run's last step can be the one running now, and only when it ends the whole run.
        let tail = live && range.end == steps.len();
        let marked = run.iter().any(important);
        let parts = if marked {
            segments(run, tail, FOLD_MIN)
                .into_iter()
                .map(|part| match part {
                    Segment::Step(at) => Segment::Step(range.start + at),
                    Segment::Fold(fold) => Segment::Fold(range.start + fold.start..range.start + fold.end),
                })
                .collect()
        } else {
            range.clone().map(Segment::Step).collect()
        };
        out.push(Card::Tools {
            steps: range,
            parts,
            important: marked,
        });
    };
    let flush_agents = |agents: &mut Option<Range<usize>>, out: &mut Vec<Card>| {
        if let Some(range) = agents.take() {
            out.push(Card::Agents(range));
        }
    };
    let grow = |range: &mut Option<Range<usize>>, at: usize| match range {
        Some(range) => range.end = at + 1,
        None => *range = Some(at..at + 1),
    };
    for (at, step) in steps.iter().enumerate() {
        match step.kind() {
            Some(Kind::Plan) => {
                flush_tools(&mut tools, &mut out);
                flush_agents(&mut agents, &mut out);
                out.push(Card::Plan(at));
            }
            Some(Kind::Agent) => {
                flush_tools(&mut tools, &mut out);
                grow(&mut agents, at);
            }
            _ => {
                flush_agents(&mut agents, &mut out);
                grow(&mut tools, at);
            }
        }
    }
    flush_tools(&mut tools, &mut out);
    flush_agents(&mut agents, &mut out);
    out
}

/// The phrases a group of counted steps reads as, for one and for many.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Explore,
    Command,
    Edit,
    Write,
    Search,
    Fetch,
    Web,
    Todo,
    Tool,
}

impl Group {
    fn of(name: &str) -> Group {
        if name == "WebFetch" {
            return Group::Fetch;
        }
        match kind(name) {
            Kind::Command => Group::Command,
            Kind::Read => Group::Explore,
            Kind::Edit => Group::Edit,
            Kind::Write => Group::Write,
            Kind::Search | Kind::Find => Group::Search,
            Kind::Web => Group::Web,
            Kind::Todo => Group::Todo,
            _ => Group::Tool,
        }
    }

    fn phrase(self, n: usize) -> String {
        let (one, many) = match self {
            Group::Explore => ("explored a file", "explored # files"),
            Group::Command => ("ran a command", "ran # commands"),
            Group::Edit => ("edited a file", "edited # files"),
            Group::Write => ("wrote a file", "wrote # files"),
            Group::Search => ("searched once", "searched # times"),
            Group::Fetch => ("fetched a page", "fetched # pages"),
            Group::Web => ("searched the web", "searched the web # times"),
            Group::Todo => ("updated the plan", "updated the plan # times"),
            Group::Tool => ("used a tool", "used # tools"),
        };
        if n == 1 {
            one.into()
        } else {
            many.replace('#', &n.to_string())
        }
    }
}

/// A fold's line, and a card's header (`foldSummary`, `toolsLabel`): "Explored 6 files, ran 2 commands". Exploring
/// leads, then each other kind in the order it first came. Reads and the searches beside them fold into "explored",
/// counted by the files read; searches with no read stay "searched 2 times". Thinking alone is "Thought".
pub fn fold_summary(steps: &[Step]) -> String {
    let mut counts: Vec<(Group, usize)> = Vec::new();
    for step in steps {
        let Step::Call { name, .. } = step else {
            continue;
        };
        let group = Group::of(name);
        match counts.iter_mut().find(|(seen, _)| *seen == group) {
            Some((_, n)) => *n += 1,
            None => counts.push((group, 1)),
        }
    }
    if counts.is_empty() {
        return "Thought".into();
    }
    if counts.iter().any(|(group, _)| *group == Group::Explore) {
        counts.retain(|(group, _)| *group != Group::Search);
    }
    // A stable sort: exploring first, the rest in their order.
    counts.sort_by_key(|(group, _)| *group != Group::Explore);
    let text = counts
        .iter()
        .map(|(group, n)| group.phrase(*n))
        .collect::<Vec<_>>()
        .join(", ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// How many of the steps failed (`foldFailures`).
pub fn fold_failures(steps: &[Step]) -> usize {
    steps
        .iter()
        .filter(|step| matches!(step, Step::Call { failed: true, .. }))
        .count()
}

/// A thinking step's line (`thinkingLabel`): "Thought for 14 s" once it took a second or more, else "Thought".
pub fn thinking_label(ms: Option<u64>) -> String {
    let ms = ms.unwrap_or(0);
    if ms < 1000 {
        return "Thought".into();
    }
    let s = (ms as f64 / 1000.).round() as u64;
    if s < 60 {
        format!("Thought for {s} s")
    } else {
        format!("Thought for {} min {} s", s / 60, s % 60)
    }
}

/// "1 step" or "6 steps", and "2 failed" when any did: a fold row's facts.
pub fn fold_facts(steps: &[Step]) -> String {
    let count = if steps.len() == 1 {
        "1 step".to_string()
    } else {
        format!("{} steps", steps.len())
    };
    match fold_failures(steps) {
        0 => count,
        failed => format!("{count} · {failed} failed"),
    }
}

/// "Show 1 earlier step" or "Show 12 earlier steps".
pub fn earlier_label(steps: usize) -> String {
    if steps == 1 {
        "Show 1 earlier step".into()
    } else {
        format!("Show {steps} earlier steps")
    }
}

#[cfg(test)]
mod tests;
