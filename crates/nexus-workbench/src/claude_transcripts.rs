//! Claude Code's own transcript of a conversation, kept where a resume finds it.
//!
//! Claude Code resumes a conversation from `<profile>/projects/<working folder as a name>/<id>.jsonl`. The profile is
//! `CLAUDE_CONFIG_DIR`, else `~/.claude`. The file is missing there when the profile changed between launches (the app
//! started from the dock, not a terminal, or another account), when the working folder moved, or when Claude Code's
//! cleanup removed it (`cleanupPeriodDays`, 30 days by default).
//!
//! So Nexus keeps a copy of each transcript at the end of each turn, under its own data folder, where Claude Code's
//! cleanup never looks: `<copies>/<session>/<id>.jsonl`. Before a resume, a transcript missing where Claude Code will
//! look is put back from the fullest one found: the other `~/.claude*` profiles, the app's account folders, the other
//! project folders, or Nexus's copy. The copy put back is new, so Claude Code's cleanup does not take it at once.
//! When nothing is found, the resume fails and [`crate::resume_recovery`] starts over from Nexus's log.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Where one run of Claude Code keeps its transcripts: its profile and working folder, and the home folder whose other
/// profiles are looked in too.
#[derive(Debug, Clone)]
pub(crate) struct Place {
    pub profile: PathBuf,
    pub cwd: PathBuf,
    pub home: Option<PathBuf>,
}

impl Place {
    /// The run's place, from the environment it is started with (`HOME`, `CLAUDE_CONFIG_DIR`). None when that names
    /// no profile folder.
    pub(crate) fn of(env: &std::collections::HashMap<String, String>, cwd: &Path) -> Option<Place> {
        let home = ["HOME", "USERPROFILE"]
            .iter()
            .find_map(|name| env.get(*name).filter(|home| !home.is_empty()))
            .map(PathBuf::from);
        let profile = crate::accounts::profile_dir(
            crate::event::SessionAgent::Claude,
            home.as_deref().unwrap_or(Path::new("")),
            &|name| env.get(name).cloned(),
        );
        profile.is_absolute().then(|| Place {
            profile,
            cwd: cwd.to_path_buf(),
            home,
        })
    }

    /// Where Claude Code looks for conversation `id`.
    pub(crate) fn expected(&self, id: &str) -> Option<PathBuf> {
        expected(&self.profile, &self.cwd, id)
    }

    /// The profiles to look in, this run's first.
    pub(crate) fn profiles(&self, accounts: Option<&Path>) -> Vec<PathBuf> {
        profiles(&self.profile, self.home.as_deref(), accounts)
    }
}

/// Claude Code cuts a longer folder name and adds a hash of the path, which only Claude Code computes.
const MAX_NAME: usize = 200;

/// Claude Code's folder name for a working folder: each character that is not an ASCII letter or digit becomes `-`.
pub fn folder_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The working folder as Claude Code reads it: with links resolved, as the system gives a process its folder.
fn real(cwd: &Path) -> PathBuf {
    if cfg!(windows) {
        cwd.to_path_buf()
    } else {
        std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf())
    }
}

/// The project folder Claude Code uses for `cwd` in `profile`. A name over [`MAX_NAME`] is found by its start, only
/// when one folder has it; None when none does.
fn project_dir(profile: &Path, cwd: &Path) -> Option<PathBuf> {
    let name = folder_name(&real(cwd));
    let projects = profile.join("projects");
    if name.len() <= MAX_NAME {
        return Some(projects.join(name));
    }
    let start = format!("{}-", &name[..MAX_NAME]);
    let mut found = std::fs::read_dir(&projects)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&start))
        .map(|entry| entry.path());
    let dir = found.next()?;
    found.next().is_none().then_some(dir)
}

/// Whether `id` can name a transcript file: letters, digits, `-` and `_` only, as Claude Code's ids (UUIDs) are. The id
/// comes from a stored session row and is joined to a folder, so one with `/` or `..` would reach outside it.
pub fn plain_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Where Claude Code looks for conversation `id` when it runs in `cwd` with `profile`. None for an id that is not
/// [`plain_id`].
pub fn expected(profile: &Path, cwd: &Path, id: &str) -> Option<PathBuf> {
    if !plain_id(id) {
        return None;
    }
    project_dir(profile, cwd).map(|dir| dir.join(format!("{id}.jsonl")))
}

/// Nexus's copy of conversation `id` of session `thread_id`.
pub fn copy_path(copies: &Path, thread_id: &str, id: &str) -> PathBuf {
    copies.join(thread_id).join(format!("{id}.jsonl"))
}

/// The profiles a transcript may be in: `first` (the launch's), then `~/.claude` and its siblings (`~/.claude-work`),
/// then the app's Claude accounts. Each once.
pub fn profiles(first: &Path, home: Option<&Path>, accounts: Option<&Path>) -> Vec<PathBuf> {
    let mut all = vec![first.to_path_buf()];
    let mut add = |dir: PathBuf| {
        if dir.is_dir() && !all.contains(&dir) {
            all.push(dir);
        }
    };
    if let Some(entries) = home.and_then(|home| std::fs::read_dir(home).ok()) {
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".claude"))
            .map(|entry| entry.path())
            .collect();
        found.sort();
        found.into_iter().for_each(&mut add);
    }
    if let Some(entries) = accounts.and_then(|root| std::fs::read_dir(root.join("claude")).ok()) {
        let mut found: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        found.sort();
        found.into_iter().for_each(&mut add);
    }
    all
}

/// Every transcript of conversation `id`: in each profile's project folders, and in Nexus's copies.
fn found(profiles: &[PathBuf], copies: Option<&Path>, id: &str) -> Vec<PathBuf> {
    let file = format!("{id}.jsonl");
    let mut dirs: Vec<PathBuf> = profiles.iter().map(|profile| profile.join("projects")).collect();
    dirs.extend(copies.map(Path::to_path_buf));
    dirs.iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flat_map(|entries| entries.flatten().map(|entry| entry.path().join(&file)))
        .filter(|path| path.is_file())
        .collect()
}

fn size_and_time(path: &Path) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
}

/// Copies `from` to `to` whole, through a file beside it, so no reader sees half a copy. A transcript is the whole
/// conversation: the copy is 0600, and a folder this makes for it 0700.
fn copy_whole(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(dir) = to.parent() {
        crate::private_fs::create_dir_all(dir)?;
    }
    let staged = to.with_extension(format!("jsonl.{}-{}", std::process::id(), crate::short_id()));
    let copied = std::fs::copy(from, &staged)
        .and_then(|_| private_mode(&staged))
        .and_then(|_| std::fs::rename(&staged, to));
    if copied.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    copied
}

#[cfg(unix)]
fn private_mode(file: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn private_mode(_file: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Before a resume: puts conversation `id` where Claude Code will look, from the fullest transcript found, when it is
/// not there. Returns where it came from, or None when nothing was needed or found, or the id is not [`plain_id`].
pub fn restore(
    expected: &Path,
    profiles: &[PathBuf],
    copies: Option<&Path>,
    id: &str,
) -> std::io::Result<Option<PathBuf>> {
    if !plain_id(id) || size_and_time(expected).is_some_and(|(len, _)| len > 0) {
        return Ok(None);
    }
    // The longest is the most of the conversation: Claude Code only appends to it. The newest breaks a tie.
    let Some(source) = found(profiles, copies, id)
        .into_iter()
        .filter(|path| path != expected)
        .filter_map(|path| size_and_time(&path).map(|meta| (meta, path)))
        .filter(|((len, _), _)| *len > 0)
        .max_by_key(|(meta, _)| *meta)
        .map(|(_, path)| path)
    else {
        return Ok(None);
    };
    copy_whole(&source, expected)?;
    Ok(Some(source))
}

/// After a turn: copies Claude Code's transcript of `id` to `copy`, when it changed since the last copy. The copies of
/// the session's earlier conversations go: a rewind or a switch left them. Returns whether it copied.
pub fn keep(profiles: &[PathBuf], expected: Option<&Path>, id: &str, copy: &Path) -> std::io::Result<bool> {
    if !plain_id(id) {
        return Ok(false);
    }
    let source = expected
        .filter(|path| path.is_file())
        .map(Path::to_path_buf)
        .or_else(|| {
            found(profiles, None, id)
                .into_iter()
                .max_by_key(|path| size_and_time(path))
        });
    let Some((source, (len, modified))) = source.and_then(|path| size_and_time(&path).map(|meta| (path, meta))) else {
        return Ok(false);
    };
    let unchanged = size_and_time(copy).is_some_and(|(kept, copied)| kept == len && copied >= modified);
    if !unchanged {
        copy_whole(&source, copy)?;
    }
    if let Some(dir) = copy.parent()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path != copy && path.extension().is_some_and(|ext| ext == "jsonl") {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(!unchanged)
}

/// Drops Nexus's copies of a session's conversations, with the session or its archiving.
pub fn forget(copies: &Path, thread_id: &str) {
    let dir = copies.join(thread_id);
    if dir.exists()
        && let Err(error) = std::fs::remove_dir_all(&dir)
    {
        eprintln!(
            "nexus-workbench: {thread_id}: could not remove {}: {error}",
            dir.display()
        );
    }
}

impl crate::Workbench {
    /// Before a resume: puts conversation `id` back where this run's Claude Code will look, when it is elsewhere.
    pub(crate) async fn restore_transcript(&self, thread_id: &str, place: Place, id: &str) {
        let config = self.config();
        let (thread_id, id) = (thread_id.to_string(), id.to_string());
        let restored = tokio::task::spawn_blocking(move || {
            let Some(want) = place.expected(&id) else {
                return Ok(None);
            };
            let profiles = place.profiles(config.accounts.as_deref());
            restore(&want, &profiles, config.transcripts.as_deref(), &id).map(|from| from.map(|from| (from, want)))
        })
        .await;
        match restored {
            Ok(Ok(Some((from, to)))) => eprintln!(
                "nexus-workbench: {thread_id}: put Claude Code's transcript back: {} to {}",
                from.display(),
                to.display()
            ),
            Ok(Ok(None)) => {}
            Ok(Err(error)) => eprintln!("nexus-workbench: {thread_id}: could not put the transcript back: {error}"),
            Err(error) => eprintln!("nexus-workbench: {thread_id}: {error}"),
        }
    }

    /// After a turn, and when the run ends: Nexus's copy of the run's conversation, in the background.
    pub(crate) fn keep_transcript(&self, thread_id: &str, live: &crate::Live) {
        let config = self.config();
        let Some(copies) = config.transcripts else {
            return;
        };
        let (place, id) = {
            let state = live.state();
            (state.transcript.clone(), state.connection.native_id.clone())
        };
        let (Some(place), Some(id)) = (place, id.filter(|id| plain_id(id))) else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let thread_id = thread_id.to_string();
        let accounts = config.accounts;
        runtime.spawn_blocking(move || {
            let copy = copy_path(&copies, &thread_id, &id);
            let profiles = place.profiles(accounts.as_deref());
            if let Err(error) = keep(&profiles, place.expected(&id).as_deref(), &id, &copy) {
                eprintln!("nexus-workbench: {thread_id}: could not copy the transcript: {error}");
            }
        });
    }

    /// Drops Nexus's copies of a session's conversations.
    pub(crate) fn forget_transcripts(&self, thread_id: &str) {
        if let Some(copies) = self.config().transcripts {
            forget(&copies, thread_id);
        }
    }
}

#[cfg(test)]
#[path = "claude_transcripts_tests.rs"]
mod tests;
