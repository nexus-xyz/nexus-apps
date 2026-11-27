//! The Commands pane's list rules, kept apart from drawing so tests can pin them: what a search matches, how each
//! command's state reads at a glance, its age, and how the keyboard moves through the rows.
use super::Entry;
use std::ops::Range;

/// How a row's leading mark reads. Hue appears only where the person must act or a command failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    /// Waiting for the person's permission.
    Attention,
    /// Running now: the mark breathes.
    Running,
    /// Starting, waiting on the provider, or stopping.
    Pending,
    /// Ended with exit code 0.
    Done,
    /// Ended with another code, or failed to start.
    Failed,
    /// Declined, denied or cancelled: it never ran, or was stopped before an outcome.
    NotRun,
    /// Nexus could not confirm the outcome.
    Unknown,
    /// Stopped, or finished without an exit code.
    Ended,
}

pub(super) fn tone(entry: &Entry) -> Tone {
    if entry.status == "awaitingUser" {
        return Tone::Attention;
    }
    if entry.stop_requested && entry.status == "running" {
        return Tone::Pending;
    }
    match (entry.status.as_str(), entry.exit_code) {
        ("running", _) => Tone::Running,
        ("starting" | "providerPending", _) => Tone::Pending,
        // Stopped by a person: the stopped mark, whatever exit code the stop left.
        ("interrupted", _) => Tone::Ended,
        (_, Some(0)) => Tone::Done,
        (_, Some(_)) | ("failed", None) => Tone::Failed,
        ("declined" | "denied" | "cancelled", _) => Tone::NotRun,
        ("unknown", _) => Tone::Unknown,
        _ => Tone::Ended,
    }
}

/// The first line of a command, and how many lines follow it, for a one-line row.
pub(super) fn first_line(command: &str) -> (&str, usize) {
    let mut lines = command.trim().lines();
    let first = lines.next().unwrap_or("").trim_end();
    (first, lines.count())
}

/// A command's name for a one-line row, or `None` when the name is only the start of the command: the registry
/// names an unnamed command after its first 48 characters, newlines included, which would repeat the command beside
/// itself and break the row over several lines.
pub(super) fn label(name: &str, command: &str) -> Option<String> {
    let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = flat(name);
    (!name.is_empty() && !flat(command).starts_with(&name)).then_some(name)
}

/// A command as a hover tooltip shows it: at most 6 lines of at most 120 characters, each cut with "…", and a last
/// "…" line when more follow. The whole command is in the row's details, where it scrolls.
pub(super) fn tooltip(command: &str) -> String {
    const LINES: usize = 6;
    const WIDTH: usize = 120;
    let mut lines = command.trim().lines();
    let mut shown: Vec<String> = lines
        .by_ref()
        .take(LINES)
        .map(|line| {
            if line.chars().count() > WIDTH {
                format!("{}…", line.chars().take(WIDTH).collect::<String>())
            } else {
                line.to_string()
            }
        })
        .collect();
    if lines.next().is_some() {
        shown.push("…".into());
    }
    shown.join("\n")
}

/// The search's words, lower case. Every word must match somewhere in a command for it to show.
pub(super) fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether `entry` shows for the search: each word is in its name, its command, its folder or its status.
pub(super) fn matches(entry: &Entry, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let haystack = format!("{}\n{}\n{}\n{}", entry.name, entry.command, entry.cwd, entry.status()).to_lowercase();
    words.iter().all(|word| haystack.contains(word.as_str()))
}

/// The byte ranges of `text` that hold a search word, merged and sorted, for marking a match. ASCII case folding
/// only, so the ranges stay on `text`'s own character boundaries.
pub(super) fn ranges(text: &str, words: &[String]) -> Vec<Range<usize>> {
    let folded = text.to_ascii_lowercase();
    let mut found: Vec<Range<usize>> = Vec::new();
    for word in words {
        let word = word.to_ascii_lowercase();
        if word.is_empty() {
            continue;
        }
        let mut from = 0;
        while let Some(at) = folded[from..].find(&word) {
            let start = from + at;
            let end = start + word.len();
            if text.is_char_boundary(start) && text.is_char_boundary(end) {
                found.push(start..end);
            }
            from = end;
        }
    }
    found.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in found {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// A short age, as the sidebar writes one: "now", "4m", "2h", "3d". `created_at` and `now` are milliseconds.
pub(super) fn age(created_at: i64, now: i64) -> Option<String> {
    if created_at <= 0 {
        return None;
    }
    let seconds = (now - created_at).max(0) / 1000;
    Some(match seconds {
        0..=44 => "now".into(),
        45..=3599 => format!("{}m", (seconds / 60).max(1)),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    })
}

/// The row the keyboard selects after a press of Up (`-1`) or Down (`1`). It wraps at the ends. With nothing
/// selected, or the selected row filtered away, Down takes the first row and Up the last.
pub(super) fn step(ids: &[&str], selected: Option<&str>, delta: i32) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let len = ids.len() as i32;
    let next = match selected.and_then(|id| ids.iter().position(|row| *row == id)) {
        Some(at) => (at as i32 + delta).rem_euclid(len),
        None if delta < 0 => len - 1,
        None => 0,
    };
    Some(ids[next as usize].to_string())
}

/// The keyboard's row after the list changes: the same row while it shows, else the first one.
pub(super) fn keep(ids: &[&str], selected: Option<&str>) -> Option<String> {
    match selected {
        Some(id) if ids.contains(&id) => Some(id.to_string()),
        _ => ids.first().map(|id| id.to_string()),
    }
}

/// "1 command", "3 commands".
pub(super) fn commands(count: usize) -> String {
    format!("{count} command{}", if count == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use serde_json::json;

    fn entry(status: &str, exit: Option<u32>) -> Entry {
        serde_json::from_value(json!({
            "id":"one","name":"Run tests","command":"cargo test -p nexus\n--quiet","cwd":"/work/Repo",
            "status":status,"private":false,"exitCode":exit
        }))
        .unwrap()
    }

    #[test]
    fn each_state_reads_as_one_tone() {
        assert_eq!(tone(&entry("awaitingUser", None)), Tone::Attention);
        assert_eq!(tone(&entry("running", None)), Tone::Running);
        let mut stopping = entry("running", None);
        stopping.stop_requested = true;
        assert_eq!(tone(&stopping), Tone::Pending);
        assert_eq!(tone(&entry("providerPending", None)), Tone::Pending);
        assert_eq!(tone(&entry("completed", Some(0))), Tone::Done);
        assert_eq!(tone(&entry("failed", Some(2))), Tone::Failed);
        assert_eq!(tone(&entry("failed", None)), Tone::Failed);
        assert_eq!(tone(&entry("declined", None)), Tone::NotRun);
        assert_eq!(tone(&entry("unknown", None)), Tone::Unknown);
        assert_eq!(tone(&entry("completed", None)), Tone::Ended);
        let stopped = entry("interrupted", Some(1));
        assert_eq!(tone(&stopped), Tone::Ended, "a stop is not a failure");
        assert_eq!(stopped.status(), "Stopped");
    }

    #[test]
    fn search_needs_every_word_across_name_command_folder_and_status() {
        let failed = entry("failed", Some(2));
        assert!(matches(&failed, &words("")));
        assert!(matches(&failed, &words("CARGO quiet")));
        assert!(matches(&failed, &words("tests repo")));
        assert!(matches(&failed, &words("exit 2")));
        assert!(!matches(&failed, &words("cargo npm")));
    }

    #[test]
    fn match_ranges_merge_and_stay_on_character_boundaries() {
        assert_eq!(ranges("cargo test", &words("car go")), vec![0..5]);
        assert_eq!(ranges("test a test", &words("test")), vec![0..4, 7..11]);
        assert_eq!(ranges("échec Test", &words("test")), vec![7..11]);
        assert!(ranges("anything", &[]).is_empty());
    }

    #[test]
    fn a_name_that_only_repeats_the_command_is_left_out_and_never_spans_lines() {
        assert_eq!(label("git log --oneline -3", "git log --oneline -3"), None);
        assert_eq!(
            label("echo \"edit me first\"\ndate", "echo \"edit me first\"\ndate"),
            None
        );
        let command = "echo \"step one\"\necho \"step two\"\nsleep 30\necho \"done\"";
        assert_eq!(label(&command[..41], command), None);
        assert_eq!(label("Run\n tests", "cargo test").as_deref(), Some("Run tests"));
        assert_eq!(label("  ", "cargo test"), None);
    }

    #[test]
    fn a_long_command_is_cut_down_for_its_tooltip() {
        assert_eq!(tooltip("cargo test"), "cargo test");
        let wide = "x".repeat(200);
        assert_eq!(tooltip(&wide), format!("{}…", "x".repeat(120)));
        let tall = (1..=10).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        assert_eq!(tooltip(&tall), "1\n2\n3\n4\n5\n6\n…");
        assert_eq!(tooltip("1\n2\n3\n4\n5\n6"), "1\n2\n3\n4\n5\n6");
    }

    #[test]
    fn a_row_shows_the_first_line_and_counts_the_rest() {
        assert_eq!(first_line("cargo test\n--quiet\n-j 4"), ("cargo test", 2));
        assert_eq!(first_line("  ls  "), ("ls", 0));
        assert_eq!(first_line(""), ("", 0));
    }

    #[test]
    fn ages_are_short_and_never_negative() {
        let now = 1_800_000_000_000;
        assert_eq!(age(0, now), None);
        assert_eq!(age(now + 5_000, now).as_deref(), Some("now"));
        assert_eq!(age(now - 50_000, now).as_deref(), Some("1m"));
        assert_eq!(age(now - 4 * 3_600_000, now).as_deref(), Some("4h"));
        assert_eq!(age(now - 3 * 86_400_000, now - 1).as_deref(), Some("2d"));
    }

    #[test]
    fn the_keyboard_wraps_and_recovers_a_lost_selection() {
        let ids = ["a", "b", "c"];
        assert_eq!(step(&ids, Some("c"), 1).as_deref(), Some("a"));
        assert_eq!(step(&ids, Some("a"), -1).as_deref(), Some("c"));
        assert_eq!(step(&ids, None, 1).as_deref(), Some("a"));
        assert_eq!(step(&ids, Some("gone"), -1).as_deref(), Some("c"));
        assert_eq!(step(&[], Some("a"), 1), None);
        assert_eq!(keep(&ids, Some("b")).as_deref(), Some("b"));
        assert_eq!(keep(&ids, Some("gone")).as_deref(), Some("a"));
        assert_eq!(keep(&[], Some("a")), None);
    }

    #[test]
    fn counts_agree_in_number() {
        assert_eq!(commands(1), "1 command");
        assert_eq!(commands(0), "0 commands");
        assert_eq!(commands(3), "3 commands");
    }
}
