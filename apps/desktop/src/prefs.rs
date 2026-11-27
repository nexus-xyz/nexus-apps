//! The person's app preferences, applied to the OS: the global shortcut and the login item. The settings, the
//! defaults, the login item's name and argument and the policy's refusals come from `nexus_desktop_core::prefs`, so
//! every app that reads the settings keeps the same choices.
//!
//! - The global shortcuts go through the `global-hotkey` crate. The panel's (Alt+Space by default) opens the panel,
//!   or closes it when it is open (`panel.rs`).
//!   The screen recording's (Alt+Shift+S, macOS only) starts and stops a recording for an agent (`record.rs`).
//!   On Linux the crate grabs the keys through X11 only. On a Wayland desktop that is XWayland, which sees the keys
//!   only while an X11 window has focus, or when the compositor passes them on.
//! - The login item goes through the `auto-launch` crate: a LaunchAgent on macOS, the Run key on Windows, an XDG
//!   autostart entry on Linux. It keeps the older Nexus installs' name ([`LOGIN_ITEM`]) and argument
//!   ([`AUTOSTART_ARG`]), so an older Nexus install and this app share one login item: the app that turned it on last
//!   is the one that opens at login.
//!
//! An administrator's policy wins over each (`Slot::refusal`, `LaunchAtLogin`). Each change registers with the OS
//! first and saves only when that worked; a failure keeps the old value and says why.

use auto_launch::{AutoLaunch, AutoLaunchBuilder};
use futures::StreamExt as _;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::{App, Global};
use nexus_desktop_core::prefs::{AUTOSTART_ARG, LOGIN_ITEM, ShortcutStatus, Slot};

/// One global shortcut, as registered with the OS now.
#[derive(Default)]
struct Registered {
    hotkey: Option<HotKey>,
    status: ShortcutStatus,
}

/// What is registered with the OS now.
struct Prefs {
    /// None when the OS refused a hotkey manager (the reason is in each slot's status).
    hotkeys: Option<GlobalHotKeyManager>,
    /// The panel's shortcut.
    panel: Registered,
    /// The screen recording's shortcut.
    record: Registered,
    /// None when this executable's path is unknown.
    launcher: Option<AutoLaunch>,
}

impl Prefs {
    fn slot(&self, slot: Slot) -> &Registered {
        match slot {
            Slot::Panel => &self.panel,
            Slot::Record => &self.record,
        }
    }
}

/// The saved shortcut of `slot`: the person's, else the default.
fn saved(slot: Slot) -> String {
    let settings = crate::desktop_settings::load();
    match slot {
        Slot::Panel => nexus_desktop_core::prefs::shortcut_of(&settings),
        Slot::Record => nexus_desktop_core::prefs::record_shortcut_of(&settings),
    }
}

impl Global for Prefs {}

/// Whether the shortcut recorder on Settings › App is waiting for keys: the workspace leaves the keys alone then.
static RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn recording() -> bool {
    RECORDING.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_recording(on: bool) {
    RECORDING.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether this launch is the login item's.
pub fn autostarted() -> bool {
    std::env::args().any(|arg| arg == AUTOSTART_ARG)
}

/// The login item for this executable, with the shared name and argument.
fn launcher() -> Result<AutoLaunch, String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    // An AppImage runs from a temporary mount: its login item starts the AppImage itself.
    let path = std::env::var("APPIMAGE")
        .ok()
        .filter(|path| cfg!(target_os = "linux") && !path.is_empty())
        .unwrap_or_else(|| exe.to_string_lossy().into_owned());
    AutoLaunchBuilder::new()
        .set_app_name(LOGIN_ITEM)
        .set_app_path(&path)
        .set_args(&[AUTOSTART_ARG])
        .build()
        .map_err(|error| error.to_string())
}

/// At launch: register the saved shortcuts, apply the organization's `LaunchAtLogin`, and run the shortcuts' presses.
pub fn start(cx: &mut App) {
    let hotkeys = match GlobalHotKeyManager::new() {
        Ok(manager) => Some(manager),
        Err(error) => {
            eprintln!("nexus-desktop-gpui: no global shortcuts: {error}");
            None
        }
    };
    let launcher = match launcher() {
        Ok(launcher) => Some(launcher),
        Err(error) => {
            eprintln!("nexus-desktop-gpui: no login item: {error}");
            None
        }
    };
    cx.set_global(Prefs {
        hotkeys,
        panel: Registered::default(),
        record: Registered::default(),
        launcher,
    });
    for slot in [Slot::Panel, Slot::Record] {
        let shortcut = saved(slot);
        match register(cx, slot, &shortcut) {
            Ok(()) => eprintln!("nexus-desktop-gpui: global shortcut {shortcut} registered"),
            // Off macOS the recording's is refused by design: nothing to say.
            Err(_) if slot == Slot::Record && !nexus_desktop_record::available(true) => {}
            Err(error) => eprintln!("nexus-desktop-gpui: {error}"),
        }
    }
    apply_managed_launch_at_login(cx);

    let (pressed, presses) = futures::channel::mpsc::unbounded();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state == HotKeyState::Pressed {
            let _ = pressed.unbounded_send(event.id);
        }
    }));
    cx.spawn(async move |cx| {
        let mut presses = presses;
        while let Some(id) = presses.next().await {
            cx.update(|cx| {
                let prefs = cx.global::<Prefs>();
                let is = |slot: Slot| prefs.slot(slot).hotkey.as_ref().map(HotKey::id) == Some(id);
                if is(Slot::Panel) {
                    // The Mac chat bar takes the panel's shortcut while it is on (`chat_bar.rs`).
                    if crate::chat_bar::enabled() {
                        crate::chat_bar::toggle(cx);
                    } else {
                        crate::panel::toggle(cx);
                    }
                } else if is(Slot::Record) {
                    crate::record::toggle(cx);
                }
            });
        }
    })
    .detach();
}

/// The panel's shortcut: the saved one, and whether it is registered or why not.
pub fn shortcut(cx: &App) -> (String, ShortcutStatus) {
    shortcut_of(cx, Slot::Panel)
}

/// `slot`'s shortcut: the saved one, and whether it is registered or why not.
pub fn shortcut_of(cx: &App, slot: Slot) -> (String, ShortcutStatus) {
    let status = cx
        .try_global::<Prefs>()
        .map(|prefs| prefs.slot(slot).status.clone())
        .unwrap_or_default();
    (saved(slot), status)
}

/// Why a global shortcut can't work on this desktop at all, if it can't: global-hotkey needs an X11 display on Linux.
fn unsupported() -> Option<&'static str> {
    let no_display = std::env::var_os("DISPLAY").is_none_or(|display| display.is_empty());
    (cfg!(target_os = "linux") && no_display).then_some("This desktop has no X11 display, which global shortcuts need.")
}

/// Register `accelerator` as `slot`'s shortcut in place of its current one. On failure the current one stays.
fn register(cx: &mut App, slot: Slot, accelerator: &str) -> Result<(), String> {
    let Prefs {
        hotkeys, panel, record, ..
    } = cx.global_mut::<Prefs>();
    let (this, other) = match slot {
        Slot::Panel => (panel, &*record),
        Slot::Record => (record, &*panel),
    };
    let refusal = slot.refusal(crate::managed::policy()).or(unsupported()).or(hotkeys
        .is_none()
        .then_some("This computer gives apps no global shortcuts."));
    if let Some(refusal) = refusal {
        if let (Some(hotkeys), Some(previous)) = (hotkeys.as_ref(), this.hotkey.take()) {
            let _ = hotkeys.unregister(previous);
        }
        this.status = ShortcutStatus {
            active: None,
            error: Some(refusal.into()),
        };
        return Err(refusal.into());
    }
    let Some(hotkeys) = hotkeys.as_ref() else {
        return Err("This computer gives apps no global shortcuts.".into());
    };
    let hotkey: HotKey = accelerator
        .parse()
        .map_err(|_| format!("{accelerator} is not a shortcut Nexus can use. Try other keys."))?;
    if this.hotkey == Some(hotkey) {
        return Ok(());
    }
    if other.hotkey == Some(hotkey) {
        return Err(format!("{accelerator} is already Nexus's other shortcut."));
    }
    let previous = this.hotkey.take();
    if let Some(previous) = previous {
        let _ = hotkeys.unregister(previous);
    }
    match hotkeys.register(hotkey) {
        Ok(()) => {
            *this = Registered {
                hotkey: Some(hotkey),
                status: ShortcutStatus {
                    active: Some(accelerator.to_string()),
                    error: None,
                },
            };
            Ok(())
        }
        Err(error) => {
            eprintln!("nexus-desktop-gpui: {accelerator} not registered: {error}");
            let message = "Another app uses that shortcut. Try other keys.".to_string();
            // Put the previous one back.
            let restored = previous.filter(|previous| hotkeys.register(*previous).is_ok());
            this.hotkey = restored;
            this.status = ShortcutStatus {
                active: restored.and(this.status.active.clone()),
                error: Some(message.clone()),
            };
            Err(message)
        }
    }
}

/// Change `slot`'s shortcut: register it, then save it. A failure keeps the old one, registered and saved.
pub fn set_shortcut(cx: &mut App, slot: Slot, accelerator: &str) -> Result<(), String> {
    let accelerator = accelerator.trim();
    if accelerator.is_empty() {
        return Err("Press a key combination with at least one modifier.".into());
    }
    if !crate::desktop_settings::writable() {
        return Err(crate::desktop_settings::READ_ONLY.into());
    }
    let previous = saved(slot);
    register(cx, slot, accelerator)?;
    let stored = crate::desktop_settings::update(|settings| {
        match slot {
            Slot::Panel => settings.shortcut = Some(accelerator.to_string()),
            Slot::Record => settings.record_shortcut = Some(accelerator.to_string()),
        }
        Ok(())
    });
    if let Err(error) = stored {
        let _ = register(cx, slot, &previous);
        return Err(error);
    }
    Ok(())
}

/// Whether the login item is there now.
pub fn launch_at_login(cx: &App) -> bool {
    cx.try_global::<Prefs>()
        .and_then(|prefs| prefs.launcher.as_ref())
        .and_then(|launcher| launcher.is_enabled().ok())
        .unwrap_or(false)
}

/// Turn the login item on or off, then save the choice. Refused when the organization decides it.
pub fn set_launch_at_login(cx: &mut App, on: bool) -> Result<(), String> {
    if crate::managed::policy().launch_at_login.is_some() {
        return Err("Your organization decides whether Nexus opens at login.".into());
    }
    if !crate::desktop_settings::writable() {
        return Err(crate::desktop_settings::READ_ONLY.into());
    }
    apply_launch_at_login(cx, on)?;
    let saved = crate::desktop_settings::update(|settings| {
        settings.launch_at_login = Some(on);
        Ok(())
    });
    if let Err(error) = saved {
        let _ = apply_launch_at_login(cx, !on);
        return Err(error);
    }
    Ok(())
}

fn apply_launch_at_login(cx: &App, on: bool) -> Result<(), String> {
    let launcher = cx
        .try_global::<Prefs>()
        .and_then(|prefs| prefs.launcher.as_ref())
        .ok_or("This build can't add itself to the login items here.")?;
    let result = if on { launcher.enable() } else { launcher.disable() };
    result.map_err(|error| {
        eprintln!("nexus-desktop-gpui: launch at login: {error}");
        "Couldn't change Open at login. Try again.".to_string()
    })
}

/// At launch: the organization's `LaunchAtLogin` when it decides; else nothing (the login item is the person's).
fn apply_managed_launch_at_login(cx: &App) {
    if let Some(on) = crate::managed::policy().launch_at_login
        && launch_at_login(cx) != on
        && let Err(error) = apply_launch_at_login(cx, on)
    {
        eprintln!("nexus-desktop-gpui: {error}");
    }
}

/// The accelerator for a key press: the modifiers, then the key. None for a press with no modifier, unless it is a
/// function key.
pub fn accelerator_of(keystroke: &gpui_kit::Keystroke) -> Option<String> {
    let modifiers = &keystroke.modifiers;
    let mut parts = Vec::new();
    if modifiers.platform {
        parts.push(if cfg!(target_os = "macos") { "Cmd" } else { "Super" });
    }
    if modifiers.control {
        parts.push("Ctrl");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    let key = keystroke.key.as_str();
    let function = key.len() > 1 && key.starts_with('f') && key[1..].chars().all(|c| c.is_ascii_digit());
    if parts.is_empty() && !function {
        return None;
    }
    let mut chars = key.chars();
    let key = chars.next()?.to_uppercase().chain(chars).collect::<String>();
    parts.push(&key);
    let accelerator = parts.join("+");
    accelerator.parse::<HotKey>().ok().map(|_| accelerator)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::Keystroke;

    fn of(keys: &'static str) -> Option<String> {
        accelerator_of(&Keystroke::parse(keys).unwrap())
    }

    #[test]
    fn a_key_press_reads_as_a_saved_accelerator() {
        assert_eq!(of("alt-space").as_deref(), Some("Alt+Space"));
        assert_eq!(of("ctrl-shift-n").as_deref(), Some("Ctrl+Shift+N"));
        assert_eq!(of("f5").as_deref(), Some("F5"));
        // A plain key is typing, not a shortcut.
        assert_eq!(of("a"), None);
        assert_eq!(of("space"), None);
    }

    #[test]
    fn the_default_shortcut_parses() {
        let default = nexus_desktop_core::prefs::shortcut_of(&Default::default());
        assert_eq!(default, "Alt+Space");
        assert!(default.parse::<HotKey>().is_ok());
    }

    #[test]
    fn the_default_record_shortcut_parses_and_is_not_the_panels() {
        let settings = Default::default();
        let record = nexus_desktop_core::prefs::record_shortcut_of(&settings);
        assert_eq!(record, "Alt+Shift+S");
        let record: HotKey = record.parse().expect("parses");
        let panel: HotKey = nexus_desktop_core::prefs::shortcut_of(&settings)
            .parse()
            .expect("parses");
        assert_ne!(record, panel);
    }

    #[test]
    fn the_login_item_starts_the_app_to_the_tray() {
        let launcher = launcher().expect("the login item");
        assert_eq!(launcher.get_app_name(), "Nexus");
        assert_eq!(launcher.get_args(), [AUTOSTART_ARG]);
    }

    /// The XDG autostart entry, written and removed for real. auto-launch writes it to `$HOME/.config/autostart` and
    /// ignores `XDG_CONFIG_HOME`, so run it only with a temporary home folder:
    /// `HOME=$(mktemp -d) cargo test -- --ignored the_login_item_is_an_autostart_entry`.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "writes an autostart entry: needs HOME in the temporary folder"]
    fn the_login_item_is_an_autostart_entry() {
        let home = std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        assert!(
            home.starts_with(std::env::temp_dir()),
            "not a temporary folder: {}",
            home.display()
        );
        let launcher = launcher().expect("the login item");
        let entry = home.join(".config/autostart/Nexus.desktop");
        launcher.enable().expect("enabled");
        let written = std::fs::read_to_string(&entry).expect("the entry");
        assert!(written.contains(AUTOSTART_ARG), "{written}");
        assert!(launcher.is_enabled().unwrap());
        launcher.disable().expect("disabled");
        assert!(!entry.exists());
    }
}
