//! The session composer's "/" and "@"/"#" menus in the side chat (`slash.rs`, `mention.rs`), with the same rows, keys
//! and popup. "/" lists the Nexus commands a side chat runs and its agent's own; "@" lists sessions and the parent
//! session's files; "#" lists the parent folder's pull requests.
//!
//! A side chat runs its own model, effort and permission menus, and compacts or restarts its own agent. The session's
//! tools (browser, terminal, status, MCP, help) open in the workspace (`SideEvent::Slash`). `/name` and `/btw` are
//! not listed: a side chat has no unique name and no side chat of its own.
use super::super::slash::mention::{self, Choice, MentionQuery};
use super::super::slash::{
    self, Action, SlashAt, SlashCommand, Submission, render_menu, rows_for, slash_commands, submission,
};
use super::composer::Section;
use super::*;
use crate::palette::Palette;
use nexus_workbench::event::WbEvent;
use std::sync::Arc;

/// Whether the side chat offers the Nexus command `action`.
fn offered(action: Action) -> bool {
    !matches!(action, Action::Name | Action::SideChat)
}

impl SideChat {
    /// The workspace that shows this side chat: "@" reads its sessions and the parent session's folder.
    pub(in crate::workspace) fn bind_workspace(&mut self, workspace: WeakEntity<Workspace>) {
        self.workspace = Some(workspace);
    }

    /// The agent's own commands, as its process last reported them, or as its log keeps them.
    fn reported_commands(&self) -> Option<Vec<String>> {
        self.summary
            .capabilities
            .as_ref()
            .and_then(|caps| caps.commands.clone())
            .or_else(|| {
                self.events.iter().rev().find_map(|stored| match &stored.event {
                    WbEvent::Commands { commands } => Some(commands.clone()),
                    _ => None,
                })
            })
    }

    /// The commands the side chat's "/" menu offers, each with why it can't run now.
    pub(super) fn slash_list(&self) -> Vec<SlashCommand> {
        let busy = self.summary.status.busy();
        let compact = self.summary.capabilities.as_ref().is_some_and(|caps| caps.compact);
        let mut commands = slash_commands(self.summary.agent, self.reported_commands().as_deref(), busy);
        commands.retain(|command| command.action.is_none_or(offered));
        for command in &mut commands {
            if command.action == Some(Action::Compact) {
                command.reason = if busy {
                    Some("Wait for the turn and queued work to finish.")
                } else if !compact {
                    Some("This process or host has not reported compaction support. Resume the agent first.")
                } else {
                    None
                };
            }
        }
        commands
    }

    /// The composer takes keys for a menu: it has the focus and holds a message, not an answer to a request.
    fn menus_live(&self, window: &Window, cx: &App) -> bool {
        self.can_compose()
            && self.request.is_none()
            && self.secret_id.is_none()
            && self.input.focus_handle(cx).is_focused(window)
    }

    /// The "/" word at the caret and its rows, while the menu is open.
    pub(super) fn slash_rows(&self, window: &Window, cx: &App) -> Option<(SlashAt, Vec<SlashCommand>)> {
        if !self.menus_live(window, cx) {
            return None;
        }
        let input = self.input.read(cx);
        let at = self
            .menus
            .word(&input.value(), input.cursor(), !input.selected_range().is_empty())?;
        let rows = rows_for(&at, self.slash_list());
        (!rows.is_empty()).then_some((at, rows))
    }

    /// The "@" or "#" word at the caret and its rows, while the menu is open and the "/" menu is not.
    pub(super) fn mention_rows(&self, window: &Window, cx: &App) -> Option<(MentionQuery, Arc<Vec<Choice>>)> {
        if !self.menus_live(window, cx) || self.slash_rows(window, cx).is_some() {
            return None;
        }
        let workspace = self.workspace.as_ref()?.upgrade()?;
        let parent = self.summary.source_thread_id.clone()?;
        let input = self.input.read(cx);
        let state = &self.menus.mention;
        // The workspace is read only once a mention is typed.
        state.matches(
            &input.value(),
            input.cursor(),
            !input.selected_range().is_empty(),
            || state.folder_for(format!("t:{parent}"), |_| workspace.read(cx).session_folder(&parent)),
            || workspace.read(cx).mention_sessions(),
            Some(self.summary.id.as_str()),
        )
    }

    /// Up or Down while a menu is open: its next row. False when none is open, so the key keeps its meaning.
    pub(super) fn menu_move(&mut self, down: bool, window: &Window, cx: &mut Context<Self>) -> bool {
        if let Some((at, rows)) = self.slash_rows(window, cx) {
            self.menus.step(at.at.query, &rows, down);
        } else if let Some((at, rows)) = self.mention_rows(window, cx) {
            self.menus.mention.step(at, &rows, down);
        } else {
            return false;
        }
        cx.notify();
        true
    }

    /// Esc while a menu is open: it closes for this text.
    pub(super) fn menu_dismiss(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let text = self.input.read(cx).value().to_string();
        if self.slash_rows(window, cx).is_some() {
            self.menus.dismiss(text);
        } else if self.mention_rows(window, cx).is_some() {
            self.menus.mention.dismiss(text);
        } else {
            return false;
        }
        cx.notify();
        true
    }

    /// Enter or Tab while a menu is open: its highlighted row goes in the message. Enter on a command already typed
    /// in full sends it instead, so it is false then.
    pub(super) fn menu_pick(&mut self, enter: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some((at, rows)) = self.slash_rows(window, cx) {
            let command = rows[self.menus.active_for(&at.at.query, rows.len())].clone();
            if enter && command.name.to_lowercase() == at.at.query {
                return false;
            }
            self.slash_pick(&at, &command, window, cx);
            return true;
        }
        if let Some((at, rows)) = self.mention_rows(window, cx) {
            let token = rows[self.menus.mention.active_for(&at, rows.len())].token.clone();
            self.mention_pick(&at, &token, window, cx);
            return true;
        }
        false
    }

    /// Put "/name " in place of the "/" word, or say why the command can't run.
    fn slash_pick(&mut self, at: &SlashAt, command: &SlashCommand, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(reason) = command.reason {
            self.note = Some(reason.into());
        } else {
            mention::put_token(&self.input, &at.at, &format!("/{}", command.name), window, cx);
            self.menus.picked();
        }
        cx.notify();
    }

    fn mention_pick(&mut self, at: &MentionQuery, token: &str, window: &mut Window, cx: &mut Context<Self>) {
        mention::put_token(&self.input, at, token, window, cx);
        self.menus.mention.picked();
        cx.notify();
    }

    /// A message that names a Nexus command runs it here instead of going to the agent; a refused one says why and
    /// stays in the composer. True when the message was taken here.
    pub(super) fn slash_submit(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let action = match submission(text, &self.slash_list()) {
            Submission::Text | Submission::Name(_) => return false,
            Submission::Refused(reason) => {
                self.note = Some(reason);
                return true;
            }
            Submission::Compact(instructions) => Err(instructions),
            Submission::Action(action) => Ok(action),
        };
        let agent_command = matches!(action, Err(_) | Ok(Action::Compact | Action::Restart));
        if agent_command && !self.can_command() {
            self.note = Some("Wait for the message in flight, then run the command again.".into());
            return true;
        }
        self.input.update(cx, |input, cx| input.set_value("", window, cx));
        self.note = None;
        match action {
            Err(instructions) => self.compact(Some(instructions), window, cx),
            Ok(action) => self.run_slash_action(action, window, cx),
        }
        true
    }

    fn compact(&mut self, instructions: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let args = json!({"threadId": self.summary.id, "instructions": instructions});
        self.command("wb_compact", args, false, None, window, cx);
    }

    fn run_slash_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let section = match action {
            Action::Model => Some(Section::Model),
            Action::Effort | Action::Permissions => Some(Section::Effort),
            _ => None,
        };
        match (action, section) {
            (_, Some(section)) => {
                if self.settings != Some(section) {
                    self.settings(section, window, cx);
                }
            }
            (Action::Compact, _) => self.compact(None, window, cx),
            (Action::Restart, _) => {
                let args = json!({"threadId": self.summary.id, "restart": true});
                self.command("wb_stop", args, false, None, window, cx);
            }
            (action, _) => cx.emit(SideEvent::Slash(action)),
        }
    }

    /// The "/" and "@"/"#" menus over the composer, as the session's.
    pub(super) fn render_menus(&self, window: &Window, p: Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut menus = Vec::new();
        let open = self.slash_rows(window, cx);
        let shown = open
            .as_ref()
            .map(|(at, rows)| (slash::menu_rows(rows), self.menus.active_for(&at.at.query, rows.len())));
        // A closed menu leaves with the rows it showed last (`motion::presence_of`); it takes no picks then.
        if let Some((phase, (rows, active))) = crate::motion::presence_of("side-slash-menu", shown, cx) {
            menus.push(render_menu(
                "side-slash-menu",
                self.menus.scroll(),
                rows,
                active,
                false,
                phase,
                p,
                |this: &mut SideChat, ix, window, cx| {
                    if let Some((at, rows)) = this.slash_rows(window, cx)
                        && let Some(command) = rows.get(ix).cloned()
                    {
                        this.slash_pick(&at, &command, window, cx);
                    }
                },
                cx,
            ));
        }
        let open = self.mention_rows(window, cx);
        self.menus
            .mention
            .load_wanted(cx, |this: &SideChat| &this.menus.mention);
        let shown = open
            .as_ref()
            .map(|(at, rows)| (mention::menu_rows(rows), self.menus.mention.active_for(at, rows.len())));
        if let Some((phase, (rows, active))) = crate::motion::presence_of("side-mention-menu", shown, cx) {
            menus.push(render_menu(
                "side-mention-menu",
                self.menus.mention.scroll(),
                rows,
                active,
                false,
                phase,
                p,
                move |this: &mut SideChat, ix, window, cx| {
                    if let Some((at, picks)) = &open
                        && let Some(choice) = picks.get(ix)
                    {
                        this.mention_pick(at, &choice.token, window, cx);
                    }
                },
                cx,
            ));
        }
        menus
    }
}
