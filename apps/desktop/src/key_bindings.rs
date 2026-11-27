//! The keys a person sets in Settings › Keyboard shortcuts, as the Nexus app's. Both apps keep them in
//! `settings.json`'s `shortcuts`, and the rules (the store's form, the recorder, the clash check) are
//! `nexus_desktop_core::key_bindings`.
//!
//! A person may change the Nexus app's Code shortcuts that this build has an action for ([`REBINDABLE`]). The others
//! of its table ⌘K, the jump keys) stay put there too, and this build's own keys ([`FIXED_KEYS`]) count in the clash
//! check. A changed key replaces the default in the keymap at once, in every window, and the shortcuts sheet (the
//! palette's `?`) and the tooltips show it. A cleared shortcut has no key.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Mutex;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{App, KeyBinding, Keystroke, Unbind};
use nexus_desktop_core::key_bindings::{self as rules, Bindings, KeyBinding as Key, KeyLike, Recorded};

use crate::workspace::session_keys::{
    AddProject, ArchiveSession, CopyBranch, CycleEffort, CycleMode, CycleModel, DeleteSession, FocusComposer,
    JUMP_BINDS, NextSession, OpenArtifacts, OpenInbox, PickModel, PreviousSession, RenameSession, SnoozeSession,
    StopTurn, ToggleChanges,
};
use crate::workspace::{
    FindInChat, GoBack, GoForward, NewChat, SideTool1, SideTool2, SideTool3, SideTool4, SideTool5, SideTool6,
    ToggleMaximizeWorkbench, ToggleSidePane, ToggleTerminal,
};

const MAC: bool = cfg!(target_os = "macos");

/// One of the Nexus app's Code shortcuts this build has an action for.
pub struct Rebindable {
    /// Its id in the Nexus app's table and in `settings.json`.
    pub id: &'static str,
    /// What the Settings row says (the Nexus app's label).
    pub label: &'static str,
    /// Its heading there.
    pub group: &'static str,
    /// Its action on `keys`, in a context (`Workspace`, or `Workspace > Input` to win over a text box's own key).
    bind: fn(&'static str, &'static str) -> KeyBinding,
}

impl Rebindable {
    /// The Nexus app's default on this platform (`forPlatform`): off a Mac the Workbench's tools are Ctrl+Alt+1 to 6.
    /// Back and Forward are this build's own, and have none here (`history_default`).
    pub fn default_key(&self) -> Option<Key> {
        defaults(self.id).map(|shortcut| shortcut.binding)
    }

    /// Its action's name (`workspace::NewChat`).
    fn action_name(&self) -> &'static str {
        (self.bind)("f1", "Workspace").action().name()
    }
}

/// The session keys that run while typing, over a text box's own key with the same keystroke.
const OVER_TEXT_BOXES: [&str; 16] = [
    "add-project",
    "previous-session",
    "next-session",
    "focus-composer",
    "toggle-changes",
    "model-picker",
    "open-inbox",
    "open-artifacts",
    "stop-turn",
    "cycle-model",
    "cycle-effort",
    "cycle-mode",
    "archive-session",
    "snooze-session",
    "copy-branch",
    "rename-session",
];

/// `id` in the Nexus app's table with this platform's default key.
fn defaults(id: &str) -> Option<rules::Shortcut> {
    rules::for_platform(&rules::code_shortcuts(), MAC)
        .into_iter()
        .find(|shortcut| shortcut.id == id)
}

/// The shortcuts a person can change here, in the Nexus app's order and groups.
pub const REBINDABLE: [Rebindable; 30] = [
    Rebindable {
        id: "new-session",
        label: "New session",
        group: "Sessions",
        bind: |keys, context| KeyBinding::new(keys, NewChat, Some(context)),
    },
    Rebindable {
        id: "add-project",
        label: "Add a project (a folder)",
        group: "Sessions",
        bind: |keys, context| KeyBinding::new(keys, AddProject, Some(context)),
    },
    Rebindable {
        id: "previous-session",
        label: "Previous session",
        group: "Sessions",
        bind: |keys, context| KeyBinding::new(keys, PreviousSession, Some(context)),
    },
    Rebindable {
        id: "next-session",
        label: "Next session",
        group: "Sessions",
        bind: |keys, context| KeyBinding::new(keys, NextSession, Some(context)),
    },
    Rebindable {
        id: "focus-composer",
        label: "Add the selection to the message, or focus the message box",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, FocusComposer, Some(context)),
    },
    Rebindable {
        id: "find-in-session",
        label: "Find in the session",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, FindInChat, Some(context)),
    },
    Rebindable {
        id: "toggle-changes",
        label: "Show or hide Changes",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, ToggleChanges, Some(context)),
    },
    Rebindable {
        id: "model-picker",
        label: "Pick the model",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, PickModel, Some(context)),
    },
    Rebindable {
        id: "open-inbox",
        label: "Open the Inbox",
        group: "Sessions",
        bind: |keys, context| KeyBinding::new(keys, OpenInbox, Some(context)),
    },
    Rebindable {
        id: "open-artifacts",
        label: "Open Artifacts",
        group: "Sessions",
        bind: |keys, context| KeyBinding::new(keys, OpenArtifacts, Some(context)),
    },
    Rebindable {
        id: "toggle-side-pane",
        label: "Show or hide the workbench",
        group: "Workbench",
        bind: |keys, context| KeyBinding::new(keys, ToggleSidePane, Some(context)),
    },
    Rebindable {
        id: "expand-workbench",
        label: "Expand or collapse the Workbench",
        group: "Workbench",
        bind: |keys, context| KeyBinding::new(keys, ToggleMaximizeWorkbench, Some(context)),
    },
    Rebindable {
        id: "tool-changes",
        label: "Changes",
        group: "Workbench",
        bind: |keys, context| KeyBinding::new(keys, SideTool1, Some(context)),
    },
    Rebindable {
        id: "tool-files",
        label: "Files",
        group: "Workbench",
        bind: |keys, context| KeyBinding::new(keys, SideTool2, Some(context)),
    },
    Rebindable {
        id: "open-browser",
        label: "Browser",
        group: "Workbench",
        bind: |keys, context| KeyBinding::new(keys, SideTool3, Some(context)),
    },
    Rebindable {
        id: "open-terminal",
        label: "Terminal",
        group: "Workbench",
        bind: |keys, context| KeyBinding::new(keys, SideTool4, Some(context)),
    },
    Rebindable {
        id: "tool-artifacts",
        label: "Artifacts",
        group: "Workbench",
        bind: |keys, context| KeyBinding::new(keys, SideTool5, Some(context)),
    },
    Rebindable {
        id: "tool-agents",
        label: "Background tasks",
        group: "Workbench",
        bind: |keys, context| KeyBinding::new(keys, SideTool6, Some(context)),
    },
    Rebindable {
        id: "toggle-terminal",
        label: "Show or hide the terminal",
        group: "Workbench",
        bind: |keys, context| KeyBinding::new(keys, ToggleTerminal, Some(context)),
    },
    Rebindable {
        id: "stop-turn",
        label: "Stop the agent's turn",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, StopTurn, Some(context)),
    },
    Rebindable {
        id: "cycle-model",
        label: "Next model",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, CycleModel, Some(context)),
    },
    Rebindable {
        id: "cycle-effort",
        label: "Next effort level",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, CycleEffort, Some(context)),
    },
    Rebindable {
        id: "cycle-mode",
        label: "Next permission mode (Full access only from its menu)",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, CycleMode, Some(context)),
    },
    Rebindable {
        id: "archive-session",
        label: "Archive or restore the session",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, ArchiveSession, Some(context)),
    },
    Rebindable {
        id: "snooze-session",
        label: "Snooze the session (a snoozed one wakes)",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, SnoozeSession, Some(context)),
    },
    Rebindable {
        id: "copy-branch",
        label: "Copy the branch name",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, CopyBranch, Some(context)),
    },
    Rebindable {
        id: "rename-session",
        label: "Rename the session (or the sidebar row with focus)",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, RenameSession, Some(context)),
    },
    Rebindable {
        id: "delete-session",
        label: "Delete the session (or the sidebar row with focus)",
        group: "Session",
        bind: |keys, context| KeyBinding::new(keys, DeleteSession, Some(context)),
    },
    Rebindable {
        id: "history-back",
        label: "Back",
        group: "Navigation",
        bind: |keys, context| KeyBinding::new(keys, GoBack, Some(context)),
    },
    Rebindable {
        id: "history-forward",
        label: "Forward",
        group: "Navigation",
        bind: |keys, context| KeyBinding::new(keys, GoForward, Some(context)),
    },
];

/// This build's keys that do not change and are not in the Nexus app's table: a key a person sets may not take them.
pub fn fixed_keys() -> Vec<(String, Key)> {
    vec![
        ("Show or hide the sidebar".to_string(), Key::mod_key("b")),
        ("Open the Assistant".to_string(), Key::mod_key("j")),
        ("Stash the draft".to_string(), Key::mod_key("s")),
        ("Show or hide the chat browser".to_string(), Key::mod_key("b").shift()),
        ("New terminal".to_string(), Key::at("`", "Backquote").shift().ctrl()),
    ]
}

pub fn rebindable(id: &str) -> Option<&'static Rebindable> {
    REBINDABLE.iter().find(|shortcut| shortcut.id == id)
}

thread_local! {
    /// The person's keys, as last read or saved. The UI thread's: the keymap, the sheet and the tooltips live there.
    static CURRENT: RefCell<Bindings> = const { RefCell::new(Bindings::new()) };
}

pub fn current() -> Bindings {
    CURRENT.with(|keys| keys.borrow().clone())
}

fn set_current(bindings: Bindings) {
    CURRENT.with(|keys| *keys.borrow_mut() = bindings);
}

/// At launch, before the keymap is made: the keys saved in `settings.json`.
pub fn load() {
    set_current(crate::desktop_settings::load().shortcuts);
}

/// `id`'s key as the person has it: their key, None when they cleared it, else this build's default.
pub fn key_of(id: &str, from: &Bindings) -> Option<Key> {
    match from.get(id) {
        Some(binding) => binding.clone(),
        None => rebindable(id).and_then(Rebindable::default_key),
    }
}

/// Whether `id` runs while typing in a text box, as the Nexus app's `global` after `forPlatform` and `withBindings`:
/// off a Mac a key with Alt does not (Ctrl+Alt is AltGr there), and Mod+Backspace deletes text in a box.
fn runs_while_typing(id: &str, key: &Key) -> bool {
    let default = defaults(id).is_none_or(|shortcut| shortcut.global);
    default && (MAC || !key.alt)
}

/// The native history key used until the person changes it.
fn history_default(id: &str) -> Option<&'static str> {
    match id {
        "history-back" => Some(crate::GO_BACK),
        "history-forward" => Some(crate::GO_FORWARD),
        _ => None,
    }
}

/// `id`'s keystroke for GPUI and its labels, or None when the person cleared it.
pub fn keystroke(id: &str) -> Option<String> {
    if !current().contains_key(id)
        && let Some(stroke) = history_default(id)
    {
        return Some(stroke.into());
    }
    key_of(id, &current()).and_then(|key| keystrokes(&key, MAC).into_iter().next())
}

/// Whether `stroke` is `id`'s key as the person has it.
pub fn pressed(id: &str, stroke: &Keystroke) -> bool {
    key_of(id, &current()).is_some_and(|key| {
        keystrokes(&key, MAC).into_iter().any(|text| {
            Keystroke::parse(intern(text))
                .is_ok_and(|bound| bound.key == stroke.key && bound.modifiers == stroke.modifiers)
        })
    })
}

/// How `id`'s key reads ("Ctrl+Shift+Enter", "⌃⇧↩"); empty when it has none.
pub fn label(id: &str) -> String {
    keystroke(id)
        .map(|keys| crate::shortcut_label::label(&keys))
        .unwrap_or_default()
}

/// A tooltip with `id`'s key after it, when it has one: "Hide the Workbench (Ctrl+Alt+\)".
pub fn with_hint(text: &str, id: &str) -> String {
    match keystroke(id) {
        Some(keys) => crate::shortcut_label::with_hint(text, &keys),
        None => text.to_string(),
    }
}

/// The current shortcut as keycaps, omitted when the person cleared it.
pub fn tooltip(text: &str, id: &str) -> Tooltip {
    match keystroke(id) {
        Some(keys) => crate::shortcut_label::tooltip(text, &keys),
        None => Tooltip::new(text.to_string()),
    }
}

/// The key's name in a GPUI keystroke: `n`, `[`, `enter`, `f5`, `space`.
fn gpui_key(key: &str) -> Option<String> {
    let named = match key {
        " " => "space",
        "Backspace" => "backspace",
        "Delete" => "delete",
        "Enter" => "enter",
        "Escape" => "escape",
        "Tab" => "tab",
        "ArrowUp" => "up",
        "ArrowDown" => "down",
        "ArrowLeft" => "left",
        "ArrowRight" => "right",
        "Home" => "home",
        "End" => "end",
        "PageUp" => "pageup",
        "PageDown" => "pagedown",
        key if rules::is_function_key(key) => return Some(key.to_lowercase()),
        key if key.chars().count() == 1 => return Some(key.to_lowercase()),
        _ => return None,
    };
    Some(named.into())
}

/// What a US keyboard types at a sign's place with Shift.
fn shifted(sign: &str) -> Option<&'static str> {
    Some(match sign {
        "`" => "~",
        "1" => "!",
        "2" => "@",
        "3" => "#",
        "4" => "$",
        "5" => "%",
        "6" => "^",
        "7" => "&",
        "8" => "*",
        "9" => "(",
        "0" => ")",
        "-" => "_",
        "=" => "+",
        "[" => "{",
        "]" => "}",
        "\\" => "|",
        ";" => ":",
        "'" => "\"",
        "," => "<",
        "." => ">",
        "/" => "?",
        _ => return None,
    })
}

/// The GPUI keystrokes a key is on a platform, the first the one labels show. A sign with Shift is bound as its
/// shifted character too, since some platforms report that (as `main.rs` binds Ctrl+~ beside Ctrl+Shift+`). A key
/// that may have Shift or not (`anyShift`) is bound both ways.
pub fn keystrokes(key: &Key, mac: bool) -> Vec<String> {
    let Some(name) = gpui_key(&key.key) else {
        return Vec::new();
    };
    if key.bare {
        return vec![name];
    }
    let modifier = if mac && !key.ctrl { "cmd" } else { "ctrl" };
    let alt = if key.alt { "alt-" } else { "" };
    let mut out = Vec::new();
    let mut add = |shift: bool, name: &str| {
        let stroke = format!("{modifier}-{alt}{}{name}", if shift { "shift-" } else { "" });
        if !out.contains(&stroke) {
            out.push(stroke);
        }
    };
    if key.shift || key.any_shift {
        add(true, &name);
        if let Some(sign) = shifted(&name) {
            add(false, sign);
            add(true, sign);
        }
    }
    if !key.shift || key.any_shift {
        add(false, &name);
    }
    out
}

/// Keystroke text lives as long as the keymap: each different one is kept once.
fn intern(text: String) -> &'static str {
    static SEEN: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let seen = seen.get_or_insert_with(HashSet::new);
    if let Some(known) = seen.get(text.as_str()) {
        return known;
    }
    let leaked: &'static str = Box::leak(text.into_boxed_str());
    seen.insert(leaked);
    leaked
}

/// The rebindable shortcuts' key bindings with the person's keys, on the `Workspace` context. Off a Mac, a key the
/// person set with Alt leaves a text box alone (Ctrl+Alt is AltGr there), as the Nexus app's `withBindings`.
pub fn bindings(from: &Bindings) -> Vec<KeyBinding> {
    let mut out = Vec::new();
    for shortcut in &REBINDABLE {
        if !from.contains_key(shortcut.id)
            && let Some(stroke) = history_default(shortcut.id)
        {
            out.push((shortcut.bind)(stroke, "Workspace"));
            if MAC {
                if shortcut.id == "history-back" {
                    out.push(KeyBinding::new(stroke, GoBack, Some("Workspace > Input")));
                } else {
                    out.push(KeyBinding::new(stroke, GoForward, Some("Workspace > Input")));
                }
            }
            continue;
        }
        let Some(key) = key_of(shortcut.id, from) else {
            continue;
        };
        let typing_alone = !runs_while_typing(shortcut.id, &key);
        for stroke in keystrokes(&key, MAC) {
            let stroke = intern(stroke);
            out.push((shortcut.bind)(stroke, "Workspace"));
            // Embedded inputs handle their own keys after keymap dispatch, including copy and word deletion.
            if OVER_TEXT_BOXES.contains(&shortcut.id) || shortcut.id == "delete-session" {
                for context in ["Workspace > Terminal", "Workspace > WebView"] {
                    out.push(KeyBinding::new(
                        stroke,
                        Unbind(shortcut.action_name().into()),
                        Some(context),
                    ));
                }
            }
            // A text box has some of these keys too (Ctrl+. is its code actions): Code's key wins there, as the Nexus
            // app's listener does while typing.
            if !typing_alone && OVER_TEXT_BOXES.contains(&shortcut.id) {
                out.push((shortcut.bind)(stroke, "Workspace > Input"));
            }
            if typing_alone {
                let name = shortcut.action_name();
                out.push(KeyBinding::new(stroke, Unbind(name.into()), Some("Workspace > Input")));
            }
        }
    }
    // Mod+1 to Mod+9 jump to the sessions in the sidebar's order. They do not change (`FIXED`).
    for (n, bind) in JUMP_BINDS.iter().enumerate() {
        let key = Key::at(&(n + 1).to_string(), &format!("Digit{}", n + 1));
        for stroke in keystrokes(&key, MAC) {
            let stroke = intern(stroke);
            let binding = bind(stroke);
            for context in ["Workspace > Terminal", "Workspace > WebView"] {
                out.push(KeyBinding::new(
                    stroke,
                    Unbind(binding.action().name().into()),
                    Some(context),
                ));
            }
            out.push(binding);
        }
    }
    out
}

/// Whether a binding in the keymap is one [`bindings`] makes, so a change replaces it.
fn is_ours(binding: &KeyBinding, names: &[&'static str]) -> bool {
    let action = binding.action();
    match action.as_any().downcast_ref::<Unbind>() {
        Some(unbind) => names.contains(&unbind.0.as_ref()),
        None => names.contains(&action.name()),
    }
}

/// Use `from` as the person's keys at once: every window's keymap has them, and the sheet and tooltips show them.
pub fn apply(cx: &mut App, from: Bindings) {
    let names: Vec<&'static str> = REBINDABLE
        .iter()
        .map(Rebindable::action_name)
        .chain(JUMP_BINDS.iter().map(|bind| bind("f1").action().name()))
        .collect();
    let kept: Vec<KeyBinding> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .filter(|binding| !is_ours(binding, &names))
        .cloned()
        .collect();
    let ours = bindings(&from);
    set_current(from);
    cx.clear_key_bindings();
    cx.bind_keys(kept.into_iter().chain(ours));
}

/// Save `from` in `settings.json` and use it. Refused while the Nexus app runs (`desktop_settings::READ_ONLY`).
pub fn save(cx: &mut App, from: Bindings) -> Result<(), String> {
    let saved = from.clone();
    crate::desktop_settings::update(move |settings| {
        settings.shortcuts = saved;
        Ok(())
    })?;
    apply(cx, from);
    Ok(())
}

/// What the Settings recorder does with a press for `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Only a modifier is down: wait.
    Wait,
    /// Esc alone: nothing changes.
    Cancel,
    /// Refused, and why.
    Refused(String),
    /// The new keys to save.
    Set(Bindings),
}

/// A GPUI keystroke as the shared recorder reads a key event: the key's name and its place on a US keyboard.
pub fn key_like(stroke: &Keystroke) -> KeyLike {
    let modifiers = &stroke.modifiers;
    let (key, code): (String, Option<String>) = match stroke.key.as_str() {
        "shift" => ("Shift".into(), None),
        "control" => ("Control".into(), None),
        "alt" => ("Alt".into(), None),
        "platform" => ("Meta".into(), None),
        "function" => ("Fn".into(), None),
        "capslock" => ("CapsLock".into(), None),
        "space" => (" ".into(), Some("Space".into())),
        name if name.len() > 1 => {
            let web = match name {
                "enter" => "Enter",
                "escape" => "Escape",
                "backspace" => "Backspace",
                "delete" => "Delete",
                "tab" => "Tab",
                "up" => "ArrowUp",
                "down" => "ArrowDown",
                "left" => "ArrowLeft",
                "right" => "ArrowRight",
                "home" => "Home",
                "end" => "End",
                "pageup" => "PageUp",
                "pagedown" => "PageDown",
                other => {
                    let upper = other.to_uppercase();
                    if rules::is_function_key(&upper) {
                        return KeyLike {
                            key: upper.clone(),
                            code: Some(upper),
                            meta: modifiers.platform,
                            ctrl: modifiers.control,
                            shift: modifiers.shift,
                            alt: modifiers.alt,
                        };
                    }
                    "Unidentified"
                }
            };
            (web.into(), (web != "Unidentified").then(|| web.to_string()))
        }
        name => {
            let c = name.chars().next().unwrap_or_default();
            let code = if c.is_ascii_alphabetic() {
                Some(format!("Key{}", c.to_ascii_uppercase()))
            } else if c.is_ascii_digit() {
                Some(format!("Digit{c}"))
            } else {
                let unshifted = REVERSE_SHIFT
                    .iter()
                    .find(|(shifted, _)| *shifted == name)
                    .map_or(name, |(_, sign)| sign);
                rules::SIGNS
                    .iter()
                    .chain(DIGITS.iter())
                    .find(|(_, sign)| *sign == unshifted)
                    .map(|(place, _)| place.to_string())
            };
            // With Alt, what it types (`∆` for ⌥J on a Mac) is the recorder's reason to go by place.
            let typed = match (&stroke.key_char, modifiers.alt) {
                (Some(typed), true) if !typed.is_empty() => typed.clone(),
                _ => name.to_string(),
            };
            (typed, code)
        }
    };
    KeyLike {
        key,
        code,
        meta: modifiers.platform,
        ctrl: modifiers.control,
        shift: modifiers.shift,
        alt: modifiers.alt,
    }
}

const DIGITS: [(&str, &str); 10] = [
    ("Digit0", "0"),
    ("Digit1", "1"),
    ("Digit2", "2"),
    ("Digit3", "3"),
    ("Digit4", "4"),
    ("Digit5", "5"),
    ("Digit6", "6"),
    ("Digit7", "7"),
    ("Digit8", "8"),
    ("Digit9", "9"),
];

/// A US keyboard's shifted characters and the key they are on.
const REVERSE_SHIFT: [(&str, &str); 21] = [
    ("~", "`"),
    ("!", "1"),
    ("@", "2"),
    ("#", "3"),
    ("$", "4"),
    ("%", "5"),
    ("^", "6"),
    ("&", "7"),
    ("*", "8"),
    ("(", "9"),
    (")", "0"),
    ("_", "-"),
    ("+", "="),
    ("{", "["),
    ("}", "]"),
    ("|", "\\"),
    (":", ";"),
    ("\"", "'"),
    ("<", ","),
    (">", "."),
    ("?", "/"),
];

/// A press in the recorder for `id`, as the Nexus app's page handles it: Esc alone cancels; a key the system or
/// another shortcut has is refused with what has it; the default pressed again resets, so a later default still
/// reaches the person.
pub fn record(id: &str, stroke: &Keystroke, from: &Bindings, mac: bool) -> Outcome {
    if stroke.key == "escape" && !stroke.modifiers.modified() {
        return Outcome::Cancel;
    }
    let binding = match rules::record_binding(&key_like(stroke), mac) {
        None => return Outcome::Wait,
        Some(Recorded::Problem(why)) => return Outcome::Refused(why.into()),
        Some(Recorded::Binding(binding)) => binding,
    };
    if keystrokes(&binding, mac).is_empty() {
        return Outcome::Refused("Use a letter, a digit, a sign, an arrow or F1 to F12.".into());
    }
    let mut more = fixed_keys();
    for shortcut in REBINDABLE.iter().filter(|shortcut| shortcut.id != id) {
        if let Some(key) = key_of(shortcut.id, from) {
            more.push((shortcut.label.to_string(), key));
        } else if !from.contains_key(shortcut.id) && mac {
            match shortcut.id {
                "history-back" => more.push(("Back".into(), Key::at("[", "BracketLeft"))),
                "history-forward" => more.push(("Forward".into(), Key::at("]", "BracketRight"))),
                _ => {}
            }
        }
    }
    if let Some(why) = rules::taken_by(&binding, Some(id), from, &more) {
        return Outcome::Refused(why);
    }
    let mut next = from.clone();
    let default = rebindable(id).and_then(Rebindable::default_key);
    if default.is_some_and(|default| rules::same_binding(&binding, &default))
        || history_default(id).is_some_and(|stroke| keystrokes(&binding, mac).iter().any(|key| key == stroke))
    {
        next.remove(id);
    } else {
        next.insert(id.to_string(), Some(binding));
    }
    Outcome::Set(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{KeyContext, Keymap};

    fn stroke(text: &str) -> Keystroke {
        Keystroke::parse(intern(text.to_string())).unwrap_or_else(|_| panic!("{text} does not parse"))
    }

    /// What `keys` runs in the workspace (and, with `typing`, with a text field focused).
    fn runs(from: &Bindings, keys: &str, typing: bool) -> Option<&'static str> {
        let keymap = Keymap::new(bindings(from));
        let mut stack = vec![KeyContext::parse("Workspace").unwrap()];
        if typing {
            stack.push(KeyContext::parse("Input").unwrap());
        }
        let (found, _) = keymap.bindings_for_input(&[stroke(keys)], &stack);
        found.first().map(|binding| binding.action().name())
    }

    const MOD: &str = if MAC { "cmd" } else { "ctrl" };

    #[test]
    fn the_defaults_are_the_keys_main_rs_bound() {
        let none = Bindings::new();
        assert_eq!(runs(&none, &format!("{MOD}-n"), false), Some("workspace::NewChat"));
        assert_eq!(runs(&none, &format!("{MOD}-f"), true), Some("workspace::FindInChat"));
        assert_eq!(
            runs(&none, crate::shortcut_label::TOGGLE_SIDE_PANE, false),
            Some("workspace::ToggleSidePane")
        );
        assert_eq!(
            runs(&none, crate::shortcut_label::MAXIMIZE_WORKBENCH, false),
            Some("workspace::ToggleMaximizeWorkbench")
        );
        // Changes, Files, Browser, Terminal, Artifacts, Agents: the Nexus app's order (`TOOLS`).
        let tools = [
            "SideTool1",
            "SideTool2",
            "SideTool3",
            "SideTool4",
            "SideTool5",
            "SideTool6",
        ];
        for (keys, tool) in crate::shortcut_label::TOOL_KEYS.iter().zip(tools) {
            assert_eq!(runs(&none, keys, false), Some(intern(format!("workspace::{tool}"))));
        }
        assert_eq!(
            rebindable("open-browser").map(Rebindable::action_name),
            Some("workspace::SideTool3")
        );
        assert_eq!(
            rebindable("open-terminal").map(Rebindable::action_name),
            Some("workspace::SideTool4")
        );
        assert_eq!(runs(&none, "ctrl-`", false), Some("workspace::ToggleTerminal"));
    }

    /// The Nexus app's Code session keys (`SHORTCUTS` in `shortcuts.ts`), with their defaults on this platform.
    #[test]
    fn the_session_keys_are_the_nexus_apps() {
        let none = Bindings::new();
        let expect = [
            (format!("{MOD}-o"), "AddProject"),
            (format!("{MOD}-shift-["), "PreviousSession"),
            (format!("{MOD}-shift-]"), "NextSession"),
            (format!("{MOD}-l"), "FocusComposer"),
            (format!("{MOD}-shift-d"), "ToggleChanges"),
            (format!("{MOD}-/"), "PickModel"),
            (format!("{MOD}-shift-i"), "OpenInbox"),
            (format!("{MOD}-shift-a"), "OpenArtifacts"),
            (format!("{MOD}-."), "StopTurn"),
            (format!("{MOD}-shift-m"), "CycleModel"),
            (format!("{MOD}-shift-e"), "CycleEffort"),
            (format!("{MOD}-alt-shift-a"), "CycleMode"),
            (format!("{MOD}-shift-s"), "ArchiveSession"),
            (format!("{MOD}-shift-h"), "SnoozeSession"),
            (format!("{MOD}-shift-c"), "CopyBranch"),
            ("f2".to_string(), "RenameSession"),
            (format!("{MOD}-backspace"), "DeleteSession"),
        ];
        for (keys, action) in expect {
            let name = format!("workspace::{action}");
            assert_eq!(runs(&none, &keys, false), Some(intern(name.clone())), "{keys}");
        }
        for n in 1..=9 {
            let name = format!("workspace::JumpSession{n}");
            assert_eq!(runs(&none, &format!("{MOD}-{n}"), false), Some(intern(name.clone())));
            // They work from the message box too.
            assert_eq!(runs(&none, &format!("{MOD}-{n}"), true), Some(intern(name.clone())));
        }
        // Mod+Backspace deletes text in a text box (the empty message box is `workspace/mod.rs`'s).
        assert_eq!(runs(&none, &format!("{MOD}-backspace"), true), None);
        assert_eq!(runs(&none, "f2", true), Some("workspace::RenameSession"));
    }

    #[test]
    fn session_keys_leave_embedded_input_keys_alone() {
        let mut from = Bindings::new();
        // Cover a person's remapping as well as every session default.
        from.insert("copy-branch".into(), Some(Key::mod_key("q")));
        for from in [Bindings::new(), from] {
            let keymap = Keymap::new(bindings(&from));
            for context in ["Terminal", "WebView"] {
                let stack = [
                    KeyContext::parse("Workspace").unwrap(),
                    KeyContext::parse(context).unwrap(),
                ];
                for shortcut in REBINDABLE
                    .iter()
                    .filter(|shortcut| OVER_TEXT_BOXES.contains(&shortcut.id) || shortcut.id == "delete-session")
                {
                    for keys in keystrokes(&key_of(shortcut.id, &from).unwrap(), MAC) {
                        let (found, _) = keymap.bindings_for_input(&[stroke(&keys)], &stack);
                        assert!(found.is_empty(), "{} captured {keys} in {context}", shortcut.id);
                    }
                }
                for n in 1..=9 {
                    let keys = format!("{MOD}-{n}");
                    let (found, _) = keymap.bindings_for_input(&[stroke(&keys)], &stack);
                    assert!(found.is_empty(), "session jump captured {keys} in {context}");
                }
            }
        }
    }

    #[test]
    fn every_session_key_is_listed_with_the_nexus_apps_label() {
        let table = rules::code_shortcuts();
        for id in [
            "add-project",
            "previous-session",
            "next-session",
            "focus-composer",
            "toggle-changes",
            "model-picker",
            "open-inbox",
            "open-artifacts",
            "stop-turn",
            "cycle-model",
            "cycle-effort",
            "cycle-mode",
            "archive-session",
            "snooze-session",
            "copy-branch",
            "rename-session",
            "delete-session",
        ] {
            let ours = rebindable(id).expect(id);
            let theirs = table.iter().find(|s| s.id == id).expect(id);
            assert_eq!((ours.label, ours.group), (theirs.label.as_str(), theirs.group));
        }
    }

    /// A text box's own Mod+. (its code actions) bound before ours, as GPUI Kit's are: Code's stop wins while typing.
    #[test]
    fn a_session_key_wins_over_a_text_boxs_own_key() {
        let stop = format!("{MOD}-.");
        let text_box = KeyBinding::new(intern(stop.clone()), GoBack, Some("Input"));
        let keymap = Keymap::new(std::iter::once(text_box).chain(bindings(&Bindings::new())).collect());
        let stack = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Input").unwrap(),
        ];
        let (found, _) = keymap.bindings_for_input(&[stroke(&stop)], &stack);
        assert_eq!(
            found.first().map(|binding| binding.action().name()),
            Some("workspace::StopTurn")
        );
    }

    #[test]
    fn a_pressed_key_is_matched_against_the_persons_key() {
        assert!(pressed("delete-session", &stroke(&format!("{MOD}-backspace"))));
        assert!(!pressed("delete-session", &stroke("backspace")));
    }

    #[test]
    fn history_defaults_can_be_changed_cleared_and_reset() {
        let defaults = Bindings::new();
        assert_eq!(runs(&defaults, crate::GO_BACK, true), Some("workspace::GoBack"));
        let mut changed = Bindings::new();
        changed.insert("history-back".into(), Some(Key::at("F8", "F8").bare()));
        assert_eq!(runs(&changed, "f8", true), Some("workspace::GoBack"));
        assert_eq!(runs(&changed, crate::GO_BACK, true), None);
        changed.insert("history-back".into(), None);
        assert_eq!(runs(&changed, "f8", true), None);
        assert_eq!(runs(&changed, crate::GO_BACK, true), None);
        changed.remove("history-back");
        assert_eq!(runs(&changed, crate::GO_BACK, true), Some("workspace::GoBack"));
    }

    #[test]
    fn the_recorder_cannot_take_native_alt_history_keys_off_a_mac() {
        for keys in ["alt-left", "alt-right"] {
            assert!(matches!(
                record("new-session", &stroke(keys), &Bindings::new(), false),
                Outcome::Refused(_)
            ));
        }
    }

    #[test]
    fn a_changed_key_moves_the_shortcut_and_frees_the_old_one() {
        let mut from = Bindings::new();
        from.insert("new-session".into(), Some(Key::mod_key("o").shift()));
        assert_eq!(runs(&from, &format!("{MOD}-shift-o"), true), Some("workspace::NewChat"));
        assert_eq!(runs(&from, &format!("{MOD}-n"), false), None);
    }

    #[test]
    fn a_cleared_shortcut_has_no_key() {
        let mut from = Bindings::new();
        from.insert("toggle-terminal".into(), None);
        assert_eq!(runs(&from, "ctrl-`", false), None);
        assert_eq!(key_of("toggle-terminal", &from), None);
    }

    #[test]
    fn a_sign_with_shift_is_bound_as_its_shifted_character_too() {
        assert_eq!(
            keystrokes(&Key::at("`", "Backquote").shift().ctrl(), false),
            ["ctrl-shift-`", "ctrl-~", "ctrl-shift-~"]
        );
        assert_eq!(keystrokes(&Key::at("F9", "F9").bare(), true), ["f9"]);
        assert_eq!(keystrokes(&Key::at("\\", "Backslash").alt(), true), ["cmd-alt-\\"]);
        assert_eq!(keystrokes(&Key::at("ArrowUp", "ArrowUp"), false), ["ctrl-up"]);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_key_set_with_alt_leaves_a_text_box_alone_off_a_mac() {
        let mut from = Bindings::new();
        from.insert("find-in-session".into(), Some(Key::at("q", "KeyQ").alt()));
        assert_eq!(runs(&from, "ctrl-alt-q", false), Some("workspace::FindInChat"));
        assert_eq!(runs(&from, "ctrl-alt-q", true), None);
        // The defaults with Alt too, as the Nexus app's `forPlatform`: Ctrl+Alt+\ and the tool keys.
        assert_eq!(runs(&Bindings::new(), "ctrl-alt-\\", true), None);
        assert_eq!(
            runs(&Bindings::new(), "ctrl-alt-\\", false),
            Some("workspace::ToggleSidePane")
        );
        assert_eq!(runs(&Bindings::new(), "ctrl-alt-3", true), None);
    }

    #[test]
    fn the_recorder_sets_refuses_cancels_and_resets() {
        let none = Bindings::new();
        let mac = MAC;
        assert_eq!(record("new-session", &stroke("escape"), &none, mac), Outcome::Cancel);
        assert_eq!(record("new-session", &stroke("shift"), &none, mac), Outcome::Wait);
        // A free key.
        let Outcome::Set(next) = record("new-session", &stroke(&format!("{MOD}-alt-j")), &none, mac) else {
            panic!("not set");
        };
        assert_eq!(next.get("new-session"), Some(&Some(Key::at("j", "KeyJ").alt())));
        // The system's, the Nexus app's and this build's own keys.
        assert_eq!(
            record("new-session", &stroke(&format!("{MOD}-q")), &none, mac),
            Outcome::Refused("The system keeps that key.".into())
        );
        assert_eq!(
            record("new-session", &stroke(&format!("{MOD}-shift-c")), &none, mac),
            Outcome::Refused("“Copy the branch name” has that key.".into())
        );
        assert_eq!(
            record("new-session", &stroke(&format!("{MOD}-b")), &none, mac),
            Outcome::Refused("“Show or hide the sidebar” has that key.".into())
        );
        assert_eq!(
            record("new-session", &stroke("ctrl-2"), &none, mac),
            Outcome::Refused(
                if mac {
                    "“Files” has that key."
                } else {
                    "“Jump to a session” has that key."
                }
                .into()
            )
        );
        // Not a key.
        assert!(matches!(
            record("new-session", &stroke("j"), &none, mac),
            Outcome::Refused(_)
        ));
        // The default pressed again is no change.
        let mut moved = Bindings::new();
        moved.insert("new-session".into(), Some(Key::mod_key("o").shift()));
        assert_eq!(
            record("new-session", &stroke(&format!("{MOD}-n")), &moved, mac),
            Outcome::Set(Bindings::new())
        );
    }

    #[test]
    fn a_function_key_alone_is_recorded() {
        let Outcome::Set(next) = record("toggle-terminal", &stroke("f7"), &Bindings::new(), MAC) else {
            panic!("not set");
        };
        assert_eq!(next.get("toggle-terminal"), Some(&Some(Key::at("F7", "F7").bare())));
    }

    #[test]
    fn a_shifted_sign_is_read_at_its_place() {
        let like = key_like(&stroke("ctrl-shift-{"));
        assert_eq!(like.code.as_deref(), Some("BracketLeft"));
        let like = key_like(&stroke("ctrl-%"));
        assert_eq!(like.code.as_deref(), Some("Digit5"));
    }

    #[test]
    fn shared_ids_match_the_nexus_app_and_native_history_keeps_its_own_ids() {
        let table = rules::code_shortcuts();
        for shortcut in &REBINDABLE {
            if matches!(shortcut.id, "history-back" | "history-forward") {
                assert_eq!(shortcut.group, "Navigation");
                assert!(history_default(shortcut.id).is_some());
                continue;
            }
            let theirs = table.iter().find(|s| s.id == shortcut.id).expect(shortcut.id);
            assert!(rules::editable(theirs.id));
            match shortcut.id {
                "expand-workbench" => {
                    // GPUI toggles the pane. The shared catalog also describes showing the conversation.
                    assert_eq!(shortcut.label, "Expand or collapse the Workbench");
                    assert_eq!(theirs.label, "Expand the workbench, or show the conversation again");
                }
                "tool-agents" => {
                    assert_eq!(shortcut.label, "Background tasks");
                    assert_eq!(theirs.label, "Agents");
                }
                _ => assert_eq!(theirs.label, shortcut.label),
            }
            assert_eq!(theirs.group, shortcut.group);
        }
    }

    #[gpui_kit::test]
    fn a_change_replaces_the_default_in_the_apps_keymap(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys(crate::key_bindings());
            let mut from = Bindings::new();
            from.insert("new-session".into(), Some(Key::mod_key("o").shift()));
            from.insert("toggle-terminal".into(), None);
            apply(cx, from);
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let stack = [KeyContext::parse("Workspace").unwrap()];
            let action = |keys: &str| {
                keymap
                    .bindings_for_input(&[stroke(keys)], &stack)
                    .0
                    .first()
                    .map(|binding| binding.action().name())
            };
            assert_eq!(action(&format!("{MOD}-shift-o")), Some("workspace::NewChat"));
            assert_eq!(action(&format!("{MOD}-n")), None);
            assert_eq!(action("ctrl-`"), None);
            // The keys that do not change stay.
            assert_eq!(action(&format!("{MOD}-b")), Some("workspace::ToggleSidebar"));
            assert_eq!(action(&format!("{MOD}-k")), Some("workspace::FindChat"));
            assert_eq!(action(&format!("{MOD}-s")), Some("workspace::StashPrompt"));
            assert_eq!(action(&format!("{MOD}-j")), Some("workspace::OpenAssistant"));
            assert_eq!(
                keystroke("new-session").as_deref(),
                Some(format!("{MOD}-shift-o").as_str())
            );
            drop(keymap);
            apply(cx, Bindings::new());
            assert_eq!(keystroke("new-session").as_deref(), Some(format!("{MOD}-n").as_str()));
        });
    }
}
