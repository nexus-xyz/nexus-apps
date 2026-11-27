//! The rules behind Ctrl+P (the file picker) and Ctrl+Shift+F (project search), as the Nexus app's
//! `desktop/src/workbench/project-files.ts`: how the picker reads what is typed and ranks the folder's files, and how
//! search results group and mark their match. `file_picker.rs` draws them. Pure: no GPUI.
//!
//! Ranges here are byte ranges, for GPUI's highlights. The Nexus app folds case with JavaScript's `toLowerCase`;
//! here a path's case folds in ASCII only, so a byte range in the folded path is the same range in the path.

use std::ops::Range;
use std::sync::Arc;

use nexus_workbench::project_files::{FEATURE, SearchMatch};

use super::slash::mention::{FileIndex, rank_files};

/// How many files the picker lists (`PICKER_LIMIT`).
pub(super) const PICKER_LIMIT: usize = 100;

/// Whether a server's host reads and searches its folder (`projectFilesOffered`). This computer's always does.
pub(super) fn offered(machine: Option<&str>) -> bool {
    machine.is_none_or(|machine| crate::machines::get().has_feature(machine, FEATURE))
}

/// What the picker's box says (`pickerQuery`): the file to find, and a line to open it at when it ends in `:42` (VS
/// Code's way; a column after it is dropped). Spaces around the text go, and a backslash reads as "/", so a path pasted
/// from Windows still finds its file.
pub(super) fn picker_query(text: &str) -> (String, Option<u32>) {
    let trimmed = text.trim().replace('\\', "/");
    let numeric = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    // `path:42`, or `path:42:7`.
    let mut parts: Vec<&str> = trimmed.rsplitn(3, ':').collect();
    parts.reverse();
    let (query, line) = match parts.as_slice() {
        [query, line, column] if numeric(line) && numeric(column) => (*query, *line),
        [.., query, line] if numeric(line) => {
            // With three parts, `query` is only the middle one: the rest before it is the path too.
            let cut = trimmed.len() - line.len() - 1;
            let _ = query;
            (&trimmed[..cut], *line)
        }
        _ => return (trimmed, None),
    };
    let line = line.parse::<u32>().ok().filter(|line| *line > 0);
    (query.trim().to_string(), line)
}

/// The files the picker lists for `text`, best first (`pickFiles`): the "@" menu's ranking (`mention.rs`
/// `rank_files`), so both pickers agree. A query with spaces must match each of its words, in any order, and the last
/// word ranks them (it is usually the file's name: "desktop store" finds store.ts in desktop/).
pub(super) fn pick_files(index: &FileIndex, text: &str, limit: usize) -> Vec<String> {
    let (query, _) = picker_query(text);
    let lower = query.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    if words.len() <= 1 {
        return rank_files(index, words.first().copied().unwrap_or(""), limit);
    }
    let kept: Vec<String> = index
        .paths
        .iter()
        .zip(&index.lower)
        .filter(|(_, path)| words.iter().all(|word| path.contains(word)))
        .map(|(path, _)| path.clone())
        .collect();
    rank_files(&FileIndex::new(Arc::new(kept)), words[words.len() - 1], limit)
}

/// Where in `path` the picker marks the query (`matchRanges`): the first place the whole query appears in the file's
/// name, else in the path, else each letter of a scattered match where it was found, neighbours joined. None when
/// nothing does.
pub(super) fn match_ranges(path: &str, text: &str) -> Vec<Range<usize>> {
    let query = picker_query(text).0.to_ascii_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let lower = path.to_ascii_lowercase();
    let name = lower.rfind('/').map_or(0, |slash| slash + 1);
    if let Some(at) = lower[name..].find(&query) {
        return vec![name + at..name + at + query.len()];
    }
    if let Some(at) = lower.find(&query) {
        return vec![at..at + query.len()];
    }
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut from = 0;
    for letter in query.chars().filter(|letter| *letter != ' ') {
        let Some(at) = lower[from..].find(letter).map(|at| from + at) else {
            return Vec::new();
        };
        let end = at + letter.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == at => last.end = end,
            _ => ranges.push(at..end),
        }
        from = end;
    }
    ranges
}

/// The ranges of `path` that fall in its file's name, counted from where the name starts.
pub(super) fn in_name(ranges: &[Range<usize>], path: &str) -> Vec<Range<usize>> {
    let by = path.rfind('/').map_or(0, |slash| slash + 1);
    ranges
        .iter()
        .filter(|range| range.end > by)
        .map(|range| range.start.saturating_sub(by)..range.end - by)
        .collect()
}

/// The ranges of `path` that fall in its folder (the part before the name).
pub(super) fn in_folder(ranges: &[Range<usize>], path: &str) -> Vec<Range<usize>> {
    let end = path.rfind('/').map_or(0, |slash| slash + 1);
    ranges
        .iter()
        .filter(|range| range.start < end)
        .map(|range| range.start..range.end.min(end))
        .collect()
}

/// Search results by file, in the order the core found them (`groupMatches`).
pub(super) fn group_matches(matches: &[SearchMatch]) -> Vec<(String, Vec<SearchMatch>)> {
    let mut groups: Vec<(String, Vec<SearchMatch>)> = Vec::new();
    for found in matches {
        match groups.iter_mut().find(|(path, _)| *path == found.path) {
            Some((_, group)) => group.push(found.clone()),
            None => groups.push((found.path.clone(), vec![found.clone()])),
        }
    }
    groups
}

/// The part of a result's text that matched, in bytes of `found.text` (`matchSpan`): the core's column, moved by where
/// the shown text starts, when the query is there; else the first place it appears; else None.
pub(super) fn match_span(found: &SearchMatch, query: &str, case_sensitive: bool) -> Option<Range<usize>> {
    if query.is_empty() {
        return None;
    }
    let fold = |text: &str| {
        if case_sensitive {
            text.to_string()
        } else {
            text.to_lowercase()
        }
    };
    let text: Vec<char> = fold(&found.text).chars().collect();
    let needle: Vec<char> = fold(query).chars().collect();
    let at_column = (found.column as usize).checked_sub(found.start as usize);
    let starts_at = |at: usize| text.get(at..at + needle.len()) == Some(needle.as_slice());
    let at = at_column
        .filter(|at| starts_at(*at))
        .or_else(|| (0..text.len()).find(|at| starts_at(*at)))?;
    // Map folded characters back to the original bytes: lowercasing İ adds a combining dot.
    let positions: Vec<_> = found
        .text
        .char_indices()
        .flat_map(|(start, ch)| {
            let end = start + ch.len_utf8();
            let count = if case_sensitive { 1 } else { ch.to_lowercase().count() };
            std::iter::repeat_n(start..end, count)
        })
        .collect();
    Some(positions.get(at)?.start..positions.get(at + needle.len() - 1)?.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILES: [&str; 7] = [
        "README.md",
        "package.json",
        "src/store.ts",
        "src/workbench/store.test.ts",
        "desktop/src/workbench/store.ts",
        "desktop/src/workbench/side-pane.tsx",
        "docs/getting started.md",
    ];

    fn index() -> FileIndex {
        FileIndex::new(Arc::new(FILES.iter().map(|path| path.to_string()).collect()))
    }

    fn pick(text: &str) -> Vec<String> {
        pick_files(&index(), text, PICKER_LIMIT)
    }

    #[test]
    fn reads_a_trailing_line_and_a_column_after_it_as_where_to_open() {
        assert_eq!(picker_query("store.ts:42"), ("store.ts".into(), Some(42)));
        assert_eq!(picker_query(" src/store.ts:7:3 "), ("src/store.ts".into(), Some(7)));
        assert_eq!(picker_query("store.ts:0"), ("store.ts".into(), None));
        assert_eq!(picker_query("store"), ("store".into(), None));
        assert_eq!(picker_query("a:b:12"), ("a:b".into(), Some(12)));
        assert_eq!(picker_query("C:/x.rs"), ("C:/x.rs".into(), None));
    }

    #[test]
    fn reads_a_windows_path_with_slashes() {
        assert_eq!(picker_query("src\\store.ts"), ("src/store.ts".into(), None));
    }

    #[test]
    fn ranks_as_the_mention_picker_does() {
        assert_eq!(
            pick("store.ts")[..3],
            [
                "src/store.ts",
                "desktop/src/workbench/store.ts",
                "src/workbench/store.test.ts"
            ]
        );
    }

    #[test]
    fn finds_scattered_letters_and_ignores_the_line() {
        assert_eq!(pick("sdpn")[0], "desktop/src/workbench/side-pane.tsx");
        assert_eq!(pick("readme:3"), ["README.md"]);
    }

    #[test]
    fn needs_every_word_of_a_query_with_spaces_in_any_order() {
        let both = ["src/workbench/store.test.ts", "desktop/src/workbench/store.ts"];
        assert_eq!(pick("workbench store"), both);
        assert_eq!(pick("store workbench"), both);
        assert_eq!(pick("workbench store.ts"), ["desktop/src/workbench/store.ts"]);
        assert_eq!(pick("getting started"), ["docs/getting started.md"]);
        assert!(pick("nothing here").is_empty());
    }

    #[test]
    fn lists_the_shallowest_files_with_no_query_at_most_the_limit() {
        assert_eq!(pick_files(&index(), "", 2), ["README.md", "package.json"]);
    }

    #[test]
    fn marks_the_query_in_the_name_before_the_folders() {
        assert_eq!(match_ranges("src/store/store.ts", "store"), [10..15]);
        assert_eq!(match_ranges("src/workbench/x.ts", "workbench"), [4..13]);
    }

    #[test]
    fn marks_each_letter_of_a_scattered_match_joining_neighbours() {
        assert_eq!(match_ranges("side-pane.tsx", "sdpa"), [0..1, 2..3, 5..7]);
        assert!(match_ranges("side-pane.tsx", "zz").is_empty());
        assert!(match_ranges("side-pane.tsx", "").is_empty());
    }

    #[test]
    fn splits_the_marks_between_the_name_and_its_folder() {
        let path = "src/workbench/store.ts";
        let ranges = match_ranges(path, "wstore");
        assert_eq!(ranges, [4..5, 14..19]);
        assert_eq!(in_name(&ranges, path), [0..5]);
        assert_eq!(in_folder(&ranges, path), [4..5]);
    }

    fn hit(path: &str, line: u32, column: u32, text: &str, start: u32) -> SearchMatch {
        SearchMatch {
            path: path.into(),
            line,
            column,
            start,
            text: text.into(),
        }
    }

    #[test]
    fn case_folding_keeps_original_utf8_highlight_offsets() {
        assert_eq!(match_span(&hit("a", 1, 1, "İ main", 1), "main", false), Some(3..7));
        assert_eq!(match_span(&hit("a", 1, 1, "İ main", 1), "i", false), Some(0..2));
        assert_eq!(match_span(&hit("a", 1, 1, "é Main", 1), "main", false), Some(3..7));
    }

    #[test]
    fn groups_by_file_in_the_order_found() {
        let groups = group_matches(&[
            hit("b.ts", 1, 1, "x", 1),
            hit("a.ts", 2, 1, "x", 1),
            hit("b.ts", 9, 1, "x", 1),
        ]);
        let lines: Vec<(String, Vec<u32>)> = groups
            .into_iter()
            .map(|(path, found)| (path, found.iter().map(|hit| hit.line).collect()))
            .collect();
        assert_eq!(lines, [("b.ts".into(), vec![1, 9]), ("a.ts".into(), vec![2])]);
    }

    #[test]
    fn marks_the_cores_column_any_case() {
        assert_eq!(
            match_span(&hit("a", 1, 5, "let Main = main", 1), "main", false),
            Some(4..8)
        );
        assert_eq!(
            match_span(&hit("a", 1, 12, "let Main = main", 1), "main", true),
            Some(11..15)
        );
    }

    #[test]
    fn follows_a_long_lines_shown_part_and_falls_back_to_the_first_place_found() {
        assert_eq!(match_span(&hit("a", 1, 101, "....main", 97), "main", false), Some(4..8));
        assert_eq!(match_span(&hit("a", 1, 1, "xx main", 1), "main", false), Some(3..7));
        assert_eq!(match_span(&hit("a", 1, 1, "nothing", 1), "main", false), None);
        assert_eq!(match_span(&hit("a", 1, 3, "é main", 1), "main", false), Some(3..7));
    }
}
