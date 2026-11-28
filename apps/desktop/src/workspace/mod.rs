//! The main window: the sessions on the left, the open chat on the right, the composer under it. Signed out, the
//! window shows the device sign-in instead.
//!
//! The parts draw in their own files: [`sidebar`], [`transcript`] (the messages), [`composer`] (the prompt box
//! and its model menu) and [`code_view`] (a Code session). This file holds the state they share and what changes it.

mod account_states;
mod agent_setup;
mod agents_tool;
mod answer_footer;
mod answer_links;
mod api_keys_view;
mod approval_keys;
mod archive;
mod artifact_picker;
mod assistant;
mod assistant_errands;
mod assistant_title;
mod backdrop;
mod backdrop_live;
mod chat_attachments;
mod chat_choices;
mod chat_column;
mod chat_menu;
mod chat_queue;
mod chat_rewind;
mod chat_rows;
mod chat_share;
mod chat_skills;
mod cite_selection;
mod claude_trust;
mod clone_form;
mod cloud_view;
pub(crate) mod code_block;
mod code_dock;
mod code_find;
mod code_folds;
mod code_jump;
mod code_media;
mod code_pages;
mod code_recovery;
mod gateway_chat;
mod kept_chats;
mod launch_motion;
mod live_chat;
mod missing_dialog;
mod permission_queue;
mod project_dialog;
mod project_drag;
mod setup_fix;
mod share_ui;
mod warm_chat;
/// Build the new session's background before the first window (`backdrop.rs`).
pub(crate) use backdrop::prewarm as prewarm_backdrop;
/// The permission modes with their names, for Settings › Projects' defaults.
pub(crate) use code_dock::MODES as PERMISSION_MODES;
pub(crate) use code_dock::MenuTrigger;
pub(crate) use code_dock::menu_item_hover;
mod auto_preview;
mod auto_route_preview;
pub(crate) mod browser_detach;
#[cfg(test)]
mod code_start_tests;
mod code_title;
mod code_view;
pub(crate) mod commands_detach;
mod composer;
mod crash_prompt;
mod crew_line;
mod cross_project;
mod diff_comments;
mod drafts;
pub(crate) mod files_detach;
/// Write every window's composer drafts now (`drafts.rs`): the app is quitting.
pub(crate) use drafts::flush as flush_composer_drafts;
mod agent_switch;
mod base_pick;
mod chat_picker;
mod code_picker;
pub(crate) use code_picker::takes_auto;
mod docs_view;
mod feedback_dialog;
mod file_chips;
mod file_open;
mod file_peek;
mod file_picker;
pub(crate) mod find_bar;
mod floating_dock;
mod folder_motion;
mod fork_points;
#[cfg(test)]
mod fork_tests;
pub(crate) mod gateway_access;
pub(crate) mod glass;
mod harness;
mod image_viewer;
mod inbox_marks;
mod inbox_view;
mod interrupt_bars;
mod issues_connect;
mod issues_view;
mod last_place;
mod library_view;
mod limit_line;
mod link_menu;
mod logo;
mod machine_source;
mod measure_send;
mod menu_side;
mod message_actions;
pub(crate) mod message_text;
mod mic;
mod minimap;
mod missing_agents;
mod model_menu;
mod models_providers;
mod models_view;
mod move_worktree;
mod navigation;
mod new_code;
pub(crate) mod nexus_sign_in;
mod no_key;
mod notice_host;
mod org_menu;
mod org_scope;
#[cfg(test)]
mod org_scope_tests;
mod outside_prompt;
mod overlay;
mod page_canvas;
mod page_header;
mod page_loading;
mod page_panel;
mod pane;
mod pane_motion;
mod panel_view;
mod pay_bar;
mod picker_navigation;
mod place_links;
mod plans_off;
mod project_files;
mod project_list;
mod project_match;
mod project_menu;
mod project_menu_github;
mod prompt_history;
mod pull_card;
mod pull_stack;
mod pull_view;
mod read_only;
mod repo_groups;
mod review_pick;
mod row_exit;
mod runs;
mod search;
mod secret_card;
mod seen;
mod send_back;
mod server_folder;
mod session_export;
pub(crate) mod session_keys;
mod session_menu;
#[cfg(test)]
mod shell_tests;
pub(crate) use session_menu::pull_color;
#[cfg(test)]
mod browser_origin_tests;
mod first_connection;
mod handoff_card;
#[cfg(test)]
pub(crate) use first_connection::tests as first_connection_tests;
#[cfg(test)]
mod live_fields_tests;
mod local_mode;
mod onboarding;
mod queue_drag;
const REQUIRES_SIGN_IN: &str = "Requires sign-in";
mod revoked_notice;
mod session_drag;
mod session_drag_edge;
mod session_drag_machine;
mod session_drop;
mod session_drop_undo;
mod session_message;
mod session_order;
mod session_peek;
mod session_peek_background;
mod session_peek_pull;
mod session_share;
mod session_snooze_drops;
mod session_start;
mod session_tree;
mod sessions_list;
mod settings_access;
mod settings_registry;
mod settings_search;
mod settings_shell;
mod settings_view;
mod setup_row;
mod shell;
mod shortcuts_page;
mod side_chat;
mod side_pane;
mod sidebar;
mod sidebar_budget;
mod sidebar_edge_cue;
mod sidebar_filter;
mod sidebar_groups;
mod sidebar_history;
mod sidebar_hold;
mod sidebar_machines;
mod sidebar_marks;
mod sidebar_resize;
mod sidebar_select;
mod sign_in;
mod sign_in_parts;
mod slash;
mod stacked_replies;
mod start_blockers;
mod start_hints;
mod start_readiness;
mod startup;
mod stash;
mod team_following;
mod team_presence;
mod team_sessions;
mod text_paste;
mod title_fade;
mod tool_body;
mod transcript;
mod update_restart;
mod update_toast;
mod uploads;
mod write_it;
pub(crate) use write_it::write_button;
mod usage;
mod usage_meter;
mod usage_prompt;
mod web_sources;
mod working_times;

use crate::workspace::overlay::{Overlay, OverlaySlot};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::UnboundedReceiver;
use futures::{FutureExt as _, StreamExt as _};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Theme, ThemeMode, TitleBar, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::backend::Backend;
use crate::code::{CodeProject, CodeStore};
use crate::code_run::{CodeEvent, CodeRunner};
use crate::gateway::{Account, Event, SearchHit};
use crate::model::{Message, ModelChoice, SessionSummary};
use crate::owner::Owner;
use crate::palette::{Palette, ThemeChoice};
/// The permission modes with their names, for the schedules form in Settings.
pub(crate) use code_dock::MODES;
pub(crate) use code_dock::{menu_box, menu_label, menu_row};
use code_view::CodeView;
use mic::MicState;
use model_menu::MenuRow;
use settings_view::SettingsSection;
use transcript::Entry;
/// The chat's Markdown style, for the Assistant's answers (`pages/agent.rs`).
pub(crate) use transcript::prose;

actions!(workspace, [NewChat, ToggleSidebar, FindChat, FindInChat]);
// Back and forward in the window's history, as the title bar's arrows (`BACK_HINT`, `FORWARD_HINT`). Over the session
// browser, the page's history. Named actions, so a changed shortcut can bind them.
actions!(workspace, [GoBack, GoForward]);
// Settings › Preferences from anywhere in the window: Ctrl+, (⌘, on macOS).
actions!(workspace, [OpenSettings]);
// The Assistant from anywhere in the window (Cmd+J, or Ctrl+J), as its sidebar row's hint shows.
actions!(workspace, [OpenAssistant]);
// A new errand from anywhere in the window (Cmd+Shift+J, or Ctrl+Shift+J): Errands with its New errand box.
actions!(workspace, [NewErrand]);
/// ⌃⌘O: the account switcher.
actions!(workspace, [SwitchAccount]);
pub use side_pane::{
    NewTerminal, SideTool1, SideTool2, SideTool3, SideTool4, SideTool5, SideTool6, ToggleChatBrowser,
    ToggleMaximizeWorkbench, ToggleSidePane, ToggleTerminal,
};
pub use stash::StashPrompt;

/// How often the Code sessions are read again from the Nexus app's store.
const CODE_POLL: Duration = Duration::from_secs(2);
/// With the store watched: the timer that only catches a missed event, and how long a burst of writes settles.
const CODE_NET: Duration = Duration::from_secs(30);
const CODE_SETTLE: Duration = Duration::from_millis(150);
/// How often the unread count is read again, and the Inbox's list while it is open.
const INBOX_COUNT_POLL: Duration = Duration::from_secs(60);
const INBOX_LIST_POLL: Duration = Duration::from_secs(8);
/// The interface font, bundled with the app (main.rs).
const FONT: &str = "DM Sans";
const PLACEHOLDER: &str = "Build anything, / for skills, @ for context";
const FIND_HINT: &str = if cfg!(target_os = "macos") { "⌘K" } else { "Ctrl K" };
const BACK_HINT: &str = if cfg!(target_os = "macos") { "⌘[" } else { "Alt ←" };
const FORWARD_HINT: &str = if cfg!(target_os = "macos") { "⌘]" } else { "Alt →" };
/// How long the copy button shows its check.
const COPIED_FOR: Duration = Duration::from_millis(1400);

/// Where a step back (`-1`) or forward (`1`) from `at` lands in a history of `len` places: none past either end.
fn history_step(at: usize, len: usize, step: isize) -> Option<usize> {
    at.checked_add_signed(step).filter(|at| *at < len)
}

/// A place in the back and forward history.
#[derive(Debug, Clone, PartialEq)]
enum Place {
    /// A Gateway chat, or a new one (`None`).
    Chat(Option<SessionSummary>),
    Code(String),
    /// The form for a new Code session.
    NewCode,
    /// The Artifacts library.
    Library,
    Inbox,
    /// A docs page, by its path under `/docs`.
    Docs(String),
    /// The person's Linear issues.
    Issues,
    Settings(SettingsSection),
    SettingsField(SettingsSection, &'static str),
    Models,
    /// A page drawn by `pages/`: a place of its own (Overview, Usage).
    Page(crate::pages::PageId),
    /// A Code session's pull request page, by the session's id.
    Pull(String),
}

/// Resources retained from the primary window; no Workspace or window-bound entities.
pub(crate) struct RetainedPrimary {
    pub(crate) identity: crate::windows::retention::Identity,
    navigation: navigation::Retained,
    new_code: Option<RetainedNewCode>,
    terminals: side_pane::RetainedPrivateTerminals,
    execution: Option<gateway_chat::Retained>,
}

impl RetainedPrimary {
    pub(crate) fn browser_copy(&self, key: &browser_detach::Key) -> bool {
        if self.identity != key.identity {
            return false;
        }
        let runs::Source::Local { data, scope } = &key.source else {
            return false;
        };
        self.terminals.has_browser_origin(
            &key.session,
            &crate::side::browser::agent::LocalOrigin {
                data: data.clone(),
                scope: scope.clone(),
                machine: None,
            },
        )
    }

    pub(crate) fn discard_stale_browsers(&mut self) {
        self.terminals.discard_stale_browsers();
    }

    pub(crate) fn browser_output_conflict(&self, key: &browser_detach::Key) -> bool {
        self.identity == key.identity && self.terminals.browser_output_conflict(key)
    }
    pub(crate) fn has_browser_origin(&self, thread: &str, origin: &crate::side::browser::agent::LocalOrigin) -> bool {
        self.terminals.has_browser_origin(thread, origin)
    }
}

struct RetainedNewCode {
    project: Option<String>,
    source: Option<runs::Source>,
    plain: bool,
    cloud: bool,
}

/// What the main area shows.
enum View {
    /// A Gateway chat: the open one (`current`), or a new one.
    Chat,
    /// A Code session of this computer, read from the Nexus app's store.
    Code(CodeView),
    /// A new Code session: its project, agent, model and permissions, then the first message (`new_code.rs`).
    NewCode,
    /// The Artifacts library: what was shared with the person, by them, and their pages.
    Library,
    /// The Inbox: notifications and the Code sessions that need the person.
    Inbox,
    /// Read-only team-visible Code sessions, through the shared session-sync reader.
    TeamSessions,
    /// Nexus UI's docs.
    Docs,
    /// The person's Linear issues, to triage one into a session (`issues_view.rs`).
    Issues,
    /// Settings, at one section.
    Settings(SettingsSection),
    /// The Models catalog.
    Models,
    /// A place drawn by `pages/` (Overview, Usage).
    Page(crate::pages::PageId),
    /// A Code session's pull request page (`pull_view.rs`).
    Pull(Entity<crate::pages::pull_request::PullRequestPage>),
}

/// The same resolver used by settings navigation also determines account links' action labels.
pub(crate) fn settings_path_is_native(path: &str) -> bool {
    settings_registry::resolve(path).is_some()
}

pub(super) enum Auth {
    /// The saved sign-in is not read yet (a slow keychain at startup, or another Gateway): the startup shell
    /// (`startup.rs`), never the sign-in card.
    Restoring,
    /// A sign-in the person started is being checked: the sign-in card, busy.
    Checking,
    SignedOut {
        error: Option<SharedString>,
    },
    Waiting {
        user_code: SharedString,
        link: SharedString,
        deadline: std::time::Instant,
    },
    SignedIn,
    Local,
}

pub struct Workspace {
    #[cfg(test)]
    browser_detach_runs: Option<runs::Runs>,
    browser_return: Option<browser_detach::Ticket>,
    #[cfg(test)]
    files_fixture_runs: Option<runs::Runs>,
    files_return: Option<files_detach::Ticket>,
    commands_return: Option<commands_detach::Ticket>,

    /// Whether this is the panel's (`panel.rs`): only the chat, under its own head (`panel_view.rs`).
    panel: bool,
    /// Nexus, as the UI asks it things (`backend.rs`); the answers come in on `events`.
    gateway: gateway_access::GatewayAccess,
    auth: Auth,
    local_mode: local_mode::LocalMode,
    #[cfg(test)]
    account_view_fixture: bool,
    view: View,
    /// Who is signed in and in which organization (`GET /v1/tenants/me`).
    account: Option<Account>,
    terminal_identity: Option<crate::windows::retention::Identity>,
    /// This window has had an account before: a later sign-in (another Gateway or organization) may open the last
    /// place again, as a launch's first window does ([`last_place::landing`]).
    had_account: bool,
    team_presence: team_presence::TeamPresence,
    /// Unread notifications, for the Inbox row.
    inbox: usize,
    /// Whether this app has an agent API to ask (`nexus_desktop_core::agent::configured`): the sidebar shows the
    /// Assistant, and its page opens, only then. Read at start and when the settings are saved here, as the Nexus app
    /// reads it at boot.
    agent: bool,
    /// The Assistant's page while the account lasts, and its sidebar row's state (`assistant.rs`).
    assistant: assistant::Assistant,
    /// Each account's Assistant for the switcher, and the switch with the Assistant open (`account_states.rs`).
    accounts: account_states::AccountStates,
    theme: ThemeChoice,
    /// The one menu, popover or dialog open over the window (`overlay.rs`).
    overlay: Option<Overlay>,
    /// A Code session's pull request form (`pr_form.rs`). Not an overlay: it holds a typed title and description and
    /// a create in flight, so a popup opened over it leaves it. Opening a top bar panel replaces it.
    pr_form: Option<code_title::PrForm>,
    /// The sidebar's session filter ([`sidebar_filter`]).
    sidebar_filter: sidebar_filter::SidebarFilter,
    /// The panel's Settings, while open: the App choices (`pages/desktop_app.rs`, compact), or none signed out.
    panel_settings: Option<panel_view::PanelSettings>,
    /// The panel's Move here: the chat the app window shows, and the watch on the app windows that keeps it current.
    main_chat: Option<SessionSummary>,
    /// One watch per app window, by its workspace: a window opened while the panel is open is watched as it opens.
    main_watch: Vec<(EntityId, Subscription)>,
    /// The chat Move here is opening.
    moving: Option<String>,
    /// The chat composer's team and web search (`chat_choices.rs`).
    choices: chat_choices::ChoicesState,
    chat_skills: chat_skills::SkillsState,
    /// The Nexus app's Code sessions store, for this Gateway and organization; `None` when there is none.
    code: Option<CodeStore>,
    /// Project metadata reads repository files; never read it on the UI thread.
    code_refresh: Option<Task<()>>,
    code_refresh_again: bool,
    projects: Vec<CodeProject>,
    working_times: working_times::WorkingTimes,
    /// The Code session row under the pointer: its actions, and its card once the pointer rests.
    peek: session_peek::Peek,
    /// The chat row under the pointer: its "…" shows beside its state.
    hovered_chat: Option<String>,
    code_error: Option<SharedString>,
    code_overview_loaded: bool,
    /// New session opened before the projects were read: it picks Code or a chat again once they are
    /// (`new_code.rs`, `settle_new_session`).
    new_session_unsettled: bool,
    /// This build's hold on the Code sessions (`code_run.rs`), shared by every window (`windows.rs`): `None` while the
    /// Nexus app owns them, and then the sessions are read-only here and `owner` says why.
    runner: Option<Rc<CodeRunner>>,
    #[cfg(test)]
    run_fixtures: HashMap<String, runs::Runs>,
    /// The servers and the cloud machines (`machines.rs`), as the sidebar groups them.
    machine_source: Arc<crate::machines::Machines>,
    servers: Vec<crate::machines::ServerView>,
    /// The servers whose Retry was pressed and whose new attempt has not ended (`sidebar_machines.rs`).
    server_retries: HashMap<String, sidebar_machines::ServerRetry>,
    cloud_machines: Vec<crate::machines::CloudView>,
    /// Cloud on the Code screen: the new session's choice, its setup and repository, a machine's end (`cloud_view.rs`).
    cloud_ui: cloud_view::CloudUi,
    /// A server's commands report here, as the runner's do (`runs.rs`).
    relay: futures::channel::mpsc::UnboundedSender<CodeEvent>,
    _relay: Task<()>,
    /// Hears the servers change, and reads them again.
    _machines: Task<()>,
    /// Who owns the Code sessions store (`owner.rs`), and the scope its lock was last tried for.
    owner: Owner,
    code_scope: Option<String>,
    /// Tries the store's lock while the person takes over from the Nexus app.
    _take_over: Option<Task<()>>,
    /// The "A folder on <server>" field under a server's head (`server_folder.rs`).
    server_folder: Option<server_folder::ServerFolder>,
    /// The agents on this computer, with their models, for the new-session form.
    code_agents: Vec<nexus_workbench::api::AgentInfo>,
    code_form: new_code::CodeForm,
    launch_motion: launch_motion::LaunchMotion,
    readiness: start_readiness::Readiness,
    /// The New session question about a project's own Claude settings (`claude_trust.rs`).
    claude_trust: claude_trust::Asks,
    composer_layout: composer::ComposerLayout,
    /// The floating composer's height, for the room under the transcript's end (`floating_dock.rs`).
    dock_height: floating_dock::DockHeight,
    /// The prompt after a crash (`crash_prompt.rs`).
    crash_prompt: crash_prompt::CrashPrompt,
    /// The first start's question about usage data (`usage_prompt.rs`).
    usage_prompt: usage_prompt::UsagePrompt,
    /// Send Feedback… (`feedback_dialog.rs`).
    feedback: feedback_dialog::FeedbackDialog,
    /// A verified update's toast, in an app window (`update_toast.rs`).
    update_toast: update_toast::UpdateToast,
    /// The notice for a withdrawn build, in an app window (`revoked_notice.rs`).
    revoked_notice: revoked_notice::RevokedNotice,
    update_restart: update_restart::UpdateRestart,
    /// The notice at the foot of the window, with Undo after an archive (`notice_host.rs`).
    notices: notice_host::NoticeState,
    notice_motion: crate::command_tray::Motion,
    closing_notice: Option<nexus_desktop_core::notice::Notice<notice_host::NoticeDo>>,
    /// Projects the person folded in the sidebar.
    folded_projects: HashSet<String>,
    /// The person's own projects and where each chat is filed (`sidebar_groups.rs`).
    groups: sidebar_groups::GroupsState,
    project_hover_motion: HashMap<String, folder_motion::Transition>,
    project_fold_motion: HashMap<String, folder_motion::Transition>,
    /// Where a closed Code session menu returns the focus, and its custom snooze field.
    session_anchor: Option<session_menu::SessionAnchor>,
    custom_snooze: Option<session_menu::CustomSnooze>,
    /// Deleted sessions' rows leaving the sidebar, or coming back when the delete failed (`row_exit.rs`).
    row_exits: row_exit::RowExits,
    /// A session row being dragged in the sidebar, and where it would land (`session_drag.rs`).
    session_drag: session_drag::DragState,
    /// A project being dragged in the sidebar, and where it would land (`project_drag.rs`).
    project_drag: project_drag::ProjectDragState,
    /// The sidebar's session rows, held while the pointer is on the card, and their glides (`sidebar_hold.rs`).
    sidebar_hold: sidebar_hold::SidebarHold,
    /// The open chat's share sheet, its title being edited in the top bar, and whether it is archived.
    chat_share: Option<Entity<chat_share::ChatShare>>,
    session_share: Option<session_share::SessionShare>,
    /// Code sessions shared with Keep updated from this window, by id: each turn's end shares them again in place.
    followed_shares: HashMap<String, session_share::Followed>,
    chat_rename: Option<(Entity<InputState>, Subscription)>,
    chat_archived: bool,
    /// The sidebar's "Archived" group: whether it is open, and what it holds.
    archived_open: bool,
    /// The "Archived" group's fold, drawn as a project's (`project_fold_motion`).
    archived_fold_motion: folder_motion::Transition,
    archived_chats: Option<Result<Vec<SessionSummary>, String>>,
    archived_code: Vec<crate::code::CodeSession>,
    renaming: Option<session_menu::Renaming>,
    /// The open Fork page's points (`fork_points.rs`).
    fork_points: Option<fork_points::ForkPoints>,
    /// The fork being made: one at a time.
    forking: Option<fork_points::Forking>,
    /// The sidebar's "Snoozed" and "Settled" groups the person unfolded.
    open_aside: HashSet<crate::code::Aside>,
    /// Each "Snoozed" and "Settled" group's fold, drawn as a project's (`project_fold_motion`).
    aside_fold_motion: HashMap<crate::code::Aside, folder_motion::Transition>,
    /// Each history head's icon-to-chevron reveal under the pointer (`sidebar_history.rs`), keyed by the group.
    history_hover_motion: HashMap<&'static str, folder_motion::Transition>,
    /// The sidebar's Add a project popover (Open a folder, Clone a repository), when open.
    clone_form: Option<clone_form::CloneForm>,
    project_dialog: Option<project_dialog::ProjectDialog>,
    /// Add artifacts, or Add to a session (`artifact_picker.rs`), while open or leaving.
    artifact_picker: Option<artifact_picker::ArtifactPicker>,
    /// Which side of its chip the where line's open menu takes (`menu_side.rs`).
    menu_side: menu_side::MenuSide,
    /// The new session's background, as picked on this computer (`backdrop.rs`).
    backdrop: backdrop::Choice,
    /// The moving background's clock and latest frame (`backdrop.rs`).
    backdrop_motion: backdrop::Motion,
    /// How far the Sessions group is opened: 12, Show more's 50, or all (`sessions_list.rs`).
    sessions_step: sessions_list::Step,
    /// The Sessions group is folded to its newest four (`recentFolded`, kept on this computer).
    recent_folded: bool,
    sessions: Vec<SessionSummary>,
    /// Whether the list came back once; until then the sidebar shows placeholders.
    sessions_loaded: bool,
    sessions_error: Option<SharedString>,
    /// The open session; `None` is a new chat, made on its first message.
    current: Option<String>,
    title: SharedString,
    entries: Vec<Entry>,
    /// The next entry's id, so an entry keeps its animation and hover group when others come and go.
    next_entry: usize,
    opening: bool,
    cached_chat: warm_chat::WarmChatRead,
    /// Chats left keep their built messages, and the sidebar's are read ahead (`kept_chats.rs`).
    kept_chats: kept_chats::KeptChats,
    notice: Option<SharedString>,
    /// The next turn id and the visible answer. Detached chats retain their own turn ids.
    turn: u64,
    asking: Option<u64>,
    detached_chats: HashMap<u64, live_chat::DetachedChat>,
    gateway_account_authority: Option<crate::backend::ExecutionAuthority>,
    pending_account_authority: Option<crate::backend::ExecutionAuthority>,
    chat_execution: Option<gateway_chat::Binding>,
    chat_execution_at: Option<u64>,
    /// What `entries` last showed of the run at `chat_execution_at`, so a token applies only what changed.
    chat_projected: Option<gateway_chat::Projected>,
    _chat_execution: Option<Subscription>,
    /// The window this workspace draws in, for work that starts where no `Window` is at hand (`gateway_chat`).
    window_handle: AnyWindowHandle,
    /// A passive callback waits for an already requested draw while the selected chat awaits its first token.
    first_token_armed: bool,
    /// Rewind's confirmation or the editor, open on one of the person's messages (`chat_rewind.rs`), the last rewind
    /// sent, whether it is an edit, and an edit's question until Gateway stores it.
    chat_rewind: Option<chat_rewind::ChatRewind>,
    rewind_ask: u64,
    rewind_edit: bool,
    edit_sent: Option<chat_rewind::EditSent>,
    /// Follow-ups queued while a chat answers (`chat_queue.rs`).
    chat_queue: chat_queue::ChatQueue,
    composer: Entity<TextareaState>,
    composer_empty: bool,
    /// Whether the composer has focus.
    composer_focused: bool,
    /// The composer's slash menu (`slash.rs`).
    slash: slash::SlashState,
    /// A line over the composer's text: dictation's notice or problem, or a note on what this build lacks.
    mic_note: Option<SharedString>,
    composer_note: Option<SharedString>,
    /// The images and files attached in the composer (`attachments.rs`), how many attaches are still being read, and
    /// whether files from outside are being dragged over the window.
    draft: crate::attachments::Draft,
    /// The key of the draft the composer holds (`drafts.rs`), and its text as last stored.
    draft_at: Option<drafts::DraftOwner>,
    draft_revision: u64,
    draft_memory_identity: uuid::Uuid,
    draft_attachments: Vec<(bool, u64)>,
    draft_text: SharedString,
    /// Comments on diff lines waiting in each Code session's draft, from Changes (`nexus_workbench::diff_comment`):
    /// chips over the composer, sent ahead of the text.
    diff_comments: HashMap<String, Vec<nexus_workbench::diff_comment::DiffComment>>,
    /// Passages of answers and their replies waiting over a Code session's composer (`stacked_replies.rs`).
    replies: stacked_replies::Replies,
    attaching: usize,
    /// A sent message's files while they go up in pieces (`uploads.rs`).
    uploading: Option<uploads::Uploading>,
    file_drag: bool,
    dictation: MicState,
    /// A next prompt Gateway suggested after the last answer: the composer's placeholder, and Tab takes it.
    suggestion: Option<SharedString>,
    /// The composer's placeholder as last set, to set it only when it changes.
    placeholder: SharedString,
    models: Vec<ModelChoice>,
    /// `GET /v1/spend` as last read, for the composer's pay notice (`pay_notice.rs`).
    spend: Option<serde_json::Value>,
    /// `GET /v1/people/me/budget` as last read: an owner's or admin's own month, for the sidebar's budget card.
    own_budget: Option<serde_json::Value>,
    /// The free start's price cap per million output tokens, for an owner: the model menu marks what it pays for.
    free_start_max: Option<f64>,
    /// `None` is Auto.
    model: Option<ModelChoice>,
    /// The model menu's search, the maker its rail shows, whether the older models are unfolded, and the row under
    /// the cursor.
    model_search: Entity<InputState>,
    model_query: String,
    model_maker: Option<String>,
    model_older: bool,
    model_cursor: usize,
    model_cursor_on_open: bool,
    model_visible_rows: Vec<String>,
    model_scroll: ScrollHandle,
    /// The search palette's state ([`search`]).
    palette_input: Entity<InputState>,
    palette_query: String,
    palette_cursor: usize,
    palette_hits: Vec<SearchHit>,
    /// The latest query's number; Gateway's results for older ones are dropped.
    palette_seq: u64,
    palette_searching: bool,
    palette_failed: bool,
    _palette_debounce: Option<Task<()>>,
    /// The artifact or page open in the Workbench's Artifacts tab (`page_panel.rs`).
    page: Option<page_panel::PagePanel>,
    /// Pages it showed lately, loaded but hidden, so going back to one is at once (`page_panel.rs`).
    kept_pages: Vec<page_panel::KeptPage>,
    /// A Code session's tools on the right (`side_pane.rs`).
    side: side_pane::SidePane,
    file_picker: file_picker::PickerState,
    /// The Agents tab's pull request section (`pull_card.rs`).
    pull_card: pull_card::PullCard,
    /// The sidebar and the main area as cached views (`pane.rs`).
    panes: Option<(Entity<pane::Pane>, Entity<pane::Pane>)>,
    /// Where the sidebar's pane put its moving marks, which the Workspace animates over it (`sidebar_marks.rs`).
    sidebar_marks: sidebar_marks::Marks,
    /// The cue on the list's edge to a waiting session out of its sight (`sidebar_edge_cue.rs`).
    sidebar_edge_cue: Entity<sidebar_edge_cue::EdgeCue>,
    /// The chat's and a Code session's transcripts as cached views, so a caret blink does not draw them (`pane.rs`).
    transcripts: pane::Transcripts,
    library: library_view::LibraryState,
    settings: settings_view::SettingsState,
    /// Setup lines' fixes run from this window: the way back from Settings and the terminal steps (`setup_fix.rs`).
    setup: setup_fix::FixState,
    /// The open page of `pages/`, as a place or a section of Settings.
    native_page: Option<(crate::pages::PageId, AnyView)>,
    models_page: models_view::ModelsState,
    library_search: Entity<InputState>,
    /// The library row being opened, until its page or text arrives.
    library_opening: Option<String>,
    /// A chat answer's page (id, version) being opened, until its frame arrives.
    page_opening: Option<(String, u32)>,
    /// A Code session's page in the Artifacts tab, and the event of its newest write when it opened: a later write
    /// makes its cards load it again.
    code_page_seen: Option<(String, i64)>,
    /// Whether that page opens on its own, beside a running chat: it does not start in full view.
    page_opening_auto: bool,
    inbox_state: inbox_view::InboxState,
    docs: docs_view::DocsState,
    issues: issues_view::IssuesState,
    /// Where the person has been, and where in that list they are (Back and Forward).
    history: Vec<Place>,
    history_at: usize,
    navigation_epoch: u64,
    navigation_retired: bool,
    navigation_restoring: bool,
    navigation_explicit: bool,
    navigation_sources: HashMap<usize, runs::Source>,
    navigation_scroll: Option<navigation::ScrollRestore>,
    #[cfg(test)]
    navigation_place_sink: Option<std::rc::Rc<std::cell::RefCell<Vec<crate::paths::LastPlace>>>>,
    sidebar_open: bool,
    sidebar_size: sidebar_resize::SidebarSize,
    /// How often the sidebar was toggled: its animation's id, so each toggle plays once and launch plays none.
    sidebar_toggles: usize,
    /// The sidebar collapsing onto the sign-in card, by its run (`startup.rs`); None once signed in.
    sign_in_collapse: Option<usize>,
    sign_in_collapses: usize,
    /// The entry whose copy button shows its check.
    copied: Option<usize>,
    _copied_reset: Option<Task<()>>,
    scroll: ScrollHandle,
    /// The open chat's messages as a list that lays out only the ones on screen (`transcript.rs`).
    chat_list: transcript::ChatList,
    /// Find in the open chat (`find_bar.rs`).
    find: find_bar::Find,
    /// The Workbench's open and maximized at the last frame: when they change, the transcript's width does.
    side_layout: (bool, bool),
    /// The chat column's width at the last frame (`chat_column.rs`).
    chat_column: chat_column::ChatColumn,
    /// How much the open Code session's top bar sheds to fit (`title_fit.rs`).
    title_fit: code_title::title_fit::TitleFit,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _events: Task<()>,
    _arrivals: Task<()>,
    /// Prompt links waiting for the account (`links.rs`), oldest first.
    pending_links: Vec<String>,
    /// A tray row asked of a window still signing in: done once it is ready (`shell.rs`).
    tray_pending: Option<shell::TrayPlace>,
    /// The launch's last place, a Code session of this computer, waiting for the store's first read in its scope
    /// (`last_place.rs`).
    last_place_waiting: Option<last_place::Waiting>,
    sign_in_form: Option<sign_in::SignInForm>,
    /// First-run onboarding: its saved state, the flow on screen and the welcome mark (`onboarding.rs`).
    onboarding: onboarding::Onboarding,
    /// What a prompt link asked for that could not be used, under the New session composer.
    link_note: Option<place_links::LinkNote>,
    /// The prompt stash's menu (`stash.rs`).
    stash_menu: stash::StashMenu,
    /// The plans on this computer, as the usage line under the New session composer reads them, and when.
    plan_usage: Option<(
        std::time::Instant,
        Result<Vec<nexus_workbench::plans::PlanUsage>, String>,
    )>,
    /// The agents whose logins on this computer the running reading has not read yet.
    plan_waiting: Vec<nexus_workbench::event::SessionAgent>,
    _plan_probe: Option<Task<()>>,
    plan_usage_host: Option<String>,
    /// When the selected Antigravity session last changed, by its host's clock, as the last plan reading started; None
    /// when none was selected or its turn ran then.
    plan_usage_turn: Option<i64>,
    /// The plan book's `changes` the composer's plans last showed (`plan_book.rs`).
    plan_book_seen: u64,
    /// What Auto would run for the new session ([`auto_route_preview`]).
    auto_route: auto_route_preview::AutoRoutePreview,
    /// A server's plans as known: its readings, newest per limit (`nexus_workbench::plan_book`).
    remote_plan_book: nexus_workbench::plan_book::PlanBook,
    _code_poll: Task<()>,
    /// Moves the working line's elapsed time while a session starts or works.
    _working_tick: Task<()>,
    _inbox_polls: [Task<()>; 2],
    /// The measured send (`measure_send.rs`), while measuring.
    measure: measure_send::MeasureStep,
}

impl Workspace {
    pub fn new(
        gateway: Arc<dyn Backend>,
        events: UnboundedReceiver<Event>,
        arrivals: UnboundedReceiver<crate::links::Arrival>,
        panel: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_machine_source(
            gateway,
            events,
            arrivals,
            panel,
            machine_source::MachineSource::configured(),
            window,
            cx,
        )
    }

    fn with_machine_source(
        gateway: Arc<dyn Backend>,
        events: UnboundedReceiver<Event>,
        arrivals: UnboundedReceiver<crate::links::Arrival>,
        panel: bool,
        source: machine_source::MachineSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let machines = source.machines;
        // The system's light or dark, kept for the next launch's first frame (`appearance.rs`).
        crate::appearance::seen(window.appearance());
        // The saved choice, not the system's appearance, from the first frame: a text view lays out with the kit's colours.
        apply_kit_theme(
            Palette::is_dark(settings_view::saved_theme(), window.appearance()),
            window,
            cx,
        );
        let sign_in_url = gateway.url().to_string();
        let update_toast = update_toast::UpdateToast::new(panel, cx);
        let revoked_notice = revoked_notice::RevokedNotice::new(panel, gateway.revoked(), cx);
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(PLACEHOLDER)
                .submit_on_enter(true)
                .auto_grow(1, 10)
        });
        let model_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search models"));
        let palette_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search or jump to…"));
        let library_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search titles and people"));
        let mut subscriptions = vec![
            // Enter sends; Shift+Enter is a new line.
            cx.subscribe_in(
                &composer,
                window,
                |this, state, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter {
                        shift: false,
                        secondary,
                    } => this.submit_now(*secondary, window, cx),
                    InputEvent::Change => {
                        this.sync_draft(window, cx);
                        this.chat_skills_typed(cx);
                        let empty = state.read(cx).value().trim().is_empty();
                        // A sleeping cloud machine starts waking at the first key, before the message is sent.
                        this.wake_on_typing(!empty);
                        if empty != this.composer_empty {
                            this.composer_empty = empty;
                            cx.notify();
                        }
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(&model_search, window, |this, state, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event
                    && picker_navigation::query_changed(&mut this.model_query, state.read(cx).value().as_ref())
                {
                    this.model_cursor = 0;
                    this.model_cursor_on_open = false;
                    this.model_visible_rows.clear();
                    this.model_scroll.set_offset(point(px(0.), px(0.)));
                    this.reveal_model_cursor();
                    cx.notify();
                }
            }),
            cx.subscribe_in(&palette_input, window, |this, state, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let query = state.read(cx).value().to_string();
                    if query != this.palette_query {
                        this.palette_typed(query, cx);
                    }
                }
            }),
            cx.subscribe_in(&library_search, window, |this, state, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.library.query = state.read(cx).value().to_string();
                    cx.notify();
                }
            }),
            // Focus in or out changes what the person looks at (`notify.rs`): draw again. A window that takes the focus
            // is where the tray, notifications and links go next (`windows.rs`).
            cx.observe_window_activation(window, |this, window, cx| {
                // The activity heartbeat goes only while a window has the focus (`usage.rs`).
                nexus_desktop_core::signals::focus(&usage::window_key(window), window.is_window_active());
                if this.panel {
                    // The panel closes when it loses the focus (`panel.rs`).
                    crate::panel::focus_changed(window.is_window_active(), cx);
                } else if window.is_window_active() {
                    crate::windows::focused(cx, window.window_handle().window_id());
                    this.activated(cx);
                }
                if !window.is_window_active() {
                    this.dismiss_cite(cx);
                }
                crate::motion::caret_follows_focus(window, cx);
                crate::web::app_focused(window.is_window_active() || cx.active_window().is_some());
                cx.notify()
            }),
            cx.observe_window_visibility(window, |this, _, window, cx| {
                this.sync_team_presence(window, cx);
                cx.notify();
            }),
            // The system went light or dark: the kit follows the choice, which may be System (`apply_kit_theme`).
            cx.observe_window_appearance(window, |this, window, cx| {
                crate::appearance::seen(window.appearance());
                apply_kit_theme(Palette::is_dark(this.theme, window.appearance()), window, cx);
                cx.notify();
            }),
        ];
        // A window that opens without the focus starts with a still caret.
        crate::motion::caret_follows_focus(window, cx);
        // Esc and Tab are read before any keybinding: the composer binds both for itself, and bindings run before
        // key listeners.
        // The prompt stash is every window's: a change in one shows in the others (`stash.rs`).
        stash::init(cx);
        drafts::init(cx);
        subscriptions.push(cx.observe_global::<drafts::ComposerDrafts>(|_, cx| cx.notify()));
        subscriptions.push(cx.observe_global::<stash::PromptStash>(|_, cx| cx.notify()));
        let this = cx.entity().downgrade();
        subscriptions.push(cx.intercept_keystrokes(move |event, window, cx| {
            nexus_desktop_core::signals::input(&usage::window_key(window));
            let _ = this.update(cx, |this, cx| this.on_keystroke(&event.keystroke, window, cx));
        }));
        let events_task = Self::listen(events, window, cx);
        // A later launch, with or without prompt links (`links.rs`).
        let arrivals_task = cx.spawn_in(window, async move |this, cx| {
            let mut arrivals = arrivals;
            while let Some(arrival) = arrivals.next().await {
                if this
                    .update_in(cx, |this, window, cx| this.arrived(arrival, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        // While the Nexus app runs the Code sessions, it writes them as they run: read them again every few seconds.
        // When this build runs them, its own events say when to read (`on_code_events`).
        let code_poll = cx.spawn(async move |this, cx| {
            // The store's files say when to read; the timer is the net under it (and the only source when the folder
            // can't be watched).
            // Native watcher callbacks wake from OS threads. Headless tests use the deterministic polling fallback.
            let watch = if cfg!(test) {
                None
            } else {
                crate::wakeups::store_changes()
            };
            let (_watcher, mut changes) = match watch {
                Some((watcher, changes)) => (Some(watcher), Some(changes)),
                None => (None, None),
            };
            loop {
                let net = if changes.is_some() { CODE_NET } else { CODE_POLL };
                let timer = cx.background_executor().timer(crate::wakeups::pace(net, cx));
                match &mut changes {
                    Some(changes) => {
                        futures::select_biased! {
                            changed = changes.next().fuse() => {
                                if changed.is_none() {
                                    break;
                                }
                                // A write comes as several events: take them as one.
                                cx.background_executor().timer(CODE_SETTLE).await;
                                while changes.try_recv().is_ok() {}
                            }
                            _ = timer.fuse() => {}
                        }
                    }
                    None => timer.await,
                }
                let read = this.update(cx, |this, cx| {
                    if this.runner.is_none() && !this.connection_open() {
                        this.refresh_code(cx)
                    }
                });
                if read.is_err() {
                    break;
                }
            }
        });
        let working_tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(crate::wakeups::pace(code_view::WORKING_TICK, cx))
                    .await;
                let ticked = this.update(cx, |this, cx| {
                    if this.connection_open() {
                        return;
                    }
                    if matches!(this.view, View::Code(_) | View::NewCode | View::Chat) {
                        this.probe_readiness(cx);
                    }
                    if matches!(&this.view, View::Code(view) if view.working_since.is_some())
                        || (matches!(&this.view, View::Code(_) | View::NewCode)
                            && this.plan_usage.as_ref().is_some_and(|(at, _)| {
                                at.elapsed() >= Duration::from_secs(60)
                                    || crate::plan_book::changes() != this.plan_book_seen
                            }))
                        || this.crew_clock_shown()
                    {
                        cx.notify();
                    }
                });
                if ticked.is_err() {
                    break;
                }
            }
        });
        // The unread count while signed in, and the Inbox's list while it is open.
        let count_poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(crate::wakeups::pace(INBOX_COUNT_POLL, cx))
                    .await;
                let alive = this.update(cx, |this, _| {
                    if let Some(account) = this.account_gateway() {
                        account.load_inbox();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let list_poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(crate::wakeups::pace(INBOX_LIST_POLL, cx))
                    .await;
                let alive = this.update(cx, |this, _| {
                    if matches!(this.view, View::Inbox) && matches!(this.auth, Auth::SignedIn) {
                        this.reload_inbox();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        // The servers: every window shows them, and the first one connects them (`machines.rs`).
        machines.set_gateway(&sign_in_url);
        let local_providers_opt_in = crate::local_providers_opt_in::get(cx).clone();
        if !local_providers_opt_in.enabled() && source.restore_sign_in {
            machines.sync(&crate::desktop_settings::load());
        }
        let watched_machines = machines.clone();
        let machines_task = cx.spawn(async move |this, cx| {
            let mut changes = watched_machines.subscribe();
            while let Some(change) = changes.next().await {
                // A burst of pushes is read once; streamed text alone only draws the open view again.
                let mut streamed = change.streamed;
                while let Ok(change) = changes.try_recv() {
                    streamed &= change.streamed;
                }
                let changed = this.update(cx, |this, cx| {
                    if streamed {
                        this.sync_stream(cx);
                    } else {
                        this.machines_changed(cx);
                    }
                });
                if changed.is_err() {
                    break;
                }
            }
        });
        let (relay, relayed) = futures::channel::mpsc::unbounded::<CodeEvent>();
        let relay_task = cx.spawn_in(window, async move |this, cx| {
            let mut relayed = relayed;
            while let Some(event) = relayed.next().await {
                if this
                    .update_in(cx, |this, window, cx| this.on_code_events(vec![event], window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        // Who is signed in, when `main.rs` already read it: the first frame is the workspace or the sign-in card.
        let remembered_local = !panel && local_providers_opt_in.enabled() && crate::paths::local_workspace_remembered();
        let first_auth = startup::first_auth(
            if source.restore_sign_in {
                gateway.saved_sign_in()
            } else {
                None
            },
            remembered_local,
        );
        if source.restore_sign_in {
            gateway.restore();
        }
        let this = cx.entity();
        let sidebar_pane = cx.new(|cx| pane::Pane::new(&this, pane::Region::Sidebar, cx));
        let center_pane = cx.new(|cx| pane::Pane::new(&this, pane::Region::Center, cx));
        let sidebar_filter = sidebar_filter::SidebarFilter::default();
        let sidebar_marks = sidebar_marks::Marks::new(&sidebar_filter.scroll, cx);
        let sidebar_edge_cue = sidebar_edge_cue::EdgeCue::new(&this, &sidebar_marks, &sidebar_filter.scroll, cx);
        let transcripts = pane::Transcripts::new(&this, &center_pane, cx);
        let mut workspace = Workspace {
            #[cfg(test)]
            browser_detach_runs: None,
            browser_return: None,
            #[cfg(test)]
            files_fixture_runs: None,
            files_return: None,
            commands_return: None,

            panel,
            gateway: gateway_access::GatewayAccess::new(gateway),
            auth: if matches!(first_auth, Auth::Local) {
                Auth::Restoring
            } else {
                first_auth
            },
            local_mode: local_mode::LocalMode {
                #[cfg(test)]
                isolated: !source.restore_sign_in,
                opt_in: local_providers_opt_in,
                remembered: remembered_local,
                ..Default::default()
            },
            #[cfg(test)]
            account_view_fixture: false,
            view: View::Chat,
            account: None,
            terminal_identity: None,
            had_account: false,
            team_presence: Default::default(),
            inbox: 0,
            agent: agent_configured(),
            theme: settings_view::saved_theme(),
            overlay: None,
            pr_form: None,
            panel_settings: None,
            main_chat: None,
            main_watch: Vec::new(),
            moving: None,
            choices: Default::default(),
            chat_skills: Default::default(),
            code: None,
            code_refresh: None,
            code_refresh_again: false,
            projects: Vec::new(),
            working_times: working_times::WorkingTimes::default(),
            peek: session_peek::Peek::default(),
            hovered_chat: None,
            code_error: None,
            code_overview_loaded: false,
            new_session_unsettled: false,
            runner: None,
            #[cfg(test)]
            run_fixtures: HashMap::new(),
            machine_source: machines.clone(),
            servers: machines.servers(),
            server_retries: HashMap::new(),
            cloud_machines: machines.cloud(),
            cloud_ui: cloud_view::CloudUi::new(window, cx),
            relay,
            _relay: relay_task,
            _machines: machines_task,
            owner: Owner::Unknown,
            code_scope: None,
            _take_over: None,
            server_folder: None,
            code_agents: Vec::new(),
            code_form: new_code::CodeForm::default(),
            launch_motion: Default::default(),
            readiness: Default::default(),
            claude_trust: Default::default(),
            composer_layout: Default::default(),
            dock_height: Default::default(),
            crash_prompt: Default::default(),
            usage_prompt: Default::default(),
            feedback: Default::default(),
            update_toast,
            revoked_notice,
            update_restart: Default::default(),
            notices: Default::default(),
            notice_motion: Default::default(),
            closing_notice: None,
            folded_projects: HashSet::new(),
            groups: Default::default(),
            project_hover_motion: HashMap::new(),
            project_fold_motion: HashMap::new(),
            session_anchor: None,
            custom_snooze: None,
            row_exits: Default::default(),
            session_drag: Default::default(),
            project_drag: Default::default(),
            sidebar_hold: Default::default(),
            chat_share: None,
            session_share: None,
            followed_shares: HashMap::new(),
            chat_rename: None,
            chat_archived: false,
            archived_open: false,
            archived_fold_motion: folder_motion::Transition::settled(0.),
            archived_chats: None,
            archived_code: Vec::new(),
            renaming: None,
            fork_points: None,
            forking: None,
            open_aside: HashSet::new(),
            aside_fold_motion: HashMap::new(),
            history_hover_motion: HashMap::new(),
            clone_form: None,
            project_dialog: None,
            artifact_picker: None,
            menu_side: Default::default(),
            backdrop: backdrop::Choice::load(),
            backdrop_motion: Default::default(),
            sessions_step: Default::default(),
            recent_folded: crate::paths::recent_folded(),
            sessions: Vec::new(),
            sessions_loaded: false,
            sessions_error: None,
            current: None,
            title: "New chat".into(),
            entries: Vec::new(),
            next_entry: 0,
            opening: false,
            cached_chat: Default::default(),
            kept_chats: Default::default(),
            notice: None,
            turn: 0,
            asking: None,
            detached_chats: HashMap::new(),
            gateway_account_authority: None,
            pending_account_authority: None,
            chat_execution: None,
            chat_execution_at: None,
            chat_projected: None,
            _chat_execution: None,
            window_handle: window.window_handle(),
            first_token_armed: false,
            chat_rewind: None,
            rewind_ask: 0,
            rewind_edit: false,
            edit_sent: None,
            chat_queue: chat_queue::ChatQueue::new(window, cx),
            composer,
            composer_empty: true,
            composer_focused: false,
            slash: Default::default(),
            mic_note: None,
            composer_note: None,
            draft: Default::default(),
            draft_at: None,
            draft_revision: 0,
            draft_memory_identity: uuid::Uuid::new_v4(),
            draft_attachments: Vec::new(),
            draft_text: SharedString::default(),
            diff_comments: HashMap::new(),
            replies: stacked_replies::Replies::new(window, cx),
            attaching: 0,
            uploading: None,
            file_drag: false,
            dictation: MicState::new(),
            suggestion: None,
            placeholder: PLACEHOLDER.into(),
            models: Vec::new(),
            spend: None,
            own_budget: None,
            free_start_max: None,
            model: None,
            model_search,
            model_query: String::new(),
            model_maker: None,
            model_older: false,
            model_cursor: 0,
            model_cursor_on_open: false,
            model_visible_rows: Vec::new(),
            model_scroll: ScrollHandle::new(),
            sidebar_filter,
            palette_input,
            palette_query: String::new(),
            palette_cursor: 0,
            palette_hits: Vec::new(),
            palette_seq: 0,
            palette_searching: false,
            palette_failed: false,
            _palette_debounce: None,
            page: None,
            kept_pages: Vec::new(),
            side: side_pane::SidePane::new(),
            file_picker: file_picker::PickerState::default(),
            pull_card: pull_card::PullCard::new(cx),
            panes: Some((sidebar_pane, center_pane)),
            sidebar_marks,
            sidebar_edge_cue,
            transcripts,
            library: library_view::LibraryState::new(),
            settings: settings_view::SettingsState::new(window, cx),
            setup: Default::default(),
            native_page: None,
            assistant: Default::default(),
            accounts: Default::default(),
            models_page: models_view::ModelsState::new(window, cx),
            library_search,
            library_opening: None,
            page_opening: None,
            code_page_seen: None,
            page_opening_auto: false,
            inbox_state: inbox_view::InboxState::new(),
            docs: docs_view::DocsState::new(),
            issues: issues_view::IssuesState::new(),
            history: vec![Place::Chat(None)],
            history_at: 0,
            navigation_epoch: 0,
            navigation_retired: false,
            navigation_restoring: false,
            navigation_explicit: false,
            navigation_sources: HashMap::new(),
            navigation_scroll: None,
            #[cfg(test)]
            navigation_place_sink: None,
            sidebar_open: !panel,
            sidebar_size: sidebar_resize::SidebarSize::new(cx),
            sidebar_toggles: 0,
            sign_in_collapse: None,
            sign_in_collapses: 0,
            copied: None,
            _copied_reset: None,
            scroll: ScrollHandle::new(),
            chat_list: transcript::ChatList::new(),
            find: Default::default(),
            side_layout: (false, false),
            chat_column: Default::default(),
            title_fit: Default::default(),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
            _events: events_task,
            _arrivals: arrivals_task,
            // The launch's links are an app window's.
            pending_links: if panel {
                Vec::new()
            } else {
                crate::links::launch_links()
            },
            tray_pending: None,
            last_place_waiting: None,
            link_note: None,
            stash_menu: Default::default(),
            sign_in_form: Some(sign_in::SignInForm::new(&sign_in_url, window, cx)),
            plan_usage: None,
            plan_waiting: Vec::new(),
            _plan_probe: None,
            plan_usage_host: None,
            plan_usage_turn: None,
            plan_book_seen: 0,
            auto_route: Default::default(),
            remote_plan_book: Default::default(),
            _code_poll: code_poll,
            _working_tick: working_tick,
            _inbox_polls: [count_poll, list_poll],
            measure: if panel {
                measure_send::MeasureStep::Done
            } else {
                Default::default()
            },
            onboarding: onboarding::Onboarding::new(cx),
        };
        if remembered_local
            && matches!(workspace.auth, Auth::Restoring)
            && workspace.gateway.saved_sign_in() == Some(false)
        {
            workspace.enter_local(local_mode::Entry::Launch, window, cx);
        }
        // Signed out at launch: the workspace's sidebar collapses onto the sign-in card (`startup.rs`).
        if matches!(workspace.auth, Auth::SignedOut { .. }) {
            workspace.collapse_to_sign_in();
        }
        // Signed in at launch: a first-run flow left half way resumes (`onboarding.rs`).
        if matches!(workspace.auth, Auth::SignedIn) {
            workspace.onboarding_at_launch();
        }
        // For trying the Artifacts tab on any address: `NEXUS_WEB_URL=https://... nexus-desktop-gpui`.
        if let Ok(url) = std::env::var("NEXUS_WEB_URL")
            && !url.is_empty()
            && !panel
            && workspace.account_gateway().is_some()
        {
            let frame = crate::gateway::PageFrame {
                url,
                title: None,
                path: String::new(),
                version_key: None,
                mint: None,
            };
            workspace.open_page(frame, window, cx);
        }
        workspace
    }

    fn handle(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        use std::sync::atomic::Ordering::SeqCst;
        let event = match event {
            Event::SignInCode { ref cancelled, .. }
                if cancelled.load(SeqCst) || !matches!(self.auth, Auth::Checking) =>
            {
                return;
            }
            Event::DeviceSignInFailed { cancelled, error } => {
                if cancelled.load(SeqCst) || !matches!(self.auth, Auth::Checking | Auth::Waiting { .. }) {
                    return;
                }
                Event::SignInFailed(error)
            }
            event => event,
        };
        if self.is_local() && matches!(event, Event::SignInCode { .. } | Event::SignInFailed(_)) {
            return;
        }
        if self.handle_detached_chat(&event, cx) {
            return;
        }
        if live_chat::event_turn(&event) == self.asking && self.asking.is_some() {
            live_chat::update_entries(&mut self.entries, &event, cx);
        }
        // A delta changes only the answer's text: the sidebar and the tray menu stay as they are.
        let streamed = matches!(event, Event::Delta { .. });
        match event {
            Event::ReadSnapshot {
                epoch,
                expected,
                event,
                revision,
            } => {
                if epoch.load(std::sync::atomic::Ordering::SeqCst) == expected {
                    if revision.as_ref().is_none_or(|(version, expected)| {
                        version.load(std::sync::atomic::Ordering::SeqCst) == *expected
                    }) {
                        if matches!(&*event, Event::Account(_)) {
                            self.pending_account_authority =
                                Some(crate::backend::ExecutionAuthority::from_read(epoch.clone(), expected));
                        }
                        self.handle(*event, window, cx);
                    } else {
                        // A read can wait in the UI queue after a local write completes.
                        // Reload the active dataset rather than replaying its old snapshot.
                        let Some(account_gateway) = self.account_gateway() else {
                            return;
                        };
                        match *event {
                            Event::Account(_) => account_gateway.load_account(),
                            Event::Models(_) | Event::Catalog(_) => account_gateway.list_models(),
                            Event::Spend(_) | Event::OwnBudget(_) | Event::FreeStartMax(_) => {
                                account_gateway.load_pays()
                            }
                            Event::Providers(_) => account_gateway.list_providers(),
                            Event::Sessions(_) => account_gateway.list_sessions(),
                            Event::OrgSettings(_) => account_gateway.load_org_settings(),
                            Event::Inbox(_) => account_gateway.load_inbox(),
                            Event::NotificationSettings(_) => account_gateway.load_notification_settings(),
                            Event::Tokens(_) => account_gateway.list_tokens(),
                            Event::Notifications { .. } if matches!(self.view, View::Inbox) => self.reload_inbox(),
                            Event::Library(_) if matches!(self.view, View::Library) => {
                                account_gateway.load_library(self.account.as_ref().map(|account| account.org.clone()))
                            }
                            // The open page's ⋯ waits on the lists too.
                            Event::Library(_) if self.page.is_some() => self.reload_library(),
                            Event::ChatRead { id, .. }
                                if self.current.as_deref() == Some(id.as_str()) && self.asking.is_none() =>
                            {
                                self.read_chat(id)
                            }
                            _ => {}
                        }
                    }
                }
            }
            Event::ChatRead { read, id, cached, chat } => {
                if self.chat_execution_at.is_some() && self.chat_binding_current(cx) {
                    let apply = self.cached_chat.receive(&id, read, cached);
                    if !cached && !matches!(apply, warm_chat::Apply::Ignore) {
                        if let Ok(chat) = &chat {
                            self.gateway_chat_metadata(&id, chat, cx);
                        }
                    }
                    return;
                }
                if self.current.as_deref() == Some(id.as_str()) && self.asking.is_none() {
                    match self.cached_chat.receive(&id, read, cached) {
                        warm_chat::Apply::Replace => {
                            self.opening = true;
                            self.handle(Event::Opened(chat), window, cx);
                        }
                        warm_chat::Apply::Metadata => {
                            if let Ok(chat) = chat {
                                transcript::fill_meta(&mut self.entries, &chat.messages);
                            }
                        }
                        warm_chat::Apply::Ignore => {}
                    }
                }
            }
            Event::SignedIn => {
                self.gateway.accept_sign_in();
                let came_from_local = self.is_local() || self.local_mode.signing_in;
                self.discard_private_terminals(cx);
                // Another sign-in or organization: the pages kept loaded are the last one's.
                self.drop_kept_pages();
                // A new sign-in over an account, as another window's organization switch: nothing of the old one stays.
                if self.account.is_some() {
                    self.leave_org();
                }
                if came_from_local {
                    self.leave_local(Auth::SignedIn, window, cx);
                    self.onboarding_after_sign_in();
                } else {
                    let before = std::mem::replace(&mut self.auth, Auth::SignedIn);
                    self.auth_moved(&before);
                }
                // What waited on a Nexus sign-in goes on once this sign-in's account is read (`nexus_sign_in.rs`).
                self.setup.signed_in = true;
                if let Some(account) = self.account_gateway() {
                    account.update_release_access();
                    account.update_workspace_services(self.runner.is_some() && self.byo_entry());
                    #[cfg(test)]
                    let isolated = self.local_mode.isolated;
                    #[cfg(not(test))]
                    let isolated = false;
                    if !isolated && (!self.panel || !self.local_mode.opt_in.enabled()) {
                        account.sync_machines(&self.machine_source, &crate::desktop_settings::load());
                    }
                }
                // A page built for an older credential must be rebuilt when the new account arrives.
                if self.byo_entry() {
                    self.settings.shell.views.clear();
                    self.native_page = None;
                    if let View::Settings(section) = self.view {
                        self.open_settings(section, window, cx);
                    }
                }
                self.move_to_pinned_org(cx);
                // The last run's crash waits for a sign-in that can send it, in an app window.
                if !self.panel {
                    self.take_crash(window, cx);
                }
                if let Some(account) = self.account_gateway() {
                    account.list_sessions();
                    account.list_models();
                    account.load_pays();
                    account.load_account();
                    account.load_inbox();
                }
                // When Gateway vouches for the person's devices, this computer registers itself now, with no visit to
                // Settings › Devices. Tests' fake backends do not serve the device directory.
                #[cfg(not(test))]
                if !self.panel
                    && let Some(account) = self.account_gateway()
                {
                    let key_changed = self.runner.as_ref().map(|runner| runner.device_key_changed());
                    crate::pages::register_this_computer(account, key_changed, cx);
                }
                // A recent chat goes now; the other rows wait for the account.
                self.tray_ready(window, cx);
                // Until the account says which organization, use the one the Nexus app was last in.
                if self.code.is_none()
                    && !came_from_local
                    && self
                        .runner
                        .as_ref()
                        .is_none_or(|runner| runner.service_handoff() == crate::code_run::ServiceHandoff::Ready)
                    && let Some(org) = crate::desktop_settings::load().org
                {
                    self.open_code_store(&org, window, cx);
                }
            }
            Event::Account(account) => {
                let authority = self.pending_account_authority.take();
                self.accept_navigation_identity(&account, cx);
                self.gateway_account_authority = authority.filter(|authority| authority.current());
                // Another organization with no `SignedIn` before it: leave the old one here, and read the new one's.
                if self.account.as_ref().is_some_and(|before| before.org != account.org) {
                    self.leave_org();
                    self.org_pays_changed();
                    if let Some(account) = self.account_gateway() {
                        account.list_sessions();
                        account.load_inbox();
                    }
                } else if self
                    .account
                    .as_ref()
                    .is_some_and(|before| before.user_id != account.user_id || before.person_id != account.person_id)
                {
                    self.onboarding.forget_plans();
                }
                // A view fixture still accepts the real read envelope and Account identity above.
                // It does not start local Code resources or background account datasets.
                #[cfg(test)]
                if self.account_view_fixture {
                    self.account = Some(account);
                    self.had_account = true;
                    return;
                }
                // Another organization, or the first account after a sign-out or a Gateway switch (`clear_session`).
                let new_owner =
                    cloud_view::new_owner(self.account.as_ref().map(|before| before.org.as_str()), &account.org);
                // An organization switch is another sign-in for the cloud machines.
                if let Some(account) = self.account_gateway() {
                    account.update_workspace_services(false);
                }
                self.open_code_store(&account.org.clone(), window, cx);
                let before = self.account.as_ref().map(|account| account.org.clone());
                self.account_read(before.as_deref(), &account.org);
                // A chat of the organization left behind is not this one's: sent in, it would go to the old session.
                // The window lands as an idle one does (`last_place::landing`).
                if self.had_account
                    && before.as_deref() != Some(account.org.as_str())
                    && matches!(self.view, View::Chat)
                    && self.current.is_some()
                {
                    self.reset_chat();
                }
                let idle = matches!(self.view, View::Chat) && self.current.is_none();
                // The first account of the launch, which the dev hook below is for.
                static OPENED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                let launch = !OPENED.swap(true, std::sync::atomic::Ordering::Relaxed);
                let land = last_place::landing(
                    before.as_deref(),
                    &account.org,
                    idle,
                    !self.panel && (launch || self.had_account),
                );
                self.load_groups(&account.org);
                if before.as_deref() != Some(account.org.as_str()) {
                    self.enter_org_accounts(&account.org, cx);
                }
                self.account = Some(account);
                // Who may fix what, for the setup lines (`setup_fix.rs`).
                self.read_setup_access(cx);
                self.open_onboarding_plan(window, cx);
                // The Assistant is the account's own: its row reads this account's marks.
                self.assistant_account(cx);
                // The switcher's rows and the other accounts' Assistants (`account_states.rs`).
                self.read_account_orgs(cx);
                self.start_account_polls(cx);
                // After the account is the new organization's: the reload asks for its repositories and its answer is
                // kept (`load_cloud_repos` guards on the account that is current when it answers).
                if new_owner {
                    self.reset_cloud_repos(window, cx);
                }
                // Not at a window's first account: its view reads what it needs as it opens.
                if self.had_account && before.as_deref() != self.account.as_ref().map(|account| account.org.as_str()) {
                    self.org_entered(cx);
                }
                self.had_account = true;
                match self.view {
                    View::Settings(section) => self.rebind_navigation_settings(section, window, cx),
                    View::Page(id) if self.native_page.is_none() => self.open_native(id, window, cx),
                    _ => {}
                }
                self.load_diff_comments();
                // For checking a page without clicking to it: `NEXUS_OPEN_PAGE=settings/members nexus-desktop-gpui`,
                // `code/new` for a new Code session, or `agent?c=<id>` for the Assistant on a conversation. Read once.
                let restored_primary = self.restore_primary_terminals(window, cx);
                let page = launch.then(|| std::env::var("NEXUS_OPEN_PAGE").ok()).flatten();
                match page {
                    Some(path) if path == "code/new" => self.open_new_code(window, cx),
                    // `pull/<session id>`: a Code session's pull request page.
                    Some(path) if path.starts_with("pull/") => self.open_pull(&path["pull/".len()..], window, cx),
                    // The Workbench's Browser with a tab per address: `browser?url=<a>&url=<b>`.
                    Some(path) if path.starts_with("browser?") => self.open_side_browser(&path, window, cx),
                    // The Models view on a tab: `models` or `models/providers`.
                    Some(path) if path == "models" || path == "models/providers" => {
                        if path.ends_with("/providers") {
                            self.models_page.tab = models_view::ModelsTab::Providers;
                        }
                        self.open_models(window, cx)
                    }
                    // The Assistant on a conversation, as a notification's click opens it: `agent?c=<id>`.
                    Some(path) if path.starts_with("agent?c=") => {
                        let id = path.trim_start_matches("agent?c=").to_string();
                        self.open_agent(Some(id), window, cx)
                    }
                    // The search palette with a query typed: `palette?q=dark mode`.
                    Some(path) if path.starts_with("palette?q=") => {
                        let query = path["palette?q=".len()..].to_string();
                        self.open_palette(window, cx);
                        self.palette_input
                            .update(cx, |state, cx| state.set_value(query.clone(), window, cx));
                        self.palette_typed(query, cx);
                    }
                    Some(path) if path.starts_with("settings?q=") => {
                        let query = path["settings?q=".len()..].to_string();
                        self.open_settings(SettingsSection::Appearance, window, cx);
                        self.settings
                            .shell
                            .query
                            .update(cx, |state, cx| state.set_value(query.clone(), window, cx));
                        self.settings.shell.query_text = query;
                        self.settings.shell.searching = true;
                        self.settings.shell.results_selected.set(Some(0));
                    }
                    Some(path) if crate::pages::skills::Target::parse(&path).is_some() => {
                        if let Some(target) = crate::pages::skills::Target::parse(&path) {
                            self.open_skills_target(target, window, cx);
                        }
                    }
                    Some(path) if settings_registry::resolve_field(&path).is_some() => {
                        if let Some((section, anchor)) = settings_registry::resolve_field(&path) {
                            self.open_settings_field(section, anchor, window, cx);
                        }
                    }
                    Some(path) if settings_registry::resolve(&path).is_some() => {
                        if let Some(section) = settings_registry::resolve(&path) {
                            self.open_settings(section, window, cx);
                        }
                    }
                    Some(path) if crate::pages::PageId::from_web_path(&path).is_some() => {
                        if let Some(id) = crate::pages::PageId::from_web_path(&path) {
                            self.open_native(id, window, cx);
                        }
                    }
                    _ if !self.pending_links.is_empty() => {
                        for url in std::mem::take(&mut self.pending_links) {
                            self.open_launch_link(&url, window, cx);
                        }
                    }
                    // A tray row asked of the window while it signed in (`shell.rs`), in place of its landing.
                    _ if self.tray_ready(window, cx) => {}
                    // After a sign-in to another Gateway or organization, the window opens on its last place
                    // there (`last_place.rs`), else on New session, which takes the remembered project.
                    // A switch with the Assistant open lands on the same view of this account.
                    _ if self.land_after_switch(window, cx) => {}
                    _ if restored_primary => {}
                    _ => match land {
                        Some(last_place::Landing::LastPlace) => {
                            if !self.open_last_place(window, cx) {
                                self.start_new_session(window, cx)
                            }
                        }
                        Some(last_place::Landing::NewSession) => self.start_new_session(window, cx),
                        None => {}
                    },
                }
                // Links that came to a window opened later (`windows.rs`), once its account is known.
                for url in std::mem::take(&mut self.pending_links) {
                    self.open_launch_link(&url, window, cx);
                }
                self.account_after_sign_in(window, cx);
                // The audience labels name the organization.
                if matches!(self.view, View::Library) {
                    self.reload_library();
                }
                self.measure_ready(cx);
            }
            Event::Inbox(count) => self.inbox = count,
            Event::Notifications { request, page } => self.inbox_loaded(request, page),
            Event::NotificationMarked(item) => self.notification_marked(item),
            Event::NotificationsRead(items) => self.notifications_read(items),
            Event::DocsIndex(sections) => self.docs_index_loaded(sections),
            Event::Docs(page) => self.docs_page_loaded(page, cx),
            Event::SignedOut => {
                crate::updates::session(self.gateway.url(), None);
                if self.is_local() {
                    return;
                }
                if self.local_mode.signing_in {
                    self.finish_sign_in_attempt(None, window, cx);
                    return;
                }
                self.discard_private_terminals(cx);
                self.side.side_chats.clear();
                self.side.side_chat_for.clear();
                self.side.side_chat_visible = false;
                // The pictures of this sign-in's pages go with it, and the pages kept loaded.
                crate::web::WebView::clear_snapshots();
                self.drop_kept_pages();
                self.chat_skills = Default::default();
                self.chat_share = None;
                let before = std::mem::replace(&mut self.auth, Auth::SignedOut { error: None });
                self.refresh_local_choice();
                if !self.local_mode.remembered {
                    self.auth_moved(&before);
                }
                // The next sign-in opens the home, as the Nexus app's does.
                crate::paths::forget_last_place();
                if !self.local_mode.opt_in.enabled()
                    && let Some(runner) = &self.runner
                {
                    runner.signed_out();
                }
                self.clear_account_services();
                self.leave_draft_identity(window, cx);
                self.clear_session(cx);
                if self.byo_entry() {
                    self.local_mode.signing_in = false;
                    self.enter_local(local_mode::Entry::SignedOut, window, cx);
                }
                // Started again on a Gateway picked at sign-in (`sign_in.rs`): go on signing in there.
                static RESUMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if std::env::args().any(|arg| arg == "--sign-in")
                    && !RESUMED.swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    if self.is_local() {
                        self.sign_in_from_local(window, cx);
                    } else {
                        self.auth = Auth::Checking;
                        self.gateway.begin_device_sign_in();
                    }
                }
            }
            Event::SignInCode {
                user_code,
                link,
                expires_in,
                ..
            } => self.sign_in_code(user_code, link, expires_in, window, cx),
            Event::DeviceSignInFailed { .. } => unreachable!("device failures were checked before dispatch"),
            Event::SwitchFailed(error) => {
                self.notice = Some(format!("Couldn't switch: {error}").into());
                self.switch_failed();
                if let Some(account) = self.account_gateway() {
                    account.load_account();
                    account.list_sessions();
                }
            }
            Event::SignInFailed(error) => {
                if !self.local_mode.signing_in {
                    self.discard_private_terminals(cx);
                }
                self.finish_sign_in_attempt(Some(error), window, cx);
            }
            Event::Sessions(Ok(sessions)) => {
                self.sessions = sessions;
                self.sessions_loaded = true;
                self.sessions_error = None;
                self.measure_ready(cx);
                self.preload_chats();
            }
            Event::Sessions(Err(error)) => {
                self.sessions_loaded = true;
                self.sessions_error = Some(error.into());
            }
            Event::Models(models) => {
                // Refresh the selected entry too, so a removed or disabled choice falls back to Auto.
                self.model = self
                    .model
                    .as_ref()
                    .and_then(|picked| chat_picker::pickable(&models, &picked.id))
                    .cloned();
                self.models = models;
                self.capture_gateway_choices(cx);
            }
            // The chat is already open: Gateway's copy after a stream ended, for the answer's quiet line.
            Event::Opened(Ok(chat))
                if self.current.as_deref() == Some(chat.id.as_str()) && !self.opening && !self.entries.is_empty() =>
            {
                if self.asking.is_none() && transcript::fill_meta(&mut self.entries, &chat.messages) {
                    cx.notify();
                }
            }
            Event::Opened(Ok(chat)) if self.current.as_deref() == Some(chat.id.as_str()) => {
                self.chat_opened(&chat);
                self.moved(&chat);
                self.chat_archived = chat.archived;
                // Back to a chat whose messages are as they were: its built entries again, not a new build.
                if !self.kept_chats.matches(&chat.id, &chat.messages, &self.entries) {
                    self.entries = match self.kept_chats.take(&chat.id, &chat.messages) {
                        Some(entries) => entries,
                        None => self.entries_from(chat.messages.clone(), cx),
                    };
                }
                self.kept_chats.shown(&chat.id, chat.messages, &self.entries);
                self.opening = false;
                self.scroll.scroll_to_bottom();
                self.measure_opened(&chat.id, window, cx);
            }
            Event::Opened(Ok(_)) => {}
            Event::Opened(Err(error)) => {
                self.opening = false;
                self.notice = Some(error.into());
            }
            Event::Created { turn, id, title } if self.asking == Some(turn) => {
                let draft_owner = self.draft_owner(cx);
                self.chat_queue_named(&id);
                self.current = Some(id.clone());
                self.file_new_chat(&id);
                self.promote_chat_draft(draft_owner, cx);
                self.title = title.clone().into();
                self.sessions.insert(
                    0,
                    SessionSummary {
                        id,
                        title,
                        day: Some(crate::model::today()),
                        at_ms: Some(crate::code::now_ms()),
                    },
                );
            }
            Event::Asked { turn, .. } if self.asking == Some(turn) => {
                self.edit_sent.take_if(|sent| sent.turn == turn);
                self.chat_question_stored(turn);
            }
            Event::Rewound { ask, outcome } => self.chat_rewound(ask, outcome, window, cx),
            Event::Delta { turn, .. } if self.asking == Some(turn) => {
                self.measure_delta(turn);
                // Follow the answer only while the reader is at the bottom; scrolling up to read stays put.
                let follow = transcript::near_bottom(&self.scroll);
                if follow {
                    self.scroll.scroll_to_bottom();
                }
            }
            Event::WritingPage { turn } if self.asking == Some(turn) => {
                // The page opens on its own when the answer ends: start the web engine meanwhile.
                crate::web::warm();
            }
            Event::SearchingWeb { turn } if self.asking == Some(turn) => {}
            Event::OrgSettings(settings) => {
                if settings.is_some() {
                    self.choices.org_settings = settings;
                    self.capture_gateway_choices(cx);
                }
            }
            Event::Answered { turn, page, .. } if self.asking == Some(turn) => {
                // As Nexus UI: a page the answer wrote opens in the Workbench's Artifacts tab, or moves it to its new
                // version.
                if let Some(page) = &page {
                    self.open_page_block(page, true, cx);
                }
                self.asking = None;
                // Once, read-only: the stored message has the model, cost and tokens the stream did not carry.
                if let Some(id) = self.current.clone() {
                    self.read_chat(id);
                }
                // Gateway orders the list by activity and may have renamed the session.
                if let Some(account) = self.account_gateway() {
                    account.list_sessions();
                    if let Some(id) = self.current.clone() {
                        account.suggest(id);
                    }
                    // The answer's spend can bring the free start's end near.
                    account.load_pays();
                }
                self.measure_answered(turn, true, window, cx);
                self.chat_answer_ended(turn, nexus_workbench::chat_queue::Outcome::Complete, cx);
            }
            Event::AnswerFailed { turn, error } if self.asking == Some(turn) => {
                self.asking = None;
                // A missing key: the answer is asked again once the key lands (`no_key.rs`).
                self.hold_chat_for_key(&error, cx);
                // A refusal can change what Nexus pays: the notice over the composer reads it again.
                if let Some(account) = self.account_gateway() {
                    account.load_pays();
                }
                self.edit_not_sent(turn, window, cx);
                self.measure_answered(turn, false, window, cx);
                self.chat_answer_ended(turn, nexus_workbench::chat_queue::Outcome::Failed, cx);
            }
            Event::Suggested { session, suggestion } => {
                // Only for the chat still open, and only while nothing is asked or typed.
                if self.current.as_deref() == Some(session.as_str()) && self.asking.is_none() {
                    self.suggestion = Some(suggestion.into());
                }
            }
            Event::Dictation { id, event } => self.on_dictation(id, event, window, cx),
            Event::Searched { seq, results } if seq & sidebar_filter::SEQ_BIT != 0 => {
                self.filter_searched(seq, results, cx)
            }
            Event::Searched { seq, results } if seq == self.palette_seq => {
                self.palette_searching = false;
                match results {
                    Ok(hits) => {
                        self.palette_hits = hits;
                        self.palette_failed = false;
                    }
                    Err(_) => {
                        self.palette_hits.clear();
                        self.palette_failed = true;
                    }
                }
            }
            Event::Searched { .. } => {}
            Event::Framed(Ok(frame)) => {
                crate::measure::page_mark("framed");
                // The panel takes the Library row it came from, for its ⋯; a page refused here clears it.
                self.open_page(frame, window, cx);
                self.library_opening = None;
            }
            Event::Framed(Err(error)) => {
                self.page_opening = None;
                self.page_opening_auto = false;
                let note: SharedString = format!("The artifact did not open: {error}").into();
                if self.library_opening.take().is_some() {
                    self.library.note = Some(note);
                } else {
                    self.composer_note = Some(note);
                }
            }
            Event::Library(lists) => self.library_loaded(lists),
            Event::ChatText(Ok(chat)) => self.open_chat_text(chat, cx),
            Event::ChatText(Err(error)) => {
                self.library_opening = None;
                self.library.note = Some(format!("The chat did not open: {error}").into());
            }
            Event::Catalog(models) => self.models_page.models = Some(models),
            Event::Spend(spend) => {
                if spend.is_some() {
                    self.spend = spend;
                }
            }
            Event::OwnBudget(own) => {
                if own.is_some() {
                    self.own_budget = own;
                }
            }
            Event::FreeStartMax(cap) => self.free_start_max = cap,
            // A failed refresh keeps a list already read.
            Event::Providers(Err(_)) if matches!(self.models_page.providers, Some(Ok(_))) => {}
            Event::Providers(providers) => self.models_page.providers = Some(providers),
            Event::NotificationSettings(channels) => self.channels_loaded(channels, cx),
            Event::Tokens(tokens) => self.tokens_loaded(tokens),
            Event::TokenCreated(made) => self.token_created(made, window, cx),
            Event::Created { .. }
            | Event::Asked { .. }
            | Event::Delta { .. }
            | Event::WritingPage { .. }
            | Event::SearchingWeb { .. }
            | Event::Answered { .. }
            | Event::AnswerFailed { .. } => {}
        }
        if streamed {
            self.notify_main(cx);
            return;
        }
        self.update_code_blocker();
        self.sync_tray(cx);
        cx.notify();
    }

    /// Read `events` from the backend and handle each on the UI thread, until the backend or this view goes.
    fn listen(events: UnboundedReceiver<Event>, window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            let mut events = events;
            while let Some(event) = events.next().await {
                if this
                    .update_in(cx, |this, window, cx| this.handle(event, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    pub(crate) fn discard_private_terminals(&mut self, cx: &mut Context<Self>) {
        self.revoke_gateway_chat();
        self.gateway_account_authority = None;
        self.pending_account_authority = None;
        if self.terminal_identity.is_some() {
            self.clear_navigation();
        }
        self.side.clear_files(false, cx);
        self.side.clear_commands(false, cx);
        self.side.clear_browsers(cx);
        self.side.clear_side_chats(cx);
        self.side.clear_private_terminals();
        self.terminal_identity = None;
    }

    /// Called after native closure, while the registry still owns this Workspace.
    pub(crate) fn retain_primary_terminals(&mut self, cx: &mut Context<Self>) -> Option<RetainedPrimary> {
        let identity = self.terminal_identity.clone()?;
        if !matches!(self.auth, Auth::SignedIn)
            || !identity.matches_credential(self.gateway.url(), self.gateway.marker().as_deref())
        {
            return None;
        }
        let execution = self.retain_gateway_chat(cx);
        let navigation = self.retain_navigation();
        Some(RetainedPrimary {
            identity,
            execution,
            navigation,
            new_code: matches!(self.view, View::NewCode).then(|| RetainedNewCode {
                project: self.code_form.project.clone(),
                source: self
                    .code_form
                    .project
                    .as_ref()
                    .and_then(|id| self.route_source(&Place::Code(id.clone()))),
                plain: self.code_form.plain,
                cloud: self.cloud_ui.on,
            }),
            terminals: self.retain_private_terminals(cx),
        })
    }

    fn restore_primary_terminals(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(identity) = self.terminal_identity.as_ref() else {
            return false;
        };
        let Some(saved) = crate::windows::take_primary_terminals(window.window_handle().window_id(), identity, cx)
        else {
            return false;
        };
        self.apply_primary_retained(saved, window, cx)
    }

    /// Forget what was read from the signed-in Gateway: the account, the chats, the Inbox and the Code sessions shown.
    fn clear_session(&mut self, cx: &mut Context<Self>) {
        browser_detach::identity_changed(None, cx);
        files_detach::identity_changed(None, cx);
        commands_detach::identity_changed(None, cx);
        self.browser_return = None;
        self.files_return = None;
        self.commands_return = None;
        self.discard_private_terminals(cx);
        self.clear_navigation();
        self.settings.shell.positions.clear();
        self.leave_org();
        self.diff_comments.clear();
        self.replies.clear();
        self.forget_assistant();
        // The pick and the list belong to the account that is gone: a sign-in as someone else, or on another Gateway,
        // starts from none.
        self.cloud_ui.reset_repos();
        self.overlay.close_person_menu();
        self.code_refresh = None;
        self.code_refresh_again = false;
        self.code_scope = None;
        self.code_overview_loaded = false;
        self.code_error = None;
        self.code = None;
        self.view = View::Chat;
        self.models.clear();
        self.model = None;
        self.reset_chat();
    }

    /// Who pays and what was spent belong to one organization: another one reads its own.
    pub(super) fn forget_org_pays(&mut self) {
        self.spend = None;
        self.own_budget = None;
        self.code_form.access = None;
    }

    /// The account is another organization's: forget the old one's payer and spend, then read the new one's.
    fn org_pays_changed(&mut self) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        self.forget_org_pays();
        account_gateway.load_pays();
        if let Some(runner) = &self.runner {
            runner.load_access();
        }
    }

    /// Settings' Gateway switch, with no restart (the Nexus app's `switch_gateway`): save `url` as the Gateway, keeping
    /// this one's organization for a switch back, then a new client for `url` signs in with its keychain entry, if
    /// any. Agents this build runs start through `url` from now on.
    pub(super) fn switch_gateway(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if crate::managed::locked_gateway(crate::managed::policy()).is_some() {
            return Err(crate::managed::GATEWAY_LOCKED.into());
        }
        let from = self.gateway.url().to_string();
        let target = nexus_desktop_core::settings::gateway_address(url)?;
        if target == from {
            return Ok(());
        }
        let restore = !self.is_local();
        // Store this before changing the launch address, so an interrupted switch can't restore the new
        // Gateway's saved account on restart.
        if !restore || crate::paths::gateway_restore_suppressed(&target) {
            crate::paths::set_gateway_restore_suppressed(&target, !restore)?;
        }
        let signed_in = matches!(self.auth, Auth::SignedIn);
        let (_, url) = crate::desktop_settings::update(|settings| {
            crate::desktop_settings::switch_gateway(settings, &from, signed_in, url)
        })?;
        // A Gateway picked at sign-in on an internal build is read first at launch (`sign_in.rs`): keep it in step.
        if crate::paths::app_gateway_url().is_some() {
            crate::paths::save_app_gateway_url(&url);
        }
        let (gateway, events) = self.gateway.choose_gateway(&url, restore);
        // The other windows move to it too (`windows.rs`).
        crate::windows::gateway_switched(cx, window.window_handle().window_id(), gateway.clone(), restore);
        if let Some(runner) = &self.runner {
            runner.set_gateway(&url, gateway.session_client());
        }
        self.follow_gateway(gateway, events, window, cx, restore);
        Ok(())
    }

    /// Move to `gateway`, after a Gateway switch here or in another window: forget what the last one showed, and sign
    /// in with the new one's keychain entry, if any.
    pub(crate) fn follow_gateway(
        &mut self,
        gateway: Arc<dyn Backend>,
        events: UnboundedReceiver<Event>,
        window: &mut Window,
        cx: &mut Context<Self>,
        restore: bool,
    ) {
        self.clear_account_services();
        self.machine_source.set_gateway(gateway.url());
        crate::updates::session(gateway.url(), None);
        self.leave_draft_identity(window, cx);
        self.gateway = gateway_access::GatewayAccess::new(gateway);
        self._events = Self::listen(events, window, cx);
        if self.is_local() {
            self.sync_local_servers();
        }
        if !self.is_local() {
            self.auth = Auth::Restoring;
            self.clear_session(cx);
        }
        if restore {
            self.gateway.restore_saved_sign_in();
        } else if !self.is_local() {
            self.handle(Event::SignedOut, window, cx);
        } else if let View::Settings(section) = self.view {
            self.settings.shell.views.clear();
            self.open_settings(section, window, cx);
        }
        cx.notify();
    }

    fn clear_account_services(&self) {
        gateway_access::AccountGateway::clear_workspace_services();
        self.machine_source.set_account_client(None);
        if self.byo_entry() {
            self.machine_source.sync(&Default::default());
        }
    }

    fn sync_local_servers(&self) {
        #[cfg(test)]
        if self.local_mode.isolated {
            return;
        }
        self.machine_source.sync_device(&crate::desktop_settings::load());
    }

    /// The Nexus app's settings were saved here: the Code sessions this build runs use the new folders and prefix.
    fn settings_saved(&mut self, settings: &nexus_desktop_core::settings::Settings) {
        crate::keep_awake::configure(settings.keep_awake.unwrap_or(true));
        self.agent = nexus_desktop_core::agent::configured(settings.agents_api_url.as_deref());
        if let Some(account) = self.account_gateway() {
            account.sync_machines(&self.machine_source, settings);
        } else if self.is_local() {
            self.machine_source.sync_device(settings);
        } else if !self.local_mode.opt_in.enabled() {
            self.machine_source.sync(settings);
        }
        if let Some(runner) = &self.runner {
            runner.apply_settings(settings);
        }
    }

    /// Open the Code sessions store for this Gateway and `org`, as the Nexus app scopes them (`<gateway>|<org>`).
    /// This build runs the sessions when the Nexus app does not (`code_run.rs`); else it only reads them.
    fn open_code_store(&mut self, org: &str, window: &mut Window, cx: &mut Context<Self>) {
        let scope = format!("{}|{}", self.gateway.url(), org);
        self.open_code_scope(scope.into(), window, cx);
    }

    fn open_code_scope(
        &mut self,
        scope: nexus_workbench::session_scope::SessionScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.code_scope.as_deref() != Some(scope.as_str()) {
            self.side.clear_side_chats(cx);
        }
        if let View::Code(view) = &mut self.view {
            view.reading = None;
            view.read_again = false;
        }
        self.code_refresh = None;
        self.code_refresh_again = false;
        self.code_scope = Some(scope.as_str().to_owned());
        self.code_overview_loaded = false;
        inbox_marks::ensure(self.session_notice_scope().as_deref(), cx);
        self.load_diff_comments();
        match &self.runner {
            Some(runner) => runner.set_scope(scope.as_str()),
            None => self.try_run_code(window, cx),
        }
        match CodeStore::open(scope) {
            Ok(mut store) => {
                if self.byo_entry()
                    && matches!(self.auth, Auth::SignedIn)
                    && let Some(store) = &mut store
                {
                    store.include_local();
                }
                self.code = store;
                self.code_error = None;
            }
            Err(error) => {
                self.code = None;
                self.code_error = Some(format!("Could not read Code sessions: {error}").into());
            }
        }
        self.refresh_code(cx);
    }

    /// Try the store's lock once, and run the Code sessions here when it is taken.
    fn try_run_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self.code_scope.clone() else {
            return;
        };
        // Isolated view tests supply their snapshots; never run a native file watcher on their scheduler.
        #[cfg(test)]
        if self.byo_entry() && self.local_mode.isolated {
            return;
        }
        // The runner another window, or a closed one, holds (`windows.rs`), else the store's lock.
        let (result, started) =
            match crate::windows::runner(cx, self.gateway.url(), &scope, self.gateway.runner_sign_in()) {
                Ok(runner) => (Ok(()), Some(runner)),
                Err(refused) => (Err(refused), None),
            };
        self.owner = self.owner.tried(result, std::time::Instant::now());
        let Some(runner) = started else {
            // Read only: the agents' models still come from this computer, so the chips name them.
            if self.code_agents.is_empty() {
                let agents = crate::code_run::read_agents();
                cx.spawn(async move |this, cx| {
                    if let Ok(agents) = agents.await {
                        let _ = this.update(cx, |this, cx| {
                            if this.code_agents.is_empty() {
                                this.code_agents = agents;
                                cx.notify();
                            }
                        });
                    }
                })
                .detach();
            }
            return;
        };
        self.runner = Some(runner);
        self._take_over = None;
        // Cloud machines and organization notifications only run for an account.
        if let Some(account) = self.account_gateway() {
            account.update_workspace_services(true);
        }
        read_only::took_over(window, cx);
    }

    /// Another window started the runner (`windows.rs`): this one runs the sessions too, no longer read-only.
    pub(crate) fn adopt_runner(&mut self, runner: Rc<CodeRunner>, window: &mut Window, cx: &mut Context<Self>) {
        if self.runner.is_some() {
            return;
        }
        if let Some(scope) = &self.code_scope {
            runner.set_scope(scope);
        }
        self.owner = self.owner.tried(Ok(()), std::time::Instant::now());
        self.runner = Some(runner);
        self._take_over = None;
        if let Some(account) = self.account_gateway() {
            account.update_workspace_services(true);
        }
        read_only::took_over(window, cx);
        if let Some(scope) = self.code_scope.clone() {
            self.open_code_scope(scope.into(), window, cx);
        }
        cx.notify();
    }

    /// Whether this window waits for `event`, a reply to what it asked (`windows::Reach::Waiting`): a session it is
    /// starting, or a clone it runs.
    pub(crate) fn waits_for(&self, event: &CodeEvent) -> bool {
        match event {
            // The window that started it, by its draft view: not every window that waits on a start.
            CodeEvent::Created { draft, .. } | CodeEvent::StartFailed { draft, .. } => {
                self.code_form.starting.as_ref() == Some(draft)
                    || self.local_mode.starts.contains_key(draft)
                    || matches!(&self.view, View::Code(view) if view.is_draft() && view.id == *draft)
            }
            CodeEvent::Cloned(_) => self.cloning(),
            _ => false,
        }
    }

    /// "Take over": the person quits the Nexus app, and this build tries the lock until it is free or the wait ends.
    /// The Nexus app is never stopped from here (`owner.rs`).
    pub(super) fn take_over(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.owner = self.owner.take_over(std::time::Instant::now());
        if !self.owner.polling() {
            return;
        }
        self._take_over = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(crate::owner::POLL).await;
                let polling = this.update_in(cx, |this, window, cx| {
                    this.try_run_code(window, cx);
                    if this.runner.is_some()
                        && let Some(scope) = this.code_scope.clone()
                    {
                        // The store may not have existed while the Nexus app never wrote it: open it now.
                        this.open_code_scope(scope.into(), window, cx);
                        if let Some(runner) = &this.runner {
                            runner.load_access();
                        }
                    }
                    cx.notify();
                    this.owner.polling()
                });
                if !matches!(polling, Ok(true)) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// The open session's answer streamed further (`stream.rs`): draw it, keeping the end in view when it was.
    pub(super) fn sync_stream(&mut self, cx: &mut Context<Self>) {
        if let Some(chat) = self.open_side_chat() {
            chat.update(cx, |chat, cx| chat.sync_stream(cx));
        }
        if self.stream_changed(cx) {
            self.notify_main(cx);
        }
    }

    fn stream_changed(&mut self, cx: &mut Context<Self>) -> bool {
        let View::Code(view) = &mut self.view else {
            return false;
        };
        // The transcript's list follows its own end (`FollowMode::Tail`) when the tail item is measured again.
        let text = crate::stream::text(&view.id);
        // What the running sub-agents are still writing, drawn at the end of their open rows.
        let running: Vec<String> = view
            .agents
            .iter()
            .filter(|(_, at)| at.running)
            .map(|(call, _)| call.clone())
            .collect();
        let drafts = crate::stream::agent_texts(&view.id, running.iter().map(String::as_str));
        let agents = view.stream_agents(drafts);
        view.stream(text, cx) | agents
    }

    /// The servers changed (`machines.rs`): their status, projects or a followed session's events.
    fn machines_changed(&mut self, cx: &mut Context<Self>) {
        let machines = self.machine_source.clone();
        self.servers = machines.servers();
        sidebar_machines::settle_retries(&mut self.server_retries, &self.servers);
        self.cloud_machines = machines.cloud();
        self.settle_wakes();
        self.cloud_panel_follow(cx);
        self.refresh_code(cx);
        cx.notify();
    }

    /// The window got the focus: what the unfocused polls skipped, read now.
    fn activated(&mut self, cx: &mut Context<Self>) {
        if self.runner.is_none() {
            self.refresh_code(cx);
        }
        if let Some(account) = self.account_gateway() {
            account.load_inbox();
            if matches!(self.view, View::Inbox) {
                self.reload_inbox();
            }
        }
    }

    /// Read the projects again, this computer's then each server's, and the open Code session when it moved.
    fn refresh_code(&mut self, cx: &mut Context<Self>) {
        let _span = crate::watchdog::span("Workspace::refresh_code");
        self.update_code_blocker();
        self.refresh_code_overview(cx);
        self.read_code_transcript(cx);
        if self.stream_changed(cx) {
            cx.notify();
        }
        self.sync_working();
    }

    fn repick_project_after_overview(&mut self) {
        // A failed/partial discovery must not turn the retained project's route into Home.
        if !self.side.restoring_route() {
            self.repick_new_code_project();
        }
    }

    /// All refresh triggers share this worker: open, Stop, agent events, focus and archive navigation.
    fn refresh_code_overview(&mut self, cx: &mut Context<Self>) {
        if self.code_refresh.is_some() {
            self.code_refresh_again = true;
            return;
        }
        let scope = self.code_scope.clone();
        let local = self.code.as_ref().map(CodeStore::reader);
        let opening_error = if local.is_none() {
            self.code_error.as_ref().map(ToString::to_string)
        } else {
            None
        };
        // A new background service can create its database after the first scope open. Try again when the
        // service reports a connection or a store change, even when the runner was already available.
        let pending_reader = if local.is_none() {
            scope
                .clone()
                .zip(self.runner.as_ref().map(|runner| runner.store_source().0))
        } else {
            None
        };
        let include_local = self.byo_entry() && matches!(self.auth, Auth::SignedIn);
        let machines = self.machine_source.clone();
        let only_local = self.is_local();
        let read = cx.background_executor().spawn(async move {
            let mut opening_error = opening_error;
            let mut reopened = None;
            if let Some((scope, data)) = pending_reader {
                match CodeStore::open_reading_in(&data, scope) {
                    Ok(Some(mut store)) => {
                        if include_local {
                            store.include_local();
                        }
                        reopened = Some(store);
                    }
                    Ok(None) => {}
                    Err(error) => opening_error = Some(error),
                }
            }
            let local = local.or_else(|| reopened.as_ref().map(CodeStore::reader));
            let (projects, archived) = match local {
                Some(reader) => reader.overview(),
                None => match opening_error {
                    Some(error) => (Err(error), Ok(Vec::new())),
                    None => (Ok(Vec::new()), Ok(Vec::new())),
                },
            };
            let (mut projects, local_error) =
                code_recovery::with_remote(projects, if only_local { Vec::new() } else { machines.projects() });
            // One order for this computer's, the servers' and the cloud's projects; activity never moves one.
            projects.sort_by(crate::code::project_order);
            let archived = archived.map(|mut archived| {
                if !only_local {
                    archived.extend(machines.archived_sessions());
                }
                archived.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                archived
            });
            (projects, archived, local_error, reopened)
        });
        self.code_refresh = Some(cx.spawn(async move |this, cx| {
            let (projects, archived, local_error, reopened) = read.await;
            let _ = this.update(cx, |this, cx| {
                if this.code_scope != scope {
                    return;
                }
                this.code_refresh = None;
                this.code_overview_loaded = true;
                this.projects = projects;
                this.code_error = local_error.map(Into::into);
                // A project can be selected after this read started. Let the queued read validate that selection.
                if !this.code_refresh_again {
                    this.repick_project_after_overview();
                }
                if let Some(store) = reopened {
                    this.code = Some(store);
                    this.read_code_transcript(cx);
                }
                if let Ok(archived) = archived {
                    this.archived_code = archived;
                }
                // A deleted row's slot goes once the lists no longer have its session: it has folded already.
                let listed: HashSet<String> = this
                    .projects
                    .iter()
                    .flat_map(|project| &project.sessions)
                    .chain(&this.archived_code)
                    .map(|session| session.id.clone())
                    .collect();
                this.row_exits
                    .forget(|id| listed.contains(id), std::time::Instant::now(), cx.reduce_motion());
                this.note_seen(cx);
                this.note_inbox_completion_seen(cx);
                if let View::Code(view) = &this.view
                    && !view.is_draft()
                    && let Some(title) = this.session_by_id(&view.id).map(|session| session.title.clone())
                {
                    this.title = title.into();
                }
                this.sync_working();
                if std::mem::take(&mut this.code_refresh_again) {
                    this.refresh_code_overview(cx);
                }
                cx.notify();
            });
        }));
    }

    /// Read and reduce the event log away from the UI thread. Changes during a read coalesce into one follow-up.
    fn read_code_transcript(&mut self, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        if view.is_draft() {
            return;
        }
        if view.reading.is_some() {
            view.read_again = true;
            return;
        }
        let id = view.id.clone();
        let scope = self.code_scope.clone();
        let machines = self.machine_source.clone();
        let machine = machines.machine_of(&id);
        // This computer's dev servers, or a server's through an SSH forward (`auto_preview.rs`); not a cloud machine's.
        let local_preview = machine.as_deref().is_none_or(|machine| machines.is_server(machine));
        let local = self.code.as_ref().map(CodeStore::reader);
        let last_seq = view.last_seq;
        let loaded = view.loaded;
        let start = view.start;
        let older = std::mem::take(&mut view.older);
        let read = cx.background_executor().spawn(async move {
            match machine {
                Some(machine) => machines
                    .transcript(&machine, &id, older)
                    .map(|answer| answer.map(|read| (Some(read), None))),
                None => local.map(|reader| {
                    if !older && loaded && last_seq > 0 && !reader.has_events_after(&id, last_seq) {
                        Ok((None, None))
                    } else {
                        reader
                            .transcript(&id, start, older)
                            .map(|read| (Some(read), reader.commands(&id)))
                    }
                }),
            }
        });
        view.reading = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| {
                if this.code_scope != scope {
                    return;
                }
                let View::Code(view) = &mut this.view else { return };
                // The task belongs to this view. Replacing the view drops it and cancels this callback.
                view.reading = None;
                let again = std::mem::take(&mut view.read_again);
                let cloud_session = this
                    .cloud_machines
                    .iter()
                    .any(|machine| machine.info.session_id.as_deref() == Some(view.id.as_str()));
                let mut preview = None;
                match result {
                    Some(Ok((Some(read), commands)))
                        if !view.loaded || (read.last, read.start) != (view.last_seq, view.start) =>
                    {
                        if local_preview {
                            preview = view
                                .auto_browser
                                .observe(&read.signals, read.last)
                                .map(|offer| (view.id.clone(), offer));
                        }
                        view.commands = commands;
                        view.set(read, cx);
                        this.note_inbox_completion_seen(cx);
                    }
                    Some(Err(_)) if view.pending.is_some() || cloud_session => {}
                    Some(Err(error)) => view.error = Some(error.into()),
                    _ => {}
                }
                this.auto_open_code_page(cx);
                this.load_answer_files(cx);
                if let Some((thread, offer)) = preview {
                    this.check_preview(thread, offer, cx);
                }
                this.stream_changed(cx);
                this.sync_working();
                if again {
                    this.read_code_transcript(cx);
                }
                cx.notify();
            });
        }));
    }

    /// What this build's Code sessions report: read the store again, open a new session, or show a problem.
    pub(crate) fn on_code_events(&mut self, events: Vec<CodeEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let _span = crate::watchdog::span("Workspace::on_code_events");
        // Streamed text alone: the open view draws it, and nothing is read again (`stream.rs`).
        if events.iter().all(|event| matches!(event, CodeEvent::Streamed)) {
            self.sync_stream(cx);
            return;
        }
        for event in events {
            match event {
                CodeEvent::Changed | CodeEvent::Streamed => {}
                CodeEvent::TurnEnded(id) => self.update_followed_share(&id, cx),
                // The draft view takes the session's id in place; with it closed, the session only joins the list.
                CodeEvent::Created { draft, id } => {
                    if self.local_mode.starts.remove(&draft).is_some() {
                        self.record_first_local_turn();
                    }
                    self.release_pending_first_provider(&draft, cx);
                    if self.code_form.starting.as_ref() == Some(&draft) {
                        self.code_form.starting = None;
                        self.cloud_ui.starting = None;
                        self.clear_first_provider(cx);
                    }
                    if self.pending_created(&draft, &id) {
                        self.rename_draft(&draft, &id, cx);
                    }
                    self.refresh_code(cx);
                }
                // A project added from the sidebar or the project menu opens a new session in it.
                CodeEvent::ProjectAdded {
                    id, scope, generation, ..
                } => {
                    if self.project_scope_ready()
                        && self.code_scope.as_deref() == Some(scope.as_str())
                        && self
                            .runner
                            .as_ref()
                            .is_some_and(|runner| runner.owns_project_result(&scope, generation))
                    {
                        self.open_new_code_in(id, window, cx);
                    } else {
                        self.project_add_refused("The project was added, but the active workspace changed.", cx);
                    }
                }
                CodeEvent::ProjectAddFailed { error, .. } => self.show_project_add_failure(&error, cx),
                CodeEvent::Cloned(cloned) => self.cloned(cloned, window, cx),
                CodeEvent::Agents(agents) => {
                    self.code_agents = agents;
                    self.refresh_auto_route(cx);
                }
                // Read with the sign-in it names: another organization's answer, landing after a switch, is dropped.
                CodeEvent::Access { credential, access } => {
                    if credential.is_some() && credential == self.gateway.marker() {
                        self.code_form.access = Some(access);
                        self.refresh_auto_route(cx);
                        self.plans_access_read(window, cx);
                        // A key that landed sends the message held for it.
                        self.key_check(window, cx);
                    }
                }
                CodeEvent::Unsent {
                    thread_id,
                    text,
                    attachments,
                    error,
                } => self.unsent(&thread_id, text, attachments, error, window, cx),
                CodeEvent::StartFailed { draft, error } => {
                    // Another window's start, or one already answered, is not this one's.
                    let ours = self.code_form.starting.as_ref() == Some(&draft);
                    if ours && (self.cloud_ui.starting.is_some() || self.start_upload_unsettled()) {
                        self.fail_start_upload(window, cx);
                    }
                    let start = if ours {
                        self.code_form.starting = None;
                        self.cloud_ui.starting.take()
                    } else {
                        None
                    };
                    let provider_start = self.local_mode.starts.remove(&draft).flatten();
                    self.release_pending_first_provider(&draft, cx);
                    if ours
                        && self.is_local()
                        && matches!(&self.view, View::Code(view) if view.id == draft
                            && view.pending.as_ref().is_some_and(|pending| pending.new.provider.is_some()))
                    {
                        self.edit_pending(window, cx);
                        self.code_form.require_provider_reselection(error);
                        self.load_local_provider_choices(cx);
                        continue;
                    }
                    if let Some(start) = provider_start {
                        self.sync_draft(window, cx);
                        self.restore_submitted_draft(start.submission, None, window, cx);
                        self.local_mode.failed_provider_drafts.insert(start.project, error);
                        continue;
                    }
                    if self.pending_failed(&draft, &error) {
                        self.start_waits_for_sign_in(&draft, &error, cx);
                    } else {
                        // Its view is closed (another session is open): its messages are not lost with it.
                        match start {
                            Some(start) => {
                                self.keep_failed_start(&start, window, cx);
                                self.composer_note =
                                    Some(format!("{error} Your messages are kept in New session.").into());
                            }
                            None => self.composer_note = Some(error.into()),
                        }
                    }
                }
                // Another command's failure (approve, stop, rename, the agents' list): a note. A start goes on.
                CodeEvent::Failed(error) => {
                    // A queued chip's Send now or take back that failed can be pressed again.
                    if let View::Code(view) = &mut self.view {
                        view.dock.acting.clear();
                    }
                    self.composer_note = Some(error.into());
                }
            }
        }
        self.refresh_code(cx);
        // A request for a secret opened or closed (`secret_card.rs`); a consent prompt shows at the next draw.
        self.follow_secret_request(window, cx);
        cx.notify();
    }

    /// A Code session's notification was clicked (`notify.rs`): open its session, or its pull request page for a
    /// pull request change.
    pub fn open_notified(&mut self, thread_id: &str, pull: bool, window: &mut Window, cx: &mut Context<Self>) {
        if pull {
            self.open_pull(thread_id, window, cx);
        } else {
            self.open_code(thread_id, window, cx);
        }
        cx.notify();
    }

    /// Open cloud session `id` from its sidebar row. A sleeping machine stays asleep: it wakes when the person types
    /// or sends there (`cloud_view.rs`).
    pub(super) fn open_cloud_session(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_code(id, window, cx);
    }

    fn open_code(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() {
            return;
        }
        if !self.navigation_restoring {
            self.browser_return = None;
            self.files_return = None;
            self.commands_return = None;
            self.reopen_commands(id, cx);
        }
        let _span = crate::watchdog::span("Workspace::open_code");
        if matches!(&self.view, View::Code(open) if open.id == id) {
            return;
        }
        self.visit(Place::Code(id.to_string()));
        self.reset_chat();
        // A cloud session not read yet (its machine sleeps) is known by the title Gateway keeps.
        self.title = self
            .session_by_id(id)
            .map(|session| session.title.clone())
            .or_else(|| {
                self.cloud_machine_of_session(id)
                    .and_then(|machine| machine.info.title.clone())
            })
            .filter(|title| !title.is_empty())
            .map(Into::into)
            .unwrap_or_else(|| "Code session".into());
        self.view = View::Code(CodeView::new(id.to_string(), window, cx));
        self.load_diff_comments();
        self.sync_draft(window, cx);
        // The session opens ready for typing, as a chat does.
        self.focus_composer(window, cx);
        self.refresh_code(cx);
        cx.notify();
    }

    /// Open a page of Nexus UI for this organization in the browser (`<ui>/app/<org>/<path>`): the places this build
    /// does not draw yet.
    /// Account pages can only be constructed for the signed-in organization.
    pub(super) fn page_ctx(&self, cx: &mut Context<Self>) -> Option<crate::pages::Ctx> {
        let gateway = self.account_gateway()?;
        let account = self.account.as_ref()?;
        let this = cx.entity().downgrade();
        Some(crate::pages::Ctx {
            device: self.device_page_ctx(cx),
            gateway,
            org: account.org.clone(),
            org_name: account.org_name.clone(),
            email: account.email.clone(),
            fixer: self.fixer(cx),
            new_cloud_session: std::rc::Rc::new({
                let this = this.clone();
                move |window, cx| {
                    this.update(cx, |workspace, cx| {
                        workspace.open_new_code(window, cx);
                        workspace.pick_cloud(window, cx);
                    })
                    .ok();
                }
            }),
            team: std::rc::Rc::new({
                let this = this.clone();
                move |cx| {
                    this.upgrade()
                        .map(|workspace| workspace.read(cx).team_live())
                        .unwrap_or_default()
                }
            }),
            watch: std::rc::Rc::new({
                let this = this.clone();
                move |person, window, cx| {
                    this.update(cx, |workspace, cx| {
                        workspace.open_team_sessions(Some(person), window, cx)
                    })
                    .ok();
                }
            }),
        })
    }

    pub(super) fn device_page_ctx(&self, cx: &mut Context<Self>) -> crate::pages::DeviceCtx {
        let this = cx.entity().downgrade();
        crate::pages::DeviceCtx {
            local_workspace: self.is_local(),
            settings_fields: self.settings.shell.fields.clone(),
            gateway_url: self.gateway.url().into(),
            navigate: std::rc::Rc::new({
                let this = this.clone();
                move |id, window, cx| {
                    this.update(cx, |workspace, cx| workspace.open_native(id, window, cx))
                        .ok();
                }
            }),
            navigate_settings: std::rc::Rc::new({
                let this = this.clone();
                move |path, window, cx| {
                    this.update(cx, |workspace, cx| {
                        if let Some((section, anchor)) = settings_registry::resolve_field(&path) {
                            workspace.open_settings_field(section, anchor, window, cx);
                        } else if let Some(section) = workspace.resolve_settings_section(&path) {
                            workspace.open_settings(section, window, cx);
                        } else {
                            workspace.open_in_browser(path.trim_start_matches('/'), cx);
                        }
                    })
                    .ok();
                }
            }),
            switch_gateway: std::rc::Rc::new({
                let this = this.clone();
                move |url, window, cx| {
                    this.update(cx, |workspace, cx| workspace.switch_gateway(&url, window, cx))
                        .unwrap_or_else(|_| Err("The window closed.".into()))
                }
            }),
            settings_saved: std::rc::Rc::new({
                let this = this.clone();
                move |settings, cx| {
                    this.update(cx, |workspace, _| workspace.settings_saved(settings)).ok();
                }
            }),
            restart_for_update: std::rc::Rc::new({
                let this = this.clone();
                move |cx| {
                    this.update(cx, |workspace, cx| workspace.restart_for_update(cx))
                        .unwrap_or_else(|_| Err("The window closed.".into()))
                }
            }),
            restart_for_service: std::rc::Rc::new({
                let this = this.clone();
                move |cx| {
                    this.update(cx, |workspace, cx| workspace.restart_for_service(cx))
                        .unwrap_or_else(|_| Err("The window closed.".into()))
                }
            }),
            new_code_session: std::rc::Rc::new({
                let this = this.clone();
                move |window, cx| {
                    this.update(cx, |workspace, cx| workspace.open_new_code(window, cx))
                        .ok();
                }
            }),
            code: std::rc::Rc::new(move |cx| {
                let Some(workspace) = this.upgrade() else {
                    return crate::pages::CodeAccess::default();
                };
                let workspace = workspace.read(cx);
                crate::pages::CodeAccess {
                    scope: workspace.code_scope.clone(),
                    schedules: workspace.runner.as_ref().map(|runner| runner.schedules()),
                    projects: workspace.runner.as_ref().map(|runner| runner.projects()),
                    note: workspace.owner.note(),
                    plans: workspace
                        .code_form
                        .access
                        .as_ref()
                        .is_some_and(|access| access.subscriptions),
                    outside: workspace.runner.as_ref().map(|runner| runner.outside()),
                    service: workspace.runner.as_ref().and_then(|runner| runner.service_status()),
                    device_key_changed: workspace.runner.as_ref().map(|runner| runner.device_key_changed()),
                }
            }),
        }
    }

    /// Settings › App's Restart to update: the app's (`windows::restart_for_update`), which the tray's row runs too.
    pub(super) fn restart_for_update(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        self.request_update_restart(cx)
    }

    /// Settings › App's Restart now, as the Nexus app's `service_restart`: refused while Code sessions run (here or in
    /// the background service). Turned off, the service stops first, with its agents (`CodeRunner::shutdown`). Then
    /// this app starts again and this copy quits.
    pub(super) fn restart_for_service(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        const RUNNING: &str = "Code sessions are still running. Finish or stop them, then restart.";
        if let Some(runner) = &self.runner {
            runner.reserve_update_restart().map_err(|_| RUNNING.to_string())?;
        }
        let exe = std::env::current_exe().map_err(|error| format!("Couldn't find this app: {error}."))?;
        if let Some(runner) = self.runner.take() {
            runner.shutdown();
        }
        std::process::Command::new(exe)
            .env("NEXUS_RESTARTED", "1")
            .spawn()
            .map_err(|error| format!("Couldn't start Nexus again: {error}."))?;
        cx.quit();
        Ok(())
    }

    /// Open a page of `pages/`: a place, or a section of Settings. A page not drawn here opens in the browser.
    pub(super) fn open_native(&mut self, id: crate::pages::PageId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() {
            return;
        }
        if !id.is_place() || (self.is_local() && id == crate::pages::PageId::Usage) {
            self.open_settings(SettingsSection::Page(id), window, cx);
            return;
        }
        if id == crate::pages::PageId::Agent {
            self.open_agent(None, window, cx);
            return;
        }
        if id.is_assistant_view() {
            self.open_assistant_view(id, None, window, cx);
            return;
        }
        let built = match self.page_ctx(cx) {
            Some(ctx) if id.is_native() => crate::pages::build(id, ctx, window, cx),
            _ => None,
        };
        let Some(view) = built else {
            self.open_in_browser(id.web_path(), cx);
            return;
        };
        self.show_page(id, view, cx);
    }

    /// The Assistant's pane, on `conversation` when a notification's click names one. Without an agent API it is not
    /// there: the app goes to New session, as the Nexus app's `AgentGate` goes home. The page is kept while the
    /// account lasts (`assistant.rs`), so coming back shows the same thread.
    pub fn open_agent(&mut self, conversation: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let id = crate::pages::PageId::Agent;
        let page = if self.agent {
            self.assistant_page(conversation, window, cx)
        } else {
            None
        };
        match page {
            Some(page) => {
                // Back after an hour, today's brief is read again.
                page.update(cx, |page, cx| {
                    page.brief_on_return(window, cx);
                    page.focus_composer(window, cx);
                });
                self.show_page(id, page.into(), cx);
                self.sync_assistant(cx);
            }
            None => self.start_new_session(window, cx),
        }
    }

    /// Show a place of `pages/` in the main area.
    fn show_page(&mut self, id: crate::pages::PageId, view: AnyView, cx: &mut Context<Self>) {
        if !matches!(self.view, View::Page(open) if open == id) {
            self.visit(Place::Page(id));
        }
        self.reset_chat();
        self.native_page = Some((id, view));
        self.view = View::Page(id);
        self.title = id.label().into();
        cx.notify();
    }

    /// A clicked notification of the organization (`path_notices.rs`): its page, here when this build draws it, else
    /// Nexus UI's at `path` (`/app?next=…`, `/app/<org>/…`) in the browser.
    pub(crate) fn open_nexus_path(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() {
            return;
        }
        if let Some(target) = crate::path_notices::skills_target(path) {
            self.open_skills_target(target, window, cx);
            return;
        }
        let local_path = if self.is_local() {
            crate::path_notices::page_of(path).map(|page| {
                let anchor = path.split_once('#').map_or("", |(_, anchor)| anchor);
                format!("/{page}#{anchor}")
            })
        } else {
            None
        };
        let settings_path = local_path.as_deref().unwrap_or(path);
        if let Some((section, anchor)) = settings_registry::resolve_field(settings_path) {
            self.open_settings_field(section, anchor, window, cx);
            return;
        }
        if let Some(section) = self.resolve_settings_section(settings_path) {
            self.open_settings(section, window, cx);
            return;
        }
        match crate::path_notices::native_page(path) {
            Some(id) => self.open_native(id, window, cx),
            None => crate::external_link::open(&format!("{}{path}", self.gateway.site_url()), cx),
        }
    }

    fn open_in_browser(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(gateway) = self.account_gateway() else { return };
        self.overlay.close_person_menu();
        let org = self
            .account
            .as_ref()
            .map(|account| account.org.clone())
            .or_else(|| crate::desktop_settings::load().org);
        let url = match org {
            Some(org) => crate::pages::org_url(&gateway.site_url(), &org, path),
            None => format!("{}/app", gateway.site_url()),
        };
        crate::external_link::open(&url, cx);
        cx.notify();
    }

    fn reset_chat(&mut self) {
        self.kept_chats.leave(self.current.as_deref(), &mut self.entries);
        self.cached_chat.clear();
        self.launch_motion = Default::default();
        self.cancel_dictation();
        self.composer_note = None;
        if !self.detach_live_chat() {
            self.chat_queue_leave();
        }
        self.view = View::Chat;
        self.current = None;
        self.chat_share = None;
        // One list serves every chat: a drag or a preview from the chat that closes must not carry over.
        self.chat_list.minimap = Default::default();
        self.title = SharedString::default();
        self.entries.clear();
        self.chat_rewind = None;
        self.rewind_edit = false;
        self.edit_sent = None;
        self.opening = false;
        self.notice = None;
        self.suggestion = None;
        self.copied = None;
        self.overlay.close_model_menu();
        self.overlay.close_person_menu();
        self.overlay.close_project_menu();
        self.overlay.close_session_menu();
        self.overlay.close_chat_menu();
        self.chat_rename = None;
        self.chat_archived = false;
        self.overlay.close_launch_menu();
        self.reset_chat_choices();
    }

    /// Send what is in the composer.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.submit_now(false, window, cx);
    }

    /// Send what is in the composer. While a chat answers it queues (`chat_queue.rs`); `now` (Ctrl+Enter, Cmd+Enter)
    /// sends it next at once, stopping the answer. A Code session reads `now` the other way round: its Enter already
    /// joins the running turn, so Ctrl+Enter queues the message for after it.
    fn submit_now(&mut self, now: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() {
            return;
        }
        self.sync_gateway_chat(window, cx);
        self.sync_draft(window, cx);
        let prompt = self.composer.read(cx).value().trim().to_string();
        if matches!(self.view, View::Code(_) | View::NewCode) {
            if self.pending_diff_comments().is_empty()
                && self.pending_replies().is_empty()
                && self.slash_submit(&prompt, window, cx)
            {
                return;
            }
            // In a running Code session, Ctrl+Enter (Cmd+Enter) queues the message for after the turn; Enter sends it
            // into the turn.
            self.submit_code(prompt, now, window, cx);
            return;
        }
        let prompt = nexus_workbench::diff_comment::prompt_text(self.pending_diff_comments(), &prompt);
        let chat = matches!(self.view, View::Chat);
        if (prompt.is_empty() && self.draft.is_empty())
            || self.attaching > 0
            || !chat
            || !matches!(self.auth, Auth::SignedIn)
        {
            return;
        }
        if let Some(problem) = crate::attachments::chat_problem(&self.draft) {
            self.composer_note = Some(problem.into());
            cx.notify();
            return;
        }
        // A used-up budget holds the message: its line says why, and it sends once the budget is raised.
        if self.hold_for_limit(cx) {
            cx.notify();
            return;
        }
        if !self.ensure_gateway_execution(cx) {
            self.composer_note = Some("Wait for the current account to finish loading before sending.".into());
            cx.notify();
            return;
        }
        let submitted = self.gateway_draft_submission(cx);
        let refused = submitted.clone();
        self.cancel_dictation();
        self.composer_note = None;
        self.composer.update(cx, |state, cx| state.set_value("", window, cx));
        self.composer_empty = true;
        let attachments = std::mem::take(&mut self.draft);
        self.draft_submitted(submitted, window, cx);
        let accepted = if self.chat_busy() {
            self.try_enqueue_chat(prompt, attachments, now, window, cx)
        } else {
            self.try_ask(prompt, attachments, cx)
        };
        if accepted {
            self.replace_diff_comments(Vec::new());
        } else {
            self.draft_refused(refused, window, cx);
        }
    }

    fn ask(&mut self, prompt: String, attachments: crate::attachments::Draft, cx: &mut Context<Self>) {
        let _ = self.try_ask(prompt, attachments, cx);
    }
    fn try_ask(&mut self, prompt: String, attachments: crate::attachments::Draft, cx: &mut Context<Self>) -> bool {
        let Some(turn) = self.next_gateway_turn(cx) else {
            return false;
        };
        self.cached_chat.mutated(self.current.as_deref());
        self.kept_chats.mutated();
        self.begin_launch_motion(cx);
        self.turn = turn;
        self.asking = Some(turn);
        self.notice = None;
        self.suggestion = None;
        self.overlay.close_model_menu();
        self.next_entry += 1;
        self.entries.push(
            Entry::user(self.next_entry, prompt.clone())
                .with_attachments(attachments.clone())
                .arriving(),
        );
        self.next_entry += 1;
        self.entries
            .push(Entry::answer(self.next_entry, "", true, cx).arriving());
        let accepted = self.launch_gateway_chat(prompt, attachments, cx);
        if !accepted {
            self.mark_gateway_revoked(cx);
        }
        // A question sent is always followed, wherever the reader was.
        self.scroll.scroll_to_bottom();
        cx.notify();
        accepted
    }

    /// Stop the answer where it is. What streamed stays, marked as stopped.
    fn stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() || (self.chat_execution.is_some() && !self.chat_binding_current(cx)) {
            return;
        }
        if let View::Code(view) = &self.view
            && self.code_running()
            && let Some(runs) = self.runs(&view.id)
        {
            runs.interrupt(view.id.clone());
            return;
        }
        if self.stop_gateway_chat(window, cx) {
            return;
        }
        let Some(turn) = self.asking.take() else {
            return;
        };
        if let Some(account) = self.account_gateway() {
            account.stop(turn);
            // Gateway made the session, and maybe stored part of the answer.
            account.list_sessions();
        }
        if let Some(entry) = self.entries.last_mut() {
            entry.streaming = false;
            entry.stopped = true;
        }
        // An edit stopped before Gateway stored it goes back to the composer, as the Nexus app's.
        self.edit_not_sent(turn, window, cx);
        // The queue holds, or sends the message asked for with Send now.
        self.chat_answer_ended(turn, nexus_workbench::chat_queue::Outcome::Stopped, cx);
        cx.notify();
    }

    /// Entries for a chat's stored messages, each with a new id.
    fn entries_from(&mut self, messages: Vec<Message>, cx: &mut Context<Self>) -> Vec<Entry> {
        messages
            .into_iter()
            .map(|message| {
                self.next_entry += 1;
                let mut entry = Entry::from_message(self.next_entry, message, cx);
                self.restore_chat_attachments(&mut entry, cx);
                entry
            })
            .collect()
    }

    /// Ask the question of a failed answer again, in place of both.
    fn retry(&mut self, cx: &mut Context<Self>) {
        if self.asking.is_some() || self.entries.len() < 2 {
            return;
        }
        if !self.ensure_gateway_execution(cx) {
            return;
        }
        let asked = &self.entries[self.entries.len() - 2];
        if !asked.attachment_load.ready() {
            self.notice = Some(
                "Wait for this message's attachments to load, or use Retry attachments before asking again.".into(),
            );
            cx.notify();
            return;
        }
        let (question, attachments) = (asked.text.to_string(), asked.attachments.clone());
        self.entries.truncate(self.entries.len() - 2);
        self.ask(question, attachments, cx);
    }

    fn copy(&mut self, entry_id: usize, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(entry_id);
        self._copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            let _ = this.update(cx, |this, cx| {
                if this.copied == Some(entry_id) {
                    this.copied = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() {
            return;
        }
        self.browser_return = None;
        self.files_return = None;
        self.commands_return = None;
        self.navigation_explicit = true;
        self.cancel_navigation_restore();
        // A project's "+" sets where the chat goes after this (`sidebar_groups.rs`); any other start goes to Recent.
        self.groups.pending = None;
        if !matches!(self.view, View::Chat) || self.current.is_some() {
            self.visit(Place::Chat(None));
        }
        self.reset_chat();
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connection_open() {
            window.focus(&self.focus, cx);
        } else {
            self.composer.update(cx, |state, cx| state.focus(window, cx));
        }
    }

    /// Register the dispatch before the UI can consume any queued reply.
    fn read_chat(&mut self, id: String) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let replace = self.opening || self.cached_chat.replacing(&id);
        let Some(read) = account_gateway.open_session(id.clone()) else {
            return;
        };
        self.cached_chat.dispatched(id, read, replace);
    }

    fn open(&mut self, session: &SessionSummary, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() {
            return;
        }
        if matches!(self.view, View::Chat) && self.current.as_deref() == Some(session.id.as_str()) {
            return;
        }
        self.visit(Place::Chat(Some(session.clone())));
        self.reset_chat();
        self.moving = None;
        self.current = Some(session.id.clone());
        self.title = session.title.clone().into();
        self.opening = true;
        if self.resume_live_chat(Some(&session.id), window, cx) {
            return;
        }
        self.read_chat(session.id.clone());
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Record a place visited: what was ahead of the current one is dropped, as in a browser.
    fn visit(&mut self, place: Place) {
        if self.navigation_restoring || !self.navigation_allowed() {
            return;
        }
        self.browser_return = None;
        self.files_return = None;
        self.commands_return = None;
        self.navigation_explicit = true;
        self.cancel_navigation_restore();

        self.session_anchor = None;
        if self.history.get(self.history_at) == Some(&place) {
            if let Some(source) = self.route_source(&place) {
                self.navigation_sources.insert(self.history_at, source);
            }
            return;
        }
        // Settings' scroll position stays with its entry, for a step back to it; entries ahead are dropped.
        if !self.settings.shell.restoring {
            self.remember_settings_position();
        }
        let at = self.history_at;
        self.settings.shell.positions.retain(|(_, entry), _| *entry <= at);
        self.history.truncate(self.history_at + 1);
        self.navigation_sources.retain(|at, _| *at <= self.history_at);
        // The next launch opens here (`last_place.rs`).
        self.remember_place(&place);
        let source = self.route_source(&place);
        self.history.push(place);
        if let Some(source) = source {
            self.navigation_sources.insert(self.history.len() - 1, source);
        }
        self.history_at = self.history.len() - 1;
    }

    /// Go back (`-1`) or forward (`1`) in the history.
    fn go(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() {
            return;
        }
        let Some(at) = history_step(self.history_at, self.history.len(), step) else {
            return;
        };
        self.browser_return = None;
        self.files_return = None;
        self.commands_return = None;
        self.navigation_explicit = true;
        self.remember_settings_position();
        self.cancel_navigation_restore();
        self.history_at = at;
        self.navigation_restoring = true;
        self.settings.shell.restoring = true;
        self.open_navigation_place(window, cx);
        self.settings.shell.restoring = false;
        self.navigation_restoring = false;
        if let Some(place) = self.history.get(at) {
            self.remember_place(place);
        }
        self.restore_settings_position(at, window, cx);
        cx.notify();
    }

    /// Open `place` as a visit would: back and forward, and the launch's last place.
    fn open_place(&mut self, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_allowed() {
            return;
        }
        match place {
            Place::Chat(None) => {
                self.reset_chat();
                if self.resume_live_chat(None, window, cx) {
                    return;
                }
                self.focus_composer(window, cx);
            }
            Place::Chat(Some(session)) => self.open(&session, window, cx),
            Place::Code(id) => self.open_code(&id, window, cx),
            Place::NewCode => self.open_new_code(window, cx),
            Place::Library => self.open_library(window, cx),
            Place::Inbox => self.open_inbox(window, cx),
            Place::Docs(path) => self.open_docs(path, cx),
            Place::Issues => self.open_issues(cx),
            Place::Settings(section) => self.open_settings(section, window, cx),
            Place::SettingsField(section, anchor) => self.open_settings_field(section, anchor, window, cx),
            Place::Models => self.open_models(window, cx),
            Place::Page(id) => self.open_native(id, window, cx),
            Place::Pull(id) => self.open_pull(&id, window, cx),
        }
    }

    fn toggle_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_overlay(Overlay::ModelMenu);
        self.overlay.close_person_menu();
        self.overlay.close_team_menu();
        if self.overlay.model_menu() {
            self.model_maker = None;
            self.model_older = false;
            self.reset_model_search(window, cx);
            // The cursor starts on the model in use.
            let picked = self.model.as_ref().map(|model| model.id.clone());
            self.model_cursor = self
                .menu_rows()
                .iter()
                .position(|row| match row {
                    MenuRow::Auto => picked.is_none(),
                    MenuRow::Model(ix) => picked.as_deref() == Some(self.models[*ix].id.as_str()),
                    MenuRow::Older(_) => false,
                })
                .unwrap_or(0);
            self.model_cursor_on_open = true;
            self.reveal_model_cursor();
        } else {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    fn pick_model(&mut self, model: Option<ModelChoice>, window: &mut Window, cx: &mut Context<Self>) {
        self.model = model;
        self.capture_gateway_choices(cx);
        self.overlay.close_model_menu();
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        // Onboarding has no sidebar (`onboarding.rs`): Cmd+B (Ctrl+B) there would change, unseen, the one that returns.
        if self.onboarding.active() {
            return;
        }
        self.sidebar_open = !self.sidebar_open;
        self.sidebar_toggles += 1;
        cx.notify();
    }

    fn on_new_chat(&mut self, _: &NewChat, window: &mut Window, cx: &mut Context<Self>) {
        self.start_new_session(window, cx);
    }

    /// Cmd+J (Ctrl+J): the Assistant, as its sidebar row. Without its row (no agent API, signed out, the panel) the
    /// key does nothing here.
    fn on_open_assistant(&mut self, _: &OpenAssistant, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel || !self.agent || !matches!(self.auth, Auth::SignedIn) {
            cx.propagate();
            return;
        }
        // Over typed text in the palette, Cmd+J hands the text off, as the palette's first row.
        if self.overlay.palette().is_some() {
            let text = self.hand_off_text();
            self.close_palette(window, cx);
            if let Some(text) = text {
                return self.hand_off(text, window, cx);
            }
        }
        self.open_agent(None, window, cx);
    }

    /// Cmd+Shift+J (Ctrl+Shift+J): a new errand. Over typed text in the palette, the box holds that text.
    /// ⌃⌘O (Ctrl+Alt+O): open the account switcher, or close it, as the sidebar's header does.
    fn on_switch_account(&mut self, _: &SwitchAccount, _: &mut Window, cx: &mut Context<Self>) {
        if self.panel || !matches!(self.auth, Auth::SignedIn) {
            cx.propagate();
            return;
        }
        self.toggle_org_menu(cx);
    }

    fn on_new_errand(&mut self, _: &NewErrand, window: &mut Window, cx: &mut Context<Self>) {
        if !self.assistant_available() {
            cx.propagate();
            return;
        }
        let mut text = String::new();
        if self.overlay.palette().is_some() {
            text = self.hand_off_text().unwrap_or_default();
            self.close_palette(window, cx);
        }
        self.new_errand(text, window, cx);
    }

    fn on_toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_sidebar(cx);
    }

    fn on_find_chat(&mut self, _: &FindChat, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_open() {
            return;
        }
        if self.overlay.palette().is_some() {
            self.close_palette(window, cx);
        } else {
            self.open_palette(window, cx);
        }
    }

    /// Esc and Tab, before the composer sees them: Esc closes the model menu or stops the answer, and Tab in an empty
    /// composer takes the suggestion. Otherwise the keys go on to the composer as usual.
    fn on_keystroke(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        // Esc during a session or project drag cancels it, before anything else takes the key (`session_drag.rs`,
        // `project_drag.rs`).
        if keystroke.key == "escape" && (self.cancel_session_drag(window, cx) || self.cancel_project_drag(window, cx)) {
            cx.stop_propagation();
            return;
        }
        if self.update_restart_key(keystroke, window, cx) {
            return;
        }
        if self.project_dialog_key(keystroke, window, cx) {
            return;
        }
        if self.artifact_picker_key(keystroke, window, cx) {
            return;
        }
        if self.missing_dialog_key(keystroke, window, cx) {
            return;
        }
        if self.side.side_chat_visible
            && self
                .open_side_chat()
                .is_some_and(|chat| chat.read(cx).focused(window, cx))
        {
            return;
        }
        if self.crew_map_focused(window, cx) {
            return;
        }
        if matches!(self.view, View::Settings(_)) && self.settings_keystroke(keystroke, window, cx) {
            return;
        }
        // Settings › App is recording a new global shortcut: every key is its (`prefs.rs`).
        if crate::prefs::recording() {
            return;
        }
        if self.chat_share.as_ref().is_some_and(|sheet| {
            !sheet.read(cx).closed
                && matches!(self.view, View::Chat)
                && self.current.as_deref() == Some(&sheet.read(cx).session)
        }) {
            return;
        }
        if self.file_picker_keystroke(keystroke, window, cx) {
            cx.stop_propagation();
            return;
        }
        if self.file_picker_open() {
            return;
        }
        let share_shortcut = if cfg!(target_os = "macos") {
            keystroke.modifiers.platform
        } else {
            keystroke.modifiers.control
        };
        // A chat's Share. In a Code session the key archives instead (`session_keys.rs`), as the Nexus app's Code page
        // claims its keys from the frame.
        if share_shortcut
            && matches!(self.view, View::Chat)
            && keystroke.modifiers.shift
            && !keystroke.modifiers.alt
            && keystroke.key.eq_ignore_ascii_case("s")
        {
            self.open_chat_share(window, cx);
            cx.stop_propagation();
            return;
        }
        let plain = !keystroke.modifiers.modified();
        if self.paste_as_text(keystroke, window, cx) {
            cx.stop_propagation();
            return;
        }
        let key = keystroke.key.as_str();
        self.note_composer_key(window, cx);
        self.prepare_slash_key(window, cx);
        // Esc in a Code session's find field closes find, not the agent's turn (`code_find.rs`).
        if plain && self.code_find_key(key, window, cx) {
            cx.stop_propagation();
            return;
        }
        if plain && key == "escape" && self.chat_skill_dismiss(cx) {
            cx.stop_propagation();
            return;
        }
        // An image shown full size takes the keys, as a dialog: Esc closes it, ← → step (`image_viewer.rs`).
        if self.image_viewer_key(key, cx) {
            cx.stop_propagation();
            return;
        }
        if self.overlay.palette().is_some() {
            if (plain || key == "tab") && self.palette_key(key, keystroke.modifiers.shift, window, cx) {
                cx.stop_propagation();
            }
            return;
        }
        let modifiers = keystroke.modifiers;
        if !self.commands_modal_open(cx)
            && keystroke.key == "n"
            && !modifiers.shift
            && modifiers.control
            && if cfg!(target_os = "macos") {
                modifiers.platform && !modifiers.alt
            } else {
                modifiers.alt && !modifiers.platform
            }
        {
            let moved = if self.side.open && self.side_tool() == side_pane::Tool::Files {
                self.detach_files(window, cx)
            } else if self.side.open && self.side_tool() == side_pane::Tool::Terminal {
                self.detach_commands(window, cx)
            } else {
                self.detach_browser(window, cx)
            };
            if let Err(error) = moved {
                self.composer_note = Some(error.into());
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // The terminal's keys are the shell's: Esc there does not stop the agent.
        if self.side_terminal_focused(window, cx) {
            return;
        }
        if self.on_filter_keystroke(keystroke, window, cx) {
            cx.stop_propagation();
            return;
        }
        // A transient drawer closes and returns the keys to the composer.
        if plain
            && key == "escape"
            && self.side.open
            && self.side.layout.drawer
            && !self.agent_panel_focused(window, cx)
            && !self.side.maximized
            && !self.page_focused(window, cx)
        {
            self.close_side_drawer(window, cx);
            cx.stop_propagation();
            return;
        }
        // Esc puts a maximized Workbench back beside the session. A page with the keys has its own Esc: a canvas says
        // when it has nothing to clear, and that leaves full view (`page_canvas.rs`).
        if plain && key == "escape" && self.side.open && self.side.maximized && !self.page_focused(window, cx) {
            self.toggle_maximize_side(window, cx);
            cx.stop_propagation();
            return;
        }
        // Mod+Backspace in the empty message box deletes the session, as the Nexus app's (`emptyBox`); with text in it,
        // or in another box, the key deletes text.
        if self.composer_empty
            && self.on_code_screen()
            && self.composer.focus_handle(cx).is_focused(window)
            && crate::key_bindings::pressed("delete-session", keystroke)
        {
            self.on_delete_session(&session_keys::DeleteSession, window, cx);
            cx.stop_propagation();
            return;
        }
        // Esc in a sub-agent's view goes back to the conversation; it never stops the lead (`agent_view.rs`).
        if plain && key == "escape" && self.agent_view_open() {
            self.close_agent_view(cx);
            cx.stop_propagation();
            return;
        }
        // Ctrl+1 to Ctrl+9 (Cmd on macOS) in the new session's model menu: the favorite model at that place, as the
        // menu's hints show. Elsewhere the keys jump to a session, as the Nexus app's (`session_keys.rs`).
        let only_shortcut = if cfg!(target_os = "macos") {
            keystroke.modifiers.platform && !keystroke.modifiers.control
        } else {
            keystroke.modifiers.control && !keystroke.modifiers.platform
        } && !keystroke.modifiers.alt
            && !keystroke.modifiers.shift;
        if only_shortcut
            && matches!(self.view, View::NewCode)
            && self.overlay.launch_menu() == Some(new_code::LaunchMenu::Model)
            && let Some(place) = key.parse::<usize>().ok().filter(|digit| (1..=9).contains(digit))
            && self.pick_favorite(place - 1, window, cx)
        {
            cx.stop_propagation();
            return;
        }
        // The project menu's search: Up, Down and Enter move through its rows, Tab and Shift+Tab cycle the scope, and
        // Esc clears the search (an empty one's Esc closes the menu below).
        let shift_only = keystroke.modifiers.shift
            && !keystroke.modifiers.control
            && !keystroke.modifiers.platform
            && !keystroke.modifiers.alt;
        if (plain || (key == "tab" && shift_only))
            && self.overlay.launch_menu() == Some(new_code::LaunchMenu::Project)
            && self.repo_search().read(cx).focus_handle(cx).is_focused(window)
            && self.project_menu_key(key, keystroke.modifiers.shift, window, cx)
        {
            cx.stop_propagation();
            return;
        }
        let session_picker = self.dock_menu_is(code_dock::Menu::Model);
        let launch_model = self.overlay.launch_menu() == Some(new_code::LaunchMenu::Model);
        let local_picker =
            launch_model && self.is_local() && self.window_handle.window_id() == window.window_handle().window_id();
        if local_picker && key == "tab" && (plain || shift_only) {
            if shift_only {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            cx.stop_propagation();
            return;
        }
        if (launch_model || session_picker) && plain {
            if local_picker && key == "enter" && !self.model_search.read(cx).focus_handle(cx).is_focused(window) {
                return; // Enter activates the focused local provider or model.
            }
            self.sync_model_rows();
            let rows = self.launch_rows();
            // The agents that can't run here follow the models: the fold, then each agent while it is open.
            let missing = self.missing_panel_rows();
            let count = rows.len() + missing.len();
            match key {
                "up" | "down" => {
                    self.model_cursor = picker_navigation::step(self.model_cursor, count, key == "down");
                    self.reveal_model_cursor();
                }
                "enter" => {
                    if let Some(row) = rows.get(self.model_cursor).cloned() {
                        self.pick_model_row(row, window, cx);
                    } else if let Some(row) = self.missing_at_cursor(rows.len(), &missing) {
                        self.pick_missing_row(row, window, cx);
                    }
                }
                "escape" => {
                    self.dismiss_code_model_menu(window, cx);
                }
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.overlay.model_menu() && plain {
            self.sync_model_rows();
            let rows = self.menu_rows();
            let count = rows.len();
            match key {
                "up" | "down" => {
                    self.model_cursor = picker_navigation::step(self.model_cursor, count, key == "down");
                    self.reveal_model_cursor();
                }
                "enter" => {
                    if let Some(row) = rows.get(self.model_cursor).copied() {
                        self.pick_menu_row(row, window, cx);
                    }
                }
                "escape" => {
                    self.overlay.close_model_menu();
                    self.focus_composer(window, cx);
                }
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // The prompt stash's menu (`stash.rs`): Up, Down, Enter, Esc, and Delete (or Mod+Backspace) deletes.
        if self.overlay.stash_menu() {
            let secondary = if cfg!(target_os = "macos") {
                keystroke.modifiers.platform
            } else {
                keystroke.modifiers.control
            };
            let took = match key {
                "up" if plain => self.stash_move(false, cx),
                "down" if plain => self.stash_move(true, cx),
                "enter" if plain => self.stash_pick_active(window, cx),
                "delete" if plain => self.stash_delete_active(cx),
                "backspace" if secondary => self.stash_delete_active(cx),
                "escape" if plain => {
                    self.focus_composer(window, cx);
                    self.stash_dismiss(cx)
                }
                _ => false,
            };
            if took {
                cx.stop_propagation();
                return;
            }
        }
        if plain && key == "escape" {
            // Esc shuts whatever overlay or title field is open, and then leaves Settings. It never reaches a running
            // turn's Stop or an approval's Deny while one is open (`escape_tests.rs`).
            if self.escape_overlay(window, cx) {
                cx.stop_propagation();
                return;
            }
            // Then the pull request form under the top bar, as before it became a separate slot.
            if self.pr_form.take().is_some() {
                cx.stop_propagation();
                cx.notify();
                return;
            }
            if self.renaming.is_some() || self.chat_rename.is_some() {
                if self.renaming.take().is_some() {
                    self.restore_session_anchor(window, cx);
                }
                self.chat_rename = None;
                cx.stop_propagation();
                cx.notify();
                return;
            }
            if matches!(self.view, View::Settings(_)) {
                if !self.settings.shell.query.read(cx).value().is_empty() {
                    self.settings
                        .shell
                        .query
                        .update(cx, |state, cx| state.set_value("", window, cx));
                    self.settings.shell.query_text.clear();
                    self.settings.shell.searching = false;
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                self.back_to_app(window, cx);
                cx.stop_propagation();
                return;
            }
            if let View::Pull(page) = &self.view {
                let (thread_id, escape) = page
                    .clone()
                    .update(cx, |page, cx| (page.thread_id().to_string(), page.escape(window, cx)));
                match escape {
                    crate::pages::pull_request::Escape::Pass => {}
                    crate::pages::pull_request::Escape::Handled => {
                        cx.stop_propagation();
                        return;
                    }
                    crate::pages::pull_request::Escape::Back => {
                        self.open_code(&thread_id, window, cx);
                        cx.stop_propagation();
                        return;
                    }
                }
            }
        }
        match key {
            "escape" if self.overlay.model_menu() || self.overlay.person_menu() || self.overlay.mic_menu() => {
                self.overlay.close_model_menu();
                self.overlay.close_person_menu();
                self.close_mic_menu();
                cx.stop_propagation();
                cx.notify();
            }
            // Esc closes Rewind's confirmation, as the Nexus app's popover (the editor takes its own Esc).
            "escape" if self.chat_rewind.as_ref().is_some_and(|open| open.editor.is_none()) => {
                self.cancel_chat_rewind(window, cx);
                cx.stop_propagation();
            }
            "escape" if self.dictation.busy() => {
                self.stop_dictation(cx);
                cx.stop_propagation();
            }
            "escape" if plain && self.agent_panel_focused(window, cx) => {
                self.back_to_agents(window, cx);
                cx.stop_propagation();
            }
            _ if self.first_connection_key(keystroke, window, cx) => {
                cx.stop_propagation();
            }
            // Menus and dictation keep their Esc before a tool approval can deny (`approval_keys.rs`).
            _ if plain && self.approval_key(key, window, cx) => {
                cx.stop_propagation();
            }
            // A question's number keys, Enter and arrows, when no text field has the keys (`code_dock.rs`).
            _ if plain && self.question_key(key, window, cx) => {
                cx.stop_propagation();
            }
            "escape" if self.esc_stops_turn() => {
                self.stop(window, cx);
                cx.stop_propagation();
            }
            // Esc in the panel's Settings goes back to the chat.
            "escape" if self.panel && plain && self.panel_settings.is_some() => {
                self.close_panel_settings(window, cx);
                cx.stop_propagation();
            }
            // Esc with nothing else to shut closes the panel.
            "escape" if self.panel && plain => {
                crate::panel::hide_later(cx);
                cx.stop_propagation();
            }
            // Cmd+, (Ctrl+, elsewhere) opens the panel's Settings.
            "," if self.panel && only_shortcut => {
                self.open_panel_settings(window, cx);
                cx.stop_propagation();
            }
            // Cmd+Enter (Ctrl+Enter elsewhere) continues the panel's chat in the app window.
            "enter" if self.panel && only_shortcut => {
                self.open_in_app(cx);
                cx.stop_propagation();
            }
            "tab" if plain && self.composer_empty && self.composer.focus_handle(cx).is_focused(window) => {
                if let Some(suggestion) = self.suggestion.take() {
                    self.composer.update(cx, |state, cx| {
                        state.set_value(suggestion.clone(), window, cx);
                        state.focus(window, cx);
                    });
                    self.composer_empty = false;
                    cx.stop_propagation();
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    /// The window head in two parts: over the sidebar, in its tone, the sidebar toggle, search and back and forward;
    /// over the main pane, its title. On macOS the head starts with an empty slot where the OS draws the traffic
    /// lights; on Windows and Linux the window controls sit at the bar's right end (GPUI Kit's `TitleBar`). With the
    /// sidebar shut, the bar also carries New session and Inbox, at the same x as the sidebar's head.
    ///
    /// With the Workbench docked, the bar is three column heads on one line: the sidebar's, the session's, and the
    /// Workbench's tab strip (`side_head`), each over its own column, with the dividers running up to the window's top
    /// edge, as in Cursor. Maximized (`side_maximized`), the Workbench's head takes the session's place.
    fn render_title_bar(
        &self,
        side_head: Option<AnyElement>,
        side_maximized: bool,
        show_navigation: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // GPUI Kit's bar pads its content for the window controls (80 on macOS, 12 elsewhere); the head reaches back
        // over that padding and sets its own: the traffic-light slot on macOS, the sidebar's icon column elsewhere.
        let kit_inset = if cfg!(target_os = "macos") { 80. } else { 12. };
        let open = self.sidebar_open;
        // Inside the sidebar card, the head starts and ends `frost::INSET` further in.
        let card = if open && show_navigation {
            crate::frost::INSET
        } else {
            0.
        };
        let lead = 10.
            + card
            + if cfg!(target_os = "macos") {
                crate::tokens::TRAFFIC_LIGHTS + 2.
            } else {
                0.
            };
        let head = h_flex()
            .relative()
            .h_full()
            .flex_none()
            .ml(px(-kit_inset))
            .pl(px(lead))
            .gap(px(2.))
            .when(open && show_navigation, |head| {
                // Forward's icon ends 10 px in from the card's right edge (`sidebar_card`, under the head).
                head.w(px(self.sidebar_width() + crate::frost::GUTTER))
                    .pr(px(10. + crate::frost::INSET))
            })
            .when(!open, |head| head.pr(px(8.)))
            .when(show_navigation, |head| {
                head.child(
                    bar_button("toggle-sidebar", Lucide::PanelLeft, palette)
                        .tooltip(|window, cx| {
                            Tooltip::new("Toggle sidebar")
                                .action(&ToggleSidebar, None)
                                .build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
                )
                .child(
                    bar_button("search", Lucide::Search, palette)
                        .when(!self.overlay.palette().is_some(), |button| {
                            button
                                .tooltip(|window, cx| Tooltip::new("Search").action(&FindChat, None).build(window, cx))
                        })
                        .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx))),
                )
                .when(!open, |head| {
                    head.child(
                        bar_button("title-new-chat", Lucide::SquarePen, palette)
                            .tooltip(|window, cx| Tooltip::new("New session").action(&NewChat, None).build(window, cx))
                            .on_click(cx.listener(|this, _, window, cx| this.start_new_session(window, cx))),
                    )
                    .child(self.render_inbox_button(palette, cx))
                    // 8 before Back: a 4 spacer between the two 2 gaps.
                    .child(div().w(px(4.)).flex_none())
                })
                .when(open, |head| head.child(div().flex_1()))
                .child(
                    nav_button("back", Lucide::ArrowLeft, self.history_at > 0, palette)
                        .tooltip(|window, cx| Tooltip::new(format!("Back  {BACK_HINT}")).build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.go(-1, window, cx))),
                )
                .child(
                    nav_button(
                        "forward",
                        Lucide::ArrowRight,
                        self.history_at + 1 < self.history.len(),
                        palette,
                    )
                    .tooltip(|window, cx| Tooltip::new(format!("Forward  {FORWARD_HINT}")).build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.go(1, window, cx))),
                )
            });
        // Every head is 44 (`tokens::HEAD`); GPUI Kit's is 34. No hairline under it: GPUI Kit draws a bottom border,
        // which cut across the sidebar's tone. The head is art-free: a screen's art starts under it.
        // It starts `frost::INSET` below the window's top, as the sidebar card does, and is clear: the window's ground
        // and the card show under it.
        TitleBar::new()
            .h(px(crate::tokens::HEAD + crate::frost::INSET))
            .pt(px(crate::frost::INSET))
            .border_b_0()
            .bg(transparent_black())
            .child(
                without_text_selection(h_flex().id("title-bar-content"))
                    // GPUI Kit's bar never shrinks below its content's width, so a crowded title ran past the window's
                    // edge. A zero basis that grows takes the bar's width, not its content's (`title_fit.rs`).
                    .h_full()
                    .w(px(0.))
                    .min_w(px(0.))
                    .flex_grow(1.)
                    // A double click follows the desktop's setting (`title_bar.rs`), before GPUI Kit's bar maximizes.
                    // Controls and the title's rename stop the click first.
                    .when(crate::title_bar::HANDLED_HERE, |bar| {
                        bar.on_click(cx.listener(|_, event: &ClickEvent, window, cx| {
                            if event.click_count() == 2 {
                                cx.stop_propagation();
                                crate::title_bar::double_click(event.position(), window, cx);
                            }
                        }))
                    })
                    .child(head)
                    .when(show_navigation, |bar| {
                        // The session's head holds the Workbench's open button while it is closed. The Workbench's
                        // head holds the close button when it is open.
                        let title = (!side_maximized).then(|| match &self.view {
                            View::Code(view) => self.render_code_title(&view.id, palette, cx).into_any_element(),
                            View::TeamSessions => div().flex_1().child("Team sessions").into_any_element(),
                            // The thread's empty title with ⋯, and an errand's crumb (`assistant_title.rs`).
                            View::Page(id) if self.assistant_titled(*id, cx) => {
                                self.render_assistant_title(palette, cx).into_any_element()
                            }
                            _ => self.render_chat_title(palette, cx).into_any_element(),
                        });
                        // Maximized, there is no title: its empty column would split the bar with the Workbench's head.
                        bar.when_some(title, |bar, title| {
                            bar.child(h_flex().flex_1().min_w(px(0.)).h_full().overflow_hidden().child(title))
                        })
                        .children(
                            (!self.workbench_hidden())
                                .then(|| self.render_side_toggle(palette, cx))
                                .flatten(),
                        )
                        .children(side_head)
                    }),
            )
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_model_rows();
        let _span = crate::watchdog::span("Workspace::render");
        self.sync_team_presence(window, cx);
        self.sync_chat_skills(cx);
        self.cite_window_changed(window);
        self.prepare_question_fields(window, cx);
        self.focus_approval(window, cx);
        self.settle_last_place(window, cx);
        if matches!(self.view, View::Code(_) | View::NewCode) {
            self.probe_plans(cx);
        }
        if self.panel {
            self.sync_chat_column(window, cx);
            return div()
                .id("panel-text-menu")
                .size_full()
                .child(self.usage_listener(window))
                .child(crate::selection::release_ends_selection())
                .child(self.render_panel(window, cx))
                .context_menu(message_text::selection_menu)
                .into_any_element();
        }
        let palette = Palette::for_choice(self.theme, window.appearance());
        let look = crate::frost::Look::now(&palette);
        cx.set_global(crate::pages::Theme(palette));
        // A page left behind stops with it: its refresh timers end with the view.
        if !matches!(self.view, View::Page(_)) {
            self.native_page = None;
        }
        // The Assistant's page stays, and knows whether the person looks at it.
        self.sync_assistant(cx);
        // Settings' views stop with it too.
        if !matches!(self.view, View::Settings(_)) {
            self.settings.shell.account_ctx = None;
            self.settings.shell.views.clear();
        }
        // What the person looks at, so its session is not notified (`notify.rs`). A server's session goes by its
        // machine too, as a server's notifications name it.
        let on_screen = match &self.view {
            View::Code(view) => Some(view.id.as_str()),
            View::Pull(page) => Some(page.read(cx).thread_id()),
            _ => None,
        };
        let on_screen = on_screen.map(|id| {
            let machine = self.session_by_id(id).and_then(|session| session.machine.as_deref());
            crate::notify::screen_key(machine, id)
        });
        crate::notify::viewing(
            window.window_handle().window_id().as_u64(),
            window.is_window_active(),
            on_screen.as_deref(),
        );
        apply_kit_theme(Palette::is_dark(self.theme, window.appearance()), window, cx);
        // The composer holds the draft of what is on screen (`drafts.rs`).
        self.sync_browser_return(window, cx);
        self.sync_files_return(window, cx);
        self.sync_commands_return(window, cx);
        self.sync_draft(window, cx);
        self.sync_placeholder(window, cx);
        self.sync_side(window, cx);
        self.sync_auto_preview(window, cx);
        self.sync_workbench_navigation(window, cx);
        self.sync_chat_column(window, cx);
        self.sync_title_fit(window);
        // The Workbench opening, closing or giving back the room changes the transcript's width, and so its height: one
        // read to its end stays at its end.
        let side_layout = (self.side.open, self.side.maximized);
        if side_layout != self.side_layout {
            self.side_layout = side_layout;
            if transcript::near_bottom(&self.scroll) {
                self.scroll.scroll_to_bottom();
            }
        }
        // Files dragged out of the window, or dropped: the composer's drop outline goes.
        if self.file_drag && !cx.has_active_drag() {
            self.file_drag = false;
            cx.notify();
        }
        let focused = self.composer.focus_handle(cx).is_focused(window);
        self.composer_focused = focused;
        self.settle_new_session(window, cx);
        self.sync_backdrop_motion(palette, window, cx);
        // The motion runs on a screen without a Workbench too, so a pane closed there has settled when the person
        // leaves, and does not slide out then.
        let side_frame = self.sync_side_motion(cx).filter(|_| !self.workbench_hidden());
        // The Workbench, and its head for the title bar when it is docked or maximized.
        let onboarding = matches!(self.auth, Auth::SignedIn) && self.onboarding.active();
        let show_navigation = self.workspace_open() && !onboarding;
        let (side_pane, side_head) = if show_navigation {
            self.render_side_pane(side_frame, palette, window, cx)
                .map_or((None, None), |(pane, head)| (Some(pane), head))
        } else {
            (None, None)
        };
        let side_maximized = side_frame.is_some_and(|frame| frame.expanded_at_rest());
        let body = match self.auth {
            // First-run onboarding, in place of the workspace (`onboarding.rs`).
            Auth::SignedIn if onboarding => self.render_onboarding(palette, window, cx),
            // The Workbench maximized: in place of the session and its composer. While it grows, shrinks or leaves,
            // the session stays beside it and gives way frame by frame.
            Auth::SignedIn | Auth::Local if side_maximized => h_flex()
                .relative()
                .size_full()
                .child(self.render_sidebar(cx))
                .children(side_pane)
                .into_any_element(),
            Auth::SignedIn | Auth::Local => {
                let landing = self.onboarding.landing();
                h_flex()
                    .relative()
                    .size_full()
                    .child(self.render_sidebar(cx))
                    // The main area is a cached view (`pane.rs`): a page's frame does not lay it out again. After
                    // onboarding it rises in once, as the sidebar opens (`onboarding.rs`).
                    .child(onboarding::land(
                        landing,
                        div()
                            .relative()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .children(self.panes.as_ref().map(|(_, center)| {
                                // A chat's or a Code session's transcript is a cached pane of its own (`pane.rs`), and
                                // GPUI draws again every cached view inside one it draws again: there, the main area
                                // is not cached, so a caret blink keeps the transcript and draws only the composer.
                                if matches!(self.view, View::Chat | View::Code(_)) {
                                    return center.clone().into_any_element();
                                }
                                let mut style = StyleRefinement::default();
                                style.size.width = Some(relative(1.).into());
                                style.size.height = Some(relative(1.).into());
                                center.clone().cached(style).into_any_element()
                            }))
                            // The closed Workbench's tools, over the session's right gutter.
                            .children(self.render_workbench_overview(palette, window, cx)),
                    ))
                    .children(side_pane)
                    .into_any_element()
            }
            Auth::Restoring => self.render_startup_shell(palette),
            _ => self.render_signed_out(palette, cx),
        };
        v_flex()
            .id("workspace")
            .key_context("Workspace")
            .relative()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_new_chat))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_find_chat))
            .on_action(cx.listener(Self::on_find_in_chat))
            .on_action(cx.listener(Self::on_stash_prompt))
            .on_action(cx.listener(Self::on_toggle_side_pane))
            .on_action(cx.listener(Self::on_toggle_terminal))
            .on_action(cx.listener(Self::on_toggle_chat_browser))
            .on_action(cx.listener(Self::on_new_terminal))
            .on_action(cx.listener(Self::on_toggle_maximize_workbench))
            .on_action(cx.listener(Self::on_toggle_page_full_view))
            .on_action(cx.listener(Self::on_side_tool_1))
            .on_action(cx.listener(Self::on_side_tool_2))
            .on_action(cx.listener(Self::on_side_tool_3))
            .on_action(cx.listener(Self::on_side_tool_4))
            .on_action(cx.listener(Self::on_side_tool_5))
            .on_action(cx.listener(Self::on_side_tool_6))
            .map(|root| self.session_key_actions(root, cx))
            .on_key_down(cx.listener(Self::on_command_key))
            .on_action(cx.listener(|this, _: &GoBack, window, cx| {
                // Right after a switch with the Assistant open, ⌘[ switches back.
                if !this.switch_back(window, cx) {
                    this.go(-1, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &GoForward, window, cx| this.go(1, window, cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings_from_keys(window, cx)))
            .on_action(cx.listener(Self::on_open_assistant))
            .on_action(cx.listener(Self::on_switch_account))
            .on_action(cx.listener(Self::on_new_errand))
            // The mouse's back and forward buttons (X11's buttons 8 and 9, Wayland's BTN_SIDE and BTN_EXTRA), as in a
            // browser. The session browser takes them first for its page (`side/browser`).
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _: &MouseDownEvent, window, cx| this.go(-1, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _: &MouseDownEvent, window, cx| this.go(1, window, cx)),
            )
            // Files dragged from a file manager: anywhere on the window, they attach to the composer on screen, as
            // in the Nexus app. A terminal takes a drop on itself first and types the paths.
            .on_drag_move(cx.listener(|this, _: &DragMoveEvent<ExternalPaths>, _, cx| {
                let shown = this.composer_shown();
                if this.file_drag != shown {
                    this.file_drag = shown;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.file_drag = false;
                if this.side.side_chat_visible
                    && let Some(chat) = this.open_side_chat()
                {
                    chat.update(cx, |chat, cx| chat.attach_paths(paths.paths().to_vec(), cx));
                } else {
                    this.attach_paths(paths.paths().to_vec(), cx);
                }
            }))
            .size_full()
            .text_color(palette.foreground)
            // The window's wash (`frost.rs`). Off onboarding, the veil over all of it but the sidebar card, then the card.
            // First-run setup keeps its own layout: no card hole, no backdrop, no card.
            .child(crate::frost::ground(look))
            .when(!onboarding, |root| {
                root.children(self.render_pane_ground(look))
                    .child(self.render_backdrop_layer(palette, cx))
                    .children(self.render_sidebar_card(look))
            })
            .child(self.usage_listener(window))
            .child(crate::selection::release_ends_selection())
            .child(self.render_title_bar(side_head, side_maximized, show_navigation, palette, cx))
            .child(div().flex_1().min_h(px(0.)).child(body))
            .when(
                self.overlay.model_menu()
                    || self.overlay.person_menu()
                    || self.overlay.attach_menu()
                    || self.overlay.mic_menu(),
                |root| {
                    // Under the open menu, over everything else: a click anywhere outside the menu closes it, the
                    // menu's own button included, which would otherwise close and reopen it.
                    root.child(
                        deferred(
                            div()
                                .id("menu-backdrop")
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .occlude()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        let was_model_menu = this.overlay.model_menu();
                                        this.overlay.close_model_menu();
                                        this.overlay.close_person_menu();
                                        this.overlay.close_attach_menu();
                                        this.close_mic_menu();
                                        if was_model_menu {
                                            this.focus_composer(window, cx);
                                        }
                                        cx.notify();
                                    }),
                                ),
                        )
                        .with_priority(0),
                    )
                },
            )
            .when(self.overlay.palette().is_some(), |root| {
                root.child(deferred(self.render_palette(palette, cx)).with_priority(2))
            })
            .children(self.render_update_toast(palette, cx))
            .children(self.render_revoked_notice(palette, cx))
            .children(
                self.account_gateway()
                    .is_some()
                    .then(|| self.render_update_recovery(palette, cx))
                    .flatten(),
            )
            .children(self.render_update_restart(palette, cx))
            .children(self.render_notice(palette, cx))
            .children(self.render_file_picker())
            .children(self.render_crash_prompt(palette, cx))
            .children(self.render_usage_prompt(palette, cx))
            .children(self.render_link_menu())
            .children(self.render_feedback(palette, cx))
            .children(self.render_project_dialog(palette, cx))
            .children(self.render_missing_dialog(palette, cx))
            .children(self.render_chat_share(cx))
            .children(self.render_library_dialog(cx))
            .children(self.render_artifact_picker(palette, cx))
            .children(self.render_session_share(palette, cx))
            .children(self.render_outside_prompt(palette, cx))
            .children(
                self.render_image_viewer(palette, window, cx)
                    .map(|viewer| deferred(viewer).with_priority(3)),
            )
            .context_menu(message_text::selection_menu)
            .into_any_element()
    }
}

impl Workspace {
    /// The main area: the chat, a Code session, or the library.
    pub(super) fn render_center(
        &mut self,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.connection_open() {
            return self.render_first_connection(palette, cx);
        }
        self.follow_session_start(window, cx);
        self.refresh_code_find(cx);
        self.refresh_code_jump(cx);
        if matches!(self.view, View::NewCode | View::Code(_))
            && let Some(blocker) = self.render_code_handoff(palette, cx)
        {
            return blocker;
        }
        match &self.view {
            View::Chat => self.render_chat(palette, cx).into_any_element(),
            View::Code(view) if self.agent_view_open() => self.render_agent_view(view, palette, cx),
            View::Code(view) => self.render_code(view, palette, cx).into_any_element(),
            View::NewCode => self.render_new_code(palette, cx).into_any_element(),
            View::Inbox => self.render_inbox(palette, cx).into_any_element(),
            View::Docs => self.render_docs(palette, cx).into_any_element(),
            View::Issues => self.render_issues(palette, cx).into_any_element(),
            View::TeamSessions => self.render_team_sessions(palette, cx),
            View::Library => {
                let sidebar = if self.sidebar_open { self.sidebar_room() } else { 0. };
                // The Workbench beside it takes its own width (it is not maximized: the library would not show).
                let side = if self.side.open { self.side_width(window) } else { 0. };
                let width = f32::from(window.viewport_size().width) - sidebar - side;
                self.render_library(width, palette, cx).into_any_element()
            }
            View::Settings(section) => {
                let section = *section;
                self.render_settings(section, palette, window, cx).into_any_element()
            }
            View::Models => self.render_models(palette, cx).into_any_element(),
            View::Pull(page) => div().size_full().child(page.clone()).into_any_element(),
            View::Page(_) => match &self.native_page {
                // The Assistant's views, under the strip that moves between them; the thread without it (`assistant.rs`).
                Some((id, view)) if self.assistant_strip(*id, cx) => {
                    self.render_assistant(*id, view.clone(), palette, cx)
                }
                Some((_, view)) => div().size_full().child(view.clone()).into_any_element(),
                None => div().into_any_element(),
            },
        }
    }
}

/// A small icon in the given color, for rows and buttons drawn by hand.
pub(crate) fn glyph(name: impl Into<Icon>, color: Hsla) -> Icon {
    Icon::new(name).size(px(14.)).text_color(color)
}

/// The chevron of a group that folds: pointing right while closed, turning down as it opens (Zeron's `COLLAPSE`).
/// `id` keeps its turn across frames; it is local to the row it sits in.
pub(super) fn disclosure(id: impl Into<ElementId>, open: bool, color: Hsla) -> impl IntoElement {
    use crate::motion::{self, TweenExt as _};
    glyph(Lucide::ChevronRight, color).with_tween(
        id,
        if open { 1. } else { 0. },
        motion::Tween::new(motion::COLLAPSE, motion::ease_out),
        |icon, turn| icon.rotate(radians(std::f32::consts::FRAC_PI_2 * turn)),
    )
}

/// Back or forward: faint, and inert, when there is nowhere to go.
fn nav_button(id: &'static str, icon: Lucide, enabled: bool, palette: Palette) -> Stateful<Div> {
    if enabled {
        bar_button(id, icon, palette)
    } else {
        div()
            .id(id)
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .child(glyph(icon, palette.icon).size(px(16.)).opacity(0.4))
    }
    .debug_selector(move || id.into())
}

impl Workspace {
    /// Inbox in the collapsed bar: with unread items, a 6px dot at the slot's corner in the Needs-you look.
    fn render_inbox_button(&self, palette: Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let unread = self.inbox;
        bar_button("title-inbox", Lucide::Inbox, palette)
            .relative()
            .tooltip(move |window, cx| {
                let label = if unread > 0 {
                    format!("Inbox  {unread} unread")
                } else {
                    "Inbox".to_string()
                };
                Tooltip::new(label).build(window, cx)
            })
            .when(unread > 0, |button| {
                button.child(
                    div()
                        .absolute()
                        .top(px(1.))
                        .right(px(1.))
                        .size(px(9.))
                        .rounded_full()
                        .border(px(1.5))
                        .border_color(palette.background)
                        .bg(palette.primary),
                )
            })
            .on_click(cx.listener(|this, _, window, cx| this.open_inbox(window, cx)))
    }
}

/// The 1px border line between two panes, full height and layout-neutral: place it with `left_0` or `right`.
pub(super) fn hairline(palette: Palette) -> Div {
    div().absolute().top_0().bottom_0().w(px(1.)).bg(palette.border)
}

/// A 28px icon button in the title bar.
fn bar_button(id: &'static str, icon: Lucide, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(28.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(crate::tokens::radius::CONTROL))
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .active(|style| style.opacity(0.8))
        .child(glyph(icon, palette.icon).size(px(16.)))
        // A press here is the button's, as a menu trigger's is (`MenuTrigger`): it must not start the title bar's
        // window move, which would take the release and the click with it. Registered first, so it runs after the
        // caller's own `on_mouse_down` and `on_click` listeners in the bubble phase. GPUI's press-to-focus runs
        // after it, so a `track_focus` on the button does not take the focus on a press.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// A press on `element` starts no window text selection, and still bubbles to what is under it. For space that moves
/// the window: on Wayland the compositor keeps the release of a window move, so a selection the press began would
/// never end, and the pointer moving over text afterwards would select it.
pub(crate) fn without_text_selection<E: InteractiveElement>(element: E) -> E {
    element.on_mouse_down(MouseButton::Left, |_, _, cx| {
        gpui_kit::base::GlobalState::suppress_text_selection(cx)
    })
}

/// Set the kit's theme (gpui-component's global `Theme`, which its inputs, scrollbars and every Markdown view read) to
/// light or dark, with the interface font. It is one global, shared by every window, and other code can switch its mode
/// (`Theme::change`, which also loads the stock font back): so this asks the kit what it holds now rather than
/// remembering what it set last.
fn apply_kit_theme(dark: bool, window: &mut Window, cx: &mut App) {
    if !kit_theme_is(Theme::global(cx), dark) {
        Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, Some(window), cx);
        Theme::update(cx, |theme| {
            theme.font_family = FONT.into();
            // No hairline under the title bar: the bar and the page share one background.
            theme.title_bar_border = gpui_kit::transparent_black();
        });
    }
    // The kit's own floating things (tooltips, context menus) wear the app's floating tier and hairline, so they follow
    // the Transparency setting with the app's menus (`frost::Tiers`).
    let palette = Palette::solid_for(dark).frosted(crate::frost::shown());
    let theme = Theme::global(cx);
    if theme.popover != palette.menu || theme.border != palette.border || theme.popover_foreground != palette.foreground
    {
        Theme::update(cx, |theme| {
            theme.popover = palette.menu;
            theme.popover_foreground = palette.foreground;
            theme.border = palette.border;
        });
    }
}

/// Whether the kit's theme already is the one `apply_kit_theme` would set: the mode, and the font it sets after.
fn kit_theme_is(theme: &Theme, dark: bool) -> bool {
    theme.mode.is_dark() == dark && theme.font_family.as_ref() == FONT
}

/// Up to two initials of a name, for the avatars: "Ada Lovelace" is "AL", "nexus-xyz" is "NE".
fn initials(name: &str) -> SharedString {
    let words: Vec<&str> = name.split_whitespace().collect();
    let letters: String = match words.as_slice() {
        [first, .., last] => first.chars().take(1).chain(last.chars().take(1)).collect(),
        [only] => only.chars().filter(|c| c.is_alphanumeric()).take(2).collect(),
        [] => "?".into(),
    };
    letters.to_uppercase().into()
}

#[cfg(test)]
mod tests {
    use super::{FONT, Theme, ThemeMode, history_step, initials, kit_theme_is};

    /// A title bar (a button, then plain space that moves the window) over a line of conversation text, laid out at
    /// known places.
    struct BarProbe {
        pressed: std::rc::Rc<std::cell::Cell<usize>>,
        clicked: std::rc::Rc<std::cell::Cell<usize>>,
        /// Presses that reached the bar itself, where GPUI Kit's `TitleBar` starts its window move.
        bar: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl gpui_kit::Render for BarProbe {
        fn render(&mut self, _: &mut gpui_kit::Window, _: &mut gpui_kit::Context<Self>) -> impl gpui_kit::IntoElement {
            use gpui_kit::{InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, div, px};
            let palette = crate::palette::Palette::for_choice(
                crate::palette::ThemeChoice::Light,
                gpui_kit::WindowAppearance::Light,
            );
            let (pressed, clicked, bar) = (self.pressed.clone(), self.clicked.clone(), self.bar.clone());
            div()
                .size_full()
                .text_size(px(14.))
                .line_height(px(22.))
                .child(
                    div()
                        .flex()
                        .h(px(28.))
                        .on_mouse_down(gpui_kit::MouseButton::Left, move |_, _, _| bar.set(bar.get() + 1))
                        .child(
                            super::bar_button("probe-toggle", super::Lucide::PanelRightOpen, palette)
                                .on_mouse_down(gpui_kit::MouseButton::Left, move |_, _, _| {
                                    pressed.set(pressed.get() + 1)
                                })
                                .on_click(move |_, _, _| clicked.set(clicked.get() + 1)),
                        )
                        .child(
                            super::without_text_selection(div().id("probe-space"))
                                .w(px(100.))
                                .h_full(),
                        ),
                )
                .child(
                    div()
                        .mt(px(12.))
                        .w(px(240.))
                        .child(super::message_text::MessageText::new("probe-text", "conversation text")),
                )
        }
    }

    fn bar_probe(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (
        &mut gpui_kit::VisualTestContext,
        [std::rc::Rc<std::cell::Cell<usize>>; 3],
    ) {
        use gpui_kit::AppContext;
        cx.update(gpui_kit::init);
        let counts = [(); 3].map(|_| std::rc::Rc::new(std::cell::Cell::new(0)));
        let probe = BarProbe {
            pressed: counts[0].clone(),
            clicked: counts[1].clone(),
            bar: counts[2].clone(),
        };
        let window = cx.add_window(move |window, cx| {
            let probe = cx.new(|_| probe);
            gpui_kit::base::Root::new(probe, window, cx)
        });
        let cx = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (cx, counts)
    }

    /// The release of a press that starts a window move never comes (the compositor keeps it on Wayland); the
    /// pointer then moves over the conversation's text with no button held. Returns the text that got selected.
    fn press_then_hover(cx: &mut gpui_kit::VisualTestContext, at: gpui_kit::Point<gpui_kit::Pixels>) -> String {
        use gpui_kit::{Modifiers, MouseButton, point, px};
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(point(px(200.), px(50.)), None, Modifiers::none());
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            gpui_kit::base::TextSelection::selected_text(window, cx).to_string()
        })
    }

    /// A title-bar button keeps its press and click, starts no window move, and leaves no text selection running.
    #[gpui_kit::test]
    fn bar_button_press_without_a_release_selects_nothing(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{Modifiers, MouseButton, point, px};
        let (cx, [pressed, clicked, bar]) = bar_probe(cx);
        let button = point(px(14.), px(14.));
        cx.simulate_mouse_down(button, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(button, MouseButton::Left, Modifiers::none());
        assert_eq!((pressed.get(), clicked.get()), (1, 1));
        assert_eq!(press_then_hover(cx, button), "");
        assert_eq!(
            (pressed.get(), bar.get()),
            (2, 0),
            "the bar never hears a button's press"
        );
    }

    /// A press on the bar's own space still reaches the bar, which moves the window, and leaves no text selection
    /// running.
    #[gpui_kit::test]
    fn window_drag_from_bar_space_selects_nothing(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{point, px};
        let (cx, [_, _, bar]) = bar_probe(cx);
        assert_eq!(press_then_hover(cx, point(px(70.), px(14.))), "");
        assert_eq!(bar.get(), 1);
    }

    /// Back and Forward (the keys, the mouse's buttons, the arrows) step one place, and stop at the ends.
    #[test]
    fn back_and_forward_step_one_place_and_stop_at_the_ends() {
        assert_eq!(history_step(0, 1, -1), None);
        assert_eq!(history_step(0, 1, 1), None);
        assert_eq!(history_step(2, 3, -1), Some(1));
        assert_eq!(history_step(1, 3, 1), Some(2));
        assert_eq!(history_step(2, 3, 1), None);
    }

    #[test]
    fn initials_take_the_first_and_last_words() {
        assert_eq!(initials("Ada Lovelace").as_ref(), "AL");
        assert_eq!(initials("nexus-xyz").as_ref(), "NE");
        assert_eq!(initials("").as_ref(), "?");
    }

    /// Started in Light on a dark system, the kit was switched back to dark after the first frame while the
    /// workspace remembered having applied Light, so Markdown kept the dark theme's white text on the light page.
    #[test]
    fn the_kit_theme_is_applied_again_when_its_mode_moved_underneath() {
        let mut theme = Theme::default();
        theme.font_family = FONT.into();
        theme.mode = ThemeMode::Light;
        assert!(kit_theme_is(&theme, false));
        theme.mode = ThemeMode::Dark;
        assert!(!kit_theme_is(&theme, false));
        assert!(kit_theme_is(&theme, true));
        // `Theme::change` loads the stock font back: the interface font is set again.
        theme.font_family = "Inter".into();
        assert!(!kit_theme_is(&theme, true));
    }
}

/// Whether this app has an agent API to ask, from the saved settings and `NEXUS_AGENTS_API_URL`.
fn agent_configured() -> bool {
    nexus_desktop_core::agent::configured(crate::desktop_settings::load().agents_api_url.as_deref())
}
