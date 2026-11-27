//! Comments on diff lines in the Changes view: lines picked in a file's diff and a comment join
//! the composer's draft as a chip, so several can pile up before one send. On send each becomes plain Markdown the
//! agent reads: the file, the lines, the picked diff in a fence, then the comment, ahead of what was typed. Picking
//! itself is the app's own: GPUI Kit's diff selection in Nexus GPUI.

use serde::{Deserialize, Serialize};

/// One line of a file's diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// `' '` context, `'+'` added, `'-'` removed, `'@'` a hunk's header, `'!'` a note.
    pub sign: char,
    pub text: String,
    /// The line's number in the old file (context and removed lines).
    pub old: Option<usize>,
    /// The line's number in the new file (context and added lines).
    pub new: Option<usize>,
}

impl DiffLine {
    fn new(sign: char, text: &str, old: Option<usize>, new: Option<usize>) -> DiffLine {
        DiffLine {
            sign,
            text: text.to_string(),
            old,
            new,
        }
    }
}

/// Which file's numbers a line position counts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Old,
    New,
}

/// One comment waiting in the draft. `quote` is the picked diff as text, kept so a later reload cannot change it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffComment {
    pub id: String,
    pub path: String,
    /// "line 12", "lines 12-14", "removed line 3".
    pub range: String,
    pub quote: String,
    pub text: String,
}

/// Quoted diff longer than this is cut, saying how many lines were left out (a comment is about a few lines).
pub const QUOTE_MAX: usize = 8000;

/// Each file's lines in a unified diff, in the patch's order (one entry per `diff --git`). Hunk headers, added,
/// removed and context lines, with their numbers; header lines and "\ No newline at end of file" carry nothing.
pub fn parse_lines(patch: &str) -> Vec<Vec<DiffLine>> {
    let mut files: Vec<Vec<DiffLine>> = Vec::new();
    let mut in_hunk = false;
    let (mut old, mut new) = (0, 0);
    for line in patch.split('\n') {
        if line.starts_with("diff --git ") {
            files.push(Vec::new());
            in_hunk = false;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("@@") {
            in_hunk = true;
            let numbers = |marker: char| -> usize {
                rest.split_whitespace()
                    .find_map(|part| part.strip_prefix(marker))
                    .and_then(|range| range.split(',').next()?.parse().ok())
                    .unwrap_or(0)
            };
            (old, new) = (numbers('-'), numbers('+'));
            file.push(DiffLine::new('@', line, None, None));
        } else if !in_hunk {
            if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
                file.push(DiffLine::new('!', "Binary file changed", None, None));
            }
        } else if let Some(text) = line.strip_prefix('+') {
            file.push(DiffLine::new('+', text, None, Some(new)));
            new += 1;
        } else if let Some(text) = line.strip_prefix('-') {
            file.push(DiffLine::new('-', text, Some(old), None));
            old += 1;
        } else if let Some(text) = line.strip_prefix(' ') {
            file.push(DiffLine::new(' ', text, Some(old), Some(new)));
            (old, new) = (old + 1, new + 1);
        }
    }
    files
}

/// The lines of one file's diff from the line numbered `start` on side `from` to the one numbered `end` on side
/// `to`, either order; empty when either is not in the diff.
pub fn lines_between(lines: &[DiffLine], from: Side, start: usize, to: Side, end: usize) -> &[DiffLine] {
    let at = |side: Side, number: usize| {
        lines.iter().position(|line| match side {
            Side::Old => line.old == Some(number),
            Side::New => line.new == Some(number),
        })
    };
    match (at(from, start), at(to, end)) {
        (Some(a), Some(b)) => &lines[a.min(b)..=a.max(b)],
        _ => &[],
    }
}

/// Lines that can be picked: the file's own (context, added, removed), not hunk headers or notes.
pub fn pickable(line: &DiffLine) -> bool {
    matches!(line.sign, ' ' | '+' | '-')
}

/// "line 12", "lines 12-14"; by the new file's numbers, or the old file's when only removed lines are picked.
pub fn range_label(lines: &[DiffLine]) -> String {
    let code: Vec<&DiffLine> = lines.iter().filter(|line| pickable(line)).collect();
    let numbers: Vec<Option<usize>> = if code.iter().any(|line| line.new.is_some()) {
        code.iter().map(|line| line.new).collect()
    } else {
        code.iter().map(|line| line.old).collect()
    };
    let known: Vec<usize> = numbers.into_iter().flatten().collect();
    let (Some(first), Some(last)) = (known.first(), known.last()) else {
        return "lines".into();
    };
    let removed = if code.iter().all(|line| line.sign == '-') {
        "removed "
    } else {
        ""
    };
    if first == last {
        format!("{removed}line {first}")
    } else {
        format!("{removed}lines {first}-{last}")
    }
}

/// A Markdown code fence that the quoted text cannot close early: longer than any run of backticks inside it.
fn fenced(language: &str, body: &str) -> String {
    let longest = body.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(3.max(longest + 1));
    format!("{fence}{language}\n{body}\n{fence}")
}

/// The picked lines as diff text (hunk headers kept as they help place them; notes left out), cut at [`QUOTE_MAX`].
pub fn quoted_diff(lines: &[DiffLine]) -> String {
    let rows: Vec<String> = lines
        .iter()
        .filter(|line| line.sign != '!')
        .map(|line| {
            if line.sign == '@' {
                line.text.clone()
            } else {
                format!("{}{}", line.sign, line.text)
            }
        })
        .collect();
    let mut kept: Vec<String> = Vec::new();
    let mut size = 0;
    for row in &rows {
        // UTF-16 length.
        let length = row.encode_utf16().count();
        if size + length + 1 > QUOTE_MAX && !kept.is_empty() {
            break;
        }
        kept.push(row.clone());
        size += length + 1;
    }
    let left = rows.len() - kept.len();
    if left > 0 {
        kept.push(format!("… {left} more {}", if left == 1 { "line" } else { "lines" }));
    }
    kept.join("\n")
}

/// The comment on the picked `lines` of `path`, or None when the comment is blank or no line of code is picked.
/// `place` follows the lines in its range (`lines 12-14 of commit a1b2c3d`) when they are not the files as they are now.
pub fn make_comment(path: &str, lines: &[DiffLine], text: &str, id: &str, place: Option<&str>) -> Option<DiffComment> {
    let comment = text.replace("\r\n", "\n").replace('\r', "\n");
    let comment = comment.trim();
    if comment.is_empty() || !lines.iter().any(pickable) {
        return None;
    }
    let range = match place {
        Some(place) => format!("{} {place}", range_label(lines)),
        None => range_label(lines),
    };
    Some(DiffComment {
        id: id.to_string(),
        path: path.to_string(),
        range,
        quote: quoted_diff(lines),
        text: comment.to_string(),
    })
}

/// What the agent reads for one comment: "In `src/a.ts`, lines 12-14:", the quoted diff in a fence, then the comment.
pub fn comment_block(comment: &DiffComment) -> String {
    format!(
        "In `{}`, {}:\n\n{}\n\n{}",
        comment.path,
        comment.range,
        fenced("diff", &comment.quote),
        comment.text
    )
}

/// The text sent for a draft: its comments in the order they were added, a blank line apart, then what was typed.
/// Without comments, the typed text exactly as it is.
pub fn prompt_text(comments: &[DiffComment], text: &str) -> String {
    if comments.is_empty() {
        return text.to_string();
    }
    comments
        .iter()
        .map(comment_block)
        .chain(Some(text.trim().to_string()).filter(|text| !text.is_empty()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk() -> DiffLine {
        DiffLine::new('@', "@@ -10,3 +10,3 @@ function load()", None, None)
    }
    fn kept() -> DiffLine {
        DiffLine::new(' ', "const a = 1;", Some(10), Some(10))
    }
    fn removed() -> DiffLine {
        DiffLine::new('-', "const b = 2;", Some(11), None)
    }
    fn added() -> DiffLine {
        DiffLine::new('+', "const b = 3;", None, Some(11))
    }
    fn after() -> DiffLine {
        DiffLine::new(' ', "return a + b;", Some(12), Some(12))
    }
    fn lines() -> Vec<DiffLine> {
        vec![hunk(), kept(), removed(), added(), after()]
    }

    fn comment() -> DiffComment {
        DiffComment {
            id: "c1".into(),
            path: "src/a.ts".into(),
            range: "line 11".into(),
            quote: "-const b = 2;\n+const b = 3;".into(),
            text: "Why 3?".into(),
        }
    }

    #[test]
    fn names_the_file_and_lines_quotes_the_picked_diff_in_a_fence_then_the_comment() {
        let made = make_comment("src/a.ts", &lines()[1..4], "  Why 3 and not 2?  ", "c1", None);
        let expected = DiffComment {
            id: "c1".into(),
            path: "src/a.ts".into(),
            range: "lines 10-11".into(),
            quote: " const a = 1;\n-const b = 2;\n+const b = 3;".into(),
            text: "Why 3 and not 2?".into(),
        };
        assert_eq!(made.as_ref(), Some(&expected));
        assert_eq!(
            comment_block(&expected),
            "In `src/a.ts`, lines 10-11:\n\n```diff\n const a = 1;\n-const b = 2;\n+const b = 3;\n```\n\nWhy 3 and not 2?"
        );
    }

    #[test]
    fn names_the_commit_its_lines_are_from_on_a_commits_diff() {
        let made = make_comment("src/a.ts", &lines()[3..4], "Why 3?", "c1", Some("of commit a1b2c3d"))
            .unwrap_or_else(|| panic!("a comment"));
        assert_eq!(made.range, "line 11 of commit a1b2c3d");
        assert_eq!(
            comment_block(&made),
            "In `src/a.ts`, line 11 of commit a1b2c3d:\n\n```diff\n+const b = 3;\n```\n\nWhy 3?"
        );
    }

    #[test]
    fn goes_ahead_of_the_typed_text_several_in_the_order_they_were_added() {
        let second = DiffComment {
            id: "c2".into(),
            path: "b.rs".into(),
            range: "line 4".into(),
            quote: "+x".into(),
            text: "Rename this".into(),
        };
        assert_eq!(
            prompt_text(&[comment(), second], "Then run the tests.\n"),
            "In `src/a.ts`, line 11:\n\n```diff\n-const b = 2;\n+const b = 3;\n```\n\nWhy 3?\n\n\
             In `b.rs`, line 4:\n\n```diff\n+x\n```\n\nRename this\n\n\
             Then run the tests."
        );
    }

    #[test]
    fn is_the_whole_message_when_nothing_is_typed_and_leaves_a_draft_without_comments_as_typed() {
        assert_eq!(
            prompt_text(&[comment()], "  "),
            "In `src/a.ts`, line 11:\n\n```diff\n-const b = 2;\n+const b = 3;\n```\n\nWhy 3?"
        );
        assert_eq!(prompt_text(&[], "  as typed \n"), "  as typed \n");
    }

    #[test]
    fn uses_a_fence_longer_than_any_run_of_backticks_in_the_quoted_code() {
        let block = comment_block(&DiffComment {
            quote: "+const s = ```;".into(),
            ..comment()
        });
        assert!(block.contains("````diff\n+const s = ```;\n````"), "{block}");
    }

    #[test]
    fn keeps_hunk_headers_but_not_notes_and_cuts_a_long_quote_saying_how_much_is_left_out() {
        let note = DiffLine::new('!', "Binary file", None, None);
        let made = make_comment("a", &[hunk(), kept(), note], "x", "c", None).unwrap_or_else(|| panic!("a comment"));
        assert_eq!(made.quote, "@@ -10,3 +10,3 @@ function load()\n const a = 1;");
        let long: Vec<DiffLine> = (0..400)
            .map(|n| DiffLine::new('+', &"y".repeat(40), None, Some(n + 1)))
            .collect();
        let quote = make_comment("a", &long, "x", "c", None)
            .unwrap_or_else(|| panic!("a comment"))
            .quote;
        assert!(quote.len() <= QUOTE_MAX + 20, "{}", quote.len());
        let tail = quote.rsplit('\n').next().unwrap_or_default();
        assert!(tail.starts_with("… ") && tail.ends_with(" more lines"), "{tail}");
    }

    #[test]
    fn is_nothing_for_a_blank_comment_or_a_pick_with_no_code_line() {
        assert_eq!(make_comment("a", &lines()[1..3], " \n ", "c", None), None);
        assert_eq!(make_comment("a", &[hunk()], "note", "c", None), None);
    }

    #[test]
    fn the_range_uses_the_new_files_numbers_or_the_old_files_for_removed_lines_only() {
        assert_eq!(range_label(&[kept()]), "line 10");
        assert_eq!(range_label(&[kept(), removed(), added(), after()]), "lines 10-12");
        assert_eq!(range_label(&[removed()]), "removed line 11");
    }

    #[test]
    fn a_comment_reads_back_from_storage_only_when_well_formed() {
        let stored = serde_json::to_value(comment()).unwrap_or_default();
        assert_eq!(serde_json::from_value::<DiffComment>(stored).ok(), Some(comment()));
        let bad = serde_json::json!({ "id": "c1", "path": "a", "range": "line 1", "quote": "+x", "text": 3 });
        assert!(serde_json::from_value::<DiffComment>(bad).is_err());
    }

    const PATCH: &str = "diff --git a/a.ts b/a.ts\nindex 1..2 100644\n--- a/a.ts\n+++ b/a.ts\n@@ -10,3 +10,3 @@ function load()\n const a = 1;\n-const b = 2;\n+const b = 3;\n return a + b;\n\\ No newline at end of file\ndiff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n";

    #[test]
    fn reads_each_files_lines_with_their_numbers() {
        let files = parse_lines(PATCH);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0], lines());
        assert_eq!(files[1], vec![DiffLine::new('!', "Binary file changed", None, None)]);
    }

    #[test]
    fn a_picked_range_is_the_lines_between_its_ends_on_either_side() {
        let lines = lines();
        assert_eq!(lines_between(&lines, Side::New, 10, Side::New, 11), &lines[1..4]);
        // From the old side's removed line to the new side's last line, and back to front.
        assert_eq!(lines_between(&lines, Side::Old, 11, Side::New, 12), &lines[2..5]);
        assert_eq!(lines_between(&lines, Side::New, 12, Side::Old, 11), &lines[2..5]);
        assert!(lines_between(&lines, Side::New, 99, Side::New, 11).is_empty());
    }
}
