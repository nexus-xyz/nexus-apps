//! The keys a person sets in Settings › Keyboard shortcuts. Only the changes are kept, in `settings.json`'s
//! `shortcuts`, by shortcut id. A cleared key is stored as null. Reset drops the change, so a later default still
//! reaches the person.
//!
//! A key is in Nexus UI's matcher form (`KeyBinding` in `shortcuts.ts`), the same on every platform: Mod is ⌘ on a Mac
//! and Ctrl elsewhere, and `ctrl` is Control on both.
//!
//! This module holds the store's reading, the recorder, the clash check and the Code shortcut table.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A key, apart from what it does: what the store keeps for a shortcut.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyBinding {
    /// The key, matched by what it types unless `code` is given: `n`, `[`, `F2`, `ArrowUp`.
    pub key: String,
    /// The key's place (`KeyA`, `Digit1`, `BracketLeft`), for digits, signs and letters typed with Alt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub shift: bool,
    /// Shift may be up or down.
    #[serde(default, skip_serializing_if = "is_false")]
    pub any_shift: bool,
    /// Pressed alone, with no modifier: F1 to F12, which type nothing.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bare: bool,
    /// Control on every platform (⌃ on a Mac) instead of Mod.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ctrl: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub alt: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl KeyBinding {
    /// Mod and `key`, matched by what it types.
    pub fn mod_key(key: &str) -> Self {
        KeyBinding {
            key: key.into(),
            ..Default::default()
        }
    }

    /// Mod and the key at `code`, which types `key` on a US keyboard.
    pub fn at(key: &str, code: &str) -> Self {
        KeyBinding {
            key: key.into(),
            code: Some(code.into()),
            ..Default::default()
        }
    }

    pub fn shift(mut self) -> Self {
        self.shift = true;
        self
    }

    pub fn alt(mut self) -> Self {
        self.alt = true;
        self
    }

    pub fn ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }

    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }
}

/// Each changed shortcut's key, by its id: None when the person cleared it.
pub type Bindings = BTreeMap<String, Option<KeyBinding>>;

/// A stored key, or Err for anything that is not one (a hand edit, a form a later app wrote).
fn binding_of(value: &Value) -> Result<Option<KeyBinding>, ()> {
    if value.is_null() {
        return Ok(None);
    }
    let raw = value.as_object().ok_or(())?;
    let key = raw
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .ok_or(())?;
    let code = match raw.get("code") {
        None | Some(Value::Null) => None,
        Some(Value::String(code)) if code.is_empty() => None,
        Some(Value::String(code)) => Some(code.clone()),
        Some(_) => return Err(()),
    };
    let flag = |name: &str| match raw.get(name) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(on)) => Ok(*on),
        Some(_) => Err(()),
    };
    Ok(Some(KeyBinding {
        key: key.into(),
        code,
        shift: flag("shift")?,
        any_shift: flag("anyShift")?,
        bare: flag("bare")?,
        ctrl: flag("ctrl")?,
        alt: flag("alt")?,
    }))
}

/// The bindings in a stored value (`parseBindings`): an entry that does not read is left out, so its shortcut keeps
/// its default. Anything but an object is no change at all.
pub fn parse_bindings(value: &Value) -> Bindings {
    let Some(raw) = value.as_object() else {
        return Bindings::new();
    };
    raw.iter()
        .filter_map(|(id, value)| binding_of(value).ok().map(|binding| (id.clone(), binding)))
        .collect()
}

/// `settings.json`'s `shortcuts`, read leniently: a bad entry keeps its default, and never fails the whole file.
pub fn lenient<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Bindings, D::Error> {
    Ok(parse_bindings(&Value::deserialize(deserializer)?))
}

// ------------------------------------------------------------------ Matching

/// The part of a key press the rules read (`KeyLike`), so tests need no window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyLike {
    /// What the key types (`n`, `N`, `∆`), or its name (`F5`, `ArrowUp`, `Shift`).
    pub key: String,
    /// Its place: `KeyN`, `Digit5`, `BracketLeft`, `F5`.
    pub code: Option<String>,
    pub meta: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl KeyLike {
    pub fn new(key: &str) -> Self {
        KeyLike {
            key: key.into(),
            ..Default::default()
        }
    }
}

fn modifiers_match(press: &KeyLike, binding: &KeyBinding, mac: bool) -> bool {
    if binding.bare {
        return !press.meta && !press.ctrl && !press.alt && !press.shift;
    }
    let modifier = if mac && !binding.ctrl {
        press.meta && !press.ctrl
    } else {
        press.ctrl && !press.meta
    };
    modifier && press.alt == binding.alt && (binding.any_shift || press.shift == binding.shift)
}

fn key_matches(press: &KeyLike, binding: &KeyBinding) -> bool {
    match &binding.code {
        Some(code) => press.code.as_deref() == Some(code.as_str()),
        // Letters follow what the key types (a Dvorak user's ⌘N is the key marked N).
        None => press.key.to_lowercase() == binding.key,
    }
}

/// Whether a press is `binding` on a platform (`matchKey` for one shortcut).
pub fn matches(binding: &KeyBinding, press: &KeyLike, mac: bool) -> bool {
    modifiers_match(press, binding, mac) && key_matches(press, binding)
}

/// The press a key is on a platform (`pressOf`): to ask whether a key is already taken. A letter matched by what it
/// types also gets its place (`KeyK`), so it meets a key set by place.
pub fn press_of(binding: &KeyBinding, mac: bool) -> KeyLike {
    let letter = binding.key.len() == 1 && binding.key.chars().all(|c| c.is_ascii_lowercase());
    let code = binding
        .code
        .clone()
        .or_else(|| letter.then(|| format!("Key{}", binding.key.to_uppercase())));
    let modifier = !binding.bare;
    KeyLike {
        key: binding.key.clone(),
        code,
        meta: modifier && mac && !binding.ctrl,
        ctrl: modifier && (!mac || binding.ctrl),
        alt: modifier && binding.alt,
        shift: modifier && binding.shift,
    }
}

/// Whether two keys are the same (`sameBinding`): field by field, so a key set to its default is no change.
pub fn same_binding(a: &KeyBinding, b: &KeyBinding) -> bool {
    a == b
}

// ------------------------------------------------------------------ The recorder

/// What a key press in the Settings recorder gives (`Recorded`): a key, or why it cannot be one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorded {
    Binding(KeyBinding),
    Problem(&'static str),
}

const MODIFIERS: [&str; 10] = [
    "Meta", "Control", "Alt", "Shift", "AltGraph", "CapsLock", "Fn", "OS", "Hyper", "Super",
];

/// Signs matched by their place, named by what a US keyboard types there.
pub const SIGNS: [(&str, &str); 12] = [
    ("Backquote", "`"),
    ("Minus", "-"),
    ("Equal", "="),
    ("BracketLeft", "["),
    ("BracketRight", "]"),
    ("Backslash", "\\"),
    ("Semicolon", ";"),
    ("Quote", "'"),
    ("Comma", ","),
    ("Period", "."),
    ("Slash", "/"),
    ("Space", " "),
];

/// Keys the recorder takes by their name.
pub const NAMED: [&str; 13] = [
    "Backspace",
    "Delete",
    "Enter",
    "Escape",
    "Tab",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Home",
    "End",
    "PageUp",
    "PageDown",
];

/// F1 to F12.
pub fn is_function_key(code: &str) -> bool {
    code.strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=12).contains(&n) && !code[1..].starts_with('0'))
}

fn single_ascii(code: &str, prefix: &str, valid: impl Fn(char) -> bool) -> Option<char> {
    let rest = code.strip_prefix(prefix)?;
    let mut chars = rest.chars();
    let first = chars.next()?;
    (chars.next().is_none() && valid(first)).then_some(first)
}

/// The key a press in the recorder sets (`recordBinding`). None while only a modifier is down. It takes Mod (⌘ on a
/// Mac, Ctrl elsewhere) or, on a Mac, ⌃, with any of Alt and Shift; or F1 to F12 alone, which type nothing. A letter
/// follows what it types, as the defaults do, unless Alt changes what it types: then its place. Digits and signs go
/// by place.
pub fn record_binding(press: &KeyLike, mac: bool) -> Option<Recorded> {
    if MODIFIERS.contains(&press.key.as_str()) {
        return None;
    }
    let code = press.code.as_deref().unwrap_or("");
    let any_modifier = press.meta || press.ctrl || press.alt || press.shift;
    if is_function_key(code) && !any_modifier {
        return Some(Recorded::Binding(KeyBinding::at(code, code).bare()));
    }
    if !mac && press.meta {
        return Some(Recorded::Problem(
            "The system keeps the Windows or Super key. Use Ctrl.",
        ));
    }
    if mac && press.meta && press.ctrl {
        return Some(Recorded::Problem("Use ⌘ or ⌃, not both."));
    }
    if !press.meta && !press.ctrl {
        return Some(Recorded::Problem(if mac {
            "Start with ⌘ or ⌃, or press F1 to F12 alone."
        } else {
            "Start with Ctrl, or press F1 to F12 alone."
        }));
    }
    let typed = if press.key.chars().count() == 1 {
        press.key.to_lowercase()
    } else {
        String::new()
    };
    let (key, place) = if let Some(letter) = single_ascii(code, "Key", |c| c.is_ascii_uppercase()) {
        let typed_letter = typed.len() == 1 && typed.chars().all(|c| c.is_ascii_lowercase());
        if !press.alt && typed_letter {
            (typed, None)
        } else {
            (letter.to_ascii_lowercase().to_string(), Some(code.to_string()))
        }
    } else if let Some(digit) = single_ascii(code, "Digit", |c| c.is_ascii_digit()) {
        (digit.to_string(), Some(code.to_string()))
    } else if let Some((_, sign)) = SIGNS.iter().find(|(place, _)| *place == code) {
        (sign.to_string(), Some(code.to_string()))
    } else if is_function_key(code) || NAMED.contains(&code) {
        (code.to_string(), Some(code.to_string()))
    } else {
        return Some(Recorded::Problem(
            "Use a letter, a digit, a sign, an arrow or F1 to F12.",
        ));
    };
    Some(Recorded::Binding(KeyBinding {
        key,
        code: place,
        shift: press.shift,
        alt: press.alt,
        ctrl: mac && press.ctrl,
        ..Default::default()
    }))
}

// ------------------------------------------------------------------ Code's shortcuts

/// One of Code's shortcuts, with its default key (`Shortcut` in `shortcuts.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub id: &'static str,
    pub label: String,
    pub group: &'static str,
    pub binding: KeyBinding,
    /// False: ignored while typing in a text box (Ctrl+Alt is AltGr on Windows and many Linux layouts).
    pub global: bool,
}

fn shortcut(id: &'static str, label: &str, group: &'static str, binding: KeyBinding) -> Shortcut {
    Shortcut {
        id,
        label: label.into(),
        group,
        binding,
        global: true,
    }
}

/// The nine jump keys share one id.
pub const JUMP: &str = "jump";

/// Keys a person cannot set (`FIXED`): ⌘K is also Nexus UI's search, so the palette keeps it, and the nine jump keys
/// go together.
pub const FIXED: [&str; 2] = ["palette", JUMP];

/// The workbench's number keys: ⌃1 to ⌃6 on a Mac, Ctrl+Alt+1 to 6 elsewhere.
const TOOL_KEYS: [&str; 6] = [
    "tool-changes",
    "tool-files",
    "open-browser",
    "open-terminal",
    "tool-artifacts",
    "tool-agents",
];

/// Code's shortcuts with their defaults, in the cheat sheet's order.
pub fn code_shortcuts() -> Vec<Shortcut> {
    let mut list = vec![
        shortcut(
            "palette",
            "Command palette: find a session or run an action",
            "Sessions",
            KeyBinding::mod_key("k"),
        ),
        shortcut("new-session", "New session", "Sessions", KeyBinding::mod_key("n")),
        shortcut(
            "add-project",
            "Add a project (a folder)",
            "Sessions",
            KeyBinding::mod_key("o"),
        ),
    ];
    for n in 1..=9 {
        let digit = n.to_string();
        list.push(shortcut(
            JUMP,
            &format!("Jump to session {n}"),
            "Sessions",
            KeyBinding::at(&digit, &format!("Digit{n}")),
        ));
    }
    let rest = [
        (
            "previous-session",
            "Previous session",
            "Sessions",
            KeyBinding::at("[", "BracketLeft").shift(),
        ),
        (
            "next-session",
            "Next session",
            "Sessions",
            KeyBinding::at("]", "BracketRight").shift(),
        ),
        (
            "search-sessions",
            "Search the session's files (no session open: the sessions in the sidebar)",
            "Sessions",
            KeyBinding::mod_key("f").shift(),
        ),
        (
            "focus-composer",
            "Add the selection to the message, or focus the message box",
            "Session",
            KeyBinding::mod_key("l"),
        ),
        (
            "open-file",
            "Open a file of the session's folder",
            "Session",
            KeyBinding::mod_key("p"),
        ),
        (
            "find-in-session",
            "Find in the session",
            "Session",
            KeyBinding::mod_key("f"),
        ),
        (
            "toggle-changes",
            "Show or hide Changes",
            "Session",
            KeyBinding::mod_key("d").shift(),
        ),
        ("model-picker", "Pick the model", "Session", KeyBinding::mod_key("/")),
        (
            "open-inbox",
            "Open the Inbox",
            "Sessions",
            KeyBinding::mod_key("i").shift(),
        ),
        (
            "open-artifacts",
            "Open Artifacts",
            "Sessions",
            KeyBinding::mod_key("a").shift(),
        ),
        (
            "toggle-side-pane",
            "Show or hide the workbench",
            "Workbench",
            KeyBinding::at("\\", "Backslash").alt(),
        ),
        (
            "expand-workbench",
            "Expand the workbench, or show the conversation again",
            "Workbench",
            KeyBinding::at("Enter", "Enter").shift(),
        ),
        (
            "tool-changes",
            "Changes",
            "Workbench",
            KeyBinding::at("1", "Digit1").ctrl(),
        ),
        ("tool-files", "Files", "Workbench", KeyBinding::at("2", "Digit2").ctrl()),
        (
            "open-browser",
            "Browser",
            "Workbench",
            KeyBinding::at("3", "Digit3").ctrl(),
        ),
        (
            "open-terminal",
            "Terminal",
            "Workbench",
            KeyBinding::at("4", "Digit4").ctrl(),
        ),
        (
            "tool-artifacts",
            "Artifacts",
            "Workbench",
            KeyBinding::at("5", "Digit5").ctrl(),
        ),
        (
            "tool-agents",
            "Agents",
            "Workbench",
            KeyBinding::at("6", "Digit6").ctrl(),
        ),
        (
            "toggle-terminal",
            "Show or hide the terminal",
            "Workbench",
            KeyBinding::at("`", "Backquote").ctrl(),
        ),
        (
            "split-terminal",
            "Split the terminal (opens it first if it is hidden)",
            "Workbench",
            KeyBinding::at("5", "Digit5").shift().ctrl(),
        ),
        (
            "stop-turn",
            "Stop the agent's turn",
            "Session",
            KeyBinding::at(".", "Period"),
        ),
        ("cycle-model", "Next model", "Session", KeyBinding::mod_key("m").shift()),
        (
            "cycle-effort",
            "Next effort level",
            "Session",
            KeyBinding::mod_key("e").shift(),
        ),
        (
            "cycle-mode",
            "Next permission mode (Full access only from its menu)",
            "Session",
            KeyBinding::at("a", "KeyA").shift().alt(),
        ),
        (
            "archive-session",
            "Archive or restore the session",
            "Session",
            KeyBinding::mod_key("s").shift(),
        ),
        (
            "snooze-session",
            "Snooze the session (a snoozed one wakes)",
            "Session",
            KeyBinding::mod_key("h").shift(),
        ),
        (
            "copy-branch",
            "Copy the branch name",
            "Session",
            KeyBinding::mod_key("c").shift(),
        ),
        (
            "stash-prompt",
            "Stash the draft (empty message box: bring a stashed one back)",
            "Session",
            KeyBinding::mod_key("s"),
        ),
        (
            "rename-session",
            "Rename the session (or the sidebar row with focus)",
            "Session",
            KeyBinding::at("F2", "F2").bare(),
        ),
        (
            "delete-session",
            "Delete the session (or the sidebar row with focus)",
            "Session",
            KeyBinding::at("Backspace", "Backspace"),
        ),
        (
            "cheat-sheet",
            "Keyboard shortcuts",
            "Help",
            KeyBinding::at("/", "Slash").alt(),
        ),
    ];
    list.extend(
        rest.into_iter()
            .map(|(id, label, group, binding)| shortcut(id, label, group, binding)),
    );
    // ⌘⌫ deletes text in a text box: Code's only outside one, or in an empty one.
    if let Some(delete) = list.iter_mut().find(|s| s.id == "delete-session") {
        delete.global = false;
    }
    list
}

/// Whether a person may change `id`'s key.
pub fn editable(id: &str) -> bool {
    !FIXED.contains(&id)
}

/// The defaults on a platform (`forPlatform`). Off a Mac, Control is Mod, so ⌃1 to ⌃6 would be the session jumps: they
/// become Ctrl+Alt+1 to 6. Ctrl+Alt is AltGr there, so every default with Alt leaves a text box alone.
pub fn for_platform(list: &[Shortcut], mac: bool) -> Vec<Shortcut> {
    if mac {
        return list.to_vec();
    }
    list.iter()
        .map(|shortcut| {
            let mut out = shortcut.clone();
            if TOOL_KEYS.contains(&shortcut.id) && shortcut.binding.ctrl {
                out.binding.ctrl = false;
                out.binding.alt = true;
            }
            if out.binding.alt {
                out.global = false;
            }
            out
        })
        .collect()
}

/// `list` with the keys a person set (`withBindings`); one they cleared is left out. Off a Mac, a key with Alt leaves
/// a text box alone.
pub fn with_bindings(list: &[Shortcut], from: &Bindings, mac: bool) -> Vec<Shortcut> {
    list.iter()
        .filter_map(|shortcut| {
            if !editable(shortcut.id) {
                return Some(shortcut.clone());
            }
            match from.get(shortcut.id) {
                None => Some(shortcut.clone()),
                Some(None) => None,
                Some(Some(binding)) => {
                    let mut bound = shortcut.clone();
                    bound.binding = binding.clone();
                    bound.global = (mac || !binding.alt) && shortcut.id != "delete-session";
                    Some(bound)
                }
            }
        })
        .collect()
}

/// Code's shortcuts as the person has them on a platform (`shortcuts`).
pub fn shortcuts(from: &Bindings, mac: bool) -> Vec<Shortcut> {
    with_bindings(&for_platform(&code_shortcuts(), mac), from, mac)
}

// ------------------------------------------------------------------ The clash check

const SYSTEM: &str = "The system keeps that key.";
const EDIT: &str = "Text boxes use that key.";

/// Keys the system, the webview or another part of the app keeps (`RESERVED`), each with Mod on both platforms, and
/// why.
fn reserved() -> Vec<(&'static str, KeyBinding)> {
    let mut keys = Vec::new();
    // Quit, close the window, hide, minimise; the next window, Spotlight and the app switcher.
    for key in ["q", "w", "h", "m"] {
        keys.push((SYSTEM, KeyBinding::mod_key(key)));
    }
    keys.push((SYSTEM, KeyBinding::at("`", "Backquote")));
    keys.push((SYSTEM, KeyBinding::at(" ", "Space")));
    keys.push((SYSTEM, KeyBinding::at("Tab", "Tab")));
    // The Mac's screenshot keys.
    for n in ["3", "4", "5"] {
        keys.push((SYSTEM, KeyBinding::at(n, &format!("Digit{n}")).shift()));
    }
    // Select all, copy, cut, paste, undo and redo.
    for key in ["a", "c", "x", "v", "z", "y"] {
        keys.push((EDIT, KeyBinding::mod_key(key)));
    }
    keys.push((EDIT, KeyBinding::mod_key("z").shift()));
    keys.push(("That key opens Settings.", KeyBinding::at(",", "Comma")));
    keys.push((
        "That key hides or shows the sidebar.",
        KeyBinding::at("\\", "Backslash"),
    ));
    for (key, code) in [("=", "Equal"), ("-", "Minus"), ("0", "Digit0")] {
        keys.push(("That key changes the text size.", KeyBinding::at(key, code)));
    }
    keys
}

/// ⌘K or Ctrl+K on any platform (`isPaletteKey`).
fn is_palette_key(press: &KeyLike) -> bool {
    press.key.to_lowercase() == "k" && (press.meta || press.ctrl) && !press.alt && !press.shift
}

/// The new-window key, Mod+Shift+N.
fn new_window() -> KeyBinding {
    KeyBinding::mod_key("n").shift()
}

/// Why `binding` cannot be set (`takenBy`): the system or another of the app's keys has it. None when it is free.
/// `except` is the shortcut being changed, whose own key does not count. Checked on both platforms, because the same
/// settings follow the person from a Mac to a Linux computer. `more` adds a shell's own keys with what has them.
pub fn taken_by(
    binding: &KeyBinding,
    except: Option<&str>,
    from: &Bindings,
    more: &[(String, KeyBinding)],
) -> Option<String> {
    let reserved = reserved();
    for mac in [true, false] {
        let press = press_of(binding, mac);
        if let Some((why, _)) = reserved.iter().find(|(_, key)| matches(key, &press, mac)) {
            return Some((*why).into());
        }
        if is_palette_key(&press) {
            return Some("That key opens the command palette.".into());
        }
        if matches(&new_window(), &press, mac) {
            return Some("That key opens a new window.".into());
        }
        let list = shortcuts(from, mac);
        if let Some(hit) = list
            .iter()
            .filter(|shortcut| Some(shortcut.id) != except)
            .find(|shortcut| matches(&shortcut.binding, &press, mac))
        {
            let name = if hit.id == JUMP {
                "Jump to a session"
            } else {
                hit.label.as_str()
            };
            return Some(format!("“{name}” has that key."));
        }
        if let Some((name, _)) = more.iter().find(|(_, key)| matches(key, &press, mac)) {
            return Some(format!("“{name}” has that key."));
        }
    }
    None
}

// ------------------------------------------------------------------ Stepping through sessions

/// The entry `step` places from `current` in `order`, wrapping round; with none on screen, the first (or the last,
/// going back). Nexus UI's `neighbour` (`nexus-ui/shortcuts.ts`), for Mod+Shift+[ and Mod+Shift+].
pub fn neighbour<'a, T: AsRef<str>>(order: &'a [T], current: Option<&str>, forward: bool) -> Option<&'a T> {
    if order.is_empty() {
        return None;
    }
    let len = order.len();
    let at = current.and_then(|current| order.iter().position(|entry| entry.as_ref() == current));
    let next = match (at, forward) {
        (None, true) => 0,
        (None, false) => len - 1,
        (Some(at), true) => (at + 1) % len,
        (Some(at), false) => (at + len - 1) % len,
    };
    order.get(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn press(key: &str) -> KeyLike {
        KeyLike::new(key)
    }

    fn set(from: &mut Bindings, id: &str, binding: Option<KeyBinding>) {
        from.insert(id.into(), binding);
    }

    fn hit(from: &Bindings, press: &KeyLike, mac: bool) -> Option<&'static str> {
        shortcuts(from, mac)
            .into_iter()
            .find(|shortcut| matches(&shortcut.binding, press, mac))
            .map(|shortcut| shortcut.id)
    }

    // ---- the person's keys

    #[test]
    fn moves_a_shortcut_to_the_new_key_on_both_platforms_and_frees_the_old_one() {
        let mut from = Bindings::new();
        set(&mut from, "new-session", Some(KeyBinding::mod_key("o").shift()));
        let o = |meta, ctrl| KeyLike {
            key: "O".into(),
            meta,
            ctrl,
            shift: true,
            ..Default::default()
        };
        assert_eq!(hit(&from, &o(true, false), true), Some("new-session"));
        assert_eq!(hit(&from, &o(false, true), false), Some("new-session"));
        let n = KeyLike {
            meta: true,
            ..press("n")
        };
        assert_eq!(hit(&from, &n, true), None);
    }

    #[test]
    fn drops_a_cleared_shortcut() {
        let mut from = Bindings::new();
        set(&mut from, "toggle-terminal", None);
        let key = KeyLike {
            ctrl: true,
            code: Some("Backquote".into()),
            ..press("`")
        };
        assert_eq!(hit(&from, &key, true), None);
        assert!(!shortcuts(&from, true).iter().any(|s| s.id == "toggle-terminal"));
    }

    #[test]
    fn leaves_a_text_box_alone_with_ctrl_alt_off_a_mac() {
        let mut from = Bindings::new();
        set(&mut from, "find-in-session", Some(KeyBinding::at("q", "KeyQ").alt()));
        let find = |mac| {
            shortcuts(&from, mac)
                .into_iter()
                .find(|s| s.id == "find-in-session")
                .expect("find-in-session shortcut exists")
        };
        assert!(!find(false).global);
        assert!(find(true).global);
    }

    #[test]
    fn does_not_let_the_palette_or_the_jump_keys_move() {
        let mut from = Bindings::new();
        set(&mut from, "palette", Some(KeyBinding::mod_key("p").shift()));
        let k = KeyLike {
            meta: true,
            ..press("k")
        };
        assert_eq!(hit(&from, &k, true), Some("palette"));
    }

    #[test]
    fn the_workbench_number_keys_are_ctrl_alt_off_a_mac() {
        let list = shortcuts(&Bindings::new(), false);
        let changes = list
            .iter()
            .find(|s| s.id == "tool-changes")
            .expect("tool-changes shortcut exists");
        assert_eq!(changes.binding, KeyBinding::at("1", "Digit1").alt());
        assert!(!changes.global);
    }

    #[test]
    fn a_rebound_non_alt_key_works_while_typing_except_delete_session() {
        let mut from = Bindings::new();
        for id in ["tool-changes", "cycle-mode", "delete-session"] {
            set(&mut from, id, Some(KeyBinding::mod_key("x").shift()));
        }
        let list = shortcuts(&from, false);
        for id in ["tool-changes", "cycle-mode"] {
            assert!(
                list.iter()
                    .find(|shortcut| shortcut.id == id)
                    .expect("rebound shortcut")
                    .global
            );
        }
        assert!(
            !list
                .iter()
                .find(|shortcut| shortcut.id == "delete-session")
                .expect("delete-session shortcut")
                .global
        );
    }

    // ---- parseBindings

    #[test]
    fn keeps_what_reads_and_drops_the_rest() {
        let value = json!({"a": {"key": "k", "shift": true}, "b": null, "c": {"key": 3}, "d": "mod+k", "e": {"key": "k", "alt": "yes"}});
        let parsed = parse_bindings(&value);
        let mut expected = Bindings::new();
        expected.insert("a".into(), Some(KeyBinding::mod_key("k").shift()));
        expected.insert("b".into(), None);
        assert_eq!(parsed, expected);
        assert!(parse_bindings(&json!([1])).is_empty());
        assert!(parse_bindings(&Value::Null).is_empty());
    }

    #[test]
    fn writes_the_stored_form() {
        let mut from = Bindings::new();
        set(&mut from, "stop-turn", Some(KeyBinding::at("F9", "F9").bare()));
        set(&mut from, "copy-branch", None);
        set(&mut from, "cycle-mode", Some(KeyBinding::mod_key("j").shift()));
        assert_eq!(
            serde_json::to_value(&from).expect("bindings serialize"),
            json!({"copy-branch": null, "cycle-mode": {"key": "j", "shift": true}, "stop-turn": {"key": "F9", "code": "F9", "bare": true}})
        );
        assert_eq!(
            parse_bindings(&serde_json::to_value(&from).expect("bindings serialize")),
            from
        );
    }

    // ---- recordBinding

    fn recorded(press: KeyLike, mac: bool) -> Option<Recorded> {
        record_binding(&press, mac)
    }

    #[test]
    fn waits_while_only_a_modifier_is_down() {
        assert_eq!(
            recorded(
                KeyLike {
                    meta: true,
                    ..press("Meta")
                },
                true
            ),
            None
        );
        assert_eq!(
            recorded(
                KeyLike {
                    shift: true,
                    ..press("Shift")
                },
                false
            ),
            None
        );
    }

    #[test]
    fn stores_mod_not_cmd_or_ctrl() {
        let k = |meta, ctrl, shift| KeyLike {
            key: if shift { "K" } else { "k" }.into(),
            code: Some("KeyK".into()),
            meta,
            ctrl,
            shift,
            alt: false,
        };
        assert_eq!(
            recorded(k(true, false, true), true),
            Some(Recorded::Binding(KeyBinding::mod_key("k").shift()))
        );
        assert_eq!(
            recorded(k(false, true, false), false),
            Some(Recorded::Binding(KeyBinding::mod_key("k")))
        );
        // ⌃ on a Mac is Control everywhere.
        let backquote = KeyLike {
            ctrl: true,
            code: Some("Backquote".into()),
            ..press("`")
        };
        assert_eq!(
            recorded(backquote, true),
            Some(Recorded::Binding(KeyBinding::at("`", "Backquote").ctrl()))
        );
    }

    #[test]
    fn takes_a_letter_by_its_place_when_alt_changes_it_digits_and_signs_always() {
        let with = |key: &str, code: &str, meta, ctrl, shift, alt| KeyLike {
            key: key.into(),
            code: Some(code.into()),
            meta,
            ctrl,
            shift,
            alt,
        };
        assert_eq!(
            recorded(with("∆", "KeyJ", true, false, false, true), true),
            Some(Recorded::Binding(KeyBinding::at("j", "KeyJ").alt()))
        );
        assert_eq!(
            recorded(with("%", "Digit5", false, true, true, false), false),
            Some(Recorded::Binding(KeyBinding::at("5", "Digit5").shift()))
        );
        assert_eq!(
            recorded(with("ü", "BracketLeft", false, true, false, false), false),
            Some(Recorded::Binding(KeyBinding::at("[", "BracketLeft")))
        );
        assert_eq!(
            recorded(with("к", "KeyR", false, true, false, false), false),
            Some(Recorded::Binding(KeyBinding::at("r", "KeyR")))
        );
    }

    #[test]
    fn takes_f1_to_f12_alone_and_refuses_other_keys_without_mod() {
        let key = |key: &str, code: &str| KeyLike {
            code: Some(code.into()),
            ..press(key)
        };
        assert_eq!(
            recorded(key("F5", "F5"), true),
            Some(Recorded::Binding(KeyBinding::at("F5", "F5").bare()))
        );
        let problem = |p: KeyLike, mac| matches!(record_binding(&p, mac), Some(Recorded::Problem(_)));
        assert!(problem(key("k", "KeyK"), true));
        assert!(problem(
            KeyLike {
                alt: true,
                ..key("k", "KeyK")
            },
            false
        ));
        assert!(problem(
            KeyLike {
                meta: true,
                ..key("k", "KeyK")
            },
            false
        ));
        assert!(problem(
            KeyLike {
                meta: true,
                ctrl: true,
                ..key("k", "KeyK")
            },
            true
        ));
        assert!(problem(
            KeyLike {
                ctrl: true,
                ..key("Unidentified", "IntlRo")
            },
            false
        ));
        assert!(!is_function_key("F13") && !is_function_key("F0") && !is_function_key("F01"));
    }

    #[test]
    fn records_a_key_the_matcher_reads_back_as_the_same_press() {
        let event = KeyLike {
            key: "ArrowUp".into(),
            code: Some("ArrowUp".into()),
            meta: true,
            alt: true,
            ..Default::default()
        };
        let Some(Recorded::Binding(binding)) = record_binding(&event, true) else {
            panic!("not recorded");
        };
        assert!(matches(&binding, &event, true));
    }

    // ---- sameBinding

    #[test]
    fn tells_a_default_pressed_again_from_a_new_key() {
        let stop = code_shortcuts()
            .into_iter()
            .find(|s| s.id == "stop-turn")
            .expect("stop-turn shortcut exists");
        assert!(same_binding(&KeyBinding::at(".", "Period"), &stop.binding));
        assert!(!same_binding(&KeyBinding::at(".", "Period").shift(), &stop.binding));
    }

    // ---- takenBy

    fn taken(binding: KeyBinding, except: Option<&str>, from: &Bindings) -> Option<String> {
        taken_by(&binding, except, from, &[])
    }

    #[test]
    fn names_the_shortcut_that_already_has_the_key() {
        let none = Bindings::new();
        assert_eq!(
            taken(KeyBinding::mod_key("n"), None, &none).as_deref(),
            Some("“New session” has that key.")
        );
        assert_eq!(
            taken(KeyBinding::at("1", "Digit1"), None, &none).as_deref(),
            Some("“Jump to a session” has that key.")
        );
        assert!(
            taken(KeyBinding::at("F2", "F2").bare(), None, &none)
                .expect("rename-session shortcut owns F2")
                .contains("Rename the session")
        );
    }

    #[test]
    fn does_not_count_the_shortcut_being_changed() {
        assert_eq!(
            taken(KeyBinding::mod_key("n"), Some("new-session"), &Bindings::new()),
            None
        );
    }

    #[test]
    fn frees_a_key_once_its_shortcut_moves_or_is_cleared() {
        let mut from = Bindings::new();
        set(&mut from, "new-session", Some(KeyBinding::at("o", "KeyO").alt()));
        assert_eq!(taken(KeyBinding::mod_key("n"), None, &from), None);
        assert_eq!(
            taken(KeyBinding::at("o", "KeyO").alt(), None, &from).as_deref(),
            Some("“New session” has that key.")
        );
        set(&mut from, "copy-branch", None);
        assert_eq!(taken(KeyBinding::mod_key("c").shift(), None, &from), None);
    }

    #[test]
    fn refuses_the_systems_keys_and_the_apps_own_on_either_platform() {
        let none = Bindings::new();
        let why = |binding| taken(binding, None, &none);
        assert_eq!(why(KeyBinding::mod_key("q")).as_deref(), Some(SYSTEM));
        assert_eq!(why(KeyBinding::mod_key("c")).as_deref(), Some(EDIT));
        assert_eq!(why(KeyBinding::mod_key("z").shift()).as_deref(), Some(EDIT));
        assert_eq!(why(KeyBinding::at("4", "Digit4").shift()).as_deref(), Some(SYSTEM));
        assert_eq!(
            why(KeyBinding::mod_key("k")).as_deref(),
            Some("That key opens the command palette.")
        );
        assert_eq!(
            why(KeyBinding::mod_key("n").shift()).as_deref(),
            Some("That key opens a new window.")
        );
        assert_eq!(
            why(KeyBinding::at("=", "Equal")).as_deref(),
            Some("That key changes the text size.")
        );
        assert_eq!(
            why(KeyBinding::at("\\", "Backslash")).as_deref(),
            Some("That key hides or shows the sidebar.")
        );
    }

    #[test]
    fn counts_a_macs_control_key_as_ctrl_elsewhere() {
        let none = Bindings::new();
        assert_eq!(
            taken(KeyBinding::mod_key("q").ctrl(), None, &none).as_deref(),
            Some(SYSTEM)
        );
        assert_eq!(
            taken(KeyBinding::mod_key("n").shift().ctrl(), None, &none).as_deref(),
            Some("That key opens a new window.")
        );
    }

    #[test]
    fn matches_a_letter_set_by_place_against_one_matched_by_what_it_types() {
        assert_eq!(
            taken(KeyBinding::at("l", "KeyL"), None, &Bindings::new()).as_deref(),
            Some("“Add the selection to the message, or focus the message box” has that key.")
        );
    }

    #[test]
    fn leaves_a_free_key_free() {
        let none = Bindings::new();
        assert_eq!(taken(KeyBinding::at("j", "KeyJ").alt(), None, &none), None);
        assert_eq!(taken(KeyBinding::at("F7", "F7").bare(), None, &none), None);
    }

    #[test]
    fn names_a_shells_own_key() {
        let more = [("Show or hide the sidebar".to_string(), KeyBinding::mod_key("b"))];
        assert_eq!(
            taken_by(&KeyBinding::mod_key("b"), None, &Bindings::new(), &more).as_deref(),
            Some("“Show or hide the sidebar” has that key.")
        );
    }

    #[test]
    fn steps_to_the_next_or_previous_session_and_wraps_round() {
        let order = ["a", "b", "c"];
        assert_eq!(neighbour(&order, Some("a"), true), Some(&"b"));
        assert_eq!(neighbour(&order, Some("c"), true), Some(&"a"));
        assert_eq!(neighbour(&order, Some("a"), false), Some(&"c"));
        // None on screen: the first going on, the last going back.
        assert_eq!(neighbour(&order, None, true), Some(&"a"));
        assert_eq!(neighbour(&order, Some("z"), false), Some(&"c"));
        assert_eq!(neighbour::<&str>(&[], None, true), None);
    }
}
