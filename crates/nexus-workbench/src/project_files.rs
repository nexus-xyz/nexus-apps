//! Reading and searching a session's folder, for the file viewer, the ⌘P file picker and project search
//! (`wb_file_read`, `wb_file_search`). Both stay inside the folder: a path is relative, has no `..`, and resolves
//! (links followed) to a file under the folder's own resolved path, so a link that leads out is refused. Search
//! never follows a link at all, as the git panel's count of untracked files does not (`git::untracked_lines`).
//!
//! Search reads the files the @-mention picker lists ([`files::list`]: what git knows of less what `.gitignore`
//! hides, or a bounded walk outside a repository), one at a time, with no index and no new dependency. Its work is
//! bounded: files over [`MAX_SEARCH_FILE`] and binary ones are skipped, it reads at most [`MAX_SEARCH_BYTES`] and
//! runs at most [`SEARCH_FOR`], and it stops at [`MAX_MATCHES`]; any of those marks the result as cut.

use crate::files;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// What a core that answers `wb_file_read` and `wb_file_search` says in its features. An older host lacks it, and
/// the app does not offer the picker or search for its sessions.
pub const FEATURE: &str = "project-files";

/// The most of a file the viewer gets. A bigger file shows its first part, cut at a line's end.
pub const MAX_READ: u64 = 1024 * 1024;
/// Files bigger than this are not searched (a log, a lockfile, generated code).
pub const MAX_SEARCH_FILE: u64 = 1024 * 1024;
/// The most one search reads, over all its files.
pub const MAX_SEARCH_BYTES: u64 = 64 * 1024 * 1024;
/// A search stops after this many matching lines.
pub const MAX_MATCHES: usize = 500;
/// A search stops after this long.
pub const SEARCH_FOR: Duration = Duration::from_secs(10);
/// The longest query, in characters.
pub const MAX_QUERY: usize = 200;
/// A matching line longer than this many characters shows only the part around the match.
pub const MAX_LINE: usize = 300;
/// Characters kept before the match when a long line is cut.
const LEAD: usize = 60;
/// Git calls a file binary when a NUL appears in its first 8000 bytes; so does the viewer.
const BINARY_PROBE: usize = 8000;

/// A file for the viewer. `text` is None for a binary file; `truncated` says only its first [`MAX_READ`] bytes came.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileText {
    pub path: String,
    pub text: Option<String>,
    pub binary: bool,
    pub truncated: bool,
    pub size: u64,
}

/// One matching line: the file, the line and the column of the first match (both from 1, the column in characters),
/// and the line's text (only the part around the match when it is long; `start` is that part's first column).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub start: u32,
    pub text: String,
}

/// A search's matches, in the file list's order, and whether it stopped before the end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
}

/// `relative` under `root`, resolved: refused when it is empty, absolute or climbs with `..`, and when it resolves
/// (through a link) outside `root`'s resolved path. Fails too when the file is not there.
pub fn contained(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(root).map_err(|_| "This session's folder is gone.".to_string())?;
    inside(&root, relative)
}

/// [`contained`], with `root` already resolved.
fn inside(root: &Path, relative: &str) -> Result<PathBuf, String> {
    const OUTSIDE: &str = "That file is outside the session's folder.";
    if relative.is_empty() || relative.contains('\0') {
        return Err("Name a file in the session's folder.".into());
    }
    let path = Path::new(relative);
    if !path
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err(OUTSIDE.into());
    }
    let full = std::fs::canonicalize(root.join(path)).map_err(|_| format!("{relative} is not in the folder."))?;
    if !full.starts_with(root) {
        return Err(OUTSIDE.into());
    }
    Ok(full)
}

/// The file at `relative` in `root`, for the viewer: at most [`MAX_READ`] bytes of it. Blocking.
pub fn read(root: &Path, relative: &str) -> Result<FileText, String> {
    let path = contained(root, relative)?;
    // Checked before opening: opening a FIFO would wait for a writer.
    let meta = std::fs::metadata(&path).map_err(|error| error.to_string())?;
    if !meta.is_file() {
        return Err(format!("{relative} is not a file."));
    }
    let file = std::fs::File::open(&path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::with_capacity(usize::try_from(meta.len().min(MAX_READ)).unwrap_or(0));
    file.take(MAX_READ + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let truncated = bytes.len() as u64 > MAX_READ;
    let size = meta.len().max(bytes.len() as u64);
    if binary(&bytes) {
        return Ok(FileText {
            path: relative.to_string(),
            text: None,
            binary: true,
            truncated: false,
            size,
        });
    }
    if truncated {
        bytes.truncate(MAX_READ as usize);
        // A cut file ends at its last whole line.
        if let Some(end) = bytes.iter().rposition(|&byte| byte == b'\n') {
            bytes.truncate(end + 1);
        }
    }
    Ok(FileText {
        path: relative.to_string(),
        text: Some(String::from_utf8_lossy(&bytes).into_owned()),
        binary: false,
        truncated,
        size,
    })
}

fn binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_PROBE)].contains(&0)
}

/// The lines of `root`'s files that hold `query` (any case unless `case_sensitive`), at most [`MAX_MATCHES`].
/// Blocking.
pub fn search(root: &Path, query: &str, case_sensitive: bool) -> Result<SearchResult, String> {
    let list = files::list(root)?;
    search_in(
        root,
        &list.files,
        list.truncated,
        query,
        case_sensitive,
        Limits::DEFAULT,
    )
}

/// How far a search goes: [`search`] uses the constants, the tests smaller ones.
#[derive(Debug, Clone, Copy)]
struct Limits {
    file: u64,
    bytes: u64,
    matches: usize,
    time: Duration,
}

impl Limits {
    const DEFAULT: Limits = Limits {
        file: MAX_SEARCH_FILE,
        bytes: MAX_SEARCH_BYTES,
        matches: MAX_MATCHES,
        time: SEARCH_FOR,
    };
}

fn search_in(
    root: &Path,
    files: &[String],
    listed_all: bool,
    query: &str,
    case_sensitive: bool,
    limits: Limits,
) -> Result<SearchResult, String> {
    if query.trim().is_empty() {
        return Err("Type something to search for.".into());
    }
    if query.chars().count() > MAX_QUERY || query.contains(['\n', '\r']) {
        return Err(format!("Search for one line of at most {MAX_QUERY} characters."));
    }
    let resolved = std::fs::canonicalize(root).map_err(|_| "This session's folder is gone.".to_string())?;
    let needle = fold(query, case_sensitive).into_owned();
    let started = Instant::now();
    let mut left = limits.bytes;
    let mut matches = Vec::new();
    // `listed_all` is the file list's own `truncated`: a folder past its cap is not searched whole either.
    let mut truncated = listed_all;
    for relative in files {
        if matches.len() >= limits.matches || left == 0 || started.elapsed() > limits.time {
            truncated = true;
            break;
        }
        let Some(bytes) = searchable(root, &resolved, relative, limits.file.min(left)) else {
            continue;
        };
        left = left.saturating_sub(bytes.len() as u64);
        let text = String::from_utf8_lossy(&bytes);
        for (index, line) in text.lines().enumerate() {
            let Some(found) = find(line, &needle, case_sensitive) else {
                continue;
            };
            matches.push(matched(relative, index, line, found));
            if matches.len() >= limits.matches {
                break;
            }
        }
    }
    if matches.len() >= limits.matches {
        truncated = true;
    }
    Ok(SearchResult { matches, truncated })
}

/// A listed file's bytes when search may read it: a plain file (never a link), inside the folder once resolved, no
/// bigger than `max`, and not binary.
fn searchable(root: &Path, resolved: &Path, relative: &str, max: u64) -> Option<Vec<u8>> {
    let meta = std::fs::symlink_metadata(root.join(relative)).ok()?;
    if !meta.file_type().is_file() || meta.len() > max {
        return None;
    }
    // A folder on the way that is a link: refused when it leads out.
    let path = inside(resolved, relative).ok()?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(max + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= max && !binary(&bytes)).then_some(bytes)
}

fn fold(text: &str, case_sensitive: bool) -> Cow<'_, str> {
    if case_sensitive {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(text.to_lowercase())
    }
}

/// The column (in characters, from 0) where `needle` (already folded) first appears in `line`.
fn find(line: &str, needle: &str, case_sensitive: bool) -> Option<usize> {
    let hay = fold(line, case_sensitive);
    let at = hay.find(needle)?;
    // Lowercasing can change a character's length, so the column is counted in the folded line.
    Some(hay[..at].chars().count())
}

fn matched(path: &str, index: usize, line: &str, column: usize) -> SearchMatch {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let length = line.chars().count();
    let start = if length <= MAX_LINE {
        0
    } else {
        column.saturating_sub(LEAD).min(length - MAX_LINE)
    };
    SearchMatch {
        path: path.to_string(),
        line: u32::try_from(index + 1).unwrap_or(u32::MAX),
        column: u32::try_from(column + 1).unwrap_or(u32::MAX),
        start: u32::try_from(start + 1).unwrap_or(u32::MAX),
        text: line.chars().skip(start).take(MAX_LINE).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-wb-project-files-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn reads_a_file_inside_the_folder() {
        let dir = temp("read");
        fs::create_dir_all(dir.join("src")).expect("src");
        fs::write(dir.join("src/main.rs"), "fn main() {}\n").expect("main");
        let file = read(&dir, "src/main.rs").expect("read");
        assert_eq!(file.text.as_deref(), Some("fn main() {}\n"));
        assert_eq!((file.binary, file.truncated, file.size), (false, false, 13));
        assert_eq!(read(&dir, "./src/main.rs").expect("dot").path, "./src/main.rs");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_paths_that_leave_the_folder() {
        let root = temp("escape");
        let dir = root.join("work");
        fs::create_dir_all(dir.join("src")).expect("src");
        fs::write(root.join("secret.txt"), "no").expect("secret");
        fs::write(dir.join("src/ok.txt"), "ok").expect("ok");
        for path in ["../secret.txt", "src/../../secret.txt", "", "src/../src/ok.txt"] {
            assert!(read(&dir, path).is_err(), "{path:?} must be refused");
        }
        let absolute = root.join("secret.txt");
        assert!(read(&dir, &absolute.to_string_lossy()).is_err());
        assert!(read(&dir, "src").is_err(), "a folder is not a file");
        assert!(read(&dir, "missing.txt").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn follows_links_inside_the_folder_but_never_out_of_it() {
        use std::os::unix::fs::symlink;
        let root = temp("links");
        let dir = root.join("work");
        fs::create_dir_all(dir.join("src")).expect("src");
        fs::create_dir_all(root.join("outside")).expect("outside");
        fs::write(root.join("outside/secret.txt"), "secret word").expect("secret");
        fs::write(dir.join("src/real.txt"), "word inside").expect("real");
        symlink(root.join("outside/secret.txt"), dir.join("leak.txt")).expect("file link");
        symlink(root.join("outside"), dir.join("out")).expect("folder link");
        symlink(dir.join("src/real.txt"), dir.join("alias.txt")).expect("inner link");

        assert!(read(&dir, "leak.txt").is_err(), "a link to a file outside");
        assert!(read(&dir, "out/secret.txt").is_err(), "a folder link outside");
        assert_eq!(
            read(&dir, "alias.txt").expect("inner").text.as_deref(),
            Some("word inside")
        );

        // Search reads no link at all, and nothing through a folder link that leads out.
        let files = list(&["alias.txt", "leak.txt", "out/secret.txt", "src/real.txt"]);
        let found = search_in(&dir, &files, false, "word", false, Limits::DEFAULT).expect("search");
        let paths: Vec<&str> = found.matches.iter().map(|hit| hit.path.as_str()).collect();
        assert_eq!(paths, vec!["src/real.txt"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_big_file_comes_cut_at_a_line_and_a_binary_one_without_text() {
        let dir = temp("caps");
        let line = "x".repeat(99) + "\n";
        let lines = (MAX_READ as usize / line.len()) + 10;
        fs::write(dir.join("big.txt"), line.repeat(lines)).expect("big");
        let big = read(&dir, "big.txt").expect("big");
        let text = big.text.expect("text");
        assert!(big.truncated);
        assert!(text.len() as u64 <= MAX_READ);
        assert!(text.ends_with('\n'), "cut at a whole line");
        assert_eq!(big.size, (line.len() * lines) as u64);

        fs::write(dir.join("image.png"), [0x89, b'P', b'N', b'G', 0, 1, 2]).expect("png");
        let image = read(&dir, "image.png").expect("png");
        assert_eq!((image.text, image.binary), (None, true));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_finds_lines_any_case_and_skips_binary_and_big_files() {
        let dir = temp("search");
        fs::write(dir.join("a.rs"), "fn Main() {}\nlet main = 1;\r\nnothing\n").expect("a");
        fs::write(dir.join("bin.dat"), b"main\0main").expect("bin");
        fs::write(dir.join("big.txt"), "main\n".repeat(100)).expect("big");
        let files = list(&["a.rs", "big.txt", "bin.dat", "gone.txt"]);
        let limits = Limits {
            file: 100,
            ..Limits::DEFAULT
        };

        let found = search_in(&dir, &files, false, "MAIN", false, limits).expect("search");
        assert!(!found.truncated);
        assert_eq!(
            found.matches,
            vec![
                SearchMatch {
                    path: "a.rs".into(),
                    line: 1,
                    column: 4,
                    start: 1,
                    text: "fn Main() {}".into()
                },
                SearchMatch {
                    path: "a.rs".into(),
                    line: 2,
                    column: 5,
                    start: 1,
                    text: "let main = 1;".into()
                },
            ]
        );
        let exact = search_in(&dir, &files, false, "Main", true, limits).expect("exact");
        assert_eq!(exact.matches.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_stops_at_its_caps_and_says_so() {
        let dir = temp("search-caps");
        fs::write(dir.join("a.txt"), "hit\n".repeat(10)).expect("a");
        fs::write(dir.join("b.txt"), "hit\n").expect("b");
        let files = list(&["a.txt", "b.txt"]);

        let few = search_in(
            &dir,
            &files,
            false,
            "hit",
            false,
            Limits {
                matches: 3,
                ..Limits::DEFAULT
            },
        )
        .expect("few");
        assert_eq!(few.matches.len(), 3);
        assert!(few.truncated);

        // The byte budget runs out after the first file: the second is never read.
        let budget = search_in(
            &dir,
            &files,
            false,
            "hit",
            false,
            Limits {
                bytes: 40,
                ..Limits::DEFAULT
            },
        )
        .expect("bytes");
        assert_eq!(budget.matches.len(), 10);
        assert!(budget.truncated);

        // A file list that was itself cut makes the search cut too.
        let listed = search_in(&dir, &files, true, "hit", false, Limits::DEFAULT).expect("listed");
        assert_eq!(listed.matches.len(), 11);
        assert!(listed.truncated);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_line_shows_the_part_around_its_match() {
        let line = format!("{}needle{}", "a".repeat(1000), "b".repeat(1000));
        let hit = matched("f", 0, &line, 1000);
        assert_eq!(hit.column, 1001);
        assert_eq!(hit.start, 941);
        assert_eq!(hit.text.chars().count(), MAX_LINE);
        assert!(hit.text.contains("needle"));
    }

    #[test]
    fn a_query_must_be_one_short_line() {
        let dir = temp("query");
        let long = "x".repeat(MAX_QUERY + 1);
        for query in ["", "   ", "a\nb", long.as_str()] {
            assert!(
                search_in(&dir, &[], false, query, false, Limits::DEFAULT).is_err(),
                "{query:?}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
