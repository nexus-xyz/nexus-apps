//! The core's record of the tasks the agent reported in this run (its sub-agents and the commands it started), and the
//! one place that decides what the session's log says about them. Claude Code reports a sub-agent's progress every few
//! seconds and every command as a task. The log keeps a running task's latest state at most every 30 seconds (every 5
//! for a sub-agent whose own work the log never shows), and no row for a command in the foreground: its tool row shows
//! it already. A new process starts with an empty book.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use crate::event::{TaskDetail, TaskStatus, WbEvent};

/// How often a running task's progress reaches the log while its own tool calls do too.
const SEEN_EVERY: Duration = Duration::from_secs(30);
/// How often it does while nothing else in the log shows the task at work.
const HIDDEN_EVERY: Duration = Duration::from_secs(5);

/// What the log does with one report.
#[derive(Debug, PartialEq)]
pub enum Take {
    /// Append this row now.
    Write(Box<WbEvent>),
    /// Kept in the book: written later, or merged into the task's end.
    Hold,
    /// Never logged: a repeat, an older run's, an update of a task that is not running, or a foreground command's.
    Drop,
}

/// What the book knows of one task.
#[derive(Debug)]
struct Entry {
    status: TaskStatus,
    /// Its protocol has named the target that Stop needs.
    stop_ready: bool,
    /// Every run of it seen in this process; the last is the current one.
    runs: Vec<String>,
    /// A row of it is in the log. A command in the foreground has none until it moves to the background, and none
    /// after it ends there.
    logged: bool,
    /// A foreground command's start row, written if it moves to the background.
    unlogged: Option<WbEvent>,
    /// Its own tool calls reach the log.
    seen: bool,
    /// When its last row was written.
    written: Instant,
    /// Progress since its last row, not written yet.
    held: Held,
    /// Its latest counts, written or not: an end that leaves them out keeps them.
    tool_count: Option<u64>,
    total_tokens: Option<u64>,
    /// The model, once known, until a row carries it.
    model: Option<String>,
    /// Where it sits, from its first start: a re-run's start lacks it.
    parent_task_id: Option<String>,
    depth: Option<u32>,
    /// A sub-agent (not a command), and whether it runs past the call that started it.
    agent: bool,
    backgrounded: Option<bool>,
    /// What the last end written in this run said, to fill in and drop a repeat.
    ended: Option<Said>,
    /// What a control calls it: "agent" or "shell", else the agent's own task type.
    kind: String,
    /// Its title, from its start.
    title: String,
    /// When its current run started (ms since 1970).
    started_at: i64,
}

/// What an end says.
#[derive(Debug, Clone, PartialEq)]
struct Said {
    status: TaskStatus,
    description: String,
    tool_count: Option<u64>,
    total_tokens: Option<u64>,
    duration_ms: Option<u64>,
}

#[derive(Debug, Default)]
struct Held {
    activity: Option<String>,
    tool_count: Option<u64>,
    total_tokens: Option<u64>,
    /// It moved to the background (or back): written at once.
    backgrounded: Option<bool>,
}

impl Held {
    fn is_empty(&self) -> bool {
        self.activity.is_none()
            && self.tool_count.is_none()
            && self.total_tokens.is_none()
            && self.backgrounded.is_none()
    }
}

impl Entry {
    fn every(&self) -> Duration {
        if self.seen { SEEN_EVERY } else { HIDDEN_EVERY }
    }

    fn run(&self) -> Option<&str> {
        self.runs.last().map(String::as_str)
    }

    /// The held progress as a row, with the model while no row carried it.
    fn progress_row(&mut self, task_id: &str, now: Instant) -> WbEvent {
        let held = std::mem::take(&mut self.held);
        self.written = now;
        WbEvent::Task {
            task_id: task_id.into(),
            tool_use_id: None,
            task_type: None,
            agent_type: None,
            status: TaskStatus::Running,
            description: String::new(),
            tool_count: held.tool_count,
            total_tokens: held.total_tokens,
            duration_ms: None,
            detail: TaskDetail {
                model: self.model.take(),
                activity: held.activity,
                backgrounded: held.backgrounded,
                progress: Some(true),
                ..TaskDetail::default()
            }
            .some(),
        }
    }
}

/// The parts of a `task` event the book reads.
struct Report<'a> {
    task_id: &'a str,
    status: TaskStatus,
    run: Option<&'a str>,
    progress: bool,
}

fn report(event: &WbEvent) -> Option<Report<'_>> {
    let WbEvent::Task {
        task_id,
        status,
        detail,
        ..
    } = event
    else {
        return None;
    };
    Some(Report {
        task_id,
        status: *status,
        run: detail.as_ref().and_then(|d| d.run_id.as_deref()),
        progress: detail.as_ref().and_then(|d| d.progress) == Some(true),
    })
}

/// The tasks of one process.
#[derive(Debug, Default)]
pub struct TaskBook {
    tasks: BTreeMap<String, Entry>,
    /// Task ids by the tool call that started them.
    calls: HashMap<String, String>,
}

impl TaskBook {
    /// One report: a driver's `task` event, or its update of a running task (`detail.progress`).
    pub fn take(&mut self, event: WbEvent, now: Instant) -> Take {
        let Some(r) = report(&event) else {
            return Take::Write(Box::new(event));
        };
        if r.progress {
            self.update(event, now)
        } else if r.status == TaskStatus::Running {
            self.start(event, now)
        } else {
            self.end(event, now)
        }
    }

    fn start(&mut self, mut event: WbEvent, now: Instant) -> Take {
        let WbEvent::Task {
            task_id,
            tool_use_id,
            task_type,
            description,
            detail,
            ..
        } = &mut event
        else {
            return Take::Drop;
        };
        let run = detail.as_ref().and_then(|d| d.run_id.clone());
        let kind = match task_type.as_deref() {
            Some("local_agent") | None => "agent".to_string(),
            Some("local_bash") => "shell".to_string(),
            Some(other) => other.to_string(),
        };
        let title = description.clone();
        if let Some(call) = tool_use_id {
            self.calls.insert(call.clone(), task_id.clone());
        }
        let command =
            task_type.as_deref() == Some("local_bash") && detail.as_ref().and_then(|d| d.backgrounded) == Some(false);
        let task_id = task_id.clone();
        if let Some(entry) = self.tasks.get_mut(&task_id) {
            // A run already seen: a repeat. A start with no run (an older agent) is taken, as before.
            if run.as_ref().is_some_and(|run| entry.runs.contains(run)) {
                return Take::Drop;
            }
            // Only what the book knows: a re-run of a task that never said where it sits keeps no detail.
            let mut place = detail.take().map(|d| *d).unwrap_or_default();
            place.parent_task_id = place.parent_task_id.or_else(|| entry.parent_task_id.clone());
            place.depth = place.depth.or(entry.depth);
            *detail = place.some();
        }
        let place = detail.as_deref();
        let entry = Entry {
            status: TaskStatus::Running,
            stop_ready: false,
            runs: run.into_iter().collect(),
            logged: !command,
            unlogged: None,
            seen: false,
            written: now,
            held: Held::default(),
            tool_count: None,
            total_tokens: None,
            model: None,
            parent_task_id: place.and_then(|d| d.parent_task_id.clone()),
            depth: place.and_then(|d| d.depth),
            agent: task_type.as_deref() != Some("local_bash"),
            backgrounded: place.and_then(|d| d.backgrounded),
            ended: None,
            kind,
            title,
            started_at: crate::now_ms(),
        };
        let entry = match self.tasks.remove(&task_id) {
            Some(old) => Entry {
                runs: old.runs.into_iter().chain(entry.runs).collect(),
                seen: old.seen,
                // A re-run's start may not say it again.
                title: if entry.title.is_empty() { old.title } else { entry.title },
                ..entry
            },
            None => entry,
        };
        let entry = self.tasks.entry(task_id).or_insert(entry);
        if command {
            entry.unlogged = Some(event);
            return Take::Drop;
        }
        Take::Write(Box::new(event))
    }

    fn update(&mut self, event: WbEvent, now: Instant) -> Take {
        let Some(r) = report(&event) else {
            return Take::Drop;
        };
        let Some(entry) = self.tasks.get_mut(r.task_id) else {
            return Take::Drop;
        };
        if entry.status != TaskStatus::Running || r.run.is_some_and(|run| entry.run().is_some_and(|ours| ours != run)) {
            return Take::Drop;
        }
        let WbEvent::Task {
            task_id,
            tool_count,
            total_tokens,
            detail,
            ..
        } = event
        else {
            return Take::Drop;
        };
        let detail = detail.map(|d| *d).unwrap_or_default();
        // A command moved to the background: its start row reaches the log now, and its end will.
        if detail.backgrounded == Some(true)
            && let Some(start) = entry.unlogged.take()
        {
            entry.written = now;
            entry.logged = true;
            let mut start = start;
            if let WbEvent::Task { detail, .. } = &mut start {
                detail.get_or_insert_with(Default::default).backgrounded = Some(true);
            }
            return Take::Write(Box::new(start));
        }
        if !entry.logged {
            return Take::Drop;
        }
        if detail.model.is_some() {
            entry.model = detail.model;
        }
        if detail.activity.is_some() {
            entry.held.activity = detail.activity;
        }
        if tool_count.is_some() {
            entry.held.tool_count = tool_count;
            entry.tool_count = tool_count;
        }
        if total_tokens.is_some() {
            entry.held.total_tokens = total_tokens;
            entry.total_tokens = total_tokens;
        }
        if detail.backgrounded.is_some() {
            entry.held.backgrounded = detail.backgrounded;
        }
        // The model alone never writes a row: it rides the task's next one.
        let due = now.duration_since(entry.written) >= entry.every();
        if entry.held.backgrounded.is_some() || (!entry.held.is_empty() && due) {
            return Take::Write(Box::new(entry.progress_row(&task_id, now)));
        }
        Take::Hold
    }

    fn end(&mut self, mut event: WbEvent, now: Instant) -> Take {
        let Some(r) = report(&event) else {
            return Take::Drop;
        };
        let Some(entry) = self.tasks.get_mut(r.task_id) else {
            // It started before this process: its end is written as it is.
            return Take::Write(Box::new(event));
        };
        if r.run.is_some_and(|run| entry.run().is_some_and(|ours| ours != run)) {
            return Take::Drop;
        }
        entry.status = r.status;
        if !entry.logged {
            entry.unlogged = None;
            return Take::Drop;
        }
        let WbEvent::Task {
            status,
            description,
            tool_count,
            total_tokens,
            duration_ms,
            detail,
            ..
        } = &mut event
        else {
            return Take::Drop;
        };
        entry.held = Held::default();
        *tool_count = tool_count.or(entry.tool_count);
        *total_tokens = total_tokens.or(entry.total_tokens);
        // Claude Code ends a task twice (`task_updated`, then `task_notification` with its summary and usage). The
        // second keeps what the first said and it does not, and is written only when it says more.
        if let Some(said) = &entry.ended {
            *tool_count = tool_count.or(said.tool_count);
            *total_tokens = total_tokens.or(said.total_tokens);
            *duration_ms = duration_ms.or(said.duration_ms);
            if description.is_empty() {
                description.clone_from(&said.description);
            }
        }
        let said = Said {
            status: *status,
            description: description.clone(),
            tool_count: *tool_count,
            total_tokens: *total_tokens,
            duration_ms: *duration_ms,
        };
        if entry.ended.as_ref() == Some(&said) {
            return Take::Drop;
        }
        if let Some(model) = entry.model.take() {
            detail.get_or_insert_with(Default::default).model = Some(model);
        }
        entry.ended = Some(said);
        entry.written = now;
        Take::Write(Box::new(event))
    }

    /// A tool call whose parent is `call` reached the log: that task's own work shows there.
    pub fn saw_work_of(&mut self, call: &str) {
        if let Some(entry) = self.calls.get(call).and_then(|task| self.tasks.get_mut(task)) {
            entry.seen = true;
        }
    }

    /// The result of `call` reached the log: a foreground command it started ends in the book, with no row.
    pub fn result_of(&mut self, call: &str) {
        if let Some(entry) = self.calls.get(call).and_then(|task| self.tasks.get_mut(task))
            && !entry.logged
            && entry.status == TaskStatus::Running
        {
            entry.status = TaskStatus::Completed;
            entry.unlogged = None;
        }
    }

    /// The task an owner names: a call id, or a task id (an approval from a sub-agent whose start was not read).
    fn task_of(&self, owner: &str) -> Option<&Entry> {
        self.calls
            .get(owner)
            .and_then(|task| self.tasks.get(task))
            .or_else(|| self.tasks.get(owner))
    }

    /// The call that started the task, when it is known.
    pub fn call_of(&self, task: &str) -> Option<&str> {
        self.calls
            .iter()
            .find(|(_, id)| id.as_str() == task)
            .map(|(call, _)| call.as_str())
    }

    /// A result of a booked task's call: that task's own (a Codex sub-agent's Agent call ends with its sub-agent), never
    /// the session's, wherever it lands.
    pub fn owns(&self, event: &WbEvent) -> bool {
        matches!(event, WbEvent::ToolResult { id, .. } if self.calls.contains_key(id))
    }

    /// An approval with this owner ends with the session's own turn: the session's own, and a foreground sub-agent the
    /// session's agent started (it runs inside that agent's call). A background sub-agent, a deeper one and an owner
    /// the book does not know keep theirs.
    pub fn ends_with_lead_turn(&self, owner: Option<&str>) -> bool {
        let Some(owner) = owner else {
            return true;
        };
        self.task_of(owner)
            .is_some_and(|task| task.agent && task.backgrounded == Some(false) && task.parent_task_id.is_none())
    }

    /// Every running task with a row in the log, ended `stopped` with `reason`: its process is gone. Its latest counts and
    /// the model no row carried yet go with it. A foreground command ends too, still with no row.
    pub fn end_all(&mut self, reason: &str) -> Vec<WbEvent> {
        let TaskBook { tasks, calls } = self;
        let call_of = |task: &str| {
            calls
                .iter()
                .find(|(_, id)| id.as_str() == task)
                .map(|(call, _)| call.clone())
        };
        let mut rows = Vec::new();
        for (task_id, entry) in tasks.iter_mut() {
            if entry.status != TaskStatus::Running {
                continue;
            }
            entry.status = TaskStatus::Stopped;
            entry.held = Held::default();
            if !entry.logged {
                entry.unlogged = None;
                continue;
            }
            entry.ended = Some(Said {
                status: TaskStatus::Stopped,
                description: String::new(),
                tool_count: entry.tool_count,
                total_tokens: entry.total_tokens,
                duration_ms: None,
            });
            rows.push(WbEvent::Task {
                task_id: task_id.clone(),
                tool_use_id: call_of(task_id),
                task_type: None,
                agent_type: None,
                status: TaskStatus::Stopped,
                description: String::new(),
                tool_count: entry.tool_count,
                total_tokens: entry.total_tokens,
                duration_ms: None,
                detail: TaskDetail {
                    model: entry.model.take(),
                    run_id: entry.run().map(String::from),
                    reason: Some(reason.into()),
                    ..TaskDetail::default()
                }
                .some(),
            });
        }
        rows
    }

    /// A running task with a row in the log, as a control needs it: its kind ("agent", "shell") and its title. None for
    /// a task the book does not know, one that ended, and a foreground command (its tool row is all the log has of it).
    pub fn running(&self, task: &str) -> Option<(&str, &str)> {
        self.tasks
            .get(task)
            .filter(|entry| entry.status == TaskStatus::Running && entry.logged)
            .map(|entry| (entry.kind.as_str(), entry.title.as_str()))
    }

    /// Any task the book knows, running or ended: its kind ("agent", "shell"), title and start. A Codex sub-agent the
    /// book saw end can still drain past its turn, and keeps its name by it.
    pub fn known(&self, task: &str) -> Option<(&str, &str, i64)> {
        self.tasks
            .get(task)
            .map(|entry| (entry.kind.as_str(), entry.title.as_str(), entry.started_at))
    }

    /// A command running in the foreground, by the tool call that started it, with its title: what can move to the
    /// background. None for a quick command Claude Code never reported, one that ended or moved already, and a sub-agent.
    pub fn foreground(&self, call: &str) -> Option<&str> {
        self.calls
            .get(call)
            .and_then(|task| self.tasks.get(task))
            .filter(|entry| entry.status == TaskStatus::Running && !entry.logged && entry.kind == "shell")
            .map(|entry| entry.title.as_str())
    }

    /// Tasks with a row in the log that still run: sub-agents and background commands, never a foreground command.
    pub fn running_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|entry| entry.status == TaskStatus::Running && entry.logged)
            .count()
    }

    /// Tasks with a row in the log that still run, oldest first: `(task id, kind, title, started at)`. Between turns
    /// these are what the agent left running in the background.
    pub fn running_tasks(&self) -> Vec<(&str, &str, &str, i64)> {
        let mut tasks: Vec<_> = self
            .tasks
            .iter()
            .filter(|(_, entry)| entry.status == TaskStatus::Running && entry.logged)
            .map(|(id, entry)| (id.as_str(), entry.kind.as_str(), entry.title.as_str(), entry.started_at))
            .collect();
        tasks.sort_by_key(|task| task.3);
        tasks
    }

    /// A sub-agent with a row in the log still runs: a linked session with one is not finished.
    pub fn agents_running(&self) -> bool {
        self.tasks
            .values()
            .any(|entry| entry.status == TaskStatus::Running && entry.logged && entry.kind == "agent")
    }

    /// The sub-agents with a row in the log that still run: what the spend cap's trip stops.
    /// Codex needs a named turn before it can stop one.
    pub fn running_agents(&self, require_ready: bool) -> Vec<String> {
        self.tasks
            .iter()
            .filter(|(_, entry)| {
                entry.status == TaskStatus::Running
                    && entry.logged
                    && entry.kind == "agent"
                    && (!require_ready || entry.stop_ready)
            })
            .map(|(task, _)| task.clone())
            .collect()
    }

    /// Marks a running task as stoppable once the protocol names its turn.
    pub fn task_ready(&mut self, task: &str) {
        if let Some(entry) = self
            .tasks
            .get_mut(task)
            .filter(|entry| entry.status == TaskStatus::Running)
        {
            entry.stop_ready = true;
        }
    }

    /// Held progress now due (`Some(now)`), or all of it (`None`, before the turn ends).
    pub fn due(&mut self, now: Option<Instant>) -> Vec<WbEvent> {
        let at = now.unwrap_or_else(Instant::now);
        self.tasks
            .iter_mut()
            .filter(|(_, entry)| {
                entry.status == TaskStatus::Running
                    && !entry.held.is_empty()
                    && now.is_none_or(|now| now.duration_since(entry.written) >= entry.every())
            })
            .map(|(task_id, entry)| entry.progress_row(task_id, at))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, secs: u64) -> Instant {
        start + Duration::from_secs(secs)
    }

    fn start(task: &str, call: &str, kind: &str, run: &str, backgrounded: bool) -> WbEvent {
        WbEvent::Task {
            task_id: task.into(),
            tool_use_id: Some(call.into()),
            task_type: Some(kind.into()),
            agent_type: None,
            status: TaskStatus::Running,
            description: "d".into(),
            tool_count: None,
            total_tokens: None,
            duration_ms: None,
            detail: TaskDetail {
                depth: Some(1),
                backgrounded: Some(backgrounded),
                run_id: Some(run.into()),
                ..TaskDetail::default()
            }
            .some(),
        }
    }

    #[test]
    fn a_stop_that_needs_a_turn_waits_until_the_task_names_one() {
        let mut book = TaskBook::default();
        let mut spawned = start("child", "call", "local_agent", "turn", true);
        if let WbEvent::Task {
            detail: Some(detail), ..
        } = &mut spawned
        {
            detail.run_id = None;
        }
        assert!(matches!(book.take(spawned, Instant::now()), Take::Write(_)));
        assert_eq!(book.running_agents(false), ["child"]);
        assert!(book.running_agents(true).is_empty());
        book.task_ready("child");
        assert_eq!(book.running_agents(true), ["child"]);
    }

    fn progress(task: &str, run: &str, activity: &str, tools: u64) -> WbEvent {
        WbEvent::Task {
            task_id: task.into(),
            tool_use_id: None,
            task_type: None,
            agent_type: None,
            status: TaskStatus::Running,
            description: String::new(),
            tool_count: Some(tools),
            total_tokens: Some(tools * 1000),
            duration_ms: None,
            detail: TaskDetail {
                activity: Some(activity.into()),
                run_id: Some(run.into()),
                progress: Some(true),
                ..TaskDetail::default()
            }
            .some(),
        }
    }

    fn update(task: &str, detail: TaskDetail) -> WbEvent {
        WbEvent::Task {
            task_id: task.into(),
            tool_use_id: None,
            task_type: None,
            agent_type: None,
            status: TaskStatus::Running,
            description: String::new(),
            tool_count: None,
            total_tokens: None,
            duration_ms: None,
            detail: TaskDetail {
                progress: Some(true),
                ..detail
            }
            .some(),
        }
    }

    fn end(task: &str, run: &str, status: TaskStatus, description: &str, tools: Option<u64>) -> WbEvent {
        WbEvent::Task {
            task_id: task.into(),
            tool_use_id: None,
            task_type: None,
            agent_type: None,
            status,
            description: description.into(),
            tool_count: tools,
            total_tokens: None,
            duration_ms: None,
            detail: TaskDetail {
                run_id: Some(run.into()),
                ..TaskDetail::default()
            }
            .some(),
        }
    }

    fn written(take: Take) -> WbEvent {
        match take {
            Take::Write(event) => *event,
            other => panic!("not written: {other:?}"),
        }
    }

    fn detail_of(event: &WbEvent) -> TaskDetail {
        match event {
            WbEvent::Task { detail, .. } => detail.as_deref().cloned().unwrap_or_default(),
            _ => TaskDetail::default(),
        }
    }

    #[test]
    fn a_start_is_written_once_per_run() {
        let (mut book, t) = (TaskBook::default(), Instant::now());
        assert!(matches!(
            book.take(start("a1", "c1", "local_agent", "r1", true), t),
            Take::Write(_)
        ));
        assert_eq!(book.take(start("a1", "c1", "local_agent", "r1", true), t), Take::Drop);
        written(book.take(end("a1", "r1", TaskStatus::Completed, "done", Some(2)), t));
        assert_eq!(book.take(start("a1", "c1", "local_agent", "r1", true), t), Take::Drop);
        // A new run is written, and keeps the first start's place where the re-run's start lacks it.
        let mut rerun = start("a1", "c1", "local_agent", "r2", true);
        if let WbEvent::Task { detail, .. } = &mut rerun {
            *detail = TaskDetail {
                run_id: Some("r2".into()),
                ..TaskDetail::default()
            }
            .some();
        }
        let row = written(book.take(rerun, t));
        assert_eq!(detail_of(&row).depth, Some(1));
        assert_eq!(detail_of(&row).run_id.as_deref(), Some("r2"));
    }

    #[test]
    fn a_foreground_command_logs_nothing_until_it_moves_to_the_background() {
        let (mut book, t) = (TaskBook::default(), Instant::now());
        assert_eq!(book.take(start("b1", "c1", "local_bash", "r1", false), t), Take::Drop);
        assert_eq!(book.take(progress("b1", "r1", "x", 1), at(t, 60)), Take::Drop);
        let moved = written(book.take(
            update(
                "b1",
                TaskDetail {
                    backgrounded: Some(true),
                    run_id: Some("r1".into()),
                    ..TaskDetail::default()
                },
            ),
            t,
        ));
        assert!(
            matches!(&moved, WbEvent::Task { tool_use_id: Some(call), status: TaskStatus::Running, .. } if call == "c1")
        );
        assert_eq!(detail_of(&moved).backgrounded, Some(true));
        written(book.take(end("b1", "r1", TaskStatus::Completed, "ok", None), t));
        // One that stays in the foreground ends with no row, by its end or by its result.
        assert_eq!(book.take(start("b2", "c2", "local_bash", "r2", false), t), Take::Drop);
        assert_eq!(book.take(end("b2", "r2", TaskStatus::Stopped, "", None), t), Take::Drop);
        // Claude Code ends a stopped command twice; the second end says more and is still not logged.
        assert_eq!(
            book.take(end("b2", "r2", TaskStatus::Stopped, "Command stopped", None), t),
            Take::Drop
        );
        assert_eq!(book.take(start("b3", "c3", "local_bash", "r3", false), t), Take::Drop);
        book.result_of("c3");
        assert_eq!(book.take(progress("b3", "r3", "x", 1), at(t, 60)), Take::Drop);
        assert_eq!(
            book.take(end("b3", "r3", TaskStatus::Completed, "done", None), t),
            Take::Drop
        );
        // A background command is written as any task.
        assert!(matches!(
            book.take(start("b4", "c4", "local_bash", "r4", true), t),
            Take::Write(_)
        ));
    }

    #[test]
    fn progress_is_written_once_per_interval() {
        let (mut book, t) = (TaskBook::default(), Instant::now());
        written(book.take(start("a1", "c1", "local_agent", "r1", true), t));
        // Hidden: every 5 seconds. The start counts as a row.
        assert_eq!(book.take(progress("a1", "r1", "one", 1), at(t, 1)), Take::Hold);
        assert_eq!(book.take(progress("a1", "r1", "two", 2), at(t, 4)), Take::Hold);
        let row = written(book.take(progress("a1", "r1", "three", 3), at(t, 5)));
        assert_eq!(detail_of(&row).activity.as_deref(), Some("three"));
        assert!(matches!(
            row,
            WbEvent::Task {
                tool_count: Some(3),
                tool_use_id: None,
                task_type: None,
                ..
            }
        ));
        // Its work shows in the log: every 30 seconds.
        book.saw_work_of("c1");
        assert_eq!(book.take(progress("a1", "r1", "four", 4), at(t, 20)), Take::Hold);
        assert_eq!(book.due(Some(at(t, 34))), []);
        assert_eq!(book.due(Some(at(t, 35))).len(), 1);
        // Held progress goes out before the turn ends, however recent.
        assert_eq!(book.take(progress("a1", "r1", "five", 5), at(t, 36)), Take::Hold);
        assert_eq!(book.due(None).len(), 1);
        assert_eq!(book.due(None), []);
    }

    #[test]
    fn the_model_rides_the_next_row() {
        let (mut book, t) = (TaskBook::default(), Instant::now());
        written(book.take(start("a1", "c1", "local_agent", "r1", true), t));
        let model = TaskDetail {
            model: Some("m".into()),
            ..TaskDetail::default()
        };
        assert_eq!(book.take(update("a1", model.clone()), at(t, 60)), Take::Hold);
        let row = written(book.take(progress("a1", "r1", "x", 1), at(t, 61)));
        assert_eq!(detail_of(&row).model.as_deref(), Some("m"));
        // Or its end, when no progress came.
        written(book.take(start("a2", "c2", "local_agent", "r2", true), t));
        assert_eq!(book.take(update("a2", model), t), Take::Hold);
        let row = written(book.take(end("a2", "r2", TaskStatus::Completed, "", None), t));
        assert_eq!(detail_of(&row).model.as_deref(), Some("m"));
    }

    #[test]
    fn an_end_merges_held_progress_and_drops_what_comes_after() {
        let (mut book, t) = (TaskBook::default(), Instant::now());
        written(book.take(start("a1", "c1", "local_agent", "r1", true), t));
        assert_eq!(book.take(progress("a1", "r1", "x", 4), at(t, 1)), Take::Hold);
        let first = written(book.take(end("a1", "r1", TaskStatus::Stopped, "", None), at(t, 2)));
        assert!(matches!(
            first,
            WbEvent::Task {
                tool_count: Some(4),
                ..
            }
        ));
        // The same end again is a repeat; one that says more is written.
        assert_eq!(
            book.take(end("a1", "r1", TaskStatus::Stopped, "", Some(4)), at(t, 2)),
            Take::Drop
        );
        let more = written(book.take(end("a1", "r1", TaskStatus::Stopped, "summary", None), at(t, 3)));
        assert!(
            matches!(
                more,
                WbEvent::Task {
                    tool_count: Some(4),
                    total_tokens: Some(4000),
                    ..
                }
            ),
            "{more:?}"
        );
        assert_eq!(book.take(progress("a1", "r1", "late", 6), at(t, 60)), Take::Drop);
        assert_eq!(book.due(None), []);
        // An end of an older run is dropped; one of a task this process never saw is written.
        written(book.take(start("a1", "c1", "local_agent", "r2", true), t));
        assert_eq!(book.take(end("a1", "r1", TaskStatus::Failed, "", None), t), Take::Drop);
        written(book.take(end("zz", "r9", TaskStatus::Completed, "", None), t));
    }

    #[test]
    fn a_background_move_is_written_at_once() {
        let (mut book, t) = (TaskBook::default(), Instant::now());
        written(book.take(start("a1", "c1", "local_agent", "r1", false), t));
        let row = written(book.take(
            update(
                "a1",
                TaskDetail {
                    backgrounded: Some(true),
                    ..TaskDetail::default()
                },
            ),
            at(t, 1),
        ));
        assert_eq!(detail_of(&row).backgrounded, Some(true));
        assert_eq!(book.take(progress("a1", "r1", "x", 1), at(t, 2)), Take::Hold);
    }

    #[test]
    fn a_re_run_of_a_task_that_never_said_where_it_sits_keeps_no_detail() {
        let (mut book, t) = (TaskBook::default(), Instant::now());
        let bare = |status| WbEvent::Task {
            task_id: "thr_c".into(),
            tool_use_id: Some("call_1".into()),
            task_type: Some("local_agent".into()),
            agent_type: None,
            status,
            description: String::new(),
            tool_count: None,
            total_tokens: None,
            duration_ms: None,
            detail: None,
        };
        written(book.take(bare(TaskStatus::Running), t));
        written(book.take(bare(TaskStatus::Completed), t));
        let rerun = written(book.take(bare(TaskStatus::Running), t));
        assert!(matches!(rerun, WbEvent::Task { detail: None, .. }), "{rerun:?}");
    }

    #[test]
    fn approvals_end_with_the_lead_turn_only_for_the_session_and_its_foreground_agents() {
        let (mut book, t) = (TaskBook::default(), Instant::now());
        written(book.take(start("bg", "c-bg", "local_agent", "r1", true), t));
        written(book.take(start("fg", "c-fg", "local_agent", "r2", false), t));
        let mut deep = start("deep", "c-deep", "local_agent", "r3", false);
        if let WbEvent::Task { detail: Some(d), .. } = &mut deep {
            d.parent_task_id = Some("bg".into());
        }
        written(book.take(deep, t));
        assert!(book.ends_with_lead_turn(None));
        assert!(book.ends_with_lead_turn(Some("c-fg")));
        // An approval names a sub-agent whose start was not read by its task id.
        assert!(book.ends_with_lead_turn(Some("fg")));
        assert!(!book.ends_with_lead_turn(Some("c-bg")));
        assert!(!book.ends_with_lead_turn(Some("c-deep")));
        assert!(!book.ends_with_lead_turn(Some("unknown")));
        assert_eq!(book.call_of("fg"), Some("c-fg"));
        let result = |id: &str| WbEvent::ToolResult {
            images: vec![],
            id: id.into(),
            output: String::new(),
            is_error: false,
            truncated: false,
            parent: None,
        };
        assert!(book.owns(&result("c-bg")));
        assert!(!book.owns(&result("lead-call")));
    }

    #[test]
    fn a_process_end_stops_every_logged_task_once() {
        let (mut book, t) = (TaskBook::default(), Instant::now());
        written(book.take(start("a1", "c1", "local_agent", "r1", true), t));
        written(book.take(start("b1", "c2", "local_bash", "r2", true), t));
        assert_eq!(book.take(start("b2", "c3", "local_bash", "r3", false), t), Take::Drop);
        written(book.take(start("a2", "c4", "local_agent", "r4", true), t));
        written(book.take(end("a2", "r4", TaskStatus::Completed, "done", Some(1)), t));
        assert_eq!(book.take(progress("a1", "r1", "x", 4), at(t, 1)), Take::Hold);
        assert_eq!(book.running_count(), 2);
        let rows = book.end_all("process_ended");
        let ended: Vec<(&str, Option<&str>, Option<u64>)> = rows
            .iter()
            .filter_map(|row| match row {
                WbEvent::Task {
                    task_id,
                    tool_use_id,
                    status: TaskStatus::Stopped,
                    tool_count,
                    ..
                } => Some((task_id.as_str(), tool_use_id.as_deref(), *tool_count)),
                _ => None,
            })
            .collect();
        assert_eq!(ended, [("a1", Some("c1"), Some(4)), ("b1", Some("c2"), None)]);
        assert!(
            rows.iter()
                .all(|row| detail_of(row).reason.as_deref() == Some("process_ended"))
        );
        assert_eq!(detail_of(&rows[0]).run_id.as_deref(), Some("r1"));
        assert_eq!(book.running_count(), 0);
        assert_eq!(book.end_all("process_ended"), []);
        // A report that comes after is no new end.
        assert_eq!(
            book.take(end("a1", "r1", TaskStatus::Stopped, "", Some(4)), t),
            Take::Drop
        );
    }

    /// What a stop may ask about: a logged task that runs, with its kind and title. Not a foreground command, an ended
    /// task, or one the book never saw. A re-run keeps its title when its start leaves it out.
    #[test]
    fn a_control_sees_only_logged_running_tasks() {
        let (mut book, t) = (TaskBook::default(), Instant::now());
        written(book.take(start("a1", "c1", "local_agent", "r1", true), t));
        written(book.take(start("b1", "c2", "local_bash", "r2", true), t));
        assert_eq!(book.take(start("b2", "c3", "local_bash", "r3", false), t), Take::Drop);
        written(book.take(start("m1", "c4", "monitor", "r4", true), t));
        assert_eq!(book.running("a1"), Some(("agent", "d")));
        assert_eq!(book.running("b1"), Some(("shell", "d")));
        assert_eq!(book.running("m1"), Some(("monitor", "d")));
        assert_eq!(book.running("b2"), None);
        assert_eq!(book.running("nope"), None);
        written(book.take(end("a1", "r1", TaskStatus::Completed, "done", None), t));
        assert_eq!(book.running("a1"), None);
        let mut rerun = start("a1", "c1", "local_agent", "r5", true);
        if let WbEvent::Task { description, .. } = &mut rerun {
            description.clear();
        }
        written(book.take(rerun, t));
        assert_eq!(book.running("a1"), Some(("agent", "d")));
    }

    /// What can move to the background: a reported foreground command, by its call, until it moves or ends. Not a
    /// background command, a sub-agent, or a call the book never saw.
    #[test]
    fn only_a_running_foreground_command_can_move() {
        let (mut book, t) = (TaskBook::default(), Instant::now());
        assert_eq!(book.take(start("b1", "c1", "local_bash", "r1", false), t), Take::Drop);
        written(book.take(start("b2", "c2", "local_bash", "r2", true), t));
        written(book.take(start("a1", "c3", "local_agent", "r3", false), t));
        assert_eq!(book.foreground("c1"), Some("d"));
        assert_eq!(book.foreground("c2"), None);
        assert_eq!(book.foreground("c3"), None);
        assert_eq!(book.foreground("nope"), None);
        let moved = update(
            "b1",
            TaskDetail {
                backgrounded: Some(true),
                run_id: Some("r1".into()),
                ..TaskDetail::default()
            },
        );
        written(book.take(moved, t));
        assert_eq!(book.foreground("c1"), None);
        assert_eq!(book.take(start("b3", "c4", "local_bash", "r4", false), t), Take::Drop);
        book.result_of("c4");
        assert_eq!(book.foreground("c4"), None);
    }

    #[test]
    fn other_events_pass_through() {
        let event = WbEvent::Commands { commands: vec![] };
        assert_eq!(
            TaskBook::default().take(event.clone(), Instant::now()),
            Take::Write(Box::new(event))
        );
    }
}
