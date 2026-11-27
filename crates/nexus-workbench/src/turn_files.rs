//! The files one turn of a session changed, with the lines it added and removed, for the row of links at the top of
//! each answer. They come from the turn's checkpoint, not from the answer's text: git's numstat between the
//! checkpoint before the turn and the turn's own, as `Workbench::changes` diffs them. Free functions, so the GPUI app
//! reads them from git directly, as it reads its Changes tab.

use std::collections::HashMap;
use std::path::Path;

use crate::{changes_source, git};

/// What a turn did to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

/// One file a turn changed. `added` and `removed` are lines; both are 0 for a binary file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnFile {
    /// Relative to the session's folder; a rename's new path.
    pub path: String,
    pub added: u32,
    pub removed: u32,
    pub status: FileStatus,
}

/// The files the turn that ended at event `turn` changed in `dir`, in git's order (by path). An error when the turn
/// has no checkpoint, or no checkpoint before it.
pub fn turn_files(dir: &Path, thread_id: &str, turn: i64) -> Result<Vec<TurnFile>, String> {
    let names = git::refs(dir, &changes_source::checkpoint_prefix(thread_id))?;
    let (from, to) = changes_source::turn_refs(&names, turn)?;
    file_stats(dir, &from, &to)
}

/// The files that differ from `from` to `to` (commits or refs), renames found, paths relative to `dir`.
pub fn file_stats(dir: &Path, from: &str, to: &str) -> Result<Vec<TurnFile>, String> {
    let args = |kind: &'static str| {
        [
            "diff",
            kind,
            "-z",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--find-renames",
            "--relative",
            from,
            to,
            "--",
        ]
    };
    let statuses = statuses(&git::git(dir, &args("--name-status"))?);
    Ok(numstat(&git::git(dir, &args("--numstat"))?)
        .into_iter()
        .map(|(path, added, removed)| {
            let status = statuses.get(&path).copied().unwrap_or(FileStatus::Modified);
            TurnFile {
                path,
                added,
                removed,
                status,
            }
        })
        .collect())
}

/// `--numstat -z`'s output as (path, added, removed). A rename is `added\tremoved\t\0old\0new\0`; a binary file says `-`.
fn numstat(out: &str) -> Vec<(String, u32, u32)> {
    let mut fields = out.split('\0');
    let mut files = Vec::new();
    while let Some(entry) = fields.next() {
        let mut parts = entry.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let path = if path.is_empty() {
            // A rename: the old path, then the new one.
            let _old = fields.next();
            fields.next().unwrap_or_default().to_string()
        } else {
            path.to_string()
        };
        if path.is_empty() {
            continue;
        }
        files.push((path, added.parse().unwrap_or(0), removed.parse().unwrap_or(0)));
    }
    files
}

/// `--name-status -z`'s output by path: `M\0path\0`, or `R100\0old\0new\0` for a rename (or a copy).
fn statuses(out: &str) -> HashMap<String, FileStatus> {
    let mut fields = out.split('\0').filter(|field| !field.is_empty());
    let mut found = HashMap::new();
    while let Some(letter) = fields.next() {
        let status = match letter.chars().next() {
            Some('A') => FileStatus::Added,
            Some('D') => FileStatus::Deleted,
            Some('R') => FileStatus::Renamed,
            Some('C') => FileStatus::Added,
            _ => FileStatus::Modified,
        };
        if letter.starts_with(['R', 'C']) {
            fields.next();
        }
        if let Some(path) = fields.next() {
            found.insert(path.to_string(), status);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        git::git(dir, args).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn a_turns_files_come_from_its_checkpoint_and_the_one_before() {
        let dir = std::env::temp_dir().join(format!("nexus-turn-files-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        run(&dir, &["init", "-q"]);
        std::fs::write(dir.join("keep.txt"), "one\ntwo\nthree\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.join("gone.txt"), "bye\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(
            dir.join("old.rs"),
            "fn main() {\n    println!(\"same\");\n}\n// a long enough body\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let prefix = changes_source::checkpoint_prefix("t1");
        git::checkpoint(&dir, &format!("{prefix}{:010}", 0)).unwrap_or_else(|e| panic!("{e}"));
        // The first turn, ending at event 7: an edit, a new file, a delete, a rename and a binary file.
        std::fs::write(dir.join("keep.txt"), "one\n2\nthree\nfour\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.join("new.md"), "# Hi\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::remove_file(dir.join("gone.txt")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::rename(dir.join("old.rs"), dir.join("new.rs")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.join("pic.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap_or_else(|e| panic!("{e}"));
        git::checkpoint(&dir, &format!("{prefix}{:010}", 7)).unwrap_or_else(|e| panic!("{e}"));
        // The second turn, ending at event 12, touches one file only.
        std::fs::write(dir.join("new.md"), "# Hi\nthere\n").unwrap_or_else(|e| panic!("{e}"));
        git::checkpoint(&dir, &format!("{prefix}{:010}", 12)).unwrap_or_else(|e| panic!("{e}"));

        let file = |path: &str, added, removed, status| TurnFile {
            path: path.into(),
            added,
            removed,
            status,
        };
        let mut first = turn_files(&dir, "t1", 7).unwrap_or_else(|e| panic!("{e}"));
        first.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(
            first,
            vec![
                file("gone.txt", 0, 1, FileStatus::Deleted),
                file("keep.txt", 2, 1, FileStatus::Modified),
                file("new.md", 1, 0, FileStatus::Added),
                file("new.rs", 0, 0, FileStatus::Renamed),
                file("pic.bin", 0, 0, FileStatus::Added),
            ]
        );
        assert_eq!(
            turn_files(&dir, "t1", 12).unwrap_or_else(|e| panic!("{e}")),
            vec![file("new.md", 1, 0, FileStatus::Modified)]
        );
        // The folder before the first turn, and a turn never recorded, have no files to show.
        assert!(turn_files(&dir, "t1", 0).is_err());
        assert!(turn_files(&dir, "t1", 9).is_err());
        assert!(turn_files(&dir, "other", 7).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_numstat_and_name_status_with_renames() {
        assert_eq!(
            numstat("3\t1\ta.rs\0-\t-\timg.png\0"),
            vec![("a.rs".into(), 3, 1), ("img.png".into(), 0, 0)]
        );
        assert_eq!(
            numstat("2\t0\t\0old name.rs\0new name.rs\0"),
            vec![("new name.rs".into(), 2, 0)]
        );
        let status = statuses("M\0a.rs\0R087\0x.rs\0y.rs\0A\0n.md\0D\0d.txt\0");
        assert_eq!(status["a.rs"], FileStatus::Modified);
        assert_eq!(status["y.rs"], FileStatus::Renamed);
        assert_eq!(status["n.md"], FileStatus::Added);
        assert_eq!(status["d.txt"], FileStatus::Deleted);
        assert!(!status.contains_key("x.rs"));
    }
}
