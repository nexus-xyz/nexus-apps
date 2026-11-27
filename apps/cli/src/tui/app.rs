//! The session's state and its rules: what a key does, what an answer's events do, and what to draw. No terminal and
//! no network here: `mod.rs` feeds it keys and the turn's events, and draws the frames it returns.
//!
//! Lines that are final (the header, a prompt, the finished lines of an answer) go to the terminal's scrollback once;
//! the frame's live part (the line still streaming, the status, the composer and the footer) is drawn again in place.

use serde_json::Value;

use crate::error::CliError;
use crate::json::{js_string, n, s};
use crate::style::usd;

use super::agent::tools::{Ask, Decision};
use super::commands;
use super::composer::Composer;
use super::list::List;
use super::logo;
use super::markdown::Markdown;
use super::mentions;
use super::turn::{Load, TurnEvent};
use super::view::{self, Hint, Theme};

mod agent;
mod agents;
mod btw;
mod slash;

/// A key, as the session reads it (`mod.rs` maps crossterm's events to these).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Paste(String),
    Enter,
    NewLine,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Esc,
    CtrlC,
    CtrlD,
    CtrlU,
    /// ⌃f: filter the sessions' list.
    CtrlF,
    CtrlK,
    CtrlW,
    Tab,
    BackTab,
    /// ⌃v: the image on the clipboard.
    PasteImage,
}

/// Where a prompt goes: Nexus chat sessions, or the agent working in this folder with tools (⇧tab switches them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Chat,
    Agent,
}

/// How the composer names a pasted image.
fn image_label(number: usize) -> String {
    format!("[Image #{number}]")
}

/// The composer's placeholder under the sessions' list.
const NEW_SESSION: &str = "Type to start a new session";

/// Rows the session keeps for itself under the mark (the composer, the footer, spacing) and above it (the header).
const ROWS_KEPT: usize = 10;

/// What the session asks of the outside after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Send turn `turn`: `content` is the prompt as Gateway gets it, `model` the model asked for (`None`: the
    /// session's own).
    Send {
        turn: u64,
        /// The prompt as typed: `@` mentions get their files' text.
        prompt: String,
        suggest: bool,
        model: Option<String>,
        /// The chat session it continues; `None`: a new one.
        session: Option<String>,
        /// Pasted images, `{data, name}`.
        images: Vec<Value>,
    },
    /// Read something a slash command needs.
    Load(Load),
    /// Keep `/suggestions on|off` for the next sessions.
    SaveSuggestions(bool),
    /// Start agent turn `turn` with `prompt` (`model`: `None`, what Gateway chooses).
    Agent {
        turn: u64,
        prompt: String,
        model: Option<String>,
        images: Vec<Value>,
    },
    /// List the workspace's files for `@` mentions.
    ListFiles,
    /// `/diff`.
    Diff,
    /// `!cmd`: run the person's command in the folder. What it printed goes into the conversation: the agent's
    /// (`for_agent`), or with the next chat prompt.
    Shell { command: String, for_agent: bool },
    /// `/compact`: summarize the agent's conversation, with the model asked for (`None`: what Gateway chooses).
    Compact { model: Option<String> },
    /// Read the image on the clipboard.
    PasteImage,
    /// The person's answer to what the agent asked.
    Decide(Decision),
    /// esc: stop the running turn.
    Stop,
    /// `/new`: forget the agent's conversation too.
    Reset,
    /// `/resume`: switch to the terminal's saved agent conversation with this id.
    OpenAgent(String),
    /// Leaving a subagent's transcript: the agent's conversation, to draw again.
    ShowMain,
    /// `/btw`: a side question about the conversation (`agent`: the agent's; else chat session `session`), with the
    /// earlier side questions and their answers.
    Btw {
        question: String,
        agent: bool,
        session: Option<String>,
        model: Option<String>,
        earlier: Vec<(String, String)>,
    },
}

/// What to draw: lines for the scrollback, then the live part, with the cursor's row and column in it. The live part
/// reaches down to the terminal's last row, as Codex keeps its composer at the bottom: the rows to spare go in at
/// `split`, between what the session shows (an answer streaming, the mark) and the composer; with `center`, half go
/// above, so the mark sits in the middle of the free space.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frame {
    /// Wipe the screen and the scrollback first: the session changed, and only its own messages show.
    pub clear: bool,
    pub history: Vec<String>,
    pub live: Vec<String>,
    pub cursor: (usize, usize),
    pub split: usize,
    pub center: bool,
}

/// An answer as it streams: its text, how much of it is already in the scrollback, and the Markdown state the next
/// line starts in.
#[derive(Debug, Default)]
struct Answer {
    text: String,
    /// The byte where the lines not yet in the scrollback start.
    shown: usize,
    /// Whether the `•` line is in the scrollback.
    started: bool,
    markdown: Markdown,
}

impl Answer {
    /// Model text: a tab becomes four spaces, and other control characters (escape codes among them) are dropped,
    /// so an answer cannot drive the terminal.
    fn push(&mut self, delta: &str) {
        for c in delta.chars() {
            match c {
                '\t' => self.text.push_str("    "),
                '\n' => self.text.push('\n'),
                c if c.is_control() => {}
                c => self.text.push(c),
            }
        }
    }

    /// The lines that can no longer change (each complete source line, in Markdown), and the lines of the one still
    /// arriving (drawn from a copy of the state, so it can be drawn again as it grows). With `all`, everything is
    /// final.
    fn take(&mut self, theme: &Theme, width: usize, all: bool) -> (Vec<String>, Vec<String>) {
        let end = self.text.len();
        self.shown = self.shown.min(end);
        if !self.started {
            // An answer's first line is its first text: blank lines before it are dropped.
            let blank = self.text[self.shown..end].len() - self.text[self.shown..end].trim_start_matches('\n').len();
            self.shown += blank;
        }
        let rest = &self.text[self.shown..end];
        let rest = if all { rest.trim_end() } else { rest };
        let complete = if all {
            rest.len()
        } else {
            rest.rfind('\n').map_or(0, |at| at + 1)
        };
        let inner = width.saturating_sub(3).max(1);
        let (finished, arriving) = (rest[..complete].to_string(), rest[complete..].to_string());
        let mut done = Vec::new();
        for source in finished.split_terminator('\n') {
            for line in self.markdown.line(theme, source, inner) {
                done.push(view::answer_line(theme, &line, !self.started));
                self.started = true;
            }
        }
        self.shown = if all { end } else { self.shown + complete };
        let mut streaming = Vec::new();
        if !arriving.is_empty() {
            let mut started = self.started;
            for line in self.markdown.clone().line(theme, &arriving, inner) {
                streaming.push(view::answer_line(theme, &line, !started));
                started = true;
            }
        }
        (done, streaming)
    }
}

#[derive(Debug)]
struct Turn {
    id: u64,
    started_ms: u64,
    answer: Answer,
}

pub struct App {
    pub theme: Theme,
    pub composer: Composer,
    /// The model shown in the footer: the one asked for, or `default model`.
    pub model: String,
    /// The model a prompt asks for (`nexus models pick`, `/model`); `None`: what the session uses.
    pub requested: Option<String>,
    /// What `/status` shows before the session's own rows: version, organization, person, Gateway.
    pub about: Vec<(String, String)>,
    /// Whether answers end with a suggested next prompt (`/suggestions`).
    pub suggestions: bool,
    /// Chat or agent.
    pub mode: Mode,
    /// `$4 of $5 left`, after each answer.
    pub budget: Option<String>,
    /// The chat session, once the first prompt made it.
    pub session: Option<String>,
    pub quit: bool,
    turn: Option<Turn>,
    turns: u64,
    history: Vec<String>,
    quit_armed: bool,
    shortcuts: bool,
    /// The mark's opacity: from, to, and when it started to change (it dims while the person types).
    fade: (f64, f64, u64),
    /// How far the mark has turned, and when the last frame was: it turns only while the composer is empty.
    spun: u64,
    ticked: Option<u64>,
    /// No prompt yet in this session: the mark shows.
    fresh: bool,
    /// A list `/model` or `/resume` opened, in place of the composer.
    list: Option<List>,
    /// The command chosen in the `/` list, and whether esc closed the list.
    popup: usize,
    popup_closed: bool,
    /// The next prompt the last answer suggested, offered in the empty composer.
    suggestion: Option<String>,
    /// What the agent asks the person to approve, and the answer chosen so far.
    approval: Option<(Ask, usize)>,
    /// "Don't ask again" for edits, and for commands, in this session.
    always_edits: bool,
    always_commands: bool,
    /// The workspace's files for `@` mentions, once listed; the match chosen; whether esc closed the list.
    files: Option<Vec<String>>,
    files_asked: bool,
    mention: usize,
    mention_closed: bool,
    /// Images pasted for the next prompt, `{data, name}`; `[Image #n]` in the composer names each.
    images: Vec<Value>,
    /// Prompts sent while a turn runs: each goes when the one before ends.
    queued: Vec<String>,
    /// Shell mode (`!` on an empty composer): ⏎ runs the composer's text as a command.
    shell: bool,
    /// The terminal's width when it was last given, for lines made without one (a slash command's notice).
    width: usize,
    /// What the session has spent, and each turn's part: what it was, the model, the cost.
    spent: f64,
    costs: Vec<(String, String, f64)>,
    /// The lines the session opened with, drawn again when it changes; and whether the next frame clears for that.
    header: Vec<String>,
    clear: bool,
    /// `/btw`: the last side question, whether its overlay is open, and the earlier ones with their answers.
    btw: Option<btw::Btw>,
    btw_open: bool,
    btw_log: Vec<(String, String)>,
    /// The conversation's subagents (the agents panel), the panel's focused row (0: `main`), the subagent whose
    /// transcript is open, and the turn's end while it is.
    agents: Vec<agents::Row>,
    panel: Option<usize>,
    viewing: Option<usize>,
    held: Vec<String>,
}

impl App {
    pub fn new(theme: Theme, model: String, budget: Option<String>, header: Vec<String>) -> App {
        App {
            theme,
            composer: Composer::default(),
            model,
            requested: None,
            about: Vec::new(),
            suggestions: false,
            mode: Mode::Chat,
            budget,
            session: None,
            quit: false,
            turn: None,
            turns: 0,
            history: header.clone(),
            header,
            clear: false,
            btw: None,
            btw_open: false,
            btw_log: Vec::new(),
            agents: Vec::new(),
            panel: None,
            viewing: None,
            held: Vec::new(),
            quit_armed: false,
            shortcuts: false,
            fade: (1.0, 1.0, 0),
            spun: 0,
            ticked: None,
            fresh: true,
            list: None,
            popup: 0,
            popup_closed: false,
            suggestion: None,
            approval: None,
            always_edits: false,
            always_commands: false,
            files: None,
            files_asked: false,
            mention: 0,
            mention_closed: false,
            images: Vec::new(),
            queued: Vec::new(),
            shell: false,
            width: 80,
            spent: 0.0,
            costs: Vec::new(),
        }
    }

    pub fn running(&self) -> bool {
        self.turn.is_some()
    }

    pub fn key(&mut self, key: Key, width: usize, now_ms: u64) -> Option<Command> {
        self.width = width;
        if self.approval.is_some() {
            if key == Key::CtrlC {
                self.approval = None;
                return self.stop(width);
            }
            return self.approval_key(&key);
        }
        if self.btw_open {
            return self.btw_key(&key);
        }
        if self.viewing.is_some() {
            return self.transcript_key(&key);
        }
        if self.panel.is_some() {
            return self.panel_key(&key, width);
        }
        if self.list.is_some() {
            return self.list_key(&key);
        }
        let before = self.composer.text().to_string();
        let mut command = self.edit(key, width, now_ms);
        if self.composer.text() != before {
            self.popup = 0;
            self.popup_closed = false;
            self.mention = 0;
            self.mention_closed = false;
            self.suggestion = None;
        }
        // The first `@` lists the workspace's files.
        if command.is_none() && self.mention_query().is_some() && self.files.is_none() && !self.files_asked {
            self.files_asked = true;
            command = Some(Command::ListFiles);
        }
        let target = if self.composer.is_empty() { 1.0 } else { logo::DIM };
        if target != self.fade.1 {
            self.fade = (self.opacity(now_ms), target, now_ms);
        }
        command
    }

    /// The `/` list under the composer, while a command is being typed.
    fn popup_matches(&self) -> Option<Vec<(commands::Slash, &'static str, &'static str)>> {
        commands::matching(self.composer.text()).filter(|_| !self.popup_closed)
    }

    /// The `@` mention being typed: where it starts, and what follows the `@`.
    fn mention_query(&self) -> Option<(usize, String)> {
        if self.mention_closed {
            return None;
        }
        mentions::current(self.composer.text(), self.composer.cursor()).map(|(start, query)| (start, query.to_string()))
    }

    /// The files that match the mention being typed.
    fn mention_matches(&self) -> Option<Vec<String>> {
        let (_, query) = self.mention_query()?;
        let files = self.files.as_deref().unwrap_or_default();
        Some(mentions::rank(files, &query).into_iter().map(String::from).collect())
    }

    fn edit(&mut self, key: Key, width: usize, now_ms: u64) -> Option<Command> {
        if key == Key::PasteImage {
            return Some(Command::PasteImage);
        }
        // `!` on an empty composer: shell mode, until esc, or backspace on an empty line.
        if !self.shell && key == Key::Char('!') && self.composer.is_empty() {
            self.shell = true;
            return None;
        }
        if self.shell {
            match key {
                Key::Esc => {
                    self.shell = false;
                    return None;
                }
                Key::Backspace if self.composer.is_empty() => {
                    self.shell = false;
                    return None;
                }
                Key::Enter if !self.running() => {
                    let command = self.composer.take()?;
                    self.shell = false;
                    return Some(self.run_shell(command));
                }
                _ => {}
            }
        }
        if let (Some((start, _)), Some(matches)) = (self.mention_query(), self.mention_matches())
            && !matches.is_empty()
        {
            let count = matches.len();
            match key {
                Key::Up => {
                    self.mention = self.mention.checked_sub(1).unwrap_or(count - 1);
                    return None;
                }
                Key::Down => {
                    self.mention = (self.mention + 1) % count;
                    return None;
                }
                Key::Tab | Key::Enter => {
                    let path = &matches[self.mention.min(count - 1)];
                    let cursor = self.composer.cursor();
                    self.composer.replace(start, cursor, &format!("@{path} "));
                    return None;
                }
                Key::Esc => {
                    self.mention_closed = true;
                    return None;
                }
                _ => {}
            }
        }
        if let Some(matches) = self.popup_matches().filter(|matches| !matches.is_empty()) {
            let count = matches.len();
            match key {
                Key::Up => {
                    self.popup = self.popup.checked_sub(1).unwrap_or(count - 1);
                    return None;
                }
                Key::Down => {
                    self.popup = (self.popup + 1) % count;
                    return None;
                }
                Key::Tab => {
                    let (_, name, _) = matches[self.popup.min(count - 1)];
                    self.composer.clear();
                    self.composer.insert(&format!("/{name} "));
                    return None;
                }
                Key::Enter if !self.running() => {
                    let (slash, _, _) = matches[self.popup.min(count - 1)];
                    self.composer.take();
                    return self.run(slash, String::new());
                }
                Key::Esc => {
                    self.popup_closed = true;
                    return None;
                }
                _ => {}
            }
        }
        // Tab or → puts the suggested prompt in the composer; ⏎ then sends it.
        if self.composer.is_empty()
            && matches!(key, Key::Tab | Key::Right)
            && let Some(suggestion) = self.suggestion.take()
        {
            self.composer.insert(&suggestion);
            return None;
        }
        if self.shortcuts {
            self.shortcuts = false;
            if matches!(key, Key::Esc | Key::Char('?')) {
                return None;
            }
        }
        if key != Key::CtrlC {
            self.quit_armed = false;
        }
        match key {
            Key::BackTab => self.switch_mode(),
            Key::Char('?') if self.composer.is_empty() => self.shortcuts = true,
            Key::Char(c) => self.composer.insert(&c.to_string()),
            Key::Paste(text) => self.composer.insert(&text),
            Key::NewLine => self.composer.insert("\n"),
            Key::Enter => return self.send(width, now_ms),
            Key::Backspace => self.composer.backspace(),
            Key::Delete => self.composer.delete(),
            // On an empty composer, ← opens the chats to switch to (`/resume`'s list), as a sidebar would.
            Key::Left if self.composer.is_empty() && !self.running() => {
                return self.run(commands::Slash::Resume, String::new());
            }
            Key::Left => self.composer.left(),
            Key::Right => self.composer.right(),
            Key::Up => self.composer.up(),
            // ↓ on an empty composer, not recalling a prompt: into the agents panel, as Claude Code's.
            Key::Down if self.composer.is_empty() && !self.composer.recalling() && self.enter_panel() => {}
            Key::Down => self.composer.down(),
            Key::Home => self.composer.home(),
            Key::End => self.composer.end(),
            Key::CtrlU => self.composer.delete_to_line_start(),
            Key::CtrlK => self.composer.delete_to_line_end(),
            Key::CtrlW => self.composer.delete_word(),
            Key::Esc => return self.stop(width),
            Key::CtrlC if self.running() => return self.stop(width),
            Key::CtrlC if !self.composer.is_empty() => self.composer.clear(),
            Key::CtrlC if self.quit_armed => self.quit = true,
            Key::CtrlC => self.quit_armed = true,
            Key::CtrlD if self.composer.is_empty() && !self.running() => self.quit = true,
            Key::CtrlD => self.composer.delete(),
            Key::Tab => self.composer.insert("    "),
            Key::PasteImage | Key::CtrlF => {}
        }
        None
    }

    fn opacity(&self, now_ms: u64) -> f64 {
        let (from, to, at) = self.fade;
        let progress = now_ms.saturating_sub(at) as f64 / logo::FADE_MS as f64;
        from + (to - from) * logo::smooth(progress)
    }

    /// ⏎: a slash command runs; a prompt starts a turn. While an answer streams, the text stays in the composer.
    fn send(&mut self, width: usize, now_ms: u64) -> Option<Command> {
        if let Some(parsed) = commands::parse(self.composer.text()) {
            // A side question doesn't wait for the turn: it is asked beside it.
            if self.running() && !matches!(parsed, Ok((commands::Slash::Btw, _))) {
                return None;
            }
            self.composer.take();
            return match parsed {
                Ok((slash, args)) => self.run(slash, args),
                Err(name) => {
                    let error = CliError::hint(format!("There is no /{name} command."), "/help lists them.");
                    self.history.extend(view::error_cell(&self.theme, &error, width));
                    None
                }
            };
        }
        // `!cmd` typed or pasted whole: the person's own command, in this folder.
        if let Some(command) = self
            .composer
            .text()
            .trim()
            .strip_prefix('!')
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            if self.running() {
                return None;
            }
            let command = command.to_string();
            self.composer.take();
            return Some(self.run_shell(command));
        }
        let prompt = self.composer.take()?;
        // While a turn runs, a prompt waits its turn.
        if self.running() {
            self.queued.push(prompt);
            return None;
        }
        self.submit(prompt, width, now_ms)
    }

    /// Run the person's command; in chat its output goes with the next prompt, in agent mode into the agent's
    /// conversation.
    fn run_shell(&mut self, command: String) -> Command {
        self.fresh = false;
        Command::Shell {
            command,
            for_agent: self.mode == Mode::Agent,
        }
    }

    /// Start a turn with `prompt` in the current mode.
    fn submit(&mut self, prompt: String, width: usize, now_ms: u64) -> Option<Command> {
        // The images still named in the prompt go with it.
        let images: Vec<Value> = std::mem::take(&mut self.images)
            .into_iter()
            .enumerate()
            .filter(|(index, _)| prompt.contains(&image_label(index + 1)))
            .map(|(_, image)| image)
            .collect();
        self.history.extend(view::prompt_cell(&self.theme, &prompt, width));
        self.history.push(String::new());
        self.fresh = false;
        self.turns += 1;
        self.turn = Some(Turn {
            id: self.turns,
            started_ms: now_ms,
            answer: Answer::default(),
        });
        if self.mode == Mode::Agent {
            self.suggestion = None;
            return Some(Command::Agent {
                turn: self.turns,
                prompt,
                model: self.requested.clone(),
                images,
            });
        }
        Some(Command::Send {
            turn: self.turns,
            prompt,
            suggest: self.suggestions,
            model: self.requested.clone(),
            session: self.session.clone(),
            images,
        })
    }

    /// esc: the answer so far stays, marked as stopped; what still arrives for it is dropped. An agent stops at once:
    /// its stream closes and no further tool runs.
    fn stop(&mut self, width: usize) -> Option<Command> {
        let mut turn = self.turn.take()?;
        self.approval = None;
        let (lines, _) = turn.answer.take(&self.theme, width, true);
        self.history.extend(lines);
        let note = match self.mode {
            Mode::Chat => "Stopped. The whole answer is kept in the session.",
            Mode::Agent => "Stopped. Tell the agent what to do instead.",
        };
        self.history.extend(view::stop_cell(&self.theme, note));
        // What was waiting goes back to the composer, to send again or change.
        if !self.queued.is_empty() {
            let waiting = std::mem::take(&mut self.queued).join("\n");
            let draft = self.composer.text().to_string();
            self.composer.clear();
            self.composer.insert(&if draft.is_empty() {
                waiting
            } else {
                format!("{waiting}\n{draft}")
            });
        }
        Some(Command::Stop)
    }

    /// An event of turn `turn`: a stopped turn's answer is dropped, its session and the budget are kept. Turn 0 is a
    /// slash command's (`slash.rs`). What it answers goes to the turn (the agent waits for an approval).
    pub fn event(&mut self, turn: u64, event: TurnEvent, width: usize, now_ms: u64) -> Option<Command> {
        let current = self.turn.as_ref().is_some_and(|running| running.id == turn);
        let event = self.loaded(event, width)?;
        match event {
            TurnEvent::Approve(ask) if current => return self.ask(ask),
            // A stopped turn's question gets no, so its thread ends.
            TurnEvent::Approve(_) => return Some(Command::Decide(Decision::Deny)),
            TurnEvent::Tool(action) if current => self.tool_ran(&action, width),
            TurnEvent::Task { id, task } if current => self.task_event(&id, &task, width),
            TurnEvent::Finished(result) if current => {
                if let Some(turn) = self.turn.take() {
                    self.agent_finished(turn, result, width, now_ms);
                }
            }
            TurnEvent::Tool(_) | TurnEvent::Task { .. } | TurnEvent::Finished(_) => {}
            TurnEvent::Files(files) => self.files = Some(files),
            TurnEvent::Image(Ok(image)) => {
                self.images.push(image);
                self.composer.insert(&format!("{} ", image_label(self.images.len())));
            }
            TurnEvent::Image(Err(error)) => {
                self.history
                    .extend(view::error_cell(&self.theme, &CliError::new(error), width));
            }
            TurnEvent::Note(note) => self.history.extend(view::notice(&self.theme, &note, width)),
            TurnEvent::Spent(cost) => self.spent += cost,
            TurnEvent::BtwDelta(text) => self.btw_delta(&text),
            TurnEvent::BtwDone(result) => self.btw_done(result.map_err(|error| error.message)),
            TurnEvent::Shell(action) => {
                self.history
                    .extend(super::agent::cells::shell_cell(&self.theme, &action, width));
                let note = match self.mode {
                    Mode::Agent => "The agent sees what it printed.",
                    Mode::Chat => "What it printed goes with your next prompt.",
                };
                self.history.push(format!("  {}", self.theme.style.dim(note)));
            }
            TurnEvent::Diff(diff) => self.show_diff(diff, width),
            TurnEvent::Compacted(result) => self.compacted(result, width),
            TurnEvent::Session(id) => self.session = Some(id),
            TurnEvent::Budget(left) => self.budget = Some(left),
            TurnEvent::Suggested {
                session,
                suggestion,
                model,
                cost,
            } => {
                // A late one from an earlier turn is not offered after a newer answer; its cost still counts.
                let suggestion = suggestion.filter(|_| turn == self.turns);
                self.suggested(&session, suggestion, model, cost)
            }
            TurnEvent::Delta(delta) if current => {
                if let Some(running) = &mut self.turn {
                    running.answer.push(&delta);
                }
            }
            TurnEvent::Done(result) if current => {
                if let Some(turn) = self.turn.take() {
                    self.finish(turn, result, width, now_ms);
                }
            }
            TurnEvent::Delta(_) | TurnEvent::Done(_) => {}
            TurnEvent::Models(_)
            | TurnEvent::Sessions(_)
            | TurnEvent::Opened(_)
            | TurnEvent::AgentOpened(_)
            | TurnEvent::MainView(_)
            | TurnEvent::Spend(_) => {}
        }
        // A turn ended: the next waiting prompt goes.
        if !self.running() && !self.queued.is_empty() {
            let next = self.queued.remove(0);
            return self.submit(next, width, now_ms);
        }
        None
    }

    /// The next prompt suggested after an answer: offered in the empty composer when it is for a session
    /// on screen and nothing runs. Its cost counts either way, and it is listed when a model was called.
    fn suggested(&mut self, session: &str, suggestion: Option<String>, model: String, cost: f64) {
        self.spent += cost;
        if cost > 0.0 || !model.is_empty() {
            self.costs.push(("suggestion".into(), model, cost));
        }
        let on_screen = self.session.as_deref() == Some(session);
        if self.suggestions && on_screen && !self.running() && self.composer.is_empty() {
            self.suggestion = suggestion;
        }
    }

    fn finish(&mut self, mut turn: Turn, result: Result<Value, CliError>, width: usize, now_ms: u64) {
        match result {
            Ok(message) => {
                // An answer that did not stream (a cached one) arrives whole.
                if turn.answer.text.is_empty() {
                    turn.answer.push(&js_string(message.get("content")));
                }
                let (lines, _) = turn.answer.take(&self.theme, width, true);
                self.history.extend(lines);
                // The suggestion comes after, from its own call (`TurnEvent::Suggested`).
                self.suggestion = None;
                let cost = n(&message, "costUsd");
                self.spent += cost;
                self.costs
                    .push(("chat".into(), s(&message, "model").unwrap_or("").to_string(), cost));
                self.history.push(view::answer_meta(
                    &self.theme,
                    &[
                        s(&message, "model").map(String::from),
                        Some(usd(n(&message, "costUsd"))),
                        Some(view::elapsed(now_ms.saturating_sub(turn.started_ms))),
                    ],
                ));
            }
            Err(error) => {
                let (lines, _) = turn.answer.take(&self.theme, width, true);
                self.history.extend(lines);
                self.history.extend(view::error_cell(&self.theme, &error, width));
            }
        }
    }

    /// Show another session: the screen and the scrollback are wiped, and the header comes back, so only what that
    /// session holds shows under it.
    pub(super) fn show_only_this_session(&mut self) {
        self.history = self.header.clone();
        self.clear = true;
    }

    /// The mark shows on a fresh session, unless a list takes its place.
    fn mark_shown(&self) -> bool {
        self.fresh && self.list.is_none() && self.theme.style.unicode
    }

    /// Whether the mark is turning or fading, so frames should come often.
    pub fn animating(&self, now_ms: u64) -> bool {
        self.mark_shown() && (self.composer.is_empty() || now_ms <= self.fade.2 + logo::FADE_MS + 100)
    }

    /// The frame to draw on a terminal `width` × `height`: lines that became final go to the scrollback, the rest is
    /// live. Before the first prompt, the Nexus mark sits above the composer when the terminal has room for it.
    pub fn frame(&mut self, width: usize, height: usize, now_ms: u64) -> Frame {
        self.width = width;
        let mut history = std::mem::take(&mut self.history);
        let mut live = Vec::new();
        // A subagent's transcript is open: the main turn's answer waits for the way back.
        if let Some(turn) = self.turn.as_mut().filter(|_| self.viewing.is_none()) {
            let (lines, streaming) = turn.answer.take(&self.theme, width, false);
            history.extend(lines);
            live.extend(streaming);
        }
        let mut split = live.len();
        let mut center = false;
        if let Some(turn) = &self.turn {
            live.push(String::new());
            // Before the first word the model is thinking; then it is writing.
            let word = if turn.answer.text.is_empty() {
                "Thinking…"
            } else {
                "Writing…"
            };
            live.push(view::status_line(
                &self.theme,
                word,
                now_ms.saturating_sub(turn.started_ms),
            ));
        }
        for waiting in &self.queued {
            let line = format!(
                "  {} {}",
                self.theme.style.dim("↳ queued:"),
                waiting.lines().next().unwrap_or("")
            );
            live.push(crate::style::truncate(
                &line,
                width.saturating_sub(1),
                self.theme.style.g.ellipsis,
            ));
        }
        if let Some((columns, rows)) =
            logo::stage(width, height.saturating_sub(ROWS_KEPT), self.theme.aspect).filter(|_| self.mark_shown())
        {
            // While the person types, the mark stands as drawn; once the composer is empty the iris goes on from there.
            if self.composer.is_empty() {
                self.spun += now_ms.saturating_sub(self.ticked.unwrap_or(now_ms));
                self.ticked = Some(now_ms);
            } else {
                self.spun = logo::FRONT_MS;
                self.ticked = None;
            }
            let indent = " ".repeat((width - columns) / 2);
            live.push(String::new());
            for line in logo::lines(columns, rows, self.spun, self.opacity(now_ms), self.theme.paint()) {
                live.push(if line.is_empty() {
                    line
                } else {
                    format!("{indent}{line}")
                });
            }
            split = live.len();
            center = true;
        }
        live.push(String::new());
        if let Some((ask, selected)) = &self.approval {
            let lines = super::agent::cells::approval_lines(&self.theme, ask, *selected, width);
            let cursor = (live.len() + lines.len() - 3 + selected, 0);
            live.extend(lines);
            return Frame {
                clear: std::mem::take(&mut self.clear),
                history,
                live,
                cursor,
                split,
                center,
            };
        }
        // A subagent's transcript: a bar in place of the composer says how to go back.
        if let Some(index) = self.viewing {
            let style = &self.theme.style;
            let name = self.agents.get(index).map_or("", |row| row.description.as_str());
            live.push(String::new());
            live.push(crate::style::truncate(
                &format!(
                    "  {} {}",
                    style.accent("Agent"),
                    style.dim(&format!("{name} · esc or ← back to main"))
                ),
                width.saturating_sub(1),
                style.g.ellipsis,
            ));
            let cursor = (live.len() - 1, 0);
            return Frame {
                clear: std::mem::take(&mut self.clear),
                history,
                live,
                cursor,
                split,
                center,
            };
        }
        // `/btw`'s overlay, in place of the composer; the turn goes on above it.
        if self.btw_open {
            let lines = self.btw_lines(width);
            let cursor = (live.len() + lines.len().saturating_sub(1), 0);
            live.extend(lines);
            return Frame {
                clear: std::mem::take(&mut self.clear),
                history,
                live,
                cursor,
                split,
                center,
            };
        }
        if let Some(list) = &self.list {
            let (lines, (row, column)) = list.lines(&self.theme, width);
            let mut cursor = (live.len() + row, column);
            live.extend(lines);
            // The sessions' list keeps a composer under it: typing there starts a new session. The cursor waits in it,
            // or in the filter's box after ⌃f.
            if list.purpose == super::list::Purpose::Session {
                live.push(String::new());
                let (composer, (row, column)) =
                    view::composer_lines(&self.theme, &self.composer, NEW_SESSION, false, width);
                if !list.filtering() {
                    cursor = (live.len() + row, column);
                }
                live.extend(composer);
            }
            return Frame {
                clear: std::mem::take(&mut self.clear),
                history,
                live,
                cursor,
                split,
                center,
            };
        }
        let placeholder = match (self.suggestion.as_deref(), self.mode) {
            _ if self.shell => "Run a command in this folder, without asking",
            (Some(suggestion), _) => suggestion,
            (None, Mode::Chat) => "Ask anything",
            (None, Mode::Agent) => "Ask the agent to change something in this folder",
        };
        let (composer, (row, column)) =
            view::composer_lines(&self.theme, &self.composer, placeholder, self.shell, width);
        let cursor = (live.len() + row, column);
        live.extend(composer);
        if let Some(matches) = self.mention_matches() {
            let matches: Vec<&str> = matches.iter().map(String::as_str).collect();
            let selected = self.mention.min(matches.len().saturating_sub(1));
            live.extend(mentions::popup(&self.theme, &matches, selected, self.files.is_none()));
        } else if let Some(matches) = self.popup_matches() {
            live.extend(commands::popup(
                &self.theme,
                &matches,
                self.popup.min(matches.len().saturating_sub(1)),
            ));
        } else if self.shortcuts {
            live.extend(view::shortcuts(&self.theme));
        } else {
            let hint = match (self.quit_armed, self.composer.is_empty(), self.suggestion.is_some()) {
                (true, _, _) => Hint::Quit,
                _ if self.shell => Hint::Shell,
                (false, true, true) => Hint::Suggestion,
                (false, true, false) => Hint::Shortcuts,
                (false, false, _) => Hint::None,
            };
            let bullet = self.theme.style.g.bullet;
            let mut right = self.model.clone();
            if self.spent > 0.0 {
                right.push_str(&format!(" {bullet} {} spent", usd(self.spent)));
            }
            if let Some(left) = &self.budget {
                right.push_str(&format!(" {bullet} {left}"));
            }
            let mode = match self.mode {
                Mode::Chat => None,
                Mode::Agent => Some(match self.approvals() {
                    "auto" => "agent mode · auto",
                    "auto-edit" => "agent mode · auto-edit",
                    _ => "agent mode",
                }),
            };
            live.push(view::footer(&self.theme, hint, mode, &right, width));
        }
        live.extend(self.panel_lines(width));
        Frame {
            clear: std::mem::take(&mut self.clear),
            history,
            live,
            cursor,
            split,
            center,
        }
    }

    /// What stays on the screen after the session: how to go on with it.
    pub fn goodbye(&self) -> Vec<String> {
        let style = &self.theme.style;
        match &self.session {
            Some(id) => vec![
                String::new(),
                format!(
                    "  {} {}",
                    style.dim("To continue this session, run"),
                    style.accent("nexus chat \"…\"")
                ),
                format!("  {}", style.dim(&format!("Session {id}"))),
            ],
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::style::Style;

    fn app() -> App {
        App::new(
            Theme::new(Style::new(false), None),
            "nexus/auto".into(),
            Some("$4.00 left for you".into()),
            vec!["header".into()],
        )
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            app.key(Key::Char(c), 40, 0);
        }
    }

    #[test]
    fn first_frame_shows_the_header_then_the_composer() {
        let mut app = app();
        let frame = app.frame(40, 10, 0);
        assert_eq!(frame.history, ["header"]);
        assert_eq!(
            frame.live,
            ["", "", "› Ask anything", "", "  nexus/auto · $4.00 left for you"]
        );
        let wide = app.frame(60, 10, 0).live;
        assert_eq!(wide[4], "  ? for shortcuts          nexus/auto · $4.00 left for you");
        assert_eq!(frame.cursor, (2, 2));
        assert!(
            app.frame(40, 10, 0).history.is_empty(),
            "the header goes to the scrollback once"
        );
    }

    #[test]
    fn a_prompt_streams_an_answer_line_by_line() {
        let mut app = app();
        app.frame(40, 10, 0);
        typed(&mut app, "hello");
        let sent = app.key(Key::Enter, 40, 1_000);
        assert_eq!(
            sent,
            Some(Command::Send {
                turn: 1,
                prompt: "hello".into(),
                suggest: false,
                model: None,
                session: None,
                images: Vec::new(),
            })
        );
        let frame = app.frame(40, 10, 1_000);
        assert_eq!(frame.history, ["", "", "› hello", "", ""]);

        app.event(1, TurnEvent::Session("s1".into()), 40, 1_100);
        app.event(1, TurnEvent::Delta("\nFirst line\nsecond".into()), 40, 1_200);
        let frame = app.frame(40, 10, 3_500);
        assert_eq!(frame.history, ["• First line"]);
        assert_eq!(frame.live[..3], ["  second", "", "⣽ Writing… 2s · esc to interrupt"]);

        app.event(1, TurnEvent::Delta(" part\x1b[2J".into()), 40, 3_600);
        let done = json!({ "model": "anthropic/claude-haiku-5-5", "costUsd": 0.0012 });
        app.event(1, TurnEvent::Done(Ok(done)), 40, 4_000);
        let frame = app.frame(40, 10, 4_000);
        assert_eq!(frame.history[0], "  second part[2J", "escape codes are dropped");
        assert!(frame.history[1].starts_with("  anthropic/claude-haiku-5-5 · $"));
        assert!(frame.history[1].ends_with(" · 3s"));
        assert!(!app.running());
        assert_eq!(app.session.as_deref(), Some("s1"));
    }

    #[test]
    fn esc_stops_and_later_events_are_dropped() {
        let mut app = app();
        typed(&mut app, "hi");
        app.key(Key::Enter, 40, 0);
        app.event(1, TurnEvent::Delta("partial".into()), 40, 10);
        app.key(Key::Esc, 40, 20);
        assert!(!app.running());
        app.event(1, TurnEvent::Delta(" more".into()), 40, 30);
        app.event(1, TurnEvent::Budget("$3.00 left for you".into()), 40, 30);
        let history = app.frame(40, 10, 40).history;
        assert!(history.contains(&"• partial".to_string()));
        assert!(history.iter().any(|line| line.starts_with("■ Stopped")));
        assert!(!history.iter().any(|line| line.contains("more")));
        assert_eq!(app.budget.as_deref(), Some("$3.00 left for you"));
    }

    #[test]
    fn an_error_shows_with_its_hint() {
        let mut app = app();
        typed(&mut app, "hi");
        app.key(Key::Enter, 40, 0);
        app.frame(40, 10, 0);
        let error = CliError::hint("Your budget is spent.", "Ask an owner to raise it.");
        app.event(1, TurnEvent::Done(Err(error)), 40, 10);
        assert_eq!(
            app.frame(40, 10, 10).history,
            ["", "■ Your budget is spent.", "  → Ask an owner to raise it."]
        );
    }

    #[test]
    fn ctrl_c_clears_then_asks_then_quits() {
        let mut app = app();
        typed(&mut app, "draft");
        app.key(Key::CtrlC, 40, 0);
        assert!(app.composer.is_empty() && !app.quit);
        app.key(Key::CtrlC, 40, 0);
        assert_eq!(
            app.frame(40, 10, 0)
                .live
                .last()
                .map(|line| line.starts_with("  ⌃c again to quit")),
            Some(true)
        );
        app.key(Key::CtrlC, 40, 0);
        assert!(app.quit);
    }

    /// ⇧tab switches chat and agent, and `?` names both.
    #[test]
    fn shift_tab_switches_chat_and_agent() {
        let mut app = app();
        app.key(Key::BackTab, 80, 0);
        assert_eq!(app.mode, Mode::Agent);
        app.key(Key::BackTab, 80, 0);
        assert_eq!(app.mode, Mode::Chat);
        app.key(Key::Char('?'), 80, 0);
        let live = app.frame(80, 30, 0).live;
        assert!(live.iter().any(|line| line.contains("chat or agent")), "{live:?}");
        assert!(!live.iter().any(|line| line.contains("ask your code")), "{live:?}");
    }

    #[test]
    fn question_mark_opens_shortcuts_only_on_an_empty_composer() {
        let mut app = app();
        app.key(Key::Char('?'), 40, 0);
        assert!(
            app.frame(40, 10, 0)
                .live
                .iter()
                .any(|line| line.contains("Keyboard shortcuts"))
        );
        app.key(Key::Esc, 40, 0);
        typed(&mut app, "why?");
        assert_eq!(app.composer.text(), "why?");
    }

    #[test]
    fn a_prompt_sent_while_answering_waits_its_turn() {
        let mut app = app();
        typed(&mut app, "one");
        app.key(Key::Enter, 40, 0);
        typed(&mut app, "two");
        assert_eq!(app.key(Key::Enter, 40, 0), None);
        assert!(app.composer.is_empty());
        assert!(
            app.frame(40, 10, 0)
                .live
                .iter()
                .any(|line| line.contains("↳ queued: two"))
        );
        let done = json!({ "model": "m", "costUsd": 0.25 });
        let next = app.event(1, TurnEvent::Done(Ok(done)), 40, 10);
        assert!(matches!(next, Some(Command::Send { turn: 2, prompt, .. }) if prompt == "two"));
        assert!(app.running());
    }

    #[test]
    fn esc_puts_waiting_prompts_back() {
        let mut app = app();
        typed(&mut app, "one");
        app.key(Key::Enter, 40, 0);
        typed(&mut app, "two");
        app.key(Key::Enter, 40, 0);
        app.key(Key::Esc, 40, 0);
        assert_eq!(app.composer.text(), "two");
    }

    #[test]
    fn bang_runs_the_persons_command() {
        let mut app = app();
        typed(&mut app, "!git status");
        assert_eq!(
            app.key(Key::Enter, 40, 0),
            Some(Command::Shell {
                command: "git status".into(),
                for_agent: false
            })
        );
    }

    #[test]
    fn bang_on_an_empty_line_is_shell_mode() {
        let mut app = app();
        app.key(Key::Char('!'), 60, 0);
        assert!(app.composer.is_empty(), "the ! is the mode, not text");
        let live = app.frame(60, 10, 0).live;
        assert!(
            live.iter()
                .any(|line| line == "! Run a command in this folder, without asking"),
            "{live:#?}"
        );
        assert!(live.last().is_some_and(|line| line.starts_with("  ! shell")));
        typed(&mut app, "ls -la");
        assert_eq!(
            app.key(Key::Enter, 60, 0),
            Some(Command::Shell {
                command: "ls -la".into(),
                for_agent: false
            })
        );
        assert!(!app.shell, "back to the prompt after it runs");
        app.key(Key::Char('!'), 60, 0);
        app.key(Key::Backspace, 60, 0);
        assert!(!app.shell, "backspace on an empty line leaves shell mode");
        typed(&mut app, "wow!");
        assert_eq!(app.composer.text(), "wow!", "a ! inside text is text");
    }

    #[test]
    fn spending_shows_in_the_footer_and_in_cost() {
        let mut app = app();
        typed(&mut app, "hi");
        app.key(Key::Enter, 60, 0);
        app.event(
            1,
            TurnEvent::Done(Ok(json!({ "model": "openai/gpt-6", "costUsd": 0.25 }))),
            60,
            10,
        );
        let footer = app.frame(80, 10, 10).live.last().cloned().unwrap_or_default();
        assert!(footer.contains("$0.25 spent"), "{footer}");
        typed(&mut app, "/cost");
        app.key(Key::Enter, 60, 20);
        let history = app.frame(80, 10, 20).history;
        assert!(
            history
                .iter()
                .any(|line| line.contains("1. chat") && line.contains("openai/gpt-6")),
            "{history:#?}"
        );
        typed(&mut app, "/compact");
        assert_eq!(
            app.key(Key::Enter, 60, 30),
            None,
            "chat sessions are not compacted here"
        );
    }
}

#[cfg(test)]
mod slash_tests {
    use serde_json::json;

    use super::*;
    use crate::style::Style;
    use crate::tui::list::Item;

    fn app() -> App {
        let mut app = App::new(
            Theme::new(Style::new(false), None),
            "default model".into(),
            None,
            Vec::new(),
        );
        app.suggestions = true;
        app
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            app.key(Key::Char(c), 60, 0);
        }
    }

    #[test]
    fn new_starts_a_clean_screen_with_the_header() {
        let mut app = App::new(
            Theme::new(Style::new(false), None),
            "m".into(),
            None,
            vec!["header".into()],
        );
        app.frame(60, 30, 0);
        typed(&mut app, "/new");
        app.key(Key::Enter, 60, 0);
        let frame = app.frame(60, 30, 0);
        assert!(frame.clear);
        assert_eq!(frame.history[0], "header");
    }

    #[test]
    fn typing_in_the_chats_starts_a_new_session_with_that_text() {
        let mut app = app();
        app.session = Some("c-1".into());
        app.key(Key::Left, 60, 0);
        let chat = Item {
            id: "c-1".into(),
            label: "Plans".into(),
            ..Item::default()
        };
        app.event(0, TurnEvent::Sessions(Ok(vec![chat])), 60, 0);
        let frame = app.frame(60, 40, 0);
        let composer = frame
            .live
            .iter()
            .position(|line| line.contains("Type to start a new session"));
        assert!(composer.is_some(), "a composer under the list: {:#?}", frame.live);
        assert_eq!(Some(frame.cursor.0), composer, "the cursor waits in it");
        assert_eq!(app.key(Key::Char('h'), 60, 0), Some(Command::Reset), "a new session");
        typed(&mut app, "i");
        assert_eq!(app.composer.text(), "hi");
        assert_eq!(app.session, None);
        assert!(app.frame(60, 30, 0).clear);
    }

    #[test]
    fn btw_asks_beside_a_running_turn_and_leaves_the_conversation_alone() {
        let mut app = app();
        typed(&mut app, "plan the release");
        assert!(matches!(app.key(Key::Enter, 60, 0), Some(Command::Send { .. })));
        assert!(app.running());
        typed(&mut app, "/btw which file was it?");
        let asked = app.key(Key::Enter, 60, 0);
        assert!(
            matches!(&asked, Some(Command::Btw { question, earlier, .. }) if question == "which file was it?" && earlier.is_empty()),
            "{asked:?}"
        );
        let live = app.frame(60, 40, 0).live.join("\n");
        assert!(
            live.contains("/btw which file was it?") && live.contains("Answering…"),
            "{live}"
        );

        app.event(0, TurnEvent::BtwDelta("It was config.toml.".into()), 60, 0);
        app.event(0, TurnEvent::BtwDone(Ok(0.002)), 60, 0);
        let frame = app.frame(60, 40, 0);
        assert!(frame.live.join("\n").contains("It was config.toml."));
        assert!(
            !frame.history.join("\n").contains("config.toml"),
            "not in the conversation"
        );
        assert!((app.spent - 0.002).abs() < 1e-9, "its cost counts");

        app.key(Key::Esc, 60, 0);
        assert!(app.running(), "esc dismissed the answer; the turn goes on");
        assert!(!app.frame(60, 40, 0).live.join("\n").contains("config.toml"));
        typed(&mut app, "/btw and the other one?");
        let next = app.key(Key::Enter, 60, 0);
        assert!(
            matches!(&next, Some(Command::Btw { earlier, .. }) if earlier == &[("which file was it?".to_string(), "It was config.toml.".to_string())]),
            "{next:?}"
        );
    }

    #[test]
    fn down_cycles_the_sessions_agents_and_opens_a_subagents_transcript() {
        use crate::tui::agent::Task;
        use crate::tui::agent::tools::Action;
        let mut app = app();
        app.mode = Mode::Agent;
        typed(&mut app, "look into it");
        assert!(matches!(app.key(Key::Enter, 60, 0), Some(Command::Agent { .. })));
        let task = |task: Task| TurnEvent::Task {
            id: "agent-1".into(),
            task,
        };
        app.event(
            1,
            task(Task::Started {
                description: "Read hello".into(),
                prompt: "Read hello.py and report.".into(),
                call: "call_1".into(),
            }),
            60,
            0,
        );
        app.event(
            1,
            task(Task::Tool(Action::Read {
                path: "hello.py".into(),
                lines: 2,
            })),
            60,
            0,
        );
        let live = app.frame(60, 40, 0).live.join("\n");
        assert!(
            live.contains("main") && live.contains("Read hello · running · 1 tool"),
            "{live}"
        );

        let report = vec![
            json!({ "role": "user", "content": "Read hello.py and report." }),
            json!({ "role": "assistant", "content": null, "tool_calls": [{ "id": "c", "type": "function",
                "function": { "name": "read_file", "arguments": "{\"path\":\"hello.py\"}" } }] }),
            json!({ "role": "tool", "tool_call_id": "c", "content": "1  hi\n2  there" }),
            json!({ "role": "assistant", "content": "It says hi." }),
        ];
        app.event(
            1,
            task(Task::Finished {
                result: "It says hi.".into(),
                error: false,
                messages: report,
            }),
            60,
            0,
        );
        assert!(
            app.frame(60, 40, 0)
                .live
                .join("\n")
                .contains("Read hello · done · 1 tool")
        );

        // ↓ into the panel, ↓ to the subagent, ⏎: its transcript, alone on the screen.
        app.key(Key::Down, 60, 0);
        app.key(Key::Down, 60, 0);
        assert_eq!(app.key(Key::Enter, 60, 0), None);
        let frame = app.frame(60, 40, 0);
        let shown = frame.history.join("\n");
        assert!(frame.clear);
        assert!(
            shown.contains("Agent · Read hello · done") && shown.contains("Read hello.py and report."),
            "{shown}"
        );
        assert!(
            shown.contains("• Read hello.py") && shown.contains("It says hi."),
            "{shown}"
        );
        assert!(!shown.contains("look into it"), "the main conversation is not shown");

        // esc: back to main, drawn again from its conversation.
        assert_eq!(app.key(Key::Esc, 60, 0), Some(Command::ShowMain));
        let main = vec![json!({ "role": "user", "content": "look into it" })];
        app.event(0, TurnEvent::MainView(main), 60, 0);
        let frame = app.frame(60, 40, 0);
        assert!(frame.clear && frame.history.join("\n").contains("look into it"));
        // ↑ from main leaves the panel.
        app.key(Key::Down, 60, 0);
        app.key(Key::Up, 60, 0);
        typed(&mut app, "x");
        assert_eq!(app.composer.text(), "x");
    }

    #[test]
    fn left_on_an_empty_composer_opens_the_chats() {
        let mut app = app();
        let chats = Some(Command::Load(Load::Sessions {
            current: None,
            saved: Vec::new(),
        }));
        assert_eq!(app.key(Key::Left, 60, 0), chats);
        typed(&mut app, "ab");
        assert_eq!(app.key(Key::Left, 60, 0), None, "with text, it moves the cursor");
        assert_eq!(app.composer.text(), "ab");
    }

    #[test]
    fn resume_lists_agent_conversations_and_switches_to_one() {
        let mut app = app();
        typed(&mut app, "/resume");
        assert_eq!(
            app.key(Key::Enter, 60, 0),
            Some(Command::Load(Load::Sessions {
                current: None,
                saved: Vec::new(),
            }))
        );
        let agent = Item {
            id: "agent:t-1".into(),
            label: "Fix the login".into(),
            group: Some("project".into()),
            ..Item::default()
        };
        let chat = Item {
            id: "c-1".into(),
            label: "Plans".into(),
            group: Some("Chat".into()),
            ..Item::default()
        };
        app.event(0, TurnEvent::Sessions(Ok(vec![agent, chat])), 60, 0);
        assert_eq!(app.key(Key::Enter, 60, 0), Some(Command::OpenAgent("t-1".into())));

        let messages = vec![
            json!({ "role": "user", "content": "Fix the login" }),
            json!({ "role": "assistant", "content": null, "tool_calls": [{ "id": "c", "type": "function",
                "function": { "name": "read_file", "arguments": "{}" } }] }),
            json!({ "role": "tool", "tool_call_id": "c", "content": "secret file text" }),
            json!({ "role": "assistant", "content": "Fixed it." }),
        ];
        app.frame(60, 30, 0);
        app.event(
            0,
            TurnEvent::AgentOpened(Ok(("Fix the login".into(), messages, Vec::new()))),
            60,
            0,
        );
        assert_eq!(app.mode, Mode::Agent);
        let frame = app.frame(60, 30, 0);
        assert!(frame.clear, "the screen starts again with this session only");
        assert!(!app.frame(60, 30, 0).clear, "once");
        let shown = frame.history.join("\n");
        assert!(
            shown.contains("Resumed Fix the login · agent mode") && shown.contains("Fixed it."),
            "{shown}"
        );
        // The tool shows as its card, as it did when it ran; what it read stays out.
        assert!(
            shown.contains("• Read") && !shown.contains("secret file text"),
            "{shown}"
        );
    }

    #[test]
    fn slash_opens_the_list_and_enter_runs_the_chosen_command() {
        let mut app = app();
        typed(&mut app, "/s");
        let live = app.frame(60, 10, 0).live;
        assert!(live.iter().any(|line| line.starts_with("❯ /status")), "{live:#?}");
        app.key(Key::Down, 60, 0);
        assert_eq!(app.key(Key::Enter, 60, 0), Some(Command::Load(Load::Spend)));
        assert!(app.composer.is_empty());
    }

    #[test]
    fn tab_completes_and_arguments_run_directly() {
        let mut app = app();
        typed(&mut app, "/mo");
        app.key(Key::Tab, 60, 0);
        assert_eq!(app.composer.text(), "/model ");
        typed(&mut app, "openai/gpt-6-mini");
        assert_eq!(app.key(Key::Enter, 60, 0), None);
        assert_eq!(app.requested.as_deref(), Some("openai/gpt-6-mini"));
        assert_eq!(app.model, "openai/gpt-6-mini");
    }

    #[test]
    fn an_unknown_command_says_so() {
        let mut app = app();
        typed(&mut app, "/nope now");
        assert_eq!(app.key(Key::Enter, 60, 0), None);
        let history = app.frame(60, 10, 0).history;
        assert!(
            history.contains(&"■ There is no /nope command.".to_string()),
            "{history:#?}"
        );
    }

    #[test]
    fn the_model_list_chooses_a_model() {
        let mut app = app();
        typed(&mut app, "/model");
        assert_eq!(
            app.key(Key::Enter, 60, 0),
            Some(Command::Load(Load::Models { current: None }))
        );
        let item = |id: &str| Item {
            id: id.into(),
            label: id.into(),
            detail: String::new(),
            current: false,
            ..Item::default()
        };
        app.event(
            0,
            TurnEvent::Models(Ok(vec![item("nexus/auto"), item("openai/gpt-6-mini")])),
            60,
            0,
        );
        assert!(
            app.frame(60, 10, 0)
                .live
                .iter()
                .any(|line| line.contains("Choose a model"))
        );
        app.key(Key::Down, 60, 0);
        app.key(Key::Enter, 60, 0);
        assert_eq!(app.requested.as_deref(), Some("openai/gpt-6-mini"));
        assert!(app.list.is_none());
    }

    /// The suggestion comes after the answer, from its own call: offered in the empty composer for the
    /// session on screen, and its cost counted.
    #[test]
    fn a_suggestion_after_the_answer_waits_in_the_composer() {
        let mut app = app();
        typed(&mut app, "hi");
        let Some(Command::Send { prompt, suggest, .. }) = app.key(Key::Enter, 60, 0) else {
            panic!("a prompt is sent");
        };
        assert_eq!((prompt.as_str(), suggest), ("hi", true));
        app.event(1, TurnEvent::Session("chs_1".into()), 60, 5);
        app.event(1, TurnEvent::Delta("Hello there.".into()), 60, 10);
        app.event(1, TurnEvent::Done(Ok(json!({ "model": "m", "costUsd": 0 }))), 60, 30);
        let suggested = |session: &str| TurnEvent::Suggested {
            session: session.into(),
            suggestion: Some("Tell me more".into()),
            model: "m".into(),
            cost: 0.001,
        };
        app.event(1, suggested("chs_other"), 60, 35);
        assert_eq!(app.suggestion, None, "another session's");
        app.event(1, suggested("chs_1"), 60, 40);
        let frame = app.frame(60, 10, 40);
        assert!(!frame.history.concat().contains("Tell me more"));
        assert!(
            frame.live.iter().any(|line| line.contains("Tell me more")),
            "it waits in the composer"
        );
        assert!((app.spent - 0.002).abs() < 1e-9, "both calls are counted");
        app.key(Key::Tab, 60, 50);
        assert_eq!(app.composer.text(), "Tell me more");
    }

    #[test]
    fn a_late_suggestion_is_not_offered_and_one_with_no_call_is_not_listed() {
        let mut app = app();
        app.session = Some("chs_1".into());
        let suggested = |suggestion: Option<&str>, model: &str, cost: f64| TurnEvent::Suggested {
            session: "chs_1".into(),
            suggestion: suggestion.map(str::to_string),
            model: model.into(),
            cost,
        };
        // Turn 2 was sent and answered before turn 1's suggestion came.
        app.turns = 2;
        app.event(1, suggested(Some("Old"), "m", 0.001), 60, 0);
        assert_eq!(app.suggestion, None, "turn 1's, after turn 2's answer");
        assert!((app.spent - 0.001).abs() < 1e-9, "its cost still counts");
        let listed = app.costs.len();
        app.event(2, suggested(None, "", 0.0), 60, 0);
        assert_eq!(app.costs.len(), listed, "no call, no cost row");
    }

    #[test]
    fn typing_dismisses_the_suggestion_and_off_asks_for_none() {
        let mut app = app();
        app.suggestion = Some("Tell me more".into());
        typed(&mut app, "x");
        assert_eq!(app.suggestion, None);
        app.composer.clear();
        typed(&mut app, "/suggestions off");
        assert_eq!(app.key(Key::Enter, 60, 0), Some(Command::SaveSuggestions(false)));
        typed(&mut app, "hi");
        assert!(matches!(
            app.key(Key::Enter, 60, 0),
            Some(Command::Send { suggest: false, .. })
        ));
    }
}

#[cfg(test)]
mod mention_tests {
    use super::*;
    use crate::style::Style;

    fn app() -> App {
        App::new(Theme::new(Style::new(false), None), "m".into(), None, Vec::new())
    }

    fn typed(app: &mut App, text: &str) -> Option<Command> {
        let mut last = None;
        for c in text.chars() {
            last = app.key(Key::Char(c), 80, 0).or(last);
        }
        last
    }

    #[test]
    fn the_first_at_lists_files_and_tab_picks_one() {
        let mut app = app();
        assert_eq!(typed(&mut app, "read @"), Some(Command::ListFiles));
        assert_eq!(typed(&mut app, "ma"), None, "listed once");
        let files = vec!["src/main.rs".to_string(), "README.md".to_string()];
        app.event(0, TurnEvent::Files(files), 80, 0);
        let live = app.frame(80, 10, 0).live;
        assert!(live.iter().any(|line| line == "❯ src/main.rs"), "{live:#?}");
        app.key(Key::Tab, 80, 0);
        assert_eq!(app.composer.text(), "read @src/main.rs ");
    }

    #[test]
    fn a_pasted_image_goes_while_its_label_stays() {
        let mut app = app();
        assert_eq!(app.key(Key::PasteImage, 80, 0), Some(Command::PasteImage));
        let image = serde_json::json!({ "data": "iVBORw0K", "name": "pasted.png" });
        app.event(0, TurnEvent::Image(Ok(image.clone())), 80, 0);
        app.event(0, TurnEvent::Image(Ok(image.clone())), 80, 0);
        assert_eq!(app.composer.text(), "[Image #1] [Image #2] ");
        app.composer.replace(0, 11, "");
        typed(&mut app, "what is this?");
        let Some(Command::Send { images, .. }) = app.key(Key::Enter, 80, 0) else {
            panic!("a prompt is sent");
        };
        assert_eq!(images, [image], "image 1's label was deleted, so only image 2 goes");
    }
}

#[cfg(test)]
mod agent_tests {
    use super::*;
    use crate::style::Style;
    use crate::tui::agent::Summary;
    use crate::tui::agent::tools::Action;

    fn agent() -> App {
        let mut app = App::new(
            Theme::new(Style::new(false), None),
            "default model".into(),
            None,
            Vec::new(),
        );
        app.key(Key::BackTab, 80, 0);
        app
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            app.key(Key::Char(c), 80, 0);
        }
    }

    #[test]
    fn shift_tab_switches_to_the_agent_and_prompts_go_to_it() {
        let mut app = agent();
        assert_eq!(app.mode, Mode::Agent);
        let footer = app.frame(80, 10, 0).live.last().cloned().unwrap_or_default();
        assert!(footer.contains("agent mode (⇧tab to cycle)"), "{footer}");
        typed(&mut app, "fix the tests");
        assert_eq!(
            app.key(Key::Enter, 80, 0),
            Some(Command::Agent {
                turn: 1,
                prompt: "fix the tests".into(),
                model: None,
                images: Vec::new(),
            })
        );
    }

    #[test]
    fn an_approval_waits_and_always_answers_the_next_one() {
        let mut app = agent();
        typed(&mut app, "run them");
        app.key(Key::Enter, 80, 0);
        let run = || Ask::Run {
            command: "cargo test".into(),
        };
        assert_eq!(app.event(1, TurnEvent::Approve(run()), 80, 0), None);
        let live = app.frame(80, 10, 0).live;
        assert!(
            live.iter().any(|line| line.contains("Allow this command?")),
            "{live:#?}"
        );
        assert_eq!(app.key(Key::Char('a'), 80, 0), Some(Command::Decide(Decision::Always)));
        assert_eq!(
            app.event(1, TurnEvent::Approve(run()), 80, 0),
            Some(Command::Decide(Decision::Allow))
        );
        let edit = Ask::Edit {
            path: "a.rs".into(),
            diff: Vec::new(),
        };
        assert_eq!(app.event(1, TurnEvent::Approve(edit), 80, 0), None, "edits still ask");
        assert_eq!(app.key(Key::Esc, 80, 0), Some(Command::Decide(Decision::Deny)));
    }

    #[test]
    fn tools_and_the_end_of_a_turn_show() {
        let mut app = agent();
        typed(&mut app, "look");
        app.key(Key::Enter, 80, 0);
        app.frame(80, 10, 0);
        app.event(1, TurnEvent::Delta("Let me look.".into()), 80, 10);
        app.event(
            1,
            TurnEvent::Tool(Action::Read {
                path: "src/main.rs".into(),
                lines: 3,
            }),
            80,
            20,
        );
        app.event(1, TurnEvent::Delta("It prints hi.".into()), 80, 30);
        let summary = Summary {
            model: Some("openai/gpt-6".into()),
            cost_usd: 0.01,
            steps: 2,
            stopped: false,
        };
        app.event(1, TurnEvent::Finished(Ok(summary)), 80, 2_000);
        let history = app.frame(80, 10, 2_000).history;
        let at = |text: &str| history.iter().position(|line| line == text);
        assert!(
            at("• Let me look.") < at("• Read src/main.rs (3 lines)"),
            "{history:#?}"
        );
        assert!(at("• Read src/main.rs (3 lines)") < at("• It prints hi."));
        assert!(
            history
                .last()
                .is_some_and(|line| line.starts_with("  openai/gpt-6 · $") && line.ends_with(" · 2 steps · 2s"))
        );
        assert!(!app.running());
    }

    #[test]
    fn auto_runs_edits_and_commands_without_asking() {
        let mut app = agent();
        typed(&mut app, "/approvals auto");
        app.key(Key::Enter, 80, 0);
        let footer = app.frame(80, 10, 0).live.last().cloned().unwrap_or_default();
        assert!(footer.contains("agent mode · auto (⇧tab to cycle)"), "{footer}");
        typed(&mut app, "go");
        app.key(Key::Enter, 80, 0);
        let edit = Ask::Edit {
            path: "a.rs".into(),
            diff: Vec::new(),
        };
        assert_eq!(
            app.event(1, TurnEvent::Approve(edit), 80, 0),
            Some(Command::Decide(Decision::Allow))
        );
        let run = Ask::Run {
            command: "cargo test".into(),
        };
        assert_eq!(
            app.event(1, TurnEvent::Approve(run), 80, 0),
            Some(Command::Decide(Decision::Allow))
        );
    }

    #[test]
    fn the_approvals_list_sets_auto_edit() {
        let mut app = agent();
        typed(&mut app, "/approvals");
        app.key(Key::Enter, 80, 0);
        assert!(
            app.frame(80, 10, 0)
                .live
                .iter()
                .any(|line| line.contains("What may the agent do without asking?"))
        );
        app.key(Key::Down, 80, 0);
        app.key(Key::Enter, 80, 0);
        assert_eq!(app.approvals(), "auto-edit");
        typed(&mut app, "go");
        app.key(Key::Enter, 80, 0);
        let run = Ask::Run {
            command: "rm -rf build".into(),
        };
        assert_eq!(app.event(1, TurnEvent::Approve(run), 80, 0), None, "commands still ask");
    }

    #[test]
    fn esc_stops_the_agent() {
        let mut app = agent();
        typed(&mut app, "go");
        app.key(Key::Enter, 80, 0);
        assert_eq!(app.key(Key::Esc, 80, 0), Some(Command::Stop));
        assert!(!app.running());
        assert_eq!(
            app.event(1, TurnEvent::Approve(Ask::Run { command: "ls".into() }), 80, 0),
            Some(Command::Decide(Decision::Deny)),
            "a stopped turn's question gets no"
        );
    }
}

#[cfg(test)]
mod logo_tests {
    use super::*;
    use crate::style::Style;

    #[test]
    fn the_mark_loops_until_typing_then_stops_and_dims() {
        let mut app = App::new(Theme::new(Style::new(false), None), "m".into(), None, Vec::new());
        let frame = app.frame(80, 40, 0);
        let (columns, rows) = logo::stage(80, 30, logo::ASPECT).expect("room for the mark");
        assert_eq!(frame.live.len(), 1 + rows + 5);
        assert!(
            frame.live[1..=rows]
                .iter()
                .any(|line| line.starts_with(&" ".repeat(10))),
            "centred"
        );
        assert_eq!(frame.cursor, (rows + 3, 2));
        assert_eq!(app.frame(80, 15, 0).live.len(), 5, "no room: no mark");
        assert!(app.animating(logo::LOOP_MS * 5), "it loops while the composer is empty");
        app.key(Key::Char('h'), 80, 8_000);
        assert!(app.animating(8_100), "it fades");
        assert!((app.opacity(8_000 + logo::FADE_MS) - logo::DIM).abs() < 1e-9);
        assert!(!app.animating(9_000), "then holds still");
        let still = app.frame(80, 40, 9_000).live;
        assert_eq!(
            app.frame(80, 40, 9_700).live,
            still,
            "it holds still while the person types"
        );
        let paint = logo::Paint {
            background: None,
            depth: None,
            aspect: logo::ASPECT,
        };
        let front: Vec<String> = logo::lines(columns, rows, logo::FRONT_MS, logo::DIM, paint);
        let shown: Vec<&str> = still[1..=rows].iter().map(|line| line.trim()).collect();
        assert_eq!(
            shown,
            front.iter().map(|line| line.trim()).collect::<Vec<_>>(),
            "F5 as drawn"
        );
        app.key(Key::Enter, 80, 10_000);
        assert!(!app.animating(10_000));
        // The mark is gone; the status line's braille spinner is all that turns.
        let braille = |line: &&String| line.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c));
        let live = app.frame(80, 40, 10_000).live;
        let left: Vec<&String> = live.iter().filter(braille).collect();
        assert!(left.len() == 1 && left[0].contains("esc to interrupt"), "{left:?}");
    }
}
