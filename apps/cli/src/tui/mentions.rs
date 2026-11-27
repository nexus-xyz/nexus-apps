//! `@` file mentions, as Codex has them: `@` and a few letters in the composer open a list of the workspace's files,
//! ranked by a fuzzy match on the path; tab or ⏎ puts the path in. When the prompt is sent, chat gets
//! each mentioned file's text after the prompt (Gateway's chat takes only data files as attachments); the agent reads
//! them itself, so its prompt keeps the paths.
//!
//! The list comes from `git ls-files` (tracked and untracked, without what `.gitignore` leaves out), or from a walk of
//! the folder when it is not a repository.

use std::path::Path;
use std::process::Command;

use super::view::Theme;

/// The most files listed, the most of one file attached, and the most of all of them.
const MAX_FILES: usize = 20_000;
const MAX_FILE_BYTES: usize = 200_000;
const MAX_TOTAL_BYTES: usize = 500_000;
/// How many matches the list shows.
pub const SHOWN: usize = 8;

/// The `@word` the cursor is at the end of: where it starts (the `@`'s byte) and what follows the `@`.
pub fn current(text: &str, cursor: usize) -> Option<(usize, &str)> {
    let before = &text[..cursor];
    let start = before
        .rfind(char::is_whitespace)
        .map_or(0, |at| at + before[at..].chars().next().map_or(1, char::len_utf8));
    let word = &before[start..];
    let query = word.strip_prefix('@')?;
    // A word that goes on after the cursor is not being typed at its end.
    if text[cursor..].chars().next().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    Some((start, query))
}

/// The files that match `query`, best first: every character of the query in order in the path, closer together and
/// later in the path (the file's name) ranking higher. An empty query lists the shortest paths.
pub fn rank<'a>(files: &'a [String], query: &str) -> Vec<&'a str> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    let mut scored: Vec<(i64, &str)> = files
        .iter()
        .filter_map(|path| score(path, &query).map(|score| (score, path.as_str())))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(b.1)));
    scored.into_iter().take(SHOWN).map(|(_, path)| path).collect()
}

fn score(path: &str, query: &[char]) -> Option<i64> {
    let lower: Vec<char> = path.to_lowercase().chars().collect();
    let name_start = path.rfind('/').map_or(0, |at| path[..=at].chars().count());
    let mut at = 0;
    let mut score = 0i64;
    let mut last: Option<usize> = None;
    for wanted in query {
        let found = (at..lower.len()).find(|&index| lower[index] == *wanted)?;
        score += 10;
        if found >= name_start {
            score += 6;
        }
        if last.is_some_and(|last| found == last + 1) {
            score += 8;
        }
        if found == 0 || matches!(lower[found - 1], '/' | '_' | '-' | '.') {
            score += 5;
        }
        last = Some(found);
        at = found + 1;
    }
    Some(score - lower.len() as i64 / 8)
}

/// The paths a prompt mentions (`@path`, a file that exists in the workspace), each once, in order.
pub fn mentioned(prompt: &str, workspace: &Path) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for word in prompt.split_whitespace() {
        let Some(path) = word.strip_prefix('@') else { continue };
        let path = path.trim_end_matches([',', '.', ';', ':', ')', '?', '!']);
        if !path.is_empty() && !paths.iter().any(|known| known == path) && workspace.join(path).is_file() {
            paths.push(path.to_string());
        }
    }
    paths
}

/// The prompt with each mentioned file's text after it, for chat; what could not be attached is
/// said in the notes.
pub fn attach(prompt: &str, paths: &[String], workspace: &Path) -> (String, Vec<String>) {
    let mut content = prompt.to_string();
    let mut notes = Vec::new();
    let mut total = 0;
    for path in paths {
        let bytes = match std::fs::read(workspace.join(path)) {
            Ok(bytes) => bytes,
            Err(error) => {
                notes.push(format!("{path} was not attached: {error}"));
                continue;
            }
        };
        let Ok(text) = String::from_utf8(bytes) else {
            notes.push(format!("{path} was not attached: it is not a text file"));
            continue;
        };
        if text.len() > MAX_FILE_BYTES || total + text.len() > MAX_TOTAL_BYTES {
            notes.push(format!(
                "{path} was not attached: it is too large (at most {} KB a file)",
                MAX_FILE_BYTES / 1000
            ));
            continue;
        }
        total += text.len();
        content.push_str(&format!("\n\n<file path=\"{path}\">\n{}\n</file>", text.trim_end()));
    }
    (content, notes)
}

/// A stored prompt without the files `attach` put after it (`/resume` shows the prompt as it was typed).
pub fn strip_files(content: &str) -> &str {
    ["\n\n<file path=\"", "\n\n<terminal command=\""]
        .iter()
        .filter_map(|mark| content.find(mark))
        .min()
        .map_or(content, |at| &content[..at])
}

/// A command the person ran with `!`, as it goes into the conversation.
pub fn terminal_block(command: &str, exit: Option<i32>, output: &str) -> String {
    let output: String = output.chars().take(4_000).collect();
    let exit = exit.map_or("timed out".to_string(), |code| code.to_string());
    let command = command.replace('"', "'");
    format!(
        "\n\n<terminal command=\"{command}\" exit=\"{exit}\">\n{}\n</terminal>",
        output.trim_end()
    )
}

/// The workspace's files, as paths relative to it.
pub fn list(workspace: &Path) -> Vec<String> {
    let git = Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(workspace)
        .output();
    if let Ok(output) = git
        && output.status.success()
    {
        let mut files: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(String::from)
            .collect();
        files.dedup();
        files.truncate(MAX_FILES);
        return files;
    }
    let mut files = Vec::new();
    walk(workspace, workspace, &mut files);
    files.sort();
    files
}

fn walk(root: &Path, folder: &Path, files: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        if files.len() >= MAX_FILES {
            return;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || matches!(name.as_str(), "target" | "node_modules" | "dist" | "build") {
            continue;
        }
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            walk(root, &path, files);
        } else if let Ok(relative) = path.strip_prefix(root) {
            files.push(relative.to_string_lossy().to_string());
        }
    }
}

/// The list under the composer: the matches, the chosen one marked.
pub fn popup(theme: &Theme, matches: &[&str], selected: usize, loading: bool) -> Vec<String> {
    let style = &theme.style;
    if loading {
        return vec![format!("  {}", style.dim("listing files…"))];
    }
    if matches.is_empty() {
        return vec![format!("  {}", style.dim("no file matches"))];
    }
    matches
        .iter()
        .enumerate()
        .map(|(index, path)| {
            if index == selected {
                theme.selected(&format!("{} {}", style.accent(style.g.select), style.accent(path)))
            } else {
                format!("  {path}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_mention_being_typed() {
        assert_eq!(current("look at @src/ma", 15), Some((8, "src/ma")));
        assert_eq!(current("@", 1), Some((0, "")));
        assert_eq!(current("mail me@x", 9), None, "an @ inside a word is not one");
        assert_eq!(current("@src done", 2), None, "the cursor is inside the word");
    }

    #[test]
    fn ranks_names_and_runs_first() {
        let files: Vec<String> = ["src/tui/app.rs", "src/apple.rs", "docs/a/p/p.md", "Cargo.toml"]
            .map(String::from)
            .to_vec();
        assert_eq!(rank(&files, "app")[..2], ["src/apple.rs", "src/tui/app.rs"]);
        assert_eq!(rank(&files, "cargo"), ["Cargo.toml"]);
        assert!(rank(&files, "zzz").is_empty());
    }

    #[test]
    fn terminal_output_goes_after_the_prompt_and_comes_off_again() {
        let block = terminal_block("git status", Some(0), "On branch main\n");
        assert_eq!(
            block,
            "\n\n<terminal command=\"git status\" exit=\"0\">\nOn branch main\n</terminal>"
        );
        assert_eq!(strip_files(&format!("what now?{block}")), "what now?");
        assert_eq!(strip_files("plain"), "plain");
    }

    #[test]
    fn attaches_mentioned_text_files() {
        let dir = std::env::temp_dir().join(format!("nexus-mentions-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("workspace");
        std::fs::write(dir.join("notes.md"), "# Notes\n").expect("file");
        std::fs::write(dir.join("logo.png"), [0x89, 0x50, 0xff, 0xfe]).expect("file");
        let found = mentioned("read @notes.md, then @logo.png and @missing.rs", &dir);
        assert_eq!(found, ["notes.md", "logo.png"]);
        let (content, notes) = attach("read @notes.md", &found, &dir);
        assert_eq!(content, "read @notes.md\n\n<file path=\"notes.md\">\n# Notes\n</file>");
        assert_eq!(notes, ["logo.png was not attached: it is not a text file"]);
        assert_eq!(list(&dir), ["logo.png", "notes.md"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
