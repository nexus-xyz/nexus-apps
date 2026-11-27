//! Codex's rollouts: the JSON-lines transcript of each conversation, `<home>/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::agents::FILE_CLOCK_SLACK;

/// The Codex conversation this run started: the newest rollout under `<home>/sessions` touched since `since` whose
/// first line (`session_meta`) has this working directory and a start (`payload.timestamp`) no earlier than `since`,
/// so another Codex still writing in the same folder is not taken for this one.
/// Later: reads Codex's on-disk rollout layout; if Codex moves it (see `codex migrate-rollouts`), this finds
/// nothing and the session just has no resume command.
pub fn codex_conversation(home: &Path, cwd: &Path, since: SystemTime) -> Option<String> {
    let since_utc = time::OffsetDateTime::from(since);
    let modified_since = since.checked_sub(FILE_CLOCK_SLACK).unwrap_or(since);
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let mut rollouts = Vec::new();
    // sessions/YYYY/MM/DD/rollout-….jsonl
    let mut dirs = vec![(home.join("sessions"), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() && depth < 3 {
                dirs.push((path, depth + 1));
            } else if let Ok(modified) = meta.modified()
                && modified >= modified_since
                && path.extension().is_some_and(|e| e == "jsonl")
            {
                rollouts.push((modified, path));
            }
        }
    }
    rollouts.sort();
    rollouts.into_iter().rev().find_map(|(_, path)| {
        let mut first = String::new();
        std::io::BufReader::new(std::fs::File::open(path).ok()?)
            .read_line(&mut first)
            .ok()?;
        let meta: Value = serde_json::from_str(&first).ok()?;
        let recorded = std::fs::canonicalize(meta.pointer("/payload/cwd")?.as_str()?).ok()?;
        let id = meta.pointer("/payload/id")?.as_str()?;
        let begun = time::OffsetDateTime::parse(
            meta.pointer("/payload/timestamp")?.as_str()?,
            &time::format_description::well_known::Rfc3339,
        )
        .ok()?;
        (recorded == cwd && is_uuid(id) && begun >= since_utc).then(|| id.to_string())
    })
}

/// Codex's rollout of conversation `id`: the file under `<home>/sessions` whose name ends `-<id>.jsonl`.
pub fn rollout(home: &Path, id: &str) -> Option<PathBuf> {
    let suffix = format!("-{id}.jsonl");
    let mut dirs = vec![(home.join("sessions"), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() && depth < 3 {
                dirs.push((path, depth + 1));
            } else if entry.file_name().to_string_lossy().ends_with(&suffix) {
                return Some(path);
            }
        }
    }
    None
}

/// A UUID in its hyphenated form only. `Uuid::try_parse` also takes the simple, braced and URN forms, and each of
/// those has another length than 36.
fn is_uuid(text: &str) -> bool {
    text.len() == 36 && uuid::Uuid::try_parse(text).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same answers as `nexus_control::agents::is_uuid`, which the CLI checked Codex's ids with before.
    #[test]
    fn uuid_is_the_hyphenated_form_only() {
        assert!(is_uuid("019a0000-0000-7000-8000-000000000001"));
        assert!(is_uuid("019A0000-0000-7000-8000-00000000000F"));
        for other in [
            "019a00000000700080000000000000001",
            "019a0000000070008000000000000001",
            "{019a0000-0000-7000-8000-000000000001}",
            "urn:uuid:019a0000-0000-7000-8000-000000000001",
            "019a0000-0000-7000-8000-00000000000g",
            "019a0000_0000-7000-8000-000000000001",
        ] {
            assert!(!is_uuid(other), "{other}");
        }
    }
}
