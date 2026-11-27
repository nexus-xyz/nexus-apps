//! What each slash command does in the session, the lists `/model` and `/resume` open, and what their Gateway reads
//! bring back. A child of `app.rs`, so it works on the session's own state.

use serde_json::Value;

use crate::error::CliError;

use crate::json::{a, js_string, n, s};
use crate::style::{pairs, usd};

use super::super::commands::{self, Slash};
use super::super::list::{Item, List, Outcome, Purpose};
use super::super::suggest;
use super::super::turn::{Load, TurnEvent};
use super::super::view;
use super::{Answer, App, Command, Key};

impl App {
    /// Run `slash` with its arguments.
    pub(super) fn run(&mut self, slash: Slash, args: String) -> Option<Command> {
        match slash {
            Slash::Model if !args.is_empty() => self.use_model(&args),
            Slash::Model => {
                return Some(Command::Load(Load::Models {
                    current: self.requested.clone(),
                }));
            }
            Slash::New => {
                self.show_only_this_session();
                self.reset_agents();
                self.session = None;
                self.fresh = true;
                self.suggestion = None;
                self.notice("New session. Your next prompt starts it.");
                return Some(Command::Reset);
            }
            Slash::Resume => {
                return Some(Command::Load(Load::Sessions {
                    saved: Vec::new(),
                    current: self.session.clone(),
                }));
            }
            Slash::Status => {
                let mut rows = self.about.clone();
                rows.push((
                    "Mode".into(),
                    match self.mode {
                        super::Mode::Chat => "chat (⇧tab switches chat and agent)".into(),
                        super::Mode::Agent => "agent: tools in this folder".into(),
                    },
                ));
                rows.push(("Approvals".into(), self.approvals().into()));
                rows.push(("Model".into(), self.model.clone()));
                rows.push((
                    "Session".into(),
                    self.session
                        .clone()
                        .unwrap_or_else(|| "new: your next prompt starts it".into()),
                ));
                rows.push(("Budget".into(), self.budget.clone().unwrap_or_else(|| "unknown".into())));
                rows.push(("Suggestions".into(), if self.suggestions { "on" } else { "off" }.into()));
                self.history.push(String::new());
                self.history.extend(pairs(&self.theme.style, &rows));
            }
            Slash::Spend => return Some(Command::Load(Load::Spend)),
            Slash::Suggestions => {
                let on = match args.to_lowercase().as_str() {
                    "on" => true,
                    "off" => false,
                    _ => !self.suggestions,
                };
                self.suggestions = on;
                if !on {
                    self.suggestion = None;
                }
                self.notice(if on {
                    "Suggested prompts are on: after each answer, a likely next prompt waits in the composer. /suggestions off stops them."
                } else {
                    "Suggested prompts are off."
                });
                return Some(Command::SaveSuggestions(on));
            }
            Slash::Approvals => match args.to_lowercase().as_str() {
                "" => {
                    let current = self.approvals();
                    let item = |id: &str, label: &str, detail: &str| Item {
                        id: id.into(),
                        label: label.into(),
                        detail: detail.into(),
                        current: current == id,
                        ..Item::default()
                    };
                    let items = vec![
                        item("ask", "Ask", "edits and commands wait for your yes"),
                        item("auto-edit", "Auto-edit", "edits run without asking; commands ask"),
                        item("auto", "Auto", "edits and commands run without asking"),
                    ];
                    self.list = Some(List::new(
                        Purpose::Approvals,
                        "What may the agent do without asking?",
                        items,
                    ));
                }
                level => self.set_approvals(level),
            },
            Slash::Diff => return Some(Command::Diff),
            Slash::Compact if self.mode != super::Mode::Agent => {
                self.notice(
                    "/compact works on the agent's conversation (⇧tab to agent mode); chat sessions live in Nexus.",
                );
            }
            Slash::Compact if self.running() => self.notice("/compact waits until the agent's turn ends."),
            Slash::Compact => {
                self.notice("Summarizing the agent's conversation…");
                return Some(Command::Compact {
                    model: self.requested.clone(),
                });
            }
            Slash::Cost => self.show_costs(),
            Slash::Help => self.history.extend(commands::help(&self.theme)),
            Slash::Btw => return self.btw(args),
            Slash::Quit => self.quit = true,
        }
        None
    }

    /// `/cost`: each turn with its model and cost, then the total.
    fn show_costs(&mut self) {
        if self.costs.is_empty() && self.spent == 0.0 {
            self.notice("Nothing spent in this session yet.");
            return;
        }
        let style = self.theme.style.clone();
        self.history.push(String::new());
        self.history.push(format!("  {}", style.bold("Spent in this session")));
        for (index, (what, model, cost)) in self.costs.iter().enumerate() {
            let model = if model.is_empty() {
                String::new()
            } else {
                format!(" {} {model}", style.g.bullet)
            };
            self.history.push(format!(
                "  {:>3}. {what}{}  {}",
                index + 1,
                style.dim(&model),
                usd(*cost)
            ));
        }
        self.history
            .push(format!("  {}  {}", style.bold("Total"), style.bold(&usd(self.spent))));
    }

    /// `/diff`: what git sees changed in this folder, the diff coloured.
    pub(super) fn show_diff(&mut self, diff: Result<(Vec<String>, Vec<String>), String>, width: usize) {
        match diff {
            Err(error) => self.history.extend(view::error_cell(
                &self.theme,
                &crate::error::CliError::new(error),
                width,
            )),
            Ok((status, _)) if status.is_empty() => self.notice("No changes in this folder."),
            Ok((status, diff)) => {
                self.notice(&format!("{} changed in this folder", status.len()));
                for line in status {
                    self.history.push(format!("    {}", self.theme.style.dim(&line)));
                }
                self.history
                    .extend(super::super::agent::cells::long_diff(&self.theme, &diff, width));
            }
        }
    }

    /// `/compact` is done: how much smaller the conversation is, and what the summary cost.
    pub(super) fn compacted(&mut self, result: Result<(usize, usize, f64), crate::error::CliError>, width: usize) {
        match result {
            Ok((before, after, cost)) => {
                self.spent += cost;
                self.costs.push(("compact".into(), String::new(), cost));
                // About four characters a token.
                self.notice(&format!(
                    "Compacted the agent's conversation: about {} tokens down to {}.",
                    before / 4,
                    after / 4
                ));
            }
            Err(error) => self.history.extend(view::error_cell(&self.theme, &error, width)),
        }
    }

    /// The agent's approval level: `ask`, `auto-edit` or `auto`.
    pub(super) fn approvals(&self) -> &'static str {
        match (self.always_edits, self.always_commands) {
            (true, true) => "auto",
            (true, false) => "auto-edit",
            _ => "ask",
        }
    }

    fn set_approvals(&mut self, level: &str) {
        let (edits, commands, note) = match level {
            "ask" => (false, false, "The agent asks before every edit and every command."),
            "auto-edit" | "edits" => (
                true,
                false,
                "Auto-edit: the agent edits files without asking, and asks before commands.",
            ),
            "auto" => (
                true,
                true,
                "Auto: the agent edits files and runs commands without asking, with your permissions. /approvals ask \
                 brings the questions back.",
            ),
            other => {
                let error = crate::error::CliError::hint(
                    format!("There is no approval level {other}."),
                    "Use ask, auto-edit or auto.",
                );
                self.history.extend(view::error_cell(&self.theme, &error, 80));
                return;
            }
        };
        self.always_edits = edits;
        self.always_commands = commands;
        self.notice(note);
    }

    fn use_model(&mut self, id: &str) {
        self.requested = Some(id.to_string());
        self.model = id.to_string();
        self.notice(&format!("Model for this session: {id}"));
    }

    pub(super) fn notice(&mut self, text: &str) {
        self.history.extend(view::notice(&self.theme, text, self.width));
    }

    /// A key while a list is open.
    pub(super) fn list_key(&mut self, key: &Key) -> Option<Command> {
        let list = self.list.as_mut()?;
        match list.key(key) {
            Outcome::Open => None,
            Outcome::Closed => {
                self.list = None;
                None
            }
            Outcome::Typed(text) => {
                // Typing in the sessions' list: a new session, with what was typed in its composer.
                self.list = None;
                let command = self.run(Slash::New, String::new());
                self.composer.insert(&text);
                command
            }
            Outcome::Chosen(id) => {
                let purpose = list.purpose;
                self.list = None;
                match purpose {
                    Purpose::Model => {
                        self.use_model(&id);
                        None
                    }
                    Purpose::Session => match id.strip_prefix(super::super::turn::AGENT_PREFIX) {
                        Some(agent) => Some(Command::OpenAgent(agent.to_string())),
                        None => Some(Command::Load(Load::Session(id))),
                    },
                    Purpose::Approvals => {
                        self.set_approvals(&id);
                        None
                    }
                }
            }
        }
    }

    /// A slash command's read, done: its list opens or its result shows. Other events go back to `event`.
    pub(super) fn loaded(&mut self, event: TurnEvent, width: usize) -> Option<TurnEvent> {
        match event {
            TurnEvent::Models(Ok(items)) => self.list = Some(List::new(Purpose::Model, "Choose a model", items)),
            TurnEvent::Sessions(Ok(items)) if items.is_empty() => self.notice("No earlier sessions."),
            TurnEvent::Sessions(Ok(items)) => {
                self.list = Some(List::new(Purpose::Session, "Resume session", items));
            }
            TurnEvent::Opened(Ok(detail)) => self.resume(&detail, width),
            TurnEvent::AgentOpened(Ok((title, messages, agents))) => {
                self.resume_agent(&title, &messages, agents, width)
            }
            TurnEvent::MainView(messages) => self.main_view(&messages, width),
            TurnEvent::AgentOpened(Err(error)) => {
                self.history
                    .extend(view::error_cell(&self.theme, &CliError::new(error), width));
            }
            TurnEvent::Spend(Ok(left)) => {
                self.notice(&left);
                self.history
                    .push(format!("  {}", self.theme.style.dim("`nexus spend` lists each call.")));
                self.budget = Some(left);
            }
            TurnEvent::Models(Err(error))
            | TurnEvent::Sessions(Err(error))
            | TurnEvent::Opened(Err(error))
            | TurnEvent::Spend(Err(error)) => self.history.extend(view::error_cell(&self.theme, &error, width)),
            event => return Some(event),
        }
        None
    }

    /// Switch to an earlier agent conversation of this folder: agent mode, its whole history as it showed, and its
    /// subagents in the agents panel; the next prompt goes on from it.
    fn resume_agent(
        &mut self,
        title: &str,
        messages: &[Value],
        agents: Vec<super::super::agent::sessions::Subagent>,
        width: usize,
    ) {
        self.show_only_this_session();
        self.reset_agents();
        self.set_agents(agents);
        self.mode = super::Mode::Agent;
        self.fresh = false;
        self.suggestion = None;
        self.notice(&format!("Resumed {title} · agent mode"));
        let lines = self.conversation_lines(messages, width);
        self.history.extend(lines);
    }

    /// An agent conversation's messages as they showed when they ran: each prompt, answer and tool card, a subagent as
    /// its card and report (or its card alone while it runs).
    pub(super) fn conversation_lines(&self, messages: &[Value], width: usize) -> Vec<String> {
        use super::super::agent::{Task, cells, sessions, tools};
        let mut lines = Vec::new();
        let result_of = |id: &str| {
            messages
                .iter()
                .find(|message| s(message, "role") == Some("tool") && s(message, "tool_call_id") == Some(id))
                .map(sessions::text_of)
        };
        for message in messages {
            let text = sessions::text_of(message);
            match s(message, "role") {
                Some("user") => {
                    lines.extend(view::prompt_cell(
                        &self.theme,
                        super::super::mentions::strip_files(&text),
                        width,
                    ));
                    lines.push(String::new());
                }
                Some("assistant") => {
                    if !text.trim().is_empty() {
                        let mut answer = Answer::default();
                        answer.push(&text);
                        lines.extend(answer.take(&self.theme, width, true).0);
                    }
                    for call in a(message, "tool_calls") {
                        let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
                        let arguments = call
                            .pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}");
                        let result = result_of(s(call, "id").unwrap_or(""));
                        if name == super::super::agent::SUBAGENT_TOOL {
                            let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
                            let field = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                            let started = Task::Started {
                                description: field("description"),
                                prompt: field("prompt"),
                                call: String::new(),
                            };
                            lines.extend(cells::task_lines(&self.theme, &started, width));
                            if let Some(result) = &result {
                                let error = result.strip_prefix("Error: ");
                                let finished = Task::Finished {
                                    result: error.unwrap_or(result).to_string(),
                                    error: error.is_some(),
                                    messages: Vec::new(),
                                };
                                lines.extend(cells::task_lines(&self.theme, &finished, width));
                            }
                            continue;
                        }
                        // A call still running has no result yet: it shows once it has one.
                        if let Some(result) = &result {
                            let action = tools::replay(name, arguments, result);
                            lines.extend(cells::tool_cell(&self.theme, &action, width));
                        }
                    }
                }
                _ => {}
            }
        }
        lines
    }

    /// Continue an earlier session: its last messages show, as they would have, and the next prompt goes to it.
    fn resume(&mut self, detail: &Value, width: usize) {
        self.show_only_this_session();
        self.reset_agents();
        let id = js_string(detail.get("id"));
        let title = s(detail, "title")
            .filter(|title| !title.is_empty())
            .unwrap_or("Untitled");
        self.session = Some(id);
        self.fresh = false;
        self.suggestion = None;
        let messages = a(detail, "messages");
        self.notice(&format!("Resumed {title}"));
        // Whether the prompt before an answer asked for a `<next>` line (an earlier session's).
        let mut asked = false;
        for message in messages {
            let content = js_string(message.get("content"));
            if s(message, "role") == Some("user") {
                asked = suggest::asked_for_next(&content);
                self.history.extend(view::prompt_cell(
                    &self.theme,
                    super::super::mentions::strip_files(suggest::strip_instruction(&content)),
                    width,
                ));
                self.history.push(String::new());
                continue;
            }
            let mut answer = Answer::default();
            answer.push(suggest::strip_suggestion(&content, asked));
            let (lines, _) = answer.take(&self.theme, width, true);
            self.history.extend(lines);
            self.history.push(view::answer_meta(
                &self.theme,
                &[s(message, "model").map(String::from), Some(usd(n(message, "costUsd")))],
            ));
        }
    }
}
