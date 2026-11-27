//! The person's app preferences, as far as they need no OS plugin: which global shortcuts (the panel's, and the screen
//! recording's), whether the policy allows each, and the menu-bar-only mode, read from the settings. Each shell
//! registers the shortcuts and the login item with its own toolkit. An administrator's policy (src/policy.rs) wins over
//! each.

use serde::Serialize;

use crate::policy::Policy;
use crate::settings::{DEFAULT_RECORD_SHORTCUT, DEFAULT_SHORTCUT, Settings};

/// The argument the login item starts the app with: it then opens to the menu bar only, with no window.
pub const AUTOSTART_ARG: &str = "--autostart";

/// The login item's name: the app's product name. It matches an older Nexus install's, so the two share one login
/// item.
pub const LOGIN_ITEM: &str = "Nexus";

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutStatus {
    /// The accelerator registered now.
    pub active: Option<String>,
    /// Why none is: turned off by the organization, or taken by another app.
    pub error: Option<String>,
}

/// What the panel's settings show.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    pub shortcut: String,
    pub shortcut_status: ShortcutStatus,
    /// False: the organization turned the shortcut off (`GlobalShortcutEnabled`).
    pub shortcut_allowed: bool,
    /// The screen recording's shortcut (crates/nexus-desktop-record).
    pub record_shortcut: String,
    pub record_shortcut_status: ShortcutStatus,
    /// False: not on this platform (macOS only).
    pub record_available: bool,
    /// False: the organization turned screen recording off (`ScreenCaptureEnabled`).
    pub record_allowed: bool,
    pub launch_at_login: bool,
    /// The organization decides it (`LaunchAtLogin` forced).
    pub launch_at_login_locked: bool,
    pub menu_bar_only: bool,
}

/// Which global shortcut: the one that opens the panel, or the one that starts and stops a screen recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Panel,
    Record,
}

impl Slot {
    pub fn other(self) -> Slot {
        match self {
            Slot::Panel => Slot::Record,
            Slot::Record => Slot::Panel,
        }
    }

    /// Why it may not be registered here, if it may not.
    pub fn refusal(self, policy: &Policy) -> Option<&'static str> {
        match self {
            Slot::Panel if !policy.global_shortcut_enabled => Some("Your organization turned the global shortcut off."),
            Slot::Record if !cfg!(target_os = "macos") => Some("Screen recordings for agents are on macOS only."),
            Slot::Record if !policy.screen_capture_enabled => Some("Your organization turned screen recording off."),
            _ => None,
        }
    }
}

/// The panel's shortcut: the person's, else [`DEFAULT_SHORTCUT`].
pub fn shortcut_of(settings: &Settings) -> String {
    settings
        .shortcut
        .clone()
        .filter(|shortcut| !shortcut.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SHORTCUT.to_string())
}

/// The screen recording's shortcut: the person's, else [`DEFAULT_RECORD_SHORTCUT`].
pub fn record_shortcut_of(settings: &Settings) -> String {
    settings
        .record_shortcut
        .clone()
        .filter(|shortcut| !shortcut.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_RECORD_SHORTCUT.to_string())
}

pub fn menu_bar_only(settings: &Settings) -> bool {
    settings.menu_bar_only.unwrap_or(false)
}
