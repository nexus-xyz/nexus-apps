//! Where a session's agent worked: the absolute paths its tool calls named.
//!
//! An answer can name a file by its name alone after the agent read it in another checkout, such as a sibling git
//! worktree. The calls still hold the full path: Claude Code's `Read` and `Edit` take `file_path`, Codex's file
//! changes become `Write` and `Edit` with `file_path`, and a command names its paths in `command`. `file_refs::locate`
//! reads these paths to open the copy the agent used, not a file with the same name in the session's folder.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The input fields that hold one path: Claude Code's file tools, `NotebookEdit`, and `Grep` and `Glob`'s folder.
const PATH_FIELDS: &[&str] = &["file_path", "notebook_path", "path"];

/// At most this many other folders are searched for a file the answer names ([`roots`]).
pub const MAX_ROOTS: usize = 4;

/// The absolute paths a tool call's `input` names: its path fields, and each absolute path in its command. A path is
/// absolute from `/`; a relative one has no folder to read it from, so it is left out.
pub fn touched_paths(input: &Value) -> Vec<String> {
    let mut paths: Vec<String> = PATH_FIELDS
        .iter()
        .filter_map(|field| input.get(*field).and_then(Value::as_str))
        .filter_map(clean)
        .collect();
    if let Some(command) = input.get("command").and_then(Value::as_str) {
        paths.extend(command_paths(command));
    }
    paths
}

/// The absolute paths a shell command names: `sed -n 1,80p '/w/b/src/a.rs'` names `/w/b/src/a.rs`, and
/// `cd /w/b && rg x` names `/w/b`.
fn command_paths(command: &str) -> Vec<String> {
    command
        .split(|c: char| c.is_whitespace() || "'\"`=;|&()<>".contains(c))
        .filter_map(clean)
        .collect()
}

/// `token` as an absolute path, without a trailing line (`:12`) or punctuation; None for anything else. On Windows a
/// path starts with its drive (`C:\repo\src\a.rs`), whose `:` is not a line.
fn clean(token: &str) -> Option<String> {
    let token = token.trim_end_matches([',', ':', '.']);
    let (drive, rest) = match token.as_bytes() {
        [letter, b':', ..] if cfg!(windows) && letter.is_ascii_alphabetic() => token.split_at(2),
        _ => ("", token),
    };
    let rest = match rest.split_once(':') {
        Some((path, line)) if line.chars().all(|c| c.is_ascii_digit() || c == ':') => path,
        _ => rest,
    };
    let token = &token[..drive.len() + rest.len()];
    let absolute = if cfg!(windows) {
        Path::new(token).is_absolute()
    } else {
        token.len() > 1 && token.starts_with('/') && !token.starts_with("//")
    };
    (absolute && !token.split(['/', '\\']).any(|part| part == "..")).then(|| token.to_string())
}

/// The folders other than `root` the agent worked in, as the top of each one's git checkout, newest first: at most
/// [`MAX_ROOTS`]. A folder that holds `root` or sits inside it is left out, so the session's own repository is never
/// listed again. `touched` comes newest first. Blocking: it reads the disk.
pub fn roots(root: &Path, touched: &[String]) -> Vec<PathBuf> {
    let own = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut found: Vec<PathBuf> = Vec::new();
    for path in touched {
        let Some(top) = checkout(Path::new(path)) else {
            continue;
        };
        if top.starts_with(&own) || own.starts_with(&top) || found.contains(&top) {
            continue;
        }
        found.push(top);
        if found.len() == MAX_ROOTS {
            break;
        }
    }
    found
}

/// The top of the git checkout that holds `path`: the nearest folder with a `.git` entry, a folder in a clone or a
/// file in a worktree. Canonical, so two spellings of one folder compare equal.
fn checkout(path: &Path) -> Option<PathBuf> {
    let path = path.canonicalize().ok()?;
    path.ancestors()
        .find(|folder| folder.join(".git").exists())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests;
