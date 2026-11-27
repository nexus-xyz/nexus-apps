//! What the session draws, as plain functions of its state: the header, a prompt, an answer, a notice, the status
//! line, the composer and the footer. Each returns finished terminal lines (styled with `style.rs`'s tokens, no wider
//! than the width given), so the tests read them without a terminal.
//!
//! The layout follows Codex's terminal UI (openai/codex, `codex-rs/tui`, Apache-2.0); the look is brand v5.0's Nexus AI
//! product layer (`brand/v5.0/nexus-ai/SPEC.md`): the name in the strong ink in the header, a prompt on a raised band
//! after `›` (areas part by tone, never by lines), an answer after `•`, `■` for a stop or an error, and
//! `⣾ Thinking… 3s · esc to interrupt` while an answer streams, the braille spinner turning and a light band sweeping
//! the words.

use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use crate::error::CliError;
use crate::safe_text::safe_text;
use crate::style::{Depth, Style, ansi256, strip_ansi};

use super::composer::Composer;
use super::logo::{Paint, is_light};

/// The CLI's style, the shade a prompt and the composer sit on (the brand's `raised`), and the fill of inline code (its
/// `selected`); neither when the terminal's background is unknown.
#[derive(Debug, Clone)]
pub struct Theme {
    pub style: Style,
    shade: Option<String>,
    fill: Option<String>,
    background: Option<(u8, u8, u8)>,
    /// The terminal's cells: a Braille dot's height over its width (`terminal::aspect`), so the mark draws round.
    pub aspect: f64,
}

impl Theme {
    pub fn new(style: Style, background: Option<(u8, u8, u8)>) -> Theme {
        // A known background decides the tones' light or dark values.
        let style = match background {
            Some(background) => style.with_light(is_light(background)),
            None => style,
        };
        let tone = |lift: Lift| {
            background
                .filter(|_| style.color)
                .and_then(|background| shade_code(background, style.depth, lift))
        };
        Theme {
            shade: tone(Lift::Raised),
            fill: tone(Lift::Selected),
            style,
            background,
            aspect: super::logo::ASPECT,
        }
    }

    /// The selected row of a list: on the `selected` fill to a cell past its end, as the app fills a selected row.
    pub fn selected(&self, text: &str) -> String {
        match &self.fill {
            Some(open) => format!("{open}{text} \x1b[49m"),
            None => text.to_string(),
        }
    }

    /// Inline code: the text on the `selected` fill (its width unchanged, so wrapping holds); in the accent without one.
    pub fn chip(&self, text: &str) -> String {
        match &self.fill {
            Some(open) => format!("{open}{}\x1b[49m", self.style.accent(text)),
            None => self.style.accent(text),
        }
    }

    /// How the Nexus mark prints on this terminal.
    pub fn paint(&self) -> Paint {
        Paint {
            background: self.background,
            depth: self.style.color.then_some(self.style.depth),
            aspect: self.aspect,
        }
    }

    /// `line` padded to `width` on the shade, or as it is without one.
    fn shaded(&self, line: &str, width: usize) -> String {
        match &self.shade {
            Some(open) => {
                let padding = width.saturating_sub(display_width(line));
                format!("{open}{line}{}\x1b[49m", " ".repeat(padding))
            }
            None => line.to_string(),
        }
    }

    fn glyph(&self, unicode: &'static str, ascii: &'static str) -> &'static str {
        if self.style.unicode { unicode } else { ascii }
    }
}

/// How far a tone steps off the terminal's background: the brand's `raised` (the composer) and `selected` (a filled
/// chip), as its app steps `main` #18181A to #212123 and #2A2A2C. A terminal band has no hairline round it, so it
/// steps a little further than the app's.
#[derive(Debug, Clone, Copy)]
enum Lift {
    Raised,
    Selected,
}

/// A tone's escape code: the background lifted toward white on a dark terminal, toward black on a light one. 16
/// colours have no tone.
fn shade_code(background: (u8, u8, u8), depth: Depth, lift: Lift) -> Option<String> {
    let (r, g, b) = background;
    let light = is_light(background);
    let (top, alpha) = match (light, lift) {
        (true, Lift::Raised) => (0.0, 0.04),
        (true, Lift::Selected) => (0.0, 0.07),
        (false, Lift::Raised) => (255.0, 0.065),
        (false, Lift::Selected) => (255.0, 0.1),
    };
    let blend = |channel: u8| (top * alpha + f64::from(channel) * (1.0 - alpha)).round() as u8;
    let (r, g, b) = (blend(r), blend(g), blend(b));
    match depth {
        Depth::True => Some(format!("\x1b[48;2;{r};{g};{b}m")),
        Depth::C256 => Some(format!("\x1b[48;5;{}m", ansi256(&format!("{r:02x}{g:02x}{b:02x}")))),
        Depth::C16 => None,
    }
}

/// How many terminal cells the text takes, without its escape codes.
pub fn display_width(text: &str) -> usize {
    strip_ansi(text).width()
}

/// `text` wrapped at `width` cells: each line with the byte offset in `text` where it starts. Words wrap whole; a word
/// longer than a line is cut. A line's leading spaces are kept (code stays indented), the spaces a wrap falls on are
/// dropped. Lines before the last never change when text is added at the end, so an answer can be shown as it streams.
pub fn wrap(text: &str, width: usize) -> Vec<(usize, String)> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut offset = 0;
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut start = offset;
        let mut used = 0;
        let mut wrapped = false;
        for (at, token) in tokens(paragraph) {
            let at = offset + at;
            let token_width = token.width();
            if token.starts_with(' ') {
                if wrapped && line.is_empty() {
                    start = at + token.len();
                } else if used + token_width <= width {
                    line.push_str(token);
                    used += token_width;
                } else {
                    lines.push((start, line.trim_end().to_string()));
                    (line, used, wrapped, start) = (String::new(), 0, true, at + token.len());
                }
                continue;
            }
            // Even a word longer than a line starts a new one, so a word still streaming never moves the line above.
            if used + token_width > width && !line.trim_end().is_empty() {
                lines.push((start, line.trim_end().to_string()));
                (line, used, wrapped, start) = (String::new(), 0, true, at);
            }
            for (index, c) in token.char_indices() {
                let char_width = c.width().unwrap_or(0);
                if used + char_width > width && used > 0 {
                    lines.push((start, std::mem::take(&mut line)));
                    (used, wrapped, start) = (0, true, at + index);
                }
                line.push(c);
                used += char_width;
            }
        }
        lines.push((start, line.trim_end().to_string()));
        offset += paragraph.len() + 1;
    }
    lines
}

/// Runs of spaces and runs of anything else, each with its byte offset.
fn tokens(text: &str) -> Vec<(usize, &str)> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut space = None;
    for (at, c) in text.char_indices() {
        let is_space = c == ' ';
        if space.is_some_and(|was| was != is_space) {
            tokens.push((start, &text[start..at]));
            start = at;
        }
        space = Some(is_space);
    }
    if start < text.len() {
        tokens.push((start, &text[start..]));
    }
    tokens
}

/// The session's first lines: `Nexus  v…`, then the organization and the person. The product is named in the strong
/// ink, its version faint; no glyph stands in for the mark (the mark is drawn above the composer).
pub fn header(theme: &Theme, version: &str, identity: &[Option<String>]) -> Vec<String> {
    let style = &theme.style;
    let who: Vec<&str> = identity
        .iter()
        .flatten()
        .map(String::as_str)
        .filter(|part| !part.is_empty())
        .collect();
    let mut lines = vec![
        String::new(),
        format!("  {}  {}", style.accent("Nexus"), style.faint(&format!("v{version}"))),
    ];
    if !who.is_empty() {
        lines.push(format!("  {}", style.dim(&who.join(&format!(" {} ", style.g.bullet)))));
    }
    lines
}

/// A dim line under the header: what to know before the first prompt. A `command` in it shows as inline code.
pub fn tip(theme: &Theme, text: &str) -> Vec<String> {
    let line = if theme.style.color {
        text.split('`')
            .enumerate()
            .map(|(index, part)| {
                if index % 2 == 1 {
                    theme.chip(part)
                } else {
                    theme.style.dim(part)
                }
            })
            .collect()
    } else {
        text.to_string()
    };
    vec![String::new(), format!("  {line}")]
}

/// A prompt as it was sent: on the shade, after `›`, with a shaded line above and below.
pub fn prompt_cell(theme: &Theme, prompt: &str, width: usize) -> Vec<String> {
    let style = &theme.style;
    let marker = style.dim(&style.bold(&format!("{} ", theme.glyph("›", ">"))));
    let mut lines = vec![String::new(), theme.shaded("", width)];
    for (index, (_, line)) in wrap(&safe_text(prompt), width.saturating_sub(3))
        .into_iter()
        .enumerate()
    {
        let prefix = if index == 0 { marker.clone() } else { "  ".into() };
        lines.push(theme.shaded(&format!("{prefix}{line}"), width));
    }
    lines.push(theme.shaded("", width));
    lines
}

/// Answer lines after `•` (the first) or under it.
pub fn answer_line(theme: &Theme, line: &str, first: bool) -> String {
    if first {
        format!("{}{line}", theme.style.dim(&format!("{} ", theme.glyph("•", "*"))))
    } else {
        format!("  {line}")
    }
}

/// A dim line under an answer: the model that answered, what it cost, and how long it took.
pub fn answer_meta(theme: &Theme, parts: &[Option<String>]) -> String {
    let parts: Vec<&str> = parts
        .iter()
        .flatten()
        .map(String::as_str)
        .filter(|part| !part.is_empty())
        .collect();
    format!(
        "  {}",
        theme.style.dim(&parts.join(&format!(" {} ", theme.style.g.bullet)))
    )
}

/// `• text`: what a slash command did.
pub fn notice(theme: &Theme, text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for (index, (_, line)) in wrap(text, width.saturating_sub(3)).into_iter().enumerate() {
        let prefix = if index == 0 {
            theme.style.dim(&format!("{} ", theme.glyph("•", "*")))
        } else {
            "  ".into()
        };
        lines.push(format!("{prefix}{line}"));
    }
    lines
}

/// `■ Stopped` and the like: red mark, plain text.
pub fn stop_cell(theme: &Theme, text: &str) -> Vec<String> {
    vec![
        String::new(),
        format!("{} {text}", theme.style.red(theme.glyph("■", "#"))),
    ]
}

/// An error as `■ message`, and its hint after `→`.
pub fn error_cell(theme: &Theme, error: &CliError, width: usize) -> Vec<String> {
    let style = &theme.style;
    let mut lines = vec![String::new()];
    for (index, (_, line)) in wrap(&safe_text(&error.message), width.saturating_sub(3))
        .into_iter()
        .enumerate()
    {
        let prefix = if index == 0 {
            style.red(theme.glyph("■", "#"))
        } else {
            " ".into()
        };
        lines.push(format!("{prefix} {line}"));
    }
    if let Some(hint) = &error.hint {
        for (index, (_, line)) in wrap(&safe_text(hint), width.saturating_sub(5)).into_iter().enumerate() {
            let prefix = if index == 0 { style.arrow.clone() } else { " ".into() };
            lines.push(format!("  {prefix} {}", style.dim(&line)));
        }
    }
    lines
}

/// `⣾ Thinking… 3s · esc to interrupt`: the braille spinner (100 ms a frame) and the status words with a light band
/// sweeping them every 2.4 s, `text-3` to the strong ink (brand v5.0's `motion-shimmer`); in bold where truecolor and
/// 256 colours are not to be had.
pub fn status_line(theme: &Theme, word: &str, elapsed_ms: u64) -> String {
    const FRAMES: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
    const ASCII: [&str; 4] = ["-", "\\", "|", "/"];
    let style = &theme.style;
    let frame = (elapsed_ms / 100) as usize;
    let spinner = if style.unicode {
        FRAMES[frame % FRAMES.len()]
    } else {
        ASCII[frame % ASCII.len()]
    };
    format!(
        "{} {} {}",
        style.faint(spinner),
        shimmer(style, word, elapsed_ms),
        style.faint(&format!("{} {} esc to interrupt", elapsed(elapsed_ms), style.g.bullet))
    )
}

/// The words with a band of light passing over them: a 30 % wide band that crosses the line, then rests off it.
fn shimmer(style: &Style, word: &str, elapsed_ms: u64) -> String {
    if !style.color {
        return word.to_string();
    }
    let chars: Vec<char> = word.chars().collect();
    let count = chars.len().max(1) as f64;
    let band = (count * 0.3).max(2.0);
    // The band's centre runs from before the first letter to past the last.
    let phase = (elapsed_ms % 2400) as f64 / 2400.0;
    let centre = -band + phase * (count + band * 2.0);
    let (base, peak) = if style.light {
        ((0x6e, 0x6e, 0x73), (0x1c, 0x1c, 0x1e))
    } else {
        ((0x8a, 0x8a, 0x8e), (0xf5, 0xf5, 0xf7))
    };
    chars
        .iter()
        .enumerate()
        .map(|(index, c)| {
            let distance = (index as f64 - centre).abs() / band;
            let light = (1.0 - distance).clamp(0.0, 1.0);
            let light = light * light * (3.0 - 2.0 * light);
            match style.depth {
                Depth::C16 if light > 0.5 => style.bold(&c.to_string()),
                Depth::C16 => style.dim(&c.to_string()),
                _ => {
                    let mix = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * light).round() as u8;
                    let hex = format!(
                        "{:02x}{:02x}{:02x}",
                        mix(base.0, peak.0),
                        mix(base.1, peak.1),
                        mix(base.2, peak.2)
                    );
                    style.rgb(&hex, crate::style::Tone::Dim, &c.to_string())
                }
            }
        })
        .collect()
}

/// `4s`, `1m 05s`, `1h 02m 05s`.
pub fn elapsed(ms: u64) -> String {
    let seconds = ms / 1000;
    match (seconds / 3600, seconds / 60 % 60, seconds % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s:02}s"),
        (h, m, s) => format!("{h}h {m:02}m {s:02}s"),
    }
}

/// At most this many composer rows show; the rest scroll with the cursor.
const COMPOSER_ROWS: usize = 8;

/// The composer on the shade, `›` before its first row: its lines, and the cursor's row and column in them.
pub fn composer_lines(
    theme: &Theme,
    composer: &Composer,
    placeholder: &str,
    shell: bool,
    width: usize,
) -> (Vec<String>, (usize, usize)) {
    let style = &theme.style;
    // Shell mode (`!`) shows its own marker, as Claude Code's bash mode does.
    let marker = if shell {
        style.yellow(&style.bold("! "))
    } else {
        style.bold(&format!("{} ", theme.glyph("›", ">")))
    };
    let inner = width.saturating_sub(3).max(1);
    let (rows, (row, column)) = if composer.is_empty() {
        (vec![style.dim(placeholder)], (0, 0))
    } else {
        layout(composer.text(), composer.cursor(), inner)
    };
    let first = (row + 1).saturating_sub(COMPOSER_ROWS);
    let mut lines = vec![theme.shaded("", width)];
    for (index, text) in rows.iter().enumerate().skip(first).take(COMPOSER_ROWS) {
        let prefix = if index == 0 { marker.clone() } else { "  ".into() };
        lines.push(theme.shaded(&format!("{prefix}{text}"), width));
    }
    lines.push(theme.shaded("", width));
    (lines, (row - first + 1, column + 2))
}

/// The composer's text cut into rows of `width` cells (by character, so the cursor maps exactly), and the cursor's
/// row and column. Control characters show as `safe_text` writes them.
fn layout(text: &str, cursor: usize, width: usize) -> (Vec<String>, (usize, usize)) {
    let mut rows = vec![String::new()];
    let mut used = 0;
    let mut at_cursor = None;
    for (index, c) in text.char_indices() {
        if index == cursor {
            at_cursor = Some((rows.len() - 1, used));
        }
        if c == '\n' {
            rows.push(String::new());
            used = 0;
            continue;
        }
        let shown = if c.is_control() {
            safe_text(&c.to_string())
        } else {
            c.to_string()
        };
        let shown_width = shown.width();
        if used + shown_width > width {
            rows.push(String::new());
            used = 0;
        }
        if let Some(row) = rows.last_mut() {
            row.push_str(&shown);
        }
        used += shown_width;
    }
    let mut cursor_at = at_cursor.unwrap_or((rows.len() - 1, used));
    // A cursor past a full row starts the next one.
    if cursor_at.1 >= width {
        rows.push(String::new());
        cursor_at = (rows.len() - 1, 0);
    }
    (rows, cursor_at)
}

/// The footer's left side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    /// `? for shortcuts`.
    Shortcuts,
    /// `⌃c again to quit`.
    Quit,
    /// `tab to use`: a suggested prompt waits in the composer.
    Suggestion,
    /// Shell mode: what `!` does, and how to leave it.
    Shell,
    /// Nothing: the person is typing.
    None,
}

/// The line under the composer: a hint on the left, the model and the budget on the right.
pub fn footer(theme: &Theme, hint: Hint, mode: Option<&str>, right: &str, width: usize) -> String {
    // `← chats` shows when it fits beside the rest; `? for shortcuts` lists it too.
    let wide = footer_with(theme, hint, mode, right, width, true);
    if display_width(&wide) <= width.saturating_sub(1) && wide.contains("chats") {
        return wide;
    }
    footer_with(theme, hint, mode, right, width, false)
}

fn footer_with(theme: &Theme, hint: Hint, mode: Option<&str>, right: &str, width: usize, chats: bool) -> String {
    let style = &theme.style;
    let left = match hint {
        Hint::Shortcuts if chats => format!(
            "  {}",
            style.dim(&format!("? for shortcuts · {} chats", theme.glyph("←", "left")))
        ),
        Hint::Shortcuts => format!("  {}", style.dim("? for shortcuts")),
        Hint::Quit => format!("  {} {}", theme.glyph("⌃c", "ctrl+c"), style.dim("again to quit")),
        Hint::Suggestion => format!(
            "  {} {}",
            theme.glyph("tab →", "tab"),
            style.dim("to use the suggestion")
        ),
        Hint::Shell => format!("  {} {}", style.yellow("! shell"), style.dim("· esc to leave")),
        Hint::None => String::new(),
    };
    // A mode other than chat shows after the hint, as Codex shows its plan mode.
    let left = match mode {
        Some(mode) if hint != Hint::Quit => {
            let joint = if left.is_empty() {
                "  ".to_string()
            } else {
                format!("{} ", style.dim(" ·"))
            };
            let cycle = style.dim(&format!(" ({} to cycle)", theme.glyph("⇧tab", "shift+tab")));
            format!("{left}{joint}{}{cycle}", style.accent(mode))
        }
        _ => left,
    };
    let right = style.dim(right);
    let (left_width, right_width) = (display_width(&left), display_width(&right));
    if left_width + right_width + 4 > width {
        // What the person must know now (quitting, shell mode) wins over the model and the budget.
        if matches!(hint, Hint::Quit | Hint::Shell) || right_width + 4 > width {
            return left;
        }
        // So does a mode (agent mode, auto): the model and the budget are cut short instead.
        let room = width.saturating_sub(left_width + 4);
        if mode.is_some() && room >= 12 {
            let right = crate::style::truncate(&right, room, style.g.ellipsis);
            let padding = width.saturating_sub(left_width + display_width(&right) + 2);
            return format!("{left}{}{right}", " ".repeat(padding));
        }
        return format!("  {right}");
    }
    format!("{left}{}{right}", " ".repeat(width - left_width - right_width - 2))
}

/// `?`: the session's keys, shown under the composer.
pub fn shortcuts(theme: &Theme) -> Vec<String> {
    let style = &theme.style;
    let keys: [(&str, &str, &str); 12] = [
        ("⏎", "enter", "send"),
        ("⇧⏎  ⌃j", "shift+enter  ctrl+j", "new line"),
        ("↑ ↓", "up down", "earlier prompts"),
        ("esc", "esc", "stop the answer"),
        (
            "⌃u ⌃k ⌃w",
            "ctrl+u ctrl+k ctrl+w",
            "delete to line start, to line end, a word",
        ),
        ("⌃c", "ctrl+c", "clear, stop, or quit"),
        ("⇧tab", "shift+tab", "chat or agent"),
        ("←", "left", "switch to another chat (empty composer)"),
        ("@", "@", "mention a file"),
        ("⌃v", "ctrl+v", "paste an image"),
        ("!", "!", "run a command in this folder"),
        ("? esc", "? esc", "close this"),
    ];
    let shown: Vec<(&str, &str)> = keys
        .iter()
        .map(|(unicode, ascii, action)| (theme.glyph(unicode, ascii), *action))
        .collect();
    let column = shown.iter().map(|(key, _)| key.width()).max().unwrap_or(0) + 3;
    let mut lines = vec![format!("  {}", style.bold("Keyboard shortcuts")), String::new()];
    lines.extend(
        shown
            .iter()
            .map(|(key, action)| format!("  {key}{}{}", " ".repeat(column - key.width()), style.dim(action))),
    );
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Theme {
        Theme::new(Style::new(false), None)
    }

    fn texts(lines: &[(usize, String)]) -> Vec<&str> {
        lines.iter().map(|(_, line)| line.as_str()).collect()
    }

    #[test]
    fn wraps_words_and_keeps_offsets() {
        let text = "the quick brown fox\n  indented line";
        let lines = wrap(text, 10);
        assert_eq!(texts(&lines), ["the quick", "brown fox", "  indented", "line"]);
        for (start, line) in &lines {
            assert!(text[*start..].starts_with(line.as_str()), "{line:?} starts at {start}");
        }
    }

    #[test]
    fn cuts_a_word_longer_than_a_line_and_counts_wide_characters() {
        assert_eq!(texts(&wrap("abcdefghij", 4)), ["abcd", "efgh", "ij"]);
        assert_eq!(texts(&wrap("日本語です", 4)), ["日本", "語で", "す"]);
    }

    #[test]
    fn lines_before_the_last_do_not_change_as_text_streams() {
        let full = "Nexus routes every model call through https://nexus.example/v1/chat/completions, with budgets.";
        for end in 1..full.len() {
            let early = wrap(&full[..end], 16);
            let late = wrap(full, 16);
            assert_eq!(early[..early.len() - 1], late[..early.len() - 1], "at {end}");
        }
    }

    #[test]
    fn header_and_cells_read_like_codex() {
        let theme = plain();
        let header = header(
            &theme,
            "0.30.1",
            &[Some("nexus-xyz".into()), Some("ana@example.com".into())],
        );
        assert_eq!(header, ["", "  Nexus  v0.30.1", "  nexus-xyz · ana@example.com"]);
        assert_eq!(prompt_cell(&theme, "hi there", 40), ["", "", "› hi there", ""]);
        assert_eq!(answer_line(&theme, "Hello", true), "• Hello");
        assert_eq!(answer_line(&theme, "again", false), "  again");
        assert_eq!(stop_cell(&theme, "Stopped"), ["", "■ Stopped"]);
    }

    #[test]
    fn status_line_counts_time() {
        let theme = plain();
        assert_eq!(
            status_line(&theme, "Thinking…", 3_400),
            "⣻ Thinking… 3s · esc to interrupt"
        );
        assert_eq!(elapsed(65_000), "1m 05s");
        assert_eq!(elapsed(3_725_000), "1h 02m 05s");
    }

    #[test]
    fn composer_places_the_cursor() {
        let theme = plain();
        let mut composer = Composer::default();
        let (lines, cursor) = composer_lines(&theme, &composer, "Ask anything", false, 40);
        assert_eq!(lines, ["", "› Ask anything", ""]);
        assert_eq!(cursor, (1, 2));
        composer.insert("abcdefgh\nxy");
        let (lines, cursor) = composer_lines(&theme, &composer, "", false, 8);
        assert_eq!(lines, ["", "› abcde", "  fgh", "  xy", ""]);
        assert_eq!(cursor, (3, 4));
    }

    #[test]
    fn footer_puts_the_model_on_the_right() {
        let theme = plain();
        let line = footer(&theme, Hint::Shortcuts, None, "nexus/auto · $4.00 left", 50);
        assert_eq!(line, "  ? for shortcuts        nexus/auto · $4.00 left");
        assert_eq!(display_width(&line), 48);
        // With room, the chats key shows too.
        let wide = footer(&theme, Hint::Shortcuts, None, "nexus/auto · $4.00 left", 70);
        assert!(wide.starts_with("  ? for shortcuts · ← chats  "), "{wide}");
        assert_eq!(
            footer(&theme, Hint::Quit, None, "a very long right side", 20),
            "  ⌃c again to quit"
        );
    }

    #[test]
    fn shade_lifts_a_dark_background_and_needs_colour() {
        let dark = Theme::new(Style::with(true, Depth::True, true, None), Some((0, 0, 0)));
        assert_eq!(dark.shaded("x", 3), "\x1b[48;2;17;17;17mx  \x1b[49m");
        assert_eq!(
            dark.chip("x"),
            "\x1b[48;2;26;26;26m\x1b[38;2;245;245;247mx\x1b[39m\x1b[49m"
        );
        let sixteen = Theme::new(Style::with(true, Depth::C16, true, None), Some((0, 0, 0)));
        assert_eq!(sixteen.shaded("x", 3), "x");
    }
}
