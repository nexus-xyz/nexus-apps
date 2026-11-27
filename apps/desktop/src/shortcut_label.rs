//! Shortcut labels, one format everywhere (tooltips, menus, the palette, the sidebar): written from the same keystroke
//! text the key bindings use (`"ctrl-shift-f"`, `"secondary-k"`), so a label can't drift from its binding.
//!
//! On macOS, symbols in Apple's order with no separators, as the system menus and Linear: `⌃⌥⇧⌘F`. Elsewhere, names
//! joined by "+" in Ctrl, Alt, Shift, Super order, as Windows, GNOME and VS Code: `Ctrl+Shift+F`. `secondary` is Cmd on
//! macOS and Ctrl elsewhere, as in GPUI's key bindings.

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::h_flex;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// The Workbench's keys, bound in `main.rs` and labelled in `workspace/side_pane.rs` from these same strings.
pub const TOGGLE_SIDE_PANE: &str = if cfg!(target_os = "macos") {
    "alt-cmd-\\"
} else {
    "alt-ctrl-\\"
};
pub const MAXIMIZE_WORKBENCH: &str = if cfg!(target_os = "macos") {
    "cmd-shift-enter"
} else {
    "ctrl-shift-enter"
};
pub const NEW_TERMINAL: &str = "ctrl-shift-`";
/// The Workbench's tools, in the order of `Tool::ALL`: Ctrl+1 to Ctrl+6 on a Mac, where Cmd+1 to Cmd+9 jump to
/// sessions, and Ctrl+Alt+1 to 6 elsewhere, where Ctrl is Mod (the Nexus app's `forPlatform`).
pub const TOOL_KEYS: [&str; 6] = if cfg!(target_os = "macos") {
    ["ctrl-1", "ctrl-2", "ctrl-3", "ctrl-4", "ctrl-5", "ctrl-6"]
} else {
    [
        "ctrl-alt-1",
        "ctrl-alt-2",
        "ctrl-alt-3",
        "ctrl-alt-4",
        "ctrl-alt-5",
        "ctrl-alt-6",
    ]
};

/// The label for a keystroke on this platform: `label("ctrl-shift-f")` is "Ctrl+Shift+F" (or "⌃⇧F" on macOS).
pub fn label(keystroke: &str) -> String {
    format_for(keystroke, cfg!(target_os = "macos"))
}

/// The keys one at a time, for keycaps: `keys("ctrl-shift-f")` is ["Ctrl", "Shift", "F"] (["⌃", "⇧", "F"] on macOS).
pub fn keys(keystroke: &str) -> Vec<String> {
    keys_for(keystroke, cfg!(target_os = "macos"))
}

/// A tooltip with its shortcut after it: "Search (Ctrl+K)".
pub fn with_hint(text: &str, keystroke: &str) -> String {
    format!("{text} ({})", label(keystroke))
}

/// A tooltip with its shortcut as keycaps after the name, as the menus show it: "Files" `Ctrl` `2`.
pub fn tooltip(text: &str, keystroke: &str) -> Tooltip {
    let text = text.to_string();
    let keys = keys(keystroke);
    Tooltip::element(move |_, cx| {
        let ink = cx.theme().popover_foreground;
        h_flex()
            .gap(px(8.))
            .items_center()
            .child(text.clone())
            .child(h_flex().gap(px(3.)).children(keys.iter().map(|key| {
                div()
                    .min_w(px(18.))
                    .h(px(18.))
                    .px(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .bg(ink.opacity(0.16))
                    .text_size(px(11.))
                    .child(key.clone())
            })))
    })
}

fn format_for(keystroke: &str, mac: bool) -> String {
    keys_for(keystroke, mac).join(if mac { "" } else { "+" })
}

fn keys_for(keystroke: &str, mac: bool) -> Vec<String> {
    let (mut ctrl, mut alt, mut shift, mut cmd) = (false, false, false, false);
    // The key is the last part; "-" alone (or "ctrl--") is the minus key.
    let (modifiers, key) = match keystroke.strip_suffix("--") {
        Some(rest) => (rest, "-"),
        None => keystroke.rsplit_once('-').unwrap_or(("", keystroke)),
    };
    for modifier in modifiers.split('-').filter(|part| !part.is_empty()) {
        match modifier {
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            "cmd" | "super" | "win" | "platform" => cmd = true,
            "secondary" if mac => cmd = true,
            "secondary" => ctrl = true,
            _ => {}
        }
    }
    let names = if mac {
        [(ctrl, "⌃"), (alt, "⌥"), (shift, "⇧"), (cmd, "⌘")]
    } else {
        [(ctrl, "Ctrl"), (alt, "Alt"), (shift, "Shift"), (cmd, "Super")]
    };
    names
        .into_iter()
        .filter_map(|(on, name)| on.then(|| name.to_string()))
        .chain(std::iter::once(key_name(key, mac)))
        .collect()
}

fn key_name(key: &str, mac: bool) -> String {
    let named = match key {
        "enter" | "return" => Some(if mac { "↩" } else { "Enter" }),
        "escape" | "esc" => Some(if mac { "⎋" } else { "Esc" }),
        "backspace" => Some(if mac { "⌫" } else { "Backspace" }),
        "delete" => Some(if mac { "⌦" } else { "Delete" }),
        "tab" => Some(if mac { "⇥" } else { "Tab" }),
        "space" => Some("Space"),
        "up" => Some("↑"),
        "down" => Some("↓"),
        "left" => Some("←"),
        "right" => Some("→"),
        "pageup" => Some("PgUp"),
        "pagedown" => Some("PgDn"),
        "home" => Some("Home"),
        "end" => Some("End"),
        _ => None,
    };
    match named {
        Some(name) => name.to_string(),
        // Letters and F-keys upper case ("F", "F5"); symbols as they are ("`", "\\", ",").
        None => key.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::{format_for, keys_for};

    #[test]
    fn linux_and_windows_join_names_with_plus() {
        assert_eq!(format_for("ctrl-shift-f", false), "Ctrl+Shift+F");
        assert_eq!(format_for("secondary-k", false), "Ctrl+K");
        // Modifier order is fixed, whatever order the binding wrote them in.
        assert_eq!(format_for("alt-ctrl-\\", false), "Ctrl+Alt+\\");
        assert_eq!(format_for("ctrl-`", false), "Ctrl+`");
        assert_eq!(format_for("ctrl-shift-enter", false), "Ctrl+Shift+Enter");
        assert_eq!(format_for("alt-left", false), "Alt+←");
        assert_eq!(format_for("ctrl--", false), "Ctrl+-");
        assert_eq!(format_for("f5", false), "F5");
    }

    #[test]
    fn macos_uses_symbols_in_apples_order() {
        assert_eq!(format_for("ctrl-shift-f", true), "⌃⇧F");
        assert_eq!(format_for("secondary-k", true), "⌘K");
        assert_eq!(format_for("cmd-shift-alt-ctrl-p", true), "⌃⌥⇧⌘P");
        assert_eq!(format_for("alt-cmd-\\", true), "⌥⌘\\");
        assert_eq!(format_for("ctrl-shift-enter", true), "⌃⇧↩");
        assert_eq!(format_for("cmd-[", true), "⌘[");
    }

    #[test]
    fn keycaps_split_the_same_label() {
        assert_eq!(keys_for("alt-ctrl-\\", false), ["Ctrl", "Alt", "\\"]);
        assert_eq!(keys_for("secondary-k", true), ["⌘", "K"]);
    }
}
