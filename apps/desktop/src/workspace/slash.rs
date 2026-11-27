//! The slash menu of a Code session's composer, as the Nexus app's (its slash commands and slash menu, and
//! Nexus UI's composer styles, `.wb-slash`): a "/" and a word at the start of the message lists
//! the commands that match it, Nexus's own first ("Commands") and then the agent's ("Skills"). Further into the message,
//! a "/word" at the caret lists the agent's commands only: Nexus's run only as a whole message. Up and Down move, Enter
//! or Tab or a click puts "/name " in place of the word, Esc closes the menu for that text.
//!
//! The agent's names are what Claude Code reported when it started (its `commands` event, kept in the session's log):
//! a new session, or one whose agent has not started yet, lists Nexus's commands only. Codex reports none.
//!
//! Nexus commands run local controls or typed session commands. Side chats remain separate work. `/name <name>` sets the
//! open session's unique name through the same update as the store's other session changes, `/name --clear` clears it
//! and `/name` alone shows it. `/rename` is the same command: it replaces Claude Code's own `/rename`, which the menu
//! no longer lists (`REPLACED`).

use crate::workspace::menu_item_hover;
use crate::workspace::overlay::OverlaySlot;
use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::api::UpdateArgs;
use nexus_workbench::event::SessionAgent;

use super::code_dock::Menu;
use super::new_code::LaunchMenu;
use super::settings_view::SettingsSection;
use super::side_pane::Tool;
use super::{View, Workspace};
use crate::file_icons::FileIcon;
use crate::palette::Palette;

// Declared here, not in `workspace/mod.rs`.
#[path = "mention.rs"]
pub(super) mod mention;
use mention::{MentionQuery, MentionState};

/// What a Nexus command opens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Action {
    Browser,
    Terminal,
    Status,
    Model,
    Effort,
    Permissions,
    Mcp,
    Help,
    SideChat,
    Compact,
    Restart,
    /// The session's unique name: shown alone, set or cleared with an argument (`Submission::Name`).
    Name,
}

/// Nexus's own commands (`UI_COMMANDS`), in the Nexus app's order, with its hints.
const UI_COMMANDS: [(Action, &str, &str); 14] = [
    (Action::Browser, "browser", "Open this session’s browser"),
    (Action::Terminal, "terminal", "Open a terminal in this session’s folder"),
    (Action::Status, "status", "Show this session’s connection details"),
    (Action::Model, "model", "Open the Nexus model picker"),
    (Action::Effort, "effort", "Open reasoning effort and permissions"),
    (Action::Permissions, "permissions", "Open the Nexus permission mode"),
    (Action::Compact, "compact", "Compact this conversation"),
    (Action::Restart, "restart", "End the agent; the next message resumes it"),
    (
        Action::Name,
        "name",
        "Name this session so other sessions and the CLI can reach it",
    ),
    (
        Action::Name,
        "rename",
        "Same as /name: name this session so other sessions and the CLI can reach it",
    ),
    (Action::Mcp, "mcp", "Open this computer's MCP settings"),
    (Action::Help, "help", "Show Nexus keyboard shortcuts"),
    (Action::SideChat, "btw", "Ask about this session in a side chat"),
    (Action::SideChat, "side", "Open a side chat"),
];

/// Nexus's commands as the search palette lists them (the Nexus app's palette runs `slashCommands`' controls too).
pub(super) fn palette_commands() -> [(Action, &'static str, &'static str); 14] {
    UI_COMMANDS
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct SlashCommand {
    pub name: String,
    pub hint: String,
    /// A Nexus command's control; None for the agent's.
    pub action: Option<Action>,
    /// Why it can't run now: the row is dimmed, and picking it says this.
    pub reason: Option<&'static str>,
}

/// The menu's state between frames.
#[derive(Default)]
pub(super) struct SlashState {
    /// The highlighted row, for `query`: another query starts again at the top.
    active: usize,
    query: String,
    /// The composer's text when Esc closed the menu: it stays closed until the text changes (then this clears, so
    /// typing the same text again opens it).
    dismissed: RefCell<Option<String>>,
    scroll: ScrollHandle,
    /// The "@" and "#" menu (`mention.rs`), which shares this one's popup.
    pub(super) mention: MentionState,
}

/// The agent's commands that a Nexus command of the same name replaces: the menu lists only Nexus's, with no
/// `agent:` alias. Any other clash keeps the agent's command as `agent:<name>`.
const REPLACED: [&str; 1] = ["rename"];

/// The word typed after a leading "/" (`slashQuery`), lowercase, or None when the text is not a bare slash command in
/// progress: a space, a second line or any other character closes the menu.
pub(super) fn slash_query(text: &str) -> Option<String> {
    let rest = text.strip_prefix('/')?;
    rest.chars().all(command_char).then(|| rest.to_lowercase())
}

fn command_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == ':' || c == '-'
}

/// The "/" command being typed at `caret`, in bytes of the composer's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SlashAt {
    /// The word, from the "/" to the caret; `query` is what follows the "/", lowercase.
    pub at: MentionQuery,
    /// The word is not at the message's start: it goes to the agent as text, so only the agent's commands fit.
    pub inline: bool,
}

/// The "/" command at `caret`, or None. A message that is only "/word" is one wherever the caret is. Elsewhere the
/// word must start the text or follow a space or a line break (so "src/main" opens nothing), and the caret must end it.
pub(super) fn slash_at(text: &str, caret: usize) -> Option<SlashAt> {
    if let Some(query) = slash_query(text) {
        let at = MentionQuery {
            query,
            start: 0,
            end: text.len(),
        };
        return Some(SlashAt { at, inline: false });
    }
    let end = mention::clamp(text, caret);
    if text[end..].chars().next().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    let start = mention::word_start(text, end);
    let rest = text[start..end].strip_prefix('/')?;
    rest.chars().all(command_char).then(|| SlashAt {
        at: MentionQuery {
            query: rest.to_lowercase(),
            start,
            end,
        },
        inline: start > 0,
    })
}

/// The commands for `agent` (`slashCommands`): Nexus's, then the agent's reported ones (only an agent whose harness
/// reads its native commands), each once, renamed `agent:<name>` where it would shadow one of Nexus's.
pub(super) fn slash_commands(agent: SessionAgent, reported: Option<&[String]>, busy: bool) -> Vec<SlashCommand> {
    let harness = nexus_workbench::harness::harness(agent);
    let mut commands: Vec<SlashCommand> = UI_COMMANDS
        .iter()
        .map(|&(action, name, hint)| SlashCommand {
            name: name.into(),
            hint: hint.into(),
            action: Some(action),
            reason: None,
        })
        .collect();
    if harness.session_features().native_commands.is_none() {
        return commands;
    }
    let mut used: Vec<String> = commands.iter().map(|command| command.name.to_lowercase()).collect();
    let reserved = used.clone();
    let mut seen: Vec<&String> = Vec::new();
    for name in reported.unwrap_or_default() {
        let valid = !name.is_empty()
            && !name.starts_with("__")
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == ':' || c == '-');
        if !valid || seen.contains(&name) || REPLACED.contains(&name.to_lowercase().as_str()) {
            continue;
        }
        seen.push(name);
        let mut alias = name.clone();
        if reserved.contains(&name.to_lowercase()) {
            alias = format!("agent:{name}");
            while used.contains(&alias.to_lowercase()) {
                alias = format!("agent:{alias}");
            }
        }
        used.push(alias.to_lowercase());
        commands.push(SlashCommand {
            name: alias,
            hint: format!("{} command: /{name}", harness.label()),
            action: None,
            reason: busy.then_some("Wait for the current turn before running a Claude Code command."),
        });
    }
    commands
}

/// The commands with no session yet: /status says to start one, as the Nexus app's `slashCommands`.
fn no_session(mut commands: Vec<SlashCommand>) -> Vec<SlashCommand> {
    for command in &mut commands {
        if command.action == Some(Action::Status) {
            command.reason = Some("Start a session first.");
        }
    }
    commands
}

/// The commands whose names hold `query` (`matchCommands`): those that start with it first, then the rest.
pub(super) fn match_commands(commands: Vec<SlashCommand>, query: &str) -> Vec<SlashCommand> {
    let (starts, rest): (Vec<_>, Vec<_>) = commands
        .into_iter()
        .filter(|command| command.name.to_lowercase().contains(query))
        .partition(|command| command.name.to_lowercase().starts_with(query));
    starts.into_iter().chain(rest).collect()
}

/// What a message sent from the composer does (`commandSubmission`), for a Nexus command or a disabled one. Anything
/// else (text, or an agent's command) goes to the agent as typed.
#[derive(Debug, PartialEq)]
pub(super) enum Submission {
    Action(Action),
    Refused(String),
    Compact(String),
    /// `/name <name>`: the session's new name; empty for `/name --clear`.
    Name(String),
    Text,
}

pub(super) fn submission(text: &str, commands: &[SlashCommand]) -> Submission {
    let Some(rest) = text.trim().strip_prefix('/') else {
        return Submission::Text;
    };
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let Some(command) = commands.iter().find(|command| command.name.eq_ignore_ascii_case(name)) else {
        return Submission::Text;
    };
    if let Some(reason) = command.reason {
        return Submission::Refused(reason.into());
    }
    match command.action {
        Some(Action::Compact) if !args.trim().is_empty() => Submission::Compact(args.trim().into()),
        Some(Action::Name) if args.trim() == "--clear" => Submission::Name(String::new()),
        Some(Action::Name) if !args.trim().is_empty() => Submission::Name(args.trim().into()),
        Some(_) if !args.trim().is_empty() => Submission::Refused(format!(
            "/{} opens a Nexus control and accepts no arguments.",
            command.name
        )),
        Some(action) => Submission::Action(action),
        None => Submission::Text,
    }
}

impl Workspace {
    /// The commands the composer on screen offers: the open session's agent's, or the new session's.
    fn slash_list(&self) -> Vec<SlashCommand> {
        match &self.view {
            View::Code(view) if !view.is_draft() => {
                let agent = self
                    .code_session()
                    .map_or(SessionAgent::Claude, |session| session.agent);
                let mut commands = slash_commands(agent, view.commands.as_deref(), self.code_running());
                for command in &mut commands {
                    if command.action == Some(Action::Compact) {
                        command.reason = if self.code_running() {
                            Some("Wait for the turn and queued work to finish.")
                        } else if !self.code_session().is_some_and(|session| session.compact) {
                            Some("This process or host has not reported compaction support. Resume the agent first.")
                        } else {
                            None
                        };
                    }
                }
                commands
            }
            View::Code(_) | View::NewCode => {
                let mut commands = slash_commands(self.code_form.agent, None, false);
                for command in &mut commands {
                    if matches!(command.action, Some(Action::Compact | Action::Restart | Action::Name)) {
                        command.reason = Some("Start a session first.");
                    }
                }
                no_session(commands)
            }
            _ => Vec::new(),
        }
    }

    /// The menu's query and rows while it is open: a Code composer, focused, holding "/word", not dismissed, with
    /// something matching.
    pub(super) fn slash_matches(&self, cx: &App) -> Option<(String, Vec<SlashCommand>)> {
        let (slash, rows) = self.slash_rows(cx)?;
        Some((slash.at.query, rows))
    }

    /// The "/" word at the caret and the rows for it. Inside a message the rows are the agent's commands only, and none
    /// is refused: the word goes to the agent as text, not as a command.
    fn slash_rows(&self, cx: &App) -> Option<(SlashAt, Vec<SlashCommand>)> {
        if !self.composer_focused || !matches!(self.view, View::Code(_) | View::NewCode) {
            return None;
        }
        let composer = self.composer.read(cx);
        // A selection is not a command being typed.
        if !composer.selected_range().is_empty() {
            return None;
        }
        let caret = composer.cursor();
        let text = composer.value();
        let mut dismissed = self.slash.dismissed.borrow_mut();
        match dismissed.as_deref() {
            Some(closed) if closed == text.as_ref() => return None,
            Some(_) => *dismissed = None,
            None => {}
        }
        drop(dismissed);
        let slash = slash_at(&text, caret)?;
        let mut commands = self.slash_list();
        if slash.inline {
            commands.retain(|command| command.action.is_none());
            for command in &mut commands {
                command.reason = None;
            }
        }
        let rows = match_commands(commands, &slash.at.query);
        (!rows.is_empty()).then_some((slash, rows))
    }

    fn slash_active(&self, query: &str, rows: usize) -> usize {
        if self.slash.query == query {
            self.slash.active.min(rows.saturating_sub(1))
        } else {
            0
        }
    }

    /// Up or Down while the menu is open: the next row, wrapping. False when it is closed (the key keeps its meaning).
    pub(super) fn slash_move(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some((query, rows)) = self.slash_matches(cx) else {
            return false;
        };
        let active = self.slash_active(&query, rows.len());
        self.slash.active = if down {
            (active + 1) % rows.len()
        } else {
            (active + rows.len() - 1) % rows.len()
        };
        self.slash.query = query;
        self.slash
            .scroll
            .scroll_to_item(self.slash_row_child(&rows, self.slash.active));
        cx.notify();
        true
    }

    /// Esc while the menu is open: it closes for this text.
    pub(super) fn slash_dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        if self.slash_matches(cx).is_none() {
            return false;
        }
        *self.slash.dismissed.borrow_mut() = Some(self.composer.read(cx).value().to_string());
        cx.notify();
        true
    }

    /// Enter or Tab while the menu is open: the highlighted command goes in the composer. Enter on a command already
    /// typed in full sends it instead (`menuKeyAction`), so it is false then.
    pub(super) fn slash_complete_active(&mut self, enter: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((query, rows)) = self.slash_matches(cx) else {
            return false;
        };
        let command = rows[self.slash_active(&query, rows.len())].clone();
        if enter && command.name.to_lowercase() == query {
            return false;
        }
        self.slash_complete(&command, window, cx);
        true
    }

    /// Put "/name " in place of the "/" word at the caret, or say why the command can't run.
    fn slash_complete(&mut self, command: &SlashCommand, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(reason) = command.reason {
            self.composer_note = Some(reason.into());
            cx.notify();
            return;
        }
        let Some((slash, _)) = self.slash_rows(cx) else {
            return;
        };
        let at = slash.at;
        let text = self.composer.read(cx).value();
        let (next, caret) = mention::insert_token(&text, &at, &format!("/{}", command.name));
        let inserted = next[at.start..next.len() - (text.len() - at.end)].to_string();
        self.composer.update(cx, |state, cx| {
            // Only the typed word is replaced, so undo takes the pick back.
            state.set_selected_range(at.start..at.end, cx);
            state.replace(inserted, window, cx);
            state.set_selected_range(caret..caret, cx);
            state.focus(window, cx);
        });
        self.composer_empty = false;
        self.slash.active = 0;
        cx.notify();
    }

    /// A message that names a Nexus command opens its control instead of going to the agent; a disabled command says
    /// why. True when the message was taken here.
    pub(super) fn slash_submit(&mut self, prompt: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(question) = super::side_chat::side_question(prompt) {
            if !matches!(self.view, View::Code(_)) {
                self.composer_note = Some("Start a session first.".into());
                cx.notify();
            } else {
                self.create_side_chat((!question.is_empty()).then(|| question.to_string()), window, cx);
            }
            return true;
        }
        match submission(prompt, &self.slash_list()) {
            Submission::Text => false,
            Submission::Refused(reason) => {
                self.composer_note = Some(reason.into());
                cx.notify();
                true
            }
            Submission::Compact(instructions) => {
                // The agent's own refusal: Codex's names Codex. Nexus, agy and Cursor never reach here, because their
                // /compact is refused above until they report compaction.
                let refusal = self.code_session().and_then(|session| {
                    nexus_workbench::harness::harness(session.agent)
                        .session_features()
                        .compact_instructions_refusal
                });
                if let Some(refusal) = refusal {
                    self.composer_note = Some(refusal.into());
                } else {
                    self.composer.update(cx, |state, cx| state.set_value("", window, cx));
                    self.composer_empty = true;
                    self.session_command(Action::Compact, Some(instructions), cx);
                }
                cx.notify();
                true
            }
            Submission::Name(name) => {
                self.composer.update(cx, |state, cx| state.set_value("", window, cx));
                self.composer_empty = true;
                self.composer_note = None;
                self.name_session(name, cx);
                true
            }
            Submission::Action(action) => {
                self.composer.update(cx, |state, cx| state.set_value("", window, cx));
                self.composer_empty = true;
                self.composer_note = None;
                self.run_slash_action(action, window, cx);
                true
            }
        }
    }

    pub(super) fn run_slash_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_slash_popups(cx);
        let session = matches!(self.view, View::Code(_));
        match action {
            Action::Browser => self.pick_tool(Tool::Browser, window, cx),
            Action::Terminal => self.pick_tool(Tool::Terminal, window, cx),
            Action::Status => self.show_connection(window, cx),
            Action::Model if session => self.open_dock_menu(Menu::Model, window, cx),
            Action::Effort | Action::Permissions if session => self.open_dock_menu(Menu::Effort, window, cx),
            Action::Model => self.open_launch_menu(LaunchMenu::Model, window, cx),
            Action::Effort | Action::Permissions => self.open_launch_menu(LaunchMenu::Model, window, cx),
            Action::Mcp => self.open_settings(SettingsSection::Page(crate::pages::PageId::Mcp), window, cx),
            Action::Help => self.open_settings(SettingsSection::Shortcuts, window, cx),
            Action::SideChat => self.create_side_chat(None, window, cx),
            Action::Compact | Action::Restart => self.session_command(action, None, cx),
            Action::Name => {
                self.composer_note = Some(match self.code_session() {
                    Some(session) => name_note(session.name.as_deref()).into(),
                    None => "Start a session first.".into(),
                });
                cx.notify();
            }
        }
    }

    /// `/name <name>`, or "" to clear it, through `change_session`: a refused name (taken, or not a name) comes back
    /// as the runner's error under the composer, and the store keeps the old name.
    fn name_session(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(session) = self.code_session() else {
            self.composer_note = Some("Start a session first.".into());
            cx.notify();
            return;
        };
        let thread_id = session.id.clone();
        self.change_session(
            UpdateArgs {
                thread_id,
                name: Some(name),
                ..UpdateArgs::default()
            },
            cx,
        );
    }

    /// A stale model popup must not take Enter or arrows from a slash command in the composer.
    pub(super) fn prepare_slash_key(&mut self, window: &Window, cx: &mut Context<Self>) {
        if matches!(self.view, View::Code(_) | View::NewCode) && self.composer.focus_handle(cx).is_focused(window) && {
            let composer = self.composer.read(cx);
            slash_at(&composer.value(), composer.cursor()).is_some()
        } {
            self.dismiss_slash_popups(cx);
        }
    }

    fn dismiss_slash_popups(&mut self, cx: &mut Context<Self>) {
        let changed = self.overlay.model_menu() || self.overlay.launch_menu().is_some();
        self.overlay.close_model_menu();
        self.overlay.close_launch_menu();
        self.close_dock_menu(cx);
        if changed {
            cx.notify();
        }
    }

    /// Slash and palette commands use the same validated local/service/server API.
    fn session_command(&mut self, action: Action, instructions: Option<String>, cx: &mut Context<Self>) {
        let Some(session) = self.code_session() else {
            self.composer_note = Some("Start a session first.".into());
            cx.notify();
            return;
        };
        if action == Action::Compact && (self.code_running() || !session.compact) {
            self.composer_note =
                Some("Wait for the turn to finish and resume an agent with compaction support.".into());
            cx.notify();
            return;
        }
        let id = session.id.clone();
        let Some(runs) = self.runs(&id) else {
            self.refuse_read_only();
            cx.notify();
            return;
        };
        let read = if action == Action::Compact {
            runs.call(
                "wb_compact",
                serde_json::json!({"threadId": id, "instructions": instructions}),
            )
        } else {
            runs.call("wb_stop", serde_json::json!({"threadId": id, "restart": true}))
        };
        cx.spawn(async move |this, cx| {
            let result = read
                .await
                .unwrap_or_else(|_| Err("The command's answer was lost.".into()));
            let _ = this.update(cx, |this, cx| {
                if this.code_session().is_some_and(|session| session.id == id) {
                    this.composer_note = result.err().map(Into::into);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn open_launch_menu(&mut self, menu: LaunchMenu, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.launch_menu() != Some(menu) {
            self.toggle_launch_menu(menu, window, cx);
        }
    }

    fn slash_row_child(&self, rows: &[SlashCommand], active: usize) -> usize {
        menu_row_child(rows.iter().map(|row| row.action.is_some()), active)
    }

    /// The slash menu: "Commands" then "Skills", each row "/name" and its hint (or why it can't run, dimmed).
    pub(super) fn render_slash_menu(
        &self,
        below: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.slash_matches(cx);
        let shown = open.as_ref().map(|(query, rows)| {
            let menu_rows: Vec<MenuRow> = rows
                .iter()
                .map(|row| {
                    let command = row.action.is_some();
                    MenuRow {
                        group: if command { "Commands" } else { "Skills" },
                        icon: if command {
                            Lucide::SquareTerminal
                        } else {
                            Lucide::Sparkles
                        }
                        .into(),
                        name: format!("/{}", row.name).into(),
                        hint: row.reason.map_or(row.hint.clone(), str::to_string).into(),
                        dim: row.reason.is_some(),
                    }
                })
                .collect();
            (menu_rows, self.slash_active(query, rows.len()))
        });
        // A closed menu leaves with the rows it showed last (`motion::presence_of`); it takes no picks then.
        let (phase, (menu_rows, active)) = crate::motion::presence_of("slash-menu", shown, cx)?;
        let rows = open.map(|(_, rows)| rows).unwrap_or_default();
        Some(render_menu(
            "slash-menu",
            &self.slash.scroll,
            menu_rows,
            active,
            below,
            phase,
            palette,
            move |this: &mut Workspace, ix, window, cx| {
                if let Some(picked) = rows.get(ix) {
                    this.slash_complete(picked, window, cx);
                }
            },
            cx,
        ))
    }
}

/// What `/name` alone says under the composer.
pub(super) fn name_note(name: Option<&str>) -> String {
    match name {
        Some(name) => format!("This session is named {name}."),
        None => "This session has no name. Type /name and a name to give it one.".into(),
    }
}

/// One row of a composer menu (slash or mention): its group's label, glyph, name, and the hint after it.
#[derive(Clone)]
pub(super) struct MenuRow {
    pub group: &'static str,
    pub icon: Glyph,
    pub name: SharedString,
    pub hint: SharedString,
    /// Can't be picked now: drawn at 60%.
    pub dim: bool,
}

/// A menu row's glyph: a Lucide icon in the row's text color, or a file's type icon in its own colors.
#[derive(Clone, Copy)]
pub(super) enum Glyph {
    Icon(Lucide),
    File(FileIcon),
}

impl From<Lucide> for Glyph {
    fn from(icon: Lucide) -> Self {
        Self::Icon(icon)
    }
}

/// The child index of row `active` in a menu's list, its group headers counted (a header starts each run of rows of
/// one group).
pub(super) fn menu_row_child<G: PartialEq>(groups: impl Iterator<Item = G>, active: usize) -> usize {
    let mut child = 0;
    let mut last = None;
    for (ix, group) in groups.enumerate() {
        if last.as_ref() != Some(&group) {
            child += 1;
            last = Some(group);
        }
        if ix == active {
            return child;
        }
        child += 1;
    }
    child
}

/// A composer menu (`.wb-slash`, v5), the slash menu's and the mention menu's: 300px, the menu fill and pop shadow, a
/// group header over each run of one group, then 30px rows of an icon, the name and the hint in small mono on the
/// right. Above the composer, or below a new session's. A press on a row calls `pick` with its index. It rises from
/// the composer (or drops toward a new session's box) and leaves the same way, as `phase` says.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_menu(
    id: &'static str,
    scroll: &ScrollHandle,
    rows: Vec<MenuRow>,
    active: usize,
    below: bool,
    phase: crate::motion::Phase,
    palette: Palette,
    pick: impl Fn(&mut Workspace, usize, &mut Window, &mut Context<Workspace>) + 'static,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let pick = Rc::new(pick);
    let mut list = v_flex()
        .id(id)
        .track_scroll(scroll)
        .occlude()
        .w(px(300.))
        .max_h(px(320.))
        .overflow_y_scroll()
        .p(px(5.))
        .rounded(px(12.))
        .bg(palette.menu)
        .shadow(palette.pop_shadow(palette.border))
        .text_size(px(13.));
    let mut group = None;
    for (ix, row) in rows.into_iter().enumerate() {
        if group != Some(row.group) {
            // No rule between the groups (no hairlines): the gap says it.
            list = list.child(
                div()
                    .px(px(8.))
                    .pt(px(if group.is_some() { 14. } else { 8. }))
                    .pb(px(4.))
                    .text_size(px(11.5))
                    .text_color(palette.faint)
                    .child(row.group),
            );
            group = Some(row.group);
        }
        let on = ix == active;
        let pick = pick.clone();
        list = list.child(
            h_flex()
                .id((id, ix))
                .h(px(30.))
                .flex_none()
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .text_color(if on { palette.foreground } else { palette.muted })
                .when(on, |row| row.bg(palette.menu_hover))
                .when(row.dim, |row| row.opacity(0.6))
                .cursor_pointer()
                .hover(|style| menu_item_hover(style, palette))
                // On press, not click: the composer keeps the focus (the Nexus app's mousedown).
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        pick(this, ix, window, cx);
                    }),
                )
                .child(match row.icon {
                    Glyph::Icon(icon) => Icon::new(icon)
                        .size(px(14.))
                        .text_color(if on { palette.foreground } else { palette.icon })
                        .into_any_element(),
                    Glyph::File(icon) => img(icon.asset(palette.dark))
                        .flex_none()
                        .size(px(14.))
                        .into_any_element(),
                })
                // The name first: it keeps up to 70% of the row, and the hint takes what is left, on the right. A row
                // without a hint (a session) gives the name all of it.
                .child(
                    div()
                        .flex_none()
                        .max_w(relative(if row.hint.is_empty() { 1. } else { 0.7 }))
                        .truncate()
                        .child(row.name),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_right()
                        .truncate()
                        .font_family("Geist Mono")
                        .text_size(px(11.))
                        .text_color(palette.faint)
                        .child(row.hint),
                ),
        );
    }
    let layer = deferred(
        anchored()
            .anchor(if below { Anchor::TopLeft } else { Anchor::BottomLeft })
            .offset(point(px(0.), px(if below { 8. } else { -8. })))
            .snap_to_window_with_margin(px(12.))
            .child(phase.menu(id, if below { -4. } else { 4. }, list)),
    )
    .with_priority(2);
    let corner = div().absolute().left_0();
    if below { corner.bottom_0() } else { corner.top_0() }
        .child(layer)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Action, Submission, match_commands, name_note, slash_at, slash_commands, slash_query, submission};
    use crate::workspace::overlay::{Overlay, OverlaySlot};
    use nexus_workbench::event::SessionAgent;

    #[test]
    fn the_menu_opens_on_a_bare_slash_word() {
        assert_eq!(slash_query("/"), Some(String::new()));
        assert_eq!(slash_query("/Vercel:De"), Some("vercel:de".into()));
        assert_eq!(slash_query("/tdd now"), None);
        assert_eq!(slash_query("hi /tdd"), None);
        assert_eq!(slash_query("/a\nb"), None);
    }

    #[test]
    fn the_menu_opens_on_a_slash_word_anywhere_at_the_caret() {
        let at = |text: &str| slash_at(text, text.len()).map(|slash| (slash.at.query, slash.at.start, slash.inline));
        assert_eq!(at("/tdd"), Some(("tdd".into(), 0, false)));
        assert_eq!(at("run /TD"), Some(("td".into(), 4, true)));
        assert_eq!(at("first line\n/re"), Some(("re".into(), 11, true)));
        assert_eq!(at("open src/main"), None);
        assert_eq!(at("/tdd now"), None);
        assert_eq!(at("a /b/c"), None);
        // The caret must end the word: the menu stays shut while it sits inside one.
        assert_eq!(slash_at("run /tdd now", 7), None);
        let args = slash_at("/tdd now", 4).unwrap();
        assert_eq!((args.at.query.as_str(), args.inline), ("tdd", false));
    }

    #[test]
    fn commands_start_with_nexus_and_rename_clashes() {
        let reported = [
            "tdd".to_string(),
            "model".to_string(),
            "__hidden".to_string(),
            "tdd".to_string(),
        ];
        let names: Vec<_> = slash_commands(SessionAgent::Claude, Some(&reported), false)
            .into_iter()
            .map(|command| command.name)
            .collect();
        assert_eq!(
            names[..14],
            [
                "browser",
                "terminal",
                "status",
                "model",
                "effort",
                "permissions",
                "compact",
                "restart",
                "name",
                "rename",
                "mcp",
                "help",
                "btw",
                "side"
            ]
        );
        assert_eq!(names[14..], ["tdd", "agent:model"]);
        assert_eq!(slash_commands(SessionAgent::Codex, Some(&reported), false).len(), 14);
    }

    #[test]
    fn matches_that_start_with_the_query_come_first() {
        let reported = ["review".to_string(), "code-review".to_string()];
        let names: Vec<_> = match_commands(slash_commands(SessionAgent::Claude, Some(&reported), false), "rev")
            .into_iter()
            .map(|command| command.name)
            .collect();
        assert_eq!(names, ["review", "code-review"]);
    }

    #[test]
    fn nexus_commands_open_their_control() {
        let commands = slash_commands(SessionAgent::Claude, Some(&["tdd".to_string()]), true);
        for (command, action) in [
            ("browser", Action::Browser),
            ("terminal", Action::Terminal),
            ("status", Action::Status),
            ("model", Action::Model),
            ("effort", Action::Effort),
            ("permissions", Action::Permissions),
            ("compact", Action::Compact),
            ("restart", Action::Restart),
            ("name", Action::Name),
            ("rename", Action::Name),
            ("mcp", Action::Mcp),
            ("help", Action::Help),
            ("btw", Action::SideChat),
            ("side", Action::SideChat),
        ] {
            for agent in [SessionAgent::Claude, SessionAgent::Codex, SessionAgent::Nexus] {
                assert_eq!(
                    submission(&format!("/{command}"), &slash_commands(agent, None, false)),
                    Submission::Action(action)
                );
            }
        }
        assert_eq!(submission("/compact", &commands), Submission::Action(Action::Compact));
        assert_eq!(
            submission("/compact keep the design", &commands),
            Submission::Compact("keep the design".into())
        );
        assert_eq!(submission("/restart", &commands), Submission::Action(Action::Restart));
        assert!(matches!(submission("/restart now", &commands), Submission::Refused(_)));
        assert_eq!(submission("/model", &commands), Submission::Action(Action::Model));
        assert!(matches!(submission("/model opus", &commands), Submission::Refused(_)));
        assert!(matches!(submission("/tdd go", &commands), Submission::Refused(_)));
        assert_eq!(submission("/unknown", &commands), Submission::Text);
        assert_eq!(submission("hello", &commands), Submission::Text);
        assert_eq!(submission("/status", &commands), Submission::Action(Action::Status));
        // With no session yet, /status says to start one.
        let none = super::no_session(commands);
        assert_eq!(
            submission("/status", &none),
            Submission::Refused("Start a session first.".into())
        );
    }

    #[test]
    fn name_sets_shows_and_clears_and_is_never_a_prompt() {
        let commands = slash_commands(SessionAgent::Claude, Some(&["name".to_string()]), true);
        assert_eq!(
            submission("/name orchestrator", &commands),
            Submission::Name("orchestrator".into())
        );
        assert_eq!(
            submission("/Name  orchestrator ", &commands),
            Submission::Name("orchestrator".into())
        );
        assert_eq!(submission("/name --clear", &commands), Submission::Name(String::new()));
        assert_eq!(submission("/name", &commands), Submission::Action(Action::Name));
        // A name the store will refuse is still never a prompt: the update refuses it with its reason.
        assert_eq!(
            submission("/name two words", &commands),
            Submission::Name("two words".into())
        );
        assert_eq!(name_note(Some("orchestrator")), "This session is named orchestrator.");
        assert_eq!(
            name_note(None),
            "This session has no name. Type /name and a name to give it one."
        );
    }

    #[test]
    fn rename_is_name_and_replaces_the_agents_rename() {
        let reported = ["rename".to_string(), "Rename".to_string(), "tdd".to_string()];
        let commands = slash_commands(SessionAgent::Claude, Some(&reported), false);
        // One /rename, Nexus's: the agent's is not listed, not even as agent:rename.
        let renames: Vec<_> = commands
            .iter()
            .filter(|command| command.name.to_lowercase().contains("rename"))
            .map(|command| (command.name.as_str(), command.action))
            .collect();
        assert_eq!(renames, [("rename", Some(Action::Name))]);
        assert!(commands.iter().any(|command| command.name == "tdd"));
        assert_eq!(
            submission("/rename orchestrator", &commands),
            Submission::Name("orchestrator".into())
        );
        assert_eq!(
            submission("/rename --clear", &commands),
            Submission::Name(String::new())
        );
        assert_eq!(submission("/rename", &commands), Submission::Action(Action::Name));
    }

    /// `/name` in a workspace over a real runner and store: it shows, sets and clears the open session's name, a
    /// taken name comes back as the store's refusal under the composer with the old name kept, and a new session's
    /// composer refuses it.
    #[gpui_kit::test]
    fn name_reaches_the_store_and_a_refusal_keeps_the_old_name(cx: &mut gpui_kit::TestAppContext) {
        use crate::code::{CodeProject, CodeSession};
        use crate::code_run::CodeEvent;
        use crate::workspace::{View, Workspace, code_view::CodeView, machine_source::MachineSource};
        use futures::StreamExt as _;
        use nexus_workbench::store::{Store, ThreadRecord};
        use std::rc::Rc;
        use std::sync::Arc;

        let id = uuid::Uuid::new_v4().simple().to_string();
        let data = std::env::temp_dir().join(format!("ngs-name-{}", &id[..8]));
        std::fs::create_dir_all(&data).expect("a temporary folder");
        let scope = "http://127.0.0.1:9|org-name".to_string();
        let store = Store::open(&data.join("workbench.sqlite3")).expect("the store opens");
        let now = nexus_workbench::now_ms();
        let project = store.add_project(&data, now).expect("a project");
        let summary = |id: &str, title: &str, name: &str| {
            serde_json::from_value::<nexus_workbench::event::ThreadSummary>(serde_json::json!({
                "id": id, "projectId": project.id, "title": title, "agent": "claude", "model": null,
                "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0, "nexusSessions": [],
                "contextTokens": null, "contextWindow": null, "createdAt": now, "updatedAt": now, "archived": false,
                "name": name,
            }))
            .expect("a session")
        };
        for (id, title, name) in [("s1", "Deploy the docs", "alpha"), ("s2", "Review", "taken")] {
            store
                .insert_thread(&ThreadRecord {
                    summary: summary(id, title, name),
                    native_id: format!("native-{id}"),
                    native_started: false,
                    scope: scope.clone(),
                    fork_from: None,
                    seed: None,
                    automatic_title: false,
                    delivered_seq: 0,
                    halt: None,
                })
                .expect("saved");
        }
        let name_of = |store: &Store| store.thread("s1").expect("read").expect("s1").summary.name;
        let (runner, mut events) = crate::code_run::test_runner(data.clone(), &scope);
        // The runner lives in this test's window: a running pull request watch would reach every later window test
        // from the `nexus-code` thread, and GPUI's test scheduler aborts the binary on that.
        assert!(
            !crate::pr_watch::GpuiShell::running(),
            "a test runner leaves the app's pull request watch off"
        );
        // The next runner event, or a failure after 20 seconds.
        let mut next = move || {
            crate::code_run::runtime()
                .block_on(async { tokio::time::timeout(std::time::Duration::from_secs(20), events.next()).await })
                .expect("the runner answers")
                .expect("the runner runs")
        };

        // The runner answers from its own threads, as in the app.
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let (gateway, _gateway_events) = crate::gateway::Gateway::start();
        let (_send, code_events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let session = CodeSession::of(summary("s1", "Deploy the docs", "alpha"));
        let project_id = session.project_id.clone();
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::with_machine_source(
                Arc::new(gateway),
                code_events,
                arrivals,
                false,
                MachineSource::isolated(),
                window,
                cx,
            );
            workspace._code_poll = gpui_kit::Task::ready(());
            workspace.code = None;
            workspace.runner = Some(Rc::new(runner));
            workspace.auth = crate::workspace::Auth::SignedIn;
            workspace.projects = vec![CodeProject {
                id: session.project_id.clone(),
                name: "docs".into(),
                branch: None,
                pinned: false,
                remotes: Vec::new(),
                sessions: vec![session],
                machine: None,
                added_at: 0,
            }];
            workspace.view = View::Code(CodeView::new("s1".into(), window, cx));
            // Drawing the open session reads no store in the data folder: no agents read, no project commands read.
            workspace.side.fixture_session("s1");
            workspace.side.agents = Some(crate::workspace::agents_tool::AgentsTool::resting("s1".into(), cx));
            crate::project_commands::saved(&project_id, Vec::new(), cx);
            workspace
        });

        workspace.update_in(cx, |workspace, window, cx| {
            // Shown alone: the name the session has.
            assert!(workspace.slash_submit("/name", window, cx));
            assert_eq!(workspace.composer_note.as_deref(), Some("This session is named alpha."));
            workspace.composer_note = None;
            assert!(workspace.slash_submit("/rename", window, cx));
            assert_eq!(workspace.composer_note.as_deref(), Some("This session is named alpha."));
            // Taken: never a prompt, and the update goes to the store.
            workspace
                .composer
                .update(cx, |input, cx| input.set_value("/name taken", window, cx));
            assert!(workspace.slash_submit("/name taken", window, cx));
            assert!(workspace.composer.read(cx).value().is_empty());
        });
        let refusal = loop {
            if let CodeEvent::Failed(error) = next() {
                break error;
            }
        };
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.on_code_events(vec![CodeEvent::Failed(refusal)], window, cx);
            let note = workspace.composer_note.as_deref().unwrap_or_default().to_string();
            assert!(note.contains("taken by"), "the store's refusal shows: {note}");
        });
        assert_eq!(
            name_of(&store).as_deref(),
            Some("alpha"),
            "a refused name keeps the old one"
        );

        for (prompt, want) in [
            ("/name beta", Some("beta")),
            ("/rename gamma", Some("gamma")),
            ("/name --clear", None),
        ] {
            workspace.update_in(cx, |workspace, window, cx| {
                assert!(workspace.slash_submit(prompt, window, cx));
            });
            while name_of(&store).as_deref() != want {
                if let CodeEvent::Failed(error) = next() {
                    panic!("{prompt} was refused: {error}");
                }
            }
        }

        // A new session has nothing to name yet.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.view = View::NewCode;
            for prompt in ["/name", "/name beta"] {
                workspace.composer_note = None;
                assert!(workspace.slash_submit(prompt, window, cx));
                assert_eq!(workspace.composer_note.as_deref(), Some("Start a session first."));
            }
        });
        assert_eq!(name_of(&store), None);
        drop(store);
        let _ = std::fs::remove_dir_all(data);
    }

    #[gpui_kit::test]
    fn slash_pickers_keep_their_destination_and_attachments_despite_stale_model_popups(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::workspace::{
            View, Workspace, code_dock::Menu, code_view::CodeView, machine_source::MachineSource, new_code::LaunchMenu,
        };
        use gpui_kit::Keystroke;
        use std::sync::Arc;

        let _agents = crate::code_run::fixture_agents(Vec::new());
        cx.update(gpui_kit::init);
        let (gateway, _events) = crate::gateway::Gateway::start();
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            Workspace::with_machine_source(
                Arc::new(gateway),
                events,
                arrivals,
                false,
                MachineSource::isolated(),
                window,
                cx,
            )
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.draft.files.push(crate::attachments::Attachment {
                artifact: None,
                id: 1,
                name: "draft.txt".into(),
                bytes: Arc::new(b"keep this attachment".to_vec()),
                media_type: None,
                thumbnail: None,
            });
            for session in [false, true] {
                workspace.view = if session {
                    View::Code(CodeView::new("fixture-session".into(), window, cx))
                } else {
                    View::NewCode
                };
                for (command, menu) in [
                    ("model", Menu::Model),
                    ("effort", Menu::Effort),
                    ("permissions", Menu::Effort),
                ] {
                    workspace.open_overlay(Overlay::ModelMenu);
                    workspace.open_overlay(Overlay::LaunchMenu(LaunchMenu::Model));
                    if let View::Code(view) = &mut workspace.view {
                        view.dock.menu = Some(Menu::Model);
                    }
                    let prompt = format!("/{command}");
                    workspace.composer.update(cx, |input, cx| {
                        input.set_value(prompt.clone(), window, cx);
                        input.focus(window, cx);
                    });
                    // The global key interceptor runs before the text area's Enter action.
                    workspace.on_keystroke(&Keystroke::parse("enter").unwrap(), window, cx);
                    assert_eq!(workspace.composer.read(cx).value().as_ref(), prompt);
                    assert!(!workspace.overlay.model_menu());
                    assert!(workspace.slash_submit(&prompt, window, cx));
                    if let View::Code(view) = &workspace.view {
                        assert_eq!(view.dock.menu, Some(menu));
                        assert_eq!(workspace.overlay.launch_menu(), None);
                    } else {
                        // The model panel holds the effort and the permissions too.
                        assert_eq!(workspace.overlay.launch_menu(), Some(LaunchMenu::Model));
                    }
                    assert!(workspace.composer.read(cx).value().is_empty());
                    assert_eq!(workspace.draft.files[0].bytes.as_slice(), b"keep this attachment");
                }
            }
        });
    }
}
