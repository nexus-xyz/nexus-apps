//! The search palette (Cmd+K, Ctrl+K, or the sidebar's Search), as the Nexus app's
//! (`ui/src/components/nexus-ui/search/palette.tsx`, `palette.css`, v5's `find-artifacts.css`): a dim over the window, a
//! 640px dialog 96px from the top with the query on top, then groups of rows. With no query it offers places, the
//! latest sessions and a few actions; typing matches them here and asks Gateway (`GET /v1/search`) for chats,
//! artifacts, people, teams and models. Every setting the query names shows in a Settings group
//! (`settings_search.rs`, as `sources.ts`'s `settingsItems`). Up and Down move (wrapping), Tab jumps a group, Enter
//! opens, Esc closes.
//!
//! In a Code session it also offers the Nexus app's Code actions (`desktop/src/workbench/palette.tsx`): Stop the agent's
//! turn, the snooze presets, Keep in list, Pin project, Add a project, the slash commands' controls, and a New session
//! row for each project. A session on this computer runs its
//! project commands from here ("Run <name>"), and Edit project commands opens Settings › Worktrees.
//!
//! With typed text and the Assistant here, the first row hands the text off to Nexus (brand v5 A21·e): "Hand off
//! “<text>”", with ⌘J (Ctrl+J), which does the same from the field.

use crate::workspace::glass::FloatingGround;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Input;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::chat_menu::ChatMenu;
use super::org_menu::{Org, OrgMenu};
use super::session_menu::{MenuPage, SessionMenu};
use super::side_pane::Command;
use super::slash;
use super::{SettingsSection, View, Workspace};
use crate::code;
use crate::gateway::SearchHit;
use crate::motion;
use crate::pages::PageId;
use crate::palette::Palette;
use nexus_workbench::api::UpdateArgs;
use nexus_workbench::scripts::{self, ProjectScript};

/// Gateway is asked once typing pauses this long (`DEBOUNCE_MS`).
pub(super) const DEBOUNCE: Duration = Duration::from_millis(120);
const WIDTH: f32 = 640.;
const TOP: f32 = 96.;
/// Rows per group.
const PER_GROUP: usize = 6;

/// A place or an action the palette offers (`sources.ts`'s `PAGES` and `ACTIONS`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Entry {
    title: &'static str,
    sub: Option<&'static str>,
    keywords: &'static str,
    icon: Lucide,
    go: Go,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Go {
    NewChat,
    ToggleSidebar,
    /// A page of Nexus UI, opened in the browser (under `/app/<org>/`).
    Page(&'static str),
    /// A Workbench command (`side_pane.rs`).
    Workbench(Command),
    /// The shortcuts sheet (the `?` scope).
    Shortcuts,
    /// The organizations (the `@` scope).
    Orgs,
}

const PAGES: &[Entry] = &[
    entry("Chat", None, "home new session", Lucide::MessageSquare, Go::NewChat),
    entry(
        "Artifacts",
        None,
        "library artifacts shared pages links",
        Lucide::Library,
        Go::Page("library"),
    ),
    entry(
        "Inbox",
        None,
        "notifications needs you unread approvals alerts",
        Lucide::Inbox,
        Go::Page("notifications"),
    ),
    entry(
        "Docs",
        None,
        "documentation help guide reference cli api",
        Lucide::BookOpen,
        Go::Page("docs"),
    ),
    entry(
        "Issues",
        Some("Linear"),
        "linear issues tickets triage assigned start session",
        Lucide::ListTodo,
        Go::Page("issues"),
    ),
    entry(
        "Usage",
        Some("Gateway"),
        "analytics spend usage reports cost",
        Lucide::ChartColumn,
        Go::Page("usage"),
    ),
    entry(
        "All sessions",
        None,
        "chats history",
        Lucide::MessageSquare,
        Go::Page("sessions"),
    ),
    entry(
        "Overview",
        Some("Gateway"),
        "gateway organization agent sessions",
        Lucide::LayoutGrid,
        Go::Page("overview"),
    ),
    entry(
        "Spend limits",
        Some("Usage"),
        "spend budget balance cost set limit monthly",
        Lucide::Coins,
        Go::Page("usage/limits"),
    ),
    entry(
        "Calls",
        Some("Usage"),
        "observe usage spend calls export",
        Lucide::Eye,
        Go::Page("usage/calls"),
    ),
    entry(
        "Models",
        Some("Gateway"),
        "explorer catalog policies prices allow deny model policy",
        Lucide::Cpu,
        Go::Page("models"),
    ),
    entry(
        "Teams",
        Some("Settings"),
        "groups tree",
        Lucide::UsersRound,
        Go::Page("settings/teams"),
    ),
    entry(
        "Members",
        Some("Settings"),
        "people invitations roles invite member email add person",
        Lucide::Users,
        Go::Page("settings/members"),
    ),
    entry(
        "Repositories",
        Some("Settings"),
        "github",
        Lucide::FolderGit2,
        Go::Page("settings/repositories"),
    ),
    entry(
        "API keys",
        Some("Settings"),
        "tokens create api key ci script",
        Lucide::KeyRound,
        Go::Page("settings/api-keys"),
    ),
    entry(
        "Audit log",
        Some("Settings"),
        "log changes who",
        Lucide::ScrollText,
        Go::Page("settings/audit"),
    ),
    entry(
        "Settings",
        None,
        "preferences theme dark light appearance",
        Lucide::Settings,
        Go::Page("settings/preferences"),
    ),
];

const ACTIONS: &[Entry] = &[
    entry(
        "New session",
        None,
        "new chat session ask start conversation",
        Lucide::SquarePen,
        Go::NewChat,
    ),
    entry(
        "Toggle sidebar",
        None,
        "hide show sidebar panel",
        Lucide::PanelLeft,
        Go::ToggleSidebar,
    ),
    entry(
        "Keyboard shortcuts",
        None,
        "keys keybindings hotkeys help cheatsheet",
        Lucide::SquareTerminal,
        Go::Shortcuts,
    ),
    entry(
        "Switch organization",
        None,
        "org company workspace account change",
        Lucide::Users,
        Go::Orgs,
    ),
];

/// The Workbench's commands, as Cursor's panel commands.
const WORKBENCH: &[Entry] = &[
    entry(
        "Toggle workbench",
        None,
        "workbench side pane panel show hide",
        Lucide::PanelRight,
        Go::Workbench(Command::Toggle),
    ),
    entry(
        "New terminal",
        None,
        "terminal shell console workbench",
        Lucide::SquareTerminal,
        Go::Workbench(Command::NewTerminal),
    ),
    entry(
        "Open browser",
        None,
        "browser web localhost preview workbench",
        Lucide::Globe,
        Go::Workbench(Command::Browser),
    ),
    entry(
        "Show changes",
        None,
        "changes diff git commit workbench",
        Lucide::FileDiff,
        Go::Workbench(Command::Changes),
    ),
    entry(
        "Show files",
        None,
        "files tree folder workbench",
        Lucide::Files,
        Go::Workbench(Command::Files),
    ),
    entry(
        "Expand workbench",
        None,
        "maximize full screen workbench restore",
        Lucide::Maximize2,
        Go::Workbench(Command::Maximize),
    ),
];

const fn entry(title: &'static str, sub: Option<&'static str>, keywords: &'static str, icon: Lucide, go: Go) -> Entry {
    Entry {
        title,
        sub,
        keywords,
        icon,
        go,
    }
}

/// What a row opens.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Target {
    Entry(Entry),
    /// A Gateway chat, by its index in `sessions`.
    Chat(usize),
    /// A Code session of this computer, by its id.
    Code(String),
    /// A Gateway result, by its index in `palette_hits`.
    Hit(usize),
    /// An action on the open session, as its menu has it.
    Session(SessionAction),
    /// Another organization.
    Org(Org),
    /// A Settings page, from a settings search hit.
    Setting(SettingsSection),
    SettingField(SettingsSection, &'static str),
    /// A setting this build does not draw: its Nexus UI page and control, in the browser.
    SettingOnWeb(String),
    /// Hand the typed text off to the Assistant, which sends it to Nexus.
    HandOff(String),
    /// Start an errand with the typed text: Errands opens with its New errand box (A23·e).
    NewErrand(String),
    /// One of the Assistant's own views, from an "Assistant › …" settings row (A24·e).
    AssistantView(crate::pages::PageId),
    /// A row that only informs (the shortcuts sheet).
    Info,
    /// A new Code session in a project, by its id.
    NewIn(String),
    /// Add a folder as a project.
    AddProject,
    /// Pin (true) or unpin a project, by its id.
    PinProject(String, bool),
    /// A Nexus slash command's control (`slash.rs`).
    Slash(slash::Action),
    /// Run a project command in a new terminal of a session's folder, by the session's id.
    RunCommand(String, ProjectScript),
    /// Settings › Worktrees, where the project's commands are edited.
    EditCommands,
    CopyPromptLink,
}

/// What the open session's menu offers (`session_menu.rs`, `chat_menu.rs`); each runs the menu's own function.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum SessionAction {
    /// The chat's Share sheet (`chat_share.rs`).
    Share,
    Rename,
    Pin(bool),
    Archive(bool),
    Unsnooze,
    CopyBranch(String),
    CopyMarkdown,
    Download,
    Delete,
    /// Find in the open Code session (`code_find.rs`).
    Find,
    OpenFile,
    SearchFiles,
    /// Stop the agent's turn.
    Stop,
    /// Snooze the session until this time (ms).
    Snooze(i64),
    /// Back from Settled into the list; a merge won't settle it again.
    KeepInList,
}

/// A row: its group, look and target.
#[derive(Debug, Clone)]
pub(super) struct Row {
    group: SharedString,
    icon: Lucide,
    pub(super) title: SharedString,
    sub: Option<SharedString>,
    pub(super) keys: Option<SharedString>,
    pub target: Target,
}

/// `rows` that match `query`, best first, with the group's best score.
fn scored(rows: Vec<Row>, query: &str) -> (f32, Vec<Row>) {
    let mut found: Vec<(f32, Row)> = rows
        .into_iter()
        .filter_map(|row| {
            let best = score(&row.title, query)
                .into_iter()
                .chain(
                    row.sub
                        .as_ref()
                        .and_then(|sub| score(sub, query))
                        .map(|value| value * 0.7),
                )
                .fold(None, |best: Option<f32>, value| {
                    Some(best.map_or(value, |best| best.max(value)))
                })?;
            Some((best, row))
        })
        .collect();
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    (
        found.first().map_or(0., |(best, _)| *best),
        found.into_iter().take(PER_GROUP).map(|(_, row)| row).collect(),
    )
}

/// A snooze preset's palette row: "Snooze for 1 hour", "Snooze until tonight", as the Nexus app words it.
fn snooze_title(label: &str) -> String {
    let mut letters = label.chars();
    match letters.next() {
        Some(first) => format!("Snooze {}{}", first.to_lowercase(), letters.as_str()),
        None => "Snooze".into(),
    }
}

/// How well `query` matches `text` (`sources.ts`'s scoring, simplified): 1 exact, ~0.9 prefix, ~0.8 word start,
/// 0.65 inside, 0.5 every word inside, else none.
fn score(text: &str, query: &str) -> Option<f32> {
    let text = text.to_lowercase();
    if text == query {
        Some(1.)
    } else if text.starts_with(query) {
        Some(0.9)
    } else if text
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(query))
    {
        Some(0.8)
    } else if text.contains(query) {
        Some(0.65)
    } else if query.split_whitespace().all(|word| text.contains(word)) {
        Some(0.5)
    } else {
        None
    }
}

impl Workspace {
    /// The palette's rows, group by group, for the query as typed.
    pub(super) fn palette_rows(&self, cx: &App) -> Vec<Row> {
        let raw = self.palette_query.trim().to_lowercase();
        let now = code::now_ms();
        // `>` keeps to actions, `/` to pages.
        let (scope, query) = match raw.chars().next() {
            Some('>') => (Some('>'), raw[1..].trim().to_string()),
            Some('/') => (Some('/'), raw[1..].trim().to_string()),
            Some('?') => (Some('?'), raw[1..].trim().to_string()),
            Some('@') => (Some('@'), raw[1..].trim().to_string()),
            _ => (None, raw.clone()),
        };
        // The sheets: every shortcut, or every organization, filtered by what follows the sign.
        if matches!(scope, Some('?' | '@')) {
            let rows = if scope == Some('?') {
                shortcut_rows()
            } else {
                self.org_rows()
            };
            return rows
                .into_iter()
                .filter(|row| query.is_empty() || score(&row.title, &query).is_some())
                .collect();
        }
        let entry_row = |group: &str, entry: &Entry, keys: Option<String>| Row {
            group: group.to_string().into(),
            icon: entry.icon,
            title: entry.title.into(),
            sub: entry.sub.map(Into::into),
            keys: keys.map(Into::into),
            target: Target::Entry(*entry),
        };
        let toggle_keys = if cfg!(target_os = "macos") { "⌘ B" } else { "Ctrl B" };
        let new_keys = crate::key_bindings::keystroke("new-session")
            .map(|keys| crate::shortcut_label::keys(&keys).join(" "))
            .unwrap_or_default();
        let keys_of = |entry: &Entry| match entry.go {
            Go::NewChat if entry.title == "New session" && !new_keys.is_empty() => Some(new_keys.clone()),
            Go::ToggleSidebar => Some(toggle_keys.to_string()),
            Go::Workbench(Command::Toggle) => Some(if cfg!(target_os = "macos") {
                "⌥ ⌘ \\".to_string()
            } else {
                "Ctrl Alt \\".to_string()
            }),
            Go::Workbench(Command::NewTerminal) => Some("Ctrl Shift `".to_string()),
            Go::Workbench(Command::Maximize) => Some("Ctrl Shift Enter".to_string()),
            _ => None,
        };

        // The sessions, chats and Code together, newest first.
        let mut sessions: Vec<(i64, Row)> = Vec::new();
        if scope.is_none() {
            for (ix, chat) in self.sessions.iter().enumerate() {
                let at = chat.at_ms.unwrap_or(0);
                sessions.push((
                    at,
                    Row {
                        group: "Sessions".into(),
                        icon: Lucide::MessageSquare,
                        title: chat.title.clone().into(),
                        sub: None,
                        keys: chat.at_ms.map(|at| code::age(at, now).into()),
                        target: Target::Chat(ix),
                    },
                ));
            }
            for project in &self.projects {
                for session in &project.sessions {
                    sessions.push((
                        session.updated_at,
                        Row {
                            group: "Sessions".into(),
                            icon: Lucide::Code,
                            title: session.title.clone().into(),
                            sub: Some(project.name.clone().into()),
                            keys: Some(code::age(session.updated_at, now).into()),
                            target: Target::Code(session.id.clone()),
                        },
                    ));
                }
            }
        }
        sessions.sort_by(|a, b| b.0.cmp(&a.0));

        let session_rows = self.session_action_rows();
        let code_rows = self.code_action_rows(cx);
        let command_rows = self.project_command_rows(cx);
        let new_in = self.new_in_rows();
        let pages: Vec<Entry> = PAGES
            .iter()
            .filter(|entry| {
                self.account_gateway().is_some() || matches!(entry.go, Go::Page("settings/preferences") | Go::NewChat)
            })
            .copied()
            .collect();
        let pages = pages.as_slice();
        let mut rows = Vec::new();
        if query.is_empty() {
            if scope != Some('/') {
                let take = if scope == Some('>') { session_rows.len() } else { 3 };
                rows.extend(session_rows.iter().take(take).cloned());
            }
            if scope == Some('/') {
                rows.extend(pages.iter().map(|entry| entry_row("Go to", entry, None)));
            } else if scope.is_none() {
                for go in ["notifications", "library", "docs"] {
                    if let Some(entry) = pages.iter().find(|entry| entry.go == Go::Page(go)) {
                        let mut row = entry_row("Go to", entry, None);
                        if go == "notifications" && self.inbox > 0 {
                            row.sub = Some(format!("{} unread", self.inbox).into());
                        }
                        rows.push(row);
                    }
                }
            }
            if scope.is_none() {
                rows.extend(sessions.into_iter().take(2).map(|(_, mut row)| {
                    row.group = "Recent".into();
                    row
                }));
            }
            if scope != Some('/') {
                // New session and Toggle sidebar; the rest (Workbench, Keyboard shortcuts...) are found by typing.
                let take = if scope == Some('>') { ACTIONS.len() } else { 2 };
                rows.extend(
                    ACTIONS
                        .iter()
                        .take(take)
                        .map(|entry| entry_row("Actions", entry, keys_of(entry))),
                );
                if scope == Some('>') {
                    rows.extend(
                        WORKBENCH
                            .iter()
                            .map(|entry| entry_row("Workbench", entry, keys_of(entry))),
                    );
                    rows.extend(command_rows);
                    rows.extend(code_rows);
                }
            }
            return rows;
        }

        // Each group sorted by its own scores; groups ordered by their best one.
        let mut groups: Vec<(f32, Vec<Row>)> = Vec::new();
        let ranked = |entries: &[Entry], group: &str| -> (f32, Vec<Row>) {
            let mut found: Vec<(f32, Row)> = entries
                .iter()
                .filter_map(|entry| {
                    let best = score(entry.title, &query)
                        .into_iter()
                        .chain(score(entry.keywords, &query).map(|value| value * 0.7))
                        .fold(None, |best: Option<f32>, value| {
                            Some(best.map_or(value, |best| best.max(value)))
                        })?;
                    Some((best, entry_row(group, entry, keys_of(entry))))
                })
                .collect();
            found.sort_by(|a, b| b.0.total_cmp(&a.0));
            (
                found.first().map_or(0., |(best, _)| *best),
                found.into_iter().take(PER_GROUP).map(|(_, row)| row).collect(),
            )
        };
        if scope != Some('>') {
            groups.push(ranked(pages, "Go to"));
            // The Nexus app ranks its settings rows from 0.8 down: under an exact page or action, above the guesses.
            let settings: Vec<Row> = settings_rows(&query, &self.settings_visibility())
                .into_iter()
                .filter(|row| {
                    self.is_local()
                        || row.title.as_ref() != "Show when I'm online"
                        || self.online_preference_available()
                })
                .collect();
            // The Assistant's settings come first in the group when they match (A24·e: "Assistant › Approvals").
            let (best, assistant) = if self.assistant_available() {
                assistant_setting_rows(&query)
            } else {
                (0., Vec::new())
            };
            groups.push((best.max(0.8), assistant.into_iter().chain(settings).collect()));
        }
        if scope != Some('/') {
            let mut found: Vec<(f32, Row)> = session_rows
                .into_iter()
                .filter_map(|row| score(&row.title, &query).map(|value| (value, row)))
                .collect();
            found.sort_by(|a, b| b.0.total_cmp(&a.0));
            groups.push((
                found.first().map_or(0., |(best, _)| *best),
                found.into_iter().map(|(_, row)| row).collect(),
            ));
            groups.push(ranked(ACTIONS, "Actions"));
            groups.push(ranked(WORKBENCH, "Workbench"));
            groups.push(scored(command_rows, &query));
            groups.push(scored(code_rows, &query));
        }
        if scope.is_none() {
            groups.push(scored(new_in, &query));
        }
        if scope.is_none() {
            let mut found: Vec<(f32, Row)> = sessions
                .into_iter()
                .filter_map(|(_, row)| score(&row.title, &query).map(|value| (value, row)))
                .collect();
            found.sort_by(|a, b| b.0.total_cmp(&a.0));
            let local: Vec<SharedString> = found.iter().map(|(_, row)| row.title.clone()).collect();
            let best = found.first().map_or(0., |(best, _)| *best);
            let mut rows: Vec<Row> = found.into_iter().take(PER_GROUP).map(|(_, row)| row).collect();
            // Gateway's results: its sessions join the local ones; the rest keep their own groups, in its order.
            let mut others: Vec<(SharedString, Vec<Row>)> = Vec::new();
            for (ix, hit) in self.palette_hits.iter().enumerate() {
                let row = hit_row(ix, hit, now);
                if hit.entity == "session" {
                    if rows.len() < PER_GROUP && !local.contains(&row.title) {
                        rows.push(row);
                    }
                } else if let Some((_, group)) = others.iter_mut().find(|(label, _)| *label == row.group) {
                    if group.len() < PER_GROUP {
                        group.push(row);
                    }
                } else {
                    others.push((row.group.clone(), vec![row]));
                }
            }
            groups.push((if rows.is_empty() { 0. } else { best.max(0.6) }, rows));
            groups.extend(others.into_iter().map(|(_, rows)| (0.55, rows)));
        }
        groups.retain(|(_, rows)| !rows.is_empty());
        groups.sort_by(|a, b| b.0.total_cmp(&a.0));
        // Hand off comes first, whatever else matches: Enter or Cmd+J sends the text to Nexus.
        let mut rows: Vec<Row> = self.hand_off_row().into_iter().collect();
        // Then New errand with the same text (A23·e), with ⇧⌘J.
        rows.extend(self.new_errand_row());
        rows.extend(groups.into_iter().flat_map(|(_, rows)| rows));
        rows
    }

    /// The typed text the Assistant would take: none in a scope (`>`, `/`, `?`, `@`), or without the Assistant here.
    pub(super) fn hand_off_text(&self) -> Option<String> {
        let text = self.palette_query.trim();
        (self.assistant_available() && !text.is_empty() && !text.starts_with(['>', '/', '?', '@']))
            .then(|| text.to_string())
    }

    /// The palette's first row over typed text (A21·e): "Hand off “dinner”", with ⌘J.
    fn hand_off_row(&self) -> Option<Row> {
        let text = self.hand_off_text()?;
        Some(Row {
            group: "Assistant".into(),
            icon: Lucide::ArrowUpRight,
            title: format!("Hand off “{text}”").into(),
            sub: None,
            keys: Some(super::sidebar::ASSISTANT_HINT.into()),
            target: Target::HandOff(text),
        })
    }

    /// The palette's second row over typed text (A23·e): "New errand “Acme SOW”", with ⇧⌘J.
    fn new_errand_row(&self) -> Option<Row> {
        let text = self.hand_off_text()?;
        Some(Row {
            group: "Assistant".into(),
            icon: Lucide::Plus,
            title: format!("New errand “{text}”").into(),
            sub: None,
            keys: Some(super::sidebar::NEW_ERRAND_HINT.into()),
            target: Target::NewErrand(text),
        })
    }

    /// The open session's menu actions (`session_menu.rs` for a Code session, `chat_menu.rs` for a chat).
    fn session_action_rows(&self) -> Vec<Row> {
        let row = |icon: Lucide, title: &str, action: SessionAction| Row {
            group: "This session".into(),
            icon,
            title: title.to_string().into(),
            sub: None,
            keys: None,
            target: Target::Session(action),
        };
        match &self.view {
            View::Code(open) => {
                let Some(session) = self.session_by_id(&open.id) else {
                    return Vec::new();
                };
                let now = code::now_ms();
                let mut rows = vec![
                    Row {
                        keys: Some(if cfg!(target_os = "macos") { "⌘ F" } else { "Ctrl F" }.into()),
                        ..row(Lucide::Search, "Find in the session", SessionAction::Find)
                    },
                    row(Lucide::Pencil, "Rename session", SessionAction::Rename),
                ];
                if super::project_files::offered(session.machine.as_deref()) {
                    for (title, key, action) in [
                        ("Open a file", "secondary-p", SessionAction::OpenFile),
                        (
                            "Search the session's files",
                            "secondary-shift-f",
                            SessionAction::SearchFiles,
                        ),
                    ] {
                        rows.push(Row {
                            keys: Some(crate::shortcut_label::label(key).into()),
                            ..row(Lucide::Search, title, action)
                        });
                    }
                }
                if self.code_running() {
                    rows.insert(
                        0,
                        Row {
                            keys: Some(if cfg!(target_os = "macos") { "⌘ ." } else { "Ctrl ." }.into()),
                            ..row(Lucide::CircleStop, "Stop the agent's turn", SessionAction::Stop)
                        },
                    );
                }
                if !session.archived {
                    rows.push(if session.pinned {
                        row(Lucide::PinOff, "Unpin session", SessionAction::Pin(false))
                    } else {
                        row(Lucide::Pin, "Pin session to the top", SessionAction::Pin(true))
                    });
                }
                rows.push(if session.archived {
                    row(
                        Lucide::ArchiveRestore,
                        "Restore session from the archive",
                        SessionAction::Archive(false),
                    )
                } else {
                    row(Lucide::Archive, "Archive session", SessionAction::Archive(true))
                });
                if session.snoozed_until.is_some_and(|until| until > now) {
                    rows.push(row(Lucide::AlarmClockOff, "Unsnooze session", SessionAction::Unsnooze));
                }
                if !session.archived {
                    let local = chrono::Local::now();
                    for (label, at) in crate::snooze::presets(local) {
                        rows.push(Row {
                            sub: Some(crate::snooze::wake_text(at, local).into()),
                            ..row(Lucide::AlarmClock, &snooze_title(label), SessionAction::Snooze(at))
                        });
                    }
                }
                if session.settled && !session.archived {
                    rows.push(Row {
                        sub: Some("Back from Settled; a merge won't settle it again".into()),
                        ..row(Lucide::ListPlus, "Keep in list", SessionAction::KeepInList)
                    });
                }
                if let Some(branch) = session.branch.clone() {
                    rows.push(row(
                        Lucide::GitBranch,
                        "Copy branch name",
                        SessionAction::CopyBranch(branch),
                    ));
                }
                rows.push(row(Lucide::Trash, "Delete session…", SessionAction::Delete));
                rows
            }
            // "This chat": Share, Find and Copy first, the three the default list shows.
            View::Chat if self.current.is_some() => {
                let chat_row = |icon: Lucide, title: &str, action: SessionAction, keys: Option<String>| Row {
                    group: "This chat".into(),
                    keys: keys.map(|keys| crate::shortcut_label::keys(&keys).join(" ").into()),
                    ..row(icon, title, action)
                };
                let mut rows = Vec::new();
                if !self.entries.is_empty() {
                    if self.asking.is_none() {
                        rows.push(chat_row(
                            Lucide::Share,
                            "Share chat",
                            SessionAction::Share,
                            Some(super::chat_menu::SHARE_KEYS.into()),
                        ));
                    }
                    rows.push(chat_row(
                        Lucide::Search,
                        "Find in this chat",
                        SessionAction::Find,
                        crate::key_bindings::keystroke("find-in-session"),
                    ));
                    rows.push(chat_row(
                        Lucide::Copy,
                        "Copy as Markdown",
                        SessionAction::CopyMarkdown,
                        None,
                    ));
                    rows.push(chat_row(
                        Lucide::Download,
                        "Download chat as .md",
                        SessionAction::Download,
                        None,
                    ));
                }
                let row = |icon: Lucide, title: &str, action: SessionAction| chat_row(icon, title, action, None);
                rows.push(row(Lucide::Pencil, "Rename chat", SessionAction::Rename));
                rows.push(if self.chat_archived {
                    row(
                        Lucide::ArchiveRestore,
                        "Restore chat from the archive",
                        SessionAction::Archive(false),
                    )
                } else {
                    row(Lucide::Archive, "Archive chat", SessionAction::Archive(true))
                });
                rows.push(row(Lucide::Trash, "Delete chat…", SessionAction::Delete));
                rows
            }
            _ => Vec::new(),
        }
    }

    /// The Code actions that are not the open session's: Pin project (the open session's, or the one New session
    /// picked), Add a project, and the slash commands' controls, as the Nexus app's palette has them.
    fn code_action_rows(&self, cx: &App) -> Vec<Row> {
        let code = matches!(self.view, View::Code(_) | View::NewCode);
        if !code {
            return Vec::new();
        }
        let row = |icon: Lucide, title: String, sub: Option<SharedString>, target: Target| Row {
            group: "Actions".into(),
            icon,
            title: title.into(),
            sub,
            keys: None,
            target,
        };
        let mut rows = Vec::new();
        if matches!(self.view, View::NewCode) {
            rows.push(row(
                Lucide::Link2,
                "Copy prompt link".into(),
                Some("Opens this draft in Nexus for desktop".into()),
                Target::CopyPromptLink,
            ));
        }
        let project_id = match &self.view {
            View::Code(open) => self.session_by_id(&open.id).map(|session| session.project_id.clone()),
            _ => self.code_form.project.clone(),
        };
        // A server's project is pinned there, as the sidebar's menu does it; this computer's needs the runner.
        if let Some(project) = project_id
            .and_then(|id| self.projects.iter().find(|project| project.id == id))
            .filter(|project| project.machine.is_some() || self.runner.is_some())
        {
            let (icon, title) = if project.pinned {
                (Lucide::PinOff, "Unpin project")
            } else {
                (Lucide::Pin, "Pin project")
            };
            rows.push(row(
                icon,
                title.into(),
                Some(project.name.clone().into()),
                Target::PinProject(project.id.clone(), !project.pinned),
            ));
        }
        rows.push(row(
            Lucide::FolderPlus,
            "Add a project".into(),
            None,
            Target::AddProject,
        ));
        // A session on this computer edits its project's commands from here (the Nexus app's "Edit project commands").
        if let View::Code(open) = &self.view
            && let Some(session) = self.session_by_id(&open.id)
            && Self::commands_unavailable(&open.id).is_none()
        {
            let title = if crate::project_commands::of(&session.project_id, cx).is_empty() {
                "Add a project command"
            } else {
                "Edit project commands"
            };
            rows.push(row(
                Lucide::Play,
                title.into(),
                Some("Commands like npm run dev, each with a button and a key".into()),
                Target::EditCommands,
            ));
        }
        for (action, name, hint) in slash::palette_commands() {
            rows.push(row(
                Lucide::ArrowUpDown,
                format!("/{name}"),
                Some(hint.into()),
                Target::Slash(action),
            ));
        }
        rows
    }

    /// The open Code session's project commands as "Run <name>" rows, with the command's first line and its key, while
    /// the session can run them (the Nexus app's palette group "Project commands").
    fn project_command_rows(&self, cx: &App) -> Vec<Row> {
        let View::Code(open) = &self.view else {
            return Vec::new();
        };
        let Some(session) = self.session_by_id(&open.id) else {
            return Vec::new();
        };
        if Self::commands_unavailable(&open.id).is_some() {
            return Vec::new();
        }
        let mac = cfg!(target_os = "macos");
        crate::project_commands::of(&session.project_id, cx)
            .into_iter()
            .map(|script| Row {
                group: "Project commands".into(),
                icon: Lucide::Play,
                title: format!("Run {}", script.name).into(),
                sub: Some(scripts::first_line(&script.command).to_string().into()),
                keys: crate::project_commands::key_caps(script.shortcut.as_deref(), mac).map(Into::into),
                target: Target::RunCommand(open.id.clone(), script),
            })
            .collect()
    }

    /// "New session in <project>" for each project, pinned first (the Nexus app's `pickerProjects`).
    fn new_in_rows(&self) -> Vec<Row> {
        let (pinned, rest): (Vec<_>, Vec<_>) = self.projects.iter().partition(|project| project.pinned);
        pinned
            .into_iter()
            .chain(rest)
            .map(|project| Row {
                group: "New session".into(),
                icon: Lucide::SquarePen,
                title: format!("New session in {}", project.name).into(),
                sub: None,
                keys: None,
                target: Target::NewIn(project.id.clone()),
            })
            .collect()
    }

    /// The organizations the `@` scope lists, as read for it (`org_menu.rs`).
    fn palette_orgs(&self) -> Option<&OrgMenu> {
        self.overlay.palette().and_then(Option::as_ref)
    }

    /// The organizations the switcher lists (`org_menu.rs`), once they are loaded.
    fn org_rows(&self) -> Vec<Row> {
        let Some(OrgMenu::Ready(orgs)) = self.palette_orgs() else {
            return Vec::new();
        };
        orgs.iter()
            .map(|org| Row {
                group: "Organizations".into(),
                icon: if org.personal { Lucide::User } else { Lucide::Users },
                title: org.name.clone().into(),
                sub: Some(match (org.personal, org.current) {
                    (_, true) => "Current".into(),
                    (true, false) => "Personal".into(),
                    _ => "Switch to this organization".into(),
                }),
                keys: None,
                target: Target::Org(org.clone()),
            })
            .collect()
    }

    /// Run one of the open session's actions: the same functions its menu calls.
    fn run_session_action(&mut self, action: SessionAction, window: &mut Window, cx: &mut Context<Self>) {
        match (&self.view, action) {
            (View::Code(open), action) => {
                let id = open.id.clone();
                let args = |args: UpdateArgs| UpdateArgs {
                    thread_id: id.clone(),
                    ..args
                };
                match action {
                    SessionAction::Rename => self.start_rename(&id, true, window, cx),
                    SessionAction::Find => self.open_code_find(window, cx),
                    SessionAction::OpenFile => self.open_file_picker(super::file_picker::Mode::Files, window, cx),
                    SessionAction::SearchFiles => self.open_file_picker(super::file_picker::Mode::Search, window, cx),
                    SessionAction::Pin(pinned) => self.change_session(
                        args(UpdateArgs {
                            pinned: Some(pinned),
                            ..UpdateArgs::default()
                        }),
                        cx,
                    ),
                    SessionAction::Archive(archived) => self.change_session(
                        args(UpdateArgs {
                            archived: Some(archived),
                            ..UpdateArgs::default()
                        }),
                        cx,
                    ),
                    SessionAction::Unsnooze => self.change_session(
                        args(UpdateArgs {
                            snoozed_until: Some(0),
                            ..UpdateArgs::default()
                        }),
                        cx,
                    ),
                    SessionAction::CopyBranch(branch) => cx.write_to_clipboard(ClipboardItem::new_string(branch)),
                    SessionAction::Stop => self.stop(window, cx),
                    SessionAction::Snooze(at) => self.change_session(
                        args(UpdateArgs {
                            snoozed_until: Some(at),
                            ..UpdateArgs::default()
                        }),
                        cx,
                    ),
                    SessionAction::KeepInList => self.change_session(
                        args(UpdateArgs {
                            keep_listed: Some(true),
                            ..UpdateArgs::default()
                        }),
                        cx,
                    ),
                    // Deleting asks first, in the top bar's menu, as everywhere else.
                    SessionAction::Delete => {
                        self.remember_session_anchor(&id, true, cx);
                        self.open_overlay(Overlay::SessionMenu(SessionMenu {
                            id,
                            page: MenuPage::Delete,
                            in_title: true,
                        }));
                    }
                    SessionAction::CopyMarkdown | SessionAction::Download | SessionAction::Share => {}
                }
            }
            (View::Chat, action) => {
                let Some(id) = self.current.clone() else { return };
                match action {
                    SessionAction::Rename => self.start_chat_rename(window, cx),
                    SessionAction::Archive(archived) => self.set_chat_archived(id, archived, cx),
                    SessionAction::CopyMarkdown => self.export_chat(false, cx),
                    SessionAction::Download => self.export_chat(true, cx),
                    SessionAction::Delete => self.open_overlay(Overlay::ChatMenu(ChatMenu::Delete)),
                    SessionAction::Share => self.open_chat_share(window, cx),
                    SessionAction::Find => self.open_find(window, cx),
                    SessionAction::Pin(_)
                    | SessionAction::Unsnooze
                    | SessionAction::CopyBranch(_)
                    | SessionAction::OpenFile
                    | SessionAction::SearchFiles
                    | SessionAction::Stop
                    | SessionAction::Snooze(_)
                    | SessionAction::KeepInList => {}
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// Stay open and move to a scope: `?` (shortcuts) or `@` (organizations).
    fn palette_scope(&mut self, sign: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_input
            .update(cx, |state, cx| state.set_value(sign, window, cx));
        self.palette_typed(sign.to_string(), cx);
    }

    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_overlay(Overlay::Palette(None));
        self.palette_cursor = 0;
        self.palette_query.clear();
        self.palette_hits.clear();
        self.palette_input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.close_palette();
        self.palette_searching = false;
        self._palette_debounce = None;
        self.focus_page(window, cx);
        cx.notify();
    }

    /// Hand the keyboard to the page now showing. The palette's own field is gone with it, and a page without
    /// a field (Usage, Inbox, Settings) would leave focus on nothing, so Ctrl+K would stay dead until a click.
    pub(super) fn focus_page(&self, window: &mut Window, cx: &mut Context<Self>) {
        match focus_for(&self.view) {
            PageFocus::Composer => self.focus_composer(window, cx),
            PageFocus::Workspace => window.focus(&self.focus, cx),
            PageFocus::Own => match self.view {
                View::Library => self.library_search.update(cx, |state, cx| state.focus(window, cx)),
                View::Models => self.models_page.search.update(cx, |state, cx| state.focus(window, cx)),
                _ => {}
            },
        }
    }

    /// The query changed: match here at once, ask Gateway once typing pauses.
    pub(super) fn palette_typed(&mut self, query: String, cx: &mut Context<Self>) {
        self.palette_query = query;
        self.palette_cursor = 0;
        self.palette_seq += 1;
        let seq = self.palette_seq;
        let query = self.palette_query.trim().to_string();
        // The organizations are read when their scope opens, and dropped when it closes.
        if query.starts_with('@') && self.account_gateway().is_some() {
            if let Some(orgs) = self.overlay.palette_mut()
                && orgs.is_none()
            {
                *orgs = Some(OrgMenu::Loading);
                self.load_orgs(cx);
            }
        } else if let Some(orgs) = self.overlay.palette_mut() {
            *orgs = None;
        }
        if self.account_gateway().is_none() || query.is_empty() || query.starts_with(['>', '/', '?', '@']) {
            self.palette_hits.clear();
            self.palette_searching = false;
            self._palette_debounce = None;
        } else {
            self.palette_searching = true;
            self._palette_debounce = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(DEBOUNCE).await;
                let _ = this.update(cx, |this, _| {
                    if this.palette_seq == seq
                        && let Some(account) = this.account_gateway()
                    {
                        account.search(seq, query);
                    }
                });
            }));
        }
        cx.notify();
    }

    fn copy_prompt_link(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.view, View::NewCode) {
            return;
        }
        let text = self.composer.read(cx).value();
        if text.trim().is_empty() {
            self.composer_note = Some("Write the prompt first: the link carries it.".into());
            cx.notify();
            return;
        }
        let remotes = self
            .code_form
            .project
            .as_ref()
            .and_then(|id| self.projects.iter().find(|project| &project.id == id))
            .map(|project| project.remotes.as_slice())
            .unwrap_or_default();
        let written = crate::links::write(
            &text,
            remotes.iter().map(String::as_str),
            self.code_form.agent,
            self.code_form.model.as_deref(),
        );
        cx.write_to_clipboard(ClipboardItem::new_string(written.link));
        self.composer_note = Some(if written.cut {
            format!(
                "Prompt link copied. It keeps the prompt's first {} characters.",
                crate::links::MAX_PROMPT
            )
            .into()
        } else {
            "Prompt link copied".into()
        });
        cx.notify();
    }

    /// Open a row and close the palette.
    pub(super) fn palette_open_row(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        match &target {
            Target::Entry(entry) if entry.go == Go::Shortcuts => return self.palette_scope("?", window, cx),
            Target::Entry(entry) if entry.go == Go::Orgs => return self.palette_scope("@", window, cx),
            Target::Info => return,
            _ => {}
        }
        self.close_palette(window, cx);
        let before = window.focused(cx);
        self.go_to_target(target, window, cx);
        // The page opened above is the one that takes the keys, not whatever was showing when the palette closed.
        // A target that focused a field of its own (Rename, a terminal) keeps it.
        if window.focused(cx) == before {
            self.focus_page(window, cx);
        }
    }

    fn go_to_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            Target::Info => {}
            Target::Session(action) => self.run_session_action(action, window, cx),
            Target::NewIn(project) => self.open_new_code_in(project, window, cx),
            Target::AddProject => self.open_project_dialog(None, window, cx),
            Target::PinProject(project, pinned) => self.update_project(project, Some(pinned), None, cx),
            Target::Slash(action) => self.run_slash_action(action, window, cx),
            Target::RunCommand(id, script) => self.run_project_command(&id, &script, window, cx),
            Target::EditCommands => self.edit_project_commands(window, cx),
            Target::CopyPromptLink => self.copy_prompt_link(cx),
            Target::Org(org) => self.switch_org(&org, window, cx),
            Target::Setting(section) => self.open_settings(section, window, cx),
            Target::SettingField(section, anchor) => self.open_settings_field(section, anchor, window, cx),
            Target::SettingOnWeb(path) => self.open_in_browser(&path, cx),
            Target::HandOff(text) => self.hand_off(text, window, cx),
            Target::NewErrand(text) => self.new_errand(text, window, cx),
            Target::AssistantView(id) => self.open_assistant(id, None, window, cx),
            Target::Entry(entry) => match entry.go {
                Go::NewChat => self.start_new_session(window, cx),
                Go::ToggleSidebar => self.toggle_sidebar(cx),
                Go::Workbench(command) => self.run_workbench(command, window, cx),
                Go::Shortcuts | Go::Orgs => {}
                Go::Page("models") => self.open_models(window, cx),
                Go::Page("settings/preferences") => self.open_settings(SettingsSection::Appearance, window, cx),
                Go::Page("settings/api-keys") => self.open_settings(SettingsSection::ApiKeys, window, cx),
                Go::Page("library") => self.open_library(window, cx),
                Go::Page("notifications") => self.open_inbox(window, cx),
                Go::Page("docs") => self.open_docs(String::new(), cx),
                Go::Page("issues") => self.open_issues(cx),
                Go::Page(path) => {
                    // A page drawn here opens here: Usage's tabs open Usage.
                    let page = path.split('/').next().filter(|first| *first == "usage").unwrap_or(path);
                    match crate::pages::PageId::from_web_path(page) {
                        Some(id) => self.open_native(id, window, cx),
                        None => self.open_in_browser(path, cx),
                    }
                }
            },
            Target::Chat(ix) => {
                if let Some(chat) = self.sessions.get(ix).cloned() {
                    self.open(&chat, window, cx);
                }
            }
            Target::Code(id) => self.open_code(&id, window, cx),
            Target::Hit(ix) => {
                let Some(hit) = self.palette_hits.get(ix).cloned() else {
                    return;
                };
                if hit.entity == "session"
                    && let Some(chat) = self.sessions.iter().find(|chat| chat.id == hit.id).cloned()
                {
                    self.open(&chat, window, cx);
                } else if hit.entity == "session" {
                    let chat = crate::model::SessionSummary {
                        id: hit.id.clone(),
                        title: hit.title.clone(),
                        day: None,
                        at_ms: None,
                    };
                    self.open(&chat, window, cx);
                } else if hit.entity == "artifact" {
                    let Some(account) = self.account_gateway() else { return };
                    // In the Workbench's Artifacts tab, as a Code session opens it.
                    self.page_opening_auto = false;
                    // The engine starts while Gateway answers with the page's address.
                    crate::web::warm();
                    account.frame_artifact(hit.id.clone(), hit.url.clone());
                } else if hit.entity == "model" {
                    self.open_models(window, cx);
                } else {
                    // People and teams: their pages are org admin, still in the browser.
                    crate::external_link::open(&format!("{}{}", self.gateway.site_url(), hit.url), cx);
                }
            }
        }
    }

    /// Up, Down, Tab, Enter and Esc while the palette is open; whether the key was the palette's.
    pub(super) fn palette_key(&mut self, key: &str, shift: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let rows = self.palette_rows(cx);
        let count = rows.len();
        match key {
            "escape" => self.close_palette(window, cx),
            "up" if count > 0 => self.palette_cursor = (self.palette_cursor + count - 1) % count,
            "down" if count > 0 => self.palette_cursor = (self.palette_cursor + 1) % count,
            "tab" if count > 0 => {
                // The first row of the next (or previous) group.
                let starts: Vec<usize> = (0..count)
                    .filter(|&ix| ix == 0 || rows[ix].group != rows[ix - 1].group)
                    .collect();
                let at = starts
                    .iter()
                    .rposition(|&start| start <= self.palette_cursor)
                    .unwrap_or(0);
                let next = if shift {
                    (at + starts.len() - 1) % starts.len()
                } else {
                    (at + 1) % starts.len()
                };
                self.palette_cursor = starts[next];
            }
            "enter" => {
                if let Some(row) = rows.into_iter().nth(self.palette_cursor) {
                    self.palette_open_row(row.target, window, cx);
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn render_palette(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.palette_rows(cx);
        let query = self.palette_query.trim().to_string();
        let scope = query.chars().next().filter(|c| matches!(c, '?' | '@'));
        let mut list = v_flex().px(px(8.)).pb(px(8.));
        let mut row_elements = Vec::new();
        for (ix, row) in rows.iter().enumerate() {
            if ix == 0 || rows[ix - 1].group != row.group {
                row_elements.push(
                    div()
                        .pt(px(12.))
                        .px(px(10.))
                        .pb(px(4.))
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(palette.faint)
                        .child(row.group.clone())
                        .into_any_element(),
                );
            }
            // A shortcut row only tells; it is never "the one Enter opens".
            let selected = ix == self.palette_cursor && !matches!(row.target, Target::Info);
            let target = row.target.clone();
            let web = matches!(&row.target, Target::Entry(entry) if opens_web(entry.go));
            let hand_off = matches!(row.target, Target::HandOff(_));
            row_elements.push(
                h_flex()
                    .id(("palette-row", ix))
                    .min_h(px(36.))
                    .px(px(10.))
                    .py(px(6.))
                    .gap(px(12.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .when(!matches!(row.target, Target::Info), |item| item.cursor_pointer())
                    .when(selected, |item| item.bg(palette.menu_hover))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.palette_cursor != ix {
                            this.palette_cursor = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.palette_open_row(target.clone(), window, cx)))
                    .child(if hand_off {
                        // Hand off is Nexus's: its mark, as the sidebar's Assistant row.
                        svg()
                            .path(crate::assets::APP_MARK)
                            .size(px(16.))
                            .flex_none()
                            .text_color(if selected { palette.foreground } else { palette.icon })
                            .into_any_element()
                    } else {
                        Icon::new(row.icon)
                            .size(px(16.))
                            .text_color(if selected { palette.foreground } else { palette.icon })
                            .into_any_element()
                    })
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(8.))
                            .items_baseline()
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(relative(0.7))
                                    .truncate()
                                    .child(row.title.clone()),
                            )
                            .when_some(row.sub.clone(), |text, sub| {
                                text.child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .truncate()
                                        .text_size(px(12.))
                                        .text_color(palette.faint)
                                        .child(sub),
                                )
                            }),
                    )
                    // Keys and ages as plain faint text; rows that open the browser end with ↗. Hand off
                    // shows its keys as caps, as its board does (A21·e).
                    .when_some(row.keys.clone(), |item, keys| {
                        if hand_off {
                            item.child(keycaps(&keys, selected, palette))
                        } else {
                            item.child(key_text(keys, palette))
                        }
                    })
                    .when(web, |item| {
                        item.child(Icon::new(Lucide::ArrowUpRight).size(px(14.)).text_color(palette.faint))
                    })
                    .into_any_element(),
            );
        }
        list = list.children(row_elements);
        if rows.is_empty() {
            list = list.child(
                v_flex()
                    .min_h(px(48.))
                    .justify_center()
                    .px(px(12.))
                    .items_center()
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .when(self.palette_searching, |empty| empty.child("Searching…"))
                    .when(
                        scope == Some('@') && matches!(self.palette_orgs(), Some(OrgMenu::Loading) | None),
                        |empty| empty.child("Loading your organizations…"),
                    )
                    .when(
                        scope == Some('@') && matches!(self.palette_orgs(), Some(OrgMenu::Failed(_))),
                        |empty| empty.child("Couldn't load your organizations."),
                    )
                    .when(
                        !self.palette_searching && scope != Some('@')
                            || scope == Some('@') && matches!(self.palette_orgs(), Some(OrgMenu::Ready(_))),
                        |empty| empty.child(format!("Nothing matches \u{201c}{query}\u{201d}.")),
                    ),
            );
        }
        if self.palette_failed {
            list = list.child(
                div()
                    .px(px(10.))
                    .pt(px(8.))
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child("Search is unavailable; showing what's already loaded."),
            );
        }
        let dialog = v_flex()
            .id("palette")
            .occlude()
            .w(px(WIDTH))
            .max_w(relative(0.94))
            .max_h(px(640.))
            .rounded(px(12.))
            .floating_ground(12., palette.menu, true, &palette)
            .border_1()
            .border_color(if palette.dark {
                palette.border
            } else {
                palette.border.opacity(0.8)
            })
            .shadow(palette.pop_shadow(palette.border))
            .overflow_hidden()
            .text_color(palette.foreground)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .h(px(52.))
                    .px(px(20.))
                    .gap(px(12.))
                    .child(Icon::new(Lucide::Search).size(px(16.)).text_color(palette.icon))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(14.))
                            .child(Input::new(&self.palette_input).appearance(false)),
                    )
                    .when(self.palette_searching, |head| {
                        head.child(
                            Icon::new(Lucide::LoaderCircle)
                                .size(px(16.))
                                .text_color(palette.muted)
                                .with_animation(
                                    "palette-spinner",
                                    Animation::new(Duration::from_millis(800))
                                        .repeat()
                                        .with_max_fps(crate::motion::LOOP_FPS),
                                    |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .id("palette-list")
                    .flex_1()
                    .min_h(px(0.))
                    .max_h(px(520.))
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                // The list runs under the footer; this keeps its last rows from being cut by a hard edge.
                div().flex_none().h(px(16.)).mt(px(-16.)).bg(linear_gradient(
                    180.,
                    linear_color_stop(palette.menu.opacity(0.), 0.),
                    linear_color_stop(palette.menu, 1.),
                )),
            )
            .child(
                // Enter Open, Esc Close, then "? Shortcuts and filters": the sheet lists > / @ and the keys.
                h_flex()
                    .h(px(40.))
                    .flex_none()
                    .px(px(16.))
                    .gap(px(16.))
                    .bg(palette.sidebar)
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .when(scope != Some('?'), |bar| bar.child(hint("Enter", "Open", palette)))
                    .child(hint("Esc", "Close", palette))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("palette-shortcuts")
                            .cursor_pointer()
                            .hover(|style| style.text_color(palette.foreground))
                            .on_click(cx.listener(|this, _, window, cx| this.palette_scope("?", window, cx)))
                            .child(hint("?", "Shortcuts and filters", palette)),
                    ),
            );
        div()
            .id("palette-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(palette.scrim)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
            )
            .child(h_flex().w_full().justify_center().pt(px(TOP)).child(
                // Opens over 160ms from 98% and clear.
                dialog.with_animation(
                    "palette-in",
                    Animation::new(motion::UI).with_easing(motion::ease_out),
                    |dialog, delta| dialog.opacity(delta).mt(px(-6. * (1. - delta))),
                ),
            ))
            .with_animation(
                "palette-scrim-in",
                Animation::new(motion::UI).with_easing(motion::ease_out),
                |overlay, delta| overlay.opacity(delta),
            )
    }
}

/// The shortcuts the app really binds (`key_bindings` in `main.rs`, with the keys the person set in Settings ›
/// Keyboard shortcuts), plus the palette's own keys. A shortcut the person cleared is left out.
fn shortcut_rows() -> Vec<Row> {
    let mac = cfg!(target_os = "macos");
    let label = |action: &str| match action.rsplit("::").next().unwrap_or(action) {
        "Quit" => Some("Quit"),
        "NewChat" => Some("New session"),
        "ToggleSidebar" => Some("Toggle sidebar"),
        "OpenAssistant" => Some("Open the Assistant"),
        "NewErrand" => Some("New errand"),
        "FindChat" => Some("Search"),
        "FindInChat" => Some("Find in the session"),
        "StashPrompt" => Some("Stash the draft (empty message box: bring a stashed one back)"),
        "ToggleSidePane" => Some("Toggle workbench"),
        "ToggleTerminal" => Some("Toggle terminal"),
        "NewTerminal" => Some("New terminal"),
        "ToggleMaximizeWorkbench" => Some("Expand workbench"),
        "SideTool1" => Some("Workbench: Changes"),
        "SideTool2" => Some("Workbench: Files"),
        "SideTool3" => Some("Workbench: Browser"),
        "SideTool4" => Some("Workbench: Terminal"),
        "SideTool5" => Some("Workbench: Artifacts"),
        "SideTool6" => Some("Workbench: Background tasks"),
        // Code's session keys (`session_keys.rs`), with the Nexus app's labels. Mod+1 to Mod+9 fold into one row.
        "JumpSession1" => Some("Jump to the 1st to 9th session in the sidebar"),
        "AddProject" => Some("Add a project (a folder)"),
        "PreviousSession" => Some("Previous session"),
        "NextSession" => Some("Next session"),
        "FocusComposer" => Some("Focus the message box"),
        "ToggleChanges" => Some("Show or hide Changes"),
        "PickModel" => Some("Pick the model"),
        "OpenInbox" => Some("Open the Inbox"),
        "OpenArtifacts" => Some("Open Artifacts"),
        "StopTurn" => Some("Stop the agent's turn"),
        "CycleModel" => Some("Next model"),
        "CycleEffort" => Some("Next effort level"),
        "CycleMode" => Some("Next permission mode (Full access only from its menu)"),
        "ArchiveSession" => Some("Archive or restore the session"),
        "SnoozeSession" => Some("Snooze the session (a snoozed one wakes)"),
        "CopyBranch" => Some("Copy the branch name"),
        "RenameSession" => Some("Rename the session"),
        "DeleteSession" => Some("Delete the session"),
        _ => None,
    };
    let keys = |binding: &gpui_kit::KeyBinding| {
        binding
            .keystrokes()
            .iter()
            .map(|stroke| {
                let stroke = stroke.inner();
                let m = &stroke.modifiers;
                let mut parts: Vec<String> = Vec::new();
                if m.control {
                    parts.push(if mac { "⌃" } else { "Ctrl" }.into());
                }
                if m.alt {
                    parts.push(if mac { "⌥" } else { "Alt" }.into());
                }
                if m.shift {
                    parts.push(if mac { "⇧" } else { "Shift" }.into());
                }
                if m.platform {
                    parts.push(if mac { "⌘" } else { "Super" }.into());
                }
                parts.push(match stroke.key.as_str() {
                    "enter" => "Enter".into(),
                    "backspace" => if mac { "⌫" } else { "Backspace" }.into(),
                    key => key.to_uppercase(),
                });
                parts.join(" ")
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let icon = |title: &str| match title {
        "Quit" => Lucide::LogOut,
        "New session" => Lucide::SquarePen,
        "Toggle sidebar" => Lucide::PanelLeft,
        "Search" => Lucide::Search,
        "Stash the draft (empty message box: bring a stashed one back)" => Lucide::Bookmark,
        "Toggle workbench" => Lucide::PanelRight,
        "Expand workbench" => Lucide::Maximize2,
        "Workbench: Changes" => Lucide::FileDiff,
        "Workbench: Files" => Lucide::Files,
        "Workbench: Browser" => Lucide::Globe,
        "Workbench: Artifacts" => Lucide::FileText,
        "Previous session" | "Next session" | "Jump to the 1st to 9th session in the sidebar" => Lucide::ArrowUpDown,
        "Add a project (a folder)" => Lucide::FolderPlus,
        "Open the Inbox" => Lucide::Inbox,
        "Open Artifacts" => Lucide::Share2,
        "Stop the agent's turn" => Lucide::CircleStop,
        "Archive or restore the session" => Lucide::Archive,
        "Snooze the session (a snoozed one wakes)" => Lucide::AlarmClock,
        "Copy the branch name" => Lucide::GitBranch,
        "Rename the session" => Lucide::Pencil,
        "Delete the session" => Lucide::Trash,
        "Move" => Lucide::ArrowUpDown,
        "Next group" => Lucide::ChevronsRight,
        "Open" => Lucide::CornerDownLeft,
        "Close" => Lucide::X,
        _ => Lucide::SquareTerminal,
    };
    let mut rows: Vec<Row> = Vec::new();
    let mut add = |group: &str, title: &str, keys: String| {
        rows.push(Row {
            group: group.to_string().into(),
            icon: icon(title),
            title: title.to_string().into(),
            sub: None,
            keys: Some(keys.into()),
            target: Target::Info,
        });
    };
    let mut seen: Vec<&'static str> = Vec::new();
    let mut found: Vec<(&'static str, String)> = Vec::new();
    for binding in crate::key_bindings() {
        let Some(title) = label(binding.action().name()) else {
            continue;
        };
        // A text box's copy of a key (`Workspace > Input`) is the same row.
        let keys = if title.starts_with("Jump to the 1st") {
            if mac { "⌘ 1–9" } else { "Ctrl 1–9" }.to_string()
        } else {
            keys(&binding)
        };
        // A second binding for the same action (Ctrl Shift ` and Ctrl ~) is one row.
        if !seen.contains(&title) {
            seen.push(title);
            found.push((title, keys));
        }
    }
    // Most used first; Quit last, so nothing destructive sits at the top of the sheet.
    found.sort_by_key(|(title, _)| *title == "Quit");
    for (title, keys) in found {
        add("Keyboard shortcuts", title, keys);
    }
    add(
        "In a Code session",
        "Find a file",
        crate::shortcut_label::label("secondary-p"),
    );
    add(
        "In a Code session",
        "Search project files",
        crate::shortcut_label::label("secondary-shift-f"),
    );
    add("In the palette", "Move", "↑ ↓".into());
    add("In the palette", "Next group", "Tab".into());
    add("In the palette", "Open", "Enter".into());
    add("In the palette", "Close", "Esc".into());
    rows
}

/// The Assistant's own settings, each a native view (A24·e: ⌘K matches settings too, "Assistant › Approvals").
/// Proactivity is a panel of the Approvals view and Projects are the Errands view's, so each opens that view.
const ASSISTANT_SETTINGS: &[(&str, &str, PageId)] = &[
    (
        "Approvals",
        "assistant nexus asks first ask before always allow spend limit rules revoke autonomy",
        PageId::AgentApprovals,
    ),
    (
        "Proactivity",
        "assistant nexus how often writes first quiet hours do not disturb dial",
        PageId::AgentApprovals,
    ),
    (
        "Privacy",
        "assistant nexus pause resume stop revoke all access disconnect what it can see trust",
        PageId::AgentPrivacy,
    ),
    (
        "Passwords and payments",
        "assistant nexus vault passwords cards payment logins autofill",
        PageId::AgentVault,
    ),
    (
        "Memory",
        "assistant nexus what nexus knows notes remember forget learning about me",
        PageId::AgentMemory,
    ),
    (
        "Discover",
        "assistant nexus suggestions ideas things to try skills",
        PageId::AgentDiscover,
    ),
    (
        "Errands",
        "assistant nexus errands tasks needs you running done",
        PageId::AgentErrands,
    ),
    (
        "Projects",
        "assistant nexus projects errands filed group",
        PageId::AgentErrands,
    ),
];

/// The Assistant's settings the query names, as Settings rows, with the best score. Nothing for an empty query.
fn assistant_setting_rows(query: &str) -> (f32, Vec<Row>) {
    if query.is_empty() {
        return (0., Vec::new());
    }
    let mut found: Vec<(f32, Row)> = ASSISTANT_SETTINGS
        .iter()
        .filter_map(|(name, keywords, view)| {
            let title = format!("Assistant › {name}");
            let best = score(name, query)
                .into_iter()
                .chain(score(&title, query))
                .chain(score(keywords, query).map(|value| value * 0.7))
                .fold(None, |best: Option<f32>, value| {
                    Some(best.map_or(value, |best| best.max(value)))
                })?;
            Some((
                // With the settings' rows (0.8 and under), above them for a name that matches.
                best.min(0.85),
                Row {
                    group: "Settings".into(),
                    icon: Lucide::Settings,
                    title: title.into(),
                    sub: None,
                    keys: None,
                    target: Target::AssistantView(*view),
                },
            ))
        })
        .collect();
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    (
        found.first().map_or(0., |(best, _)| *best),
        found.into_iter().map(|(_, row)| row).collect(),
    )
}

/// Every setting the query names, best first, as rows (`settingsItems`): the setting's name over its group and page.
/// Only what this account may open is listed (`Visibility`).
fn settings_rows(query: &str, visibility: &super::settings_registry::Visibility) -> Vec<Row> {
    super::settings_search::search(query, visibility)
        .into_iter()
        .take(PER_GROUP)
        .map(|hit| {
            let locked = visibility.signed_out && hit.requires_sign_in();
            let target = match hit.section {
                Some(section) => hit
                    .anchor
                    .map_or(Target::Setting(section), |anchor| Target::SettingField(section, anchor)),
                None if locked => {
                    let section = super::SettingsSection::GatewayLink(hit.dest);
                    hit.anchor
                        .map_or(Target::Setting(section), |anchor| Target::SettingField(section, anchor))
                }
                None => Target::SettingOnWeb(format!(
                    "{}{}",
                    hit.dest.web_path(),
                    hit.anchor.map(|anchor| format!("#{anchor}")).unwrap_or_default()
                )),
            };
            Row {
                group: "Settings".into(),
                icon: if locked {
                    Lucide::Lock
                } else if hit.section.is_some() {
                    Lucide::Settings
                } else {
                    Lucide::ExternalLink
                },
                title: hit.label.into(),
                sub: if locked {
                    Some(crate::pages::ui::REQUIRES_SIGN_IN.into())
                } else {
                    (hit.breadcrumb.len() > 1).then(|| hit.breadcrumb[..hit.breadcrumb.len() - 1].join(" › ").into())
                },
                keys: None,
                target,
            }
        })
        .collect()
}

fn hit_row(ix: usize, hit: &SearchHit, _now: i64) -> Row {
    let icon = match hit.entity.as_str() {
        "session" => Lucide::MessageSquare,
        "artifact" => Lucide::Share2,
        "member" | "invitation" => Lucide::Users,
        "team" => Lucide::UsersRound,
        "model" | "policy" => Lucide::Cpu,
        _ => Lucide::ArrowUpRight,
    };
    Row {
        group: if hit.entity == "session" {
            "Sessions".into()
        } else {
            hit.group.clone().into()
        },
        icon,
        title: hit.title.clone().into(),
        sub: hit.subtitle.clone().map(Into::into),
        keys: None,
        target: Target::Hit(ix),
    }
}

/// A row's keys, one filled cap per key (`.nx-palette-keycaps`).
pub(super) fn keycaps(keys: &str, selected: bool, palette: Palette) -> impl IntoElement {
    h_flex().flex_none().gap(px(4.)).children(keys.split(' ').map(|key| {
        div()
            .min_w(px(20.))
            .h(px(20.))
            .px(px(5.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .bg(if selected { palette.selected } else { palette.hover })
            .text_size(px(11.))
            .text_color(palette.muted)
            .child(key.to_string())
    }))
}

/// A row's keys or age as plain faint text: "Ctrl Shift S", "4m". No box per key.
fn key_text(text: SharedString, palette: Palette) -> impl IntoElement {
    div()
        .flex_none()
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(palette.faint)
        .child(text)
}

/// A footer hint: the key in faint text, then what it does, 6 apart.
fn hint(key: &'static str, label: &'static str, palette: Palette) -> impl IntoElement {
    h_flex()
        .gap(px(6.))
        .child(div().text_color(palette.faint).child(key))
        .child(label)
}

/// Whether an entry opens in the system browser (a Nexus UI page this build doesn't draw), so its row ends with ↗.
fn opens_web(go: Go) -> bool {
    match go {
        Go::Page("models" | "settings/preferences" | "settings/api-keys" | "library" | "notifications" | "docs") => {
            false
        }
        Go::Page(path) => {
            let page = path.split('/').next().filter(|first| *first == "usage").unwrap_or(path);
            crate::pages::PageId::from_web_path(page).is_none()
        }
        _ => false,
    }
}

/// Where the keyboard goes when the palette closes.
#[derive(Debug, PartialEq)]
enum PageFocus {
    /// The message field of a chat or Code session.
    Composer,
    /// The workspace itself: the page has no field, but shortcuts like Ctrl+K must still reach it.
    Workspace,
    /// The page has its own search field (Library, Models). It takes the keyboard when the page opens, and
    /// again when the palette closes over a page that is already open, since opening the palette moved focus away.
    Own,
}

fn focus_for(view: &View) -> PageFocus {
    match view {
        View::Chat | View::Code(_) | View::NewCode => PageFocus::Composer,
        View::Library | View::Models => PageFocus::Own,
        View::Inbox
        | View::TeamSessions
        | View::Docs
        | View::Issues
        | View::Settings(_)
        | View::Page(_)
        | View::Pull(_) => PageFocus::Workspace,
    }
}

#[cfg(test)]
mod tests;
