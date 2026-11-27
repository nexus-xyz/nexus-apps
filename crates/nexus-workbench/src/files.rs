//! The files of a session's folder, for the composer's @-mention picker: what git knows of (tracked, plus untracked
//! files that `.gitignore` does not hide), or, outside a repository, a walk of the folder that skips hidden folders and
//! the usual build output. Capped at [`MAX_FILES`], and kept for [`FRESH_FOR`] so typing after "@" (or reopening the
//! picker) doesn't run git again; after that the next call lists anew, so files an agent just wrote show up.

use crate::process;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// At most this many paths (T3 Code's index stops at 25k too); a bigger folder lists its first ones.
pub const MAX_FILES: usize = 25_000;
/// How long a folder's list is reused.
pub const FRESH_FOR: Duration = Duration::from_secs(10);

/// Folders a walk outside a repository never enters: hidden ones (`.git`, `.venv`) and these.
const SKIPPED: &[&str] = &["node_modules", "target", "dist", "build", "__pycache__"];

/// A folder's files, relative to it with `/` separators, sorted, and whether the list was cut at [`MAX_FILES`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileList {
    pub files: Arc<Vec<String>>,
    pub truncated: bool,
}

impl serde::Serialize for FileList {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("FileList", 2)?;
        out.serialize_field("files", self.files.as_slice())?;
        out.serialize_field("truncated", &self.truncated)?;
        out.end()
    }
}

type Cache = Mutex<HashMap<PathBuf, (Instant, FileList)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The files of `dir`, from the cache when it listed them less than [`FRESH_FOR`] ago. Blocking: run it off the
/// async runtime's workers.
pub fn list(dir: &Path) -> Result<FileList, String> {
    let now = Instant::now();
    if let Some((at, list)) = cache().lock().map_err(|e| e.to_string())?.get(dir)
        && now.duration_since(*at) < FRESH_FOR
    {
        return Ok(list.clone());
    }
    let list = read(dir)?;
    let mut cache = cache().lock().map_err(|e| e.to_string())?;
    // Old entries go when anything is listed, so folders of closed sessions don't stay in memory.
    cache.retain(|_, (at, _)| now.duration_since(*at) < FRESH_FOR);
    cache.insert(dir.to_path_buf(), (now, list.clone()));
    Ok(list)
}

/// Lists `dir` now, without the cache.
pub fn read(dir: &Path) -> Result<FileList, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a folder.", dir.display()));
    }
    let mut files = match git_files(dir) {
        Some(files) => files,
        None => walk(dir),
    };
    files.sort();
    files.dedup();
    let truncated = files.len() > MAX_FILES;
    cap(&mut files, MAX_FILES);
    Ok(FileList {
        files: Arc::new(files),
        truncated,
    })
}

/// Keeps at most `max` of the sorted `files`: the shallowest ones (then by name), so a big folder loses its deepest
/// files, not a whole alphabetical tail of folders. The result is sorted again.
fn cap(files: &mut Vec<String>, max: usize) {
    if files.len() <= max {
        return;
    }
    files.sort_by_cached_key(|path| (path.matches('/').count(), path.clone()));
    files.truncate(max);
    files.sort();
}

/// Tracked files and untracked ones not ignored, relative to `dir` (a folder deeper in a repository lists only
/// what is under it). None when `dir` is not in a repository or git can't run.
fn git_files(dir: &Path) -> Option<Vec<String>> {
    let (out, cut) = ls_files(dir, GIT_FOR, MAX_GIT_OUTPUT)?;
    // A file deleted but not yet committed is still "cached": only what is on the disk can be mentioned. An untracked
    // embedded repository comes as one entry ("vendor/lib/"), a folder: not a file.
    Some(pick(&out, cut, MAX_FILES, |path| dir.join(path).is_file()))
}

/// The paths of `git ls-files -z` output that exist, shallowest first, at most `max + 1` of them (one past the cap,
/// so the caller can tell the list was cut). `cut` says the output ended mid-way, so its last entry may be half a path.
fn pick(out: &[u8], cut: bool, max: usize, exists: impl Fn(&str) -> bool) -> Vec<String> {
    let text = String::from_utf8_lossy(out);
    let mut paths: Vec<&str> = text.split('\0').filter(|path| !path.is_empty()).collect();
    if cut {
        // The last one may be half a path.
        paths.pop();
    }
    // Only the shallowest ones are kept ([`cap`]), so only those are looked at on the disk: a repository of a million
    // files costs a sort, not a million file checks.
    paths.sort_unstable_by_key(|path| (path.matches('/').count(), *path));
    paths.dedup();
    paths
        .into_iter()
        .filter(|path| exists(path))
        .take(max + 1)
        .map(String::from)
        .collect()
}

/// How long `git ls-files` may take (a folder on a slow network disk, say) before the folder is walked instead.
const GIT_FOR: Duration = Duration::from_secs(10);
/// The most of `git ls-files`'s output read: about a million paths. Past it the kept files are the shallowest of what
/// was read, not of the repository: git prints untracked files first, then tracked ones in path order, so a shallow
/// file late in the alphabet can lose to deeper ones. The list is then marked as cut.
const MAX_GIT_OUTPUT: u64 = 64 * 1024 * 1024;

/// `git ls-files`'s output (at most `max` bytes, and whether it was cut there), or None when git fails or takes longer
/// than `limit` (then it is killed).
fn ls_files(dir: &Path, limit: Duration, max: u64) -> Option<(Vec<u8>, bool)> {
    use std::io::Read as _;
    let mut child = process::command("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.fsmonitor=false",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let (sent, read) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut stdout = stdout.take(max + 1);
        let ok = stdout.read_to_end(&mut out).is_ok();
        let _ = sent.send(ok.then_some(out));
    });
    // Nothing in time (or the read failed): None.
    let Some(mut out) = read.recv_timeout(limit).unwrap_or_default() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let cut = out.len() as u64 > max;
    if cut {
        out.truncate(max as usize);
        // Past the cap git may still be writing: it is not waited for.
        let _ = child.kill();
    }
    let status = child.wait().ok()?;
    (cut || status.success()).then_some((out, cut))
}

/// The files under `dir` outside a repository, breadth first, stopping just past [`MAX_FILES`].
fn walk(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut queue = std::collections::VecDeque::from([PathBuf::new()]);
    while let Some(relative) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(dir.join(&relative)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else { continue };
            let path = relative.join(&name);
            if kind.is_dir() {
                if !name.starts_with('.') && !SKIPPED.contains(&name.as_str()) {
                    queue.push_back(path);
                }
            } else if kind.is_file() {
                files.push(path.to_string_lossy().replace('\\', "/"));
                if files.len() > MAX_FILES {
                    return files;
                }
            }
        }
    }
    files
}

/// Whether the Files tool's filter keeps `path`: it holds the filter's text, in any case (`pane-files-model.ts`'s
/// `fileTree`). An empty or blank filter keeps every path.
pub fn matches_filter(path: &str, filter: &str) -> bool {
    let needle = filter.trim().to_lowercase();
    needle.is_empty() || path.to_lowercase().contains(&needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_filter_keeps_paths_that_hold_it_in_any_case() {
        let kept: Vec<_> = ["src/Store.ts", "src/view.ts", "README.md"]
            .into_iter()
            .filter(|path| matches_filter(path, "store"))
            .collect();
        assert_eq!(kept, ["src/Store.ts"]);
        assert!(matches_filter("a.rs", "  "));
        assert!(matches_filter("src/Lib.rs", " src/l "));
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-wb-files-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = process::command("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git")
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn a_repository_lists_tracked_and_untracked_files_but_not_ignored_ones() {
        let dir = temp("repo");
        git(&dir, &["init", "-q"]);
        fs::create_dir_all(dir.join("src")).expect("src");
        fs::create_dir_all(dir.join("node_modules/pkg")).expect("deps");
        fs::write(dir.join(".gitignore"), "node_modules/\n*.log\n").expect("ignore");
        fs::write(dir.join("src/main.rs"), "fn main() {}").expect("main");
        fs::write(dir.join("gone.txt"), "x").expect("gone");
        git(&dir, &["add", "."]);
        fs::remove_file(dir.join("gone.txt")).expect("rm");
        fs::write(dir.join("notes with space.md"), "new").expect("untracked");
        fs::write(dir.join("debug.log"), "ignored").expect("log");
        fs::write(dir.join("node_modules/pkg/index.js"), "ignored").expect("dep");

        let list = read(&dir).expect("list");
        assert_eq!(*list.files, vec![".gitignore", "notes with space.md", "src/main.rs"]);
        assert!(!list.truncated);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plain_folder_is_walked_without_hidden_or_build_folders() {
        let dir = temp("plain");
        fs::create_dir_all(dir.join("a/b")).expect("a/b");
        fs::create_dir_all(dir.join(".cache")).expect("hidden");
        fs::create_dir_all(dir.join("node_modules")).expect("deps");
        fs::write(dir.join("a/b/c.txt"), "").expect("c");
        fs::write(dir.join("top.txt"), "").expect("top");
        fs::write(dir.join(".env"), "").expect("dotfile");
        fs::write(dir.join(".cache/x"), "").expect("x");
        fs::write(dir.join("node_modules/y.js"), "").expect("y");

        let list = read(&dir).expect("list");
        assert_eq!(*list.files, vec![".env", "a/b/c.txt", "top.txt"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_list_is_reused_while_fresh() {
        let dir = temp("cache");
        fs::write(dir.join("one.txt"), "").expect("one");
        let first = list(&dir).expect("first");
        fs::write(dir.join("two.txt"), "").expect("two");
        let second = list(&dir).expect("second");
        assert!(
            Arc::ptr_eq(&first.files, &second.files),
            "the cached list, not a new one"
        );
        assert_eq!(*read(&dir).expect("fresh").files, vec!["one.txt", "two.txt"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_over_the_cap_loses_its_deepest_files_not_an_alphabetical_tail() {
        let mut files: Vec<String> = ["a/deep/er/x.rs", "a/y.rs", "b.md", "z/w.rs", "zz.md"]
            .into_iter()
            .map(String::from)
            .collect();
        files.sort();
        cap(&mut files, 3);
        assert_eq!(files, vec!["a/y.rs", "b.md", "zz.md"]);
        let mut small = vec!["a".to_string(), "b/c".to_string()];
        cap(&mut small, 5);
        assert_eq!(small, vec!["a", "b/c"]);
    }

    #[test]
    fn an_untracked_embedded_repository_is_not_listed_as_a_file() {
        let dir = temp("embedded");
        git(&dir, &["init", "-q"]);
        fs::write(dir.join("top.txt"), "x").expect("top");
        let inner = dir.join("vendor/lib");
        fs::create_dir_all(&inner).expect("inner");
        git(&inner, &["init", "-q"]);
        fs::write(inner.join("lib.rs"), "x").expect("lib");

        let list = read(&dir).expect("list");
        assert_eq!(*list.files, vec!["top.txt"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_output_past_the_cap_is_cut_and_its_half_path_dropped() {
        let dir = temp("cut");
        git(&dir, &["init", "-q"]);
        for name in ["aaaa.txt", "bbbb.txt", "cccc.txt"] {
            fs::write(dir.join(name), "x").expect("file");
        }
        let (whole, cut) = ls_files(&dir, GIT_FOR, MAX_GIT_OUTPUT).expect("ls-files");
        assert!(!cut);
        assert_eq!(whole, b"aaaa.txt\0bbbb.txt\0cccc.txt\0");
        // 12 bytes: "aaaa.txt\0bbb", the second path cut short.
        let (part, cut) = ls_files(&dir, GIT_FOR, 12).expect("ls-files");
        assert!(cut);
        assert_eq!(part, b"aaaa.txt\0bbb");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The pipeline before `pick` was split out: every path checked on the disk, then sorted, deduped, cut.
    fn old_pipeline(paths: &[&str], exists: impl Fn(&str) -> bool, max: usize) -> (Vec<String>, bool) {
        let mut files: Vec<String> = paths.iter().filter(|p| exists(p)).map(|p| p.to_string()).collect();
        files.sort_by_key(|path| (path.matches('/').count(), path.clone()));
        files.dedup();
        let truncated = files.len() > max;
        files.truncate(max);
        files.sort();
        (files, truncated)
    }

    fn kept(out: &[u8], cut: bool, max: usize, exists: impl Fn(&str) -> bool) -> (Vec<String>, bool) {
        let mut files = pick(out, cut, max, exists);
        files.sort();
        files.dedup();
        let truncated = files.len() > max;
        cap(&mut files, max);
        (files, truncated)
    }

    #[test]
    fn picking_matches_checking_every_path_when_some_are_missing_and_there_are_too_many() {
        let paths = [
            "z.md",
            "a/b/c/d.rs",
            "a/b.rs",
            "gone/one.rs",
            "m/n.rs",
            "a/b.rs",
            "top.md",
            "gone.txt",
            "q/r/s.rs",
            "x/y.rs",
            "k.md",
            "w/gone.rs",
            "b/c/d/e.rs",
        ];
        let exists = |p: &str| !p.contains("gone");
        let out: Vec<u8> = paths.iter().flat_map(|p| p.bytes().chain([0])).collect();
        for max in [1, 5, 7, 9, 20] {
            assert_eq!(
                kept(&out, false, max, exists),
                old_pipeline(&paths, exists, max),
                "max {max}"
            );
        }
        // Missing files must not use up the room: 9 exist, so max 8 is over the cap and 9 is exactly at it.
        assert!(kept(&out, false, 8, exists).1);
        assert!(!kept(&out, false, 9, exists).1);
    }

    #[test]
    fn a_cut_output_loses_its_last_entry_and_a_whole_one_keeps_all() {
        let all = |_: &str| true;
        assert_eq!(pick(b"a.txt\0b.txt\0c", true, 10, all), vec!["a.txt", "b.txt"]);
        assert_eq!(
            pick(b"a.txt\0b.txt\0c.txt\0", false, 10, all),
            vec!["a.txt", "b.txt", "c.txt"]
        );
        assert_eq!(
            pick(b"a.txt\0b.txt\0c.txt", false, 10, all),
            vec!["a.txt", "b.txt", "c.txt"]
        );
    }

    #[test]
    fn a_missing_folder_says_so() {
        assert!(read(Path::new("/no/such/folder/here")).is_err());
    }
}
