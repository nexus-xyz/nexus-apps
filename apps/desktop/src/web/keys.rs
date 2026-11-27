//! GPUI's keystrokes as Chromium's key events. Chromium reads a key by its Windows virtual-key code (`VK_*`, the code
//! for the physical key) and, for text, by the character it types. So a key press is a `RAWKEYDOWN` with the code,
//! then a `CHAR` per character typed, and the release a `KEYUP`. The app builds them here; the web host process hands
//! them to Chromium (`host.rs`).

use gpui_kit::{Keystroke, Modifiers};

use super::protocol::{KeyInput, KeyKind};

// `cef_event_flags_t`.
const SHIFT: u32 = 1 << 1;
pub(super) const CONTROL: u32 = 1 << 2;
const ALT: u32 = 1 << 3;
pub(super) const LEFT_BUTTON: u32 = 1 << 4;
pub(super) const MIDDLE_BUTTON: u32 = 1 << 5;
pub(super) const RIGHT_BUTTON: u32 = 1 << 6;
const COMMAND: u32 = 1 << 7;
/// A scroll in exact pixels (a touchpad): Chromium moves the page by it at once. Without it, Chromium animates each
/// event as a wheel notch, and a touchpad's stream of small events turns into animations that fight each other.
pub(super) const PRECISE_SCROLL: u32 = 1 << 14;

/// The modifiers as Chromium's event flags.
pub(super) fn flags(modifiers: &Modifiers) -> u32 {
    let mut flags = 0;
    if modifiers.shift {
        flags |= SHIFT;
    }
    if modifiers.control {
        flags |= CONTROL;
    }
    if modifiers.alt {
        flags |= ALT;
    }
    if modifiers.platform {
        flags |= COMMAND;
    }
    flags
}

/// A key press as Chromium's events: `RawDown` with the key's code, then a `Char` per UTF-16 unit it types.
pub(super) fn key_down(keystroke: &Keystroke) -> Vec<KeyInput> {
    let down = KeyInput {
        kind: KeyKind::RawDown,
        modifiers: flags(&keystroke.modifiers),
        code: key_code(&keystroke.key),
        character: 0,
    };
    // Text, unless a shortcut: Ctrl+C copies, it does not type "c".
    let shortcut = keystroke.modifiers.control || keystroke.modifiers.platform;
    let typed = match keystroke.key_char.as_deref() {
        Some(text) if !shortcut => Some(text.to_string()),
        // Enter types a newline, as in a browser.
        None if keystroke.key == "enter" && !shortcut => Some("\r".into()),
        _ => None,
    };
    std::iter::once(down)
        .chain(typed.iter().flat_map(|text| text.encode_utf16()).map(|unit| KeyInput {
            kind: KeyKind::Char,
            code: unit as i32,
            character: unit,
            ..down
        }))
        .collect()
}

pub(super) fn key_up(keystroke: &Keystroke) -> KeyInput {
    KeyInput {
        kind: KeyKind::Up,
        modifiers: flags(&keystroke.modifiers),
        code: key_code(&keystroke.key),
        character: 0,
    }
}

/// A key for `nexus-servo-host`, which turns GPUI's key name and text into the DOM's key.
pub(super) fn servo_key(down: bool, keystroke: &Keystroke) -> serde_json::Value {
    let modifiers = &keystroke.modifiers;
    serde_json::json!({
        "cmd": "key", "down": down, "key": keystroke.key, "text": keystroke.key_char,
        "shift": modifiers.shift, "ctrl": modifiers.control, "alt": modifiers.alt, "meta": modifiers.platform,
    })
}

/// The Windows virtual-key code of GPUI's key name (`"a"`, `"enter"`, `"f5"`); 0 when there is none.
fn key_code(key: &str) -> i32 {
    let named = match key {
        "backspace" => 0x08,
        "tab" => 0x09,
        "enter" => 0x0D,
        "shift" => 0x10,
        "control" => 0x11,
        "alt" => 0x12,
        "escape" => 0x1B,
        "space" => 0x20,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" => 0x2D,
        "delete" => 0x2E,
        ";" => 0xBA,
        "=" => 0xBB,
        "," => 0xBC,
        "-" => 0xBD,
        "." => 0xBE,
        "/" => 0xBF,
        "`" => 0xC0,
        "[" => 0xDB,
        "\\" => 0xDC,
        "]" => 0xDD,
        "'" => 0xDE,
        _ => 0,
    };
    if named != 0 {
        return named;
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<i32>().ok())
        && (1..=24).contains(&n)
    {
        return 0x70 + n - 1;
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as i32,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyKind, key_code, key_down};
    use gpui_kit::Keystroke;

    #[test]
    fn keys_map_to_windows_codes() {
        assert_eq!(key_code("a"), 'A' as i32);
        assert_eq!(key_code("7"), '7' as i32);
        assert_eq!(key_code("enter"), 0x0D);
        assert_eq!(key_code("f5"), 0x74);
        assert_eq!(key_code("/"), 0xBF);
        assert_eq!(key_code("fn"), 0);
    }

    #[test]
    fn typing_sends_the_code_then_the_text() {
        let keystroke = Keystroke::parse("shift-a").unwrap();
        let keystroke = Keystroke {
            key_char: Some("A".into()),
            ..keystroke
        };
        let events = key_down(&keystroke);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, KeyKind::RawDown);
        assert_eq!(events[0].code, 'A' as i32);
        assert_eq!(events[1].kind, KeyKind::Char);
        assert_eq!(events[1].character, 'A' as u16);
    }

    #[test]
    fn shortcuts_type_nothing() {
        let keystroke = Keystroke {
            key_char: Some("c".into()),
            ..Keystroke::parse("ctrl-c").unwrap()
        };
        assert_eq!(key_down(&keystroke).len(), 1);
    }
}
