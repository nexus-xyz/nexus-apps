//! A project's git repository: its branch, read without running git, and the worktrees sessions work in (`git`).

use crate::checkpoint_scope;
use crate::gitlab;
use crate::process;
use crate::source_host::Host;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// The checked-out branch of the repository at `path` (a worktree too), or None for a detached HEAD or no repository.
pub fn branch(path: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir(path)?.join("HEAD")).ok()?;
    head.trim().strip_prefix("ref: refs/heads/").map(String::from)
}

/// Whether `path` is the top folder of a repository (or of a worktree): it has a `.git` of its own.
pub fn is_repo(path: &Path) -> bool {
    git_dir(path).is_some()
}

/// The top folder of the repository a project's folder is in: the folder itself, or the nearest folder above it with
/// a `.git`, found without running git. A folder below the top must be a part of it as [`in_repo`] says (git asks,
/// for that case only): not ignored, and not below a repository at the home folder. None outside any repository.
pub fn project_top(path: &Path) -> Option<PathBuf> {
    if is_repo(path) {
        return Some(path.to_path_buf());
    }
    let top = path.ancestors().skip(1).find(|dir| is_repo(dir))?;
    in_repo(path).then(|| top.to_path_buf())
}

/// What a project shows as its branch: the branch checked out in the repository its folder is in ([`project_top`]),
/// or the short id of the commit checked out on a detached HEAD. None only outside any repository: both apps read
/// None as "not a git project", so a detached HEAD or a folder deeper in a repository must not give None.
pub fn project_branch(path: &Path) -> Option<String> {
    let top = project_top(path)?;
    branch(&top).or_else(|| {
        let head = std::fs::read_to_string(git_dir(&top)?.join("HEAD")).ok()?;
        let id = head.trim();
        (id.len() >= 7 && id.chars().all(|c| c.is_ascii_hexdigit())).then(|| id[..7].to_string())
    })
}

/// Whether `dir` is in a repository, at its top or deeper (git asks), as a part of it: not a folder the repository
/// ignores, and not a folder below a repository at the home folder (a dotfiles repository is not the project's, and
/// its checkpoints would fill that repository with the project's files).
pub fn in_repo(dir: &Path) -> bool {
    let Ok(top) = git(dir, &["rev-parse", "--show-toplevel"]) else {
        return false;
    };
    let prefix = git(dir, &["rev-parse", "--show-prefix"]).unwrap_or_default();
    if prefix.is_empty() {
        return !top.is_empty();
    }
    let home = std::env::var_os("HOME").and_then(|home| std::fs::canonicalize(home).ok());
    if home.is_some_and(|home| std::fs::canonicalize(&top).is_ok_and(|top| top == home)) {
        return false;
    }
    git(Path::new(&top), &["check-ignore", "--quiet", "--", &prefix]).is_err()
}

/// The index of the repository `dir` is in, a folder deeper in it too.
fn index_of(dir: &Path) -> Option<PathBuf> {
    let path = PathBuf::from(git(dir, &["rev-parse", "--git-path", "index"]).ok()?);
    Some(if path.is_absolute() { path } else { dir.join(path) })
}

/// The branch checked out where `dir` is, a folder deeper in a repository too (git asks; `branch` reads only a `.git`
/// beside it). None for a detached HEAD, or outside any repository.
pub fn checked_out(dir: &Path) -> Option<String> {
    git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .filter(|name| !name.is_empty())
}

/// `.git` as a folder, or the folder a worktree's `.git` file points to (`gitdir: …`).
fn git_dir(path: &Path) -> Option<PathBuf> {
    let dot_git = path.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let target = text.trim().strip_prefix("gitdir:")?.trim();
    let target = Path::new(target);
    Some(if target.is_absolute() {
        target.to_path_buf()
    } else {
        path.join(target)
    })
}

/// The most remotes `remotes` reads from one repository.
const MAX_REMOTES: usize = 16;

/// The fetch URLs of the repository at `path` (a worktree too), `origin` first, read from its `config` without running
/// git. A prompt link names its repository by one of them (the desktop's `prompt-link.ts`). Credentials in a URL
/// (`https://user:token@…`) are taken out: the list goes to the webview. Includes and `insteadOf` are not followed.
pub fn remotes(path: &Path) -> Vec<String> {
    config_text(path).map(|config| remotes_in(&config)).unwrap_or_default()
}

/// The text of the `config` file of the repository at `path` (a worktree too), read without running git.
pub(crate) fn config_text(path: &Path) -> Option<String> {
    let dir = git_dir(path)?;
    // A worktree's own folder holds `commondir`, the way to the repository's folder, where `config` is.
    let common = std::fs::read_to_string(dir.join("commondir"))
        .map(|text| dir.join(text.trim()))
        .unwrap_or(dir);
    std::fs::read_to_string(common.join("config")).ok()
}

/// The last value of `key` in the `[section]` (no subsection) of a git config file's text; names are
/// case-insensitive, as git reads them.
pub(crate) fn config_value(config: &str, section: &str, key: &str) -> Option<String> {
    let mut inside = false;
    let mut found = None;
    for line in config.lines() {
        let line = line.trim();
        if let Some(header) = line.strip_prefix('[') {
            inside = header
                .split(']')
                .next()
                .is_some_and(|name| name.trim().eq_ignore_ascii_case(section));
            continue;
        }
        if !inside {
            continue;
        }
        if let Some((name, value)) = line.split_once('=')
            && name.trim().eq_ignore_ascii_case(key)
        {
            found = Some(value.trim().trim_matches('"').to_string());
        }
    }
    found
}

/// The remotes' URLs in a git config file's text, `origin` first, then in the file's order.
fn remotes_in(config: &str) -> Vec<String> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut section: Option<String> = None;
    for line in config.lines() {
        let line = line.trim();
        if let Some(header) = line.strip_prefix('[') {
            // `[remote "origin"]`: the section's kind is case-insensitive, its name is not.
            let header = header.split(']').next().unwrap_or_default();
            section = header
                .split_once(char::is_whitespace)
                .filter(|(kind, _)| kind.eq_ignore_ascii_case("remote"))
                .map(|(_, name)| name.trim().trim_matches('"').to_string());
            continue;
        }
        let Some(name) = &section else { continue };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        if key.trim().eq_ignore_ascii_case("url")
            && !value.is_empty()
            && !found.iter().any(|(other, _)| other == name)
            && found.len() < MAX_REMOTES
        {
            found.push((name.clone(), without_credentials(value)));
        }
    }
    found.sort_by_key(|(name, _)| name != "origin");
    found.into_iter().map(|(_, url)| url).collect()
}

/// `url` without the user and password of a `scheme://user:password@host/…` URL. `git@host:path` has no secret and stays.
fn without_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority = rest.split('/').next().unwrap_or_default();
    match authority.rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => url.to_string(),
    }
}

/// git in `dir`: its output, trimmed, or what it said went wrong. Never prompts for credentials, and never takes the
/// index lock just to refresh its stat cache (`GIT_OPTIONAL_LOCKS=0`): the agent may be running git in the same folder,
/// and its `git add` or `git commit` would fail on the lock.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_with(dir, args, &[])
}

pub(crate) fn git_with(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
    let text = git_untrimmed(dir, args, env)?;
    // A `-z` list is NUL-separated names, kept as they are: a name may start or end with a space.
    Ok(if args.contains(&"-z") {
        text
    } else {
        text.trim().to_string()
    })
}

/// git in `dir`, its output as git wrote it: what a patch reader needs. A patch's last line may be a context line
/// that holds a single space, and trimming it leaves the last hunk one line short of its header's counts.
/// `core.fsmonitor` is turned off: a repository's own `.git/config` could otherwise name a program that `status` or
/// `diff` runs. Each output is kept to [`GIT_LISTING_CAP`].
fn git_untrimmed(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
    let mut command = process::command("git");
    command
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .envs(env.iter().copied());
    let out = output_capped(command).map_err(|error| format!("Could not run git: {error}"))?;
    if !out.success {
        return Err(out.stderr.trim().to_string());
    }
    Ok(out.stdout)
}

/// The most of each output (stdout, stderr) a command's run keeps. Past it the rest is read and dropped, so the
/// command never stalls on a full pipe and a command that prints forever can't grow memory without end.
pub(crate) const OUTPUT_CAP: usize = 4 * 1024 * 1024;

/// [`OUTPUT_CAP`] for git's own listings of a repository (`ls-files`, `status`, `diff-tree`), which grow with the
/// repository: a large one lists more than 4 MiB of paths, and a list cut short would leave files out of a checkpoint.
/// Still a bound, so no output grows memory without end.
pub(crate) const GIT_LISTING_CAP: usize = 64 * 1024 * 1024;

/// Reads `pipe` to its end, keeping at most `cap` bytes: the rest is read and dropped so the writer never blocks.
fn read_capped(mut pipe: impl std::io::Read, cap: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..n.min(room)]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    kept
}

/// Runs `command` to its end with nothing on its input, each output kept to [`GIT_LISTING_CAP`] (stderr on a thread of
/// its own, so neither pipe can fill and stall it).
fn output_capped(mut command: std::process::Command) -> std::io::Result<Ran> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stderr = child
        .stderr
        .take()
        .map(|pipe| std::thread::spawn(move || read_capped(pipe, GIT_LISTING_CAP)));
    let stdout = child
        .stdout
        .take()
        .map(|pipe| read_capped(pipe, GIT_LISTING_CAP))
        .unwrap_or_default();
    let stderr = stderr.and_then(|reader| reader.join().ok()).unwrap_or_default();
    let status = child.wait()?;
    Ok(Ran {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// What a new worktree starts from when none is named: the remote's default branch (`origin/main`), else the branch
/// checked out in the repository, else (a detached HEAD) the commit checked out.
pub fn default_base(repo: &Path) -> Option<String> {
    git(
        repo,
        &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .filter(|base| !base.is_empty())
    .or_else(|| checked_out(repo))
    .or_else(|| git(repo, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]).ok())
}

/// Whether `base` is a full commit id rather than a branch: a pull request can't target it, so it is not recorded as
/// the branch's `gh-merge-base`.
fn is_commit_id(base: &str) -> bool {
    base.len() >= 40 && base.chars().all(|c| c.is_ascii_hexdigit())
}

/// Fails unless `value` is safe to hand git as a ref, a remote or a branch name: never empty, never starting with `-`
/// or holding a part that does (git would read it as an option, and `--upload-pack=<cmd>` runs a program), and within the rules of
/// `git check-ref-format --allow-onelevel`. A full commit id passes. `what` names it in the message.
pub(crate) fn check_ref_arg(what: &str, value: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("\"{value}\" is not a valid {what}: {why}."));
    if value.is_empty() {
        return bad("it is empty");
    }
    if value.starts_with('-') {
        return bad("it starts with -");
    }
    // `origin/-x` splits into a remote and a name, and the name goes to git on its own.
    if value.split('/').any(|part| part.starts_with('-')) {
        return bad("a part starts with -");
    }
    if value.chars().any(|c| c.is_control() || " ~^:?*[\\".contains(c)) {
        return bad("it has a character git doesn't allow");
    }
    if value == "@" || value.contains("@{") {
        return bad("it has @{ or is @");
    }
    if value.contains("..") {
        return bad("it has ..");
    }
    if value.starts_with('/') || value.ends_with('/') || value.contains("//") {
        return bad("it has an empty part between slashes");
    }
    if value.ends_with('.')
        || value
            .split('/')
            .any(|part| part.starts_with('.') || part.ends_with(".lock"))
    {
        return bad("a part starts with . or ends with . or .lock");
    }
    Ok(())
}

/// Fails with words a person understands when `base` names no commit because the repository has none yet (git alone
/// says "invalid reference: main").
fn has_commits(repo: &Path, base: &str) -> Result<(), String> {
    let found = |name: &str| {
        git(
            repo,
            &["rev-parse", "--verify", "--quiet", &format!("{name}^{{commit}}")],
        )
        .is_ok()
    };
    if !found(base) && !found("HEAD") {
        // An unborn HEAD (a fresh `git checkout --orphan`) is no proof the repository is empty: other branches may have commits.
        let any_ref = git(
            repo,
            &[
                "for-each-ref",
                "--count=1",
                "--format=%(refname)",
                "refs/heads",
                "refs/remotes",
            ],
        )
        .is_ok_and(|refs| !refs.is_empty());
        return Err(if any_ref {
            format!(
                "{base} has no commits yet, so there is nothing to start a worktree from. Pick another branch, or make a first commit on it."
            )
        } else {
            format!(
                "{} has no commits yet, so there is nothing to start a worktree from. Make a first commit, then try again.",
                repo.display()
            )
        });
    }
    Ok(())
}

/// One lock per repository (its common git folder) for changes to its worktrees. Two `git worktree add`s at once in
/// one repository can fail: one reads the other's half-made `.git/worktrees/<name>/` ("failed to read …/commondir"),
/// and two `git config` writes can find `.git/config` locked. A crew's lead starts its children together, so this
/// app makes worktrees at once. Never held during a fetch.
static WORKTREE_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = LazyLock::new(Default::default);

fn worktree_lock(repo: &Path) -> Arc<Mutex<()>> {
    let common = git(repo, &["rev-parse", "--git-common-dir"])
        .map(|dir| repo.join(dir.trim()))
        .unwrap_or_else(|_| repo.to_path_buf());
    let key = std::fs::canonicalize(&common).unwrap_or(common);
    let mut locks = WORKTREE_LOCKS.lock().unwrap_or_else(|p| p.into_inner());
    locks.entry(key).or_default().clone()
}

/// Runs `change` to the repository's worktrees under [`worktree_lock`].
pub(crate) fn changing_worktrees<T>(repo: &Path, change: impl FnOnce() -> T) -> T {
    let lock = worktree_lock(repo);
    let _held = lock.lock().unwrap_or_else(|p| p.into_inner());
    change()
}

/// `git worktree add`, tried once more when it met another process's half-made worktree (a person's own git, or a
/// second app), which [`worktree_lock`] cannot hold back.
fn worktree_add(repo: &Path, args: &[&str]) -> Result<String, String> {
    match git(repo, args) {
        Err(error) if error.contains("commondir") => git(repo, args),
        done => done,
    }
}

/// Takes away a worktree the app just made, and its branch, whatever is in it: for a step that failed after it.
pub fn discard_worktree(repo: &Path, dir: &Path, branch: &str) {
    changing_worktrees(repo, || {
        let _ = git(repo, &["worktree", "remove", "--force", &dir.to_string_lossy()]);
        let _ = git(repo, &["branch", "-D", branch]);
    });
}

/// How long a new worktree waits for its base's fetch. Past it the worktree is not made: a remote that hangs would
/// otherwise hold the new session (a linked one with it) for good.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// Creates `branch` from `base` in a new worktree at `dir`. A remote base is fetched first, within [`FETCH_TIMEOUT`];
/// offline, the copy already here is used. The base is recorded as `branch.<branch>.gh-merge-base`, as T3 Code does,
/// so `gh` compares against it.
pub fn add_worktree(repo: &Path, dir: &Path, branch: &str, base: &str) -> Result<(), String> {
    add_worktree_within(repo, dir, branch, base, FETCH_TIMEOUT)
}

fn add_worktree_within(repo: &Path, dir: &Path, branch: &str, base: &str, deadline: Duration) -> Result<(), String> {
    check_ref_arg("branch name", branch)?;
    git(repo, &["check-ref-format", "--branch", branch])
        .map_err(|_| format!("{branch} is not a valid branch name."))?;
    check_ref_arg("base", base)?;
    let remotes = git(repo, &["remote"]).unwrap_or_default();
    let merge_base = match base.split_once('/') {
        Some((remote, name)) if remotes.lines().any(|line| line == remote) => {
            check_ref_arg("remote", remote)?;
            check_ref_arg("branch name", name)?;
            fetch_within(repo, remote, name, deadline)?;
            name
        }
        _ => base,
    };
    has_commits(repo, base)?;
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    changing_worktrees(repo, || {
        worktree_add(
            repo,
            &["worktree", "add", "-b", branch, "--", &dir.to_string_lossy(), base],
        )?;
        if !is_commit_id(merge_base) {
            let _ = git(repo, &["config", &format!("branch.{branch}.gh-merge-base"), merge_base]);
        }
        Ok(())
    })
}

/// `git fetch <remote> <name>` in `repo`, given up after `deadline` with the fetch and all it started killed. A fetch
/// that fails (offline, say) is no error: the copy already here is used. Its output is not read, so a process it
/// left holding a pipe can't hold this up.
fn fetch_within(repo: &Path, remote: &str, name: &str, deadline: Duration) -> Result<(), String> {
    let mut command = process::command("git");
    command
        .arg("-C")
        .arg(repo)
        .args(["-c", "core.fsmonitor=false", "fetch", "--quiet", "--", remote, name])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Its own process group, so a kill reaches the transport it started (ssh, upload-pack).
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let Ok(mut child) = command.spawn() else {
        return Ok(());
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return Ok(()),
            Ok(None) if started.elapsed() >= deadline => {
                // The whole group: git and the transport it started.
                process::kill(Some(child.id()));
                let _ = child.wait();
                return Err(format!(
                    "Fetching {name} from {remote} took longer than {} seconds, so the worktree wasn't made. Check \
                     the connection to {remote}, then try again.",
                    deadline.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Checks out pull request `number` of `origin` in a new worktree at `dir`, on a new local `branch` (its head as it is
/// now), with `base` (`main`, the branch it targets) fetched and recorded as the branch's `gh-merge-base`, so the diff
/// compares against what the pull request does. GitHub keeps a pull request's head at `pull/<n>/head`, GitLab a merge
/// request's at `refs/merge-requests/<n>/head`.
pub fn add_review_worktree(
    repo: &Path,
    dir: &Path,
    branch: &str,
    number: u64,
    base: &str,
    tool_path: &str,
) -> Result<(), String> {
    let host = Host::of_repo(repo);
    check_ref_arg("branch name", branch)?;
    git(repo, &["check-ref-format", "--branch", branch])
        .map_err(|_| format!("{branch} is not a valid branch name."))?;
    check_ref_arg("base", base)?;
    if !git(repo, &["remote"])
        .unwrap_or_default()
        .lines()
        .any(|line| line == "origin")
    {
        return Err(format!(
            "Reviewing a {} needs the repository's {} remote as origin.",
            host.request(),
            host.name()
        ));
    }
    // Into a remote-tracking ref, not just FETCH_HEAD: the pull request's commits then count as pushed (`--remotes`), so
    // the worktree can be removed later without a push the review session is not allowed to make.
    let (source, tracking) = match host {
        Host::GitHub => (
            format!("pull/{number}/head"),
            format!("refs/remotes/origin/pr/{number}"),
        ),
        Host::GitLab => (
            format!("refs/merge-requests/{number}/head"),
            format!("refs/remotes/origin/mr/{number}"),
        ),
    };
    git_timed(
        repo,
        &["fetch", "--quiet", "--", "origin", &format!("+{source}:{tracking}")],
        FETCH_TIMEOUT,
    )
    .map_err(|error| format!("Could not fetch {} {}: {error}", host.request(), host.reference(number)))?;
    let head = git(repo, &["rev-parse", &tracking])?;
    // Offline or a base that is gone: the copy of it already here, if any, is enough for the diff.
    let _ = git_timed(repo, &["fetch", "--quiet", "--", "origin", base], FETCH_TIMEOUT);
    crate::clone::complete_history(repo, tool_path, FETCH_TIMEOUT)?;
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    changing_worktrees(repo, || {
        worktree_add(
            repo,
            &["worktree", "add", "-b", branch, "--", &dir.to_string_lossy(), &head],
        )
    })?;
    let _ = git(repo, &["config", &format!("branch.{branch}.gh-merge-base"), base]);
    Ok(())
}

/// The commit checked out in `dir`, in full.
pub fn head_commit(dir: &Path) -> Option<String> {
    git(dir, &["rev-parse", "HEAD"]).ok()
}

/// Whether `repo` has a local branch `name`.
pub fn has_branch(repo: &Path, name: &str) -> bool {
    git(
        repo,
        &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{name}")],
    )
    .is_ok()
}

/// An open pull request of the repository, as the GitHub CLI lists it (or a GitLab merge request, as `glab` does).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestInfo {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// The author's GitHub (or GitLab) login.
    pub author: String,
    /// Its branch.
    pub head: String,
    /// The branch it targets.
    pub base: String,
    /// Where it lives; absent: GitHub.
    #[serde(default, skip_serializing_if = "Host::is_github")]
    pub host: Host,
}

/// How long a GitHub (or GitLab) CLI call may take before it is given up on (and killed with what it started).
pub const GH_LIMIT: Duration = Duration::from_secs(60);

/// The host's CLI in `dir` (`path` finds it, as for [`pull_request`]): its output, or what went wrong in words for a
/// person.
fn cli(host: Host, dir: &Path, path: &str, args: &[&str]) -> Result<String, String> {
    cli_within(
        host,
        Path::new(host.program()),
        dir,
        path,
        args,
        GH_LIMIT,
        &host.needs("This"),
    )
}

/// `program` (the host's CLI) in `dir`, within `limit`; `missing` is the answer when it cannot be run. Prompts are off
/// for both tools: `gh` reads `GH_PROMPT_DISABLED`, `glab` reads `NO_PROMPT`.
fn cli_within(
    host: Host,
    program: &Path,
    dir: &Path,
    path: &str,
    args: &[&str],
    limit: Duration,
    missing: &str,
) -> Result<String, String> {
    let mut command = process::command(program);
    command
        .args(args)
        .current_dir(dir)
        .env("PATH", path)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_PROMPT", "true")
        .env("GIT_TERMINAL_PROMPT", "0");
    let out = output_within(command, limit)
        .map_err(|_| missing.to_string())?
        .ok_or_else(|| format!("{} didn't answer within {}. Try again.", host.name(), seconds(limit)))?;
    if out.success {
        Ok(out.stdout.trim().to_string())
    } else {
        Err(host.error(&out.stderr))
    }
}

const PR_FIELDS: &str = "number,title,url,author,headRefName,baseRefName";

fn pr_info(value: &serde_json::Value) -> Option<PullRequestInfo> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Some(PullRequestInfo {
        number: value.get("number")?.as_u64()?,
        title: text("title"),
        url: text("url"),
        author: value
            .get("author")
            .and_then(|author| author.get("login"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        head: text("headRefName"),
        base: text("baseRefName"),
        host: Host::GitHub,
    })
}

/// The repository's open pull requests, most recent first (at most 30).
pub fn pull_requests(dir: &Path, path: &str) -> Result<Vec<PullRequestInfo>, String> {
    let host = Host::of_repo(dir);
    let args: &[&str] = match host {
        Host::GitHub => &["pr", "list", "--state", "open", "--limit", "30", "--json", PR_FIELDS],
        Host::GitLab => gitlab::LIST,
    };
    let out = cli(host, dir, path, args)?;
    if out.is_empty() {
        return Ok(vec![]);
    }
    let list: Vec<serde_json::Value> = serde_json::from_str(&out).map_err(|error| error.to_string())?;
    Ok(list
        .iter()
        .filter_map(|value| match host {
            Host::GitHub => pr_info(value),
            Host::GitLab => gitlab::info(value),
        })
        .collect())
}

/// Pull request `number` of the repository, open or not.
pub fn pull_request_info(dir: &Path, number: u64, path: &str) -> Result<PullRequestInfo, String> {
    let host = Host::of_repo(dir);
    let number_text = number.to_string();
    let found = match host {
        Host::GitHub => {
            let out = cli(host, dir, path, &["pr", "view", &number_text, "--json", PR_FIELDS])?;
            let value: serde_json::Value = serde_json::from_str(&out).map_err(|error| error.to_string())?;
            pr_info(&value)
        }
        Host::GitLab => {
            let out = cli(host, dir, path, &gitlab::view(Some(&number_text)))?;
            let value: serde_json::Value = serde_json::from_str(&out).map_err(|error| error.to_string())?;
            gitlab::info(&value)
        }
    };
    found.ok_or_else(|| {
        format!(
            "{} did not describe {} {}.",
            host.program(),
            host.request(),
            host.reference(number)
        )
    })
}

/// How long the cleanup's look at a pull request may take ([`merged_head`]).
const MERGED_LIMIT: Duration = Duration::from_secs(20);

/// The head commit of the pull request of the branch checked out in `dir` (or of pull request `number`, a review's),
/// when it merged. None: it has none, it is open or closed unmerged, or the host's CLI can't say.
pub fn merged_head(dir: &Path, path: &str, number: Option<u64>) -> Option<String> {
    let host = Host::of_repo(dir);
    let number = number.map(|number| number.to_string());
    let program = Path::new(host.program());
    match host {
        Host::GitHub => {
            let mut args = vec!["pr", "view"];
            args.extend(number.as_deref());
            args.extend(["--json", "state,headRefOid"]);
            let out = cli_within(host, program, dir, path, &args, MERGED_LIMIT, "").ok()?;
            merged_head_of(&out)
        }
        Host::GitLab => {
            // `glab mr view` finds only an open merge request for the branch, so a merged one is listed by its branch.
            let branch = match &number {
                Some(_) => String::new(),
                None => branch(dir)?,
            };
            let args = match &number {
                Some(number) => gitlab::view(Some(number)),
                None => gitlab::merged_of_branch(&branch),
            };
            let out = cli_within(host, program, dir, path, &args, MERGED_LIMIT, "").ok()?;
            gitlab::merged_head(&serde_json::from_str(&out).ok()?)
        }
    }
}

fn merged_head_of(out: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(out).ok()?;
    if value.get("state")?.as_str()? != "MERGED" {
        return None;
    }
    let head = value.get("headRefOid")?.as_str()?;
    (!head.is_empty()).then(|| head.to_string())
}

/// The branch-name prefix new worktrees start with unless the person sets another (Settings › Worktrees).
pub const BRANCH_PREFIX: &str = "nexus/";

/// Fails, saying why, unless `prefix` and a name after it make a branch git takes (`git check-ref-format --branch`).
/// Empty is fine: the names have no prefix.
pub fn check_branch_prefix(prefix: &str) -> Result<(), String> {
    let name = format!("{prefix}x");
    let bad = |why: &str| Err(format!("\"{prefix}\" can't start a branch name: {why}."));
    if prefix.len() > 60 {
        return bad("keep it to 60 characters");
    }
    if let Some(c) = prefix
        .chars()
        .find(|c| c.is_ascii_control() || " ~^:?*[\\".contains(*c))
    {
        return match c {
            ' ' => bad("it has a space"),
            c if c.is_ascii_control() => bad("it has a control character"),
            c => bad(&format!("git doesn't allow {c} in branch names")),
        };
    }
    if name.starts_with('-') {
        return bad("it starts with -");
    }
    if name.starts_with('/') || name.contains("//") {
        return bad("it has an empty part between slashes");
    }
    if name.contains("..") {
        return bad("it has ..");
    }
    if name.contains("@{") {
        return bad("it has @{");
    }
    if name
        .split('/')
        .any(|part| part.starts_with('.') || part.ends_with(".lock"))
    {
        return bad("a part starts with . or ends with .lock");
    }
    Ok(())
}

/// Creates `branch` in a new worktree at `dir` holding the files of the snapshot `checkpoint` ([`checkpoint`]): the
/// branch starts at the commit the snapshot was taken on, and what the snapshot adds to it is left as uncommitted
/// changes, as it was in the folder then. `base` is recorded as in [`add_worktree`].
pub fn fork_worktree(repo: &Path, dir: &Path, branch: &str, checkpoint: &str, base: &str) -> Result<(), String> {
    check_ref_arg("branch name", branch)?;
    git(repo, &["check-ref-format", "--branch", branch])
        .map_err(|_| format!("{branch} is not a valid branch name."))?;
    check_ref_arg("base", base)?;
    let start = git(repo, &["rev-parse", "--verify", "--quiet", &format!("{checkpoint}^")])
        .map_err(|_| "That point has no commit to start a branch from.".to_string())?;
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    changing_worktrees(repo, || {
        worktree_add(
            repo,
            &["worktree", "add", "-b", branch, "--", &dir.to_string_lossy(), &start],
        )
    })?;
    let files = git(dir, &["read-tree", "-u", "--reset", checkpoint]).and_then(|_| git(dir, &["reset", "-q"]));
    if let Err(error) = files {
        discard_worktree(repo, dir, branch);
        return Err(error);
    }
    let remotes = git(repo, &["remote"]).unwrap_or_default();
    let merge_base = match base.split_once('/') {
        Some((remote, name)) if remotes.lines().any(|line| line == remote) => name,
        _ => base,
    };
    if !is_commit_id(merge_base) {
        changing_worktrees(repo, || {
            let _ = git(repo, &["config", &format!("branch.{branch}.gh-merge-base"), merge_base]);
        });
    }
    Ok(())
}

/// What [`move_to_worktree`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Moved {
    /// What the new branch counts against: the branch checked out in the repository, else (a detached HEAD) its commit.
    pub base: String,
    /// The snapshot of what moved ([`checkpoint`]'s kind of commit): a copy of the work that never goes with a folder.
    pub snapshot: String,
    /// Why the project's folder still holds some of the changes, when it could not be cleaned. They are in the new
    /// worktree all the same.
    pub left: Option<String>,
}

/// Moves what isn't committed in `repo` (staged, unstaged and untracked files; not ignored ones) into a new worktree at
/// `dir`, on a new `branch` from `repo`'s HEAD, and leaves `repo` clean. What was staged stays staged. Until the
/// changes are checked in the new worktree, a failed step takes the worktree and its branch away and leaves `repo` as
/// it was. Refused mid-merge or mid-rebase, and when a repository inside `repo` changed (only its commit would move,
/// not its files).
pub fn move_to_worktree(repo: &Path, dir: &Path, branch: &str) -> Result<Moved, String> {
    check_ref_arg("branch name", branch)?;
    git(repo, &["check-ref-format", "--branch", branch])
        .map_err(|_| format!("{branch} is not a valid branch name."))?;
    if operation_in_progress(repo) {
        return Err(
            "A merge, rebase or cherry-pick is under way in the project's folder. Finish or abort it first.".into(),
        );
    }
    let head = git(repo, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .map_err(|_| "The project's folder has no commit yet to start a worktree from.".to_string())?;
    let base = checked_out(repo).unwrap_or_else(|| head.clone());
    let snapshot = snapshot(repo)?;
    // The person's own index, as a tree: what is staged moves staged.
    let staged = git(repo, &["write-tree"])?;
    let changed = git(repo, &["diff-tree", "-r", "--no-renames", "-z", &head, &snapshot])?;
    // `:<old mode> <new mode> <old id> <new id> <status>`, then the path: 160000 is a repository's commit.
    let gitlink = |field: &str| {
        field
            .strip_prefix(':')
            .is_some_and(|meta| meta.split(' ').take(2).any(|mode| mode == "160000"))
    };
    if changed.split('\0').any(gitlink) {
        return Err(
            "A repository inside the project's folder has changes, and only its commit would move, not its files. \
             Commit them in it first."
                .into(),
        );
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    changing_worktrees(repo, || {
        worktree_add(
            repo,
            &["worktree", "add", "-b", branch, "--", &dir.to_string_lossy(), &head],
        )
    })?;
    let carried = (|| {
        git(dir, &["read-tree", "-u", "--reset", &snapshot])?;
        git(dir, &["read-tree", &staged])?;
        let _ = git(dir, &["update-index", "-q", "--refresh"]);
        // Checked, not assumed: the new folder holds what the old one did, the staged part too.
        if tree_of(dir, &self::snapshot(dir)?)? != tree_of(repo, &snapshot)? || git(dir, &["write-tree"])? != staged {
            return Err("The changes did not arrive whole in the new worktree.".to_string());
        }
        // Nothing may have changed in the project's folder since the snapshot: the clean below would lose it.
        if tree_of(repo, &self::snapshot(repo)?)? != tree_of(repo, &snapshot)? {
            return Err("Files in the project's folder changed during the move. Try again.".to_string());
        }
        Ok(())
    })();
    if let Err(error) = carried {
        discard_worktree(repo, dir, branch);
        return Err(format!(
            "The session didn't move, and the project's folder is as it was: {error}"
        ));
    }
    if !is_commit_id(&base) {
        changing_worktrees(repo, || {
            let _ = git(repo, &["config", &format!("branch.{branch}.gh-merge-base"), &base]);
        });
    }
    // The project's folder back to its HEAD: the index first, then the files the snapshot changed. The work is safe in
    // the new worktree by now, so a failure here says what stayed rather than undo the move.
    let left = git(repo, &["reset", "-q"])
        .and_then(|_| revert_files(repo, &head))
        .err();
    Ok(Moved { base, snapshot, left })
}

/// The tree of `commit`.
fn tree_of(dir: &Path, commit: &str) -> Result<String, String> {
    git(dir, &["rev-parse", &format!("{commit}^{{tree}}")])
}

/// Points `refname` at `commit`.
pub fn set_ref(dir: &Path, refname: &str, commit: &str) -> Result<(), String> {
    git(dir, &["update-ref", refname, commit]).map(|_| ())
}

/// Removes a worktree, unless it holds work that exists nowhere else: uncommitted changes, or commits on no remote.
/// Its branch stays.
pub fn remove_worktree(repo: &Path, dir: &Path) -> Result<(), String> {
    remove_worktree_beyond(repo, dir, None)
}

/// [`remove_worktree`], with the commits `pushed` reaches counted as pushed too ([`would_lose_work_beyond`]).
pub fn remove_worktree_beyond(repo: &Path, dir: &Path, pushed: Option<&str>) -> Result<(), String> {
    if dir.exists() {
        would_lose_work_beyond(dir, pushed)?;
    }
    drop_worktree(repo, dir)
}

/// [`remove_worktree`] without its check for work: for a caller that made it. git still refuses a folder that is not
/// one of the repository's worktrees.
pub fn drop_worktree(repo: &Path, dir: &Path) -> Result<(), String> {
    drop_worktree_with_force(repo, dir, false)
}

pub(crate) fn drop_worktree_with_force(repo: &Path, dir: &Path, discard_uncommitted: bool) -> Result<(), String> {
    if !dir.exists() {
        return changing_worktrees(repo, || git(repo, &["worktree", "prune"])).map(drop);
    }
    let path = dir.to_string_lossy();
    let mut args = vec!["worktree", "remove", path.as_ref()];
    // git refuses to remove a worktree with submodules unless forced; `would_lose_work` checked them already.
    if discard_uncommitted || dir.join(".gitmodules").exists() {
        args.push("--force");
    }
    changing_worktrees(repo, || git(repo, &args)).map(drop)
}

/// Whether `dir` is still a worktree git can read: its `.git` names a folder that exists. A `git worktree remove` that
/// could not delete every file (on Windows, one a program holds open) still drops that folder, and leaves the rest.
pub fn is_worktree(dir: &Path) -> bool {
    git_dir(dir).is_some_and(|dir| dir.exists())
}

/// Fails, saying why, when removing the worktree at `dir` would lose work: uncommitted changes, or commits on no remote,
/// in it or in its submodules (a worktree's submodules live inside it, and go with it).
pub fn would_lose_work(dir: &Path) -> Result<(), String> {
    would_lose_work_beyond(dir, None)
}

/// The delete confirmation in Desktop offers to discard the changes reported by this error.
pub const WORKTREE_UNCOMMITTED: &str = "This worktree has uncommitted changes. Commit or discard them first.";
pub(crate) const WORKTREE_UNPUSHED: &str = "This worktree has commits that aren't pushed. Push them first.";

/// [`would_lose_work`], with the commits `pushed` reaches counted as pushed too: a merged pull request's head, which
/// GitHub keeps after a squash merge deleted its branch.
pub fn would_lose_work_beyond(dir: &Path, pushed: Option<&str>) -> Result<(), String> {
    // Every submodule looked into, whatever `.gitmodules` or the config says: `--force` would delete what they hide.
    if !git(dir, &["status", "--porcelain", "--ignore-submodules=none"])?.is_empty() {
        return Err(WORKTREE_UNCOMMITTED.into());
    }
    let unpushed = "git log --oneline HEAD --not --remotes";
    let mut log = vec!["log", "--oneline", "HEAD", "--not", "--remotes"];
    log.extend(pushed);
    if !git(dir, &log)?.is_empty()
        || !git(dir, &["submodule", "foreach", "--quiet", "--recursive", unpushed])?.is_empty()
    {
        return Err(WORKTREE_UNPUSHED.into());
    }
    Ok(())
}

/// Deletes the local branch `branch` unless one of its commits is on no remote and no other local branch; false when
/// it stays for that reason, or because git refuses to delete it. A branch that is already gone counts as deleted.
pub fn delete_branch_if_safe(repo: &Path, branch: &str) -> Result<bool, String> {
    let head = format!("refs/heads/{branch}");
    if git(repo, &["rev-parse", "--verify", "--quiet", &head]).is_err() {
        return Ok(true);
    }
    // `--exclude` before `--branches` takes the name without `refs/heads/`.
    let exclude = format!("--exclude={branch}");
    let only_here = git(
        repo,
        &["log", "--oneline", &head, "--not", "--remotes", &exclude, "--branches"],
    )?;
    if !only_here.is_empty() {
        return Ok(false);
    }
    // A branch git will not delete (checked out in the project's folder, say) stays: that must not fail the delete of
    // a session, for the same error would come back on every retry.
    Ok(git(repo, &["branch", "-D", branch]).is_ok())
}

/// Snapshots `dir`'s files, tracked and untracked but not ignored, as a commit at `refname`, through a scratch index:
/// the person's own index, files and branches are untouched. The commit's parent is HEAD. The scratch index starts as a
/// copy of the person's (for its file stat cache); `add` then makes it match the disk, so a file staged but deleted
/// drops out. Untracked files go in as [`checkpoint_scope`] says: never a cache folder or a big file. It gives up after
/// [`SNAPSHOT_LIMIT`], for the session's next turn waits for it.
pub fn checkpoint(dir: &Path, refname: &str) -> Result<(), String> {
    let commit = snapshot(dir)?;
    git(dir, &["update-ref", refname, &commit]).map(|_| ())
}

/// How long a checkpoint may take. A session's first turn starts only after its checkpoint, so this bounds that wait.
pub(crate) const SNAPSHOT_LIMIT: Duration = Duration::from_secs(20);

/// [`checkpoint`]'s commit, under no ref.
pub(crate) fn snapshot(dir: &Path) -> Result<String, String> {
    snapshot_until(dir, Instant::now() + SNAPSHOT_LIMIT)
}

fn snapshot_until(dir: &Path, deadline: Instant) -> Result<String, String> {
    let index = std::env::temp_dir().join(format!("nexus-checkpoint-{}", uuid::Uuid::new_v4().simple()));
    let index_path = index.to_string_lossy().into_owned();
    // Checkpoints are the app's, not the person's: a fixed author, so a missing user.name never stops one.
    let env = [
        ("GIT_INDEX_FILE", index_path.as_str()),
        ("GIT_AUTHOR_NAME", "Nexus"),
        ("GIT_AUTHOR_EMAIL", "checkpoint@nexus.invalid"),
        ("GIT_COMMITTER_NAME", "Nexus"),
        ("GIT_COMMITTER_EMAIL", "checkpoint@nexus.invalid"),
    ];
    let result = (|| {
        let run = |args: &[&str], input: Option<Vec<u8>>| git_until(dir, args, &env, input, deadline);
        let head = git(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok();
        // Start from a copy of the real index: it remembers each file's size and time, so `add` only rehashes files
        // that changed instead of the whole tree. Without one (or if it cannot be copied), start from HEAD.
        let seeded = index_of(dir).is_some_and(|real| seed_index(&real, &index));
        if seeded {
            unhide_entries(dir, &env)?;
        } else if let Some(head) = &head {
            git_with(dir, &["read-tree", head], &env)?;
        }
        if let Err(error) = run(&["add", "--update", "--", "."], None) {
            // With nothing tracked here yet, git says `.` matched nothing: there is nothing to update.
            let tracked = run(&["ls-files", "--cached", "-z", "--", "."], None)?;
            if !tracked.is_empty() {
                return Err(error);
            }
        }
        let listed = run(
            &[
                "ls-files",
                "--others",
                "--exclude-standard",
                "--directory",
                "-z",
                "--",
                ".",
            ],
            None,
        )?;
        let mut ignored = |paths: &[String]| {
            let input = paths.join("\0").into_bytes();
            let out = run_until(dir, &["check-ignore", "--stdin", "-z"], &env, Some(input), deadline)?;
            // 1: none of them is ignored.
            if !out.success && !out.stderr.is_empty() {
                return Err(out.stderr.trim().to_string());
            }
            Ok(out
                .stdout
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(String::from)
                .collect())
        };
        let scope = checkpoint_scope::untracked(dir, &listed, &mut ignored, deadline)?;
        if !scope.add.is_empty() {
            let add = [
                "--literal-pathspecs",
                "add",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ];
            if run(&add, Some(scope.add.join("\0").into_bytes())).is_err() {
                // A file went while the folder was read: add what is still there.
                let still: Vec<&str> = scope
                    .add
                    .iter()
                    .map(String::as_str)
                    .filter(|path| std::fs::symlink_metadata(dir.join(path)).is_ok())
                    .collect();
                if !still.is_empty() {
                    run(&add, Some(still.join("\0").into_bytes()))?;
                }
            }
        }
        let tree = run(&["write-tree"], None)?;
        let mut args = vec!["commit-tree", tree.as_str(), "-m", "Nexus checkpoint"];
        let note = checkpoint_scope::note(&scope.skipped);
        if let Some(note) = &note {
            args.extend(["-m", note.as_str()]);
        }
        if let Some(head) = &head {
            args.extend(["-p", head.as_str()]);
        }
        run(&args, None)
    })();
    let _ = std::fs::remove_file(&index);
    // A git stopped at the deadline leaves its lock behind.
    let _ = std::fs::remove_file(format!("{index_path}.lock"));
    result
}

/// git in `dir` with `env`, killed with what it started at `deadline`: what [`run_until`] ran, as [`git_with`] gives it.
fn git_until(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    input: Option<Vec<u8>>,
    deadline: Instant,
) -> Result<String, String> {
    let out = run_until(dir, args, env, input, deadline)?;
    if !out.success {
        return Err(out.stderr.trim().to_string());
    }
    // A `-z` list is NUL-separated names, kept as they are: a name may start or end with a space.
    Ok(if args.contains(&"-z") {
        out.stdout
    } else {
        out.stdout.trim().to_string()
    })
}

/// git in `dir` with `env` and `input`, killed with what it started at `deadline`: a snapshot's step.
fn run_until(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    input: Option<Vec<u8>>,
    deadline: Instant,
) -> Result<Ran, String> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(checkpoint_scope::too_slow());
    }
    let mut command = process::command("git");
    command
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .envs(env.iter().copied());
    output_within_input(command, input, left)
        .map_err(|error| format!("Could not run git: {error}"))?
        .ok_or_else(checkpoint_scope::too_slow)
}

/// What putting `dir`'s files back as they are in the checkpoint `target` would change: each path (relative to `dir`)
/// and whether `target` has it (false: the file goes).
pub fn revert_plan(dir: &Path, target: &str) -> Result<Vec<(String, bool)>, String> {
    let now = snapshot(dir)?;
    let listed = git(
        dir,
        &[
            "diff",
            "--name-status",
            "--no-renames",
            "--relative",
            "-z",
            &now,
            target,
            "--",
        ],
    )?;
    // What the target left out was never looked at: its absence there does not mean the turn made it.
    let skipped = checkpoint_scope::skipped_in(&git(dir, &["log", "-1", "--format=%B", target])?);
    let mut fields = listed.split('\0').filter(|field| !field.is_empty());
    let mut plan = Vec::new();
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        let restored = status != "D";
        if restored || !checkpoint_scope::kept(path, &skipped) {
            plan.push((path.to_string(), restored));
        }
    }
    Ok(plan)
}

/// Puts `dir`'s files (tracked and untracked, not ignored) back as they are in the checkpoint `target`: files it has
/// are written from it, files it lacks are deleted. The index, HEAD and branches are left alone, so what the agent
/// committed since stays committed and shows as a change. Returns how many files changed.
pub fn revert_files(dir: &Path, target: &str) -> Result<usize, String> {
    let plan = revert_plan(dir, target)?;
    let (keep, gone): (Vec<_>, Vec<_>) = plan.iter().partition(|(_, kept)| *kept);
    // Deletes first, so a file the target has where a folder now is can be written. A failure leaves the folder part
    // way, but a retry plans again from the disk and finishes it.
    for (path, _) in &gone {
        let path_on_disk = dir.join(path);
        // A repository the agent cloned in is one entry (a gitlink), a folder on disk.
        let removed = match std::fs::symlink_metadata(&path_on_disk) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&path_on_disk),
            _ => std::fs::remove_file(&path_on_disk),
        };
        match removed {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{path}: {error}")),
        }
    }
    let source = format!("--source={target}");
    for chunk in keep.chunks(500) {
        let mut args = vec!["--literal-pathspecs", "restore", source.as_str(), "--worktree", "--"];
        args.extend(chunk.iter().map(|(path, _)| path.as_str()));
        git(dir, &args)?;
    }
    Ok(plan.len())
}

/// Copies the index at `from` to `to`, keeping its modified time: git trusts an entry's stat only when the file is
/// older than the index, so a fresh time would hide an edit made in the same tick as the last `add`.
fn seed_index(from: &Path, to: &Path) -> bool {
    let copied = (|| -> std::io::Result<()> {
        let modified = std::fs::metadata(from)?.modified()?;
        std::fs::copy(from, to)?;
        std::fs::File::options().write(true).open(to)?.set_modified(modified)
    })();
    copied.is_ok()
}

/// Clears the bits that make `add` skip an entry on the scratch index: assume-unchanged, and skip-worktree on files
/// that are on disk (a common way to hide local edits). Skip-worktree files absent from disk are sparse-checkout
/// entries and keep the bit, so they stay in the snapshot as they are in the index.
fn unhide_entries(dir: &Path, env: &[(&str, &str)]) -> Result<(), String> {
    let listed = git_with(dir, &["ls-files", "-v", "-z"], env)?;
    let mut assumed = Vec::new();
    let mut skipped = Vec::new();
    for entry in listed.split('\0') {
        let Some((tag, path)) = entry.split_once(' ') else {
            continue;
        };
        if tag.chars().any(|c| c.is_ascii_lowercase()) {
            assumed.push(path);
        }
        if tag.eq_ignore_ascii_case("s") && dir.join(path).exists() {
            skipped.push(path);
        }
    }
    for (flag, paths) in [("--no-assume-unchanged", assumed), ("--no-skip-worktree", skipped)] {
        for chunk in paths.chunks(500) {
            let mut args = vec!["update-index", flag, "--"];
            args.extend(chunk);
            git_with(dir, &args, env)?;
        }
    }
    Ok(())
}

/// The refs under `prefix`, sorted by name.
pub fn refs(dir: &Path, prefix: &str) -> Result<Vec<String>, String> {
    let out = git(dir, &["for-each-ref", "--sort=refname", "--format=%(refname)", prefix])?;
    Ok(out.lines().map(String::from).collect())
}

/// Deletes every ref under `prefix`.
pub fn drop_refs(dir: &Path, prefix: &str) -> Result<(), String> {
    for name in refs(dir, prefix)? {
        git(dir, &["update-ref", "-d", &name])?;
    }
    Ok(())
}

/// Makes git write a blank context line as a single space, whatever the person's `diff.suppressBlankEmpty` says: the
/// Changes parser skips an empty line, which would count every line after it one short.
const KEEP_BLANK_CONTEXT: &str = "diff.suppressBlankEmpty=false";

/// The unified diff from `from` to `to` (commits or refs), renames found, as git wrote it; cut by [`cut_patch`] at
/// `max` bytes (the bool says so).
/// Paths always carry `a/` and `b/`, whatever the person's `diff.noprefix` or `diff.mnemonicPrefix` say, so the
/// Changes parser can strip them blindly; a blank context line is always a single space ([`KEEP_BLANK_CONTEXT`]).
/// Paths are relative to `dir` (and files outside it are left out), so a session whose folder sits below the
/// repository's top still gets paths that resolve inside it.
pub fn diff(dir: &Path, from: &str, to: &str, max: usize) -> Result<(String, bool), String> {
    diff_with(dir, from, to, max, &[])
}

pub(crate) fn phone_diff(dir: &Path, from: &str, to: &str, max: usize) -> Result<(String, bool), String> {
    diff_with(dir, from, to, max, &["--no-textconv"])
}

fn diff_with(dir: &Path, from: &str, to: &str, max: usize, options: &[&str]) -> Result<(String, bool), String> {
    let mut args = vec!["-c", KEEP_BLANK_CONTEXT, "diff", "--no-color", "--no-ext-diff"];
    args.extend_from_slice(options);
    args.extend([
        "--find-renames",
        "--relative",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        from,
        to,
        "--",
    ]);
    let mut text = git_untrimmed(dir, &args, &[])?;
    let cut = cut_patch(&mut text, max);
    Ok((text, cut))
}

/// Cuts `text` to at most `max` bytes, on a character's boundary; true when it cut.
pub fn cut_at(text: &mut String, max: usize) -> bool {
    if text.len() <= max {
        return false;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    true
}

/// Cuts a patch to at most `max` bytes after its last whole line; true when it cut. Half a hunk header or half a file
/// header is not a line a parser can read, and a `--- ` header without its `+++ ` line is not either, so that goes
/// too (a removed line that reads `--- ` goes with it: one more line of a hunk already cut). A first line longer than
/// `max` is cut on a character instead.
fn cut_patch(text: &mut String, max: usize) -> bool {
    if !cut_at(text, max) {
        return false;
    }
    if let Some(end) = text.rfind('\n') {
        text.truncate(end + 1);
        let last = text[..end].rfind('\n').map_or(0, |at| at + 1);
        if text[last..].starts_with("--- ") {
            text.truncate(last);
        }
    }
    true
}

/// The tracked files of `dir` against the commit `rev`, staged or not, as a unified diff as git wrote it, cut by
/// [`cut_patch`] at `max` bytes (the bool says so): what a commit of everything would add since `rev`. `diff-index`,
/// not `diff`, for the reason [`uncommitted`] gives.
pub fn patch_since(dir: &Path, rev: &str, max: usize) -> Result<(String, bool), String> {
    let mut text = git_untrimmed(
        dir,
        &[
            "-c",
            KEEP_BLANK_CONTEXT,
            "diff-index",
            "--patch",
            "--no-color",
            "--no-ext-diff",
            "--find-renames",
            "--relative",
            rev,
            "--",
        ],
        &[],
    )?;
    let cut = cut_patch(&mut text, max);
    Ok((text, cut))
}

/// The files of `dir` git does not track yet and does not ignore, by their paths.
pub fn untracked_files(dir: &Path) -> Result<Vec<String>, String> {
    Ok(git(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(String::from)
        .collect())
}

/// The subjects of up to `limit` commits `range` names (`HEAD`, `origin/main..HEAD`), newest first. Empty when it
/// names none, as in a repository with no commits yet.
pub fn subjects(dir: &Path, range: &str, limit: usize) -> Vec<String> {
    git(
        dir,
        &[
            "log",
            "--no-color",
            "--format=%s",
            &format!("--max-count={limit}"),
            range,
            "--",
        ],
    )
    .map(|text| {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(String::from)
            .collect()
    })
    .unwrap_or_default()
}

/// Where GitHub looks for a repository's pull request template, in its order.
const PR_TEMPLATES: &[&str] = &[
    ".github/pull_request_template.md",
    ".github/PULL_REQUEST_TEMPLATE.md",
    "pull_request_template.md",
    "PULL_REQUEST_TEMPLATE.md",
    "docs/pull_request_template.md",
    "docs/PULL_REQUEST_TEMPLATE.md",
];

/// The repository's pull request template, cut at `max` bytes; None when it has none (or only a folder of them).
pub fn pull_request_template(dir: &Path, max: usize) -> Option<String> {
    let top = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"]).ok()?);
    PR_TEMPLATES.iter().find_map(|name| {
        let mut text = std::fs::read_to_string(top.join(name)).ok()?;
        cut_at(&mut text, max);
        (!text.trim().is_empty()).then_some(text)
    })
}

/// Commits everything in `dir` that changed, as the person's own git identity (their hooks run). False: nothing to commit.
/// `marked`: in a nexus-xyz repository the commit names Nexus as a co-author, as a session's own commits do
/// ([`crate::Host::marks_commits`]).
pub fn commit_all(dir: &Path, message: &str, marked: bool) -> Result<bool, String> {
    use nexus_client::commit_trailer::{Surface, VARIABLE, parameters};
    if git(dir, &["status", "--porcelain"])?.is_empty() {
        return Ok(false);
    }
    git(dir, &["add", "--all"])?;
    let commit = ["commit", "--quiet", "-m", message];
    if marked {
        let trailer = parameters(std::env::var(VARIABLE).ok().as_deref(), Surface::Desktop);
        git_with(dir, &commit, &[(VARIABLE, &trailer)])?;
    } else {
        git(dir, &commit)?;
    }
    Ok(true)
}

/// What a commit of everything in `dir` would take: files changed, lines added, lines removed. Tracked changes,
/// staged or not, against HEAD; an untracked file counts all its lines (none when binary).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stat {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
}

pub fn uncommitted(dir: &Path) -> Result<Stat, String> {
    let mut stat = Stat::default();
    // `diff-index`, not `diff`: `git diff` rewrites the index to refresh its stat cache, whatever GIT_OPTIONAL_LOCKS says.
    for line in git(dir, &["diff-index", "--numstat", "--no-renames", "HEAD", "--"])?.lines() {
        let mut columns = line.splitn(3, '\t');
        stat.files += 1;
        // A binary file's columns are `-`.
        stat.insertions += columns.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
        stat.deletions += columns.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
    }
    for name in untracked_files(dir)? {
        stat.files += 1;
        stat.insertions = stat.insertions.saturating_add(untracked_lines(&dir.join(name)));
    }
    Ok(stat)
}

/// Bytes of an untracked file read before the count stops: a multi-GB log must not be read end to end on every refresh.
const UNTRACKED_READ_CAP: u64 = 64 * 1024 * 1024;
/// Git calls a file binary when a NUL appears in its first 8000 bytes (xdiff `buffer_is_binary`).
const BINARY_PROBE: u64 = 8000;

/// The lines an untracked file adds, as git counts them: none for a binary file (a NUL byte in its first 8000
/// bytes, git's own rule; `.gitattributes` overrides are not applied), one for a symbolic link (its target, never
/// followed: a link to `/dev/zero` or a huge file must not be read), none for anything else that is not a plain
/// file. Read in pieces, never whole. Past 64 MiB the read stops and the lines seen so far are returned, so the
/// count of a huge file is an approximation.
fn untracked_lines(path: &Path) -> u32 {
    untracked_lines_capped(path, UNTRACKED_READ_CAP)
}

fn untracked_lines_capped(path: &Path, cap: u64) -> u32 {
    use std::io::Read;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.file_type().is_symlink() {
        return 1;
    }
    if !meta.is_file() {
        return 0;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return 0;
    };
    let mut buffer = vec![0u8; 64 * 1024];
    let (mut lines, mut last, mut seen, mut truncated) = (0u64, None, 0u64, false);
    loop {
        if seen >= cap {
            truncated = true;
            break;
        }
        let want = usize::try_from(cap - seen).map_or(buffer.len(), |left| left.min(buffer.len()));
        let read = match file.read(&mut buffer[..want]) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return 0,
        };
        let piece = &buffer[..read];
        if seen < BINARY_PROBE {
            let probe = usize::try_from(BINARY_PROBE - seen).map_or(read, |left| left.min(read));
            if piece[..probe].contains(&0) {
                return 0;
            }
        }
        seen += read as u64;
        lines += piece.iter().filter(|&&byte| byte == b'\n').count() as u64;
        last = piece.last().copied();
    }
    // A cut-off read ends mid-line as likely as not: that partial line is not counted.
    let unterminated = u64::from(!truncated && last.is_some_and(|byte| byte != b'\n'));
    u32::try_from(lines + unterminated).unwrap_or(u32::MAX)
}

/// The local branches, most recently committed to first.
pub fn local_branches(dir: &Path) -> Result<Vec<String>, String> {
    let out = git(
        dir,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:short)",
            "refs/heads/",
        ],
    )?;
    Ok(out.lines().map(String::from).collect())
}

/// The most branches [`worktree_bases`] lists.
const MAX_BASES: usize = 300;

/// What a new worktree can start from, for the New session form's picker (as T3 Code's base branch picker): the
/// default base first ([`default_base`], `origin/main`), then the checked-out branch, then the local branches and the
/// remote ones, each most recent first. No `<remote>/HEAD`, and no name twice.
pub fn worktree_bases(repo: &Path) -> Result<Vec<String>, String> {
    let refs = git(
        repo,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:short)%09%(symref)",
            "refs/heads/",
            "refs/remotes/",
        ],
    )?;
    let listed = refs
        .lines()
        .filter(|line| line.split_once('\t').is_none_or(|(_, symref)| symref.is_empty()))
        .map(|line| line.split('\t').next().unwrap_or(line).to_string());
    let mut bases: Vec<String> = Vec::new();
    for name in default_base(repo).into_iter().chain(checked_out(repo)).chain(listed) {
        if !name.is_empty() && !bases.contains(&name) {
            bases.push(name);
        }
        if bases.len() == MAX_BASES {
            break;
        }
    }
    Ok(bases)
}

/// Checks out the local branch `name`. git refuses, changing nothing, when that would overwrite uncommitted work.
pub fn switch(dir: &Path, name: &str) -> Result<(), String> {
    if !local_branches(dir)?.iter().any(|branch| branch == name) {
        return Err(format!("There is no local branch {name}."));
    }
    git(dir, &["switch", "--quiet", "--", name]).map(|_| ())
}

/// How long a push may take. A big first push over a slow link takes minutes; a remote that never answers is given
/// up on here, rather than holding the commit button for good. A `pre-push` hook runs inside the push, so its time
/// counts too.
pub const PUSH_LIMIT: Duration = Duration::from_secs(5 * 60);

/// Pushes `branch` to `origin` and tracks it there, within [`PUSH_LIMIT`].
pub fn push(dir: &Path, branch: &str) -> Result<(), String> {
    push_within(dir, branch, PUSH_LIMIT)
}

fn push_within(dir: &Path, branch: &str, limit: Duration) -> Result<(), String> {
    git_timed(dir, &["push", "--quiet", "--set-upstream", "origin", branch], limit).map(|_| ())
}

/// Where the checked-out branch stands against another branch: the commits each has that the other lacks.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tracking {
    /// The other branch, as git names it short (`origin/main`).
    pub name: String,
    /// Commits here that it lacks.
    pub ahead: u32,
    /// Commits it has that are not here yet.
    pub behind: u32,
}

/// The branch the checked-out one tracks (`origin/feature`), as git names it short. None: a detached HEAD, no upstream
/// set, or an upstream that is gone.
pub fn upstream(dir: &Path) -> Option<String> {
    git(
        dir,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    )
    .ok()
    .filter(|name| !name.is_empty())
}

/// HEAD against `other`. `rev-list` only reads, so this is safe while an agent runs git in the same folder.
fn compare(dir: &Path, other: &str) -> Option<Tracking> {
    let out = git(
        dir,
        &["rev-list", "--left-right", "--count", &format!("HEAD...{other}")],
    )
    .ok()?;
    let mut counts = out.split_whitespace().map(|n| n.parse::<u32>().ok());
    Some(Tracking {
        name: other.to_string(),
        ahead: counts.next()??,
        behind: counts.next()??,
    })
}

/// The checked-out branch against its upstream, as last fetched.
pub fn tracking(dir: &Path) -> Option<Tracking> {
    compare(dir, &upstream(dir)?)
}

/// The ref a worktree's branch was made from, by its recorded `gh-merge-base` (`main`): the remote's copy
/// (`origin/main`) when there is one, else the local branch.
pub fn base_ref(dir: &Path, branch: &str) -> Option<String> {
    let base = merge_base_branch(dir, branch)?;
    let exists = |name: &str| {
        git(
            dir,
            &["rev-parse", "--verify", "--quiet", &format!("{name}^{{commit}}")],
        )
        .is_ok()
    };
    [format!("origin/{base}"), base].into_iter().find(|name| exists(name))
}

/// The checked-out branch against the branch its worktree was made from.
pub fn base_tracking(dir: &Path) -> Option<Tracking> {
    compare(dir, &base_ref(dir, &checked_out(dir)?)?)
}

/// What a command run by [`output_within`] did.
pub(crate) struct Ran {
    pub(crate) success: bool,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// After a command exited, how long its output may still take to arrive: a process it left behind (an ssh
/// ControlMaster, say) can hold the pipes open for good.
/// The same wait the agent runs get ([`process::DRAIN`]). When it runs out, the two reader threads stay blocked in
/// reading until the leftover process closes its pipes: bounded, and harmless beyond a parked thread each.
const OUTPUT_DRAIN: Duration = process::DRAIN;

/// A limit as words for a message: whole seconds as they are, a fraction with one decimal (`0.3 seconds`).
pub(crate) fn seconds(limit: Duration) -> String {
    if limit.subsec_nanos() == 0 {
        format!("{} seconds", limit.as_secs())
    } else {
        format!("{:.1} seconds", limit.as_secs_f64())
    }
}

/// Runs `command` with nothing on its input, reading both outputs as it goes (so a long one cannot fill a pipe and
/// stall it). Past `limit` it is killed with everything it started (its own process group: git's ssh, gh's git), and
/// the answer is None. For the network, where a remote that hangs would hold the caller forever.
pub(crate) fn output_within(command: std::process::Command, limit: Duration) -> std::io::Result<Option<Ran>> {
    run_within(command, None, limit, false, OUTPUT_CAP)
}

/// [`output_within`] for repository code: when it ends, what it left running in its process group is killed too, so a
/// child it left behind (or one that waits for a secret to appear) does not outlive it.
pub(crate) fn output_ending_all(command: std::process::Command, limit: Duration) -> std::io::Result<Option<Ran>> {
    run_within(command, None, limit, true, OUTPUT_CAP)
}

/// [`output_within`], with `input` written to the command's input (on a thread of its own, so a command that writes
/// before it reads cannot stall on a full pipe). It runs the checkpoint's git listings, so each output is kept to
/// [`GIT_LISTING_CAP`].
fn output_within_input(
    command: std::process::Command,
    input: Option<Vec<u8>>,
    limit: Duration,
) -> std::io::Result<Option<Ran>> {
    run_within(command, input, limit, false, GIT_LISTING_CAP)
}

fn run_within(
    mut command: std::process::Command,
    input: Option<Vec<u8>>,
    limit: Duration,
    end_leftovers: bool,
    cap: usize,
) -> std::io::Result<Option<Ran>> {
    use std::io::{Read, Write};
    command
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn()?;
    if let (Some(input), Some(mut pipe)) = (input, child.stdin.take()) {
        // Dropping the pipe at the end closes it, so the command sees the end of its input.
        std::thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let bytes = pipe.map(|pipe| read_capped(pipe, cap)).unwrap_or_default();
            let _ = tx.send(bytes);
        });
        rx
    };
    let stdout = read(child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let stderr = read(child.stderr.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            if end_leftovers {
                process::end_leftovers(Some(child.id()));
            }
            break status;
        }
        if Instant::now() >= deadline {
            // SIGTERM first, so git removes the lock it holds; SIGKILL if it has not ended shortly.
            process::terminate(Some(child.id()));
            let grace = Instant::now() + Duration::from_secs(2);
            while Instant::now() < grace && child.try_wait()?.is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
            process::kill(Some(child.id()));
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let drained = Instant::now() + OUTPUT_DRAIN;
    let take = |rx: std::sync::mpsc::Receiver<Vec<u8>>| {
        let bytes = rx
            .recv_timeout(drained.saturating_duration_since(Instant::now()))
            .unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    Ok(Some(Ran {
        success: status.success(),
        stdout: take(stdout),
        stderr: take(stderr),
    }))
}

/// git, killed with what it started after `limit`: for the network, where a remote that hangs would hold the caller
/// forever.
fn git_timed(dir: &Path, args: &[&str], limit: Duration) -> Result<String, String> {
    let mut command = process::command("git");
    command
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    let out = output_within(command, limit)
        .map_err(|error| format!("Could not run git: {error}"))?
        .ok_or_else(|| {
            format!(
                "The remote didn't answer within {} (or a git hook, such as pre-push, ran too long).",
                seconds(limit)
            )
        })?;
    if out.success {
        Ok(out.stdout.trim().to_string())
    } else {
        Err(out.stderr.trim().to_string())
    }
}

/// How long a fetch may take.
const FETCH_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Fetches the branches HEAD is compared against: its upstream and, in a worktree, the branch it was made from. Only
/// those branches, not the whole remote, and no tags: it runs in the background. It writes no `FETCH_HEAD` and starts
/// no `gc`, so it changes nothing but the remote-tracking refs (checkpoints are refs, and stay). Nothing to fetch is no
/// error; a branch that fails does not stop the others, and the first error is returned after.
pub fn fetch(dir: &Path) -> Result<(), String> {
    let remotes = git(dir, &["remote"]).unwrap_or_default();
    let mut wanted: Vec<(String, String)> = Vec::new();
    if let Some(branch) = checked_out(dir) {
        let remote = git(dir, &["config", &format!("branch.{branch}.remote")]).unwrap_or_default();
        let merge = git(dir, &["config", &format!("branch.{branch}.merge")]).unwrap_or_default();
        if let Some(name) = merge.strip_prefix("refs/heads/")
            && remotes.lines().any(|line| line == remote)
        {
            wanted.push((remote.clone(), name.to_string()));
        }
        if let Some((remote, name)) = base_ref(dir, &branch).as_deref().and_then(|base| base.split_once('/'))
            && remotes.lines().any(|line| line == remote)
        {
            wanted.push((remote.to_string(), name.to_string()));
        }
    }
    wanted.dedup();
    // `name` comes from the repository's own config (`branch.<b>.merge`, `gh-merge-base`): one that git would read as
    // an option is dropped.
    wanted
        .retain(|(remote, name)| check_ref_arg("remote", remote).is_ok() && check_ref_arg("branch name", name).is_ok());
    let mut first_error = None;
    for (remote, name) in &wanted {
        let args = [
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--no-auto-gc",
            "--",
            remote.as_str(),
            name.as_str(),
        ];
        if let Err(error) = git_timed(dir, &args, FETCH_LIMIT) {
            first_error.get_or_insert(format!("Couldn't fetch {remote}/{name}: {error}"));
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// What a pull did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pulled {
    /// The branch is up to date with its upstream (it moved forward, or already was).
    Done,
    /// The rebase stopped on conflicts in these files, and was aborted: the branch and the files are as they were.
    Conflict(Vec<String>),
}

/// Whether git is part way through a rebase, merge or cherry-pick in `dir`.
fn operation_in_progress(dir: &Path) -> bool {
    [
        "rebase-merge",
        "rebase-apply",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ]
    .iter()
    .any(|name| {
        git(dir, &["rev-parse", "--git-path", name]).is_ok_and(|path| {
            let path = PathBuf::from(path);
            if path.is_absolute() { path } else { dir.join(path) }.exists()
        })
    })
}

/// Brings the checked-out branch up to date with its upstream, fetched first. Fast-forward only; when both sides have
/// new commits, only with `rebase`: the branch's own commits are replayed on top of the upstream's. A rebase that
/// stops on a conflict is aborted at once, so the folder is never left half-rebased, and the conflicted files are
/// returned. Refuses with words a person understands when there is nothing to pull from, or when uncommitted changes
/// are in the way. Only the branch moves: other refs (checkpoints among them) stay where they are.
pub fn pull(dir: &Path, rebase: bool) -> Result<Pulled, String> {
    let branch = checked_out(dir).ok_or_else(|| "Check out a branch to pull.".to_string())?;
    let upstream = upstream(dir).ok_or_else(|| {
        format!("{branch} doesn't track a remote branch yet, so there is nothing to pull. Push it first.")
    })?;
    if operation_in_progress(dir) {
        return Err("git is part way through a rebase or merge in this folder. Finish or abort it first.".into());
    }
    fetch(dir)?;
    let now = compare(dir, &upstream).ok_or_else(|| format!("Couldn't compare {branch} with {upstream}."))?;
    if now.behind == 0 {
        return Ok(Pulled::Done);
    }
    if now.ahead == 0 {
        return git(dir, &["merge", "--ff-only", "--quiet", &upstream])
            .map(|_| Pulled::Done)
            .map_err(|error| {
                if error.contains("would be overwritten") {
                    format!("Pulling would overwrite files you changed. Commit or discard them first.\n\n{error}")
                } else {
                    error
                }
            });
    }
    if !rebase {
        return Err(format!(
            "{branch} and {upstream} both have new commits, so it can't simply move forward. Rebase onto {upstream} instead."
        ));
    }
    if !git(dir, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err("Commit or discard your changes before rebasing.".into());
    }
    // No autostash (a stash that fails to come back is work hidden from the person), no other branches moved along,
    // no editor: the person's config could ask for any of these.
    let rebased = git_with(
        dir,
        &[
            "-c",
            "rebase.autoStash=false",
            "-c",
            "rebase.updateRefs=false",
            "rebase",
            "--quiet",
            &upstream,
        ],
        &[("GIT_EDITOR", "true"), ("GIT_SEQUENCE_EDITOR", "true")],
    );
    match rebased {
        Ok(_) => Ok(Pulled::Done),
        Err(error) if operation_in_progress(dir) => {
            let files = git(dir, &["diff", "--name-only", "--diff-filter=U", "-z"])
                .unwrap_or_default()
                .split('\0')
                .filter(|name| !name.is_empty())
                .map(String::from)
                .collect::<Vec<_>>();
            git(dir, &["rebase", "--abort"]).map_err(|abort| {
                format!(
                    "The rebase stopped ({error}) and could not be undone ({abort}). Run `git rebase --abort` in {}.",
                    dir.display()
                )
            })?;
            if files.is_empty() {
                // Stopped for something other than a conflict (an untracked file in the way, say): undone all the same.
                return Err(format!(
                    "The rebase stopped and was undone, nothing changed.\n\n{error}"
                ));
            }
            Ok(Pulled::Conflict(files))
        }
        Err(error) => Err(format!("The rebase didn't start, nothing changed.\n\n{error}")),
    }
}

/// Makes a new local branch `name` at HEAD and checks it out; uncommitted changes come along.
pub fn create_branch(dir: &Path, name: &str) -> Result<(), String> {
    let name = name.trim();
    git(dir, &["check-ref-format", "--branch", name]).map_err(|_| format!("{name} is not a valid branch name."))?;
    if has_branch(dir, name) {
        return Err(format!("There is already a branch {name}."));
    }
    git(dir, &["switch", "--quiet", "-c", name]).map(|_| ())
}

/// The branch a pull request from `branch` targets: the one recorded when its worktree was made.
pub fn merge_base_branch(dir: &Path, branch: &str) -> Option<String> {
    git(dir, &["config", &format!("branch.{branch}.gh-merge-base")])
        .ok()
        .filter(|base| !base.is_empty())
}

/// The checked-out branch's pull request, opened with the host's CLI when there is none yet: its URL. `path` finds
/// `gh` or `glab` (an app started from the dock lacks the login shell's `PATH`); its own sign-in is used, never read
/// here.
pub fn pull_request(dir: &Path, base: &str, title: &str, body: &str, path: &str) -> Result<String, String> {
    let host = Host::of_repo(dir);
    pull_request_with(host, Path::new(host.program()), dir, base, title, body, path)
}

fn pull_request_with(
    host: Host,
    program: &Path,
    dir: &Path,
    base: &str,
    title: &str,
    body: &str,
    path: &str,
) -> Result<String, String> {
    let missing = host.needs(&format!("Opening a {}", host.request()));
    let run = |args: &[&str]| cli_within(host, program, dir, path, args, GH_LIMIT, &missing);
    let (open, create) = match host {
        Host::GitHub => (
            run(&["pr", "view", "--json", "url", "--jq", ".url"]).ok(),
            vec!["pr", "create", "--base", base, "--title", title, "--body", body],
        ),
        Host::GitLab => (
            run(&gitlab::view(None))
                .ok()
                .and_then(|out| gitlab::open_url(&serde_json::from_str(&out).ok()?)),
            gitlab::create(base, title, body),
        ),
    };
    if let Some(url) = open
        && !url.is_empty()
    {
        return Ok(url);
    }
    let out = run(&create)?;
    // `glab` indents the address under the merge request's title; `gh` prints it alone on the last line.
    out.lines()
        .rev()
        .map(|line| if host == Host::GitLab { line.trim() } else { line })
        .find(|line| line.starts_with("https://"))
        .map(String::from)
        .ok_or_else(|| format!("{} did not say where the {} is: {out}", host.program(), host.request()))
}

/// What the GitHub CLI said, with the one fix people need most often named.
pub(crate) fn gh_error(stderr: &str) -> String {
    Host::GitHub.error(stderr)
}

/// Where `branch`'s work began on `base`: what "all changes" compares against.
pub fn merge_base(dir: &Path, base: &str, commit: &str) -> Result<String, String> {
    git(dir, &["merge-base", base, commit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_branch_of_a_repository_and_a_worktree() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-git-{}", std::process::id()));
        let repo = root.join("repo");
        let worktree = root.join("wt");
        let _ = std::fs::create_dir_all(repo.join(".git/worktrees/wt"));
        let _ = std::fs::create_dir_all(&worktree);
        let _ = std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n");
        let _ = std::fs::write(repo.join(".git/worktrees/wt/HEAD"), "ref: refs/heads/feature/x\n");
        let _ = std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/wt").display()),
        );
        assert_eq!(branch(&repo).as_deref(), Some("main"));
        assert_eq!(branch(&worktree).as_deref(), Some("feature/x"));
        assert_eq!(branch(&root), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lists_the_default_base_then_the_checked_out_branch_then_the_rest() {
        let root = scratch_repo("bases");
        for name in ["older", "feature"] {
            git(&root, &["branch", name]).unwrap_or_else(|e| panic!("{e}"));
        }
        git(&root, &["switch", "--quiet", "feature"]).unwrap_or_else(|e| panic!("{e}"));
        // No `origin`: the default base is the checked-out branch, listed once.
        let bases = worktree_bases(&root).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(bases.first().map(String::as_str), Some("feature"));
        let mut rest = bases[1..].to_vec();
        rest.sort();
        assert_eq!(rest, ["main", "older"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_project_deeper_in_a_repository_or_on_a_detached_head_is_still_a_git_project() {
        let root = scratch_repo("project-branch");
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap_or_else(|e| panic!("{e}"));
        for dir in ["apps/web", "build/out"] {
            std::fs::create_dir_all(root.join(dir)).unwrap_or_else(|e| panic!("{e}"));
        }
        assert_eq!(project_branch(&root).as_deref(), Some("main"));
        assert_eq!(project_top(&root.join("apps/web")).as_deref(), Some(root.as_path()));
        assert_eq!(project_branch(&root.join("apps/web")).as_deref(), Some("main"));
        // A folder the repository ignores is not a part of it.
        assert_eq!(project_branch(&root.join("build/out")), None);
        git(&root, &["checkout", "--quiet", "--detach"]).unwrap_or_else(|e| panic!("{e}"));
        let short = git(&root, &["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(project_branch(&root), Some(short.trim().to_string()));
        assert_eq!(project_branch(&std::env::temp_dir()), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_the_remotes_origin_first_without_credentials() {
        let config = "[core]\n\turl = not-a-remote\n[remote \"upstream\"]\n\turl = git@github.com:acme/app.git\n\
            \tfetch = +refs/heads/*:refs/remotes/upstream/*\n[Remote \"origin\"]\n\tURL = \"https://me:tok_secret@github.com/me/app.git\"\n\
            [remote \"empty\"]\n\turl =\n[branch \"main\"]\n\tremote = origin\n";
        assert_eq!(
            remotes_in(config),
            ["https://github.com/me/app.git", "git@github.com:acme/app.git"]
        );
        assert_eq!(
            without_credentials("ssh://git@host.example:22/a/b"),
            "ssh://host.example:22/a/b"
        );
        assert_eq!(
            without_credentials("https://github.com/a/b@c"),
            "https://github.com/a/b@c"
        );
        assert_eq!(without_credentials("/srv/git/app"), "/srv/git/app");
    }

    #[test]
    fn reads_the_remotes_of_a_repository_and_its_worktree() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-remotes-{}", std::process::id()));
        let repo = root.join("repo");
        let worktree = root.join("wt");
        let _ = std::fs::create_dir_all(repo.join(".git/worktrees/wt"));
        let _ = std::fs::create_dir_all(&worktree);
        let _ = std::fs::write(
            repo.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://github.com/acme/app\n",
        );
        let _ = std::fs::write(repo.join(".git/worktrees/wt/commondir"), "../..\n");
        let _ = std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/wt").display()),
        );
        assert_eq!(remotes(&repo), ["https://github.com/acme/app"]);
        assert_eq!(remotes(&worktree), ["https://github.com/acme/app"]);
        assert!(remotes(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Worktrees made at once in one repository all succeed: a crew's lead starts its children together, and two
    /// `git worktree add`s at once can fail on each other's half-made worktree. Several rounds, as the race is rare.
    #[test]
    fn worktrees_made_at_once_in_one_repository_all_succeed() {
        let root = scratch_repo("at-once");
        for round in 0..10 {
            let made: Vec<Result<(), String>> = std::thread::scope(|scope| {
                let adds: Vec<_> = (0..4)
                    .map(|i| {
                        let root = &root;
                        scope.spawn(move || {
                            let dir = root.join(".worktrees").join(format!("w{round}-{i}"));
                            add_worktree(root, &dir, &format!("w{round}-{i}"), "main")
                        })
                    })
                    .collect();
                adds.into_iter()
                    .map(|add| add.join().unwrap_or_else(|_| Err("panicked".into())))
                    .collect()
            });
            assert!(made.iter().all(Result::is_ok), "round {round}: {made:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A remote that never answers fails the new worktree at the deadline, with the fetch and its transport killed,
    /// instead of holding the session for good. A remote that is only gone (offline) still uses the copy here.
    #[cfg(unix)]
    #[test]
    fn a_fetch_that_hangs_fails_the_worktree_at_its_deadline() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (remote, repo) = (root.join("remote"), root.join("repo"));
        let run = |dir: &Path, args: &[&str]| git(dir, args).unwrap_or_else(|e| panic!("{e}"));
        for dir in [&remote, &repo] {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("{e}"));
            run(dir, &["init", "--quiet", "--initial-branch", "main"]);
            run(
                dir,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "--quiet",
                    "--allow-empty",
                    "-m",
                    "one",
                ],
            );
        }
        run(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
        run(&repo, &["fetch", "--quiet", "origin"]);
        // The transport git starts for this remote sleeps rather than answer.
        run(&repo, &["config", "remote.origin.uploadpack", "sleep 30;:"]);
        let started = Instant::now();
        let error = add_worktree_within(
            &repo,
            &root.join("wt/hung"),
            "nexus/hung",
            "origin/main",
            Duration::from_secs(1),
        )
        .err()
        .unwrap_or_default();
        assert!(error.contains("took longer than 1 seconds"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        assert!(!root.join("wt/hung").exists());
        // Offline: the fetch fails at once, and the copy of origin/main already here is used.
        run(&repo, &["config", "remote.origin.uploadpack", "false;:"]);
        add_worktree_within(
            &repo,
            &root.join("wt/offline"),
            "nexus/offline",
            "origin/main",
            Duration::from_secs(5),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A push to a remote that never answers fails at its deadline, with git and the transport it started killed,
    /// instead of holding the commit button for good.
    #[cfg(unix)]
    #[test]
    fn a_push_that_hangs_fails_at_its_deadline_and_ends_its_transport() {
        let root = scratch_repo("push-hang");
        let remote = root.join(".remote.git");
        let marker = root.join(".transport-outlived-the-push");
        git(&root, &["init", "--quiet", "--bare", &remote.to_string_lossy()]).unwrap_or_else(|e| panic!("{e}"));
        git(&root, &["remote", "add", "origin", &remote.to_string_lossy()]).unwrap_or_else(|e| panic!("{e}"));
        // The transport sleeps rather than answer, then leaves a mark if it was not killed.
        let pack = format!("sleep 2; touch '{}';:", marker.display());
        git(&root, &["config", "remote.origin.receivepack", &pack]).unwrap_or_else(|e| panic!("{e}"));
        let started = Instant::now();
        let error = push_within(&root, "main", Duration::from_millis(500))
            .err()
            .unwrap_or_default();
        assert!(error.contains("didn't answer"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        std::thread::sleep(Duration::from_secs(3));
        assert!(!marker.exists(), "the push's transport outlived the deadline");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A GitHub CLI call that hangs is given up on; one that prints more than a pipe holds still finishes.
    #[cfg(unix)]
    #[test]
    fn a_github_cli_call_is_bounded_and_reads_long_output() {
        let dir = std::env::temp_dir().join(format!("nexus-workbench-gh-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        // Written from a child shell: a script written here can be busy (ETXTBSY) when it runs, while other tests spawn.
        let fake = |name: &str, script: &str| {
            let program = dir.join(name);
            process::write_fake_script(&program, &format!("#!/bin/sh\n{script}\n"));
            program
        };
        let path = std::env::var("PATH").unwrap_or_default();
        let hung = fake("gh-hung", "exec sleep 30");
        let started = Instant::now();
        let error = cli_within(
            Host::GitHub,
            &hung,
            &dir,
            &path,
            &[],
            Duration::from_millis(300),
            "missing",
        )
        .err();
        assert_eq!(
            error.as_deref(),
            Some("GitHub didn't answer within 0.3 seconds. Try again.")
        );
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        // 200 KiB on stdout and stderr each: past a pipe's buffer, so it only finishes if both are read as it runs.
        let long = fake(
            "gh-long",
            "head -c 204800 /dev/zero | tr '\\0' x; head -c 204800 /dev/zero >&2",
        );
        let out = cli_within(
            Host::GitHub,
            &long,
            &dir,
            &path,
            &[],
            Duration::from_secs(10),
            "missing",
        )
        .unwrap_or_else(|e| panic!("the long-output gh failed: {e}"));
        assert_eq!(out.len(), 204800);
        let missing = cli_within(
            Host::GitHub,
            &dir.join("absent"),
            &dir,
            &path,
            &[],
            Duration::from_secs(1),
            "missing",
        )
        .err();
        assert_eq!(missing.as_deref(), Some("missing"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn makes_a_worktree_and_removes_it_only_when_nothing_would_be_lost() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
        let run = |dir: &Path, args: &[&str]| git(dir, args).unwrap_or_else(|e| panic!("{e}"));
        run(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        run(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "one",
            ],
        );
        assert_eq!(default_base(&repo).as_deref(), Some("main"));

        let dir = root.join("worktrees/repo/nexus-fix");
        assert!(add_worktree(&repo, &dir, "bad..name", "main").is_err());
        add_worktree(&repo, &dir, "nexus/fix", "main").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(branch(&dir).as_deref(), Some("nexus/fix"));
        assert_eq!(run(&repo, &["config", "branch.nexus/fix.gh-merge-base"]), "main");

        std::fs::write(dir.join("new.txt"), "x").unwrap_or_else(|e| panic!("{e}"));
        assert!(
            remove_worktree(&repo, &dir)
                .err()
                .unwrap_or_default()
                .contains("uncommitted")
        );
        run(&dir, &["add", "."]);
        run(
            &dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                "two",
            ],
        );
        assert!(
            remove_worktree(&repo, &dir)
                .err()
                .unwrap_or_default()
                .contains("aren't pushed")
        );

        // Pushed work is safe to drop: a worktree off the remote's main, with nothing new, goes.
        let remote = root.join("remote.git");
        run(&root, &["init", "--quiet", "--bare", &remote.to_string_lossy()]);
        run(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
        run(&repo, &["push", "--quiet", "origin", "main"]);
        let clean = root.join("worktrees/repo/nexus-clean");
        add_worktree(&repo, &clean, "nexus/clean", "origin/main").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(run(&repo, &["config", "branch.nexus/clean.gh-merge-base"]), "main");
        remove_worktree(&repo, &clean).unwrap_or_else(|e| panic!("{e}"));
        assert!(!clean.exists());
        // Already gone from disk: git forgets it.
        std::fs::remove_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        remove_worktree(&repo, &dir).unwrap_or_else(|e| panic!("{e}"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A new repository at `root/repo` (empty: no commits), and `root`.
    fn empty_repo(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("nexus-workbench-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
        git(&repo, &["init", "--quiet", "--initial-branch", "main"]).unwrap_or_else(|e| panic!("{e}"));
        (root, repo)
    }

    fn commit(repo: &Path, message: &str) {
        let identity = ["-c", "user.name=t", "-c", "user.email=t@t"];
        let args = [&identity[..], &["commit", "--quiet", "--allow-empty", "-m", message]].concat();
        git(repo, &args).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn a_base_or_branch_that_git_would_read_as_an_option_is_refused() {
        let (root, repo) = empty_repo("wt-dash");
        commit(&repo, "one");
        let ran = root.join("ran");
        let evil = format!("--upload-pack=touch {}", ran.display());
        for (branch, base) in [
            ("nexus/a", evil.as_str()),
            ("nexus/a", "-b"),
            ("nexus/a", "origin/--upload-pack=x"),
            ("-x", "main"),
        ] {
            let error = add_worktree(&repo, &root.join("wt"), branch, base)
                .err()
                .unwrap_or_default();
            assert!(error.contains("starts with -"), "{error}");
            assert!(!root.join("wt").exists());
        }
        let error = fork_worktree(&repo, &root.join("wt"), "nexus/b", "HEAD", "-c")
            .err()
            .unwrap_or_default();
        assert!(error.contains("starts with -"), "{error}");
        let error = add_review_worktree(&repo, &root.join("wt"), "review/pr-1", 1, "--upload-pack=x", "")
            .err()
            .unwrap_or_default();
        assert!(error.contains("starts with -"), "{error}");
        assert!(!ran.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ref_arguments_follow_gits_ref_format() {
        for good in [
            "main",
            "origin/main",
            "release/1.2",
            "HEAD",
            "0123456789abcdef0123456789abcdef01234567",
        ] {
            assert_eq!(check_ref_arg("base", good), Ok(()), "{good}");
        }
        for bad in [
            "",
            "-x",
            "--upload-pack=x",
            "origin/-x",
            "a..b",
            "a b",
            "a~1",
            "a^",
            "a:b",
            "@",
            "a@{1}",
            "/a",
            "a/",
            "a//b",
            ".a",
            "a/.b",
            "a.lock",
            "a.",
            "a\nb",
        ] {
            assert!(check_ref_arg("base", bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_capped_read_keeps_the_start_and_drains_the_rest() {
        let input = vec![b'x'; 300 * 1024];
        let kept = read_capped(&input[..], 100 * 1024);
        assert_eq!(kept.len(), 100 * 1024);
        assert_eq!(read_capped(&b"short"[..], 100), b"short");
    }

    #[test]
    fn a_repository_with_no_commits_says_so_instead_of_gits_invalid_reference() {
        let (root, repo) = empty_repo("wt-empty");
        let base = default_base(&repo).unwrap_or_else(|| "main".into());
        let error = add_worktree(&repo, &root.join("wt"), "nexus/a", &base)
            .err()
            .unwrap_or_default();
        assert!(error.contains("no commits yet"), "{error}");
        assert!(!root.join("wt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unborn_branch_in_a_repository_with_commits_does_not_claim_the_repository_is_empty() {
        let (root, repo) = empty_repo("wt-orphan");
        commit(&repo, "one");
        git(&repo, &["checkout", "--quiet", "--orphan", "wip"]).unwrap_or_else(|e| panic!("{e}"));
        let base = default_base(&repo).unwrap_or_default();
        assert_eq!(base, "wip");
        let error = add_worktree(&repo, &root.join("wt"), "nexus/a", &base)
            .err()
            .unwrap_or_default();
        assert!(error.starts_with("wip has no commits yet"), "{error}");
        assert!(!error.contains("Make a first commit, then try again"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_detached_head_with_no_default_branch_starts_a_worktree_from_its_commit() {
        let (root, repo) = empty_repo("wt-detached");
        commit(&repo, "one");
        git(&repo, &["checkout", "--quiet", "--detach"]).unwrap_or_else(|e| panic!("{e}"));
        let head = git(&repo, &["rev-parse", "HEAD"]).unwrap_or_default();
        assert_eq!(default_base(&repo), Some(head.clone()));
        let dir = root.join("wt");
        add_worktree(&repo, &dir, "nexus/a", &head).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(git(&dir, &["rev-parse", "HEAD"]), Ok(head));
        // A commit is no branch for a pull request to target: none is recorded.
        assert!(merge_base_branch(&repo, "nexus/a").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An untracked link is one line, as git counts it, and is never followed: one to `/dev/zero` would be read
    /// forever. A file larger than one read is counted in pieces, the same as whole.
    #[cfg(unix)]
    #[test]
    fn counts_untracked_links_without_following_them_and_big_files_in_pieces() {
        let (root, repo) = empty_repo("untracked-link");
        commit(&repo, "one");
        std::os::unix::fs::symlink("/dev/zero", repo.join("zero")).unwrap_or_else(|e| panic!("{e}"));
        let big = "line\n".repeat(50_000) + "no newline at the end";
        std::fs::write(repo.join("big.txt"), &big).unwrap_or_else(|e| panic!("{e}"));
        // A NUL past the first 8000 bytes does not make a file binary to git: one unterminated line.
        let mut late_nul = vec![b'a'; 100_000];
        late_nul[90_000] = 0;
        std::fs::write(repo.join("late.bin"), late_nul).unwrap_or_else(|e| panic!("{e}"));
        // A NUL within the first 8000 bytes does: no lines.
        let mut early_nul = vec![b'a'; 100_000];
        early_nul[7_999] = 0;
        std::fs::write(repo.join("early.bin"), early_nul).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            uncommitted(&repo),
            Ok(Stat {
                files: 4,
                insertions: 50_001 + 1 + 1,
                deletions: 0
            })
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Past the read cap the count stops and returns the lines seen so far.
    #[test]
    fn stops_counting_an_untracked_file_past_the_read_cap() {
        let (root, repo) = empty_repo("untracked-cap");
        let path = repo.join("log.txt");
        std::fs::write(&path, "line\n".repeat(1_000)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(untracked_lines_capped(&path, u64::MAX), 1_000);
        // 5 bytes a line: a 500 byte cap sees 100 lines.
        assert_eq!(untracked_lines_capped(&path, 500), 100);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn counts_an_untracked_file_whose_name_starts_with_a_space() {
        let (root, repo) = empty_repo("untracked-space");
        commit(&repo, "one");
        std::fs::write(repo.join(" lead.txt"), "a\nb\n").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            uncommitted(&repo),
            Ok(Stat {
                files: 1,
                insertions: 2,
                deletions: 0
            })
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn checks_out_a_pull_request_for_review_against_its_base() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-review-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let remote = root.join("remote.git");
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
        let run = |dir: &Path, args: &[&str]| git(dir, args).unwrap_or_else(|e| panic!("{e}"));
        let commit = |dir: &Path, message: &str| {
            run(
                dir,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "--quiet",
                    "--allow-empty",
                    "-m",
                    message,
                ],
            )
        };
        run(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        commit(&repo, "one");
        run(&root, &["init", "--quiet", "--bare", &remote.to_string_lossy()]);
        run(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
        run(&repo, &["push", "--quiet", "origin", "main"]);
        // Someone's pull request #7: a commit GitHub keeps at refs/pull/7/head, on no branch here.
        run(&repo, &["switch", "--quiet", "-c", "theirs"]);
        commit(&repo, "their change");
        let head = run(&repo, &["rev-parse", "HEAD"]);
        run(&repo, &["push", "--quiet", "origin", "HEAD:refs/pull/7/head"]);
        run(&repo, &["switch", "--quiet", "main"]);
        run(&repo, &["branch", "--quiet", "-D", "theirs"]);

        let dir = root.join("reviews/repo/pr-7");
        add_review_worktree(&repo, &dir, "review/pr-7", 7, "main", "").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(branch(&dir).as_deref(), Some("review/pr-7"));
        assert_eq!(run(&dir, &["rev-parse", "HEAD"]), head);
        assert_eq!(run(&repo, &["config", "branch.review/pr-7.gh-merge-base"]), "main");
        assert!(has_branch(&repo, "review/pr-7"));
        assert!(!has_branch(&repo, "review/pr-8"));
        // The pull request's own commit is not "unpushed": the review worktree can be removed again.
        remove_worktree(&repo, &dir).unwrap_or_else(|e| panic!("{e}"));
        assert!(!dir.exists());
        add_review_worktree(&repo, &dir, "review/pr-7-again", 7, "main", "").unwrap_or_else(|e| panic!("{e}"));
        assert!(
            add_review_worktree(&repo, &root.join("reviews/repo/pr-8"), "review/pr-8", 8, "main", "")
                .err()
                .unwrap_or_default()
                .contains("#8")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn checks_out_a_gitlab_merge_request_for_review_from_its_own_ref() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-mr-{}", uuid::Uuid::new_v4().simple()));
        let repo = root.join("repo");
        let remote = root.join("remote.git");
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
        let run = |dir: &Path, args: &[&str]| git(dir, args).unwrap_or_else(|e| panic!("{e}"));
        let commit = |dir: &Path, message: &str| {
            run(
                dir,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "--quiet",
                    "--allow-empty",
                    "-m",
                    message,
                ],
            )
        };
        run(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        commit(&repo, "one");
        run(&root, &["init", "--quiet", "--bare", &remote.to_string_lossy()]);
        run(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
        // A local remote names no host: the repository's setting says it is GitLab.
        run(&repo, &["config", "nexus.host", "gitlab"]);
        run(&repo, &["push", "--quiet", "origin", "main"]);
        // Someone's merge request !7: a commit GitLab keeps at refs/merge-requests/7/head.
        run(&repo, &["switch", "--quiet", "-c", "theirs"]);
        commit(&repo, "their change");
        let head = run(&repo, &["rev-parse", "HEAD"]);
        run(&repo, &["push", "--quiet", "origin", "HEAD:refs/merge-requests/7/head"]);
        run(&repo, &["switch", "--quiet", "main"]);
        run(&repo, &["branch", "--quiet", "-D", "theirs"]);

        let dir = root.join("reviews/repo/pr-7");
        add_review_worktree(&repo, &dir, "review/pr-7", 7, "main", "").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(run(&dir, &["rev-parse", "HEAD"]), head);
        assert_eq!(run(&repo, &["rev-parse", "refs/remotes/origin/mr/7"]), head);
        assert_eq!(run(&repo, &["config", "branch.review/pr-7.gh-merge-base"]), "main");
        remove_worktree(&repo, &dir).unwrap_or_else(|e| panic!("{e}"));
        let error = add_review_worktree(&repo, &root.join("reviews/repo/pr-8"), "review/pr-8", 8, "main", "")
            .err()
            .unwrap_or_default();
        assert!(error.starts_with("Could not fetch merge request !8"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A folder holding a fake host CLI `name` that runs `script`, for `PATH`.
    #[cfg(unix)]
    fn fake_cli(name: &str, script: &str) -> PathBuf {
        let bin = std::env::temp_dir().join(format!("nexus-workbench-cli-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("{e}"));
        process::write_fake_script(&bin.join(name), &format!("#!/bin/sh\n{script}\n"));
        bin
    }

    #[cfg(unix)]
    #[test]
    fn opens_a_gitlab_merge_request_with_glab_and_finds_one_already_open() {
        let repo = scratch_repo("glab-open");
        git(
            &repo,
            &["remote", "add", "origin", "git@gitlab.example.com:team/sub/app.git"],
        )
        .unwrap_or_default();
        // No open merge request yet: `glab mr view` fails, `glab mr create` opens one and prints its address indented.
        let bin = fake_cli(
            "glab",
            r#"test "$NO_PROMPT" = true || exit 9
case "$1 $2" in
  "mr view") echo 'no open merge request available for "ada/fix"' >&2; exit 1 ;;
  "mr create") test "$3 $4 $5 $6 $7 $8" = "--target-branch main --title Fix the login --description Body" || exit 3
    test "$9" = --yes || exit 4
    printf 'Creating merge request for ada/fix into main in team/sub/app

!12 Fix the login (ada/fix)
 https://gitlab.example.com/team/sub/app/-/merge_requests/12
' ;;
  *) exit 5 ;;
esac"#,
        );
        let path = bin.to_string_lossy().into_owned();
        assert_eq!(
            pull_request(&repo, "main", "Fix the login", "Body", &path),
            Ok("https://gitlab.example.com/team/sub/app/-/merge_requests/12".to_string())
        );
        // One already open: its address, and nothing created.
        let open = fake_cli(
            "glab",
            r#"test "$1 $2 $3 $4" = "mr view --output json" || exit 3
printf '%s' '{"iid":12,"state":"opened","web_url":"https://gitlab.example.com/team/sub/app/-/merge_requests/12"}'"#,
        );
        assert_eq!(
            pull_request(&repo, "main", "Fix the login", "", &open.to_string_lossy()),
            Ok("https://gitlab.example.com/team/sub/app/-/merge_requests/12".to_string())
        );
        // No glab: the same kind of hint as a missing gh.
        let error = pull_request(&repo, "main", "Fix the login", "", "/nonexistent")
            .err()
            .unwrap_or_default();
        assert_eq!(
            error,
            "Opening a merge request needs the GitLab CLI: install glab, run `glab auth login`, then try again."
        );
        for dir in [repo, bin, open] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_github_repository_still_opens_its_pull_request_with_gh() {
        let repo = scratch_repo("gh-open");
        git(&repo, &["remote", "add", "origin", "https://github.com/o/r.git"]).unwrap_or_default();
        let bin = fake_cli(
            "gh",
            r#"test "$GH_PROMPT_DISABLED" = 1 || exit 9
case "$1 $2" in
  "pr view") echo 'no pull requests found for branch "ada/fix"' >&2; exit 1 ;;
  "pr create") test "$3 $4 $5 $6 $7 $8" = "--base main --title Fix --body Body" || exit 3
    printf 'Warning: 1 uncommitted change
https://github.com/o/r/pull/9
' ;;
  *) exit 5 ;;
esac"#,
        );
        let path = bin.to_string_lossy().into_owned();
        assert_eq!(
            pull_request(&repo, "main", "Fix", "Body", &path),
            Ok("https://github.com/o/r/pull/9".to_string())
        );
        let error = pull_request(&repo, "main", "Fix", "", "/nonexistent")
            .err()
            .unwrap_or_default();
        assert_eq!(
            error,
            "Opening a pull request needs the GitHub CLI: install gh, run `gh auth login`, then try again."
        );
        let _ = std::fs::remove_dir_all(repo);
        let _ = std::fs::remove_dir_all(bin);
    }

    #[cfg(unix)]
    #[test]
    fn lists_and_describes_gitlab_merge_requests_and_reads_a_merged_head() {
        let repo = scratch_repo("glab-list");
        git(&repo, &["remote", "add", "origin", "https://gitlab.com/team/app.git"]).unwrap_or_default();
        let request = r#"{"iid":7,"title":"Fix","web_url":"https://gitlab.com/team/app/-/merge_requests/7","state":"merged","sha":"abc","author":{"username":"ada"},"source_branch":"ada/fix","target_branch":"main"}"#;
        let bin = fake_cli(
            "glab",
            &format!(
                r#"case "$*" in
  "mr list --per-page 30 --output json") printf '%s' '[{request}]' ;;
  "mr view 7 --output json") printf '%s' '{request}' ;;
  "mr list --merged --source-branch "*) printf '%s' '[{request}]' ;;
  *) exit 5 ;;
esac"#
            ),
        );
        let path = bin.to_string_lossy().into_owned();
        let list = pull_requests(&repo, &path).unwrap_or_default();
        assert_eq!(list.len(), 1);
        assert_eq!(
            (list[0].number, list[0].host, list[0].base.as_str()),
            (7, Host::GitLab, "main")
        );
        assert_eq!(
            pull_request_info(&repo, 7, &path).map(|info| info.head),
            Ok("ada/fix".to_string())
        );
        assert_eq!(merged_head(&repo, &path, Some(7)).as_deref(), Some("abc"));
        assert_eq!(merged_head(&repo, &path, None).as_deref(), Some("abc"));
        let _ = std::fs::remove_dir_all(repo);
        let _ = std::fs::remove_dir_all(bin);
    }

    #[test]
    fn checkpoints_without_touching_the_index_and_diffs_between_them() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-cp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap_or_else(|e| panic!("{e}"));
        let run = |args: &[&str]| git(&root, args).unwrap_or_else(|e| panic!("{e}"));
        run(&["init", "--quiet", "--initial-branch", "main"]);
        std::fs::write(root.join(".gitignore"), "secret\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-m",
            "one",
        ]);

        let prefix = "refs/nexus/checkpoints/t_1/";
        checkpoint(&root, &format!("{prefix}00000000")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("a.txt"), "two\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("new.txt"), "fresh\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("secret"), "token\n").unwrap_or_else(|e| panic!("{e}"));
        run(&["add", "a.txt"]);
        checkpoint(&root, &format!("{prefix}00000007")).unwrap_or_else(|e| panic!("{e}"));

        // The person's index still holds exactly what they staged; nothing new is staged, nothing committed.
        assert_eq!(run(&["diff", "--cached", "--name-only"]), "a.txt");
        assert_eq!(run(&["rev-list", "--count", "HEAD"]), "1");
        let names = refs(&root, prefix).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(names, [format!("{prefix}00000000"), format!("{prefix}00000007")]);
        let (text, cut) = diff(&root, &names[0], &names[1], 1 << 20).unwrap_or_else(|e| panic!("{e}"));
        assert!(!cut);
        assert!(text.contains("-one\n+two"), "{text}");
        assert!(text.contains("+fresh"), "{text}");
        assert!(!text.contains("token"), "ignored files stay out: {text}");
        let (short, cut) = diff(&root, &names[0], &names[1], 10).unwrap_or_else(|e| panic!("{e}"));
        assert!(cut && short.len() <= 10);
        assert_eq!(
            merge_base(&root, "main", &names[1]).unwrap_or_else(|e| panic!("{e}")),
            run(&["rev-parse", "HEAD"])
        );

        drop_refs(&root, prefix).unwrap_or_else(|e| panic!("{e}"));
        assert!(refs(&root, prefix).unwrap_or_else(|e| panic!("{e}")).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reverts_the_files_to_a_checkpoint_and_leaves_the_index_alone() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-revert-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap_or_else(|e| panic!("{e}"));
        let run = |args: &[&str]| git(&root, args).unwrap_or_else(|e| panic!("{e}"));
        run(&["init", "--quiet", "--initial-branch", "main"]);
        std::fs::write(root.join(".gitignore"), "secret\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("gone.txt"), "x\n").unwrap_or_else(|e| panic!("{e}"));
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-m",
            "one",
        ]);
        // At the checkpoint: a file only on disk, with a name git would read as a pattern.
        std::fs::write(root.join("src/[x].txt"), "draft\n").unwrap_or_else(|e| panic!("{e}"));
        let target = "refs/nexus/checkpoints/t_r/0000000004";
        checkpoint(&root, target).unwrap_or_else(|e| panic!("{e}"));

        std::fs::write(root.join("a.txt"), "two\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("src/[x].txt"), "changed\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("later.txt"), "new\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::remove_file(root.join("gone.txt")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("secret"), "token\n").unwrap_or_else(|e| panic!("{e}"));
        run(&["add", "a.txt"]);
        // A repository the agent cloned in: one entry in a checkpoint, a folder on disk.
        let clone = root.join("clone");
        std::fs::create_dir_all(&clone).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(clone.join("f"), "x\n").unwrap_or_else(|e| panic!("{e}"));
        for args in [
            &["init", "--quiet"][..],
            &["add", "f"],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                "c",
            ],
        ] {
            git(&clone, args).unwrap_or_else(|e| panic!("{e}"));
        }

        let mut plan = revert_plan(&root, target).unwrap_or_else(|e| panic!("{e}"));
        plan.sort();
        let expected: Vec<(String, bool)> = vec![
            ("a.txt".into(), true),
            ("clone".into(), false),
            ("gone.txt".into(), true),
            ("later.txt".into(), false),
            ("src/[x].txt".into(), true),
        ];
        assert_eq!(plan, expected);
        assert_eq!(revert_files(&root, target).unwrap_or_else(|e| panic!("{e}")), 5);
        assert!(!clone.exists());
        let read = |name: &str| std::fs::read_to_string(root.join(name)).ok();
        assert_eq!(read("a.txt").as_deref(), Some("one\n"));
        assert_eq!(read("gone.txt").as_deref(), Some("x\n"));
        assert_eq!(read("src/[x].txt").as_deref(), Some("draft\n"));
        assert_eq!(read("later.txt"), None);
        // Ignored files are not the session's to revert; the person's staging stays as it was.
        assert_eq!(read("secret").as_deref(), Some("token\n"));
        assert_eq!(run(&["diff", "--cached", "--name-only"]), "a.txt");
        assert!(revert_plan(&root, target).unwrap_or_else(|e| panic!("{e}")).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_checkpoint_leaves_out_cache_folders_and_big_files_and_a_revert_keeps_them() {
        let root = scratch_repo("cache-out");
        let write = |name: &str, text: &str| {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap_or(&root)).unwrap_or_else(|e| panic!("{e}"));
            std::fs::write(path, text).unwrap_or_else(|e| panic!("{e}"));
        };
        // An untracked build folder (cargo tags its target folder) and a big untracked file, before the turn.
        write(
            "out/target/CACHEDIR.TAG",
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        );
        write("out/target/debug/app.o", "object\n");
        write("out/notes.txt", "notes\n");
        std::fs::File::create(root.join("big.bin"))
            .and_then(|file| file.set_len(checkpoint_scope::MAX_FILE + 1))
            .unwrap_or_else(|e| panic!("{e}"));
        let target = "refs/nexus/checkpoints/t_c/0000000000";
        checkpoint(&root, target).unwrap_or_else(|e| panic!("{e}"));
        let files = git(&root, &["ls-tree", "-r", "--name-only", target]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(files.lines().collect::<Vec<_>>(), ["a.txt", "out/notes.txt"]);

        // The turn builds again, shrinks the big file under the limit and writes a file of its own.
        write("out/target/debug/next.o", "object\n");
        std::fs::write(root.join("big.bin"), "small\n").unwrap_or_else(|e| panic!("{e}"));
        write("made.txt", "new\n");
        let mut plan = revert_plan(&root, target).unwrap_or_else(|e| panic!("{e}"));
        plan.sort();
        assert_eq!(plan, [("made.txt".to_string(), false)]);
        revert_files(&root, target).unwrap_or_else(|e| panic!("{e}"));
        assert!(!root.join("made.txt").exists());
        assert!(root.join("big.bin").exists());
        assert!(root.join("out/target/debug/next.o").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_checkpoint_past_its_deadline_fails() {
        let root = scratch_repo("deadline");
        assert_eq!(snapshot_until(&root, Instant::now()), Err(checkpoint_scope::too_slow()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn checkpoints_a_tree_whose_index_is_missing_or_ahead_of_head() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-cp-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap_or_else(|e| panic!("{e}"));
        let run = |args: &[&str]| git(&root, args).unwrap_or_else(|e| panic!("{e}"));
        run(&["init", "--quiet", "--initial-branch", "main"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("gone.txt"), "x\n").unwrap_or_else(|e| panic!("{e}"));
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-m",
            "one",
        ]);

        // Staged but deleted from disk, staged new file, unstaged edit: the snapshot is what is on disk.
        std::fs::remove_file(root.join("gone.txt")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("staged.txt"), "s\n").unwrap_or_else(|e| panic!("{e}"));
        run(&["add", "staged.txt"]);
        std::fs::write(root.join("a.txt"), "two\n").unwrap_or_else(|e| panic!("{e}"));
        let first = "refs/nexus/checkpoints/t_2/00000001";
        checkpoint(&root, first).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(run(&["ls-tree", "-r", "--name-only", first]), "a.txt\nstaged.txt");
        assert_eq!(run(&["show", &format!("{first}:a.txt")]), "two");
        assert_eq!(run(&["rev-parse", &format!("{first}^")]), run(&["rev-parse", "HEAD"]));

        // No index file at all: it falls back to HEAD and still snapshots the disk.
        std::fs::remove_file(root.join(".git/index")).unwrap_or_else(|e| panic!("{e}"));
        let second = "refs/nexus/checkpoints/t_2/00000002";
        checkpoint(&root, second).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(run(&["ls-tree", "-r", "--name-only", second]), "a.txt\nstaged.txt");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn checkpoints_files_hidden_with_assume_unchanged_or_skip_worktree() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-cp-hidden-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).unwrap_or_else(|e| panic!("{e}"));
        let run = |args: &[&str]| git(&root, args).unwrap_or_else(|e| panic!("{e}"));
        run(&["init", "--quiet", "--initial-branch", "main"]);
        for name in ["assumed.txt", "skipped.txt", "plain.txt"] {
            std::fs::write(root.join(name), "one\n").unwrap_or_else(|e| panic!("{e}"));
        }
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-m",
            "one",
        ]);
        run(&["update-index", "--assume-unchanged", "assumed.txt"]);
        run(&["update-index", "--skip-worktree", "skipped.txt"]);
        for name in ["assumed.txt", "skipped.txt", "plain.txt"] {
            std::fs::write(root.join(name), "two\n").unwrap_or_else(|e| panic!("{e}"));
        }

        let cp = "refs/nexus/checkpoints/t_3/00000001";
        checkpoint(&root, cp).unwrap_or_else(|e| panic!("{e}"));
        for name in ["assumed.txt", "skipped.txt", "plain.txt"] {
            assert_eq!(run(&["show", &format!("{cp}:{name}")]), "two", "{name}");
        }
        // The person's own index keeps its bits.
        assert_eq!(
            run(&["ls-files", "-v", "assumed.txt", "skipped.txt"]),
            "h assumed.txt\nS skipped.txt"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn diffs_with_a_and_b_prefixes_whatever_the_config_says() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-prefix-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).unwrap_or_else(|e| panic!("{e}"));
        let run = |args: &[&str]| git(&root, args).unwrap_or_else(|e| panic!("{e}"));
        let commit = |message: &str| {
            run(&["add", "."]);
            run(&[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                message,
            ]);
        };
        run(&["init", "--quiet", "--initial-branch", "main"]);
        std::fs::write(root.join("x.bin"), [0u8, 1, 2]).unwrap_or_else(|e| panic!("{e}"));
        commit("one");
        std::fs::write(root.join("x.bin"), [0u8, 3, 4]).unwrap_or_else(|e| panic!("{e}"));
        commit("two");
        for (key, value) in [("diff.noprefix", "true"), ("diff.mnemonicPrefix", "true")] {
            run(&["config", key, value]);
            let (text, _) = diff(&root, "HEAD~1", "HEAD", 1 << 20).unwrap_or_else(|e| panic!("{e}"));
            assert!(text.starts_with("diff --git a/x.bin b/x.bin\n"), "{key}: {text}");
            run(&["config", "--unset", key]);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn diff_paths_are_relative_to_a_folder_below_the_repo_top() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-rel-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(root.join("api/src")).unwrap_or_else(|e| panic!("{e}"));
        let run = |args: &[&str]| git(&root, args).unwrap_or_else(|e| panic!("{e}"));
        run(&["init", "--quiet", "--initial-branch", "main"]);
        std::fs::write(root.join("api/src/a.ts"), "one\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("top.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
        let commit = |message: &str| {
            run(&["add", "."]);
            run(&[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                message,
            ]);
        };
        commit("one");
        std::fs::write(root.join("api/src/a.ts"), "two\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("top.txt"), "two\n").unwrap_or_else(|e| panic!("{e}"));
        commit("two");
        let (text, _) = diff(&root.join("api"), "HEAD~1", "HEAD", 1 << 20).unwrap_or_else(|e| panic!("{e}"));
        assert!(text.starts_with("diff --git a/src/a.ts b/src/a.ts\n"), "{text}");
        assert!(!text.contains("top.txt"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn commits_what_changed_and_pushes_the_branch() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-push-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
        let run = |dir: &Path, args: &[&str]| git(dir, args).unwrap_or_else(|e| panic!("{e}"));
        run(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        run(&repo, &["config", "user.name", "t"]);
        run(&repo, &["config", "user.email", "t@t"]);
        run(&repo, &["commit", "--quiet", "--allow-empty", "-m", "one"]);
        let remote = root.join("remote.git");
        run(&root, &["init", "--quiet", "--bare", &remote.to_string_lossy()]);
        run(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
        run(&repo, &["push", "--quiet", "origin", "main"]);
        let dir = root.join("wt");
        add_worktree(&repo, &dir, "nexus/fix", "origin/main").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(merge_base_branch(&dir, "nexus/fix").as_deref(), Some("main"));

        assert!(!commit_all(&dir, "Nothing yet", false).unwrap_or_else(|e| panic!("{e}")));
        std::fs::write(dir.join("fix.txt"), "fixed\n").unwrap_or_else(|e| panic!("{e}"));
        assert!(commit_all(&dir, "Fix the bug", false).unwrap_or_else(|e| panic!("{e}")));
        assert_eq!(run(&dir, &["log", "-1", "--format=%s"]), "Fix the bug");
        push(&dir, "nexus/fix").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(run(&remote, &["log", "-1", "--format=%s", "nexus/fix"]), "Fix the bug");
        // Pushed: removing the worktree is now safe.
        remove_worktree(&repo, &dir).unwrap_or_else(|e| panic!("{e}"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_commit_in_a_nexus_repository_names_nexus_desktop() {
        const CHILD: &str = "NEXUS_WORKBENCH_TRAILER_TEST";
        if std::env::var_os(CHILD).is_none() {
            // Isolate every git call, including commit_all, without changing other test threads' environment.
            let global = std::env::temp_dir().join(format!("nexus-workbench-gitconfig-{}", uuid::Uuid::new_v4()));
            std::fs::write(&global, "").unwrap_or_else(|e| panic!("{e}"));
            let status = process::command(std::env::current_exe().unwrap_or_else(|e| panic!("{e}")))
                .args([
                    "--exact",
                    "git::tests::a_commit_in_a_nexus_repository_names_nexus_desktop",
                ])
                .env(CHILD, "1")
                .env("GIT_CONFIG_GLOBAL", &global)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env_remove(nexus_client::commit_trailer::VARIABLE)
                .env_remove("GIT_CONFIG_COUNT")
                .status()
                .unwrap_or_else(|e| panic!("{e}"));
            let _ = std::fs::remove_file(global);
            assert!(status.success(), "the isolated commit trailer test failed");
            return;
        }
        let repo = std::env::temp_dir().join(format!("nexus-workbench-trailer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
        let run = |args: &[&str]| git(&repo, args).unwrap_or_else(|e| panic!("{e}"));
        run(&["init", "--quiet", "--initial-branch", "main"]);
        let hooks = repo.join(".git/no-machine-hooks");
        std::fs::create_dir(&hooks).expect("empty hooks directory");
        run(&[
            "config",
            "--local",
            "core.hooksPath",
            hooks.to_str().expect("hooks path"),
        ]);
        run(&["config", "user.name", "t"]);
        run(&["config", "user.email", "t@t"]);
        run(&["remote", "add", "origin", "git@github.com:nexus-xyz/nexus.git"]);
        // Someone outside Nexus: no trailer.
        std::fs::write(repo.join("other.txt"), "other\n").unwrap_or_else(|e| panic!("{e}"));
        assert!(commit_all(&repo, "Other work", false).unwrap_or_else(|e| panic!("{e}")));
        assert_eq!(run(&["log", "-1", "--format=%B"]), "Other work");
        std::fs::write(repo.join("fix.txt"), "fixed\n").unwrap_or_else(|e| panic!("{e}"));
        assert!(commit_all(&repo, "Fix the bug", true).unwrap_or_else(|e| panic!("{e}")));
        // Git before 2.54 runs no hooks from config, and commits as before.
        let hooks_in_config = git(
            &repo,
            &[
                "-c",
                "hook.probe.event=commit-msg",
                "-c",
                "hook.probe.command=true",
                "hook",
                "list",
                "commit-msg",
            ],
        )
        .is_ok_and(|hooks| hooks.lines().any(|line| line.trim() == "probe"));
        let expected = if hooks_in_config {
            "Fix the bug\n\nCo-authored-by: Nexus Desktop <desktop@nexus.inc>"
        } else {
            "Fix the bug"
        };
        assert_eq!(run(&["log", "-1", "--format=%B"]), expected);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn names_the_fix_for_a_signed_out_github_cli() {
        assert!(gh_error("To get started with GitHub CLI, please run:  gh auth login").contains("isn't signed in"));
        assert_eq!(
            gh_error("pull request create failed: no commits\n"),
            "pull request create failed: no commits"
        );
    }

    #[test]
    fn counts_what_a_commit_would_take_and_switches_branches() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-stat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap_or_else(|e| panic!("{e}"));
        let run = |args: &[&str]| git(&root, args).unwrap_or_else(|e| panic!("{e}"));
        let write = |name: &str, text: &[u8]| std::fs::write(root.join(name), text).unwrap_or_else(|e| panic!("{e}"));
        run(&["init", "--quiet", "--initial-branch", "main"]);
        write("a.txt", b"one\ntwo\nthree\n");
        run(&["add", "a.txt"]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-m",
            "one",
        ]);
        assert_eq!(uncommitted(&root), Ok(Stat::default()));

        write("a.txt", b"one\n2\nthree\nfour\n");
        write("new.txt", b"x\ny");
        write("blob.bin", b"\0\x01");
        assert_eq!(
            uncommitted(&root),
            Ok(Stat {
                files: 3,
                insertions: 4,
                deletions: 1
            })
        );

        run(&["branch", "other"]);
        assert_eq!(local_branches(&root).map(|list| list.len()), Ok(2));
        assert!(switch(&root, "--orphan").is_err());
        switch(&root, "other").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(branch(&root).as_deref(), Some("other"));
        // `git branch` refuses a name like this, but a fetch or update-ref can make one; it is a branch, not a flag.
        run(&["update-ref", "refs/heads/--orphan", "HEAD"]);
        switch(&root, "--orphan").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(branch(&root).as_deref(), Some("--orphan"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A scratch repository with one committed file, `a.txt`, under the system's temp folder.
    fn scratch_repo(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nexus-workbench-{name}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).unwrap_or_else(|e| panic!("{e}"));
        let run = |args: &[&str]| git(&root, args).unwrap_or_else(|e| panic!("{e}"));
        run(&["init", "--quiet", "--initial-branch", "main"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-m",
            "one",
        ]);
        root
    }

    /// What `git status` says in `dir`, sorted: `XY path` lines.
    fn status_of(dir: &Path) -> Vec<String> {
        let mut lines: Vec<String> = git(dir, &["status", "--porcelain", "--untracked-files=all"])
            .unwrap_or_else(|e| panic!("{e}"))
            .lines()
            .map(String::from)
            .collect();
        lines.sort();
        lines
    }

    #[test]
    fn a_move_carries_staged_unstaged_and_untracked_changes_and_leaves_the_folder_clean() {
        let root = scratch_repo("move");
        let write = |name: &str, text: &str| std::fs::write(root.join(name), text).unwrap_or_else(|e| panic!("{e}"));
        write(".gitignore", "secret.env\n");
        git(&root, &["add", ".gitignore"]).unwrap_or_else(|e| panic!("{e}"));
        git(
            &root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                "ignore",
            ],
        )
        .unwrap_or_else(|e| panic!("{e}"));
        write("a.txt", "staged\n");
        git(&root, &["add", "a.txt"]).unwrap_or_else(|e| panic!("{e}"));
        write("a.txt", "staged then edited\n");
        std::fs::create_dir_all(root.join("new")).unwrap_or_else(|e| panic!("{e}"));
        write("new/b.txt", "untracked\n");
        write("secret.env", "ignored\n");
        let before = status_of(&root);
        let dir = root.with_extension("move");
        let moved = move_to_worktree(&root, &dir, "nexus/move").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(moved.base, "main");
        assert_eq!(moved.left, None);
        // The same changes, the staged part staged.
        assert_eq!(status_of(&dir), before);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap_or_default(),
            "staged then edited\n"
        );
        assert_eq!(git(&dir, &["show", ":a.txt"]).unwrap_or_default(), "staged");
        assert_eq!(
            std::fs::read_to_string(dir.join("new/b.txt")).unwrap_or_default(),
            "untracked\n"
        );
        assert_eq!(branch(&dir).as_deref(), Some("nexus/move"));
        assert_eq!(
            git(&root, &["config", "branch.nexus/move.gh-merge-base"]).unwrap_or_default(),
            "main"
        );
        // The folder is clean, but for what git ignores: that is the person's, and never moves.
        assert_eq!(status_of(&root), Vec::<String>::new());
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap_or_default(), "one\n");
        assert!(!root.join("new/b.txt").exists());
        assert!(root.join("secret.env").exists() && !dir.join("secret.env").exists());
        // The snapshot holds what moved.
        assert_eq!(
            git(&root, &["show", &format!("{}:new/b.txt", moved.snapshot)]).unwrap_or_default(),
            "untracked"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_move_that_fails_leaves_the_folder_as_it_was() {
        let root = scratch_repo("move-fails");
        std::fs::write(root.join("a.txt"), "mine\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("c.txt"), "new\n").unwrap_or_else(|e| panic!("{e}"));
        let before = status_of(&root);
        // The branch is taken: git refuses the worktree.
        git(&root, &["branch", "taken"]).unwrap_or_else(|e| panic!("{e}"));
        let dir = root.with_extension("taken");
        assert!(move_to_worktree(&root, &dir, "taken").is_err());
        assert!(move_to_worktree(&root, &dir, "bad..name").is_err());
        // A repository inside with changes: only its commit would move.
        let inner = root.join("inner");
        std::fs::create_dir_all(&inner).unwrap_or_else(|e| panic!("{e}"));
        git(&inner, &["init", "--quiet"]).unwrap_or_else(|e| panic!("{e}"));
        git(
            &inner,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "i",
            ],
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let error = move_to_worktree(&root, &dir, "nexus/inner").err().unwrap_or_default();
        assert!(error.contains("repository inside"), "{error}");
        std::fs::remove_dir_all(&inner).unwrap_or_else(|e| panic!("{e}"));
        // Mid-merge.
        let merge_head = PathBuf::from(git(&root, &["rev-parse", "--git-path", "MERGE_HEAD"]).unwrap_or_default());
        let merge_head = if merge_head.is_absolute() {
            merge_head
        } else {
            root.join(merge_head)
        };
        let head = git(&root, &["rev-parse", "HEAD"]).unwrap_or_default();
        std::fs::write(&merge_head, format!("{head}\n")).unwrap_or_else(|e| panic!("{e}"));
        assert!(move_to_worktree(&root, &dir, "nexus/merge").is_err());
        std::fs::remove_file(&merge_head).unwrap_or_else(|e| panic!("{e}"));
        // Nothing moved, nothing was left behind.
        assert_eq!(status_of(&root), before);
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap_or_default(),
            "mine\n"
        );
        assert!(!dir.exists());
        assert!(!has_branch(&root, "nexus/inner") && !has_branch(&root, "nexus/merge"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_move_from_a_detached_head_counts_against_its_commit() {
        let root = scratch_repo("move-detached");
        let head = git(&root, &["rev-parse", "HEAD"]).unwrap_or_default();
        git(&root, &["checkout", "--quiet", "--detach"]).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("a.txt"), "two\n").unwrap_or_else(|e| panic!("{e}"));
        let dir = root.with_extension("detached");
        let moved = move_to_worktree(&root, &dir, "nexus/detached").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(moved.base, head);
        assert!(git(&root, &["config", "branch.nexus/detached.gh-merge-base"]).is_err());
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap_or_default(), "two\n");
        assert_eq!(status_of(&root), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_the_repository_ignores_is_not_in_it() {
        let root = scratch_repo("in-repo");
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap_or_else(|e| panic!("{e}"));
        for dir in ["app", "build/out"] {
            std::fs::create_dir_all(root.join(dir)).unwrap_or_else(|e| panic!("{e}"));
        }
        assert!(in_repo(&root));
        assert!(in_repo(&root.join("app")));
        assert!(!in_repo(&root.join("build/out")));
        assert!(!in_repo(&std::env::temp_dir()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reading_a_repository_leaves_its_index_alone() {
        // The git panel reads while the agent works: if a read rewrote the index, it would hold `index.lock`, and
        // the agent's own `git add` or `git commit` at that moment would fail.
        let root = scratch_repo("index-lock");
        // Same content, another modified time: git's stat cache is stale, which a refresh would write back.
        let file = std::fs::File::options()
            .write(true)
            .open(root.join("a.txt"))
            .unwrap_or_else(|e| panic!("{e}"));
        file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
            .unwrap_or_else(|e| panic!("{e}"));
        let index = || std::fs::read(root.join(".git/index")).unwrap_or_else(|e| panic!("{e}"));
        let before = index();
        assert_eq!(uncommitted(&root).map(|stat| stat.files), Ok(0));
        would_lose_work(&root).ok();
        assert!(before == index(), "a read rewrote the index");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn removes_a_worktree_with_submodules_only_when_nothing_in_them_would_be_lost() {
        let root = std::env::temp_dir().join(format!("nexus-workbench-sub-{}", uuid::Uuid::new_v4().simple()));
        let (sub, repo, remote, dir) = (
            root.join("sub"),
            root.join("repo"),
            root.join("remote.git"),
            root.join("wt"),
        );
        std::fs::create_dir_all(&sub).unwrap_or_else(|e| panic!("{e}"));
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
        let run = |dir: &Path, args: &[&str]| {
            let mut all = vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "protocol.file.allow=always",
            ];
            all.extend(args);
            git(dir, &all).unwrap_or_else(|e| panic!("{e}"))
        };
        run(&sub, &["init", "--quiet", "--initial-branch", "main"]);
        run(&sub, &["commit", "--quiet", "--allow-empty", "-m", "s"]);
        run(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        run(&repo, &["submodule", "add", "--quiet", &sub.to_string_lossy(), "sub"]);
        run(&repo, &["commit", "--quiet", "-m", "with a submodule"]);
        run(&root, &["init", "--quiet", "--bare", &remote.to_string_lossy()]);
        run(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
        run(&repo, &["push", "--quiet", "origin", "main"]);
        add_worktree(&repo, &dir, "nexus/sub", "origin/main").unwrap_or_else(|e| panic!("{e}"));
        run(&dir, &["submodule", "update", "--quiet", "--init"]);

        // An edit inside the submodule, even one the config tells `status` to ignore, would go with it: refused.
        run(&dir, &["config", "submodule.sub.ignore", "all"]);
        std::fs::write(dir.join("sub/draft.txt"), "x\n").unwrap_or_else(|e| panic!("{e}"));
        let refused = remove_worktree(&repo, &dir).err().unwrap_or_default();
        assert!(refused.contains("uncommitted"), "{refused}");
        std::fs::remove_file(dir.join("sub/draft.txt")).unwrap_or_else(|e| panic!("{e}"));
        run(&dir, &["config", "--unset", "submodule.sub.ignore"]);

        // A commit inside the submodule that is on no remote would go with the worktree: refused.
        run(&dir.join("sub"), &["commit", "--quiet", "--allow-empty", "-m", "local"]);
        run(&dir, &["commit", "--quiet", "-am", "bump"]);
        run(&dir, &["push", "--quiet", "origin", "HEAD:refs/heads/nexus/sub"]);
        let refused = remove_worktree(&repo, &dir).err().unwrap_or_default();
        assert!(refused.contains("aren't pushed"), "{refused}");

        // Pushed there too: the worktree goes, submodule and all.
        run(&dir.join("sub"), &["push", "--quiet", "origin", "HEAD:refs/heads/kept"]);
        run(&dir.join("sub"), &["fetch", "--quiet", "origin"]);
        remove_worktree(&repo, &dir).unwrap_or_else(|e| panic!("{e}"));
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A bare remote with one commit on main, and two clones of it with an identity of their own: `mine` (the person's
    /// folder) and `theirs` (someone else pushing). Returns root, mine, theirs.
    fn shared_repo(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("nexus-workbench-{name}-{}", uuid::Uuid::new_v4().simple()));
        let (remote, seed) = (root.join("remote.git"), root.join("seed"));
        std::fs::create_dir_all(&seed).unwrap_or_else(|e| panic!("{e}"));
        let run = |dir: &Path, args: &[&str]| git(dir, args).unwrap_or_else(|e| panic!("{e}"));
        run(
            &root,
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch",
                "main",
                &remote.to_string_lossy(),
            ],
        );
        run(&seed, &["init", "--quiet", "--initial-branch", "main"]);
        std::fs::write(seed.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
        run(&seed, &["add", "."]);
        commit(&seed, "one");
        run(&seed, &["push", "--quiet", &remote.to_string_lossy(), "main"]);
        let clone = |name: &str| {
            let dir = root.join(name);
            run(
                &root,
                &["clone", "--quiet", &remote.to_string_lossy(), &dir.to_string_lossy()],
            );
            run(&dir, &["config", "user.name", name]);
            run(&dir, &["config", "user.email", "t@t"]);
            dir
        };
        (root.clone(), clone("mine"), clone("theirs"))
    }

    fn change(dir: &Path, file: &str, text: &str, message: &str) {
        std::fs::write(dir.join(file), text).unwrap_or_else(|e| panic!("{e}"));
        git(dir, &["add", file]).unwrap_or_else(|e| panic!("{e}"));
        git(dir, &["commit", "--quiet", "-m", message]).unwrap_or_else(|e| panic!("{e}"));
    }

    fn counts(dir: &Path) -> (u32, u32) {
        tracking(dir).map(|t| (t.ahead, t.behind)).unwrap_or((99, 99))
    }

    #[test]
    fn counts_ahead_and_behind_after_a_fetch_and_fast_forwards_a_pull() {
        let (root, mine, theirs) = shared_repo("sync-ff");
        assert_eq!(upstream(&mine).as_deref(), Some("origin/main"));
        assert_eq!(counts(&mine), (0, 0));
        change(&theirs, "b.txt", "b\n", "theirs");
        git(&theirs, &["push", "--quiet"]).unwrap_or_else(|e| panic!("{e}"));
        change(&mine, "c.txt", "c\n", "mine");
        assert_eq!(counts(&mine), (1, 0), "nothing fetched yet");
        fetch(&mine).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(counts(&mine), (1, 1));
        // A fetch leaves no FETCH_HEAD behind.
        assert!(!mine.join(".git/FETCH_HEAD").exists());

        // Only behind: a pull moves forward, with an uncommitted edit elsewhere left as it was.
        git(&mine, &["reset", "--quiet", "--hard", "origin/main~1"]).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(mine.join("a.txt"), "edited\n").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(pull(&mine, false), Ok(Pulled::Done));
        assert_eq!(counts(&mine), (0, 0));
        assert!(mine.join("b.txt").exists());
        assert_eq!(
            std::fs::read_to_string(mine.join("a.txt")).ok().as_deref(),
            Some("edited\n")
        );
        // Up to date already: nothing to do, no error.
        assert_eq!(pull(&mine, false), Ok(Pulled::Done));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_diverged_branch_pulls_only_by_rebase_and_its_checkpoints_stay() {
        let (root, mine, theirs) = shared_repo("sync-rebase");
        change(&theirs, "b.txt", "b\n", "theirs");
        git(&theirs, &["push", "--quiet"]).unwrap_or_else(|e| panic!("{e}"));
        change(&mine, "c.txt", "c\n", "mine");
        let cp = "refs/nexus/checkpoints/t_s/0000000004";
        checkpoint(&mine, cp).unwrap_or_else(|e| panic!("{e}"));
        let cp_commit = git(&mine, &["rev-parse", cp]).unwrap_or_default();

        let error = pull(&mine, false).err().unwrap_or_default();
        assert!(error.contains("both have new commits"), "{error}");
        assert_eq!(counts(&mine), (1, 1), "a refused pull changes nothing");

        // Uncommitted tracked changes are in a rebase's way: refused, nothing touched.
        std::fs::write(mine.join("a.txt"), "edited\n").unwrap_or_else(|e| panic!("{e}"));
        let error = pull(&mine, true).err().unwrap_or_default();
        assert!(error.contains("Commit or discard"), "{error}");
        git(&mine, &["checkout", "--quiet", "--", "a.txt"]).unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(pull(&mine, true), Ok(Pulled::Done));
        assert_eq!(counts(&mine), (1, 0));
        assert_eq!(git(&mine, &["log", "-1", "--format=%s"]).ok().as_deref(), Some("mine"));
        assert_eq!(git(&mine, &["rev-parse", cp]).ok(), Some(cp_commit));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_rebase_that_conflicts_is_aborted_and_names_the_files() {
        let (root, mine, theirs) = shared_repo("sync-conflict");
        change(&theirs, "a.txt", "theirs\n", "theirs");
        git(&theirs, &["push", "--quiet"]).unwrap_or_else(|e| panic!("{e}"));
        change(&mine, "a.txt", "mine\n", "mine");
        let cp = "refs/nexus/checkpoints/t_c/0000000004";
        checkpoint(&mine, cp).unwrap_or_else(|e| panic!("{e}"));
        let (head, cp_commit) = (git(&mine, &["rev-parse", "HEAD"]), git(&mine, &["rev-parse", cp]));
        std::fs::write(mine.join("untracked.txt"), "x\n").unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(pull(&mine, true), Ok(Pulled::Conflict(vec!["a.txt".into()])));
        assert!(!operation_in_progress(&mine), "the rebase is aborted");
        assert_eq!(git(&mine, &["rev-parse", "HEAD"]), head);
        assert_eq!(checked_out(&mine).as_deref(), Some("main"));
        assert_eq!(
            std::fs::read_to_string(mine.join("a.txt")).ok().as_deref(),
            Some("mine\n")
        );
        assert!(mine.join("untracked.txt").exists());
        assert_eq!(git(&mine, &["rev-parse", cp]), cp_commit);
        assert_eq!(counts(&mine), (1, 1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_untracked_file_in_a_rebases_way_fails_cleanly() {
        let (root, mine, theirs) = shared_repo("sync-untracked");
        change(&theirs, "new.txt", "theirs\n", "theirs");
        git(&theirs, &["push", "--quiet"]).unwrap_or_else(|e| panic!("{e}"));
        change(&mine, "c.txt", "c\n", "mine");
        std::fs::write(mine.join("new.txt"), "mine, not committed\n").unwrap_or_else(|e| panic!("{e}"));
        let head = git(&mine, &["rev-parse", "HEAD"]);

        assert!(pull(&mine, true).is_err());
        assert!(!operation_in_progress(&mine), "nothing left half-done");
        assert_eq!(git(&mine, &["rev-parse", "HEAD"]), head);
        assert_eq!(checked_out(&mine).as_deref(), Some("main"));
        assert_eq!(
            std::fs::read_to_string(mine.join("new.txt")).ok().as_deref(),
            Some("mine, not committed\n")
        );
        // Same for a fast-forward.
        git(&mine, &["reset", "--quiet", "--hard", "HEAD~1"]).unwrap_or_else(|e| panic!("{e}"));
        let error = pull(&mine, false).err().unwrap_or_default();
        assert!(error.contains("overwrite"), "{error}");
        assert_eq!(counts(&mine), (0, 1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_detached_head_cannot_pull() {
        let (root, mine, _) = shared_repo("sync-detached");
        git(&mine, &["switch", "--quiet", "--detach", "HEAD"]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(tracking(&mine), None);
        assert!(
            pull(&mine, true)
                .err()
                .unwrap_or_default()
                .contains("Check out a branch")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_branch_with_no_upstream_says_there_is_nothing_to_pull() {
        let (root, mine, _) = shared_repo("sync-none");
        create_branch(&mine, "feature/x").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(checked_out(&mine).as_deref(), Some("feature/x"));
        assert_eq!(tracking(&mine), None);
        let error = pull(&mine, false).err().unwrap_or_default();
        assert!(error.contains("doesn't track a remote branch"), "{error}");
        assert!(
            create_branch(&mine, "main")
                .err()
                .unwrap_or_default()
                .contains("already")
        );
        assert!(
            create_branch(&mine, "bad..name")
                .err()
                .unwrap_or_default()
                .contains("not a valid")
        );
        // Nothing to fetch is no error.
        fetch(&mine).unwrap_or_else(|e| panic!("{e}"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_worktree_counts_against_the_branch_it_was_made_from() {
        let (root, mine, theirs) = shared_repo("sync-base");
        let dir = root.join("wt");
        add_worktree(&mine, &dir, "nexus/a", "origin/main").unwrap_or_else(|e| panic!("{e}"));
        change(&dir, "w.txt", "w\n", "work");
        change(&theirs, "b.txt", "b\n", "theirs");
        git(&theirs, &["push", "--quiet"]).unwrap_or_else(|e| panic!("{e}"));
        fetch(&dir).unwrap_or_else(|e| panic!("{e}"));
        // Until its first push, git has it track the remote branch it was made from: the same counts.
        assert_eq!(tracking(&dir), base_tracking(&dir));
        assert_eq!(
            base_tracking(&dir),
            Some(Tracking {
                name: "origin/main".into(),
                ahead: 1,
                behind: 1
            })
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_branch_prefix_is_checked_as_git_checks_a_branch_name() {
        let dir = std::env::temp_dir();
        for prefix in [
            "", "nexus/", "jd/", "team/jd-", "feat_", "a.b/", "@/", "x@y/", " a/", "a b/", "a~/", "a^/", "a:/", "a?/",
            "a*/", "a[/", "a\\b/", "a./", "-a/", "/a/", "a//", "a..b/", ".a/", "a/.b/", "a.lock/", "a@{/", "\t/",
        ] {
            let git_says = git(&dir, &["check-ref-format", "--branch", &format!("{prefix}x")]).is_ok();
            assert_eq!(check_branch_prefix(prefix).is_ok(), git_says, "{prefix:?}");
        }
        assert!(check_branch_prefix(&"a".repeat(61)).is_err());
        assert_eq!(
            check_branch_prefix("my team/").err().as_deref(),
            Some("\"my team/\" can't start a branch name: it has a space.")
        );
    }

    #[test]
    fn reads_a_merged_pull_requests_head() {
        assert_eq!(
            merged_head_of(r#"{"state":"MERGED","headRefOid":"abc123"}"#).as_deref(),
            Some("abc123")
        );
        assert_eq!(merged_head_of(r#"{"state":"OPEN","headRefOid":"abc123"}"#), None);
        assert_eq!(merged_head_of(r#"{"state":"CLOSED","headRefOid":"abc123"}"#), None);
        assert_eq!(merged_head_of(r#"{"state":"MERGED","headRefOid":""}"#), None);
        assert_eq!(merged_head_of("no pull requests found"), None);
    }

    /// A squash merge leaves the branch's commits on no remote once its branch is deleted: only the pull request's head,
    /// named, counts them as pushed. Uncommitted changes still keep the worktree.
    #[test]
    fn a_merged_heads_commits_count_as_pushed() {
        let (root, mine, _theirs) = shared_repo("merged-head");
        let dir = root.join("wt");
        add_worktree(&mine, &dir, "nexus/a", "origin/main").unwrap_or_else(|e| panic!("{e}"));
        change(&dir, "w.txt", "w\n", "work");
        let head = git(&dir, &["rev-parse", "HEAD"]).unwrap_or_else(|e| panic!("{e}"));
        assert!(would_lose_work(&dir).is_err());
        would_lose_work_beyond(&dir, Some(&head)).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.join("w.txt"), "changed\n").unwrap_or_else(|e| panic!("{e}"));
        assert!(
            would_lose_work_beyond(&dir, Some(&head))
                .err()
                .unwrap_or_default()
                .contains("uncommitted")
        );
        git(&dir, &["checkout", "--quiet", "--", "w.txt"]).unwrap_or_else(|e| panic!("{e}"));
        remove_worktree_beyond(&mine, &dir, Some(&head)).unwrap_or_else(|e| panic!("{e}"));
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn patch_since_a_commit_is_cut_after_a_whole_line_and_lists_untracked_files_apart() {
        let root = scratch_repo("patch-since");
        std::fs::write(root.join("a.txt"), "ééé\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("new.txt"), "x\n").unwrap_or_else(|e| panic!("{e}"));
        let (whole, cut) = patch_since(&root, "HEAD", 100_000).unwrap_or_else(|e| panic!("{e}"));
        assert!(!cut && whole.contains("-one\n+ééé"), "{whole}");
        assert!(!whole.contains("new.txt"));
        let at = whole.find('é').unwrap_or_else(|| panic!("{whole}")) + 1;
        let (part, cut) = patch_since(&root, "HEAD", at).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            cut && part.len() < at && part.ends_with("-one\n") && whole.starts_with(&part),
            "{part:?}"
        );
        assert_eq!(untracked_files(&root), Ok(vec!["new.txt".to_string()]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_subjects_newest_first_and_the_first_pull_request_template_github_would() {
        let root = scratch_repo("subjects");
        assert_eq!(subjects(&root, "HEAD", 20), vec!["one".to_string()]);
        assert_eq!(subjects(&root, "no-such-ref", 20), Vec::<String>::new());
        assert_eq!(pull_request_template(&root, 100), None);
        std::fs::create_dir_all(root.join("docs")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join("docs/pull_request_template.md"), "docs").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(pull_request_template(&root, 100).as_deref(), Some("docs"));
        std::fs::create_dir_all(root.join(".github")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.join(".github/pull_request_template.md"), "## Summary\nlong")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(pull_request_template(&root, 10).as_deref(), Some("## Summary"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn patches_keep_a_last_context_line_that_holds_a_space() {
        let root = scratch_repo("blank-last");
        let run = |args: &[&str]| git(&root, args).unwrap_or_else(|e| panic!("{e}"));
        let commit = |message: &str| {
            run(&["add", "."]);
            run(&[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                message,
            ]);
        };
        std::fs::write(root.join("notes.md"), "# Old\none\ntwo\n\n").unwrap_or_else(|e| panic!("{e}"));
        commit("old");
        std::fs::write(root.join("notes.md"), "# New\none\ntwo\n\n").unwrap_or_else(|e| panic!("{e}"));
        let (since, cut) = patch_since(&root, "HEAD", 1 << 20).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            !cut && since.ends_with("@@ -1,4 +1,4 @@\n-# Old\n+# New\n one\n two\n \n"),
            "{since:?}"
        );
        commit("new");
        let (text, cut) = diff(&root, "HEAD~1", "HEAD", 1 << 20).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            !cut && text.ends_with("@@ -1,4 +1,4 @@\n-# Old\n+# New\n one\n two\n \n"),
            "{text:?}"
        );
        // git reads it back whole: the folder is at HEAD, so the patch applies in reverse.
        let file = root.join(".git/blank-last.patch");
        std::fs::write(&file, &text).unwrap_or_else(|e| panic!("{e}"));
        run(&["apply", "--check", "--reverse", &file.to_string_lossy()]);
        std::fs::write(&file, text.trim_end()).unwrap_or_else(|e| panic!("{e}"));
        assert!(git(&root, &["apply", "--check", "--reverse", &file.to_string_lossy()]).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_comment_after_a_blank_context_line_anchors_to_its_line_whatever_suppress_blank_empty_says() {
        let root = scratch_repo("suppress-blank-empty");
        let run = |args: &[&str]| git(&root, args).unwrap_or_else(|e| panic!("{e}"));
        run(&["config", "diff.suppressBlankEmpty", "true"]);
        let commit = |message: &str| {
            run(&["add", "."]);
            run(&[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                message,
            ]);
        };
        std::fs::write(root.join("a.txt"), "one\n\ntwo\nthree\n").unwrap_or_else(|e| panic!("{e}"));
        commit("blank");
        std::fs::write(root.join("a.txt"), "one\n\ntwo\nTHREE\n").unwrap_or_else(|e| panic!("{e}"));
        let (since, _) = patch_since(&root, "HEAD", 1 << 20).unwrap_or_else(|e| panic!("{e}"));
        commit("upper");
        let (between, _) = diff(&root, "HEAD~1", "HEAD", 1 << 20).unwrap_or_else(|e| panic!("{e}"));
        for patch in [since, between] {
            let files = crate::diff_comment::parse_lines(&patch);
            let added = files[0].iter().find(|line| line.sign == '+');
            assert_eq!(added.and_then(|line| line.new), Some(4), "{patch:?}");
            let picked = added.map(std::slice::from_ref).unwrap_or_default();
            assert_eq!(crate::diff_comment::range_label(picked), "line 4", "{patch:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn patch_is_cut_after_its_last_whole_line_and_never_inside_a_header() {
        let patch = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-a\n+b\n c\n";
        let cut = |max: usize| {
            let mut text = patch.to_string();
            let cut = cut_patch(&mut text, max);
            (text, cut)
        };
        assert_eq!(cut(patch.len()), (patch.to_string(), false));
        // Inside a source line, and right after one.
        let after_removed = patch.find("+b").unwrap_or_else(|| panic!("{patch}"));
        assert_eq!(cut(after_removed + 1).0, patch[..after_removed]);
        assert_eq!(cut(after_removed), (patch[..after_removed].to_string(), true));
        // Inside the hunk header: the header goes whole.
        let hunk = patch.find("@@").unwrap_or_else(|| panic!("{patch}"));
        assert_eq!(cut(hunk + 5).0, patch[..hunk]);
        // Inside `+++ `: its `--- ` line goes too.
        let new_side = patch.find("+++").unwrap_or_else(|| panic!("{patch}"));
        assert_eq!(cut(new_side + 2).0, "diff --git a/x b/x\n");
        // A first line longer than the cut keeps whole characters.
        let mut long = "éééé".to_string();
        assert!(cut_patch(&mut long, 3) && long == "é");
    }
}
