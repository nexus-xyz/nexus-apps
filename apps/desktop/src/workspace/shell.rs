//! What the app around the window asks of the workspace (`windows.rs`, `tray.rs`): the tray's rows and the menu's state.
//!
//! Every tray row but Ask Nexus acts on an app window, never on the quick panel: New Chat starts a chat, Settings…
//! opens Settings › This computer, and Check for Updates… opens it at Desktop updates. A window opened for the row
//! waits for its sign-in (a recent chat) or its account (the others) and does the row then, in place of its landing.

use gpui_kit::*;

use super::{Auth, SettingsSection, Workspace};
use crate::model::SessionSummary;
use crate::pages::PageId;
use crate::tray::{Command, MenuState};

/// The Settings field Check for Updates… opens at (`settings_registry.rs`).
const UPDATES_FIELD: &str = "desktop-updates";

/// A tray row a window does, kept while the window waits for it (`Workspace::tray_pending`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TrayPlace {
    /// A recent chat, or the chat the panel left: opened once signed in.
    Session(SessionSummary),
    /// New Chat: a new chat, the composer focused.
    NewChat,
    /// Settings…: Settings › This computer.
    Settings,
    /// Check for Updates…, or a refused Restart to Update: Settings › This computer at Desktop updates.
    Updates,
}

impl TrayPlace {
    /// The place a tray row asks of a window. None for the rows the app does (`windows.rs`).
    pub(crate) fn of(command: Command) -> Option<TrayPlace> {
        match command {
            Command::Session { id, title } => Some(TrayPlace::Session(SessionSummary {
                id,
                title,
                day: None,
                at_ms: None,
            })),
            Command::NewChat => Some(TrayPlace::NewChat),
            Command::Settings => Some(TrayPlace::Settings),
            Command::CheckForUpdates | Command::RestartToUpdate => Some(TrayPlace::Updates),
            Command::Feedback
            | Command::Open
            | Command::NewWindow
            | Command::Panel
            | Command::TogglePanel
            | Command::Record
            | Command::RecordCancel
            | Command::Quit => None,
        }
    }

    /// Whether the window can go there now. A chat needs the sign-in. The others wait for the account too, so the
    /// window's landing (`last_place.rs`) does not replace them.
    pub(crate) fn ready(&self, signed_in: bool, account: bool) -> bool {
        match self {
            TrayPlace::Session(_) => signed_in,
            TrayPlace::NewChat | TrayPlace::Settings | TrayPlace::Updates => signed_in && account,
        }
    }
}

impl Workspace {
    /// A tray row that works in the window.
    pub(crate) fn on_tray(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        // The window's feedback dialog.
        if command == Command::Feedback {
            if self.account_gateway().is_some() {
                self.open_feedback(window, cx);
            } else {
                self.show_notice("Sign in to send feedback.", None, std::time::Duration::from_secs(8), cx);
            }
            return;
        }
        if let Some(place) = TrayPlace::of(command) {
            self.go_tray(place, window, cx);
        }
    }

    /// Go to `place` now, or once the window is ready for it ([`Workspace::tray_ready`]).
    pub(crate) fn go_tray(&mut self, place: TrayPlace, window: &mut Window, cx: &mut Context<Self>) {
        if self.tray_place_ready(&place) {
            self.tray_go_now(place, window, cx);
        } else {
            self.tray_pending = Some(place);
        }
    }

    /// The sign-in or the account came back: the row kept for it goes now. Answers whether one went.
    pub(super) fn tray_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.tray_pending.take() {
            Some(place) if self.tray_place_ready(&place) => {
                self.tray_go_now(place, window, cx);
                true
            }
            kept => {
                self.tray_pending = kept;
                false
            }
        }
    }

    fn tray_place_ready(&self, place: &TrayPlace) -> bool {
        (self.is_local() && !matches!(place, TrayPlace::Session(_)))
            || place.ready(matches!(self.auth, Auth::SignedIn), self.account.is_some())
    }

    fn tray_go_now(&mut self, place: TrayPlace, window: &mut Window, cx: &mut Context<Self>) {
        let computer = SettingsSection::Page(PageId::DesktopApp);
        match place {
            TrayPlace::Session(session) => self.open(&session, window, cx),
            TrayPlace::NewChat if self.is_local() => self.start_new_session(window, cx),
            TrayPlace::NewChat => self.new_chat(window, cx),
            TrayPlace::Settings => self.open_settings(computer, window, cx),
            TrayPlace::Updates => self.open_settings_field(computer, UPDATES_FIELD, window, cx),
        }
    }

    /// Show who is signed in and the recent chats in the tray's menu.
    pub(super) fn sync_tray(&self, cx: &mut App) {
        let signed_in = matches!(self.auth, Auth::SignedIn);
        let shortcut = crate::prefs::shortcut(cx).1.active;
        let record_shortcut = crate::prefs::shortcut_of(cx, nexus_desktop_core::prefs::Slot::Record)
            .1
            .active;
        crate::tray::sync(
            cx,
            MenuState::new(signed_in, &self.sessions)
                .with_shortcut(shortcut)
                .with_record(crate::record::available(), record_shortcut),
        );
    }
}
