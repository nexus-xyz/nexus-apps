//! What an opened tool call shows, as the Nexus app's timeline shows it (the Nexus web app):
//! its output cut to four lines with the whole one click away, an edit as the lines it takes out and puts in, and long
//! text cut to 400 lines. The same rule for the apps written in Rust, with the TypeScript tests ported. Pure.

/// How many lines an opened call's output shows before "Show all N lines".
pub const PREVIEW_LINES: usize = 4;

/// The most lines a written file or an input shows ([`clip`]).
const MAX_LINES: usize = 400;

/// An opened call's output, cut (`previewOutput`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPreview {
    /// Its first two lines (all of it when it fits).
    pub head: String,
    /// Its last two lines: a test run's or a build's verdict is at its end.
    pub tail: String,
    /// How many lines are left out between them; 0 when it fits.
    pub more: usize,
    pub total: usize,
}

/// `text` cut to four lines: its first two and its last two, so the verdict at the end shows. Trailing newlines do
/// not count as lines.
pub fn preview_output(text: &str) -> OutputPreview {
    let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
    let total = lines.len();
    if total <= PREVIEW_LINES {
        return OutputPreview {
            head: lines.join("\n"),
            tail: String::new(),
            more: 0,
            total,
        };
    }
    let half = PREVIEW_LINES / 2;
    OutputPreview {
        head: lines[..half].join("\n"),
        tail: lines[total - half..].join("\n"),
        more: total - PREVIEW_LINES,
        total,
    }
}

/// "Show all 2,400 lines".
pub fn show_all_label(total: usize) -> String {
    format!("Show all {} lines", thousands(total))
}

/// "… 2,396 more lines".
pub fn more_lines_label(more: usize) -> String {
    format!("… {} more lines", thousands(more))
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A long text cut to its first 400 lines, saying how many more there were (`clip`).
pub fn clip(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() <= MAX_LINES {
        return text.to_string();
    }
    format!(
        "{}\n… {} more lines",
        lines[..MAX_LINES].join("\n"),
        lines.len() - MAX_LINES
    )
}

/// One line of an edit: kept as context, removed or added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    Same,
    Removed,
    Added,
}

/// An edit's lines (`diffRows`): those both sides share at the start and the end, once, as context; the others
/// removed, then added. The agent's own strings, not a computed diff.
pub fn diff_rows(before: &str, after: &str) -> Vec<(Sign, String)> {
    let removed: Vec<&str> = if before.is_empty() {
        Vec::new()
    } else {
        before.split('\n').collect()
    };
    let added: Vec<&str> = if after.is_empty() {
        Vec::new()
    } else {
        after.split('\n').collect()
    };
    let mut head = 0;
    while head < removed.len() && head < added.len() && removed[head] == added[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < removed.len() - head
        && tail < added.len() - head
        && removed[removed.len() - 1 - tail] == added[added.len() - 1 - tail]
    {
        tail += 1;
    }
    let row = |sign: Sign| move |text: &&str| (sign, text.to_string());
    removed[..head]
        .iter()
        .map(row(Sign::Same))
        .chain(removed[head..removed.len() - tail].iter().map(row(Sign::Removed)))
        .chain(added[head..added.len() - tail].iter().map(row(Sign::Added)))
        .chain(removed[removed.len() - tail..].iter().map(row(Sign::Same)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn shows_short_output_whole() {
        assert_eq!(
            preview_output("a\nb\nc\nd\n"),
            OutputPreview {
                head: "a\nb\nc\nd".into(),
                tail: String::new(),
                more: 0,
                total: 4
            }
        );
    }

    #[test]
    fn cuts_long_output_to_four_lines_keeping_its_end() {
        assert_eq!(
            preview_output(&format!("{}\n64 passed in 18 s", lines(213))),
            OutputPreview {
                head: "line 1\nline 2".into(),
                tail: "line 213\n64 passed in 18 s".into(),
                more: 210,
                total: 214
            }
        );
    }

    #[test]
    fn says_how_many_lines_it_left_out_and_offers_them_all() {
        let preview = preview_output(&lines(2400));
        assert_eq!(more_lines_label(preview.more), "… 2,396 more lines");
        assert_eq!(show_all_label(preview.total), "Show all 2,400 lines");
        assert_eq!(show_all_label(12), "Show all 12 lines");
    }

    #[test]
    fn an_edit_shows_its_shared_lines_once_then_what_it_removes_and_adds() {
        let rows = diff_rows("fn a() {\n    old();\n}", "fn a() {\n    new();\n    more();\n}");
        assert_eq!(
            rows,
            vec![
                (Sign::Same, "fn a() {".to_string()),
                (Sign::Removed, "    old();".to_string()),
                (Sign::Added, "    new();".to_string()),
                (Sign::Added, "    more();".to_string()),
                (Sign::Same, "}".to_string()),
            ]
        );
        assert!(diff_rows("", "").is_empty());
        assert_eq!(diff_rows("", "x"), vec![(Sign::Added, "x".to_string())]);
    }

    #[test]
    fn clips_long_text_to_four_hundred_lines() {
        assert_eq!(clip("a\nb"), "a\nb");
        let clipped = clip(&lines(410));
        assert!(clipped.ends_with("line 400\n… 10 more lines"));
    }
}
