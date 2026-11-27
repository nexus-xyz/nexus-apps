//! The project menu's search: whether a row's text matches the query, how well, and which letters to
//! mark. Every word of the query must be found. A word scores higher as an exact match, a prefix, a substring at a
//! word's start, a substring, then scattered letters in order. Case is ignored.

use std::ops::Range;

/// How well `text` matched, and the byte ranges of `text` to mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Found {
    pub score: i32,
    pub marks: Vec<Range<usize>>,
}

const EXACT: i32 = 1000;
const PREFIX: i32 = 800;
const WORD_START: i32 = 600;
const SUBSTRING: i32 = 400;
const SCATTERED: i32 = 100;

/// The query to match: a pasted GitHub URL (`https://github.com/owner/name.git`, `git@github.com:owner/name`)
/// becomes `owner/name`, so it finds the repository's row. Other text is kept, trimmed.
pub(super) fn normalize(query: &str) -> String {
    let query = query.trim();
    // ASCII only: the byte offsets found in `lower` must hold in `query`.
    let lower = query.to_ascii_lowercase();
    let path = ["github.com/", "github.com:"]
        .iter()
        .find_map(|host| lower.find(host).map(|at| &query[at + host.len()..]));
    let Some(path) = path else {
        return query.to_string();
    };
    let mut parts = path.split(['/', '?', '#']).filter(|part| !part.is_empty());
    match (parts.next(), parts.next()) {
        (Some(owner), Some(name)) => format!("{owner}/{}", name.strip_suffix(".git").unwrap_or(name)),
        _ => query.to_string(),
    }
}

/// Matches `query` (already `normalize`d) against `text`. None when a word is missing; an empty query matches with
/// no marks.
pub(super) fn find(query: &str, text: &str) -> Option<Found> {
    let chars: Vec<(usize, char)> = text.char_indices().map(|(at, c)| (at, fold(c))).collect();
    let mut score = 0;
    let mut marks = Vec::new();
    for word in query.split_whitespace() {
        let word: Vec<char> = word.chars().map(fold).collect();
        let (word_score, hits) = find_word(&word, &chars)?;
        score += word_score;
        marks.extend(hits.into_iter().map(|ix| byte_range(text, &chars, ix)));
    }
    Some(Found {
        score,
        marks: merge(marks),
    })
}

/// The better of two matches; ties keep `a`.
pub(super) fn best(a: Option<Found>, b: Option<Found>) -> Option<Found> {
    match (a, b) {
        (Some(a), Some(b)) if b.score > a.score => Some(b),
        (Some(a), _) => Some(a),
        (None, b) => b,
    }
}

/// `marks` of a text that starts `offset` bytes into the matched text, for a row that draws part of it. Ranges
/// outside it are dropped and ranges across its start are cut.
pub(super) fn shift(marks: &[Range<usize>], offset: usize, len: usize) -> Vec<Range<usize>> {
    marks
        .iter()
        .filter_map(|range| {
            let start = range.start.max(offset);
            let end = range.end.min(offset + len);
            (start < end).then(|| start - offset..end - offset)
        })
        .collect()
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// The word's score and the char indices it hit.
fn find_word(word: &[char], text: &[(usize, char)]) -> Option<(i32, Vec<usize>)> {
    if word.is_empty() {
        return Some((0, Vec::new()));
    }
    let n = word.len();
    let at = |start: usize| text.len() >= start + n && (0..n).all(|ix| text[start + ix].1 == word[ix]);
    let run = |start: usize| (start..start + n).collect::<Vec<_>>();
    if text.len() == n && at(0) {
        return Some((EXACT, run(0)));
    }
    if at(0) {
        return Some((PREFIX, run(0)));
    }
    let starts: Vec<usize> = (1..text.len()).filter(|&start| at(start)).collect();
    let word_start = |start: &usize| !text[start - 1].1.is_alphanumeric();
    if let Some(&start) = starts.iter().find(|start| word_start(start)) {
        return Some((WORD_START, run(start)));
    }
    if let Some(&start) = starts.first() {
        return Some((SUBSTRING, run(start)));
    }
    // Scattered letters, in order, taken as early as possible; each gap costs a point.
    let mut hits = Vec::with_capacity(n);
    let mut next = 0;
    for &c in word {
        let ix = (next..text.len()).find(|&ix| text[ix].1 == c)?;
        hits.push(ix);
        next = ix + 1;
    }
    let gaps = hits.windows(2).filter(|pair| pair[1] != pair[0] + 1).count() as i32;
    Some(((SCATTERED - gaps).max(1), hits))
}

fn byte_range(text: &str, chars: &[(usize, char)], ix: usize) -> Range<usize> {
    let start = chars[ix].0;
    let end = chars.get(ix + 1).map_or(text.len(), |(at, _)| *at);
    start..end
}

/// Sorted, with touching and overlapping ranges joined.
fn merge(mut marks: Vec<Range<usize>>) -> Vec<Range<usize>> {
    marks.sort_by_key(|range| range.start);
    let mut out: Vec<Range<usize>> = Vec::with_capacity(marks.len());
    for range in marks {
        match out.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => out.push(range),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{best, find, normalize, shift};

    fn score(query: &str, text: &str) -> Option<i32> {
        find(query, text).map(|found| found.score)
    }

    #[test]
    fn closer_matches_score_higher() {
        let exact = score("redline", "redline").unwrap();
        let prefix = score("redl", "redline").unwrap();
        let word = score("redl", "adalab/redline").unwrap();
        let inside = score("line", "redline").unwrap();
        let scattered = score("redl", "rust-edge-data-lake").unwrap();
        assert!(exact > prefix && prefix > word && word > inside && inside > scattered);
        assert_eq!(score("redlx", "redline"), None);
    }

    #[test]
    fn case_is_ignored_and_every_word_must_match() {
        assert!(score("NEXUS", "acme/nexus").is_some());
        assert!(score("nexus mono", "nexus mono").is_some());
        assert_eq!(score("nexus argo", "acme/nexus"), None);
        assert!(find("", "anything").unwrap().marks.is_empty());
    }

    #[test]
    fn marks_cover_the_matched_letters() {
        assert_eq!(find("redl", "redline").unwrap().marks, [0..4]);
        assert_eq!(find("redl", "Adalab/redline").unwrap().marks, [7..11]);
        // r, e, d, l scattered: each one marked alone.
        assert_eq!(find("rdl", "red-lake").unwrap().marks, [0..1, 2..3, 4..5]);
        // Multibyte letters keep their byte bounds.
        assert_eq!(find("é", "café").unwrap().marks, [3..5]);
    }

    #[test]
    fn a_pasted_github_url_becomes_owner_and_name() {
        for url in [
            "https://github.com/acme/argocd",
            "github.com/acme/argocd/",
            "https://github.com/acme/argocd.git",
            "git@github.com:acme/argocd.git",
            "https://github.com/acme/argocd/pull/12",
            "  https://GitHub.com/acme/argocd?tab=readme ",
        ] {
            assert_eq!(normalize(url), "acme/argocd", "{url}");
        }
        assert_eq!(normalize(" redl "), "redl");
        // A letter whose lowercase is longer, before the host, does not shift the cut.
        assert_eq!(normalize("İ github.com/a/b"), "a/b");
        assert_eq!(normalize("https://github.com/acme"), "https://github.com/acme");
        assert_eq!(
            score(&normalize("https://github.com/acme/argocd"), "acme/argocd"),
            Some(1000)
        );
    }

    #[test]
    fn best_keeps_the_higher_score_and_shift_cuts_marks_to_a_part() {
        let a = find("redl", "redline");
        let b = find("redline", "redline");
        assert_eq!(best(a.clone(), b.clone()), b);
        assert_eq!(best(a.clone(), None), a);
        assert_eq!(best(None, a.clone()), a);
        // "adalab/redline": the name starts at byte 7.
        assert_eq!(shift(&[0..2, 5..9], 7, 7), [0..2]);
        assert_eq!(shift(&[0..2, 5..9], 0, 6), [0..2, 5..6]);
    }
}
