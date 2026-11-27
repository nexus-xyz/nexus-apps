//! The session's agents, as Claude Code shows them: a panel under the composer with `main` and each
//! subagent of the conversation, its state and how many tools it ran. ↓ on an empty composer moves into it and cycles;
//! ↑ from `main` goes back to the composer. Enter opens a subagent's transcript in place of the conversation (its
//! prompt, its tools as cards, its report; a running one's tools as they come), and Esc or ← draws `main` again.
//!
//! While a transcript is open, the main turn goes on unseen: what it says is in its conversation, which is drawn again
//! on the way back, and its end (the model, the cost) waits in `held` until then.

use serde_json::Value;

use super::super::agent::sessions::Subagent;
use super::super::agent::tools::Action;
use super::super::agent::{Task, cells};
use super::{App, Command, Key};

/// One subagent of the conversation.
#[derive(Debug, Clone, Default)]
pub struct Row {
    /// The turn's id for it (`agent-1`), while it runs; empty for one of a resumed conversation.
    pub id: String,
    pub description: String,
    pub prompt: String,
    pub running: bool,
    pub error: bool,
    /// The tools it ran, as they came; once it ends, its own conversation holds them.
    pub tools: Vec<Action>,
    pub messages: Vec<Value>,
}

impl Row {
    fn tool_count(&self) -> usize {
        if self.messages.is_empty() {
            return self.tools.len();
        }
        self.messages
            .iter()
            .filter_map(|message| message.get("tool_calls").and_then(Value::as_array))
            .map(Vec::len)
            .sum()
    }
}

impl From<Subagent> for Row {
    fn from(agent: Subagent) -> Row {
        Row {
            description: agent.description,
            prompt: agent.prompt,
            error: agent.error,
            messages: agent.messages,
            ..Row::default()
        }
    }
}

impl App {
    /// A conversation with none: a new session, a chat session, another agent conversation before its own are set.
    pub(super) fn reset_agents(&mut self) {
        self.agents.clear();
        self.panel = None;
        self.viewing = None;
        self.held.clear();
    }

    pub(super) fn set_agents(&mut self, saved: Vec<Subagent>) {
        self.agents = saved.into_iter().map(Row::from).collect();
    }

    /// A subagent's report from the turn: kept in its row. Answers whether its transcript is the one open.
    pub(super) fn track(&mut self, id: &str, task: &Task) -> bool {
        let at = match task {
            Task::Started {
                description, prompt, ..
            } => {
                self.agents.push(Row {
                    id: id.to_string(),
                    description: description.clone(),
                    prompt: prompt.clone(),
                    running: true,
                    ..Row::default()
                });
                self.agents.len() - 1
            }
            _ => match self.agents.iter().rposition(|row| row.running && row.id == id) {
                Some(at) => at,
                None => return false,
            },
        };
        let row = &mut self.agents[at];
        match task {
            Task::Tool(action) => row.tools.push(action.clone()),
            Task::Finished { error, messages, .. } => {
                row.running = false;
                row.error = *error;
                row.messages = messages.clone();
            }
            Task::Started { .. } => {}
        }
        self.viewing == Some(at)
    }

    /// ↓ on an empty composer: into the panel, on `main`.
    pub(super) fn enter_panel(&mut self) -> bool {
        if self.agents.is_empty() {
            return false;
        }
        self.panel = Some(0);
        true
    }

    /// A key while the panel is focused: ↓ ↑ cycle, ↑ from `main` and esc leave it, ⏎ opens; typing goes back to the
    /// composer.
    pub(super) fn panel_key(&mut self, key: &Key, width: usize) -> Option<Command> {
        let rows = self.agents.len() + 1;
        let at = self.panel.unwrap_or(0);
        match key {
            Key::Down => self.panel = Some((at + 1) % rows),
            Key::Up if at == 0 => self.panel = None,
            Key::Up => self.panel = Some(at - 1),
            Key::Esc => self.panel = None,
            Key::Enter if at == 0 => self.panel = None,
            Key::Enter => self.open_transcript(at - 1, width),
            Key::Char(c) => {
                self.panel = None;
                self.composer.insert(&c.to_string());
            }
            _ => {}
        }
        None
    }

    /// A subagent's transcript, in place of the conversation.
    fn open_transcript(&mut self, index: usize, width: usize) {
        self.panel = None;
        self.viewing = Some(index);
        self.show_only_this_session();
        let row = self.agents[index].clone();
        let status = if row.running {
            "running"
        } else if row.error {
            "failed"
        } else {
            "done"
        };
        self.notice(&format!("Agent · {} · {status}", row.description));
        if row.messages.is_empty() {
            // Still running: its prompt, and its tools as they came.
            let prompt = super::super::view::prompt_cell(&self.theme, &row.prompt, width);
            self.history.extend(prompt);
            self.history.push(String::new());
            for action in &row.tools {
                let lines = cells::tool_cell(&self.theme, action, width);
                self.history.extend(lines);
            }
        } else {
            let lines = self.conversation_lines(&row.messages, width);
            self.history.extend(lines);
        }
    }

    /// A key while a transcript is open: esc or ← go back to `main`; typing goes back too, into the composer.
    pub(super) fn transcript_key(&mut self, key: &Key) -> Option<Command> {
        match key {
            Key::Esc | Key::Left | Key::CtrlC => {}
            Key::Char(c) => self.composer.insert(&c.to_string()),
            _ => return None,
        }
        self.viewing = None;
        Some(Command::ShowMain)
    }

    /// Lines for the transcript open now, from its subagent's report.
    pub(super) fn transcript_task(&mut self, task: &Task, width: usize) {
        match task {
            Task::Tool(action) => {
                let lines = cells::tool_cell(&self.theme, action, width);
                self.history.extend(lines);
            }
            Task::Finished { .. } => {
                let lines = cells::task_lines(&self.theme, task, width);
                self.history.extend(lines);
            }
            Task::Started { .. } => {}
        }
    }

    /// Back on `main`: its conversation drawn again, and the turn's end if it came meanwhile. A turn still running
    /// shows its answer so far again from its start.
    pub(super) fn main_view(&mut self, messages: &[Value], width: usize) {
        self.show_only_this_session();
        let lines = self.conversation_lines(messages, width);
        self.history.extend(lines);
        self.history.append(&mut self.held);
        if let Some(turn) = &mut self.turn {
            turn.answer.shown = 0;
            turn.answer.started = false;
        }
    }

    /// The panel's lines, under the composer: `main`, then each subagent with its state and tools.
    pub(super) fn panel_lines(&self, width: usize) -> Vec<String> {
        if self.agents.is_empty() || self.viewing.is_some() {
            return Vec::new();
        }
        let style = &self.theme.style;
        let fit = |line: String| crate::style::truncate(&line, width.saturating_sub(1), style.g.ellipsis);
        let mark = |at: usize| {
            if self.panel == Some(at) {
                style.accent(style.g.select)
            } else {
                " ".to_string()
            }
        };
        let hint = if self.panel.is_none() {
            style.dim(&format!(
                "  {} to view the agents",
                if style.unicode { "↓" } else { "down" }
            ))
        } else {
            style.dim("  ⏎ to open · esc to leave")
        };
        let main = if self.panel == Some(0) {
            style.accent("main")
        } else {
            "main".to_string()
        };
        let mut lines = vec![fit(format!("  {} {main}{hint}", mark(0)))];
        for (index, row) in self.agents.iter().enumerate() {
            let (glyph, state) = if row.running {
                (style.accent(if style.unicode { "◐" } else { "~" }), "running")
            } else if row.error {
                (style.red(if style.unicode { "✕" } else { "x" }), "failed")
            } else {
                (style.green(if style.unicode { "✓" } else { "v" }), "done")
            };
            let tools = row.tool_count();
            let noun = if tools == 1 { "tool" } else { "tools" };
            let name = if self.panel == Some(index + 1) {
                style.accent(&row.description)
            } else {
                row.description.clone()
            };
            lines.push(fit(format!(
                "  {}   {glyph} {name} {}",
                mark(index + 1),
                style.dim(&format!("· {state} · {tools} {noun}"))
            )));
        }
        lines
    }
}
