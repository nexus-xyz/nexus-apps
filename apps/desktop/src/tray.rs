//! The tray icon (Windows, Linux) and the menu-bar item (macOS), with its menu.
//! GPUI has no tray: the `tray-icon` crate draws it.
//!
//! The menu is Ask Nexus (the panel, `panel.rs`, with the global shortcut beside it), New Chat, Record for an Agent
//! (macOS, `record.rs`, with its shortcut), the recent chats under "Recent", Open Nexus, New Window, Settings…, Send
//! Feedback…, Check for Updates… and Quit Nexus. Signed out, Sign In… takes the place of the chat items. While a screen
//! recording runs, the menu is Finish Recording and Cancel Recording, the item shows the recording's time, and a click
//! on it stops the recording. Send Feedback… opens the window's feedback dialog (`workspace/feedback_dialog.rs`).
//! Check for Updates… checks (`updates.rs`) and opens the app window's Settings › This computer at Desktop updates,
//! where the result shows. Once a verified update is ready (`updates::offered`), Restart to Update takes its place, so a
//! person with the windows closed still has the offer. A refused restart (Code sessions still running) opens the same
//! place, which says why. macOS only announces updates, so its menu keeps Check for Updates…, and Settings offers the
//! download.
//! A left click on the item opens or closes the panel. Linux trays report no clicks, so there the
//! menu's Ask Nexus opens it. Only those two open the panel: New Chat starts a chat in the app window focused last, and
//! Settings… opens its Settings › This computer (`workspace/shell.rs`).
//!
//! On Linux the item is a StatusNotifierItem through libappindicator. Its menu is a GTK 3 menu, so
//! GTK runs on a thread of its own, beside GPUI's loop, and that thread owns the item. On macOS and Windows the item is
//! made on GPUI's main thread, whose event loop serves it. A menu row's id reaches GPUI over a channel
//! ([`start`]), and [`command`] reads it.

use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui_kit::{App, Global};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// How many recent chats the menu lists.
const RECENT: usize = 5;
/// The longest title a menu row shows.
const TITLE_CHARS: usize = 48;

/// What the menu shows: whether someone is signed in, and the recent chats (id and title).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuState {
    pub signed_in: bool,
    pub recent: Vec<(String, String)>,
    /// The global shortcut while it is registered, shown beside Ask Nexus.
    pub shortcut: Option<String>,
    /// Whether this computer records for agents (`record::available`): the menu has Record for an Agent.
    pub record: bool,
    /// The recording's shortcut while it is registered, shown beside Record for an Agent.
    pub record_shortcut: Option<String>,
    /// A recording runs: the menu is Finish and Cancel. [`sync`] reads it from `record.rs`.
    pub recording: bool,
    /// A verified update is ready: Restart to Update takes Check for Updates…'s place. [`sync`] reads it from
    /// `updates.rs`.
    pub update_ready: bool,
}

impl MenuState {
    /// The state for `sessions` (any order): the most recent [`RECENT`], their titles cut to fit a menu.
    pub fn new(signed_in: bool, sessions: &[crate::model::SessionSummary]) -> MenuState {
        let mut recent: Vec<_> = sessions.iter().collect();
        recent.sort_by_key(|session| std::cmp::Reverse(session.at_ms.unwrap_or(0)));
        MenuState {
            signed_in,
            recent: if signed_in {
                recent
                    .into_iter()
                    .take(RECENT)
                    .map(|session| (session.id.clone(), short_title(&session.title)))
                    .collect()
            } else {
                Vec::new()
            },
            shortcut: None,
            record: false,
            record_shortcut: None,
            recording: false,
            update_ready: false,
        }
    }

    /// The state with the global `shortcut` beside Ask Nexus.
    pub fn with_shortcut(mut self, shortcut: Option<String>) -> MenuState {
        self.shortcut = shortcut;
        self
    }

    /// The state with Record for an Agent, when this computer records, and its `shortcut` beside it.
    pub fn with_record(mut self, available: bool, shortcut: Option<String>) -> MenuState {
        self.record = available;
        self.record_shortcut = shortcut;
        self
    }
}

/// What a menu row asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open Nexus or Sign In…: the window last used.
    Open,
    /// Ask Nexus: the panel.
    Panel,
    /// A left click on the item: the panel opens, or closes when it is open.
    TogglePanel,
    /// Another app window (`windows.rs`).
    NewWindow,
    NewChat,
    /// A recent chat, opened in the window last used.
    Session {
        id: String,
        title: String,
    },
    Settings,
    /// Send Feedback…: the feedback dialog in the window last used (`workspace/feedback_dialog.rs`).
    Feedback,
    /// Check for Updates…: a check, and the app window's Settings at Desktop updates, where its result shows.
    CheckForUpdates,
    /// Restart to Update: install the verified update and start it, as Settings › App's button.
    RestartToUpdate,
    /// Record for an Agent, or Finish Recording: start a screen recording, or stop the one in progress.
    Record,
    /// Cancel Recording: stop it and keep nothing.
    RecordCancel,
    Quit,
}

/// One row of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Item {
        id: String,
        text: String,
        enabled: bool,
        /// The key shown beside it, where the platform's menus show one.
        accelerator: Option<String>,
    },
    Separator,
}

fn item(id: &str, text: &str, accelerator: Option<&str>) -> Row {
    Row::Item {
        id: id.into(),
        text: text.into(),
        enabled: true,
        accelerator: accelerator.map(String::from),
    }
}

/// The menu's rows. Only keys this build binds are shown (`main.rs`).
fn rows(state: &MenuState) -> Vec<Row> {
    if state.recording {
        return vec![
            item("record-finish", "Finish Recording", None),
            item("record-cancel", "Cancel Recording", None),
        ];
    }
    let record = state
        .record
        .then(|| item("record", "Record for an Agent", state.record_shortcut.as_deref()));
    let mut rows = Vec::new();
    if state.signed_in {
        rows.push(item("panel", "Ask Nexus", state.shortcut.as_deref()));
        rows.push(item("new", "New Chat", Some("CmdOrCtrl+N")));
        rows.extend(record);
        if !state.recent.is_empty() {
            rows.push(Row::Separator);
            rows.push(Row::Item {
                id: "recent".into(),
                text: "Recent".into(),
                enabled: false,
                accelerator: None,
            });
            for (id, title) in &state.recent {
                rows.push(item(&format!("session:{id}"), title, None));
            }
        }
    } else {
        rows.push(item("sign-in", "Sign In…", None));
        rows.extend(record);
    }
    rows.push(Row::Separator);
    rows.push(item("open", "Open Nexus", None));
    rows.push(item("new-window", "New Window", None));
    rows.push(item("settings", "Settings…", None));
    rows.push(item("feedback", "Send Feedback…", None));
    rows.push(if state.update_ready {
        item("update-restart", "Restart to Update", None)
    } else {
        item("updates", "Check for Updates…", None)
    });
    rows.push(Row::Separator);
    rows.push(item("quit", "Quit Nexus", Some("CmdOrCtrl+Q")));
    rows
}

/// The command for the menu row `id`, on the menu `state` drew.
fn command_of(id: &str, state: &MenuState) -> Option<Command> {
    match id {
        "open" | "sign-in" => Some(Command::Open),
        "panel" => Some(Command::Panel),
        "panel-toggle" => Some(Command::TogglePanel),
        "new" => Some(Command::NewChat),
        "new-window" => Some(Command::NewWindow),
        "settings" => Some(Command::Settings),
        "feedback" => Some(Command::Feedback),
        "updates" => Some(Command::CheckForUpdates),
        "update-restart" => Some(Command::RestartToUpdate),
        "record" | "record-finish" => Some(Command::Record),
        "record-cancel" => Some(Command::RecordCancel),
        "quit" => Some(Command::Quit),
        id => {
            let id = id.strip_prefix("session:")?;
            let title = state
                .recent
                .iter()
                .find(|(each, _)| each == id)
                .map(|(_, title)| title.clone())
                .unwrap_or_default();
            Some(Command::Session { id: id.into(), title })
        }
    }
}

/// A recent chat's title, cut to fit a menu.
fn short_title(title: &str) -> String {
    if title.chars().count() > TITLE_CHARS {
        format!("{}…", title.chars().take(TITLE_CHARS - 1).collect::<String>())
    } else {
        title.to_string()
    }
}

/// The menu for `state`.
fn menu(state: &MenuState) -> Menu {
    let menu = Menu::new();
    for row in rows(state) {
        let appended = match row {
            Row::Separator => menu.append(&PredefinedMenuItem::separator()),
            Row::Item {
                id,
                text,
                enabled,
                accelerator,
            } => {
                let accelerator = accelerator.and_then(|keys| keys.parse().ok());
                menu.append(&MenuItem::with_id(id, text, enabled, accelerator))
            }
        };
        if let Err(error) = appended {
            log::warn!("tray menu: {error}");
        }
    }
    menu
}

// macOS tints the black template mark to suit the menu bar. Windows and Linux draw the app icon with its graphite tile,
// which shows on a light or a dark bar (`assets/icons`).
#[cfg(target_os = "macos")]
const ICON: &[u8] = crate::assets::held!("icons/tray.png");
#[cfg(not(target_os = "macos"))]
const ICON: &[u8] = crate::assets::held!("icons/32x32.png");

fn icon() -> Result<Icon, String> {
    let image = image::load_from_memory_with_format(ICON, image::ImageFormat::Png)
        .map_err(|error| format!("the tray icon: {error}"))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).map_err(|error| format!("the tray icon: {error}"))
}

/// Make the item with the menu for `state`.
fn build(state: &MenuState) -> Result<TrayIcon, String> {
    TrayIconBuilder::new()
        .with_id("nexus-desktop-gpui")
        .with_icon(icon()?)
        .with_icon_as_template(true)
        .with_tooltip("Nexus")
        .with_menu(Box::new(menu(state)))
        // A left click opens the panel; the menu is on the right click. Linux trays report no clicks and show the menu
        // on any click.
        .with_menu_on_left_click(false)
        .build()
        .map_err(|error| format!("the tray icon: {error}"))
}

/// The item while it is up, as a GPUI global: the state its menu shows.
pub struct Tray {
    state: MenuState,
    /// macOS and Windows: the item lives on GPUI's main thread. On Linux the GTK thread holds it.
    #[cfg(not(target_os = "linux"))]
    icon: TrayIcon,
}

impl Global for Tray {}

/// The item while it waits for the first window's first frame ([`start`]): the menu it will show.
#[cfg_attr(target_os = "linux", allow(dead_code))]
struct Held(MenuState);

impl Global for Held {}

/// Put the item up. Answers the ids of the menu rows clicked, or None when this computer shows no tray (the reason is
/// logged). The app then keeps its window-only behaviour. With `hold` (a window opens at launch), macOS and Windows put
/// the item up once that window has drawn (`launch.rs`). Linux starts its GTK thread at once, as it must know
/// whether there is a tray.
pub fn start(cx: &mut App, hold: bool) -> Option<UnboundedReceiver<String>> {
    #[cfg(not(target_os = "linux"))]
    if hold {
        cx.set_global(Held(MenuState::default()));
        crate::launch::after_first_frame(cx, |cx| {
            if !put_up(cx) && cx.windows().is_empty() {
                cx.quit();
            }
        });
        return Some(listen());
    }
    #[cfg(target_os = "linux")]
    let _ = hold;
    put_up(cx).then(listen)
}

/// Make the item, with the menu last synced, and keep it as the [`Tray`] global. False when there is none (logged).
fn put_up(cx: &mut App) -> bool {
    let state = cx.try_global::<Held>().map(|held| held.0.clone()).unwrap_or_default();
    // Linux: GTK's thread keeps the item; elsewhere the global below does.
    #[cfg(target_os = "linux")]
    let started = linux::start(state.clone());
    #[cfg(not(target_os = "linux"))]
    let started = build(&state);
    if cx.has_global::<Held>() {
        cx.remove_global::<Held>();
    }
    #[cfg_attr(target_os = "linux", allow(clippy::let_unit_value, unused_variables))]
    let icon = match started {
        Ok(icon) => icon,
        Err(error) => {
            log::warn!("no tray: {error}");
            return false;
        }
    };
    cx.set_global(Tray {
        state,
        #[cfg(not(target_os = "linux"))]
        icon,
    });
    watch_updates(cx);
    true
}

/// The ids of the menu rows clicked, and "panel-toggle" for a left click on the item.
fn listen() -> UnboundedReceiver<String> {
    let (clicked, clicks) = unbounded();
    let menu_clicked = clicked.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = menu_clicked.unbounded_send(event.id.0);
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = clicked.unbounded_send("panel-toggle".into());
        }
    }));
    clicks
}

/// Redraw the menu on each new update state, so Restart to Update shows once one is ready and goes once it is taken.
fn watch_updates(cx: &mut App) {
    let Some(mut changes) = crate::updates::subscribe() else {
        return;
    };
    cx.spawn(async move |cx| {
        while changes.changed().await.is_ok() {
            cx.update(refresh);
        }
    })
    .detach();
}

/// Where the item is, in physical pixels, for the panel to open under it. Linux trays do not say.
pub fn icon_rect(cx: &App) -> Option<(f64, f64, f64, f64)> {
    #[cfg(not(target_os = "linux"))]
    {
        let rect = cx.try_global::<Tray>()?.icon.rect()?;
        Some((
            rect.position.x,
            rect.position.y,
            f64::from(rect.size.width),
            f64::from(rect.size.height),
        ))
    }
    #[cfg(target_os = "linux")]
    {
        let _ = cx;
        None
    }
}

/// Whether the item is up, or held for the first frame: closing the main window then keeps the app running.
pub fn is_up(cx: &App) -> bool {
    cx.has_global::<Tray>() || cx.has_global::<Held>()
}

/// Show `state` in the menu, when it changed. Whether a recording runs is read here, from `record.rs`, and whether an
/// update is ready, from `updates.rs`.
pub fn sync(cx: &mut App, mut state: MenuState) {
    state.recording = crate::record::is_recording(cx);
    state.update_ready = crate::updates::offered().is_some();
    // Held for the first frame: the item is made with this menu.
    if cx.has_global::<Held>() {
        cx.global_mut::<Held>().0 = state;
        return;
    }
    let Some(tray) = cx.try_global::<Tray>() else {
        return;
    };
    if tray.state == state {
        return;
    }
    #[cfg(target_os = "linux")]
    linux::set_menu(state.clone());
    #[cfg(not(target_os = "linux"))]
    tray.icon.set_menu(Some(Box::new(menu(&state))));
    cx.global_mut::<Tray>().state = state;
}

/// The menu again, for a recording that started or stopped, or an update that is ready or taken.
pub fn refresh(cx: &mut App) {
    if let Some(state) = cx.try_global::<Tray>().map(|tray| tray.state.clone()) {
        sync(cx, state);
    }
}

/// The text beside the menu-bar item (a recording's time, then what became of it), or none. macOS shows it; Windows
/// and Linux trays have no text, and those builds do not record.
pub fn set_title(cx: &App, title: Option<&str>) {
    #[cfg(not(target_os = "linux"))]
    if let Some(tray) = cx.try_global::<Tray>() {
        tray.icon.set_title(title);
    }
    #[cfg(target_os = "linux")]
    let _ = (cx, title);
}

/// The command for the menu row `id`.
pub fn command(cx: &App, id: &str) -> Option<Command> {
    let state = cx.try_global::<Tray>().map(|tray| &tray.state)?;
    command_of(id, state)
}

/// Linux: GTK's thread, which owns the item and its menu.
#[cfg(target_os = "linux")]
mod linux {
    use std::cell::RefCell;
    use std::time::Duration;

    use super::{MenuState, TrayIcon};

    thread_local! {
        static ICON: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
    }

    /// How long the app waits for GTK to start and the item to go up.
    const START_WAIT: Duration = Duration::from_secs(5);

    /// Whether libayatana-appindicator or libappindicator is installed: libappindicator-sys panics without one, so
    /// look first, by the names it loads.
    fn appindicator_installed() -> bool {
        [c"libayatana-appindicator3.so.1", c"libappindicator3.so.1"]
            .into_iter()
            .any(|name| {
                // SAFETY: the name is a valid C string; the handle is closed at once and nothing from it is used.
                let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
                if handle.is_null() {
                    return false;
                }
                unsafe { libc::dlclose(handle) };
                true
            })
    }

    /// Whether a StatusNotifierItem host listens (KDE, or GNOME with the AppIndicator extension). Without one,
    /// libappindicator falls back to an XEmbed icon that a Wayland desktop does not show, and a closed window would
    /// leave the app running with no way back.
    fn host_listens() -> bool {
        use gtk::gio;
        use gtk::glib::{ToVariant as _, VariantTy};
        let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
            return false;
        };
        bus.call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&("org.kde.StatusNotifierWatcher",).to_variant()),
            VariantTy::new("(b)").ok(),
            gio::DBusCallFlags::NONE,
            1000,
            gio::Cancellable::NONE,
        )
        .ok()
        .and_then(|answer| answer.get::<(bool,)>())
        .is_some_and(|(owned,)| owned)
    }

    /// Start GTK's thread and put the item up on it.
    pub fn start(state: MenuState) -> Result<(), String> {
        if !appindicator_installed() {
            return Err("neither libayatana-appindicator3 nor libappindicator3 is installed".into());
        }
        if !host_listens() {
            return Err("no StatusNotifierItem host (org.kde.StatusNotifierWatcher) on the session bus".into());
        }
        let (ready, readied) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || {
                if let Err(error) = gtk::init() {
                    let _ = ready.send(Err(format!("GTK did not start: {error}")));
                    return;
                }
                match super::build(&state) {
                    Ok(icon) => ICON.with(|slot| *slot.borrow_mut() = Some(icon)),
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                }
                let _ = ready.send(Ok(()));
                gtk::main();
            })
            .map_err(|error| format!("the tray thread did not start: {error}"))?;
        readied
            .recv_timeout(START_WAIT)
            .map_err(|_| "GTK did not answer".to_string())?
    }

    /// Replace the menu, on GTK's thread.
    pub fn set_menu(state: MenuState) {
        gtk::glib::MainContext::default().invoke(move || {
            ICON.with(|slot| {
                if let Some(icon) = slot.borrow().as_ref() {
                    icon.set_menu(Some(Box::new(super::menu(&state))));
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn a_held_item_keeps_the_latest_menu_without_creating_a_native_icon(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Held(MenuState::default()));
            let state = MenuState {
                signed_in: true,
                recent: vec![("session".into(), "Latest chat".into())],
                ..Default::default()
            };
            sync(cx, state);
            assert!(is_up(cx));
            assert!(!cx.has_global::<Tray>());
            assert!(cx.global::<Held>().0.signed_in);
            assert_eq!(
                cx.global::<Held>().0.recent,
                vec![("session".into(), "Latest chat".into())]
            );
        });
    }
    use crate::model::SessionSummary;

    fn session(id: &str, title: &str, at_ms: i64) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            title: title.into(),
            day: None,
            at_ms: Some(at_ms),
        }
    }

    fn ids(state: &MenuState) -> Vec<String> {
        rows(state)
            .into_iter()
            .map(|row| match row {
                Row::Item { id, .. } => id,
                Row::Separator => "-".into(),
            })
            .collect()
    }

    #[test]
    fn signed_in_the_menu_has_new_chat_and_the_recent_chats() {
        let sessions: Vec<_> = (0..7).map(|n| session(&format!("s{n}"), "Chat", n)).collect();
        let state = MenuState::new(true, &sessions);
        assert_eq!(
            ids(&state),
            [
                "panel",
                "new",
                "-",
                "recent",
                "session:s6",
                "session:s5",
                "session:s4",
                "session:s3",
                "session:s2",
                "-",
                "open",
                "new-window",
                "settings",
                "feedback",
                "updates",
                "-",
                "quit"
            ]
        );
        assert_eq!(command_of("new-window", &state), Some(Command::NewWindow));
        assert_eq!(command_of("panel", &state), Some(Command::Panel));
        assert_eq!(command_of("feedback", &state), Some(Command::Feedback));
        assert_eq!(
            command_of("session:s5", &state),
            Some(Command::Session {
                id: "s5".into(),
                title: "Chat".into()
            })
        );
    }

    #[test]
    fn signed_out_sign_in_takes_the_chat_items_place() {
        let state = MenuState::new(false, &[session("s1", "Chat", 1)]);
        assert!(state.recent.is_empty());
        assert_eq!(
            ids(&state),
            [
                "sign-in",
                "-",
                "open",
                "new-window",
                "settings",
                "feedback",
                "updates",
                "-",
                "quit"
            ]
        );
        assert_eq!(command_of("sign-in", &state), Some(Command::Open));
        assert_eq!(command_of("quit", &state), Some(Command::Quit));
        assert_eq!(command_of("recent", &state), None);
    }

    #[test]
    fn ask_nexus_shows_the_shortcut_and_a_click_on_the_item_toggles_the_panel() {
        let state = MenuState::new(true, &[]).with_shortcut(Some("Alt+Space".into()));
        let ask = rows(&state).into_iter().find_map(|row| match row {
            Row::Item { id, accelerator, .. } if id == "panel" => Some(accelerator),
            _ => None,
        });
        assert_eq!(ask, Some(Some("Alt+Space".to_string())));
        // A left click on the item sends `panel-toggle`, signed in or not.
        assert_eq!(command_of("panel-toggle", &state), Some(Command::TogglePanel));
        assert_eq!(
            command_of("panel-toggle", &MenuState::default()),
            Some(Command::TogglePanel)
        );
    }

    #[test]
    fn record_for_an_agent_follows_new_chat_and_a_recording_shows_finish_and_cancel() {
        let state = MenuState::new(true, &[]).with_record(true, Some("Alt+Shift+S".into()));
        assert_eq!(
            ids(&state),
            [
                "panel",
                "new",
                "record",
                "-",
                "open",
                "new-window",
                "settings",
                "feedback",
                "updates",
                "-",
                "quit"
            ]
        );
        let record = rows(&state).into_iter().find_map(|row| match row {
            Row::Item { id, accelerator, .. } if id == "record" => Some(accelerator),
            _ => None,
        });
        assert_eq!(record, Some(Some("Alt+Shift+S".to_string())));
        assert_eq!(command_of("record", &state), Some(Command::Record));
        // Signed out, it follows Sign In….
        let signed_out = MenuState::new(false, &[]).with_record(true, None);
        assert_eq!(ids(&signed_out)[..2], ["sign-in", "record"]);
        // Where it does not record, there is no row.
        assert!(!ids(&MenuState::new(true, &[])).contains(&"record".to_string()));
        // While recording, the menu is the recording's.
        let recording = MenuState {
            recording: true,
            ..state.clone()
        };
        assert_eq!(ids(&recording), ["record-finish", "record-cancel"]);
        assert_eq!(command_of("record-finish", &recording), Some(Command::Record));
        assert_eq!(command_of("record-cancel", &recording), Some(Command::RecordCancel));
    }

    #[test]
    fn a_ready_update_turns_check_for_updates_into_restart_to_update() {
        let state = MenuState::new(true, &[]);
        assert_eq!(ids(&state)[ids(&state).len() - 3..], ["updates", "-", "quit"]);
        let text = |state: &MenuState, wanted: &str| {
            rows(state).into_iter().find_map(|row| match row {
                Row::Item { id, text, .. } if id == wanted => Some(text),
                _ => None,
            })
        };
        assert_eq!(text(&state, "updates").as_deref(), Some("Check for Updates…"));
        assert_eq!(command_of("updates", &state), Some(Command::CheckForUpdates));
        let ready = MenuState {
            update_ready: true,
            ..state.clone()
        };
        let ready_ids = ids(&ready);
        assert_eq!(
            ready_ids[ready_ids.len() - 4..],
            ["feedback", "update-restart", "-", "quit"]
        );
        assert!(!ready_ids.contains(&"updates".to_string()));
        assert_eq!(text(&ready, "update-restart").as_deref(), Some("Restart to Update"));
        assert_eq!(command_of("update-restart", &ready), Some(Command::RestartToUpdate));
        // The menus differ, so `sync` rebuilds it when the update state changes.
        assert_ne!(state, ready);
        // Signed out, the row is there too; while recording, the menu is the recording's only.
        assert!(ids(&MenuState::new(false, &[])).contains(&"updates".to_string()));
        let recording = MenuState {
            recording: true,
            ..ready
        };
        assert_eq!(ids(&recording), ["record-finish", "record-cancel"]);
    }

    #[test]
    fn long_titles_are_cut_for_the_menu() {
        assert_eq!(short_title("Rollout plan"), "Rollout plan");
        let cut = short_title(&"a".repeat(60));
        assert_eq!(cut.chars().count(), TITLE_CHARS);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn the_icon_decodes() {
        assert!(icon().is_ok());
    }

    #[test]
    fn the_shown_keys_parse() {
        let state = MenuState::new(true, &[])
            .with_shortcut(Some("Alt+Space".into()))
            .with_record(true, Some(nexus_desktop_core::settings::DEFAULT_RECORD_SHORTCUT.into()));
        for row in rows(&state) {
            if let Row::Item {
                accelerator: Some(keys),
                ..
            } = row
            {
                assert!(
                    keys.parse::<tray_icon::menu::accelerator::Accelerator>().is_ok(),
                    "{keys}"
                );
            }
        }
    }
}
