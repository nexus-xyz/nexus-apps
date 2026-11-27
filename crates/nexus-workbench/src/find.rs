//! Find's matching, shared by the apps' find bars: any case, the text as typed (`find-logic.ts`'s `textRanges`). And
//! find in the Changes view: each place the text sits in a diff's lines, file
//! by file, top to bottom. It reads the parsed lines, not what is drawn, so a folded file is found too. It reads each
//! line's code (context, added, removed), not hunk headers, notes or paths.
use std::ops::Range;

use crate::diff_comment::{DiffLine, pickable};

/// Where `query` sits in `text`, ignoring case: byte ranges of `text`, without overlaps.
pub fn text_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    // Lowercasing can change a letter's length, so each folded char keeps the span of the char it came from.
    let folded: Vec<(char, usize, usize)> = text
        .char_indices()
        .flat_map(|(at, ch)| ch.to_lowercase().map(move |lower| (lower, at, at + ch.len_utf8())))
        .collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at + needle.len() <= folded.len() {
        if folded[at..at + needle.len()]
            .iter()
            .map(|c| c.0)
            .eq(needle.iter().copied())
        {
            found.push(folded[at].1..folded[at + needle.len() - 1].2);
            at += needle.len();
        } else {
            at += 1;
        }
    }
    found
}

/// One place the text sits in a diff: the `occurrence`th (from 0) in line `line` of file `file`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChangesMatch {
    pub file: usize,
    pub line: usize,
    pub occurrence: usize,
}

/// Every place `query` sits in the files' lines (`diff_comment::parse_lines`), in the order the view shows them.
pub fn changes_matches<'a>(files: impl IntoIterator<Item = &'a [DiffLine]>, query: &str) -> Vec<ChangesMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for (file, lines) in files.into_iter().enumerate() {
        for (line, diff_line) in lines.iter().enumerate() {
            if !pickable(diff_line) {
                continue;
            }
            let count = text_ranges(&diff_line.text, query).len();
            found.extend((0..count).map(|occurrence| ChangesMatch { file, line, occurrence }));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_comment::parse_lines;

    #[test]
    fn finds_every_occurrence_ignoring_case() {
        assert_eq!(text_ranges("Nexus nexus NEXUS", "nexus"), vec![0..5, 6..11, 12..17]);
        assert_eq!(text_ranges("aaaa", "aa"), vec![0..2, 2..4]);
        assert_eq!(text_ranges("anything", ""), Vec::<Range<usize>>::new());
        assert_eq!(text_ranges("anything", "zzz"), Vec::<Range<usize>>::new());
    }

    #[test]
    fn ranges_stay_on_char_boundaries_with_accents_and_expanding_case() {
        let text = "Olá ÁRVORE, İstanbul árvore";
        for range in text_ranges(text, "árvore") {
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
        }
        assert_eq!(text_ranges(text, "árvore").len(), 2);
        // 'İ' lowercases to two chars, 'i' + a combining dot.
        assert_eq!(text_ranges("İx", "x"), vec![2..3]);
    }

    fn files() -> Vec<Vec<DiffLine>> {
        parse_lines(
            &[
                "diff --git a/src/a.ts b/src/a.ts",
                "--- a/src/a.ts",
                "+++ b/src/a.ts",
                "@@ -1,3 +1,3 @@ function retry() {",
                " const retry = 1;",
                "-retry(retry);",
                "+again(Retry);",
                "diff --git a/img.png b/img.png",
                "Binary files a/img.png and b/img.png differ",
                "diff --git a/src/retry.ts b/src/retry.ts",
                "--- a/src/retry.ts",
                "+++ b/src/retry.ts",
                "@@ -1 +1 @@",
                "-old",
                "+RETRY",
            ]
            .join("\n"),
        )
    }

    fn matches(query: &str) -> Vec<ChangesMatch> {
        let files = files();
        changes_matches(files.iter().map(Vec::as_slice), query)
    }

    #[test]
    fn finds_the_text_in_each_files_code_lines_top_to_bottom_any_case() {
        let at = |file, line, occurrence| ChangesMatch { file, line, occurrence };
        assert_eq!(
            matches("retry"),
            vec![at(0, 1, 0), at(0, 2, 0), at(0, 2, 1), at(0, 3, 0), at(2, 2, 0)]
        );
    }

    #[test]
    fn skips_hunk_headers_notes_and_paths_and_finds_nothing_for_no_text() {
        assert!(matches("function").is_empty());
        assert!(matches("Binary").is_empty());
        assert!(matches("png").is_empty());
        assert!(matches("").is_empty());
    }
}
