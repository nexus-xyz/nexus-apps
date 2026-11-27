//! Markdown in answers, drawn for a terminal as Codex draws it: headings in bold, list items with `•`, quotes behind a
//! bar, code blocks set apart, and **bold**, *italic*, `code` and [links](url) inline. It goes a line at a time, with
//! the state a line needs from the lines before (inside a code block or not), so an answer can be drawn as it streams:
//! lines that are complete are final, the one still arriving is drawn again as it grows.
//!
//! Tables and HTML pass through as text.

use unicode_width::UnicodeWidthChar as _;

use super::view::Theme;

/// What a line needs to know about the lines before it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Markdown {
    /// Inside a fenced code block: the fence that closes it (```` ``` ```` or `~~~`).
    fence: Option<String>,
}

/// How a run of text looks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Look {
    bold: bool,
    italic: bool,
    code: bool,
    link: bool,
    dim: bool,
}

impl Markdown {
    /// One source line (without its newline) as the terminal lines it takes at `width` cells.
    pub fn line(&mut self, theme: &Theme, source: &str, width: usize) -> Vec<String> {
        let style = &theme.style;
        let trimmed = source.trim_start();
        // A fence opens or closes a code block; the fence line itself shows only its language, if any.
        if let Some(fence) = fence_of(trimmed) {
            match &self.fence {
                Some(open)
                    if trimmed.starts_with(open.as_str()) && trimmed.trim_end() == trimmed[..open.len()].trim_end() =>
                {
                    self.fence = None;
                    return Vec::new();
                }
                Some(_) => {}
                None => {
                    self.fence = Some(fence.to_string());
                    let language = trimmed[fence.len()..].trim();
                    return if language.is_empty() {
                        Vec::new()
                    } else {
                        vec![style.dim(language)]
                    };
                }
            }
        }
        if self.fence.is_some() {
            let bar = style.faint(if style.unicode { "▏ " } else { "| " });
            return hard_wrap(source, width.saturating_sub(2))
                .into_iter()
                .map(|piece| format!("{bar}{piece}"))
                .collect();
        }
        if trimmed.is_empty() {
            return vec![String::new()];
        }
        // A rule.
        let compact: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.len() >= 3
            && (compact.chars().all(|c| c == '-')
                || compact.chars().all(|c| c == '*')
                || compact.chars().all(|c| c == '_'))
        {
            let rule = if style.unicode { "─" } else { "-" };
            return vec![style.faint(&rule.repeat(width.min(40)))];
        }
        // A heading: its text in bold, without the #s.
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            let text = trimmed[hashes..].trim().trim_end_matches('#').trim_end();
            let base = Look {
                bold: true,
                ..Look::default()
            };
            return wrap_spans(theme, &inline(text, base), width, "", "");
        }
        // A quote: behind a bar, dim.
        if let Some(rest) = trimmed.strip_prefix('>') {
            let bar = format!("{} ", style.faint(if style.unicode { "▌" } else { "|" }));
            let base = Look {
                dim: true,
                ..Look::default()
            };
            return wrap_spans(theme, &inline(rest.trim_start(), base), width, &bar, &bar);
        }
        // A list item: its marker, then its text with a hanging indent.
        let indent = &source[..source.len() - trimmed.len()];
        let indent = " ".repeat(indent.chars().count().min(12));
        if let Some((marker, rest)) = list_item(trimmed) {
            let first = format!("{indent}{marker} ");
            let hang = " ".repeat(first.chars().count());
            let shown_marker = if marker == "•" {
                style.dim(&first)
            } else {
                first.clone()
            };
            return wrap_spans(theme, &inline(rest, Look::default()), width, &shown_marker, &hang);
        }
        wrap_spans(theme, &inline(trimmed, Look::default()), width, &indent, &indent)
    }
}

/// ```` ``` ```` or `~~~` (three or more) at the start of a line.
fn fence_of(line: &str) -> Option<&str> {
    for mark in ['`', '~'] {
        let count = line.chars().take_while(|c| *c == mark).count();
        if count >= 3 {
            return Some(&line[..count]);
        }
    }
    None
}

/// `- item`, `* item`, `+ item` (as `•`), and `1. item` or `1) item` (kept).
fn list_item(line: &str) -> Option<(String, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            // A task list's box shows as it is.
            return Some(("•".into(), rest));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if (1..=3).contains(&digits) {
        let rest = &line[digits..];
        for close in [". ", ") "] {
            if let Some(after) = rest.strip_prefix(close) {
                return Some((format!("{}{}", &line[..digits], close.trim_end()), after));
            }
        }
    }
    None
}

/// The text's inline marks as runs: `**bold**`, `*italic*` / `_italic_`, `` `code` ``, `[text](url)` (the url after
/// it, dim). An unclosed mark is text.
fn inline(text: &str, base: Look) -> Vec<(String, Look)> {
    let chars: Vec<char> = text.chars().collect();
    let mut runs: Vec<(String, Look)> = Vec::new();
    let mut look = base;
    let mut current = String::new();
    let push = |runs: &mut Vec<(String, Look)>, current: &mut String, look: Look| {
        if !current.is_empty() {
            runs.push((std::mem::take(current), look));
        }
    };
    let closes = |from: usize, mark: &[char]| -> bool { (from..chars.len()).any(|at| chars[at..].starts_with(mark)) };
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if c == '\\' && at + 1 < chars.len() && "\\`*_[]()#>-".contains(chars[at + 1]) {
            current.push(chars[at + 1]);
            at += 2;
            continue;
        }
        if c == '`'
            && let Some(end) = (at + 1..chars.len()).find(|&end| chars[end] == '`')
        {
            push(&mut runs, &mut current, look);
            let code: String = chars[at + 1..end].iter().collect();
            runs.push((code, Look { code: true, ..look }));
            at = end + 1;
            continue;
        }
        if c == '[' {
            let close = (at + 1..chars.len()).find(|&end| chars[end] == ']');
            if let Some(close) = close
                && chars.get(close + 1) == Some(&'(')
                && let Some(paren) = (close + 2..chars.len()).find(|&end| chars[end] == ')')
            {
                push(&mut runs, &mut current, look);
                let label: String = chars[at + 1..close].iter().collect();
                let url: String = chars[close + 2..paren].iter().collect();
                runs.push((label.clone(), Look { link: true, ..look }));
                if url != label {
                    runs.push((format!(" ({url})"), Look { dim: true, ..look }));
                }
                at = paren + 1;
                continue;
            }
        }
        let double = at + 1 < chars.len() && chars[at + 1] == c && (c == '*' || c == '_');
        if double && (look.bold || closes(at + 2, &[c, c])) {
            push(&mut runs, &mut current, look);
            look.bold = !look.bold;
            at += 2;
            continue;
        }
        // A single `*` or `_` marks italic only where it starts or ends a word (not snake_case, not 2 * 3).
        let before = at.checked_sub(1).map(|b| chars[b]);
        let after = chars.get(at + 1).copied();
        let opens =
            !look.italic && after.is_some_and(|a| !a.is_whitespace()) && before.is_none_or(|b| !b.is_alphanumeric());
        let ends =
            look.italic && before.is_some_and(|b| !b.is_whitespace()) && after.is_none_or(|a| !a.is_alphanumeric());
        if (c == '*' || c == '_') && ((opens && closes(at + 1, &[c])) || ends) {
            push(&mut runs, &mut current, look);
            look.italic = !look.italic;
            at += 1;
            continue;
        }
        current.push(c);
        at += 1;
    }
    push(&mut runs, &mut current, look);
    runs
}

/// Runs wrapped at `width` cells, word by word, `first` before the first line and `rest` before the others (both
/// already styled; their width counts).
fn wrap_spans(theme: &Theme, runs: &[(String, Look)], width: usize, first: &str, rest: &str) -> Vec<String> {
    let prefix_width = |prefix: &str| super::view::display_width(prefix);
    let mut lines: Vec<Vec<(String, Look)>> = vec![Vec::new()];
    let mut used = prefix_width(first);
    let limit = |line: usize| width.max(prefix_width(if line == 0 { first } else { rest }) + 1);
    for (text, look) in runs {
        for word in split_keeping_spaces(text) {
            let word_width: usize = word.chars().map(|c| c.width().unwrap_or(0)).sum();
            let line = lines.len() - 1;
            let is_space = word.starts_with(' ');
            if used + word_width > limit(line) && used > prefix_width(if line == 0 { first } else { rest }) {
                if is_space {
                    continue;
                }
                lines.push(Vec::new());
                used = prefix_width(rest);
            }
            // A word longer than a whole line is cut.
            let mut piece = String::new();
            for c in word.chars() {
                let w = c.width().unwrap_or(0);
                if used + w > limit(lines.len() - 1) && used > prefix_width(rest) {
                    if let Some(last) = lines.last_mut() {
                        last.push((std::mem::take(&mut piece), *look));
                    }
                    lines.push(Vec::new());
                    used = prefix_width(rest);
                }
                piece.push(c);
                used += w;
            }
            if let Some(last) = lines.last_mut() {
                last.push((piece, *look));
            }
        }
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(index, runs)| {
            let mut line = String::from(if index == 0 { first } else { rest });
            for (text, look) in &runs {
                line.push_str(&paint(theme, text, *look));
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// Runs of spaces and runs of anything else.
fn split_keeping_spaces(text: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    for c in text.chars() {
        match parts.last_mut() {
            Some(last) if last.starts_with(' ') == (c == ' ') => last.push(c),
            _ => parts.push(c.to_string()),
        }
    }
    parts
}

fn paint(theme: &Theme, text: &str, look: Look) -> String {
    let style = &theme.style;
    let mut out = text.to_string();
    if look.code {
        out = theme.chip(&out);
    }
    if look.link {
        out = style.underline(&style.accent(&out));
    }
    if look.dim {
        out = style.dim(&out);
    }
    if look.italic {
        out = if style.color {
            format!("\x1b[3m{out}\x1b[23m")
        } else {
            out
        };
    }
    if look.bold {
        out = style.bold(&out);
    }
    out
}

/// Code: cut at `width` cells, never at words.
fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = vec![String::new()];
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width {
            lines.push(String::new());
            used = 0;
        }
        if let Some(line) = lines.last_mut() {
            line.push(c);
        }
        used += w;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;

    fn render(source: &str, width: usize) -> Vec<String> {
        let theme = Theme::new(Style::new(false), None);
        let mut markdown = Markdown::default();
        source
            .lines()
            .flat_map(|line| markdown.line(&theme, line, width))
            .collect()
    }

    #[test]
    fn headings_lists_and_quotes() {
        let shown = render("# Budgets\n- one key for every model\n2. second\n> a quote\n---", 40);
        assert_eq!(
            shown,
            [
                "Budgets",
                "• one key for every model",
                "2. second",
                "▌ a quote",
                "─".repeat(40).as_str()
            ]
        );
    }

    #[test]
    fn inline_marks_go_and_links_keep_their_url() {
        let shown = render(
            "Run **`nexus models`** to see *your* models, see [docs](https://x.dev) or snake_case_name.",
            120,
        );
        assert_eq!(
            shown,
            ["Run nexus models to see your models, see docs (https://x.dev) or snake_case_name."]
        );
    }

    #[test]
    fn code_blocks_keep_their_text() {
        let shown = render("```rust\nfn main() {\n    let x = 2 * 3;\n}\n```\nafter", 40);
        assert_eq!(shown, ["rust", "▏ fn main() {", "▏     let x = 2 * 3;", "▏ }", "after"]);
    }

    #[test]
    fn list_items_wrap_under_their_text() {
        let shown = render("- a list item that is long enough to wrap", 20);
        assert_eq!(shown, ["• a list item that", "  is long enough to", "  wrap"]);
    }

    #[test]
    fn colour_marks_bold_and_code() {
        let theme = Theme::new(Style::new(true), None);
        let line = Markdown::default().line(&theme, "**hi** `x`", 40).concat();
        assert!(
            line.contains("\x1b[1mhi\x1b[22m") && line.contains("\x1b[1mx\x1b[22m"),
            "{line:?}"
        );
    }
}
