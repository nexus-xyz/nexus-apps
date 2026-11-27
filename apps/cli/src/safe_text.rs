//! Text that another person wrote, made safe to print on a terminal (`safe-text.ts`). A promotion request
//! carries a skill's description, note and full text, and the author is not the reader. A terminal runs what it reads:
//! an escape sequence can move the cursor, rewrite earlier lines, set the title or fill the clipboard. A reader also
//! cannot see a zero-width or bidirectional control character, so it can hide or reorder text.
//!
//! `safe_text` shows each such character as its code in brackets, `[U+001B]`, so the reader sees that it is there and
//! nothing reaches the terminal as a command. A newline and a tab pass. Every other control character is a hazard: the
//! C0 controls (escape, backspace, carriage return, bell and the rest), DEL, and the C1 controls (U+0080 to U+009F,
//! which include the single-character CSI and OSC). So are the characters a reader cannot see: the soft hyphen, the
//! zero-width, bidirectional and word-joiner characters, the invisible operators, the fillers, the line and paragraph
//! separators, the byte-order mark, the shorthand and musical format controls, and the Unicode tag and
//! variation-selector characters that hide text. Only the variation selectors U+FE0E and U+FE0F pass, because emoji use
//! them.
//!
//! The set is the same in both CLIs: `skills-cli.json` in this crate lists it, and a test in each CLI reads
//! that file.
//!
//! `safe_json` does the same for a JSON document that a person may read in a terminal. The escape is `\u009b`, so the
//! document means what it meant, and a machine that parses it reads the same text.

use std::fmt::Write;

/// Whether `c` is a control character the terminal could run, or a character the reader cannot see. A newline and a tab
/// are not.
pub fn is_hazard(c: char) -> bool {
    matches!(c,
        '\u{0}'..='\u{8}'
        | '\u{b}'..='\u{1f}'
        | '\u{7f}'..='\u{9f}'
        | '\u{ad}'
        | '\u{34f}'
        | '\u{61c}'
        | '\u{115f}'..='\u{1160}'
        | '\u{17b4}'..='\u{17b5}'
        | '\u{180b}'..='\u{180f}'
        | '\u{200b}'..='\u{200f}'
        | '\u{2028}'..='\u{202e}'
        | '\u{2060}'..='\u{206f}'
        | '\u{3164}'
        | '\u{fe00}'..='\u{fe0d}'
        | '\u{feff}'
        | '\u{ffa0}'
        | '\u{fff9}'..='\u{fffb}'
        | '\u{1bca0}'..='\u{1bca3}'
        | '\u{1d173}'..='\u{1d17a}'
        | '\u{e0000}'..='\u{e007f}'
        | '\u{e0100}'..='\u{e01ef}')
}

/// The text with each hazard shown as `[U+XXXX]`. Text without one is returned as it is.
pub fn safe_text(text: &str) -> String {
    if !text.chars().any(is_hazard) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        if is_hazard(c) {
            let _ = write!(out, "[U+{:04X}]", c as u32);
        } else {
            out.push(c);
        }
    }
    out
}

/// A JSON document with each hazard written as a `\uXXXX` escape (a surrogate pair above U+FFFF). A hazard can only be
/// inside a string, because JSON allows nothing else there, and a string means the same with the escape.
pub fn safe_json(json: &str) -> String {
    if !json.chars().any(is_hazard) {
        return json.to_string();
    }
    let mut out = String::with_capacity(json.len() + 16);
    for c in json.chars() {
        if !is_hazard(c) {
            out.push(c);
        } else if (c as u32) <= 0xffff {
            let _ = write!(out, "\\u{:04x}", c as u32);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                let _ = write!(out, "\\u{unit:04x}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_a_newline_and_a_tab_pass_unchanged() {
        let text = "# Title\n\n\tIndented line with é, 日本語 and an emoji 👍🏽 (U+FE0F: ❤️).\n";
        assert_eq!(safe_text(text), text);
        assert_eq!(safe_json("{\n  \"a\": \"b\\n\"\n}"), "{\n  \"a\": \"b\\n\"\n}");
    }

    #[test]
    fn escape_sequences_never_reach_the_terminal() {
        // ESC, a CSI sequence, an OSC title with BEL and ST, the C1 CSI and OSC, backspace, carriage return and DEL.
        let attack = "a\u{1b}[2J\u{1b}]0;title\u{7}b\u{1b}\\c\u{9b}31md\u{9d}0;t\u{9c}e\u{8}f\rg\u{7f}h";
        let shown = safe_text(attack);
        assert_eq!(
            shown,
            "a[U+001B][2J[U+001B]]0;title[U+0007]b[U+001B]\\c[U+009B]31md[U+009D]0;t[U+009C]e[U+0008]f[U+000D]g[U+007F]h"
        );
        assert!(!shown.chars().any(is_hazard), "{shown:?}");
    }

    #[test]
    fn invisible_and_bidirectional_characters_show_as_codes() {
        assert_eq!(safe_text("a\u{200b}b"), "a[U+200B]b");
        assert_eq!(safe_text("\u{feff}start"), "[U+FEFF]start");
        // The Trojan Source set: embeddings, overrides, pops and isolates.
        assert_eq!(
            safe_text(
                "\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2066}\u{2067}\u{2068}\u{2069}\u{200e}\u{200f}\u{61c}"
            ),
            "[U+202A][U+202B][U+202C][U+202D][U+202E][U+2066][U+2067][U+2068][U+2069][U+200E][U+200F][U+061C]"
        );
        assert_eq!(safe_text("x\u{200c}\u{200d}\u{2060}y"), "x[U+200C][U+200D][U+2060]y");
        assert_eq!(safe_text("\u{2028}\u{2029}"), "[U+2028][U+2029]");
        // Tag characters spell hidden ASCII.
        assert_eq!(safe_text("hi\u{e0041}\u{e0042}"), "hi[U+E0041][U+E0042]");
        assert_eq!(
            safe_text("\u{ad}\u{34f}\u{3164}\u{ffa0}"),
            "[U+00AD][U+034F][U+3164][U+FFA0]"
        );
    }

    #[test]
    fn a_very_long_line_is_kept_whole_and_every_hazard_in_it_is_shown() {
        let line = format!("{}\u{1b}[31m{}\u{202e}", "x".repeat(1_000_000), "y".repeat(1_000_000));
        let shown = safe_text(&line);
        assert_eq!(
            shown.len(),
            2_000_000 + "[U+001B]".len() + "[31m".len() + "[U+202E]".len()
        );
        assert!(shown.starts_with(&"x".repeat(1_000_000)));
        assert!(shown.ends_with("[U+202E]"));
        assert!(!shown.contains('\u{1b}'));
        // One long line with nothing to show comes back as it is.
        let plain = "z".repeat(3_000_000);
        assert_eq!(safe_text(&plain), plain);
    }

    #[test]
    fn json_keeps_its_meaning_with_each_hazard_escaped() {
        let value = serde_json::json!({
            "content": "a\u{9b}31m \u{200b}\u{202e}b\u{7f}\u{e0041}",
            "list": ["\u{feff}"],
        });
        let text = serde_json::to_string_pretty(&value).expect("json");
        let safe = safe_json(&text);
        assert!(!safe.chars().any(is_hazard), "{safe:?}");
        assert!(
            safe.contains("\\u009b31m \\u200b\\u202eb\\u007f\\udb40\\udc41"),
            "{safe}"
        );
        let back: serde_json::Value = serde_json::from_str(&safe).expect("still json");
        assert_eq!(back, value);
    }

    #[test]
    fn variation_selectors_hide_text_except_the_two_that_emoji_use() {
        // Sixteen variation selectors in the BMP and 240 above it carry a byte each; only the two of emoji pass.
        assert_eq!(
            safe_text("a\u{fe00}b\u{fe0d}c\u{fe0e}d\u{fe0f}e\u{e0100}"),
            "a[U+FE00]b[U+FE0D]c\u{fe0e}d\u{fe0f}e[U+E0100]"
        );
        assert_eq!(
            safe_text("\u{180f}\u{1bca0}\u{1bca3}\u{1d173}\u{1d17a}"),
            "[U+180F][U+1BCA0][U+1BCA3][U+1D173][U+1D17A]"
        );
    }

    #[test]
    fn the_hazard_set_is_the_set_in_skills_cli_json() {
        // The TypeScript CLI's test reads the same file, so a change to one list fails the other CLI's test.
        let shared: serde_json::Value =
            serde_json::from_str(include_str!("../skills-cli.json")).expect("skills-cli.json");
        let mut ranges: Vec<[u32; 2]> = Vec::new();
        for code in (0..=0x10ffff_u32).filter(|code| char::from_u32(*code).is_some_and(is_hazard)) {
            match ranges.last_mut() {
                Some(last) if last[1] + 1 == code => last[1] = code,
                _ => ranges.push([code, code]),
            }
        }
        assert_eq!(serde_json::json!(ranges), shared["hazards"]);
    }
}
