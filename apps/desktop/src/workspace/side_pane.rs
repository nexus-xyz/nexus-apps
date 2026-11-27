//! The Workbench, as the Nexus app's side pane (`desktop/src/workbench/side-pane.tsx`): tools in tabs on the right
//! (Changes, Files, then Browser and Terminal once opened, and Artifacts while a page is open), a "+" for the tools
//! not open yet, and a left edge that resizes it. It works on every screen: in a Code session on the session's folder,
//! on the New session screen on the picked project's, elsewhere on the home folder, where only Terminal and Browser
//! show. Alt+Cmd/Ctrl+\ shows and hides it, Ctrl+1 to Ctrl+6 pick a tool (Ctrl+Alt+1 to 6 off a Mac), Ctrl+` the
//! terminal, Ctrl+Shift+` a new terminal, Cmd/Ctrl+Shift+Enter gives it the whole window. The tools themselves are in
//! `src/side/`.

use crate::motion::TweenExt as _;
use crate::workspace::MenuTrigger;
use crate::workspace::menu_item_hover;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{View, Workspace, bar_button};
use crate::code::Status;
use crate::palette::Palette;
use crate::shortcut_label;
use crate::side::browser::{Browser, BrowserEvent, RetainedBrowser};
use crate::side::changes::{Changes, ChangesEvent};
use crate::side::files::Files;
use crate::side::shared_terminals::SharedTerminals;
use crate::side::terminals::{RetainedTerminalSet, TerminalSet, TerminalSetEvent};
use nexus_browser::place::Place;
use nexus_browser::profile::Profile;

mod browser_detach;
mod commands_controls;
mod commands_detach;
mod connection;
mod empty;
mod entries;
mod files_detach;
mod fix_terminal;
pub(super) mod navigation;
pub(super) mod nothing_open;
mod overview;
mod server_terminal;
mod tab_motion;
mod visibility;

pub(in crate::workspace) use fix_terminal::FixTerminal;

actions!(
    workspace,
    [
        ToggleSidePane,
        ToggleTerminal,
        ToggleChatBrowser,
        NewTerminal,
        ToggleMaximizeWorkbench,
        SideTool1,
        SideTool2,
        SideTool3,
        SideTool4,
        SideTool5,
        SideTool6
    ]
);

const DEFAULT_WIDTH: f32 = nexus_desktop_core::pane_resize::DEFAULT;
type TerminalSetFactory = fn(PathBuf, &mut Context<TerminalSet>) -> TerminalSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Tool {
    Changes,
    Files,
    Terminal,
    Browser,
    Artifacts,
    Agents,
}

/// Every entry in the Workbench strip participates in creation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tab {
    Tool(Tool),
    SideChat,
}

impl Tool {
    /// Default order and fixed shortcut mapping. Open tabs keep their creation order.
    pub(in crate::workspace) const ALL: [Tool; 6] = [
        Tool::Changes,
        Tool::Files,
        Tool::Browser,
        Tool::Terminal,
        Tool::Artifacts,
        Tool::Agents,
    ];

    fn label(self) -> &'static str {
        match self {
            Tool::Changes => "Changes",
            Tool::Files => "Files",
            Tool::Terminal => "Terminal",
            Tool::Browser => "Browser",
            Tool::Artifacts => "Artifacts",
            Tool::Agents => "Background tasks",
        }
    }

    /// Its shortcut's id (`key_bindings.rs`): Ctrl+1 to Ctrl+6 on a Mac, Ctrl+Alt+1 to 6 elsewhere, unless the person
    /// changed it.
    pub(super) fn shortcut_id(self) -> &'static str {
        match self {
            Tool::Changes => "tool-changes",
            Tool::Files => "tool-files",
            Tool::Terminal => "open-terminal",
            Tool::Browser => "open-browser",
            Tool::Artifacts => "tool-artifacts",
            Tool::Agents => "tool-agents",
        }
    }

    fn icon(self) -> Lucide {
        match self {
            Tool::Changes => Lucide::FileDiff,
            Tool::Files => Lucide::Files,
            Tool::Terminal => Lucide::SquareTerminal,
            Tool::Browser => Lucide::Globe,
            Tool::Artifacts => Lucide::FileText,
            Tool::Agents => Lucide::Bot,
        }
    }
}

/// The pane's left edge, while it is dragged.
#[derive(Clone)]
struct Resize;

impl Render for Resize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// What the Workbench works on, from the screen: its folder and whether Changes and Files show.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Scope {
    /// A Code session, by its thread id: the session's folder.
    Session(String),
    /// The New session screen's picked project, by its id: the project's folder.
    Project(String),
    /// A saved Gateway chat owns its Browser tabs, without exposing a project folder.
    Chat(String),
    /// Anywhere else: the home folder, Terminal and Browser only.
    Home,
}

/// The Workbench's commands in the palette (`search.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Command {
    Toggle,
    NewTerminal,
    Browser,
    Changes,
    Files,
    Maximize,
}

/// Private terminal owners and their Workbench presentation, without any window-bound entities.
/// The window registry must retain this only for an ordinary primary-window close, and must
/// discard it on quit or an account/Gateway change. Session browser pages share the identity envelope.
pub(crate) struct RetainedPrivateTerminals {
    side_chats: super::side_chat::RetainedSides,
    browsers: HashMap<String, RetainedBrowser>,
    parked_browsers: Vec<ParkedBrowser>,
    browser_source: Option<crate::side::browser::agent::LocalOrigin>,
    browser_authority: Option<crate::backend::ExecutionAuthority>,
    home_browser: String,
    sets: HashMap<PathBuf, RetainedTerminalSet>,
    terminal_factory: TerminalSetFactory,
    home: nexus_desktop_core::home_bench::Visibility,
    visibility: visibility::Visibility,
    layout: nexus_desktop_core::pane_layout::Visibility,
    open: bool,
    maximized: bool,
    width: f32,
    tool: Tool,
    session_tool: Tool,
    dir: PathBuf,
    scope: Option<Scope>,
    tab_order: HashMap<String, Vec<Tab>>,
}

/// Why a window does not take an agent's browser call (`Workspace::browser_refusal`).
pub(crate) const BROWSER_SIGNED_OUT: &str = "Sign in to Nexus in a window to use the browser.";
pub(crate) const BROWSER_NOT_READ: &str = "The Nexus window is still reading its sessions. Try again shortly.";
pub(crate) const BROWSER_OTHER_ORG: &str =
    "No Nexus window is signed in to this session's organization. Sign in to it to use the browser.";
pub(crate) const BROWSER_NO_SESSION: &str = "No Nexus window holds this session.";
pub(crate) const BROWSER_NO_SERVER_SESSION: &str =
    "No Nexus window holds this server session. Check that the server is connected in Settings › Remote servers.";

/// Plain local Code pages waiting for their exact store reader; no old native view survives.
struct ParkedBrowser {
    session: String,
    browser: RetainedBrowser,
    source: crate::side::browser::agent::LocalOrigin,
    identity: crate::windows::retention::Identity,
    authority: crate::side::browser::agent::Guard,
}
impl ParkedBrowser {
    fn output_conflict(&self, key: &super::browser_detach::Key) -> bool {
        self.identity == key.identity
            && self.session == key.session
            && (self.authority)()
            && !matches!(&key.source, super::runs::Source::Local { data, scope }
                if *data == self.source.data && *scope == self.source.scope)
    }
    fn matches(&self, thread: &str, source: &crate::side::browser::agent::LocalOrigin) -> bool {
        self.session == thread && &self.source == source && (self.authority)()
    }
}

impl RetainedPrivateTerminals {
    /// A captured Account epoch is sticky even when its final credential fingerprint matches again.
    pub(super) fn discard_stale_browsers(&mut self) {
        if self.browser_authority.as_ref().is_some_and(|proof| !proof.current()) {
            self.browsers.clear();
        }
        self.parked_browsers.retain(|saved| (saved.authority)());
    }
    pub(super) fn has_browser_origin(&self, thread: &str, origin: &crate::side::browser::agent::LocalOrigin) -> bool {
        (self.browser_source.as_ref() == Some(origin)
            && self.browser_authority.as_ref().is_some_and(|proof| proof.current())
            && self.browsers.contains_key(thread))
            || self.parked_browsers.iter().any(|saved| saved.matches(thread, origin))
    }
    pub(super) fn browser_output_conflict(&self, key: &super::browser_detach::Key) -> bool {
        if self.parked_browsers.iter().any(|saved| saved.output_conflict(key)) {
            return true;
        }
        if self.browser_authority.as_ref().is_none_or(|proof| !proof.current()) {
            return false;
        }
        let Some(origin) = &self.browser_source else {
            return false;
        };
        if matches!(&key.source, super::runs::Source::Local { data, scope } if *data == origin.data && *scope == origin.scope)
        {
            return false;
        }
        match crate::code::CodeStore::open_reading_in(&origin.data, origin.scope.clone()) {
            Ok(Some(code)) => code.summary(&key.session).map_or(true, |summary| summary.is_some()),
            _ => true,
        }
    }
}

pub(super) struct SidePane {
    pub open: bool,
    visibility: visibility::Visibility,
    navigation: HashMap<String, navigation::Navigation>,
    navigation_wake: Option<Task<()>>,
    /// The check that a file the agent linked exists, before following opens it (`navigation.rs`). A newer one
    /// replaces it.
    follow_file: Option<Task<()>>,
    pub(super) pending_pages: HashMap<String, (String, u64)>,
    motion: Cell<super::pane_motion::Motion>,
    motion_task: RefCell<Option<(usize, Task<()>)>>,
    /// The tab on screen, and whether it came by a switch from another while the pane was open (it fades in).
    shown_tab: Cell<Option<(usize, bool)>>,
    /// Whether the Workbench was open at the overview's last frame, and since when it has been so (None: since the
    /// window opened). The overview moves for a while after a change (`overview.rs`).
    overview: Cell<Option<(bool, Option<std::time::Instant>)>>,
    home: nexus_desktop_core::home_bench::Visibility,
    home_browser: String,
    session_tool: Tool,
    pub layout: nexus_desktop_core::pane_layout::Visibility,
    /// The Workbench takes the whole content area: no session, no composer. Not remembered.
    pub maximized: bool,
    width: f32,
    resize_focus: Option<FocusHandle>,
    tool: Tool,
    /// Each scope keeps existing tabs in place and appends newly visible tools.
    tab_order: RefCell<HashMap<String, Vec<Tab>>>,
    /// Tabs growing into the strip or leaving it.
    tab_motion: tab_motion::TabMotion,
    /// What the tools are for; Changes and Files start over for another.
    scope: Option<Scope>,
    /// A restored route keeps its known folder until fresh metadata resolves it.
    restoring_scope: Option<(Scope, super::Place)>,
    /// The scope's folder, where a terminal starts.
    dir: PathBuf,
    /// The server the scope is on: its Changes and Files read there, and its shells run there when its host opens
    /// them (`side_pane/server_terminal.rs`).
    server: Option<String>,
    changes: Option<Entity<Changes>>,
    files: Option<Entity<Files>>,
    files_key: Option<super::files_detach::Key>,
    files_error: Option<String>,
    files_blocked: bool,
    /// The terminals of each folder (or server session, [`SidePane::terminal_key`]), kept while the Workbench hides or
    /// the screen changes, so their shells go on.
    terminals: HashMap<PathBuf, (Entity<TerminalSet>, Subscription)>,
    terminal_factory: TerminalSetFactory,
    server_shells: server_terminal::ServerShells,
    server_set_factory: server_terminal::ServerSetFactory,
    shared: HashMap<String, Entity<SharedTerminals>>,
    shared_keys: HashMap<String, super::commands_detach::Key>,
    /// Each session's browser (a project's, or the home folder's, elsewhere), kept while another shows, with its tabs
    /// and pages, as the Nexus app keeps one per session.
    browsers: HashMap<String, Entity<Browser>>,
    parked_browsers: Vec<ParkedBrowser>,
    browser_events: HashMap<String, Subscription>,
    /// The session and transcript length its browser last heard the session's servers for.
    found_blocks: Option<(String, usize)>,
    /// The session, transcript length and whether older events were still unread when the Changes tab last read its
    /// marks, and its file edits then.
    read_blocks: Option<(String, usize, bool, usize)>,
    plus_menu: bool,
    pub(super) side_chats: HashMap<String, (Entity<super::side_chat::SideChat>, Subscription)>,
    pub(super) side_chat_for: HashMap<String, String>,
    pub(super) side_chat_visible: bool,
    pub(super) side_chat_busy: bool,
    #[cfg(test)]
    pub(super) side_chat_fixture_runs: Option<super::runs::Runs>,
    #[cfg(test)]
    pub(super) side_chat_fixture_guard: Option<std::rc::Rc<dyn Fn() -> bool>>,
    pub(super) side_chat_retired: bool,
    pub(super) side_chat_waiting: HashMap<String, super::side_chat::RetainedChat>,
    pub(super) side_chat_creation: super::side_chat::Creation,
    pub(super) side_chat_creation_watch: Option<Task<()>>,
    pub(super) side_chat_error: Option<String>,
    pub(super) agents: Option<super::agents_tool::AgentsTool>,
    /// The session's Connection details (/status), shown in the tools' place while its session is open.
    pub(super) connection: Option<connection::ConnectionPane>,
    /// Whether the agent was working at the last look: when it stops, Changes and Files read again.
    was_running: bool,
    /// Whether a page was open at the last look: a new one opens the Artifacts tab.
    had_page: bool,
    _changes: Option<Subscription>,
    _save: Option<Task<()>>,
}

impl SidePane {
    pub(super) fn restoring_route(&self) -> bool {
        self.restoring_scope.is_some()
    }

    pub(super) fn clear_browsers(&mut self, cx: &mut App) {
        self.parked_browsers.clear();
        self.browser_events.clear();
        for (_, browser) in std::mem::take(&mut self.browsers) {
            // Retire even if an in-flight action still holds an Entity clone. Dropping
            // the extracted owners closes local and cloud pages before another close.
            browser.update(cx, |browser, cx| drop(browser.retain(cx)));
        }
    }

    pub(super) fn clear_side_chats(&mut self, cx: &mut App) {
        self.side_chat_creation.revoke();
        self.side_chat_creation = Default::default();
        self.side_chat_creation_watch = None;
        self.side_chat_waiting.clear();
        for (_, (chat, _)) in std::mem::take(&mut self.side_chats) {
            chat.update(cx, |chat, _| chat.retire(true));
        }
        self.side_chat_for.clear();
        self.side_chat_visible = false;
        self.side_chat_busy = false;
    }

    pub(super) fn clear_private_terminals(&mut self) {
        self.terminals.clear();
        self.restoring_scope = None;
        self.visibility = Default::default();
        self.open = false;
        self.maximized = false;
        self.layout = Default::default();
    }

    /// Keep seeded session tools through the first headless render.
    #[cfg(test)]
    pub(super) fn fixture_session(&mut self, id: &str) {
        self.scope = Some(Scope::Session(id.into()));
    }

    /// The pane's saved width, for tests that draw it narrow.
    #[cfg(test)]
    pub(super) fn fixture_width(&mut self, width: f32) {
        self.width = width;
    }

    /// Agents navigates within its panel and always leaves room for the conversation.
    pub(super) fn select_tool(&mut self, tool: Tool) {
        self.tool = tool;
        if tool == Tool::Agents {
            self.maximized = false;
        }
    }

    pub fn new() -> SidePane {
        SidePane {
            resize_focus: None,
            open: false,
            visibility: Default::default(),
            navigation: HashMap::new(),
            navigation_wake: None,
            follow_file: None,
            pending_pages: HashMap::new(),
            motion: Cell::default(),
            motion_task: RefCell::default(),
            shown_tab: Cell::default(),
            overview: Cell::default(),
            home: Default::default(),
            home_browser: String::new(),
            session_tool: Tool::Changes,
            layout: Default::default(),
            maximized: false,
            width: crate::paths::side_pane_width().unwrap_or(DEFAULT_WIDTH),
            tool: Tool::Changes,
            tab_order: RefCell::default(),
            tab_motion: Default::default(),
            scope: None,
            restoring_scope: None,
            dir: home_dir(),
            server: None,
            changes: None,
            files: None,
            files_key: None,
            files_error: None,
            files_blocked: false,
            terminals: HashMap::new(),
            terminal_factory: TerminalSet::new,
            server_shells: server_terminal::machine_shells,
            server_set_factory: TerminalSet::on_server,
            shared: HashMap::new(),
            shared_keys: HashMap::new(),
            browsers: HashMap::new(),
            parked_browsers: Vec::new(),
            browser_events: HashMap::new(),
            found_blocks: None,
            read_blocks: None,
            plus_menu: false,
            side_chats: HashMap::new(),
            side_chat_for: HashMap::new(),
            side_chat_visible: false,
            side_chat_busy: false,
            #[cfg(test)]
            side_chat_fixture_runs: None,
            #[cfg(test)]
            side_chat_fixture_guard: None,
            side_chat_retired: false,
            side_chat_waiting: HashMap::new(),
            side_chat_creation: Default::default(),
            side_chat_creation_watch: None,
            side_chat_error: None,
            agents: None,
            connection: None,
            was_running: false,
            had_page: false,
            _changes: None,
            _save: None,
        }
    }

    /// Reconcile visibility without sorting existing tabs back into the shortcut order. A tab that joins a strip
    /// that already has tabs grows in (`tab_motion.rs`).
    fn ordered_tabs(&self, available: Vec<Tab>) -> Vec<Tab> {
        let key = self.browser_key();
        let mut scopes = self.tab_order.borrow_mut();
        let tabs = scopes.entry(key.clone()).or_default();
        tabs.retain(|tool| available.contains(tool));
        let had_tabs = !tabs.is_empty();
        for tool in available {
            if !tabs.contains(&tool) {
                if had_tabs {
                    self.tab_motion.join(&key, tool);
                }
                tabs.push(tool);
            }
        }
        tabs.clone()
    }

    /// Forget a closed tab immediately, even if it reopens before the next render. It leaves the strip narrowing.
    fn forget_tab(&self, tool: Tool) {
        let key = self.browser_key();
        if let Some(tabs) = self.tab_order.borrow_mut().get_mut(&key) {
            if let Some(index) = tabs.iter().position(|tab| *tab == Tab::Tool(tool)) {
                self.tab_motion.leave(&key, index, tool.label(), tool.icon());
            }
            tabs.retain(|tab| *tab != Tab::Tool(tool));
        }
    }

    /// The tab on screen, or last on screen while the pane hides.
    /// The terminals of the folder on screen.
    fn terminal(&self) -> Option<&Entity<TerminalSet>> {
        self.terminals.get(&self.terminal_key()).map(|(set, _)| set)
    }

    fn shared_terminal(&self) -> Option<&Entity<SharedTerminals>> {
        match &self.scope {
            Some(Scope::Session(id)) => self.shared.get(id),
            _ => None,
        }
    }

    fn has_commands(&self) -> bool {
        self.shared_terminal().is_some()
            || matches!(&self.scope, Some(Scope::Session(id)) if self.shared_keys.contains_key(id))
    }

    pub(super) fn command_activity(&self, cx: &mut Context<Workspace>) -> Option<Entity<SharedTerminals>> {
        // The same entity must not render in both the conversation and its Commands pane.
        if self.open && self.tool == Tool::Terminal {
            return None;
        }
        let activity = self
            .shared_terminal()
            .filter(|view| view.read(cx).is_current() && view.read(cx).has_command_attention())
            .cloned()?;
        activity.update(cx, |view, cx| view.allow_history(false, cx));
        Some(activity)
    }

    /// The browser's name for the scope: the session's id, else the project's, else the home folder's.
    fn browser_key(&self) -> String {
        if self.home.home {
            return self.home_browser.clone();
        }
        match &self.scope {
            Some(Scope::Session(id)) => id.clone(),
            Some(Scope::Project(id)) => format!("project:{id}"),
            Some(Scope::Chat(id)) => format!("chat:{id}"),
            Some(Scope::Home) | None => "home".into(),
        }
    }

    /// The browser of the scope on screen.
    fn browser(&self) -> Option<&Entity<Browser>> {
        self.browsers.get(&self.browser_key())
    }

    /// Whether Changes and Files show: a session's or a project's folder.
    fn has_folder(&self) -> bool {
        matches!(self.scope, Some(Scope::Session(_) | Scope::Project(_)))
    }

    /// A session that has not started yet (a draft, scoped to its project): it has no stored session for Commands,
    /// so its own shells are drawn as the Commands tab it gets once it starts (`side/terminals/commands.rs`).
    pub(super) fn draft_commands(&self) -> bool {
        !self.home.home && self.server.is_none() && matches!(self.scope, Some(Scope::Project(_)))
    }

    /// Whether the terminal tool is the Commands tab, now or as a draft's.
    fn shows_commands(&self) -> bool {
        self.has_commands() || self.draft_commands()
    }

    /// Whether the terminal tool shows: a folder's shells here, or a server's note.
    fn has_terminal(&self) -> bool {
        self.shows_commands() || self.terminal().is_some() || (self.server.is_some() && self.tool == Tool::Terminal)
    }

    /// A page appeared, on any screen: the Workbench opens on its Artifacts tab.
    fn sync_page(&mut self, has_page: bool) {
        if has_page && !self.had_page {
            self.open = true;
            self.tool = Tool::Artifacts;
        }
        self.had_page = has_page;
    }

    /// Another page replaced the open one: the Workbench opens on it again, even if it was closed.
    pub(super) fn page_replaced(&mut self) {
        self.had_page = false;
    }

    /// The Workbench is open on `tool`.
    pub(super) fn showing(&self, tool: Tool) -> bool {
        self.open && self.tool == tool
    }

    /// The page in full view: the Artifacts tab, maximized over the whole content area.
    pub(super) fn page_full(&self) -> bool {
        self.open && self.maximized && self.tool == Tool::Artifacts
    }

    /// Put the page in full view, or the Workbench back beside the session. Whether the page is in full view now.
    pub(super) fn toggle_page_full(&mut self) -> bool {
        if self.page_full() {
            self.maximized = false;
        } else {
            self.open = true;
            self.tool = Tool::Artifacts;
            self.maximized = true;
            self.plus_menu = false;
        }
        self.page_full()
    }

    /// Leave the page's full view, if it is in it: the canvas's Escape. Whether it left.
    pub(super) fn leave_page_full(&mut self) -> bool {
        let full = self.page_full();
        if full {
            self.maximized = false;
        }
        full
    }

    /// The tab to show when `tool` goes: Changes where there is a folder, else another open tool.
    fn fallback(&self) -> Option<Tool> {
        if self.home.home {
            return if self.terminal().is_some() {
                Some(Tool::Terminal)
            } else if self.browser().is_some() {
                Some(Tool::Browser)
            } else {
                None
            };
        }
        if self.has_folder() {
            Some(Tool::Changes)
        } else if self.terminal().is_some() {
            Some(Tool::Terminal)
        } else if self.browser().is_some() {
            Some(Tool::Browser)
        } else {
            None
        }
    }
}

/// The header's "Move to window" button, beside Maximize and Close, as the other window controls are.
fn move_window_button(id: &'static str, palette: Palette) -> Stateful<Div> {
    bar_button(id, Lucide::SquareArrowOutUpRight, palette)
        .debug_selector(move || id.into())
        .tooltip(|window, cx| Tooltip::new("Move to window").build(window, cx))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(Into::into)
        .unwrap_or_else(|| ".".into())
}
impl Workspace {
    /// Called before the primary window drops. Existing event subscriptions are dropped here;
    /// the saved value contains only processes and ordinary data, never old WindowHandles.
    pub(crate) fn retain_private_terminals(&mut self, cx: &mut Context<Self>) -> RetainedPrivateTerminals {
        self.side.clear_files(true, cx);
        self.side.clear_commands(true, cx);
        let sets = std::mem::take(&mut self.side.terminals)
            .into_iter()
            .map(|(dir, (set, subscription))| {
                drop(subscription);
                (dir, set.update(cx, |set, cx| set.retain(cx)))
            })
            .collect();
        self.side.browser_events.clear();
        let home_browser = self.side.home_browser.clone();
        let owned = std::mem::take(&mut self.side.browsers);
        let mut browsers = HashMap::new();
        let mut parked_browsers = std::mem::take(&mut self.side.parked_browsers);
        let side_chats = self.retain_side_chats(cx);
        let browser_source = self.code.as_ref().map(|code| {
            let (data, scope) = code.store_source();
            crate::side::browser::agent::LocalOrigin {
                data: data.to_path_buf(),
                scope: scope.into(),
                machine: None,
            }
        });
        for (session, browser) in owned {
            super::browser_detach::park(&browser, cx);
            let local = browser.read(cx).local_owner();
            let browser = browser.update(cx, |browser, cx| browser.retain(cx));
            if let Some((source, identity, authority)) = local {
                parked_browsers.push(ParkedBrowser {
                    session,
                    browser,
                    source,
                    identity,
                    authority,
                });
            } else {
                browsers.insert(session, browser);
            }
        }
        RetainedPrivateTerminals {
            side_chats,
            browsers,
            parked_browsers,
            browser_source,
            browser_authority: self.gateway_account_authority.clone(),
            home_browser,
            sets,
            terminal_factory: self.side.terminal_factory,
            home: std::mem::take(&mut self.side.home),
            visibility: std::mem::take(&mut self.side.visibility),
            layout: std::mem::take(&mut self.side.layout),
            open: self.side.open,
            maximized: self.side.maximized,
            width: self.side.width,
            tool: self.side.tool,
            session_tool: self.side.session_tool,
            dir: self.side.dir.clone(),
            scope: self
                .side
                .restoring_scope
                .as_ref()
                .map(|(scope, _)| scope.clone())
                .or_else(|| self.side.scope.clone()),
            tab_order: self.side.tab_order.borrow().clone(),
        }
    }

    /// Restore only after the registry verifies the original account/Gateway is still active
    /// and restores the workspace route.
    /// Recreate each view and bind terminal-close events to this new window.
    pub(crate) fn restore_private_terminals(
        &mut self,
        mut saved: RetainedPrivateTerminals,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The bundle can leave Shell while still current, then expire before this consumer runs.
        saved.discard_stale_browsers();
        let browser_authority = saved.browser_authority.clone();
        self.side.parked_browsers.append(&mut saved.parked_browsers);
        self.side.terminals = saved
            .sets
            .into_iter()
            .map(|(dir, saved)| {
                let set = cx.new(|cx| TerminalSet::restore(saved, cx));
                let subscription = cx.subscribe_in(&set, window, Self::on_terminal_set_event);
                (dir, (set, subscription))
            })
            .collect();
        // Home belongs to this primary resource slot, not its retired native window. Keep
        // the CEF/session identity inside Browser intact, remapping only the new UI lookup.
        let org = self
            .account
            .as_ref()
            .map(|account| account.org.as_str())
            .unwrap_or_default();
        let home_key =
            nexus_desktop_core::home_bench::home_browser(&format!("{:?}", window.window_handle().window_id()), org);
        if let Some(tabs) = saved.tab_order.remove(&saved.home_browser) {
            saved.tab_order.insert(home_key.clone(), tabs);
        }
        for (key, browser) in saved.browsers {
            if browser_authority.as_ref().is_some_and(|proof| !proof.current()) {
                continue;
            }
            let key = if key == saved.home_browser {
                home_key.clone()
            } else {
                key
            };
            let browser = cx.new(|cx| Browser::restore(browser, window, cx));
            if browser_authority.as_ref().is_some_and(|proof| !proof.current()) {
                browser.update(cx, |browser, cx| drop(browser.retain(cx)));
                continue;
            }
            self.bind_browser(key, browser, window, cx);
        }
        self.side.home_browser = home_key;
        self.side.terminal_factory = saved.terminal_factory;
        self.side.open = saved.open;
        self.side.maximized = saved.maximized;
        self.side.width = saved.width;
        self.side.tool = saved.tool;
        self.side.session_tool = saved.session_tool;
        // Rebuild Files/Changes for the restored route; those entities belonged to the old window.
        self.side.scope = None;
        self.side.restoring_scope = saved
            .scope
            .filter(|scope| matches!(scope, Scope::Session(_) | Scope::Project(_)))
            .map(|scope| {
                (
                    scope,
                    self.history
                        .get(self.history_at)
                        .cloned()
                        .unwrap_or(super::Place::Chat(None)),
                )
            });
        self.side.dir = saved.dir;
        self.side.home = saved.home;
        self.side.visibility = saved.visibility;
        self.side.layout = saved.layout;
        self.side.tab_order = RefCell::new(saved.tab_order);
        self.restore_side_chats(saved.side_chats, window, cx);
        self.restore_parked_browsers(window, cx);
        cx.notify();
    }

    /// A replacement can read no store yet, or another store with the same session IDs.
    /// Keep plain pages until the original reader and captured Account proof both admit them.
    fn restore_parked_browsers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = std::mem::take(&mut self.side.parked_browsers);
        for saved in pending {
            if !(saved.authority)() {
                continue;
            }
            if self.terminal_identity.as_ref() != Some(&saved.identity)
                || !self.owns_browser_origin(&saved.source, &saved.session)
                || self.side.browsers.contains_key(&saved.session)
            {
                self.side.parked_browsers.push(saved);
                continue;
            }
            let browser = cx.new(|cx| Browser::restore(saved.browser, window, cx));
            if !(saved.authority)()
                || self.terminal_identity.as_ref() != Some(&saved.identity)
                || !self.owns_browser_origin(&saved.source, &saved.session)
            {
                browser.update(cx, |browser, cx| drop(browser.retain(cx)));
                continue;
            }
            self.bind_browser(saved.session, browser, window, cx);
        }
    }

    pub(in crate::workspace) fn parked_browser_output_conflict(&self, key: &super::browser_detach::Key) -> bool {
        self.side.parked_browsers.iter().any(|saved| saved.output_conflict(key))
    }

    pub(crate) fn has_parked_browser_origin(
        &self,
        thread: &str,
        origin: &crate::side::browser::agent::LocalOrigin,
    ) -> bool {
        self.navigation_allowed()
            && self.side.parked_browsers.iter().any(|saved| {
                self.terminal_identity.as_ref() == Some(&saved.identity)
                    && saved
                        .identity
                        .matches_credential(self.gateway.url(), self.gateway.marker().as_deref())
                    && saved.matches(thread, origin)
            })
    }

    pub(super) fn session_place(&self, id: &str) -> crate::side::Place {
        self.scope_place(&Scope::Session(id.to_string()))
    }

    /// Open a picker result in the same Files tool as a tree row.
    pub(super) fn open_in_files(
        &mut self,
        path: String,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_manual_workbench();
        if self.route_files_open(&path, line, window, cx) {
            return;
        }
        self.side.open = true;
        self.side.tool = Tool::Files;
        self.sync_side(window, cx);
        if let Some(files) = &self.side.files {
            files.update(cx, |files, cx| files.open_at(path, line, window, cx));
        }
        cx.notify();
    }

    /// Whether a Workbench terminal has the keys, so the workspace's own keys (Esc stops the agent) leave them.
    pub(super) fn side_terminal_focused(&self, window: &Window, cx: &App) -> bool {
        self.side.open
            && self.side.tool == Tool::Terminal
            && self
                .side
                .terminal()
                .is_some_and(|set| set.read(cx).contains_focus(window, cx))
    }

    fn side_scope(&self) -> Scope {
        match &self.view {
            // A pending view has no stored session yet. Its tools can only read the chosen project.
            View::Code(view) if view.is_draft() => view
                .pending
                .as_ref()
                .map(|pending| Scope::Project(pending.new.project_id.clone()))
                .unwrap_or(Scope::Home),
            View::Code(view) => Scope::Session(view.id.clone()),
            // A project here or on a server: a server's tools read there (`scope_place`). The cloud screen keeps its own.
            View::NewCode => {
                let project = self
                    .code_form
                    .project
                    .as_ref()
                    .and_then(|id| self.projects.iter().find(|project| &project.id == id))
                    .filter(|_| !self.cloud_ui.on);
                match project {
                    Some(project) => Scope::Project(project.id.clone()),
                    None => Scope::Home,
                }
            }
            View::Chat if self.current.is_some() => Scope::Chat(self.current.clone().unwrap_or_default()),
            _ => Scope::Home,
        }
    }

    /// The server a session or project of `scope` is on: the servers' own list, else the sidebar's project.
    fn scope_machine(&self, scope: &Scope) -> Option<String> {
        let (Scope::Session(id) | Scope::Project(id)) = scope else {
            return None;
        };
        crate::machines::get().machine_of(id).or_else(|| {
            matches!(scope, Scope::Project(_))
                .then(|| self.projects.iter().find(|project| &project.id == id))
                .flatten()
                .and_then(|project| project.machine.clone())
        })
    }

    /// Where the scope's tools read: its server's session or project, else its folder here.
    fn scope_place(&self, scope: &Scope) -> crate::side::Place {
        let machines = crate::machines::get();
        if let Scope::Session(id) | Scope::Project(id) = scope
            && let Some(machine) = self.scope_machine(scope)
        {
            let thread = matches!(scope, Scope::Session(_)).then(|| id.clone());
            let project = matches!(scope, Scope::Project(_)).then(|| id.clone());
            return crate::side::Place::Server(crate::side::server::ServerPlace {
                machines,
                machine,
                thread,
                project,
            });
        }
        crate::side::Place::Here(self.scope_folder(scope))
    }

    /// The scope's folder: a session's or a project's, else (or when it is not known) the home folder.
    fn scope_folder(&self, scope: &Scope) -> Result<crate::code::SessionFolder, String> {
        // A server's folders are there, not on this computer (`scope_place`).
        if self.scope_machine(scope).is_some() {
            return Err("This folder is on a server.".into());
        }
        let Some(code) = &self.code else {
            return Err("The Nexus app's sessions aren't on this computer.".into());
        };
        match scope {
            Scope::Session(id) => code.folder(id),
            Scope::Project(id) => code
                .project_folder(id)
                .map(|path| crate::code::SessionFolder { path, base: None }),
            Scope::Home | Scope::Chat(_) => Err("No folder.".into()),
        }
    }

    #[cfg(test)]
    pub(in crate::workspace) fn fixture_select_code_browser(
        &mut self,
        session: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.view = View::Code(super::code_view::CodeView::new(session.into(), window, cx));
        self.side.scope = Some(Scope::Session(session.into()));
        self.side.tool = Tool::Browser;
        self.side.open = true;
        // Fixture clicks exercise manual interaction without reading or writing the person's preference.
        self.side
            .navigation
            .insert(session.into(), navigation::Navigation::new(true));
    }
    #[cfg(test)]
    pub(in crate::workspace) fn fixture_session_browser(&self, session: &str) -> Option<Entity<Browser>> {
        self.side.browsers.get(session).cloned()
    }

    #[cfg(test)]
    pub(in crate::workspace) fn fixture_local_browser(
        &mut self,
        session: &str,
        page: crate::web::RetainedPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Browser> {
        self.side.scope = Some(Scope::Session(session.into()));
        self.side.tool = Tool::Browser;
        self.side.open = true;
        let profile = self.browser_profile();
        let browser = cx.new(|cx| Browser::fixture_local(session, page, profile, window, cx));
        assert!(
            browser.read(cx).profile() == profile,
            "local fixture must use its accepted Workspace profile"
        );
        self.bind_browser(session.into(), browser, window, cx)
    }
    /// A new browser for `key` (a session's id), opening `url`. Its screenshots and text go to the composer, and its
    /// Stop interrupts the session's agent.
    fn make_browser(
        &mut self,
        key: String,
        url: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Browser>> {
        if let Some(owner) = self.browser_detach_key(&key) {
            if let Some(browser) = self.restore_returned_browser(&owner, window, cx) {
                return Some(browser);
            }
            if !super::browser_detach::allow_new(&owner, window.window_handle().window_id(), cx) {
                return None;
            }
        }
        let place = browser_place(&key);
        let profile = self.browser_profile_of(place.as_ref().ok());
        // A Code session's agent drives its browser: a local session's, and a server session's, whose host passes its
        // agent's browser calls here (`machines/server_tools.rs`). Not home's, a project's or a cloud session's.
        let agent = match &place {
            Ok(Place::Local) => self
                .code
                .as_ref()
                .is_some_and(|code| code.summary(&key).is_ok_and(|summary| summary.is_some())),
            Ok(place) => place.agent_drives(),
            Err(_) => false,
        };
        // A cloud machine's addresses are the machine's own: its browser opens only what Preview forwards.
        let url = url.filter(|_| place.as_ref().is_ok_and(Place::checks_servers));
        let managed = agent || matches!(&self.view, View::Code(view) if view.id == key);
        let browser = cx.new(|cx| {
            let mut browser = Browser::new(key.clone(), place.ok(), profile, url, window, cx);
            if managed {
                browser.manage_navigation();
            }
            if agent {
                browser.allow_agent();
            }
            browser
        });
        Some(self.bind_browser(key, browser, window, cx))
    }

    fn bind_browser(
        &mut self,
        key: String,
        browser: Entity<Browser>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Browser> {
        if let (Some(origin), Some(identity), Some(proof)) = (
            self.browser_source(&key),
            &self.terminal_identity,
            &self.gateway_account_authority,
        ) {
            if self.owns_browser_origin(&origin, &key) {
                browser.update(cx, |browser, _| {
                    browser.bind_permission_owner(origin, identity.clone(), proof.clone())
                });
            }
        }
        // A parked owner already holds its exact Runs/source. A fresh reader may become ready before
        // this Workspace's runner; attach that owner without recapturing or falling back to dynamic routing.
        let validated_key = self.browser_detach_key(&key);
        let restored = validated_key
            .as_ref()
            .is_some_and(|key| super::browser_detach::attached(key, &browser, window, cx));
        let origin = if restored {
            None
        } else {
            self.browser_origin(&key, window.window_handle().window_id(), cx)
        };
        let registered = restored
            || origin
                .as_ref()
                .is_none_or(|origin| super::browser_detach::register(origin.clone(), &browser, window, false, cx));
        if !registered {
            browser.update(cx, |browser, cx| drop(browser.retain(cx)));
            return browser;
        }
        let owner_key = if restored {
            validated_key
        } else {
            origin.map(|origin| origin.key)
        };
        let session = key.clone();
        let events = cx.subscribe_in(
            &browser,
            window,
            move |this, browser, event: &BrowserEvent, window, cx| {
                if let Some(key) = &owner_key {
                    if let Some(owner) = super::browser_detach::event_owner(key, browser.entity_id(), cx) {
                        super::browser_detach::route_event(owner, browser.entity_id(), event, cx);
                    }
                    return;
                }
                match event {
                    // "Screenshot into the message": the page's image goes to the composer, as an attached file does.
                    BrowserEvent::Screenshot(image) => this.attach(vec![image.clone()], cx),
                    BrowserEvent::Text(text) => this.add_to_composer(text, window, cx),
                    BrowserEvent::StopAgent => {
                        if let Some(runs) = this.runs(&session) {
                            runs.interrupt(session.clone());
                        }
                    }
                    BrowserEvent::Navigated => {
                        if matches!(&this.view, View::Code(view) if view.id == session) {
                            this.manual_workbench();
                        }
                    }
                    BrowserEvent::Activity(tab) => this.native_browser_activity(&session, tab, cx),
                    BrowserEvent::Permission => cx.notify(),
                }
            },
        );
        self.side.browsers.insert(key.clone(), browser.clone());
        self.side.browser_events.insert(key, events);
        self.side.found_blocks = None;
        browser
    }

    /// Whether this window holds the browser of the session `thread`.
    pub(crate) fn has_browser(&self, thread: &str) -> bool {
        self.side.browsers.contains_key(thread)
    }

    /// A caller's authenticated local store must own this session before any browser is created: a session in that
    /// store, or one that the caller's SSH server holds.
    pub(crate) fn owns_browser_origin(&self, origin: &crate::side::browser::agent::LocalOrigin, thread: &str) -> bool {
        self.browser_refusal(origin, thread).is_none()
    }

    /// Why this window does not take a browser call from `origin` for session `thread`, or `None` when it does.
    pub(crate) fn browser_refusal(
        &self,
        origin: &crate::side::browser::agent::LocalOrigin,
        thread: &str,
    ) -> Option<&'static str> {
        if !self.navigation_allowed()
            || self
                .gateway_account_authority
                .as_ref()
                .is_none_or(|proof| !proof.current())
            || self.terminal_identity.as_ref().is_none_or(|identity| {
                !identity.matches_credential(self.gateway.url(), self.gateway.marker().as_deref())
            })
            || self
                .account
                .as_ref()
                .and_then(|account| self.current_identity(account))
                .as_ref()
                != self.terminal_identity.as_ref()
        {
            return Some(BROWSER_SIGNED_OUT);
        }
        let Some(code) = &self.code else {
            return Some(BROWSER_NOT_READ);
        };
        let (data, scope) = code.store_source();
        if scope != origin.scope {
            return Some(BROWSER_OTHER_ORG);
        }
        let known = data == origin.data
            && match &origin.machine {
                None => code.summary(thread).is_ok_and(|summary| summary.is_some()),
                Some(machine) => self.machine_source.server_session(machine, thread).is_some(),
            };
        match (known, &origin.machine) {
            (true, _) => None,
            (false, None) => Some(BROWSER_NO_SESSION),
            (false, Some(_)) => Some(BROWSER_NO_SERVER_SESSION),
        }
    }

    /// The source of session `key`'s browser in this window: this window's store, and the SSH server that holds
    /// the session when this store does not.
    pub(super) fn browser_source(&self, key: &str) -> Option<crate::side::browser::agent::LocalOrigin> {
        let code = self.code.as_ref()?;
        let (data, scope) = code.store_source();
        let local = code.summary(key).is_ok_and(|summary| summary.is_some());
        let machine = (!local)
            .then(|| self.machine_source.machine_of(key))
            .flatten()
            .filter(|machine| self.machine_source.server_session(machine, key).is_some());
        Some(crate::side::browser::agent::LocalOrigin {
            data: data.to_path_buf(),
            scope: scope.into(),
            machine,
        })
    }

    pub(crate) fn browser_authority(
        &self,
        origin: &crate::side::browser::agent::LocalOrigin,
        thread: &str,
    ) -> Option<crate::backend::ExecutionAuthority> {
        self.owns_browser_origin(origin, thread)
            .then(|| self.gateway_account_authority.clone())
            .flatten()
    }

    /// The browser of the session `thread` for its agent's tools, made in the background when the session has none:
    /// the pane stays as it is. An error when the session is not this store's.
    pub(crate) fn agent_browser(
        &mut self,
        thread: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Entity<Browser>, String> {
        self.restore_parked_browsers(window, cx);
        if self.side.parked_browsers.iter().any(|saved| saved.session == thread) {
            return Err("This browser is waiting for its original local source.".into());
        }
        let gone = || "This session is no longer available.".to_string();
        let origin = self.browser_source(thread).ok_or_else(gone)?;
        // A session in this store, else the one its SSH server holds (`browser_source`).
        let summary = match &origin.machine {
            None => self.code.as_ref().ok_or_else(gone)?.summary(thread)?.ok_or_else(gone)?,
            Some(machine) => self.machine_source.server_session(machine, thread).ok_or_else(gone)?,
        };
        let browser = match self.side.browsers.get(thread) {
            Some(browser) => browser.clone(),
            None => self
                .make_browser(thread.to_string(), None, window, cx)
                .ok_or("This session Browser is in another window.")?,
        };
        // A coding agent by its entry's name; Nexus's own agent, and an id no agent has, as "The agent".
        let name = nexus_client::agent_entries::Agent::parse(summary.agent.as_str())
            .map_or("The agent", |agent| agent.entry().identity.name);
        if let (Some(identity), Some(proof)) = (&self.terminal_identity, &self.gateway_account_authority) {
            if !browser.update(cx, |browser, _| {
                browser.bind_permission_owner(origin, identity.clone(), proof.clone())
            }) {
                return Err("This browser's original source authority is no longer available.".into());
            }
        } else {
            return Err("This browser source has no accepted account authority.".into());
        }
        browser.update(cx, |browser, _| browser.set_agent(name, &summary.title));
        Ok(browser)
    }

    /// Text from the browser for the message: after what is typed, on its own lines.
    fn add_to_composer(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.composer.read(cx).value().to_string();
        let value = if typed.trim().is_empty() {
            text.to_string()
        } else {
            format!("{}\n\n{text}", typed.trim_end())
        };
        self.composer.update(cx, |state, cx| {
            state.set_value(value.clone(), window, cx);
            state.focus(window, cx);
        });
        self.composer_empty = value.trim().is_empty();
        cx.notify();
    }

    /// The website profile of a browser at `place`: a cloud machine's own, else the sign-in's ([`Self::browser_profile`]).
    fn browser_profile_of(&self, place: Option<&Place>) -> Profile {
        match place.and_then(Place::machine) {
            Some(machine) => cloud_profile(machine),
            None => self.browser_profile(),
        }
    }

    /// Keep the Workbench's tools with the screen: Changes and Files for its folder (none outside a session or a
    /// project), and read again when the agent stops.
    /// The website profile of the sign-in on screen, as the Nexus app picks it (`configure_browser_profile`): its
    /// Gateway, organization and credential, saved unless the organization keeps nothing on this computer.
    fn browser_profile(&self) -> Profile {
        let org = self
            .account
            .as_ref()
            .map(|account| account.org.clone())
            .unwrap_or_default();
        let marker = self.gateway.marker().unwrap_or_default();
        let scope = nexus_browser::profile::scope(self.gateway.url(), &org, &marker);
        Profile::new(
            &crate::app_id::get(),
            &scope,
            crate::managed::local_cache(crate::managed::policy()),
        )
    }

    /// Another sign-in (an organization switch, a sign-out, another Gateway) closes the browsers of the last one, as
    /// the Nexus app's do. Their saved website profiles stay for when that sign-in comes back.
    /// A cloud session's browser keeps the machine's own unsaved profile, as the Nexus app's
    /// (`configure_cloud_profile`): its pages never see this computer's cookies or sign-ins.
    fn close_other_profiles(&mut self, cx: &mut Context<Self>) {
        if self.side.browsers.is_empty() {
            return;
        }
        let current = self.browser_profile();
        let profiles: Vec<(String, Profile)> = self
            .side
            .browsers
            .iter()
            .map(|(key, browser)| (key.clone(), browser.read(cx).profile()))
            .collect();
        let wanted = |key: &str| match browser_place(key) {
            Ok(Place::Cloud(machine) | Place::Server(machine)) => cloud_profile(&machine),
            _ => current,
        };
        for key in other_profiles(&profiles, wanted) {
            self.side.browser_events.remove(&key);
            if let Some(browser) = self.side.browsers.remove(&key) {
                browser.update(cx, |browser, cx| browser.close_popups(None, cx));
            }
        }
    }

    /// Loading a restored route must not resolve its temporary missing metadata as Home.
    fn waiting_for_restored_scope(&mut self) -> bool {
        let Some((scope, route)) = self.side.restoring_scope.clone() else {
            return false;
        };
        let same_target = match (&scope, &self.view) {
            (Scope::Project(id), View::NewCode) => self.code_form.project.as_ref() == Some(id) && !self.cloud_ui.on,
            _ => true,
        };
        let same_route = self.history.get(self.history_at) == Some(&route) && same_target;
        if !same_route {
            self.side.restoring_scope = None;
            return false;
        }
        let resolved = self.code_overview_loaded
            && self.side_scope() == scope
            && self
                .scope_folder(&scope)
                .is_ok_and(|folder| folder.path == self.side.dir);
        if resolved {
            self.side.restoring_scope = None;
        }
        !resolved
    }

    pub(super) fn sync_side(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        super::files_detach::revoke_stale(cx);
        super::commands_detach::revoke_stale(cx);
        self.restore_parked_browsers(window, cx);
        self.sync_returned_browser(window, cx);
        if self.waiting_for_restored_scope() {
            return;
        }
        if let Some(session) = self.side.agents.as_mut().and_then(|state| state.pending_session.take()) {
            self.open_code(&session, window, cx);
        }
        self.side.resize_focus.get_or_insert_with(|| cx.focus_handle());
        let scope = self.side_scope();
        let home = matches!(self.view, View::NewCode);
        if home && !self.side.home.home {
            self.side.session_tool = self.side.tool;
        } else if !home && self.side.home.home {
            self.side.tool = self.side.session_tool;
        }
        let shown = self.side.layout.docked_choice(self.side.open);
        if let Some(open) = self.side.visibility.switch(&scope, home, shown) {
            self.side.open = self.side.layout.switch_context(open);
            self.side.maximized = false;
        }
        self.side.home.home = home;
        if home {
            let org = self
                .account
                .as_ref()
                .map(|account| account.org.as_str())
                .unwrap_or_default();
            let window_id = format!("{:?}", window.window_handle().window_id());
            self.side.home_browser = nexus_desktop_core::home_bench::home_browser(&window_id, org);
        }
        let was_drawer = self.side.layout.drawer;
        self.side.open = self
            .side
            .layout
            .resize(f32::from(window.viewport_size().width), self.side.open);
        if was_drawer != self.side.layout.drawer {
            self.side.maximized = false;
        }
        self.close_other_profiles(cx);
        if self.side.scope.as_ref() != Some(&scope) {
            for state in self.side.navigation.values_mut() {
                state.reset_history();
            }
            self.side.side_chat_visible = false;
            self.side.side_chat_error = None;
            self.side.agents = None;
            self.side.changes = None;
            self.side.read_blocks = None;
            self.side.clear_files(true, cx);
            self.side._changes = None;
            self.side.plus_menu = false;
            self.side.dir = self
                .scope_folder(&scope)
                .map(|folder| folder.path)
                .unwrap_or_else(|_| home_dir());
            self.side.server = match self.scope_place(&scope) {
                crate::side::Place::Server(server) => Some(server.machine),
                crate::side::Place::Here(_) => None,
            };
            self.side.scope = Some(scope.clone());
        }
        self.sync_returned_files(window, cx);
        self.sync_returned_commands(window, cx);
        self.sync_files_source(window, cx);
        // Read even while the tab hides: its label's badge and ring, and `Workspace::crew`, follow the crew.
        self.sync_agents_tool(cx);
        self.sync_side_chat(window, cx);

        self.ensure_shared_terminal(window, cx);
        // The folder's shells are a draft's Commands, or terminals of their own elsewhere.
        let draft_commands = self.side.draft_commands();
        if let Some(set) = self.side.terminal().cloned() {
            set.update(cx, |set, cx| set.set_commands_look(draft_commands, cx));
        }
        let active = match &self.view {
            View::Code(view) => Some(view.id.as_str()),
            _ => None,
        };
        for (id, shared) in &self.side.shared {
            shared.update(cx, |view, cx| view.monitor(Some(id.as_str()) == active, cx));
        }
        // With no session, a shell starts only when the person asks for one: a closed one stays closed, and an empty
        // Workbench lists what it can open (`side_pane/empty.rs`).
        if self.side.open && self.side.tool == Tool::Terminal && !home {
            self.ensure_terminal(window, cx);
        }
        // The top bar's chip and the Agents tab both show the session's pull requests: read them once per session.
        self.read_pull(cx);
        let has_page = self.page.is_some();
        let auto_held = self.page_opening_auto
            && (!self.side.open
                || match &self.view {
                    View::Code(view) => self.side.navigation.get(&view.id).is_some_and(|state| state.manual),
                    _ => false,
                });
        if auto_held {
            self.side.had_page = has_page;
        } else {
            self.side.sync_page(has_page);
        }
        if home {
            let tab = nexus_desktop_core::home_bench::home_tab(if self.side.tool == Tool::Browser {
                "browser"
            } else {
                "terminal"
            });
            self.side.tool = if tab == "browser" {
                Tool::Browser
            } else {
                Tool::Terminal
            };
        }
        // The session's servers, for its browser: read again only when the transcript grew.
        if let View::Code(view) = &self.view
            && let Some(browser) = self.side.browser().cloned()
        {
            let seen = Some((view.id.clone(), view.blocks.len()));
            if self.side.found_blocks != seen {
                self.side.found_blocks = seen;
                let found = crate::side::session_urls(&view.blocks);
                browser.update(cx, |browser, cx| browser.set_found(found, cx));
            }
        }
        // A tool that has nothing here gives way.
        let missing = match self.side.tool {
            Tool::Changes => !self.side.has_folder(),
            Tool::Files => {
                !self.side.has_folder()
                    && self.side.files_error.is_none()
                    && !self
                        .selected_files_key()
                        .is_some_and(|key| super::files_detach::reserved(&key, window.window_handle().window_id(), cx))
            }
            Tool::Terminal => !self.side.has_terminal(),
            Tool::Browser => {
                self.side.browser().is_none()
                    && !self
                        .browser_detach_key(&self.side.browser_key())
                        .is_some_and(|key| super::browser_detach::is_detached(&key, cx))
            }
            Tool::Artifacts => !has_page,
            Tool::Agents => !matches!(self.view, View::Code(_)),
        };
        if missing {
            match self.side.fallback() {
                Some(tool) => self.side.tool = tool,
                // Nothing open outside a folder: a terminal, as Ctrl+` would. With no session, the empty Workbench.
                None if self.side.open && !home => {
                    self.side.tool = Tool::Terminal;
                    self.ensure_terminal(window, cx);
                }
                None => {}
            }
        }
        if !self.side.open || !self.side.has_folder() {
            return;
        }
        let place = self.scope_place(&scope);
        let thread = match &scope {
            Scope::Session(id) => Some(id.clone()),
            _ => None,
        };
        // A server's session commits there; this computer's need this build to run the sessions.
        let can_commit = thread.as_deref().is_some_and(|thread| self.runs(thread).is_some());
        if self.side.changes.is_none() {
            let place = place.clone();
            let thread = thread.clone();
            let changes = cx.new(|cx| Changes::new(thread, place, can_commit, window, cx));
            self.side._changes = Some(cx.subscribe_in(&changes, window, Self::on_changes_event));
            self.side.changes = Some(changes);
        }
        if self.side.files.is_none()
            && !self.side.files_blocked
            && !matches!(&scope, Scope::Session(_) if matches!(&place, crate::side::Place::Here(_)))
        {
            self.side.files = Some(cx.new(|cx| Files::new(place, window, cx)));
            self.side.files_key = None;
        }
        if let Some(changes) = &self.side.changes {
            let reason = self.account_control_reason();
            changes.update(cx, |changes, cx| {
                changes.set_can_commit(can_commit, cx);
                changes.set_write_refusal(reason, cx);
            });
        }
        // The session's turn ends number its turns in the picker; a file the agent wrote reloads what follows edits.
        if let (View::Code(view), Some(changes)) = (&self.view, &self.side.changes)
            && thread.as_deref() == Some(view.id.as_str())
            && self
                .side
                .read_blocks
                .as_ref()
                .is_none_or(|(id, len, more, _)| id != &view.id || *len != view.blocks.len() || *more != view.more)
        {
            let (ends, edits) = crate::side::changes::transcript_marks(&view.blocks, view.more);
            let edited = self
                .side
                .read_blocks
                .as_ref()
                .is_some_and(|(id, _, _, seen)| id == &view.id && *seen != edits);
            self.side.read_blocks = Some((view.id.clone(), view.blocks.len(), view.more, edits));
            changes.update(cx, |changes, cx| {
                changes.set_turn_ends(ends, cx);
                if edited {
                    changes.agent_edited(cx);
                }
            });
        }
        let running = thread.is_some()
            && self
                .code_session()
                .is_some_and(|session| session.status == Status::Running);
        if self.side.was_running && !running {
            self.refresh_side(cx);
        }
        self.side.was_running = running;
    }

    fn refresh_side(&mut self, cx: &mut Context<Self>) {
        if let Some(changes) = &self.side.changes {
            changes.update(cx, |changes, cx| changes.refresh(cx));
        }
        if let Some(files) = &self.side.files {
            files.update(cx, |files, cx| files.refresh(cx));
        }
    }

    fn on_changes_event(
        &mut self,
        changes: &Entity<Changes>,
        event: &ChangesEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let thread = match self.side.scope.clone() {
            Some(Scope::Session(thread)) => Some(thread),
            _ => None,
        };
        match event {
            ChangesEvent::Fix(key, fix) => self.run_setup_fix(key.clone(), fix.clone(), window, cx),
            ChangesEvent::Commit(message) => {
                let Some((thread, runs)) = thread.and_then(|thread| Some((thread.clone(), self.runs(&thread)?))) else {
                    return;
                };
                let answer = runs.commit(thread, message.clone());
                changes.update(cx, |changes, cx| changes.committed(answer, window, cx));
            }
            ChangesEvent::WriteIt => {
                let work =
                    thread.and_then(|thread| self.write_draft(&thread, None, nexus_workbench::draft::Kind::Commit));
                changes.update(cx, |changes, cx| changes.drafted(work, window, cx));
            }
            ChangesEvent::Comment(comment) => {
                let Some(thread) = thread else { return };
                self.diff_comments.entry(thread).or_default().push(comment.clone());
                self.save_diff_comments();
                cx.notify();
            }
            ChangesEvent::Touched => {
                if thread.is_some() {
                    self.manual_workbench();
                }
            }
        }
    }

    fn ensure_shared_terminal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SharedTerminals>> {
        let View::Code(view) = &self.view else {
            return None;
        };
        if view.is_draft() {
            return None;
        }
        let id = view.id.clone();
        if let Some(terminal) = self.side.shared.get(&id) {
            return terminal.read(cx).is_current().then(|| terminal.clone());
        }
        if let Some(machine) = self.side.server.clone() {
            return self.ensure_server_commands(machine, id, window, cx);
        }
        let key = self.commands_key(&id)?;
        if let Some(terminal) = self.restore_returned_commands(&key, window, cx) {
            return Some(terminal);
        }
        if super::commands_detach::reserved(&key, window.window_handle().window_id(), cx) {
            self.side.shared_keys.insert(id, key);
            return None;
        }
        let (key, runner, guard) = self
            .commands_attached_source(&id, window.window_handle().window_id())
            .ok()?;
        let terminal = cx.new(|cx| SharedTerminals::new_guarded(runner, id.clone(), guard, window, cx));
        self.rebind_shared_terminal(&terminal, id.clone(), window, cx);
        self.side.shared_keys.insert(id.clone(), key);
        self.side.shared.insert(id, terminal.clone());
        Some(terminal)
    }

    /// The Commands of session `id` on server `machine`, when its host shares its sessions' commands
    /// (`shared-terminal` in its hello). They stay in this window: a server's session has no local owner to move with.
    fn ensure_server_commands(
        &mut self,
        machine: String,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SharedTerminals>> {
        let machines = crate::machines::get();
        if !machines.has_feature(&machine, nexus_workbench::terminal::FEATURE) {
            return None;
        }
        let runner = self.runner.clone()?;
        let proof = self.gateway_account_authority.clone()?;
        let guard: crate::side::shared_terminals::Guard = std::sync::Arc::new(move || proof.current());
        let terminal =
            cx.new(|cx| SharedTerminals::new_server(runner, machines, machine, id.clone(), guard, window, cx));
        self.rebind_shared_terminal(&terminal, id.clone(), window, cx);
        self.side.shared.insert(id, terminal.clone());
        Some(terminal)
    }

    fn rebind_shared_terminal(
        &mut self,
        terminal: &Entity<SharedTerminals>,
        thread: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(terminal, window, move |this, terminal, event, window, cx| {
            if !terminal.read(cx).is_current() {
                return;
            }
            use super::notice_host::NoticeDo;
            use crate::side::shared_terminals::SharedTerminalsEvent;
            match event {
                SharedTerminalsEvent::Deleted(ids) => {
                    this.show_notice(
                        format!("{} command(s) deleted", ids.len()),
                        Some(("Undo", NoticeDo::RestoreCommands(terminal.clone(), ids.clone()))),
                        nexus_desktop_core::notice::UNDO,
                        cx,
                    );
                    return;
                }
                SharedTerminalsEvent::RestoreFailed(ids, error) => {
                    this.show_notice(
                        format!("Could not restore commands: {error}"),
                        Some(("Retry", NoticeDo::RestoreCommands(terminal.clone(), ids.clone()))),
                        nexus_desktop_core::notice::NOTICE,
                        cx,
                    );
                    return;
                }
                // The tray above the composer mounts only while it has something to show: draw again to add it or
                // drop it.
                SharedTerminalsEvent::Activity => {
                    cx.notify();
                    return;
                }
                // A setup fix's command ended (`setup_fix.rs`).
                SharedTerminalsEvent::FixEnded {
                    token,
                    ok,
                    code,
                    last_line,
                } => {
                    if let Some(key) = this.setup.tokens.remove(token) {
                        this.fix_step_ended(key, *ok, *code, last_line.clone(), window, cx);
                    }
                    return;
                }
                SharedTerminalsEvent::OpenCommands => {}
            }
            if !matches!(&this.view, View::Code(view) if view.id == thread) {
                return;
            }
            this.pick_tool(Tool::Terminal, window, cx);
            terminal.update(cx, |terminal, cx| {
                terminal.allow_history(true, cx);
                terminal.show_pending(cx);
            });
        })
        .detach();
    }

    /// The folder's terminals to show, with one terminal when there are none yet; None for a server's session, which
    /// has none. A draft's Commands opens on its list, as a started session's does, not on a new shell.
    fn ensure_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<TerminalSet>> {
        if self.side.draft_commands() {
            return self.side.terminal().cloned();
        }
        self.ensure_shell_set(window, cx)
    }

    /// The folder's terminals, made when there are none yet, for a new shell; a server session's on its server, None
    /// when its host opens no shells.
    fn ensure_shell_set(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<TerminalSet>> {
        if self.side.server.is_some() {
            return self.ensure_server_terminal(window, cx);
        }
        if self.side.has_commands() {
            return None;
        }
        Some(self.ensure_local_terminal(window, cx))
    }

    fn ensure_local_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalSet> {
        if let Some(set) = self.side.terminal() {
            return set.clone();
        }
        let dir = self.side.dir.clone();
        let terminal_factory = self.side.terminal_factory;
        let commands = self.side.draft_commands();
        let set = cx.new(|cx| {
            let mut set = terminal_factory(dir.clone(), cx);
            set.set_commands_look(commands, cx);
            set
        });
        let subscription = cx.subscribe_in(&set, window, Self::on_terminal_set_event);
        self.side.terminals.insert(dir, (set.clone(), subscription));
        set
    }

    /// The last terminal of a folder closed: its tab goes.
    fn on_terminal_set_event(
        &mut self,
        set: &Entity<TerminalSet>,
        event: &TerminalSetEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let TerminalSetEvent::Empty = event;
        let dir = set.read(cx).dir().clone();
        let shown = dir == self.side.terminal_key();
        self.side.terminals.remove(&dir);
        // A draft's Commands tab stays, on its empty list, as a started session's does.
        if shown && self.side.draft_commands() {
            if self.side.tool == Tool::Terminal {
                self.focus_composer(window, cx);
            }
            cx.notify();
            return;
        }
        if shown {
            self.side.forget_tab(Tool::Terminal);
        }
        if shown && self.side.tool == Tool::Terminal {
            match self.side.fallback() {
                Some(tool) => self.side.tool = tool,
                None => {
                    self.side.open = false;
                    self.side.maximized = false;
                    self.focus_composer(window, cx);
                }
            }
        }
        cx.notify();
    }

    /// The tool the Workbench shows.
    pub(super) fn side_tool(&self) -> Tool {
        self.side.tool
    }

    pub(super) fn close_side_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.manual_workbench();
        self.side.open = false;
        self.side.maximized = false;
        self.side.plus_menu = false;
        self.composer.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub(super) fn toggle_side_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_side(window, cx);
        self.manual_workbench();
        // Hiding keeps the tools: a terminal's shell and a page go on, and come back as they were.
        self.side.open = !self.side.open;
        self.side.maximized = false;
        self.side.plus_menu = false;
        self.sync_side(window, cx);
        if self.side.open {
            self.focus_side_tool(window, cx);
        } else {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    fn focus_side_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(set) = self.side.terminal().cloned() {
            set.update(cx, |set, cx| set.focus(window, cx));
        }
    }

    /// The Workbench's tool on screen takes the keys as it shows: the terminal, the Agents list, a blank
    /// Browser tab's address bar, or the Files filter. Changes has no field to type in, so focus stays where it was.
    pub(super) fn focus_side_tool(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.side.tool {
            Tool::Terminal => self.focus_side_terminal(window, cx),
            Tool::Agents => {
                if let Some(state) = &self.side.agents {
                    state.focus.focus(window, cx);
                }
            }
            Tool::Browser => {
                if let Some(browser) = self.side.browser().cloned() {
                    browser.update(cx, |browser, cx| browser.focus_if_blank(window, cx));
                }
            }
            Tool::Files => {
                if let Some(files) = self.side.files.clone() {
                    files.update(cx, |files, cx| files.focus_filter(window, cx));
                }
            }
            _ => {}
        }
    }

    /// Show `tool`, opening it (and the Workbench) when it is not yet.
    /// `NEXUS_OPEN_PAGE=browser?url=<a>&url=<b>`: the Browser tool, with a tab per address, for checking it.
    pub(super) fn open_side_browser(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.show_tool(Tool::Browser, false, window, cx);
        let Some(browser) = self.side.browser().cloned() else {
            return;
        };
        let query = path.split_once('?').map_or("", |(_, query)| query);
        for (key, url) in url::form_urlencoded::parse(query.as_bytes()) {
            if key == "url" {
                browser.update(cx, |browser, cx| {
                    browser.open_new(&url, window, cx);
                });
            }
        }
    }

    /// Show `url` (a web address) in the Browser tab of the scope on screen, opening it and the Workbench: a cloud
    /// machine's port forwarded to this computer (the Cloud panel's Preview).
    pub(super) fn show_in_side_browser(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.show_tool(Tool::Browser, false, window, cx);
        if let Some(browser) = self.side.browser().cloned() {
            browser.update(cx, |browser, cx| browser.open(url, window, cx));
        }
    }

    /// The person picked `tool`: it shows, and its main field takes the keys.
    pub(super) fn pick_tool(&mut self, tool: Tool, window: &mut Window, cx: &mut Context<Self>) {
        if tool == Tool::Artifacts && self.is_local() {
            return;
        }
        self.show_tool(tool, true, window, cx);
    }

    /// Show `tool`. `focus` is false when the caller loads something into it next, such as an address the Browser
    /// opens: a focused address bar would hold the typed text over the page's address.
    fn show_tool(&mut self, tool: Tool, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.manual_workbench();
        // First, so tools for another screen have started over.
        self.sync_side(window, cx);
        // Files also opens without a folder when its pane shows an error, or when this screen's files are in a
        // window of their own (`files_detach`), so it can say so or bring that window back.
        let files_elsewhere = matches!(tool, Tool::Files)
            && !self.side.home.home
            && (self.side.files_error.is_some()
                || self
                    .selected_files_key()
                    .is_some_and(|key| super::files_detach::reserved(&key, window.window_handle().window_id(), cx)));
        if !self.can_pick_tool(tool) && !files_elsewhere {
            return;
        }
        self.side.open = true;
        self.side.plus_menu = false;
        self.side.side_chat_visible = false;
        self.side.connection = None;

        self.side.select_tool(tool);
        if self.side.home.home {
            self.side.session_tool = tool;
        }
        match tool {
            Tool::Terminal => {
                self.ensure_terminal(window, cx);
            }
            Tool::Browser if self.side.browser().is_none() => {
                let url = match &self.view {
                    View::Code(view) => crate::side::latest_local_url(&view.blocks),
                    _ => None,
                };
                let key = self.side.browser_key();
                if self.side.parked_browsers.iter().any(|saved| saved.session == key) {
                    self.composer_note = Some("This browser is waiting for its original local source.".into());
                } else {
                    let _ = self.make_browser(key, url, window, cx);
                }
            }
            _ => {}
        }
        self.sync_side(window, cx);
        if focus {
            self.focus_side_tool(window, cx);
        }
        cx.notify();
    }

    /// Show the Changes tab on `source`, scrolled to file `path` once read (a chip at the top of an answer).
    pub(super) fn focus_changes(
        &mut self,
        source: nexus_workbench::changes_source::ChangesSource,
        path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pick_tool(Tool::Changes, window, cx);
        if let Some(changes) = &self.side.changes {
            changes.update(cx, |changes, cx| changes.focus(source, path, cx));
        }
    }

    pub(super) fn close_tool(&mut self, tool: Tool, window: &mut Window, cx: &mut Context<Self>) {
        self.manual_workbench();
        match tool {
            Tool::Terminal => {
                let key = self.side.terminal_key();
                self.side.terminals.remove(&key);
            }
            Tool::Browser => {
                let key = self.side.browser_key();
                self.side.browser_events.remove(&key);
                if let Some(browser) = self.side.browsers.remove(&key) {
                    browser.update(cx, |browser, cx| browser.close_popups(None, cx));
                }
            }
            Tool::Artifacts => self.page = None,
            Tool::Changes | Tool::Files | Tool::Agents => return,
        }
        self.side.forget_tab(tool);
        if self.side.tool == tool {
            match self.side.fallback() {
                Some(next) => self.side.tool = next,
                None => {
                    self.side.open = false;
                    self.side.maximized = false;
                    self.focus_composer(window, cx);
                }
            }
        }
        cx.notify();
    }

    /// A new terminal in the folder's set, shown and focused.
    pub(super) fn new_side_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_manual_workbench();
        self.sync_side(window, cx);
        if let Some(shared) = self.ensure_shared_terminal(window, cx) {
            self.side.open = true;
            self.side.tool = Tool::Terminal;
            shared.update(cx, |view, cx| view.open_shell(cx));
            cx.notify();
            return;
        }
        if self.route_commands_action(window, cx, |commands, cx| commands.open_shell(cx)) {
            return;
        }
        let fresh = self.side.terminal().is_none();
        self.side.open = true;
        self.side.tool = Tool::Terminal;
        let Some(set) = self.ensure_shell_set(window, cx) else {
            cx.notify();
            return;
        };
        if !fresh {
            set.update(cx, |set, cx| set.add(cx));
        }
        set.update(cx, |set, cx| set.focus(window, cx));
        cx.notify();
    }

    /// The server the side pane's shells run on, or None on this computer.
    pub(super) fn side_server(&self) -> Option<&str> {
        self.side.server.as_deref()
    }

    /// Run `command` in a new terminal of the folder's set, named `name` or after the command, shown and focused: Run on
    /// a shell block (`code_block.rs`) and a project command, as the Nexus app's `runInTerminal`. The first terminal of
    /// a folder runs it itself.
    pub(super) fn run_in_side_terminal(
        &mut self,
        command: &str,
        name: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_side(window, cx);
        if let Some(view) = self.ensure_shared_terminal(window, cx) {
            let command = command.to_string();
            let name = name.map(str::to_string);
            self.side.open = true;
            self.side.tool = Tool::Terminal;
            view.update(cx, |view, cx| view.run(command, name, cx));
            cx.notify();
            return;
        }
        let detached_command = command.to_string();
        let detached_name = name.map(str::to_string);
        if self.route_commands_action(window, cx, move |commands, cx| {
            commands.run(detached_command, detached_name, cx)
        }) {
            return;
        }
        let fresh = self.side.terminal().is_none();
        let Some(set) = self.ensure_shell_set(window, cx) else {
            return;
        };
        self.side.open = true;
        self.side.tool = Tool::Terminal;
        set.update(cx, |set, cx| {
            set.run(command, name, !fresh, cx);
            set.focus(window, cx);
        });
        cx.notify();
    }

    /// The Workbench over the whole content area, or back beside the session.
    pub(super) fn toggle_maximize_side(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.side.tool == Tool::Agents {
            return;
        }
        if !self.side.open {
            self.side.open = true;
            self.side.maximized = true;
            self.sync_side(window, cx);
        } else {
            self.side.maximized = !self.side.maximized;
        }
        self.side.plus_menu = false;
        if self.side.maximized {
            // The composer is hidden: the keys go to the Workbench.
            if self.side.tool == Tool::Terminal {
                self.focus_side_terminal(window, cx);
            } else {
                window.focus(&self.focus, cx);
            }
        } else if !self.side_terminal_focused(window, cx) {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    /// A palette command (`search.rs`).
    pub(super) fn run_workbench(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::Toggle => self.toggle_side_pane(window, cx),
            Command::NewTerminal => self.new_side_terminal(window, cx),
            Command::Browser => self.pick_tool(Tool::Browser, window, cx),
            Command::Changes => self.pick_tool(Tool::Changes, window, cx),
            Command::Files => self.pick_tool(Tool::Files, window, cx),
            Command::Maximize => self.toggle_maximize_side(window, cx),
        }
    }

    /// Whether the screen has no Workbench: Settings and Team sessions are not about a session or a folder, so the
    /// pane, its button and its keys stay away there.
    pub(super) fn workbench_hidden(&self) -> bool {
        matches!(self.view, View::TeamSessions | View::Settings(_))
    }

    /// Settings opened: the Workbench closes, and stays closed when the person leaves.
    pub(super) fn close_workbench_for_settings(&mut self) {
        self.side.open = false;
        self.side.maximized = false;
        self.side.plus_menu = false;
    }

    pub(super) fn on_toggle_side_pane(&mut self, _: &ToggleSidePane, window: &mut Window, cx: &mut Context<Self>) {
        if self.workbench_hidden() {
            return;
        }
        self.toggle_side_pane(window, cx);
    }

    /// Ctrl+`: to the terminal, opening the Workbench on it; from the terminal, the Workbench hides.
    pub(super) fn on_toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if self.workbench_hidden() {
            return;
        }
        if self.side.tool == Tool::Terminal && self.side_terminal_focused(window, cx) {
            self.side.open = false;
            self.side.maximized = false;
            self.focus_composer(window, cx);
            cx.notify();
        } else {
            self.pick_tool(Tool::Terminal, window, cx);
        }
    }

    pub(super) fn on_new_terminal(&mut self, _: &NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if self.workbench_hidden() {
            return;
        }
        self.new_side_terminal(window, cx);
    }

    pub(super) fn on_toggle_maximize_workbench(
        &mut self,
        _: &ToggleMaximizeWorkbench,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workbench_hidden() {
            return;
        }
        self.toggle_maximize_side(window, cx);
    }

    /// Ctrl+Alt+1 to 6: the Workbench's tool, on a screen that has a Workbench.
    fn pick_tool_key(&mut self, tool: Tool, window: &mut Window, cx: &mut Context<Self>) {
        if self.workbench_hidden() {
            return;
        }
        self.pick_tool(tool, window, cx);
    }

    pub(super) fn chat_browser_shown(&self) -> bool {
        self.side.open && self.side.tool == Tool::Browser
    }

    pub(super) fn toggle_chat_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.view, View::Chat) {
            return;
        }
        if self.chat_browser_shown() {
            self.side.open = false;
            self.side.maximized = false;
            self.focus_composer(window, cx);
            cx.notify();
        } else {
            self.pick_tool(Tool::Browser, window, cx);
        }
    }

    pub(super) fn on_toggle_chat_browser(
        &mut self,
        _: &ToggleChatBrowser,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_chat_browser(window, cx);
    }

    pub(super) fn on_side_tool_1(&mut self, _: &SideTool1, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_tool_key(Tool::ALL[0], window, cx);
    }
    pub(super) fn on_side_tool_2(&mut self, _: &SideTool2, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_tool_key(Tool::ALL[1], window, cx);
    }
    pub(super) fn on_side_tool_3(&mut self, _: &SideTool3, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_tool_key(Tool::ALL[2], window, cx);
    }
    pub(super) fn on_side_tool_4(&mut self, _: &SideTool4, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_tool_key(Tool::ALL[3], window, cx);
    }
    pub(super) fn on_side_tool_5(&mut self, _: &SideTool5, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_tool_key(Tool::ALL[4], window, cx);
    }
    pub(super) fn on_side_tool_6(&mut self, _: &SideTool6, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_tool_key(Tool::Agents, window, cx);
    }

    /// The pane's width in this window: as the person left it, between its least and the most the session spares.
    fn side_maximum(&self, window: &Window) -> f32 {
        let width = f32::from(window.viewport_size().width);
        let room = width - if self.sidebar_open { self.sidebar_room() } else { 0. };
        nexus_desktop_core::pane_resize::maximum(room, width)
    }

    pub(super) fn side_width(&self, window: &Window) -> f32 {
        nexus_desktop_core::pane_resize::clamp(self.side.width, self.side_maximum(window))
    }

    fn set_side_width(&mut self, width: f32, window: &Window, cx: &mut Context<Self>) {
        self.side.width = nexus_desktop_core::pane_resize::clamp(width, self.side_maximum(window));
        let saved = self.side.width;
        self.side._save = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            crate::paths::save_side_pane_width(saved);
        }));
        cx.notify();
    }

    fn resize_side(&mut self, x: Pixels, window: &Window, cx: &mut Context<Self>) {
        let width = f32::from(window.viewport_size().width) - f32::from(x);
        self.set_side_width(width, window, cx);
    }

    /// The Workbench's motion this frame: whether it shows, and how it moves. The workspace reads it before it lays
    /// out, so a maximized pane that grows, shrinks or leaves keeps the session beside it until it settles.
    pub(super) fn sync_side_motion(&self, cx: &mut Context<Self>) -> Option<super::pane_motion::Frame> {
        let kind = if self.side.maximized {
            super::pane_motion::Kind::Expanded
        } else if self.side.layout.drawer {
            super::pane_motion::Kind::Drawer
        } else {
            super::pane_motion::Kind::Docked
        };
        let mut motion = self.side.motion.get();
        let frame = motion.update(self.side.open, kind, std::time::Instant::now(), cx.reduce_motion());
        self.side.motion.set(motion);
        let Some(frame) = frame else {
            self.side.motion_task.borrow_mut().take();
            self.side.shown_tab.set(None);
            return None;
        };
        let previous_epoch = self.side.motion_task.borrow().as_ref().map(|(epoch, _)| *epoch);
        match frame.wake_epoch() {
            Some(epoch) if previous_epoch != Some(epoch) => {
                let task = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(crate::motion::PANEL).await;
                    let _ = this.update(cx, |this, cx| {
                        let mut motion = this.side.motion.get();
                        motion.finish(epoch);
                        this.side.motion.set(motion);
                        cx.notify();
                    });
                });
                *self.side.motion_task.borrow_mut() = Some((epoch, task));
            }
            None => {
                self.side.motion_task.borrow_mut().take();
            }
            _ => {}
        }
        Some(frame)
    }

    /// The Workbench, while it is open or leaving: beside the session, or over the whole content area when maximized.
    /// Returns the pane, and, when it is docked or maximized, its head for the window's title bar: there its tab strip
    /// is the Workbench's column head, on one line with the session's, as in Cursor (`Frame::head_in_title`). A drawer
    /// keeps its head, over the session.
    pub(super) fn render_side_pane(
        &self,
        frame: Option<super::pane_motion::Frame>,
        palette: Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<(AnyElement, Option<AnyElement>)> {
        use super::pane_motion::Kind;
        let frame = frame?;
        let in_title = frame.head_in_title();
        let maximized = self.side.maximized;
        // The layout comes from the motion, which keeps a leaving pane as it was.
        let settled_full = frame.expanded_at_rest();
        let growing = frame.growing();
        let rest_kind = if frame.kind == Kind::Expanded {
            frame.rest
        } else {
            frame.kind
        };
        let drawer = rest_kind == Kind::Drawer;
        let drawer_layout = frame.kind == Kind::Drawer && !growing;
        let full = f32::from(window.viewport_size().width) - if self.sidebar_open { self.sidebar_room() } else { 0. };
        let width = if drawer {
            let room =
                f32::from(window.viewport_size().width) - if self.sidebar_open { self.sidebar_room() } else { 0. };
            let content = match self.side.tool {
                Tool::Terminal => nexus_desktop_core::pane_layout::Content::Terminal,
                Tool::Artifacts => nexus_desktop_core::pane_layout::Content::Artifact,
                _ => nexus_desktop_core::pane_layout::Content::Default,
            };
            nexus_desktop_core::pane_layout::drawer_width(room, content)
        } else {
            self.side_width(window)
        };
        let side_chat = self.side.side_chat_visible.then(|| self.open_side_chat()).flatten();
        let in_side_chat = side_chat.is_some() || self.side.side_chat_visible && self.waiting_side_chat().is_some();
        let tools = if self.side.home.home {
            vec![Tool::Browser, Tool::Terminal]
        } else {
            Tool::ALL.to_vec()
        };
        // A tool is a tab while it has something open, with or without a session: with none, a closed shell or
        // browser takes its tab with it.
        let tabs: Vec<Tool> = tools
            .into_iter()
            .filter(|tool| match tool {
                Tool::Changes => self.side.has_folder(),
                Tool::Files => {
                    self.side.has_folder()
                        || self.side.files_error.is_some()
                        || self
                            .selected_files_key()
                            .is_some_and(|key| super::files_detach::owner(&key, cx).is_some())
                }
                Tool::Terminal => self.side.has_terminal(),
                Tool::Browser => {
                    self.side.browser().is_some()
                        || self
                            .browser_detach_key(&self.side.browser_key())
                            .is_some_and(|key| super::browser_detach::is_detached(&key, cx))
                }
                Tool::Artifacts => self.account_gateway().is_some() && self.page.is_some(),
                Tool::Agents => matches!(self.view, View::Code(_)),
            })
            .collect();
        let ordered = self.side.ordered_tabs(
            tabs.iter()
                .copied()
                .map(Tab::Tool)
                .chain((self.open_side_chat().is_some() || self.waiting_side_chat().is_some()).then_some(Tab::SideChat))
                .collect(),
        );
        // No tab: the pane lists what it can open (`side_pane/empty.rs`), so "+" would repeat it.
        let empty = ordered.is_empty() && !in_side_chat;
        let offered = if empty {
            Vec::new()
        } else {
            entries::offered(self.workbench_entries(), &ordered)
        };
        // Too narrow for every tab's name: the tabs not chosen show their icon only, named by their tooltip, rather
        // than run under the buttons on the right. A named tab is its padding and icon (36px), about 7px a letter at
        // 12.5px, and 20px more for a ×; the buttons are 28px, all 4px apart, with the header's 10px of padding.
        let pane_width = if frame.kind == Kind::Expanded { full } else { width };
        let plus = !offered.is_empty();
        if !plus {
            // "+" went with its last entry: its menu is closed, so a later "+" does not draw it leaving.
            crate::motion::presence("side-plus-menu", false, cx);
        }
        // Files, Commands and a session's Browser can move to their own window: their button sits with the pane's.
        let move_window = if in_side_chat || empty {
            None
        } else {
            match self.side.tool {
                Tool::Files => self.files_move_button(palette, cx),
                Tool::Terminal if self.side.server.is_none() && self.side.has_commands() => {
                    self.commands_move_button(palette, cx)
                }
                Tool::Browser => self.browser_move_button(palette, cx),
                _ => None,
            }
        };
        // Keep the close button at the Workbench's far right, including when its head sits in the title bar.
        // The session head shows the open button only while the Workbench is closed.
        let close = true;
        let window_controls = if in_title && !cfg!(target_os = "macos") {
            102.
        } else {
            0.
        };
        // The page's header has its own Full view, the same state as maximizing on Artifacts: one control, not two.
        let maximize = !empty
            && match self.side.tool {
                Tool::Agents => false,
                Tool::Artifacts => in_side_chat || self.page.is_none(),
                _ => true,
            };
        // Following the agent: a small button among the pane's (`navigation.rs`), not a row over it.
        let follow = if in_side_chat {
            None
        } else {
            self.render_navigation(palette, cx)
        };
        let buttons = usize::from(maximize)
            + usize::from(close)
            + usize::from(plus)
            + usize::from(move_window.is_some())
            + usize::from(follow.is_some());
        let named: f32 = tabs
            .iter()
            .map(|tool| {
                let closable = matches!(tool, Tool::Terminal | Tool::Browser | Tool::Artifacts)
                    && !(*tool == Tool::Terminal && self.side.shows_commands());
                36. + 7. * tool.label().len() as f32 + if closable { 20. } else { 0. } + 4.
            })
            .sum();
        let compact = named
            + if self.open_side_chat().is_some() || self.waiting_side_chat().is_some() {
                150.
            } else {
                0.
            }
            + buttons as f32 * 32.
            + window_controls
            + 10.
            + 16.
            > pane_width;
        // The first tab's icon sits 12px in (`side::INSET`), as the tools' content below it.
        let header = h_flex()
            .flex_none()
            .w_full()
            .when(in_title, |header| header.h_full())
            .when(!in_title, |header| header.h(px(36.)))
            .pl(px(crate::side::INSET - 8.))
            .pr(px(6.))
            .gap(px(6.))
            // A narrow pane scrolls its tabs rather than push the buttons on the right out of it.
            .child(
                h_flex()
                    .id("side-tabs")
                    .min_w(px(0.))
                    .overflow_x_scroll()
                    .gap(px(4.))
                    // In the title bar a press on a tab is the tab's: it must not start the window move, which would
                    // take the release and the click with it (as `bar_button`). The space beside the tabs still moves
                    // the window.
                    .when(in_title, |tabs| {
                        tabs.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    })
                    .children(self.render_side_strip(&ordered, compact, palette, cx)),
            )
            .when(plus, |header| {
                header.child(div().flex_none().child(self.render_side_plus(offered, palette, cx)))
            })
            .child(div().flex_1())
            .children(follow)
            .children(move_window)
            .when(maximize, |header| {
                header.child(
                    bar_button(
                        "side-maximize",
                        if maximized {
                            Lucide::Minimize2
                        } else {
                            Lucide::Maximize2
                        },
                        palette,
                    )
                    .flex_none()
                    .debug_selector(|| "side-maximize".into())
                    .tooltip(move |window, cx| {
                        crate::key_bindings::tooltip(
                            if maximized { "Collapse (Esc)" } else { "Expand" },
                            "expand-workbench",
                        )
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_maximize_side(window, cx))),
                )
            })
            .when(close, |header| {
                header.child(
                    bar_button("side-close", Lucide::X, palette)
                        .flex_none()
                        .debug_selector(|| "side-close".into())
                        .tooltip(|window, cx| {
                            crate::key_bindings::tooltip("Hide the Workbench", "toggle-side-pane").build(window, cx)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.close_side_drawer(window, cx)),
                        ),
                )
            });
        let body: AnyElement = if let Some(chat) = side_chat {
            chat.into_any_element()
        } else if self.side.side_chat_visible && self.waiting_side_chat().is_some() {
            let (title, draft, files) = self.waiting_side_chat().unwrap().preview();
            v_flex()
                .gap(px(12.))
                .p(px(16.))
                .child(crate::pages::ui::section_head(title, palette))
                .child(crate::side::note(
                    "Reconnect to this session to resume. Your draft is kept.",
                    palette,
                ))
                .child(div().child(draft))
                .children(files.into_iter().map(|name| div().child(name)))
                .into_any_element()
        } else if empty {
            self.render_workbench_empty(palette, cx)
        } else {
            match self.side.tool {
                Tool::Changes => self.side.changes.clone().map(|view| view.into_any_element()),
                Tool::Files => Some(self.render_files_body(palette, cx)),
                // A server's session shows its Commands when its host shares them, else its own shell or a note.
                Tool::Terminal if self.side.has_commands() => Some(self.render_commands_body(palette, cx)),
                Tool::Terminal if self.side.server.is_some() && self.side.terminal().is_none() => {
                    Some(crate::side::note(crate::side::server::NO_TERMINAL, palette).into_any_element())
                }
                Tool::Terminal if self.side.draft_commands() && self.side.terminal().is_none() => {
                    let workspace = cx.entity().downgrade();
                    Some(crate::side::terminals::render_empty_commands(
                        std::rc::Rc::new(move |window, cx| {
                            if let Some(workspace) = workspace.upgrade() {
                                workspace.update(cx, |this, cx| this.new_side_terminal(window, cx));
                            }
                        }),
                        palette,
                    ))
                }
                Tool::Terminal => self.side.terminal().cloned().map(|view| view.into_any_element()),
                Tool::Browser => Some(self.render_browser_body(palette, cx)),
                Tool::Artifacts => {
                    self.render_page(palette, 44., pane_width < super::page_header::SHARE_LABEL_MIN_WIDTH, cx)
                }
                Tool::Agents => Some(self.render_agents_tool(palette, pane_width, cx)),
            }
            .unwrap_or_else(|| div().into_any_element())
        };
        // Connection details and side chats each replace the tools.

        let connection_header = self.render_connection_header(palette, cx);
        let connection_body = self.render_connection_body(palette, cx);

        let elsewhere =
            matches!(self.side_scope(), Scope::Home) && (self.cloud_ui.on || self.code_form.project.is_some());
        let body = if self.side.home.home && self.side.tool == Tool::Terminal && elsewhere && !empty {
            v_flex()
                .size_full()
                .child(crate::side::note(
                    "The session runs on another machine. These shells run on this computer, in your home folder.",
                    palette,
                ))
                .child(div().flex_1().min_h(px(0.)).child(body))
                .into_any_element()
        } else {
            body
        };
        let head = connection_header.unwrap_or_else(|| header.into_any_element());
        let (pane_head, title_head) = if in_title {
            (None, Some(head))
        } else {
            (Some(head), None)
        };
        // The left edge, 6px wide over the border: dragged, it sets the width.
        let edge = div()
            .id("side-resize")
            .absolute()
            .top_0()
            .left(px(-3.))
            .w(px(6.))
            .h_full()
            .cursor_col_resize()
            .when_some(self.side.resize_focus.as_ref(), |edge, focus| {
                edge.track_focus(focus).tab_index(0)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if let Some(focus) = &this.side.resize_focus {
                        window.focus(focus, cx);
                    }
                    if event.click_count == 2 {
                        this.side.width = DEFAULT_WIDTH;
                        this.side._save = Some(cx.spawn(async move |_, cx| {
                            cx.background_executor().timer(Duration::from_millis(400)).await;
                            crate::paths::save_side_pane_width(DEFAULT_WIDTH);
                        }));
                        cx.notify();
                        cx.stop_propagation();
                    }
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if let Some(width) = nexus_desktop_core::pane_resize::key_width(
                    this.side_width(window),
                    this.side_maximum(window),
                    &event.keystroke.key,
                ) {
                    this.set_side_width(width, window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_drag(Resize, |_, _, _, cx| cx.new(|_| Resize));
        let pane = v_flex()
            .id("side-pane")
            .relative()
            .when(drawer_layout, |pane| {
                pane.absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        if this.side.open {
                            this.close_side_drawer(window, cx);
                        }
                    }))
            })
            // At rest maximized, or growing and shrinking: the pane fills its frame. Otherwise it keeps its width, also
            // while it slides (at the whole area's width when it leaves maximized).
            .when(settled_full || growing, |pane| pane.flex_1().min_w(px(0.)))
            .when(!settled_full && !growing, |pane| {
                pane.w(px(frame.slide_width(width, full).unwrap_or(width))).flex_none()
            })
            .h_full()
            // A pane on the page's fill, set off by one hairline on its left edge (below).
            .bg(palette.background)
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<Resize>, window, cx| {
                if this.side.open {
                    this.resize_side(event.event.position.x, window, cx);
                }
            }))
            .children(pane_head)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .capture_any_mouse_down(cx.listener(|this, _, _, _| this.manual_workbench()))
                    .on_scroll(cx.listener(|this, _, _, _| this.manual_workbench()))
                    .capture_key_down(cx.listener(|this, _, _, _| this.manual_workbench()))
                    .child(self.side_body_switch(connection_body.unwrap_or(body), in_side_chat, frame.moving)),
            )
            // One hairline on its left edge, except where it fills the whole area at rest.
            .when(!settled_full, |pane| pane.child(super::hairline(palette).left_0()))
            .when(frame.kind == Kind::Docked && !growing, |pane| pane.child(edge))
            .into_any_element();
        // The head in the title bar keeps the pane's width and moves with it: it slides in and out, and grows and
        // shrinks, along the same curves. Its hairline runs on up from the pane's, so the divider meets the window's
        // top edge.
        let title_head = title_head.map(|head| {
            let slot = h_flex()
                .id("side-head")
                .relative()
                .h_full()
                // Maximized, the head takes the room the Workbench's toggle leaves, not the content area's width: a
                // fixed `full` ran past the bar by the toggle's width and clipped the restore button off its end.
                .when(settled_full || growing, |slot| slot.flex_1().min_w(px(0.)))
                .when(!settled_full && !growing, |slot| {
                    slot.w(px(frame.slide_width(width, full).unwrap_or(width))).flex_none()
                })
                .child(head)
                .when(!settled_full, |slot| slot.child(super::hairline(palette).left_0()))
                .into_any_element();
            // Too little room (maximized with the sidebar shut, or growing): the session's title gives way first, then
            // the head, rather than push the window's controls out of the bar.
            div()
                .flex()
                .h_full()
                .min_w(px(0.))
                .when(settled_full, |wrap| wrap.flex_1())
                .overflow_hidden()
                .child(super::pane_motion::render(slot, width, full, frame))
                .into_any_element()
        });
        Some((super::pane_motion::render(pane, width, full, frame), title_head))
    }

    /// The tool's content, fading in when another tab takes the pane (`motion::UI`), as a popover does. Only a switch
    /// fades: the pane opening, or another session's content under the same tab, shows at once.
    fn side_body_switch(&self, body: AnyElement, side_chat: bool, moving: bool) -> AnyElement {
        let key = if side_chat { usize::MAX } else { self.side.tool as usize };
        let switched = match self.side.shown_tab.get() {
            Some((shown, switched)) if shown == key => switched,
            Some(_) => !moving,
            None => false,
        };
        self.side.shown_tab.set(Some((key, switched)));
        div()
            .size_full()
            .child(body)
            .with_animation(
                ("side-body", key),
                Animation::new(crate::motion::UI).with_easing(crate::motion::ease_out),
                move |body, delta| if switched { body.opacity(delta) } else { body },
            )
            .into_any_element()
    }

    pub(in crate::workspace) fn restore_agents_after_side_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.side.tool = Tool::Agents;
        self.side.session_tool = Tool::Agents;
        if let Some(agents) = &self.side.agents {
            agents.focus.focus(window, cx);
        }
    }

    fn render_side_chat_tab(&self, compact: bool, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.session_account_gateway().is_none() {
            return None;
        }
        let Some(chat) = self.open_side_chat() else {
            self.waiting_side_chat()?;
            return Some(
                crate::pages::ui::button("side-chat-tab", "Side chat", p)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.side.side_chat_visible = true;
                        this.sync_side_chat(window, cx);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        };
        let selected = self.side.side_chat_visible;
        let icon_only = compact && !selected;
        Some(
            h_flex()
                .id("side-chat-tab")
                .group("side-chat-tab")
                .flex_none()
                .h(px(26.))
                .when(icon_only, |tab| tab.w(px(30.)).justify_center())
                .when(!icon_only, |tab| tab.pl(px(8.)).pr(px(4.)))
                .gap(px(6.))
                .rounded(px(7.))
                .text_size(px(12.5))
                .cursor_pointer()
                .when(selected, |tab| tab.bg(p.selected).text_color(p.foreground))
                .when(!selected, |tab| {
                    tab.text_color(p.muted)
                        .hover(|style| style.bg(p.hover).text_color(p.foreground))
                        .tooltip(|window, cx| Tooltip::new("Open side chat").build(window, cx))
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.side.side_chat_visible = true;
                    this.sync_side_chat(window, cx);
                    if let Some(chat) = this.open_side_chat() {
                        chat.update(cx, |chat, cx| chat.focus_input(window, cx));
                    }
                    cx.notify();
                }))
                .child(Icon::new(Lucide::MessageSquare).size(px(14.)))
                .when(!icon_only, |tab| {
                    tab.child("Side chat").child(
                        div()
                            .id("close-side-chat-tab")
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .when(!selected, |close| {
                                close.invisible().group_hover("side-chat-tab", |style| style.visible())
                            })
                            .hover(|style| style.bg(if selected { p.hover } else { p.selected }))
                            .tooltip(|window, cx| Tooltip::new("Close side chat").build(window, cx))
                            .on_click(cx.listener(move |_, _, window, cx| {
                                cx.stop_propagation();
                                chat.update(cx, |chat, cx| chat.close(window, cx));
                            }))
                            .child(Icon::new(Lucide::X).size(px(12.)).text_color(p.faint)),
                    )
                })
                .into_any_element(),
        )
    }

    /// Whether `tool`'s tab shows its dot. The Commands tab shows it while the session has a command that runs or waits;
    /// another tab, while the agent did something there the person has not seen.
    fn side_tab_dot(&self, tool: Tool, cx: &App) -> bool {
        if tool == Tool::Terminal
            && let Some(commands) = self.side.shared_terminal()
        {
            return commands.read(cx).has_active();
        }
        matches!(&self.view, View::Code(view) if self.side.navigation.get(&view.id).is_some_and(|state| {
            state.unread.iter().any(|target| target.tool() == tool)
        }))
    }

    /// A tool's tab: icon and name. The tools that open and close (Terminal, Browser, Artifacts) also have a ×, shown
    /// only while the tab is chosen or under the pointer, as Zed's and Linear's tabs, so at rest every tab looks alike;
    /// its slot is kept so the tab doesn't change width.
    /// `compact`: the pane is too narrow for every name, so this tab shows its icon only unless it is chosen.
    fn render_side_tab(&self, tool: Tool, compact: bool, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let chosen = self.side.tool == tool && !self.side.side_chat_visible;
        let icon_only = compact && !chosen;
        let closable = matches!(tool, Tool::Terminal | Tool::Browser | Tool::Artifacts)
            && !icon_only
            && !(tool == Tool::Terminal && self.side.shows_commands());
        let group: SharedString = format!("side-tab-{}", tool as usize).into();
        h_flex()
            .id(("side-tab", tool as usize))
            .debug_selector(move || format!("side-tab-{}", tool as usize))
            .group(group.clone())
            .flex_none()
            .h(px(26.))
            // Icon only, the tab still grows for its dot and the Agents count: at a fixed 30px they spilled onto the
            // next tab.
            .when(icon_only, |tab| tab.min_w(px(30.)).px(px(8.)).justify_center())
            .when(!icon_only, |tab| tab.pl(px(8.)).pr(px(if closable { 4. } else { 8. })))
            .gap(px(6.))
            .rounded(px(7.))
            .text_size(px(12.5))
            .cursor_pointer()
            .relative()
            // The fill fades under the pointer, as the sidebar's rows; the chosen tab is filled already.
            .when(!chosen, |tab| {
                tab.child(crate::motion::hover_fill("hover", palette.hover, 7.))
                    .text_color(palette.muted)
                    .hover(|style| style.text_color(palette.foreground))
            })
            .when(chosen, |tab| tab.text_color(palette.foreground))
            // Not on the chosen tab: its pane is already open, and the tooltip would sit over the first rows of it.
            .when(!chosen, |tab| {
                tab.tooltip(move |window, cx| {
                    crate::key_bindings::tooltip(tool.label(), tool.shortcut_id()).build(window, cx)
                })
            })
            .on_click(cx.listener(move |this, _, window, cx| this.pick_tool(tool, window, cx)))
            .child(Icon::new(tool.icon()).size(px(14.)))
            .when(!icon_only, |tab| {
                tab.child(if tool == Tool::Terminal && self.side.shows_commands() {
                    "Commands"
                } else {
                    tool.label()
                })
            })
            .when(self.side_tab_dot(tool, cx), |tab| {
                tab.child(div().size(px(5.)).rounded_full().bg(palette.muted))
            })
            .when(tool == Tool::Agents, |tab| tab.children(self.agents_tab_badge(palette)))
            .when(closable, |tab| {
                tab.child(
                    div()
                        .id(("side-tab-close", tool as usize))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .when(!chosen, |close| {
                            close.invisible().group_hover(group.clone(), |style| style.visible())
                        })
                        .hover(|style| style.bg(if chosen { palette.hover } else { palette.selected }))
                        .tooltip(move |window, cx| Tooltip::new(format!("Close {}", tool.label())).build(window, cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_tool(tool, window, cx);
                        }))
                        .child(Icon::new(Lucide::X).size(px(12.)).text_color(palette.faint)),
                )
            })
            // A tab that is chosen fills at once, under the click; the one it replaces lets its fill go over
            // `motion::HOVER`, so the choice reads as moving from one tab to the other.
            .with_tween(
                ("side-tab-fill", tool as usize),
                if chosen { 1. } else { 0. },
                crate::motion::Tween::new(crate::motion::HOVER, crate::motion::standard),
                move |tab, fill| {
                    let fill = if chosen { 1. } else { fill };
                    if fill > 0. {
                        tab.bg(palette.selected.opacity(fill))
                    } else {
                        tab
                    }
                },
            )
    }

    /// The strip's tabs, in their order. A tab that just joined grows in, and one that just closed narrows out where it
    /// was (`side_pane/tab_motion.rs`).
    fn render_side_strip(
        &self,
        ordered: &[Tab],
        compact: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let scope = self.side.browser_key();
        let still = cx.reduce_motion();
        let mut strip = Vec::with_capacity(ordered.len() + 1);
        for &tab in ordered {
            let (element, key, label) = match tab {
                Tab::Tool(tool) => (
                    Some(self.render_side_tab(tool, compact, palette, cx).into_any_element()),
                    tool as usize,
                    tool.label(),
                ),
                Tab::SideChat => (self.render_side_chat_tab(compact, palette, cx), usize::MAX, "Side chat"),
            };
            let Some(element) = element else {
                continue;
            };
            strip.push(match self.side.tab_motion.joining(&scope, tab).filter(|_| !still) {
                Some(since) => tab_motion::grow(element, key, label, since),
                None => element,
            });
        }
        if let Some(leaving) = self.side.tab_motion.leaving(&scope).filter(|_| !still) {
            strip.insert(leaving.index.min(strip.len()), tab_motion::shrink(&leaving, palette));
        }
        strip
    }

    /// "+": the Workbench's entries that are not tabs yet (`side_pane/entries.rs`), the tools first, then the actions
    /// under a hairline, each with its keys. While it is open the button stays filled and drops its tooltip, so the
    /// tooltip never sits over the menu it opened. It leaves as menus do (`motion::presence`).
    fn render_side_plus(
        &self,
        offered: Vec<entries::Entry>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.side.plus_menu;
        let button = bar_button("side-plus", Lucide::Plus, palette)
            .when(open, |button| button.bg(palette.selected))
            .when(!open, |button| {
                button.tooltip(|window, cx| Tooltip::new("Add a tool or a terminal").build(window, cx))
            })
            .menu_trigger(
                open,
                cx,
                |this| this.side.plus_menu,
                |this, _, cx| {
                    this.side.plus_menu = !this.side.plus_menu;
                    cx.notify();
                },
            );
        let menu = v_flex()
            .id("side-plus-menu")
            .occlude()
            .min_w(px(200.))
            .p(px(4.))
            .gap(px(1.))
            .rounded(px(10.))
            .bg(palette.menu)
            .shadow(palette.pop_shadow(palette.pop_ring()))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.side.plus_menu {
                    this.side.plus_menu = false;
                    cx.notify();
                }
            }));
        let mut rows: Vec<AnyElement> = Vec::with_capacity(offered.len() + 1);
        for (index, entry) in offered.iter().copied().enumerate() {
            if entry.action() && index > 0 {
                rows.push(
                    div()
                        .py(px(4.))
                        .px(px(6.))
                        .child(div().h(px(1.)).bg(palette.border))
                        .into_any_element(),
                );
            }
            let slug = entry.slug();
            let refusal = self.entry_refusal(entry);
            rows.push(
                h_flex()
                    .id(SharedString::from(format!("side-plus-{slug}")))
                    .debug_selector(move || format!("side-plus-{slug}"))
                    .h(px(32.))
                    .pl(px(8.))
                    .pr(px(6.))
                    .gap(px(8.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .when(refusal.is_none(), |row| {
                        row.cursor_pointer()
                            .hover(|style| menu_item_hover(style, palette))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_entry(entry, window, cx)))
                    })
                    .when_some(refusal, |row, reason| {
                        row.opacity(0.55)
                            .tooltip(move |window, cx| Tooltip::new(reason).build(window, cx))
                    })
                    .child(Icon::new(entry.icon()).size(px(15.)).text_color(palette.icon))
                    .child(div().flex_1().child(self.entry_label(entry)))
                    .when_some(Self::entry_keys(entry), |row, keys| row.child(key_hint(&keys, palette)))
                    .into_any_element(),
            );
        }
        let menu = menu.children(rows);
        div()
            .relative()
            .child(button)
            .when_some(crate::motion::presence("side-plus-menu", open, cx), |frame, phase| {
                frame.child(
                    div().absolute().top_0().left_0().child(
                        deferred(
                            anchored()
                                .anchor(Anchor::TopLeft)
                                .offset(point(px(0.), px(32.)))
                                .snap_to_window_with_margin(px(12.))
                                .child(phase.menu("side-plus-menu", 4., menu)),
                        )
                        .with_priority(1),
                    ),
                )
            })
    }

    /// Show the Workbench from the session head. Its own head holds the close button while it is open.
    pub(super) fn render_side_toggle(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.side.open {
            return None;
        }
        Some(
            bar_button("toggle-side-pane", Lucide::PanelRightOpen, palette)
                .mr(px(8.))
                .debug_selector(|| "toggle-side-pane".into())
                .tooltip(|window, cx| {
                    crate::key_bindings::tooltip("Show the Workbench", "toggle-side-pane").build(window, cx)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.toggle_side_pane(window, cx)),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod command_key_tests;

#[cfg(test)]
mod home_tests;
#[cfg(test)]
mod install_terminal_tests;

#[cfg(test)]
mod maximized_head_tests;

#[cfg(test)]
mod pending_tests {
    use super::*;
    use core::prelude::v1::test;

    #[gpui_kit::test]
    fn restored_routes_do_not_spawn_home_shells_while_metadata_is_delayed(cx: &mut TestAppContext) {
        let _agents = crate::code_run::fixture_agents(Vec::new());
        use crate::workspace::{code_view::CodeView, machine_source::MachineSource};
        cx.update(gpui_kit::init);
        let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
        let (_, events) = futures::channel::mpsc::unbounded();
        let (_, arrivals) = futures::channel::mpsc::unbounded();
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::with_machine_source(
                std::sync::Arc::new(gateway),
                events,
                arrivals,
                false,
                MachineSource::isolated(),
                window,
                cx,
            );
            workspace._code_poll = Task::ready(());
            workspace
        });
        workspace.update_in(cx, |workspace, window, cx| {
            for project in [true, false] {
                let dir = std::env::temp_dir();
                let scope = if project {
                    Scope::Project("delayed-project".into())
                } else {
                    Scope::Session("delayed-thread".into())
                };
                workspace.view = if project {
                    View::NewCode
                } else {
                    View::Code(CodeView::new("delayed-thread".into(), window, cx))
                };
                workspace.history = vec![if project {
                    super::super::Place::NewCode
                } else {
                    super::super::Place::Code("delayed-thread".into())
                }];
                workspace.history_at = 0;
                workspace.code_form.project = Some("delayed-project".into());
                workspace.cloud_ui.on = false;
                workspace.side = SidePane::new();
                workspace.side.terminal_factory = TerminalSet::for_view_test;
                workspace.side.scope = Some(scope);
                workspace.side.dir = dir.clone();
                workspace.side.open = true;
                workspace.side.tool = Tool::Terminal;
                let set = cx.new(|cx| TerminalSet::for_view_test(dir.clone(), cx));
                let subscription = cx.subscribe_in(&set, window, Workspace::on_terminal_set_event);
                workspace.side.terminals.insert(dir.clone(), (set, subscription));
                let saved = workspace.retain_private_terminals(cx);
                workspace.side = SidePane::new();
                workspace.restore_private_terminals(saved, window, cx);
                workspace.code_overview_loaded = false;
                for loaded in [false, false, true] {
                    workspace.code_overview_loaded = loaded;
                    workspace.repick_project_after_overview();
                    workspace.sync_side(window, cx);
                    assert_eq!(workspace.side.terminals.len(), 1);
                    assert!(workspace.side.terminals.contains_key(&dir));
                    assert_eq!(workspace.side.dir, dir);
                    assert!(workspace.side.restoring_scope.is_some());
                }
                if project {
                    workspace.code_form.project = Some("chosen-other-project".into());
                } else {
                    workspace.view = View::Library;
                    workspace.visit(super::super::Place::Library);
                }
                assert!(!workspace.waiting_for_restored_scope());
                assert!(workspace.side.restoring_scope.is_none());
                workspace.side.clear_private_terminals();
            }
        });
    }

    #[gpui_kit::test]
    fn pending_sessions_keep_project_tools_without_polling_a_draft_id(cx: &mut TestAppContext) {
        use crate::code_run::NewSession;
        use crate::workspace::{code_view::CodeView, machine_source::MachineSource};
        use nexus_workbench::event::{SessionAgent, Via};

        let _agents = crate::code_run::fixture_agents(Vec::new());
        cx.update(gpui_kit::init);
        let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
        let (_, events) = futures::channel::mpsc::unbounded();
        let (_, arrivals) = futures::channel::mpsc::unbounded();
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::with_machine_source(
                std::sync::Arc::new(gateway),
                events,
                arrivals,
                false,
                MachineSource::isolated(),
                window,
                cx,
            );
            workspace._code_poll = Task::ready(());
            workspace
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.view = View::Code(CodeView::pending(
                NewSession {
                    project_id: "project".into(),
                    agent: SessionAgent::Codex,
                    model: None,
                    permission_mode: "default".into(),
                    effort: None,
                    via: Via::Subscription,
                    provider: None,
                    worktree: false,
                    review: None,
                    base: None,
                    text: "Keep my prompt".into(),
                    attachments: Default::default(),
                },
                window,
                cx,
            ));
            let View::Code(view) = &workspace.view else {
                unreachable!()
            };
            let draft = view.id.clone();
            for failed in [false, true] {
                if failed {
                    assert!(workspace.pending_failed(&draft, "Sign in to Nexus to start Codex."));
                }
                workspace.sync_agents_tool(cx);
                assert!(workspace.side.agents.is_none(), "a draft has no agent tree to poll");
                assert!(matches!(workspace.side_scope(), Scope::Project(id) if id == "project"));
                assert!(workspace.ensure_shared_terminal(window, cx).is_none());
            }
            assert!(workspace.pending_created(&draft, "thread"));
            assert!(matches!(workspace.side_scope(), Scope::Session(id) if id == "thread"));
        });
    }

    /// Before a session starts, its Terminal tab is the Commands tab it gets once it starts: named Commands, open on
    /// the empty list with New shell, each shell a row, and still there when the last shell stops.
    #[gpui_kit::test]
    fn a_draft_sessions_terminal_is_drawn_as_its_commands(cx: &mut TestAppContext) {
        use crate::code_run::NewSession;
        use crate::workspace::{code_view::CodeView, machine_source::MachineSource};
        use nexus_workbench::event::{SessionAgent, Via};

        let _agents = crate::code_run::fixture_agents(Vec::new());
        cx.update(gpui_kit::init);
        // The Workbench is in place at once, not sliding in, so a click lands on its rows.
        cx.update(|cx| cx.set_reduce_motion(true));
        let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
        let (_, events) = futures::channel::mpsc::unbounded();
        let (_, arrivals) = futures::channel::mpsc::unbounded();
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::with_machine_source(
                std::sync::Arc::new(gateway),
                events,
                arrivals,
                false,
                MachineSource::isolated(),
                window,
                cx,
            );
            workspace._code_poll = Task::ready(());
            workspace.code = None;
            workspace.runner = None;
            workspace.auth = crate::workspace::Auth::SignedIn;
            workspace
        });
        workspace.update_in(cx, |_, window, _| window.activate_window());
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.view = View::Code(CodeView::pending(
                NewSession {
                    project_id: "project".into(),
                    agent: SessionAgent::Codex,
                    model: None,
                    permission_mode: "default".into(),
                    effort: None,
                    via: Via::Subscription,
                    provider: None,
                    worktree: false,
                    review: None,
                    base: None,
                    text: "Keep my prompt".into(),
                    attachments: Default::default(),
                },
                window,
                cx,
            ));
            workspace.side.terminal_factory = TerminalSet::for_view_test;
            workspace.pick_tool(Tool::Terminal, window, cx);
            assert!(workspace.side.draft_commands());
            assert!(
                workspace.side.shows_commands(),
                "the tab is named Commands and has no ×"
            );
            assert!(workspace.side.terminal().is_none(), "opening the tab starts no shell");
            assert!(workspace.side.showing(Tool::Terminal));
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("commands-pane-header").is_some());
        assert!(cx.debug_bounds("draft-command-row-1").is_none());

        workspace.update_in(cx, |workspace, window, cx| workspace.new_side_terminal(window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("commands-pane-header").is_some());
        let stop = cx
            .debug_bounds("row-stop-draft-1")
            .expect("the shell is a Commands row");
        cx.simulate_click(stop.center(), gpui_kit::Modifiers::none());
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _, _| {
            assert!(workspace.side.terminal().is_none());
            assert!(
                workspace.side.showing(Tool::Terminal),
                "the Commands tab stays on its empty list"
            );
            assert!(workspace.side.has_terminal());
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("commands-pane-header").is_some());
        assert!(cx.debug_bounds("draft-command-row-1").is_none());
        workspace.update_in(cx, |workspace, _, _| workspace.side.clear_private_terminals());
    }
}

/// A shortcut in a menu row: one small filled keycap per key, as Linear's menus.
fn key_hint(keystroke: &str, palette: Palette) -> impl IntoElement {
    h_flex()
        .flex_none()
        .gap(px(3.))
        .children(shortcut_label::keys(keystroke).into_iter().map(|key| {
            div()
                .min_w(px(18.))
                .h(px(18.))
                .px(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .bg(palette.selected)
                .text_size(px(11.))
                .text_color(palette.muted)
                .child(key)
        }))
}

/// Where the browser named `key` shows pages, as the Nexus app's `wb_browser` decides (`nexus_browser::place`): this
/// computer for a local session, a project or home; the machine for a session a cloud machine's relay carries; the
/// server for a server's session, whose dev servers open through an SSH forward (`side/browser/server_preview.rs`).
fn browser_place(key: &str) -> Result<Place, String> {
    let machines = crate::machines::get();
    let machine = machines.machine_of(key);
    nexus_browser::place::session_with_servers(
        machine.is_none(),
        key,
        machine.as_deref(),
        |machine| machines.is_cloud(machine),
        |machine| machines.is_server(machine),
    )
}

/// A cloud machine's or a server's website profile: its own, never saved (`nexus_browser::profile::cloud_scope`, by
/// the machine's id). A server's `localhost` pages never share this computer's cookies.
fn cloud_profile(machine: &str) -> Profile {
    Profile::new(
        &crate::app_id::get(),
        &nexus_browser::profile::cloud_scope(machine),
        false,
    )
}

/// The browsers whose website profile is not the one `wanted` names for them: they belong to another sign-in, or
/// their session's place changed.
fn other_profiles(browsers: &[(String, Profile)], wanted: impl Fn(&str) -> Profile) -> Vec<String> {
    browsers
        .iter()
        .filter(|(key, profile)| *profile != wanted(key))
        .map(|(key, _)| key.clone())
        .collect()
}

#[cfg(test)]
mod profile_tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn another_sign_in_closes_the_last_ones_browsers_only() {
        let scope = |org: &str, marker: &str| nexus_browser::profile::scope("https://api.nexus.inc", org, marker);
        let a = Profile::new("app", &scope("org_1", "m1"), true);
        let b = Profile::new("app", &scope("org_2", "m1"), true);
        let browsers = vec![("one".to_string(), a), ("two".to_string(), a), ("three".to_string(), b)];
        assert_eq!(other_profiles(&browsers, |_| a), ["three"]);
        assert_eq!(other_profiles(&browsers, |_| b), ["one", "two"]);
        let unsaved = Profile::new("app", &scope("org_1", "m1"), false);
        assert_eq!(
            other_profiles(&browsers, |_| unsaved),
            ["one", "two", "three"],
            "the organization stopped keeping data on this computer"
        );
    }

    #[test]
    fn a_cloud_sessions_browser_keeps_its_machines_profile_through_a_sign_in_change() {
        let signed_in = Profile::new(
            "app",
            &nexus_browser::profile::scope("https://api.nexus.inc", "o", "m"),
            true,
        );
        let cloud = cloud_profile("m_cloud");
        assert!(!cloud.persistent, "a cloud machine's pages are never saved");
        assert_ne!(cloud, cloud_profile("m_other"), "one per machine");
        let browsers = vec![("local".to_string(), signed_in), ("cloud".to_string(), cloud)];
        let wanted = |key: &str| if key == "cloud" { cloud } else { signed_in };
        assert!(other_profiles(&browsers, wanted).is_empty());
        // The machine no longer carries the session (signed out, deleted): its browser closes.
        assert_eq!(other_profiles(&browsers, |_| signed_in), ["cloud"]);
    }
}

#[cfg(test)]
mod page_tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn new_workbench_tabs_append_after_agents_in_either_creation_order() {
        for (first, second) in [
            (Tab::Tool(Tool::Terminal), Tab::Tool(Tool::Browser)),
            (Tab::Tool(Tool::Browser), Tab::Tool(Tool::Terminal)),
        ] {
            let side = SidePane::new();
            let base = vec![
                Tab::Tool(Tool::Changes),
                Tab::Tool(Tool::Files),
                Tab::Tool(Tool::Agents),
                Tab::SideChat,
            ];
            assert_eq!(side.ordered_tabs(base.clone()), base);
            let available = |terminal, browser| {
                Tool::ALL
                    .into_iter()
                    .map(Tab::Tool)
                    .filter(|tool| match tool {
                        Tab::Tool(Tool::Terminal) => terminal,
                        Tab::Tool(Tool::Browser) => browser,
                        Tab::Tool(Tool::Artifacts) => false,
                        _ => true,
                    })
                    .chain([Tab::SideChat])
                    .collect()
            };
            let mut expected = base;
            expected.push(first);
            assert_eq!(
                side.ordered_tabs(available(
                    first == Tab::Tool(Tool::Terminal),
                    first == Tab::Tool(Tool::Browser)
                )),
                expected
            );
            expected.push(second);
            assert_eq!(side.ordered_tabs(available(true, true)), expected);
            // Selecting an existing tool does not move its tab.
            assert_eq!(side.ordered_tabs(available(true, true)), expected);
            // Closing and reopening before a render still appends it last.
            let Tab::Tool(tool) = first else { unreachable!() };
            side.forget_tab(tool);
            assert_eq!(
                side.ordered_tabs(available(true, true)),
                [
                    Tab::Tool(Tool::Changes),
                    Tab::Tool(Tool::Files),
                    Tab::Tool(Tool::Agents),
                    Tab::SideChat,
                    second,
                    first
                ]
            );
        }
    }

    #[test]
    fn workbench_tab_order_survives_scope_switches_and_removes_missing_tools() {
        let mut side = SidePane::new();
        side.scope = Some(Scope::Session("one".into()));
        side.ordered_tabs(vec![Tab::Tool(Tool::Agents)]);
        assert_eq!(
            side.ordered_tabs(vec![Tab::Tool(Tool::Browser), Tab::Tool(Tool::Agents)]),
            [Tab::Tool(Tool::Agents), Tab::Tool(Tool::Browser)]
        );
        side.scope = Some(Scope::Session("two".into()));
        assert_eq!(
            side.ordered_tabs(vec![Tab::Tool(Tool::Terminal)]),
            [Tab::Tool(Tool::Terminal)]
        );
        side.scope = Some(Scope::Session("one".into()));
        assert_eq!(
            side.ordered_tabs(vec![Tab::Tool(Tool::Browser), Tab::Tool(Tool::Agents)]),
            [Tab::Tool(Tool::Agents), Tab::Tool(Tool::Browser)]
        );
        assert_eq!(
            side.ordered_tabs(vec![Tab::Tool(Tool::Agents)]),
            [Tab::Tool(Tool::Agents)]
        );
        assert_eq!(
            side.ordered_tabs(vec![Tab::Tool(Tool::Browser), Tab::Tool(Tool::Agents)]),
            [Tab::Tool(Tool::Agents), Tab::Tool(Tool::Browser)]
        );
    }

    #[test]
    fn gateway_chats_keep_browser_tabs_separate_without_project_tools() {
        let mut side = SidePane::new();
        side.scope = Some(Scope::Chat("one".into()));
        assert_eq!(side.browser_key(), "chat:one");
        assert!(!side.has_folder());
        side.scope = Some(Scope::Chat("two".into()));
        assert_eq!(side.browser_key(), "chat:two");
        side.scope = Some(Scope::Session("one".into()));
        assert_eq!(side.browser_key(), "one");
        assert!(side.has_folder());
    }

    /// A page opened in Chat (the home scope, no folder) shows in the Workbench's Artifacts tab, which opens for it,
    /// as in a Code session. There is no page panel beside the chat any more.
    #[test]
    fn a_page_opened_in_chat_opens_the_workbench_on_artifacts() {
        let mut side = SidePane::new();
        side.scope = Some(Scope::Home);
        side.sync_page(false);
        assert!(!side.open);
        side.sync_page(true);
        assert!(side.open, "the Workbench opens for the page");
        assert_eq!(side.tool, Tool::Artifacts);
        // Closed by hand with the page still there: it stays closed on the next frame.
        side.open = false;
        side.sync_page(true);
        assert!(!side.open);
        // Another page replaces it: the Workbench opens on it again.
        side.page_replaced();
        side.sync_page(true);
        assert!(side.open);
        assert_eq!(side.tool, Tool::Artifacts);
    }

    /// A page that is open but hidden (the Workbench closed, or another tab on top) is not on screen, so its chat
    /// card offers Open again.
    #[test]
    fn a_hidden_page_is_not_shown() {
        let mut side = SidePane::new();
        side.scope = Some(Scope::Session("s".into()));
        side.page_replaced();
        side.sync_page(true);
        assert!(side.showing(Tool::Artifacts));
        side.open = false;
        assert!(!side.showing(Tool::Artifacts), "the Workbench closed");
        side.open = true;
        side.tool = Tool::Changes;
        assert!(!side.showing(Tool::Artifacts), "another tab on top");
    }

    /// The page's full view is the Workbench maximized on Artifacts. The toggle opens it from a closed
    /// Workbench or another tab, and puts the Workbench back beside the session, still open on the page.
    #[test]
    fn the_page_full_view_toggles_the_workbench_maximized_on_artifacts() {
        let mut side = SidePane::new();
        side.scope = Some(Scope::Home);
        side.plus_menu = true;
        assert!(!side.page_full());
        assert!(side.toggle_page_full());
        assert!(side.open && side.maximized && !side.plus_menu);
        assert_eq!(side.tool, Tool::Artifacts);
        assert!(!side.toggle_page_full());
        assert!(side.open, "the Workbench stays open beside the session");
        assert!(!side.maximized);
        assert_eq!(side.tool, Tool::Artifacts);
        // Another tool maximized is not the page's full view: the toggle moves it to the page, still maximized.
        side.tool = Tool::Browser;
        side.maximized = true;
        assert!(!side.page_full());
        assert!(side.toggle_page_full());
        assert_eq!(side.tool, Tool::Artifacts);
    }

    /// The canvas's Escape leaves the page's full view, and does nothing otherwise: not to a maximized Browser, not
    /// to a page beside the session.
    #[test]
    fn the_canvas_escape_leaves_only_the_page_full_view() {
        let mut side = SidePane::new();
        assert!(!side.leave_page_full());
        side.toggle_page_full();
        assert!(side.leave_page_full());
        assert!(side.open && !side.maximized);
        assert!(!side.leave_page_full());
        side.tool = Tool::Browser;
        side.maximized = true;
        assert!(!side.leave_page_full());
        assert!(side.maximized, "a maximized Browser stays as it was");
    }

    /// With no folder and no other tool open, nothing takes the Artifacts tab's place: the Workbench closes.
    #[test]
    fn chat_has_no_tab_to_fall_back_to_when_the_page_closes() {
        let mut side = SidePane::new();
        side.scope = Some(Scope::Home);
        assert_eq!(side.fallback(), None);
        side.scope = Some(Scope::Session("s".into()));
        assert_eq!(side.fallback(), Some(Tool::Changes));
    }
}

impl Workspace {
    pub(super) fn agents_folder_label(&self) -> String {
        match self.scope_folder(&self.side_scope()) {
            Ok(folder) => folder.path.display().to_string(),
            Err(_) => self
                .side
                .agents
                .as_ref()
                .and_then(|state| state.summary.as_ref())
                .and_then(|summary| summary.worktree.as_ref())
                .map(|worktree| worktree.path.clone())
                .unwrap_or_else(|| "Project folder on the session’s machine".into()),
        }
    }
}

#[cfg(test)]
mod browser_retention_tests {
    use super::{Browser, Scope, Tab, Tool, Workspace};
    use gpui_kit::{AppContext, Context, Task, TestAppContext, Window};

    #[gpui_kit::test]
    fn adopted_local_and_cloud_owners_revoke_before_immediate_close_without_render(cx: &mut TestAppContext) {
        let _agents = crate::code_run::fixture_agents(Vec::new());
        cx.update(gpui_kit::init);
        let (local, local_probe) = crate::web::fixture_page(121);
        let (cloud, cloud_probe) = crate::web::fixture_page(122);
        let make_workspace = |window: &mut Window, cx: &mut Context<Workspace>| {
            let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
            let (_, events) = futures::channel::mpsc::unbounded();
            let (_, arrivals) = futures::channel::mpsc::unbounded();
            let mut workspace = Workspace::with_machine_source(
                std::sync::Arc::new(gateway),
                events,
                arrivals,
                false,
                crate::workspace::machine_source::MachineSource::isolated(),
                window,
                cx,
            );
            workspace._code_poll = Task::ready(());
            workspace
        };
        let saved = {
            let (workspace, cx) = cx.add_window_view(make_workspace);
            workspace.update_in(cx, |workspace, window, cx| {
                let local = cx.new(|cx| Browser::fixture_retained("local-session", local, window, cx));
                let cloud = cx.new(|cx| {
                    let mut browser = Browser::fixture_retained("cloud-session", cloud, window, cx);
                    browser.fixture_cloud_identity("machine-a", super::cloud_profile("machine-a"));
                    browser
                });
                workspace.bind_browser("local-session".into(), local, window, cx);
                workspace.bind_browser("cloud-session".into(), cloud, window, cx);
                let saved = workspace.retain_private_terminals(cx);
                window.remove_window();
                saved
            })
        };
        let saved = {
            let (workspace, cx) = cx.add_window_view(make_workspace);
            workspace.update_in(cx, |workspace, window, cx| {
                workspace.restore_private_terminals(saved, window, cx);
                let old: Vec<_> = workspace.side.browsers.values().cloned().collect();
                assert!(local_probe.alive() && cloud_probe.alive());
                // No render, lazy profile cleanup or executor turn occurs between adoption,
                // revocation and closing again. Cloud website profiles alone are not authority.
                workspace.handle(
                    crate::gateway::Event::SignInFailed("fixture revoked".into()),
                    window,
                    cx,
                );
                assert!(!local_probe.alive() && !cloud_probe.alive());
                assert!(workspace.side.browsers.is_empty() && workspace.side.browser_events.is_empty());
                for browser in old {
                    browser.update(cx, |browser, cx| {
                        let call = browser.agent(
                            "browser_tabs".into(),
                            serde_json::json!({}),
                            "fixture-run".into(),
                            std::sync::Arc::new(|| true),
                            window,
                            cx,
                        );
                        let error = futures::FutureExt::now_or_never(call)
                            .expect("retired view must refuse immediately")
                            .unwrap_err();
                        assert!(error.contains("parked"));
                    });
                }
                let saved = workspace.retain_private_terminals(cx);
                assert!(saved.browsers.is_empty());
                window.remove_window();
                saved
            })
        };
        let (workspace, cx) = cx.add_window_view(make_workspace);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.restore_private_terminals(saved, window, cx);
            assert!(workspace.side.browsers.is_empty());
            assert!(!local_probe.alive() && !cloud_probe.alive());
        });
    }

    #[gpui_kit::test]
    fn session_browser_owner_crosses_workspace_destruction_with_fresh_bindings(cx: &mut TestAppContext) {
        let _agents = crate::code_run::fixture_agents(Vec::new());
        cx.update(gpui_kit::init);
        let (page, probe) = crate::web::fixture_page(111);
        let (home_page, home_probe) = crate::web::fixture_page(112);
        let make_workspace = |window: &mut Window, cx: &mut Context<Workspace>| {
            let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
            let (_, events) = futures::channel::mpsc::unbounded();
            let (_, arrivals) = futures::channel::mpsc::unbounded();
            let mut workspace = Workspace::with_machine_source(
                std::sync::Arc::new(gateway),
                events,
                arrivals,
                false,
                crate::workspace::machine_source::MachineSource::isolated(),
                window,
                cx,
            );
            workspace._code_poll = Task::ready(());
            workspace
        };
        let (mut saved, mut old_browser_id) = {
            let (workspace, cx) = cx.add_window_view(make_workspace);
            workspace.update_in(cx, |workspace, window, cx| {
                let browser = cx.new(|cx| Browser::fixture_retained("retained-session", page, window, cx));
                let old_id = browser.entity_id();
                workspace.bind_browser("retained-session".into(), browser, window, cx);
                let key = nexus_desktop_core::home_bench::home_browser(
                    &format!("{:?}", window.window_handle().window_id()),
                    "",
                );
                workspace.side.home_browser = key.clone();
                workspace
                    .side
                    .tab_order
                    .borrow_mut()
                    .insert(key.clone(), vec![Tab::Tool(Tool::Browser)]);
                let home = cx.new(|cx| Browser::fixture_retained(&key, home_page, window, cx));
                workspace.bind_browser(key, home, window, cx);
                workspace.side.open = true;
                workspace.side.tool = Tool::Browser;
                workspace.side.scope = Some(Scope::Session("retained-session".into()));
                let mut saved = workspace.retain_private_terminals(cx);
                saved.discard_stale_browsers();
                assert!(saved.browsers.contains_key("retained-session"));
                assert!(workspace.side.browsers.is_empty() && workspace.side.browser_events.is_empty());
                window.remove_window();
                (saved, old_id)
            })
        };
        assert!(probe.alive() && probe.parked() && home_probe.alive() && home_probe.parked());
        for _ in 0..2 {
            let old_home_key = saved.home_browser.clone();
            let (workspace, cx) = cx.add_window_view(make_workspace);
            (saved, old_browser_id) = workspace.update_in(cx, |workspace, window, cx| {
                workspace.restore_private_terminals(saved, window, cx);
                assert!(workspace.side.open);
                assert!(matches!(workspace.side.tool, Tool::Browser));
                let browser = workspace.side.browsers.get("retained-session").unwrap();
                let new_id = browser.entity_id();
                assert_ne!(new_id, old_browser_id);
                assert!(workspace.side.browser_events.contains_key("retained-session"));
                let home_key = workspace.side.home_browser.clone();
                assert_ne!(home_key, old_home_key);
                assert!(workspace.side.browsers.contains_key(&home_key));
                assert!(!workspace.side.browsers.contains_key(&old_home_key));
                assert_eq!(workspace.side.browsers.len(), 2);
                assert_eq!(workspace.side.tab_order.borrow().len(), 1);
                assert_eq!(
                    workspace.side.tab_order.borrow().get(&home_key),
                    Some(&vec![Tab::Tool(Tool::Browser)])
                );
                assert!(probe.alive() && !probe.parked() && home_probe.alive() && !home_probe.parked());
                let saved = workspace.retain_private_terminals(cx);
                window.remove_window();
                (saved, new_id)
            });
        }
        // Rejecting the identity envelope disposes both actual CEF owners.
        drop(saved);
        assert!(!probe.alive() && !home_probe.alive());
    }
}
