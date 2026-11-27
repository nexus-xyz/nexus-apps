//! Which files an agent's answer names, and where a click on one opens it.
//!
//! A Markdown link to a local path names a file, as [`inline_media::file_reference`] reads it. A code span names one
//! only when it reads as a path: the Nexus app's `fileReference(text, true)` in `output-references.ts`, with one more
//! rule here. A span without a `/` must end in an extension from [`KNOWN`], so `self.files` or `foo.bar()` stays code.
//!
//! A click opens the file in the Workbench's Files tool, which shows text, Markdown, HTML, images, PDFs and videos. A
//! format it cannot show opens in the system app instead, on this computer only ([`opens`]).

use std::path::Path;

use crate::inline_media::{FileReference, file_reference};

/// Where a click on a file reference opens it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opens {
    /// The Workbench's Files tool, at the line.
    Files,
    /// The operating system's app for the format.
    App,
}

/// Formats the Files tool cannot show but a document app can: office documents, and AVIF, which GPUI does not draw.
/// Every one of them is in `output_files::can_open`, so "Open in app" may launch it.
const APP_ONLY: &[&str] = &[
    "doc", "docx", "odt", "rtf", "xls", "xlsx", "ods", "ppt", "pptx", "odp", "avif",
];

/// Extensions that name a file even without a folder in the path: [`CODE`], [`TEXT`] and [`MEDIA`].
const KNOWN: [&[&str]; 3] = [CODE, TEXT, MEDIA];

/// Source code.
const CODE: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "rb", "go", "java", "kt", "kts", "swift", "c", "h", "cc",
    "cpp", "hpp", "cs", "m", "mm", "php", "lua", "dart", "ex", "exs", "erl", "zig", "nix", "scala", "hs", "ml", "elm",
    "vue", "svelte", "sql", "sh", "bash", "zsh", "fish", "ps1", "proto", "graphql", "gql", "tf", "hcl", "gradle",
    "ipynb", "wasm",
];

/// Text, configuration and markup.
const TEXT: &[&str] = &[
    "md", "mdx", "markdown", "txt", "json", "jsonc", "toml", "yaml", "yml", "lock", "ini", "cfg", "conf", "env", "log",
    "xml", "csv", "tsv", "html", "htm", "css", "scss", "less", "patch", "diff",
];

/// Media and documents.
const MEDIA: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "pdf", "mp4", "webm", "mov", "doc", "docx", "odt", "rtf",
    "xls", "xlsx", "ods", "ppt", "pptx", "odp",
];

/// Commands a code span may hold: `cargo test.rs` is a command, not a file.
const COMMANDS: &[&str] = &[
    "npm", "pnpm", "yarn", "git", "cargo", "python", "node", "cd", "cat", "open",
];

/// The file a Markdown link's target names: a relative or absolute path, a local `file:` URL or a `sandbox:/` path,
/// with an optional line. None for web links and other schemes.
pub fn link_reference(target: &str) -> Option<FileReference> {
    file_reference(target)
}

/// The file a code span names, when its text reads as a path: `src/main.rs:12`, `README.md` or `./a b/c.txt`.
pub fn code_reference(text: &str) -> Option<FileReference> {
    let reference = file_reference(text)?;
    let path = reference.path.as_str();
    let extension = extension(path)?;
    if path.contains(['<', '>', '|', '`', '*', ';', '=', '(', ')', '{', '}', '$', '&']) || is_command(path) {
        return None;
    }
    if path.contains(char::is_whitespace) && !is_rooted(path) {
        return None;
    }
    let extension = extension.to_ascii_lowercase();
    if !path.contains('/') && !KNOWN.iter().any(|known| known.contains(&extension.as_str())) {
        return None;
    }
    Some(reference)
}

/// Where the file an answer names is, for the session's folder `root`. An agent often names a file by its name alone
/// (`demo.gif`, `side_pane.rs`), from a folder it moved to, or from another checkout it read, so the path
/// as written may lead nowhere, or to another copy of the file.
///
/// `touched` holds the absolute paths the session's tool calls named, newest first
/// ([`crate::file_origin::touched_paths`]); `list` lists a folder's files, as [`crate::files::list`] reads them. In order:
/// 1. a path from `/`, `~/` or `../` opens as written;
/// 2. the newest file the agent touched whose path ends with it, when it is outside `root`: the copy the agent used;
/// 3. the path as written in `root`, when a file is there;
/// 4. the newest touched file of step 2 in `root`; then the path under `.nexus/artifacts/`, where agents save media;
/// 5. the one file in `root`'s list whose path ends with it;
/// 6. the one file that does across the other checkouts the agent worked in ([`crate::file_origin::roots`]).
///
/// A file in `root` comes as a path from `root`, and one elsewhere as its absolute path. None when no file, or more
/// than one, matches: the path then opens as written, and Files says it is not found. Blocking: it reads the disk.
pub fn locate(root: &Path, path: &str, touched: &[String], list: impl Fn(&Path) -> Vec<String>) -> Option<String> {
    if is_rooted(path) && !path.starts_with("./") {
        return crate::output_files::resolve(root, path)
            .is_ok()
            .then(|| path.to_string());
    }
    let path = path.trim_start_matches("./");
    if path.is_empty() || path.split('/').any(|part| part == "..") {
        return None;
    }
    let tail = format!("/{path}");
    let touched_file = touched
        .iter()
        .find(|file| slashed(file).ends_with(&tail) && Path::new(file).is_file())
        .map(|file| (from_root(root, file), file));
    // The newest copy the agent touched is outside `root`: that copy, even when `root` has one too.
    if let Some((file, absolute)) = &touched_file
        && file.as_str() == absolute.as_str()
    {
        return Some(file.clone());
    }
    if crate::output_files::resolve(root, path).is_ok() {
        return Some(path.to_string());
    }
    if let Some((file, _)) = touched_file {
        return Some(file);
    }
    let artifact = format!("{}/{path}", crate::output_media::ARTIFACTS_DIR);
    if crate::output_files::resolve(root, &artifact).is_ok() {
        return Some(artifact);
    }
    let ends = |file: &String| file == path || file.ends_with(&tail);
    let mut found = list(root).into_iter().filter(ends);
    if let Some(first) = found.next() {
        return found.next().is_none().then_some(first);
    }
    let mut found = crate::file_origin::roots(root, touched).into_iter().flat_map(|other| {
        let files = list(&other);
        files.into_iter().filter(ends).map(move |file| other.join(file))
    });
    let first = found.next()?;
    found.next().is_none().then(|| first.to_string_lossy().into_owned())
}

/// `file` with `/` between its parts. A Windows tool call names `C:\repo\src\a.rs`: its `\` count as `/` there, so
/// the tail matches on every system. A `\` in a Unix file name stays one.
fn slashed(file: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(windows) {
        file.replace('\\', "/").into()
    } else {
        file.into()
    }
}

/// `file`, an absolute path, as a path from `root` when it is in it; else as it is.
fn from_root(root: &Path, file: &str) -> String {
    let inside = |root: &Path| {
        Path::new(file)
            .strip_prefix(root)
            .ok()
            .map(|relative| relative.to_path_buf())
    };
    inside(root)
        .or_else(|| root.canonicalize().ok().and_then(|root| inside(&root)))
        .map_or_else(|| file.to_string(), |relative| relative.to_string_lossy().into_owned())
}

/// Where a click opens `path`. `local`: the session runs on this computer, so the system app can open its files.
pub fn opens(path: &str, local: bool) -> Opens {
    let app_only = extension(path).is_some_and(|extension| APP_ONLY.contains(&extension.to_ascii_lowercase().as_str()));
    if local && app_only { Opens::App } else { Opens::Files }
}

/// The last part of a path: `side_pane.rs` for `src/workspace/side_pane.rs`.
pub fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

/// What a chip says: the file name, and its line after a colon.
pub fn chip_label(reference: &FileReference) -> String {
    match reference.line {
        Some(line) => format!("{}:{line}", file_name(&reference.path)),
        None => file_name(&reference.path).to_string(),
    }
}

/// The lines a chip's preview shows: from two lines above `line` (from 1), [`PEEK_LINES`] in all; the
/// file's first lines without one. Answers the first shown line's number and the lines, each cut to [`PEEK_WIDTH`]
/// characters. A line past the end shows the file's last lines.
pub fn peek_lines(text: &str, line: Option<u32>) -> (u32, Vec<String>) {
    let all: Vec<&str> = text.lines().collect();
    let wanted = line.map_or(1, |line| line.saturating_sub(2).max(1)) as usize;
    let first = wanted.min(all.len().saturating_sub(PEEK_LINES - 1).max(1));
    let shown = all
        .iter()
        .skip(first - 1)
        .take(PEEK_LINES)
        .map(|line| {
            let line = line.replace('\t', "    ");
            match line.char_indices().nth(PEEK_WIDTH) {
                Some((at, _)) => format!("{}…", &line[..at]),
                None => line,
            }
        })
        .collect();
    (first as u32, shown)
}

/// How many lines a chip's preview shows.
pub const PEEK_LINES: usize = 7;
/// How many characters of a line it shows.
pub const PEEK_WIDTH: usize = 72;

/// `(?:^|/)[^/]+\.([a-z][a-z\d]{0,11})$`: the file name's extension, when it has a stem and the extension starts with
/// a letter and is at most 12 letters and digits.
fn extension(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next()?;
    let (stem, extension) = name.rsplit_once('.')?;
    let mut chars = extension.chars();
    let starts = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    (!stem.is_empty() && starts && extension.len() <= 12 && chars.all(|c| c.is_ascii_alphanumeric()))
        .then_some(extension)
}

/// `^(?:npm|pnpm|yarn|git|cargo|python\d?|node|cd|cat|open)\s`
fn is_command(path: &str) -> bool {
    let Some((word, _)) = path.split_once(char::is_whitespace) else {
        return false;
    };
    let word = word.to_ascii_lowercase();
    let word = match word.strip_prefix("python") {
        Some(digit) if digit.len() == 1 && digit.chars().all(|c| c.is_ascii_digit()) => "python",
        _ => word.as_str(),
    };
    COMMANDS.contains(&word)
}

/// `^(?:\.{1,2}/|/|~/|[a-z]:/)`: a path that starts from somewhere, so a space in it is part of a name.
fn is_rooted(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("./")
        || path.starts_with("../")
        || path.starts_with('/')
        || path.starts_with("~/")
        || (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/')
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod worktree_tests;
