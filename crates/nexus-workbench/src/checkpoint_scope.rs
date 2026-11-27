//! Which untracked files a checkpoint ([`crate::git::snapshot`]) takes. A checkpoint runs before a session's first
//! turn and after each turn, and the next turn waits for it, so it must never hash a build's output: `git add --all`
//! once spent minutes on a 159 GB cargo target folder while the session showed "Starting session…".
//!
//! The rules are the same for every checkpoint, so a revert compares like with like:
//! - A folder with a `CACHEDIR.TAG` (cargo's target folders, many tools' caches) stays out, whole.
//! - A file over [`MAX_FILE`] stays out.
//! - More than [`MAX_ENTRIES`] untracked entries, or more than [`MAX_BYTES`] to add, fails the checkpoint: a partial
//!   one would depend on the order the files were found in.
//!
//! What stays out is noted in the checkpoint's commit message ([`note`]). A revert never deletes it ([`kept`]).

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

/// The largest untracked file a checkpoint takes.
pub(crate) const MAX_FILE: u64 = 16 << 20;
/// The most a checkpoint adds in untracked files, all together.
pub(crate) const MAX_BYTES: u64 = 512 << 20;
/// The most untracked files and folders a checkpoint looks at.
pub(crate) const MAX_ENTRIES: usize = 50_000;

/// How a `CACHEDIR.TAG` starts (<https://bford.info/cachedir/>).
const CACHEDIR_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";
/// The commit-message line that lists what a checkpoint left out, as a JSON array of paths relative to its folder.
const NOTE_KEY: &str = "Nexus-skipped: ";

/// The untracked paths a checkpoint adds, and those it leaves out (a folder ends with `/`).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Scope {
    pub(crate) add: Vec<String>,
    pub(crate) skipped: Vec<String>,
}

/// Which of the paths it gets `.gitignore` files ignore (`git check-ignore`).
pub(crate) type Ignored<'a> = dyn FnMut(&[String]) -> Result<Vec<String>, String> + 'a;

/// Whether `dir` holds a valid `CACHEDIR.TAG`.
pub(crate) fn is_cache_dir(dir: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; CACHEDIR_SIGNATURE.len()];
    std::fs::File::open(dir.join("CACHEDIR.TAG"))
        .and_then(|mut file| file.read_exact(&mut head))
        .is_ok_and(|()| head == CACHEDIR_SIGNATURE)
}

/// The scope of a checkpoint of `dir`. `listed` is `git ls-files --others --exclude-standard --directory -z`: files,
/// and folders (ending with `/`) that git did not enter. This walks those folders level by level. `ignored` gets each
/// level's new paths and returns those `.gitignore` files ignore (`git check-ignore`). Past `deadline` it fails.
pub(crate) fn untracked(
    dir: &Path,
    listed: &str,
    ignored: &mut Ignored<'_>,
    deadline: Instant,
) -> Result<Scope, String> {
    let mut scope = Scope::default();
    let mut bytes = 0u64;
    let mut entries = 0usize;
    let mut level: Vec<String> = listed
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(String::from)
        .collect();
    while !level.is_empty() {
        let mut next = Vec::new();
        for entry in level {
            entries += 1;
            if entries + next.len() > MAX_ENTRIES {
                return Err(too_many());
            }
            if Instant::now() >= deadline {
                return Err(too_slow());
            }
            let Some(folder) = entry.strip_suffix('/') else {
                let size = std::fs::symlink_metadata(dir.join(&entry)).map_or(0, |meta| meta.len());
                if size > MAX_FILE {
                    scope.skipped.push(entry);
                    continue;
                }
                bytes += size;
                if bytes > MAX_BYTES {
                    return Err(too_big());
                }
                scope.add.push(entry);
                continue;
            };
            let path = dir.join(folder);
            // A repository inside is one entry (a gitlink), as `git add` takes it.
            if path.join(".git").exists() {
                scope.add.push(folder.to_string());
                continue;
            }
            if is_cache_dir(&path) {
                scope.skipped.push(entry);
                continue;
            }
            let Ok(children) = std::fs::read_dir(&path) else {
                continue;
            };
            for child in children.flatten() {
                // A name that is not UTF-8 stays out of every checkpoint alike, so no revert touches it.
                let Ok(name) = child.file_name().into_string() else {
                    continue;
                };
                // A link to a folder is a file to git: it is not followed.
                let is_dir = child.file_type().is_ok_and(|kind| kind.is_dir());
                next.push(format!("{folder}/{name}{}", if is_dir { "/" } else { "" }));
            }
        }
        if next.is_empty() {
            break;
        }
        let ignored: HashSet<String> = ignored(&next)?.into_iter().collect();
        level = next.into_iter().filter(|path| !ignored.contains(path)).collect();
    }
    Ok(scope)
}

/// The commit-message line that records `skipped`, or None when nothing was left out.
pub(crate) fn note(skipped: &[String]) -> Option<String> {
    (!skipped.is_empty()).then(|| format!("{NOTE_KEY}{}", serde_json::Value::from(skipped.to_vec())))
}

/// The paths a checkpoint's commit `message` says it left out ([`note`]).
pub(crate) fn skipped_in(message: &str) -> Vec<String> {
    message
        .lines()
        .find_map(|line| line.strip_prefix(NOTE_KEY))
        .and_then(|list| serde_json::from_str(list).ok())
        .unwrap_or_default()
}

/// Whether a revert to a checkpoint that left out `skipped` must keep `path`: the checkpoint never looked at it, so its
/// absence there does not mean the turn made it.
pub(crate) fn kept(path: &str, skipped: &[String]) -> bool {
    skipped.iter().any(|left_out| match left_out.strip_suffix('/') {
        Some(folder) => path == folder || path.starts_with(left_out.as_str()),
        None => path == left_out,
    })
}

fn too_many() -> String {
    format!(
        "This folder has more than {MAX_ENTRIES} untracked files that .gitignore does not cover, too many to snapshot \
         for the turn's changes. Add build and cache folders to .gitignore."
    )
}

fn too_big() -> String {
    format!(
        "This folder's untracked files that .gitignore does not cover come to more than {} MB, too much to snapshot \
         for the turn's changes. Add build and cache folders to .gitignore.",
        MAX_BYTES >> 20
    )
}

pub(crate) fn too_slow() -> String {
    "Snapshotting this folder for the turn's changes took too long, so this turn's changes are not recorded. Add \
     build and cache folders to .gitignore."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn folder(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("nexus-scope-{name}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).unwrap_or_else(|e| panic!("{e}"));
        root
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{e}"));
        }
        std::fs::write(path, bytes).unwrap_or_else(|e| panic!("{e}"));
    }

    fn later() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    #[test]
    fn walks_folders_and_leaves_out_caches_big_files_and_ignored_paths() {
        let root = folder("walk");
        write(&root.join("top.txt"), b"top");
        write(&root.join("new/a.txt"), b"a");
        write(&root.join("new/deep/b.txt"), b"b");
        write(&root.join("new/skip.log"), b"log");
        write(&root.join("new/target/CACHEDIR.TAG"), CACHEDIR_SIGNATURE);
        write(&root.join("new/target/debug/huge.o"), b"object");
        write(&root.join("new/fake/CACHEDIR.TAG"), b"not a tag");
        write(&root.join("new/fake/kept.txt"), b"kept");
        std::fs::File::create(root.join("new/big.bin"))
            .and_then(|file| file.set_len(MAX_FILE + 1))
            .unwrap_or_else(|e| panic!("{e}"));
        let mut asked = Vec::new();
        let mut ignored = |paths: &[String]| {
            asked.extend(paths.iter().cloned());
            Ok(paths.iter().filter(|path| path.ends_with(".log")).cloned().collect())
        };
        let mut scope = untracked(&root, "top.txt\0new/\0", &mut ignored, later()).unwrap_or_else(|e| panic!("{e}"));
        scope.add.sort();
        scope.skipped.sort();
        assert_eq!(
            scope.add,
            [
                "new/a.txt",
                "new/deep/b.txt",
                "new/fake/CACHEDIR.TAG",
                "new/fake/kept.txt",
                "top.txt"
            ]
        );
        assert_eq!(scope.skipped, ["new/big.bin", "new/target/"]);
        assert!(
            !asked.iter().any(|path| path.starts_with("new/target/debug")),
            "{asked:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_repository_inside_is_one_entry() {
        let root = folder("nested");
        write(&root.join("sub/.git"), b"gitdir: elsewhere\n");
        write(&root.join("sub/file.txt"), b"x");
        let scope = untracked(&root, "sub/\0", &mut |_| Ok(Vec::new()), later()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(scope.add, ["sub"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn too_many_files_or_too_late_fails_rather_than_taking_part() {
        let root = folder("limits");
        for index in 0..=MAX_ENTRIES {
            write(&root.join(format!("many/{index}")), b"");
        }
        let error = untracked(&root, "many/\0", &mut |_| Ok(Vec::new()), later()).err();
        assert_eq!(error, Some(too_many()));
        let error = untracked(&root, "many/\0", &mut |_| Ok(Vec::new()), Instant::now()).err();
        assert_eq!(error, Some(too_slow()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_note_round_trips_and_keeps_what_was_left_out() {
        let skipped = vec!["target/".to_string(), "big.bin".to_string()];
        let message = format!("Nexus checkpoint\n\n{}\n", note(&skipped).unwrap_or_default());
        let read = skipped_in(&message);
        assert_eq!(read, skipped);
        assert!(kept("target", &read));
        assert!(kept("target/debug/x.o", &read));
        assert!(kept("big.bin", &read));
        assert!(!kept("targets/x", &read));
        assert!(!kept("big.bin.txt", &read));
        assert_eq!(note(&[]), None);
        assert!(skipped_in("Nexus checkpoint").is_empty());
    }
}
