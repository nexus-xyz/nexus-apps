//! Agent mode in the session: ⇧tab switches between chat and agent, the agent's tool runs show as they happen, and an
//! edit or a command waits for the person's answer in place of the composer. A child of `app.rs`.

use super::super::agent::cells;
use super::super::agent::tools::{Action, Ask, Decision};
use super::super::agent::{Summary, Task};
use super::super::view;
use super::{Answer, App, Command, Key, Mode, Turn};
use crate::error::CliError;
use crate::style::usd;

impl App {
    /// ⇧tab: chat (Nexus chat sessions) or agent (tools in this folder). The next prompt goes to the mode chosen.
    pub(super) fn switch_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Chat => Mode::Agent,
            Mode::Agent => Mode::Chat,
        };
    }

    /// The agent asks before an edit or a command: answered at once when the person said "don't ask again".
    pub(super) fn ask(&mut self, ask: Ask) -> Option<Command> {
        let always = match &ask {
            Ask::Edit { .. } => self.always_edits,
            // The terminal session has no MCP servers; a tool would be asked as a command is.
            Ask::Run { .. } | Ask::Tool { .. } => self.always_commands,
        };
        if always {
            return Some(Command::Decide(Decision::Allow));
        }
        self.approval = Some((ask, 0));
        None
    }

    /// A key while an approval waits: ↑ ↓ and ⏎, 1 2 3, or y a n; esc is no.
    pub(super) fn approval_key(&mut self, key: &Key) -> Option<Command> {
        let (ask, selected) = self.approval.as_mut()?;
        let decision = match key {
            Key::Up => {
                *selected = selected.checked_sub(1).unwrap_or(2);
                return None;
            }
            Key::Down => {
                *selected = (*selected + 1) % 3;
                return None;
            }
            Key::Enter => [Decision::Allow, Decision::Always, Decision::Deny][(*selected).min(2)].clone(),
            Key::Char('1' | 'y' | 'Y') => Decision::Allow,
            Key::Char('2' | 'a' | 'A') => Decision::Always,
            Key::Char('3' | 'n' | 'N') | Key::Esc => Decision::Deny,
            _ => return None,
        };
        if decision == Decision::Always {
            match ask {
                Ask::Edit { .. } => self.always_edits = true,
                Ask::Run { .. } | Ask::Tool { .. } => self.always_commands = true,
            }
        }
        self.approval = None;
        Some(Command::Decide(decision))
    }

    /// A tool ran: what the agent said before it goes to the scrollback, then the tool's lines.
    pub(super) fn tool_ran(&mut self, action: &Action, width: usize) {
        // A subagent's transcript is open: the conversation holds the tool, and shows it on the way back.
        if self.viewing.is_some() {
            if let Some(turn) = &mut self.turn {
                turn.answer = Answer::default();
            }
            return;
        }
        if let Some(turn) = &mut self.turn {
            let (lines, _) = turn.answer.take(&self.theme, width, true);
            self.history.extend(lines);
            turn.answer = Answer::default();
        }
        self.history.extend(cells::tool_cell(&self.theme, action, width));
    }

    /// A subagent started, ran a tool, or finished: its lines go under its `Agent` card.
    pub(super) fn task_event(&mut self, id: &str, task: &Task, width: usize) {
        let open_here = self.track(id, task);
        if self.viewing.is_some() {
            if open_here {
                self.transcript_task(task, width);
            }
            if let Some(turn) = &mut self.turn {
                turn.answer = Answer::default();
            }
            return;
        }
        if let Some(turn) = &mut self.turn {
            let (lines, _) = turn.answer.take(&self.theme, width, true);
            self.history.extend(lines);
            turn.answer = Answer::default();
        }
        self.history.extend(cells::task_lines(&self.theme, task, width));
    }

    /// The turn ended: the last words, then the model, what every step cost together, and how long it took.
    pub(super) fn agent_finished(
        &mut self,
        mut turn: Turn,
        result: Result<Summary, CliError>,
        width: usize,
        now_ms: u64,
    ) {
        self.approval = None;
        // With a subagent's transcript open, the last words are in the conversation, drawn on the way back; the turn's
        // end waits in `held` until then.
        let viewing = self.viewing.is_some();
        if !viewing {
            let (lines, _) = turn.answer.take(&self.theme, width, true);
            if !lines.is_empty() {
                self.history.push(String::new());
            }
            self.history.extend(lines);
        }
        let before = self.history.len();
        match result {
            Ok(summary) if summary.stopped => {
                self.history.extend(view::stop_cell(
                    &self.theme,
                    "Stopped. Tell the agent what to do instead.",
                ));
            }
            Ok(summary) => {
                self.costs.push((
                    "agent".into(),
                    summary.model.clone().unwrap_or_default(),
                    summary.cost_usd,
                ));
                let steps = format!("{} step{}", summary.steps, if summary.steps == 1 { "" } else { "s" });
                self.history.push(view::answer_meta(
                    &self.theme,
                    &[
                        summary.model,
                        Some(usd(summary.cost_usd)),
                        Some(steps),
                        Some(view::elapsed(now_ms.saturating_sub(turn.started_ms))),
                    ],
                ));
            }
            Err(error) => self.history.extend(view::error_cell(&self.theme, &error, width)),
        }
        if viewing {
            let end = self.history.split_off(before);
            self.held.extend(end);
        }
    }
}
