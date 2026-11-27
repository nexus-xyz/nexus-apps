//! A repository cloned as a new project (`wb_project_clone`): a URL, or a GitHub `owner/repo` that the person's GitHub
//! CLI sign-in finds, cloned with `git clone --progress` into `<parent>/<name>`. The window polls where the clone stands
//! (`wb_project_clone_progress`), the same on this computer and through a Nexus host. When the host refuses the
//! machine's SSH key, the clone runs again over HTTPS, as T3 Code's does.
//!
//! On a cloud machine a clone is shallow and blobless: one commit of the default branch, without tags
//! (`--depth=1 --single-branch --no-tags --filter=blob:none`), the single-commit fetch Nexus Review's sandbox makes.
//! A large repository is then ready in seconds. Its other branches are tracked
//! at once, and its history comes after, in the background ([`deepen`]); a failure there is only logged. git fetches a
//! file's content when a checkout or a diff needs it. Its host turns this on ([`blobless_when`]). A clone on the
//! person's own computer or server stays whole, so it works offline.
//!
//! A cloud machine may start from its organization's saved copy of the repository: Gateway's builder cloned
//! it the same way (the clone alone, no `.agents/setup`) and left a marker that names it ([`WARM_MARKER`]). A clone of that
//! repository then moves the copy into place and fetches only its new commits, with the person's own GitHub sign-in
//! ([`warm`]). A copy that fails to update is removed, and the clone runs as usual. A usual clone runs no repository code:
//! [`agents_setup`] exists for a setup a person or organization opted into, and has no caller in a clone.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{git, process};

/// What a core that clones (`wb_project_clone`) says in its features.
pub const FEATURE: &str = "project-clone";

/// How long a clone may go without a word from git before it is given up on (and killed with what it started). git
/// reports about once a second while it receives objects, so only a stalled network or transport gets here.
pub const STALL_LIMIT: Duration = Duration::from_secs(5 * 60);

/// What git said last, of the lines a clone keeps for its error message.
const KEPT_LINES: usize = 40;

/// How long a cloud clone's history may take to arrive in the background ([`deepen`]) before it is given up on.
pub(crate) const DEEPEN_LIMIT: Duration = Duration::from_secs(30 * 60);

/// Under the home folder: the marker of a saved copy, `{"repo": "<owner/name>", "path": "<its folder>"}`.
/// Gateway's builder writes it; the first clone of that repository takes the copy.
pub const WARM_MARKER: &str = ".nexus/warm/repo.json";

/// A repository's own setup, run once after its clone on a cloud machine when it is executable.
pub const AGENTS_SETUP: &str = ".agents/setup";

/// How long `.agents/setup` may run: the cap on a run on the machine.
const AGENTS_SETUP_LIMIT: Duration = Duration::from_secs(540);

/// A repository's per-person hook, run on every boot and wake of a cloud machine once the person's credentials are in
/// place, when it is executable. The hook for steps that need the person (a login, a token-backed fetch);
/// `.agents/setup` never sees their credentials.
pub const AGENTS_RESUME: &str = ".agents/resume";

/// How long `.agents/resume` may run. It runs on every wake, so it is short: past this it is stopped and reported.
pub const AGENTS_RESUME_LIMIT: Duration = Duration::from_secs(10);

/// Under the home folder: what the last `.agents/resume` printed.
const AGENTS_RESUME_LOG: &str = ".nexus/agents-resume.log";

/// Under the home folder: what the last `.agents/setup` printed.
const AGENTS_SETUP_LOG: &str = ".nexus/agents-setup.log";

/// The file whose presence makes clones shallow and blobless: a cloud machine's `cloud.json`. Read at each clone,
/// because Gateway may write it after the host started. None: whole clones.
static BLOBLESS_WHEN: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The projects whose history is being fetched right now, so a second caller waits for that fetch instead of starting
/// another one against the same `shallow.lock`.
static DEEPENING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Held while `dest`'s deepen runs.
struct Deepening(PathBuf);

impl Deepening {
    /// None when a deepen of `dest` already runs.
    fn start(dest: &Path) -> Option<Deepening> {
        let mut running = DEEPENING.lock().ok()?;
        if running.iter().any(|d| d == dest) {
            return None;
        }
        running.push(dest.to_path_buf());
        Some(Deepening(dest.to_path_buf()))
    }
}

impl Drop for Deepening {
    fn drop(&mut self) {
        if let Ok(mut running) = DEEPENING.lock() {
            running.retain(|d| d != &self.0);
        }
    }
}

fn deepening(dest: &Path) -> bool {
    DEEPENING.lock().is_ok_and(|running| running.iter().any(|d| d == dest))
}

/// Makes clones shallow and blobless while `marker` exists (a cloud machine's host passes its `cloud.json`); None turns
/// it off.
pub fn blobless_when(marker: Option<PathBuf>) {
    if let Ok(mut slot) = BLOBLESS_WHEN.lock() {
        *slot = marker;
    }
}

fn blobless() -> bool {
    BLOBLESS_WHEN
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .is_some_and(|marker| marker.exists())
}

/// `git clone`'s own arguments for `url`. `--progress` keeps git talking when its stderr is a pipe, so [`STALL_LIMIT`]
/// stops only a clone that really stalled. On a cloud machine (`cloud`): one commit of the default branch, no tags.
fn clone_args(url: &str, cloud: bool) -> Vec<&str> {
    let mut args = vec!["clone", "--progress"];
    if cloud {
        args.extend(["--filter=blob:none", "--depth=1", "--single-branch", "--no-tags"]);
    }
    args.extend(["--", url]);
    args
}

/// A GitHub token for one cloud machine's own setup at boot ([`crate::cloud_setup`]): a GitHub App token
/// that may only read the one repository, for an hour at most. git gets it as an HTTP header in its environment only
/// (`GIT_CONFIG_*`, as Gateway's warm builder does), never in its config, the remote URL or a file. It is never
/// printed: its `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct GithubAuth(String);

impl GithubAuth {
    /// None for a token that is not letters, digits and `_` (every GitHub token is).
    pub fn new(token: &str) -> Option<GithubAuth> {
        let plain =
            !token.is_empty() && token.len() <= 512 && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        plain.then(|| GithubAuth(token.to_string()))
    }

    /// The header's value: the token as GitHub's basic `x-access-token` user.
    fn header(&self) -> String {
        use base64::Engine as _;
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{}", self.0));
        format!("AUTHORIZATION: basic {basic}")
    }
}

impl std::fmt::Debug for GithubAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GithubAuth(..)")
    }
}

/// git, with the GitHub CLI's sign-in for `https://github.com` when `github_https`, or `auth` for it when given, never
/// asking on a terminal.
fn git_command(path: &str, github_https: bool, auth: Option<&GithubAuth>) -> std::process::Command {
    git_command_with(path, github_https, auth, false)
}

/// [`git_command`] that, with `locked`, also ignores every credential helper the repository's or the user's config
/// names (a saved copy's `.agents/setup` could have planted one), so only `auth` or the GitHub CLI's sign-in is asked.
fn git_command_with(path: &str, github_https: bool, auth: Option<&GithubAuth>, locked: bool) -> std::process::Command {
    let mut command = process::command("git");
    if locked {
        command.args([
            "-c",
            "credential.helper=",
            "-c",
            "credential.https://github.com.helper=",
        ]);
    }
    // A repository's own config can name programs git runs (`core.fsmonitor`, hooks); none of them run here.
    command.args(["-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null"]);
    if let Some(auth) = auth {
        // The token goes in the header below: no helper (a planted one would receive it) is asked.
        command.args(["-c", "credential.helper="]);
        command
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraheader")
            .env("GIT_CONFIG_VALUE_0", auth.header());
    } else if github_https {
        // The GitHub CLI's sign-in, as `gh auth setup-git` would set it, after any helper the person has.
        command.args(["-c", "credential.https://github.com.helper=!gh auth git-credential"]);
    }
    command
        .env("PATH", path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never");
    command
}

/// After a shallow clone at `dest`: `origin`'s other branches are tracked now, so a fetch of a base branch or a switch
/// to another branch finds them, and the history is fetched in the background. The session never waits for it, and a
/// failure is only logged: the clone works without it.
fn deepen_in_background(dest: PathBuf, path: String, github_https: bool, auth: Option<GithubAuth>) {
    if let Err(error) = track_all_branches(&dest, &path) {
        eprintln!("nexus-workbench: clone {}: {error}", dest.display());
    }
    spawn_deepen(dest, path, github_https, auth);
}

/// Fetches `dest`'s history on a thread of its own, unless a deepen of it already runs.
fn spawn_deepen(dest: PathBuf, path: String, github_https: bool, auth: Option<GithubAuth>) {
    let Some(guard) = Deepening::start(&dest) else {
        return;
    };
    std::thread::spawn(move || {
        let _guard = guard;
        if let Err(error) = deepen(&dest, &path, github_https, auth.as_ref(), DEEPEN_LIMIT) {
            eprintln!("nexus-workbench: clone {}: {error}", dest.display());
        }
    });
}

/// Whether `dest`'s `origin` is a GitHub https URL and the GitHub CLI is there to sign it in: the test the clone itself
/// used (`plan.gh && url.starts_with("https://github.com/")`).
fn github_https_of(dest: &Path, path: &str) -> bool {
    on_path("gh", path)
        && git_command(path, false, None)
            .arg("-C")
            .arg(dest)
            .args(["config", "--get", "remote.origin.url"])
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .starts_with("https://github.com/")
            })
}

/// Completes `repo`'s history before a review worktree is made from it (a pull request's head may fork below a shallow
/// tip, and then it has no merge-base). A deepen already running is waited for, not duplicated; none running, one
/// starts with the clone's own PATH and sign-in. A deepen that outlasts `wait` goes on in the background, and this is
/// an error while the repository is still shallow. A complete repository costs one cheap check.
pub fn complete_history(repo: &Path, path: &str, wait: Duration) -> Result<(), String> {
    let shallow = || git::git(repo, &["rev-parse", "--is-shallow-repository"]).as_deref() == Ok("true");
    if !shallow() {
        return Ok(());
    }
    if !deepening(repo) {
        remove_stale_lock(repo);
        spawn_deepen(repo.to_path_buf(), path.to_string(), github_https_of(repo, path), None);
    }
    let until = Instant::now() + wait;
    while deepening(repo) {
        if Instant::now() >= until {
            return Err(
                "The project's history is still arriving, so the review worktree wasn't made. Try again in a few \
                 minutes."
                    .into(),
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if shallow() {
        return Err(
            "The project's history could not be fetched, so the review worktree wasn't made. Check the \
                    connection and the GitHub sign-in, then try again."
                .into(),
        );
    }
    Ok(())
}

/// A `shallow.lock` older than [`DEEPEN_LIMIT`] was left by a fetch that died: removed.
fn remove_stale_lock(dest: &Path) {
    let lock = dest.join(".git").join("shallow.lock");
    let stale = std::fs::metadata(&lock)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age > DEEPEN_LIMIT);
    if stale {
        let _ = std::fs::remove_file(&lock);
    }
}

/// [`resume_deepen`], on a cloud machine only.
pub fn resume_deepen_if_blobless(dest: PathBuf, path: String) {
    if blobless() {
        resume_deepen(dest, path);
    }
}

/// Picks up a deepen a closed host left unfinished: when the project at `dest` is still shallow, its history is fetched
/// in the background again, signed in as the clone was. A stale `shallow.lock` (older than [`DEEPEN_LIMIT`]) is removed
/// first.
pub fn resume_deepen(dest: PathBuf, path: String) {
    let shallow = git_command(&path, false, None)
        .arg("-C")
        .arg(&dest)
        .args(["rev-parse", "--is-shallow-repository"])
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "true");
    if !shallow {
        return;
    }
    remove_stale_lock(&dest);
    let github_https = github_https_of(&dest, &path);
    spawn_deepen(dest, path, github_https, None);
}

/// `origin`'s fetch refspec back to every branch: `--single-branch` narrowed it to the one cloned.
fn track_all_branches(dest: &Path, path: &str) -> Result<(), String> {
    let out = git_command(path, false, None)
        .arg("-C")
        .arg(dest)
        .args(["remote", "set-branches", "origin", "*"])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "could not track origin's branches: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Fetches the rest of a shallow clone's history from `origin`, within `limit`: still without tags, and still
/// blobless (the clone keeps its filter in its config).
fn deepen(
    dest: &Path,
    path: &str,
    github_https: bool,
    auth: Option<&GithubAuth>,
    limit: Duration,
) -> Result<(), String> {
    // With the GitHub CLI's sign-in, a helper the copy's config names (a saved copy's setup could have planted one)
    // must not be asked for the token first: the config is locked, as in the saved copy's own fetch.
    let mut command = git_command_with(path, github_https, auth, github_https);
    command
        .arg("-C")
        .arg(dest)
        .args(["fetch", "--quiet", "--unshallow", "--no-tags", "origin"]);
    let out = git::output_within(command, limit)
        .map_err(|error| format!("could not run git: {error}"))?
        .ok_or_else(|| format!("its history did not arrive within {}", git::seconds(limit)))?;
    if out.success {
        Ok(())
    } else {
        Err(format!("could not fetch its history: {}", out.stderr.trim()))
    }
}

const SOURCE_HELP: &str = "Paste a repository URL (https://… or git@…:…), or a GitHub repository as owner/name.";

/// Where a clone stands, as git last said it: `Receiving objects` at 45%.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub stage: String,
    pub percent: Option<u8>,
}

impl Progress {
    pub fn stage(stage: &str) -> Progress {
        Progress {
            stage: stage.into(),
            percent: None,
        }
    }
}

/// What the person typed, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `owner/repo` on GitHub.
    GitHub { owner: String, repo: String },
    /// A URL git clones as it is.
    Url(String),
}

/// Reads a repository URL (`https://`, `http://`, `ssh://`, `git://`, `file://`, `user@host:path`,
/// `github.com/owner/repo`) or a GitHub `owner/repo`.
pub fn parse(text: &str) -> Result<Source, String> {
    let text = text.trim();
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return Err(SOURCE_HELP.into());
    }
    if let Some(rest) = text.strip_prefix("github.com/") {
        return Ok(Source::Url(format!("https://github.com/{rest}")));
    }
    if let Some((scheme, rest)) = text.split_once("://") {
        let known = ["https", "http", "ssh", "git", "file"].contains(&scheme.to_ascii_lowercase().as_str());
        return if known && !rest.is_empty() {
            Ok(Source::Url(text.into()))
        } else {
            Err(SOURCE_HELP.into())
        };
    }
    if scp_like(text) {
        return Ok(Source::Url(text.into()));
    }
    let mut parts = text.split('/');
    if let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) {
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        if github_name(owner) && github_name(repo) && !owner.starts_with('.') {
            return Ok(Source::GitHub {
                owner: owner.into(),
                repo: repo.into(),
            });
        }
    }
    Err(SOURCE_HELP.into())
}

/// A GitHub owner's or repository's name: letters, digits, `-`, `_` and `.`, not starting with `-` (an option to `gh`).
pub(crate) fn github_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `user@host:path`, git's short form of an SSH URL.
fn scp_like(text: &str) -> bool {
    text.split_once(':').is_some_and(|(head, path)| {
        !path.is_empty()
            && !head.contains('/')
            && head
                .split_once('@')
                .is_some_and(|(user, host)| !user.is_empty() && !host.is_empty())
    })
}

fn is_ssh(url: &str) -> bool {
    url.get(..6).is_some_and(|scheme| scheme.eq_ignore_ascii_case("ssh://")) || scp_like(url)
}

/// The same repository over HTTPS: `git@github.com:o/r.git` and `ssh://git@github.com:22/o/r` are
/// `https://github.com/o/r.git` and `https://github.com/o/r`. None for a URL that is not SSH.
pub fn https_of(url: &str) -> Option<String> {
    let (host, path) = if is_ssh(url) && url.contains("://") {
        let (authority, path) = url.get(6..)?.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        (host.split(':').next()?, path)
    } else if scp_like(url) {
        let (head, path) = url.split_once(':')?;
        (head.rsplit('@').next()?, path.trim_start_matches('/'))
    } else {
        return None;
    };
    (!host.is_empty() && !path.is_empty()).then(|| format!("https://{host}/{path}"))
}

/// The folder a clone of `url` goes in: its last part, without `.git`.
pub fn name_of(url: &str) -> Option<String> {
    let last = url.trim_end_matches('/').rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty() && name != "." && name != ".." && !name.contains(['\\', '\0'])).then(|| name.to_string())
}

/// What a clone does: the URLs it tries in order (the next only when the one before was refused for its SSH key), the
/// folder's name, and whether the GitHub CLI signs git in to github.com over HTTPS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub urls: Vec<String>,
    pub name: String,
    pub gh: bool,
}

/// The plan for `source`, with `path` (the agents' `PATH`) to find `gh`. A GitHub `owner/repo` is looked up with the
/// GitHub CLI, which says whether the person can reach it and whether they clone over SSH (`git_protocol`). Without the
/// CLI, it is cloned over HTTPS, which reaches a public repository.
pub fn plan(source: &Source, path: &str) -> Result<Plan, String> {
    let gh = on_path("gh", path);
    let (urls, name) = match source {
        Source::Url(url) => {
            let name = name_of(url).ok_or_else(|| format!("{url} doesn't name a repository."))?;
            let mut urls = vec![url.clone()];
            urls.extend(is_ssh(url).then(|| https_of(url)).flatten());
            (urls, name)
        }
        Source::GitHub { owner, repo } if gh => github(owner, repo, path)?,
        Source::GitHub { owner, repo } => (vec![format!("https://github.com/{owner}/{repo}.git")], repo.clone()),
    };
    Ok(Plan { urls, name, gh })
}

/// `owner/repo` as the GitHub CLI knows it: its URLs (SSH first when that is the person's protocol) and its name.
fn github(owner: &str, repo: &str, path: &str) -> Result<(Vec<String>, String), String> {
    let full = format!("{owner}/{repo}");
    let view = gh(path, &["repo", "view", &full, "--json", "name,url,sshUrl"])?;
    let view: serde_json::Value =
        serde_json::from_str(&view).map_err(|_| format!("The GitHub CLI didn't describe {full}."))?;
    let field = |key: &str| view[key].as_str().filter(|text| !text.is_empty()).map(String::from);
    let https = field("url")
        .map(|url| format!("{url}.git"))
        .ok_or_else(|| format!("The GitHub CLI didn't describe {full}."))?;
    let name = field("name").unwrap_or_else(|| repo.to_string());
    let ssh = gh(path, &["config", "get", "git_protocol", "-h", "github.com"]).is_ok_and(|protocol| protocol == "ssh");
    let urls = match field("sshUrl") {
        Some(url) if ssh => vec![url, https],
        _ => vec![https],
    };
    Ok((urls, name))
}

fn gh(path: &str, args: &[&str]) -> Result<String, String> {
    let mut command = process::command("gh");
    command
        .args(args)
        .env("PATH", path)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    let out = git::output_within(command, git::GH_LIMIT)
        .map_err(|error| format!("Could not run the GitHub CLI: {error}"))?
        .ok_or_else(|| {
            format!(
                "GitHub didn't answer within {}. Try again.",
                git::seconds(git::GH_LIMIT)
            )
        })?;
    if out.success {
        Ok(out.stdout.trim().to_string())
    } else {
        Err(git::gh_error(&out.stderr))
    }
}

/// Whether `program` is in one of `path`'s folders.
pub fn on_path(program: &str, path: &str) -> bool {
    let names: &[String] = &if cfg!(windows) {
        vec![format!("{program}.exe"), format!("{program}.cmd")]
    } else {
        vec![program.to_string()]
    };
    std::env::split_paths(path).any(|dir| names.iter().any(|name| dir.join(name).is_file()))
}

/// `<parent>/<name>`, with a leading `~` read as `home` (an empty parent is `home`), and `parent` made when missing.
/// Refuses a relative parent, and a destination that already holds anything; an empty one is taken away, so a failed
/// clone can remove all it made.
pub fn destination(parent: &str, name: &str, home: &Path) -> Result<PathBuf, String> {
    let parent = parent_folder(parent, home);
    if !parent.is_absolute() {
        return Err("Choose the folder by its full path, or start it with ~/.".into());
    }
    let dest = parent.join(name);
    let taken = || {
        format!(
            "{} already exists. Open it with Add project, or choose another folder.",
            dest.display()
        )
    };
    match std::fs::read_dir(&dest).map(|mut entries| entries.next().is_some()) {
        Ok(true) => return Err(taken()),
        Ok(false) => std::fs::remove_dir(&dest).map_err(|_| taken())?,
        Err(_) if dest.exists() => return Err(taken()),
        Err(_) => {}
    }
    std::fs::create_dir_all(&parent).map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    Ok(dest)
}

/// The folder `parent` names: a leading `~` is `home`, and an empty one is `home`.
fn parent_folder(parent: &str, home: &Path) -> PathBuf {
    let parent = parent.trim();
    match parent.strip_prefix('~') {
        Some("") => home.to_path_buf(),
        Some(rest) if rest.starts_with(['/', '\\']) => home.join(&rest[1..]),
        _ if parent.is_empty() => home.to_path_buf(),
        _ => PathBuf::from(parent),
    }
}

/// The GitHub `owner/name` that `source` names, lowercased, as Gateway keys its saved copies; None for a repository
/// elsewhere.
pub fn github_key(source: &Source) -> Option<String> {
    let path = match source {
        Source::GitHub { owner, repo } => format!("{owner}/{repo}"),
        Source::Url(url) => {
            let https = https_of(url).unwrap_or_else(|| url.clone());
            let rest = https
                .strip_prefix("https://")
                .or_else(|| https.strip_prefix("http://"))?;
            let (host, path) = rest.split_once('/')?;
            if !host.eq_ignore_ascii_case("github.com") {
                return None;
            }
            path.trim_end_matches('/').to_string()
        }
    };
    let path = path.strip_suffix(".git").unwrap_or(&path);
    let (owner, repo) = path.split_once('/')?;
    (github_name(owner) && github_name(repo) && !owner.starts_with('.'))
        .then(|| format!("{owner}/{repo}").to_ascii_lowercase())
}

/// On a cloud machine whose saved copy is of `source` ([`WARM_MARKER`]): the copy, moved to `<parent>/<name>` and
/// brought up to date with the default branch's newest commit, as the clone's project. None when there is no such
/// copy, or it failed to update: it is removed then, and the caller clones as usual. The marker goes either way, so
/// only the first clone takes the copy. `auth` signs git in instead of the person's GitHub CLI (a machine's own setup).
/// `chosen` hears the clone's folder before the copy moves there or git touches it, so a caller that saves its state can
/// remove a copy cut off midway, as it does a clone.
pub fn warm(
    source: &Source,
    parent: &str,
    home: &Path,
    path: &str,
    auth: Option<&GithubAuth>,
    chosen: &dyn Fn(&Path),
    progress: &dyn Fn(Progress),
) -> Option<PathBuf> {
    if !blobless() {
        return None;
    }
    take_copy(source, parent, home, path, auth, chosen, progress)
}

#[allow(clippy::too_many_arguments)]
fn take_copy(
    source: &Source,
    parent: &str,
    home: &Path,
    path: &str,
    auth: Option<&GithubAuth>,
    chosen: &dyn Fn(&Path),
    progress: &dyn Fn(Progress),
) -> Option<PathBuf> {
    let marker = home.join(WARM_MARKER);
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&marker).ok()?).ok()?;
    let repo = saved["repo"].as_str()?.to_string();
    let copy = PathBuf::from(saved["path"].as_str()?);
    if github_key(source).as_deref() != Some(repo.as_str()) || !copy.join(".git").is_dir() {
        return None;
    }
    let _ = std::fs::remove_file(&marker);
    progress(Progress::stage(&format!("Starting from a saved copy of {repo}")));
    match refresh(source, &copy, parent, home, path, auth, chosen) {
        Ok(dest) => Some(dest),
        Err((dest, error)) => {
            eprintln!("nexus-workbench: the saved copy of {repo} did not update, so it is cloned: {error}");
            let _ = std::fs::remove_dir_all(&dest);
            progress(Progress::stage("Cloning the repository"));
            None
        }
    }
}

/// Moves the saved copy at `copy` to the clone's folder, and fetches and checks out `origin`'s default branch at its
/// newest commit, shallow and blobless as a clone. Its history follows in the background ([`deepen`]). `chosen` hears
/// the clone's folder first. On an error, where the copy is now.
fn refresh(
    source: &Source,
    copy: &Path,
    parent: &str,
    home: &Path,
    path: &str,
    auth: Option<&GithubAuth>,
    chosen: &dyn Fn(&Path),
) -> Result<PathBuf, (PathBuf, String)> {
    let failed = |at: &Path, error: String| (at.to_path_buf(), error);
    let name = match source {
        Source::GitHub { repo, .. } => Some(repo.clone()),
        Source::Url(url) => name_of(url),
    }
    .ok_or_else(|| failed(copy, "no folder name".into()))?;
    let dest = if parent_folder(parent, home).join(&name) == copy {
        chosen(copy);
        copy.to_path_buf()
    } else {
        let dest = destination(parent, &name, home).map_err(|error| failed(copy, error))?;
        chosen(&dest);
        std::fs::rename(copy, &dest).map_err(|error| failed(copy, format!("could not move it: {error}")))?;
        dest
    };
    // The copy's config came from Gateway's builder: it must hold nothing a fresh clone lacks.
    let planted = take_out_planted_config(&dest, path);
    if !planted.is_empty() {
        return Err(failed(
            &dest,
            format!("the copy's git config had {}", planted.join(", ")),
        ));
    }
    let github_https = on_path("gh", path);
    let git = |args: &[&str]| -> Result<(), String> {
        let mut command = git_command_with(path, github_https, auth, true);
        command.arg("-C").arg(&dest).args(args);
        let out = git::output_within(command, STALL_LIMIT)
            .map_err(|error| format!("could not run git: {error}"))?
            .ok_or_else(|| format!("git {} did not finish within {}", args[0], git::seconds(STALL_LIMIT)))?;
        if out.success {
            Ok(())
        } else {
            Err(format!("git {}: {}", args[0], out.stderr.trim()))
        }
    };
    git(&["fetch", "--quiet", "--depth=1", "--no-tags", "origin", "HEAD"]).map_err(|error| failed(&dest, error))?;
    git(&["reset", "--quiet", "--hard", "FETCH_HEAD"]).map_err(|error| failed(&dest, error))?;
    deepen_in_background(dest.clone(), path.to_string(), github_https, auth.cloned());
    Ok(dest)
}

/// On a cloud machine, for a setup a person or organization opted into: runs the repository's `.agents/setup` once when it is an
/// executable file, from `dest`, for at most [`AGENTS_SETUP_LIMIT`], with what it printed in [`AGENTS_SETUP_LOG`].
/// Ok when it finished well or there is none to run. A failure (a non-zero exit, a run stopped at the limit or that
/// could not start, git config it planted) answers why, for the person: the clone stands either way.
pub fn agents_setup(dest: &Path, home: &Path, path: &str, progress: &dyn Fn(Progress)) -> Result<(), String> {
    if !blobless() {
        return Ok(());
    }
    run_setup(dest, home, path, AGENTS_SETUP_LIMIT, true, progress)
}

/// `.agents/setup` run as the host's own user: tests of what does not depend on the user.
#[cfg(test)]
pub(crate) fn run_agents_setup(
    dest: &Path,
    home: &Path,
    path: &str,
    limit: Duration,
    progress: &dyn Fn(Progress),
) -> Result<(), String> {
    run_setup(dest, home, path, limit, false, progress)
}

/// `.agents/setup` run as its own OS user, as on a cloud machine ([`crate::setup_user`]).
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn run_agents_setup_as_user(dest: &Path, home: &Path, path: &str, limit: Duration) -> Result<(), String> {
    run_setup(dest, home, path, limit, true, &|_| {})
}

/// Runs `.agents/setup` from `dest`; as its own OS user when `separate` ([`crate::setup_user`]), which every cloud
/// machine's setup is. Where that user cannot be had (not Linux, no passwordless sudo), the setup fails: it never runs
/// as the host's user instead.
fn run_setup(
    dest: &Path,
    home: &Path,
    path: &str,
    limit: Duration,
    separate: bool,
    progress: &dyn Fn(Progress),
) -> Result<(), String> {
    let script = dest.join(AGENTS_SETUP);
    if !executable(&script) {
        return Ok(());
    }
    progress(Progress::stage("Running .agents/setup"));
    // The clone's hooks before the setup user gets it: one it adds or changes would run later as the host's user.
    let hooks_before = separate.then(|| hooks_of(dest));
    if separate {
        #[cfg(target_os = "linux")]
        // A clone handed over in part goes back whole.
        let handed = crate::setup_user::hand_over(home, dest).inspect_err(|_| {
            let _ = crate::setup_user::take_back(dest);
        });
        #[cfg(not(target_os = "linux"))]
        let handed: Result<(), String> = Err("a separate setup user needs Linux".into());
        if let Err(why) = handed {
            let why = format!("{AGENTS_SETUP} could not run as its own user: {why}");
            eprintln!("nexus-workbench: {why} in {}", dest.display());
            if let Err(error) = write_log(&home.join(AGENTS_SETUP_LOG), &format!("failed\n{why}\n")) {
                eprintln!("nexus-workbench: could not write the setup log: {error}");
            }
            return Err(format!("{why}. Its output is in ~/{AGENTS_SETUP_LOG}."));
        }
    }
    let run_id = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let ran = if separate {
        #[cfg(target_os = "linux")]
        let ran = crate::setup_user::run(&script, dest, path, SETUP_RUN_VAR, &run_id, limit);
        #[cfg(not(target_os = "linux"))]
        let ran = Err(std::io::Error::other("a separate setup user needs Linux"));
        ran
    } else {
        let mut command = process::command(&script);
        // Repository code gets a short list of variables: nothing of the host's environment (its tokens, its grant).
        command
            .current_dir(dest)
            .env_clear()
            .env("PATH", path)
            .env("HOME", home)
            .env("LANG", "C.UTF-8")
            .env(SETUP_RUN_VAR, &run_id);
        git::output_ending_all(command, limit)
    };
    let (said, mut why) = match ran {
        Ok(Some(out)) => {
            let why = (!out.success).then(|| {
                let last = format!("{}\n{}", out.stdout, out.stderr)
                    .lines()
                    .map(str::trim)
                    .rfind(|line| !line.is_empty())
                    .map(|line| format!(": {}", line.chars().take(200).collect::<String>()))
                    .unwrap_or_default();
                format!("{AGENTS_SETUP} failed{last}")
            });
            let said = format!(
                "{}\n{}{}",
                if out.success { "done" } else { "failed" },
                out.stdout,
                out.stderr
            );
            (said, why)
        }
        Ok(None) => (
            format!("stopped after {}", git::seconds(limit)),
            Some(format!(
                "{AGENTS_SETUP} did not finish within {} and was stopped",
                git::seconds(limit)
            )),
        ),
        Err(error) => (
            format!("could not run it: {error}"),
            Some(format!("{AGENTS_SETUP} could not run: {error}")),
        ),
    };
    // What left its process group (`setsid`, a double fork) still carries the run's mark: it ends too.
    end_marked(&run_id);
    // The clone goes back to the host's user before git reads its config as that user. Until it does, the setup has
    // not ended: a clone left the setup user's is never the agent's.
    #[cfg(target_os = "linux")]
    if separate && let Err(error) = crate::setup_user::take_back(dest) {
        let why = format!("{AGENTS_SETUP} ended, but its clone could not be taken back from its user: {error}");
        eprintln!("nexus-workbench: {why} in {}", dest.display());
        if let Err(error) = write_log(&home.join(AGENTS_SETUP_LOG), &format!("failed\n{why}\n{said}")) {
            eprintln!("nexus-workbench: could not write the setup log: {error}");
        }
        return Err(format!("{why}. Its output is in ~/{AGENTS_SETUP_LOG}."));
    }
    let mut planted = take_out_planted_config(dest, path);
    if let Some(before) = &hooks_before {
        planted.extend(take_out_planted_hooks(dest, before));
    }
    let said = if planted.is_empty() {
        said
    } else {
        why = Some(format!(
            "{AGENTS_SETUP} changed the repository's git config or hooks ({}); those entries were removed",
            planted.join(", ")
        ));
        format!(
            "failed\nthe setup changed the repository's git config or hooks ({}); those entries were removed\n{said}",
            planted.join(", ")
        )
    };
    let log = home.join(AGENTS_SETUP_LOG);
    if let Err(error) = write_log(&log, &said) {
        eprintln!("nexus-workbench: could not write {}: {error}", log.display());
    }
    match why {
        None => Ok(()),
        Some(why) => {
            eprintln!("nexus-workbench: {} in {}: {why}", AGENTS_SETUP, dest.display());
            Err(format!("{why}. Its output is in ~/{AGENTS_SETUP_LOG}."))
        }
    }
}

/// Runs the repository's `.agents/resume` from `dest` for at most `limit`, with exactly `env` as its environment: the
/// agent's own (the host's, with the person's credentials from `agent.env`), so it can do what a session could. Ok when
/// it finished well or there is none to run. A failure (a non-zero exit, a stop at the limit, a run that could not
/// start) answers a short why for the status line; it never blocks anything. What it printed is in
/// [`AGENTS_RESUME_LOG`], and what it left running ends with it.
pub fn run_agents_resume(dest: &Path, home: &Path, env: &[(String, String)], limit: Duration) -> Result<(), String> {
    let script = dest.join(AGENTS_RESUME);
    if !executable(&script) {
        return Ok(());
    }
    let run_id = format!(
        "resume-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let mut command = process::command(&script);
    command
        .current_dir(dest)
        .env_clear()
        .envs(env.iter().cloned())
        .env(SETUP_RUN_VAR, &run_id);
    let (said, why) = match git::output_ending_all(command, limit) {
        Ok(Some(out)) if out.success => (format!("done\n{}{}", out.stdout, out.stderr), None),
        Ok(Some(out)) => {
            let last = format!("{}\n{}", out.stdout, out.stderr)
                .lines()
                .map(str::trim)
                .rfind(|line| !line.is_empty())
                .map(|line| format!(": {}", line.chars().take(200).collect::<String>()))
                .unwrap_or_default();
            (
                format!("failed\n{}{}", out.stdout, out.stderr),
                Some(format!("{AGENTS_RESUME} failed{last}")),
            )
        }
        Ok(None) => (
            format!("stopped after {}", git::seconds(limit)),
            Some(format!(
                "{AGENTS_RESUME} did not finish within {} and was stopped",
                git::seconds(limit)
            )),
        ),
        Err(error) => (
            format!("could not run it: {error}"),
            Some(format!("{AGENTS_RESUME} could not run: {error}")),
        ),
    };
    end_marked(&run_id);
    let log = home.join(AGENTS_RESUME_LOG);
    if let Err(error) = write_log(&log, &said) {
        eprintln!("nexus-workbench: could not write {}: {error}", log.display());
    }
    match why {
        None => Ok(()),
        Some(why) => {
            eprintln!("nexus-workbench: {why} in {}", dest.display());
            Err(format!("{why}. Its output is in ~/{AGENTS_RESUME_LOG}."))
        }
    }
}

/// Writes what a repository script printed to `log`, which only the person may read (its folder made 0700, the file
/// 0600, written through a temporary file so a link planted at `log` is replaced, not followed): the output can hold
/// secrets from the script's environment.
fn write_log(log: &Path, said: &str) -> std::io::Result<()> {
    if let Some(folder) = log.parent() {
        crate::private_fs::create_dir_all(folder)?;
    }
    crate::private_fs::write(log, said.as_bytes())
}

/// The variable that marks every process of one `.agents/setup` run.
const SETUP_RUN_VAR: &str = "NEXUS_SETUP_RUN";

/// Ends every process still carrying the mark of setup run `id` in its environment, wherever its session or group is.
/// Limit: a child that drops the mark (`env -u`, `env -i`) escapes this. A cloud machine's setup runs as its own user
/// ([`crate::setup_user`]), whose processes all end however they left.
#[cfg(target_os = "linux")]
fn end_marked(id: &str) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    let needle = format!("{SETUP_RUN_VAR}={id}");
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<i32>().ok()) else {
            continue;
        };
        let Ok(environ) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        if environ.split(|b| *b == 0).any(|var| var == needle.as_bytes()) {
            let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn end_marked(_id: &str) {}

/// The repository config keys a clone, git itself or a usual setup writes, and that name no program for git to run and
/// no place for it to sign in to. Any other key is [`planted_key`]. `git sparse-checkout` writes
/// `extensions.worktreeconfig` to the repository's config and keeps its own state (`core.sparsecheckout`,
/// `core.sparsecheckoutcone`) in `config.worktree`, which [`take_out_planted_config`] scans with the same rules.
const EXPECTED_KEYS: &[&str] = &[
    "core.repositoryformatversion",
    "core.filemode",
    "core.bare",
    "core.logallrefupdates",
    "core.ignorecase",
    "core.precomposeunicode",
    "core.symlinks",
    "core.autocrlf",
    "core.eol",
    "core.safecrlf",
    "core.longpaths",
    "core.quotepath",
    "core.sparsecheckout",
    "core.sparsecheckoutcone",
    "core.untrackedcache",
    "core.splitindex",
    "core.preloadindex",
    "core.checkstat",
    "core.trustctime",
    "core.commitgraph",
    "core.multipackindex",
    "extensions.objectformat",
    "extensions.partialclone",
    "extensions.refstorage",
    "extensions.preciousobjects",
    "extensions.worktreeconfig",
    "user.name",
    "user.email",
    "init.defaultbranch",
    "pull.rebase",
    "pull.ff",
    "push.default",
    "push.autosetupremote",
    "fetch.prune",
    "fetch.prunetags",
    "fetch.writecommitgraph",
    "rebase.autostash",
    "rebase.autosquash",
    "rebase.updaterefs",
    "merge.ff",
    "merge.conflictstyle",
    "rerere.enabled",
    "status.showuntrackedfiles",
    "diff.renames",
    "diff.algorithm",
    "diff.colormoved",
    "blame.ignorerevsfile",
    "color.ui",
    "gc.auto",
    "gc.autodetach",
    "index.version",
    "feature.manyfiles",
    "lfs.repositoryformatversion",
];

/// Whether a repository config key is not one a clone or a usual setup writes ([`EXPECTED_KEYS`], and the safe keys of a
/// remote, a branch or a submodule): anything else may name something git would run (`alias.*`, `core.fsmonitor`,
/// `diff.<driver>.textconv`, `remote.<name>.uploadpack`, ...) or a place it would sign in to, so it counts as planted.
fn planted_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    if EXPECTED_KEYS.contains(&key.as_str()) {
        return false;
    }
    // `<section>.<subsection>.<name>`: the subsection (a remote's or branch's name) may itself hold dots.
    let safe = |section: &str, names: &[&str]| {
        key.strip_prefix(section)
            .and_then(|rest| rest.rsplit_once('.'))
            .is_some_and(|(subsection, name)| !subsection.is_empty() && names.contains(&name))
    };
    !(safe(
        "remote.",
        &[
            "url",
            "pushurl",
            "fetch",
            "push",
            "tagopt",
            "prune",
            "prunetags",
            "promisor",
            "partialclonefilter",
            "mirror",
        ],
    ) || safe(
        "branch.",
        &[
            "remote",
            "merge",
            "rebase",
            "pushremote",
            "description",
            "gh-merge-base",
            "vscode-merge-base",
        ],
    ) || safe("submodule.", &["url", "active", "branch", "shallow"]))
}

/// The files in `dest`'s `.git/hooks`, by name, with their bytes (a link's target, as bytes). Git's own `*.sample`
/// files never run and are left out.
fn hooks_of(dest: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let Ok(entries) = std::fs::read_dir(dest.join(".git").join("hooks")) else {
        return Default::default();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".sample") {
                return None;
            }
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).ok()?;
            let bytes = if meta.file_type().is_symlink() {
                std::fs::read_link(&path).ok()?.into_os_string().into_encoded_bytes()
            } else if meta.is_file() {
                std::fs::read(&path).ok()?
            } else {
                b"<folder>".to_vec()
            };
            Some((name, bytes))
        })
        .collect()
}

/// Takes out each hook a setup that ran as its own user added to `dest`'s `.git/hooks`, or changed, against `before`
/// ([`hooks_of`] before the run), and says which (`hook <name>`). Git would run them later as the host's user, with
/// the person's credentials, on a commit or a checkout. An added one is removed; a changed one gets its old bytes back.
fn take_out_planted_hooks(dest: &Path, before: &std::collections::BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let git_dir = dest.join(".git");
    let hooks = git_dir.join("hooks");
    // A setup that swaps `.git` or `.git/hooks` for a link would point the walk below at a folder the host's user
    // owns, where every file would count as planted and be removed. Neither link is followed. A linked `.git` is
    // only reported; a linked hooks folder is replaced by a real one holding the hooks from before.
    let is_link = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
    if is_link(&git_dir) {
        return vec!["link .git".to_string()];
    }
    if is_link(&hooks) {
        let replaced = std::fs::remove_file(&hooks)
            .and_then(|()| std::fs::create_dir(&hooks))
            .and_then(|()| {
                before
                    .iter()
                    .filter(|(_, bytes)| bytes.as_slice() != b"<folder>")
                    .try_for_each(|(name, bytes)| {
                        let path = hooks.join(name);
                        std::fs::write(&path, bytes).and_then(|()| executable_hook(&path))
                    })
            });
        if let Err(error) = replaced {
            eprintln!(
                "nexus-workbench: could not replace the linked hooks folder in {}: {error}",
                dest.display()
            );
        }
        return vec!["link .git/hooks".to_string()];
    }
    let mut planted = Vec::new();
    for (name, bytes) in hooks_of(dest) {
        if before.get(&name) == Some(&bytes) {
            continue;
        }
        let path = hooks.join(&name);
        let removed = match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&path),
            _ => std::fs::remove_file(&path),
        };
        let restored = removed.and_then(|()| match before.get(&name) {
            Some(old) if old.as_slice() != b"<folder>" => {
                std::fs::write(&path, old).and_then(|()| executable_hook(&path))
            }
            _ => Ok(()),
        });
        if let Err(error) = restored {
            eprintln!(
                "nexus-workbench: could not take out the hook {name} in {}: {error}",
                dest.display()
            );
        }
        planted.push(format!("hook {name}"));
    }
    planted
}

#[cfg(unix)]
fn executable_hook(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn executable_hook(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Removes from `dest`'s own git config every key a setup planted ([`planted_key`]: anything but what a clone or a
/// usual setup writes, so `credential.*`, `core.fsmonitor`, `alias.*`, `url.*`, `diff.<driver>.textconv` and the like),
/// and says which keys. A fresh clone has none of them. The repository's config and each worktree's `config.worktree`
/// (git reads it once `extensions.worktreeconfig` is set) are scanned alike.
fn take_out_planted_config(dest: &Path, path: &str) -> Vec<String> {
    let git_dir = dest.join(".git");
    let mut worktree_configs = vec![git_dir.join("config.worktree")];
    if let Ok(entries) = std::fs::read_dir(git_dir.join("worktrees")) {
        worktree_configs.extend(entries.flatten().map(|entry| entry.path().join("config.worktree")));
    }
    let mut sources: Vec<Vec<std::ffi::OsString>> = vec![vec!["--local".into()]];
    sources.extend(
        worktree_configs
            .into_iter()
            .filter(|file| file.is_file())
            .map(|file| vec!["--file".into(), file.into_os_string()]),
    );
    let mut planted = Vec::new();
    for source in &sources {
        let mut command = git_command(path, false, None);
        command
            .arg("-C")
            .arg(dest)
            .arg("config")
            .args(source)
            .args(["--list", "--name-only"])
            .stdin(Stdio::null());
        let Ok(out) = command.output() else {
            continue;
        };
        let mut keys: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|key| planted_key(key))
            .map(str::to_string)
            .collect();
        keys.sort();
        keys.dedup();
        for key in &keys {
            let mut command = git_command(path, false, None);
            command
                .arg("-C")
                .arg(dest)
                .arg("config")
                .args(source)
                .args(["--unset-all", key])
                .stdin(Stdio::null());
            let _ = command.output();
        }
        planted.extend(keys);
    }
    planted.sort();
    planted.dedup();
    planted
}

/// Whether `file` is a file someone may run.
fn executable(file: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(file).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        false
    }
}

/// Clones `plan` into `dest` (a [`destination`]), with `path` as git's `PATH`, telling `progress` where it stands. A
/// failed attempt leaves nothing at `dest`.
pub fn clone(plan: &Plan, dest: &Path, path: &str, progress: &dyn Fn(Progress)) -> Result<(), String> {
    clone_within(plan, dest, path, STALL_LIMIT, None, progress)
}

/// [`clone`], signed in to GitHub with `auth` instead of the person's GitHub CLI: a cloud machine's own setup at boot.
pub fn clone_with(
    plan: &Plan,
    dest: &Path,
    path: &str,
    auth: &GithubAuth,
    progress: &dyn Fn(Progress),
) -> Result<(), String> {
    clone_within(plan, dest, path, STALL_LIMIT, Some(auth), progress)
}

fn clone_within(
    plan: &Plan,
    dest: &Path,
    path: &str,
    stall: Duration,
    auth: Option<&GithubAuth>,
    progress: &dyn Fn(Progress),
) -> Result<(), String> {
    let blobless = blobless();
    for (attempt, url) in plan.urls.iter().enumerate() {
        if attempt > 0 {
            progress(Progress::stage("The SSH key was refused: cloning over HTTPS"));
        }
        let github_https = plan.gh && url.starts_with("https://github.com/");
        let failed = match run(url, dest, path, github_https, auth, blobless, stall, progress) {
            Outcome::Done => {
                if blobless {
                    deepen_in_background(dest.to_path_buf(), path.to_string(), github_https, auth.cloned());
                }
                return Ok(());
            }
            // Another clone took the folder since it was checked: it is that clone's, so it stays.
            Outcome::Failed(said) if said.contains("already exists and is not an empty directory") => {
                return Err(failure(url, &said));
            }
            Outcome::Failed(said) if ssh_refused(&said) && attempt + 1 < plan.urls.len() => None,
            Outcome::Failed(said) => Some(failure(url, &said)),
            Outcome::Stalled => Some(format!(
                "git said nothing for {} while cloning {url}, so the clone was stopped. Check the connection, then \
                 try again.",
                git::seconds(stall)
            )),
            Outcome::NotRun(error) => Some(format!("Could not run git: {error}")),
        };
        let _ = std::fs::remove_dir_all(dest);
        if let Some(error) = failed {
            return Err(error);
        }
    }
    Err(SOURCE_HELP.into())
}

enum Outcome {
    Done,
    /// What git said (its lines that were not progress).
    Failed(String),
    Stalled,
    NotRun(std::io::Error),
}

/// One `git clone`. Its stderr is read as it comes: each line (git ends progress lines with `\r`) goes to `progress`
/// when it is progress, and is kept otherwise. Past `stall` without a line, git is killed with what it started.
#[allow(clippy::too_many_arguments)]
fn run(
    url: &str,
    dest: &Path,
    path: &str,
    github_https: bool,
    auth: Option<&GithubAuth>,
    blobless: bool,
    stall: Duration,
    progress: &dyn Fn(Progress),
) -> Outcome {
    let mut command = git_command(path, github_https, auth);
    command
        .args(clone_args(url, blobless))
        .arg(dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // Its own process group, so a kill reaches the transport it started (ssh, a credential helper).
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Outcome::NotRun(error),
    };
    let (tx, lines) = mpsc::channel::<String>();
    if let Some(mut pipe) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut bytes = [0u8; 4096];
            let mut line = Vec::new();
            while let Ok(n) = pipe.read(&mut bytes)
                && n > 0
            {
                for &byte in &bytes[..n] {
                    if byte == b'\r' || byte == b'\n' {
                        if !line.is_empty() && tx.send(String::from_utf8_lossy(&line).into_owned()).is_err() {
                            return;
                        }
                        line.clear();
                    } else {
                        line.push(byte);
                    }
                }
            }
            if !line.is_empty() {
                let _ = tx.send(String::from_utf8_lossy(&line).into_owned());
            }
        });
    }
    let mut kept: Vec<String> = Vec::new();
    let mut hear = |line: String| match parse_progress(&line) {
        Some(stage) => progress(stage),
        None => {
            if kept.len() == KEPT_LINES {
                kept.remove(0);
            }
            kept.push(line);
        }
    };
    let mut heard = Instant::now();
    let mut open = true;
    let status = loop {
        if open {
            match lines.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    heard = Instant::now();
                    hear(line);
                    continue;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => open = false,
            }
        } else {
            std::thread::sleep(Duration::from_millis(50));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if heard.elapsed() >= stall => {
                process::kill(Some(child.id()));
                let _ = child.kill();
                let _ = child.wait();
                return Outcome::Stalled;
            }
            Ok(None) => {}
            Err(error) => return Outcome::NotRun(error),
        }
    };
    // What git said last, unless a process it left behind holds the pipe open.
    let drained = Instant::now() + process::DRAIN;
    while let Ok(line) = lines.recv_timeout(drained.saturating_duration_since(Instant::now())) {
        hear(line);
    }
    if status.success() {
        Outcome::Done
    } else {
        Outcome::Failed(kept.join("\n"))
    }
}

/// A line of git's progress: `Receiving objects:  45% (450/1000), 1.20 MiB | 2.00 MiB/s` is `Receiving objects` at
/// 45, and `remote: Enumerating objects: 12, done.` is `Enumerating objects`. None for anything else (errors, hints).
pub fn parse_progress(line: &str) -> Option<Progress> {
    let line = line.trim();
    let line = line.strip_prefix("remote:").map(str::trim).unwrap_or(line);
    if line.starts_with("Cloning into") {
        return Some(Progress::stage("Connecting"));
    }
    let (stage, rest) = line.split_once(": ")?;
    if stage.is_empty() || !stage.chars().all(|c| c.is_ascii_alphabetic() || c == ' ') {
        return None;
    }
    if ["fatal", "error", "warning", "hint", "ssh"].contains(&stage.to_ascii_lowercase().as_str()) {
        return None;
    }
    let percent = rest
        .trim_start()
        .split_once('%')
        .and_then(|(number, _)| number.trim().parse::<u8>().ok())
        .map(|percent| percent.min(100));
    Some(Progress {
        stage: stage.to_string(),
        percent,
    })
}

/// Whether the transport, not the repository, failed over SSH: the key or host key refused, or the port closed.
fn ssh_refused(said: &str) -> bool {
    [
        "Permission denied (publickey",
        "Host key verification failed",
        "ssh: connect to host",
    ]
    .iter()
    .any(|sign| said.contains(sign))
}

/// What went wrong, for a person: git's `fatal:` and `error:` lines, with what to do when it could not sign in.
fn failure(url: &str, said: &str) -> String {
    if ssh_refused(said) {
        return format!(
            "The host refused this machine's SSH key, or its host key isn't known yet ({}). Add the key to the host, \
             or paste the repository's https:// URL.",
            said.lines().find(|line| ssh_refused(line)).unwrap_or_default().trim()
        );
    }
    let lines: Vec<&str> = said
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("fatal:").or_else(|| line.strip_prefix("error:")))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let text = if lines.is_empty() {
        said.lines().last().unwrap_or("git clone failed.").trim().to_string()
    } else {
        lines.join(" ")
    };
    let signed_out = [
        "could not read Username",
        "Authentication failed",
        "Repository not found",
    ]
    .iter()
    .any(|sign| said.contains(sign));
    if !signed_out {
        return text;
    }
    let hint = if url.contains("github.com") {
        "Check the name, and that `gh auth login` signed you in to an account that can read it."
    } else {
        "Check the URL, and that git can sign in to its host on this machine."
    };
    format!("{text} {hint}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_keys_a_clone_or_a_usual_setup_writes_are_not_planted() {
        for key in [
            "core.repositoryformatversion",
            "core.bare",
            "remote.origin.url",
            "remote.origin.fetch",
            "remote.origin.promisor",
            "branch.main.remote",
            "branch.main.merge",
            "branch.release/1.2.gh-merge-base",
            "branch.main.vscode-merge-base",
            "extensions.worktreeconfig",
            "core.sparsecheckout",
            "core.sparsecheckoutcone",
            "user.email",
            "submodule.lib.url",
        ] {
            assert!(!planted_key(key), "{key}");
        }
        for key in [
            "alias.st",
            "core.fsmonitor",
            "core.gitproxy",
            "core.askpass",
            "core.editor",
            "core.hooksPath",
            "sequence.editor",
            "gpg.program",
            "gpg.ssh.program",
            "diff.json.textconv",
            "diff.json.command",
            "merge.ours.driver",
            "remote.origin.uploadpack",
            "remote.origin.receivepack",
            "remote.origin.proxy",
            "remote.origin.vcs",
            "protocol.ext.allow",
            "credential.helper",
            "url.https://evil.example/.insteadof",
            "include.path",
            "filter.lfs.smudge",
            "submodule.lib.update",
            "branch.main.mergeoptions",
        ] {
            assert!(planted_key(key), "{key}");
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-clone-{name}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        dir
    }

    #[test]
    fn a_sparse_checkout_stays_and_a_key_planted_in_config_worktree_is_taken_out() {
        let root = temp("sparse");
        let repo = origin(&root);
        let path = std::env::var("PATH").unwrap_or_default();
        git_in(&repo, &["sparse-checkout", "set", "--cone", "docs"]);
        assert_eq!(take_out_planted_config(&repo, &path), Vec::<String>::new());
        git_in(&repo, &["sparse-checkout", "list"]);
        git_in(&repo, &["config", "--worktree", "core.fsmonitor", "/tmp/x"]);
        assert_eq!(
            take_out_planted_config(&repo, &path),
            vec!["core.fsmonitor".to_string()]
        );
        git_in(&repo, &["sparse-checkout", "list"]);
        let worktree = std::fs::read_to_string(repo.join(".git/config.worktree")).unwrap_or_default();
        assert!(!worktree.contains("fsmonitor"), "{worktree}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_hook_the_setup_added_or_changed_is_taken_out_and_one_from_before_stays() {
        let root = temp("hooks");
        let hooks = root.join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(hooks.join("pre-commit.sample"), "#!/bin/sh\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(hooks.join("pre-push"), "#!/bin/sh\nmine\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(hooks.join("commit-msg"), "#!/bin/sh\nmine too\n").unwrap_or_else(|e| panic!("{e}"));
        let before = hooks_of(&root);
        assert_eq!(take_out_planted_hooks(&root, &before), Vec::<String>::new());
        std::fs::write(hooks.join("post-checkout"), "#!/bin/sh\nplanted\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(hooks.join("pre-push"), "#!/bin/sh\nchanged\n").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(hooks.join("post-merge.sample"), "#!/bin/sh\n").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            take_out_planted_hooks(&root, &before),
            vec!["hook post-checkout".to_string(), "hook pre-push".to_string()]
        );
        assert!(!hooks.join("post-checkout").exists());
        let read = |name: &str| std::fs::read_to_string(hooks.join(name)).unwrap_or_default();
        assert_eq!(read("pre-push"), "#!/bin/sh\nmine\n");
        assert_eq!(read("commit-msg"), "#!/bin/sh\nmine too\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_hooks_folder_the_setup_swapped_for_a_link_is_not_followed() {
        let root = temp("hooks-link");
        let hooks = root.join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(hooks.join("pre-push"), "#!/bin/sh\nmine\n").unwrap_or_else(|e| panic!("{e}"));
        let before = hooks_of(&root);
        // The host user's folder the link points at: none of its files may go.
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(outside.join("id_ed25519"), "key").unwrap_or_else(|e| panic!("{e}"));
        std::fs::remove_dir_all(&hooks).unwrap_or_else(|e| panic!("{e}"));
        std::os::unix::fs::symlink(&outside, &hooks).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            take_out_planted_hooks(&root, &before),
            vec!["link .git/hooks".to_string()]
        );
        assert!(outside.join("id_ed25519").exists());
        let meta = std::fs::symlink_metadata(&hooks).unwrap_or_else(|e| panic!("{e}"));
        assert!(meta.is_dir() && !meta.file_type().is_symlink());
        assert_eq!(
            std::fs::read_to_string(hooks.join("pre-push")).unwrap_or_default(),
            "#!/bin/sh\nmine\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let ok = process::command("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    }

    /// A repository with one commit and one file, at `<root>/origin`.
    fn origin(root: &Path) -> PathBuf {
        let repo = root.join("origin");
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
        git_in(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        std::fs::write(repo.join("README.md"), "hi\n").unwrap_or_else(|e| panic!("{e}"));
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "--quiet", "-m", "one"]);
        repo
    }

    fn url(text: &str) -> Result<Source, String> {
        Ok(Source::Url(text.into()))
    }

    #[test]
    fn reads_urls_and_github_names() {
        assert_eq!(
            parse(" https://github.com/acme/app.git "),
            url("https://github.com/acme/app.git")
        );
        assert_eq!(parse("git@github.com:acme/app.git"), url("git@github.com:acme/app.git"));
        assert_eq!(
            parse("ssh://git@host:2222/acme/app"),
            url("ssh://git@host:2222/acme/app")
        );
        assert_eq!(parse("github.com/acme/app"), url("https://github.com/acme/app"));
        assert_eq!(
            parse("acme/app.git"),
            Ok(Source::GitHub {
                owner: "acme".into(),
                repo: "app".into()
            })
        );
        for refused in [
            "",
            "app",
            "acme/app/x",
            "~/code/app",
            "/srv/app",
            "ftp://host/app",
            "acme / app",
            "-acme/app",
            "C:\\code",
        ] {
            assert_eq!(parse(refused), Err(SOURCE_HELP.into()), "{refused}");
        }
    }

    #[test]
    fn names_the_folder_and_finds_the_https_url_of_an_ssh_one() {
        assert_eq!(name_of("https://github.com/acme/app.git").as_deref(), Some("app"));
        assert_eq!(name_of("git@host:app.git").as_deref(), Some("app"));
        assert_eq!(name_of("https://host/group/sub/tool/").as_deref(), Some("tool"));
        assert_eq!(name_of("https://host/.."), None);
        assert_eq!(
            https_of("git@github.com:acme/app.git").as_deref(),
            Some("https://github.com/acme/app.git")
        );
        assert_eq!(
            https_of("ssh://git@host.dev:2222/acme/app").as_deref(),
            Some("https://host.dev/acme/app")
        );
        assert_eq!(https_of("https://github.com/acme/app"), None);
    }

    /// Shallow and blobless only on a cloud machine: the host names its `cloud.json`, and only a file that exists turns
    /// it on.
    #[test]
    fn a_cloud_machines_clone_is_shallow_and_blobless() {
        assert_eq!(clone_args("u", false), ["clone", "--progress", "--", "u"]);
        assert_eq!(
            clone_args("u", true),
            [
                "clone",
                "--progress",
                "--filter=blob:none",
                "--depth=1",
                "--single-branch",
                "--no-tags",
                "--",
                "u"
            ]
        );
        assert!(!blobless());
        let marker = temp("blobless").join("cloud.json");
        blobless_when(Some(marker.clone()));
        assert!(!blobless(), "no cloud.json yet");
        blobless_when(None);
        assert!(!blobless());
    }

    #[test]
    fn reads_gits_progress_lines() {
        assert_eq!(
            parse_progress("Receiving objects:  45% (450/1000), 1.20 MiB | 2.00 MiB/s"),
            Some(Progress {
                stage: "Receiving objects".into(),
                percent: Some(45)
            })
        );
        assert_eq!(
            parse_progress("remote: Enumerating objects: 12, done."),
            Some(Progress::stage("Enumerating objects"))
        );
        assert_eq!(
            parse_progress("Cloning into 'app'..."),
            Some(Progress::stage("Connecting"))
        );
        assert_eq!(parse_progress("fatal: repository 'x' not found"), None);
        assert_eq!(parse_progress("remote: Total 5 (delta 0), reused 0 (delta 0)"), None);
    }

    /// Every phase a shallow clone goes through reads with its percentage, the remote's too; git's warnings and hints
    /// do not.
    #[test]
    fn reads_each_phase_of_a_shallow_clone() {
        let at = |stage: &str, percent: u8| {
            Some(Progress {
                stage: stage.into(),
                percent: Some(percent),
            })
        };
        assert_eq!(
            parse_progress("remote: Counting objects:  43% (3/7)"),
            at("Counting objects", 43)
        );
        assert_eq!(
            parse_progress("remote: Compressing objects: 100% (5/5), done."),
            at("Compressing objects", 100)
        );
        assert_eq!(
            parse_progress("Receiving objects: 100% (7/7), 1.02 KiB | 1.02 MiB/s, done."),
            at("Receiving objects", 100)
        );
        assert_eq!(
            parse_progress("Resolving deltas:   0% (0/2)"),
            at("Resolving deltas", 0)
        );
        assert_eq!(
            parse_progress("Updating files:  12% (1200/10000)"),
            at("Updating files", 12)
        );
        assert_eq!(
            parse_progress("Unpacking objects: 250% (5/2)"),
            at("Unpacking objects", 100),
            "a percentage past 100 reads as 100"
        );
        assert_eq!(
            parse_progress("Receiving objects: 1234 (1234/?)"),
            Some(Progress::stage("Receiving objects"))
        );
        assert_eq!(
            parse_progress("warning: --depth is ignored in local clones; use file:// instead."),
            None
        );
        assert_eq!(parse_progress("hint: Using 'master' as the name"), None);
        assert_eq!(parse_progress("error: RPC failed; curl 92"), None);
        assert_eq!(parse_progress(""), None);
    }

    /// A shallow clone tracks `origin`'s other branches at once, and gets its whole history afterwards.
    #[test]
    fn a_shallow_clone_tracks_every_branch_and_deepens() {
        let root = temp("deepen");
        let repo = origin(&root);
        git_in(&repo, &["commit", "--quiet", "--allow-empty", "-m", "two"]);
        git_in(&repo, &["branch", "other"]);
        let source = format!("file://{}", repo.display());
        let dest = root.join("app");
        git_in(
            &root,
            &[
                "clone",
                "--quiet",
                "--depth=1",
                "--single-branch",
                "--no-tags",
                &source,
                "app",
            ],
        );
        let read = |args: &[&str]| {
            let out = process::command("git")
                .arg("-C")
                .arg(&dest)
                .args(args)
                .output()
                .unwrap_or_else(|e| panic!("{e}"));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        assert_eq!(read(&["rev-parse", "--is-shallow-repository"]), "true");
        let path = std::env::var("PATH").unwrap_or_default();
        track_all_branches(&dest, &path).unwrap_or_else(|e| panic!("{e}"));
        deepen(&dest, &path, false, None, Duration::from_secs(60)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(read(&["rev-parse", "--is-shallow-repository"]), "false");
        assert_eq!(read(&["rev-list", "--count", "HEAD"]), "2");
        assert!(
            !read(&["rev-parse", "--verify", "--quiet", "origin/other"]).is_empty(),
            "the other branch is tracked"
        );
        // Deepening a whole clone again fails, and only says so.
        let error = deepen(&dest, &path, false, None, Duration::from_secs(60))
            .err()
            .unwrap_or_default();
        assert!(error.contains("could not fetch its history"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn keys_github_repositories_as_gateway_does() {
        let key = |text: &str| parse(text).ok().and_then(|source| github_key(&source));
        for text in [
            "Acme/Web",
            "https://github.com/acme/web.git",
            "github.com/ACME/web",
            "git@github.com:acme/web.git",
            "ssh://git@github.com/acme/web",
        ] {
            assert_eq!(key(text).as_deref(), Some("acme/web"), "{text}");
        }
        for text in [
            "https://gitlab.com/acme/web",
            "file:///srv/acme/web",
            "git@host:acme/web.git",
        ] {
            assert_eq!(key(text), None, "{text}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_saved_copy_runs_nothing_its_config_plants() {
        let root = temp("planted");
        let repo = origin(&root);
        let home = root.join("home");
        std::fs::create_dir_all(home.join(".nexus/warm")).unwrap_or_else(|e| panic!("{e}"));
        let source = format!("file://{}", repo.display());
        // The copy's folder differs from `Web` by more than case, so the move also works on a case-insensitive disk (macOS).
        git_in(&home, &["clone", "--quiet", "--depth=1", "--no-tags", &source, "saved"]);
        git_in(&repo, &["commit", "--quiet", "--allow-empty", "-m", "two"]);
        let copy = home.join("saved");
        let ran = root.join("ran");
        let hook = root.join("hook.sh");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch {}\n", ran.display())).unwrap_or_else(|e| panic!("{e}"));
        std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap_or_else(|e| panic!("{e}"));
        git_in(&copy, &["config", "core.fsmonitor", &hook.display().to_string()]);
        let helper_ran = root.join("helper-ran");
        git_in(
            &copy,
            &[
                "config",
                "credential.helper",
                &format!("!touch {}", helper_ran.display()),
            ],
        );
        let saved = serde_json::json!({ "repo": "acme/web", "path": copy });
        std::fs::write(home.join(WARM_MARKER), saved.to_string()).unwrap_or_else(|e| panic!("{e}"));
        let path = std::env::var("PATH").unwrap_or_default();
        let github = Source::GitHub {
            owner: "Acme".into(),
            repo: "Web".into(),
        };
        let chosen = Mutex::new(Vec::new());
        let auth = GithubAuth::new("ghs_secret123").unwrap_or_else(|| panic!("plain"));
        let taken = take_copy(
            &github,
            "~",
            &home,
            &path,
            Some(&auth),
            &|at| chosen.lock().unwrap_or_else(|e| panic!("{e}")).push(at.to_path_buf()),
            &|_| {},
        );
        // Refused: no copy, its folder gone, and nothing it planted ran while the token was in git's environment.
        assert_eq!(taken, None);
        assert!(!copy.exists() && !home.join("Web").exists());
        assert!(!ran.exists(), "the planted fsmonitor ran");
        assert!(!helper_ran.exists(), "the planted credential helper ran");
        assert_eq!(
            chosen.lock().unwrap_or_else(|e| panic!("{e}")).as_slice(),
            [home.join("Web")]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_saved_copy_is_moved_and_brought_up_to_date() {
        let root = temp("warm");
        let repo = origin(&root);
        let home = root.join("home");
        std::fs::create_dir_all(home.join(".nexus/warm")).unwrap_or_else(|e| panic!("{e}"));
        let copy = home.join("web");
        let source = format!("file://{}", repo.display());
        git_in(&home, &["clone", "--quiet", "--depth=1", "--no-tags", &source, "web"]);
        git_in(&repo, &["commit", "--quiet", "--allow-empty", "-m", "two"]);
        let marker = home.join(WARM_MARKER);
        let mark = |repo: &str, at: &Path| {
            let saved = serde_json::json!({ "repo": repo, "path": at });
            std::fs::write(&marker, saved.to_string()).unwrap_or_else(|e| panic!("{e}"));
        };
        let path = std::env::var("PATH").unwrap_or_default();
        let github = Source::GitHub {
            owner: "Acme".into(),
            repo: "Web".into(),
        };
        let stages = Mutex::new(Vec::new());
        let progress = |progress: Progress| stages.lock().unwrap_or_else(|e| panic!("{e}")).push(progress.stage);

        // Another repository's copy is not taken, and stays.
        mark("acme/api", &copy);
        assert_eq!(take_copy(&github, "~", &home, &path, None, &|_| {}, &progress), None);
        assert!(marker.exists() && copy.exists());

        // This one's is moved into the asked folder, with the newest commit, once.
        mark("acme/web", &copy);
        let dest =
            take_copy(&github, "~/code", &home, &path, None, &|_| {}, &progress).unwrap_or_else(|| panic!("no copy"));
        assert_eq!(dest, home.join("code").join("Web"));
        assert!(!copy.exists() && !marker.exists());
        let head = process::command("git")
            .arg("-C")
            .arg(&dest)
            .args(["log", "-1", "--format=%s"])
            .output()
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "two");
        assert_eq!(
            stages.lock().unwrap_or_else(|e| panic!("{e}")).as_slice(),
            ["Starting from a saved copy of acme/web"]
        );
        assert_eq!(take_copy(&github, "~", &home, &path, None, &|_| {}, &progress), None);

        // A copy that cannot update is removed, and the clone runs as usual.
        git_in(
            &dest,
            &["remote", "set-url", "origin", "file:///nonexistent/nexus-warm"],
        );
        mark("acme/web", &dest);
        assert_eq!(
            take_copy(&github, "~/code", &home, &path, None, &|_| {}, &progress),
            None
        );
        assert!(!dest.exists() && !marker.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn runs_an_executable_agents_setup_once_and_logs_it() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp("agents-setup");
        let (home, repo) = (root.join("home"), root.join("repo"));
        std::fs::create_dir_all(repo.join(".agents")).unwrap_or_else(|e| panic!("{e}"));
        let script = repo.join(AGENTS_SETUP);
        // Written by another process: a file this one held open for writing can make the run fail with "text file
        // busy" when another test forks meanwhile.
        let write = |body: &str| {
            let ok = process::command("sh")
                .args(["-c", "printf '%s' \"$1\" > \"$2\"", "sh", body])
                .arg(&script)
                .status()
                .is_ok_and(|status| status.success());
            assert!(ok, "could not write the script");
        };
        write("#!/bin/sh\necho set up\ntouch ran\n");
        let path = std::env::var("PATH").unwrap_or_default();
        let stages = Mutex::new(Vec::new());
        let progress = |progress: Progress| stages.lock().unwrap_or_else(|e| panic!("{e}")).push(progress.stage);

        // Not executable: nothing runs.
        assert_eq!(
            run_agents_setup(&repo, &home, &path, Duration::from_secs(30), &progress),
            Ok(())
        );
        assert!(!repo.join("ran").exists() && !home.join(AGENTS_SETUP_LOG).exists());

        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            run_agents_setup(&repo, &home, &path, Duration::from_secs(30), &progress),
            Ok(())
        );
        assert!(repo.join("ran").exists());
        let log = std::fs::read_to_string(home.join(AGENTS_SETUP_LOG)).unwrap_or_default();
        assert!(log.starts_with("done") && log.contains("set up"), "{log}");
        assert_eq!(
            stages.lock().unwrap_or_else(|e| panic!("{e}")).as_slice(),
            ["Running .agents/setup"]
        );

        // A failing or slow one answers why, and logs what it printed.
        write("#!/bin/sh\necho npm broke >&2\nexit 1\n");
        let why = run_agents_setup(&repo, &home, &path, Duration::from_secs(30), &progress)
            .err()
            .unwrap_or_default();
        assert!(
            why.contains("failed: npm broke") && why.contains(AGENTS_SETUP_LOG),
            "{why}"
        );
        let log = std::fs::read_to_string(home.join(AGENTS_SETUP_LOG)).unwrap_or_default();
        assert!(log.starts_with("failed") && log.contains("npm broke"), "{log}");
        write("#!/bin/sh\nsleep 5\n");
        let why = run_agents_setup(&repo, &home, &path, Duration::from_millis(300), &progress)
            .err()
            .unwrap_or_default();
        assert!(why.contains("did not finish"), "{why}");
        let log = std::fs::read_to_string(home.join(AGENTS_SETUP_LOG)).unwrap_or_default();
        assert!(log.starts_with("stopped"), "{log}");

        // It sees a short list of variables, none of the host's, and what it leaves running ends with it.
        write("#!/bin/sh\nenv > env.txt\nsleep 30 &\necho $! > left.pid\n");
        assert_eq!(
            run_agents_setup(&repo, &home, &path, Duration::from_secs(30), &progress),
            Ok(())
        );
        let seen = std::fs::read_to_string(repo.join("env.txt")).unwrap_or_default();
        assert!(seen.contains("PATH=") && seen.contains("HOME="), "{seen}");
        assert!(
            !seen.contains("CARGO"),
            "the host's environment reached the script: {seen}"
        );
        // A setsid child leaves the group, and still ends; a planted git config is taken out.
        git_in(&repo, &["init", "--quiet"]);
        write("#!/bin/sh\nsetsid sleep 31 &\ngit config core.fsmonitor /tmp/x\ngit config credential.helper '!x'\n");
        let why = run_agents_setup(&repo, &home, &path, Duration::from_secs(30), &progress)
            .err()
            .unwrap_or_default();
        assert!(why.contains("core.fsmonitor"), "{why}");
        std::thread::sleep(Duration::from_millis(300));
        let sleeping = std::fs::read_dir("/proc")
            .map(|d| {
                d.flatten().any(|e| {
                    std::fs::read_to_string(e.path().join("cmdline")).is_ok_and(|c| c.starts_with("sleep\x0031"))
                })
            })
            .unwrap_or(false);
        assert!(!sleeping, "a setsid child outlived the setup");
        let log = std::fs::read_to_string(home.join(AGENTS_SETUP_LOG)).unwrap_or_default();
        assert!(log.starts_with("failed") && log.contains("core.fsmonitor"), "{log}");
        write("#!/bin/sh\nenv > env.txt\nsleep 30 &\necho $! > left.pid\n");
        assert_eq!(
            run_agents_setup(&repo, &home, &path, Duration::from_secs(30), &progress),
            Ok(())
        );
        let left = std::fs::read_to_string(repo.join("left.pid")).unwrap_or_default();
        let alive = |pid: &str| std::path::Path::new(&format!("/proc/{}", pid.trim())).exists();
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !left.trim().is_empty() && !alive(&left),
            "a child the script left still runs: {left}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_review_worktree_below_the_shallow_tip_completes_the_history() {
        let root = temp("below-tip");
        let repo = origin(&root);
        // Someone's pull request #7 forks from "one", below the tip of main ("two").
        git_in(&repo, &["switch", "--quiet", "-c", "theirs"]);
        git_in(&repo, &["commit", "--quiet", "--allow-empty", "-m", "their change"]);
        git_in(&repo, &["update-ref", "refs/pull/7/head", "HEAD"]);
        git_in(&repo, &["switch", "--quiet", "main"]);
        git_in(&repo, &["commit", "--quiet", "--allow-empty", "-m", "two"]);
        let source = format!("file://{}", repo.display());
        git_in(
            &root,
            &[
                "clone",
                "--quiet",
                "--depth=1",
                "--single-branch",
                "--no-tags",
                &source,
                "app",
            ],
        );
        let dest = root.join("app");
        let path = std::env::var("PATH").unwrap_or_default();
        track_all_branches(&dest, &path).unwrap_or_else(|e| panic!("{e}"));
        let wt = root.join("wt");
        git::add_review_worktree(&dest, &wt, "review/pr-7", 7, "main", &path).unwrap_or_else(|e| panic!("{e}"));
        let shallow = git::git(&dest, &["rev-parse", "--is-shallow-repository"]).unwrap_or_default();
        assert_eq!(shallow, "false", "the history was completed before the worktree");
        assert!(git::merge_base(&wt, "origin/main", "HEAD").is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_new_session_worktree_does_not_deepen_the_clone() {
        let root = temp("no-deepen");
        let repo = origin(&root);
        git_in(&repo, &["commit", "--quiet", "--allow-empty", "-m", "two"]);
        let source = format!("file://{}", repo.display());
        git_in(&root, &["clone", "--quiet", "--depth=1", "--no-tags", &source, "app"]);
        let dest = root.join("app");
        git::add_worktree(&dest, &root.join("wt"), "nexus/x", "origin/main").unwrap_or_else(|e| panic!("{e}"));
        let shallow = git::git(&dest, &["rev-parse", "--is-shallow-repository"]).unwrap_or_default();
        assert_eq!(shallow, "true", "a session forked at the tip needs no history");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_destination_is_new_or_empty_under_a_full_path() {
        let home = temp("home");
        assert_eq!(destination("~/code", "app", &home), Ok(home.join("code/app")));
        assert!(home.join("code").is_dir());
        assert_eq!(destination("", "app", &home), Ok(home.join("app")));
        assert!(destination("code", "app", &home).is_err());
        std::fs::create_dir_all(home.join("code/app/src")).unwrap_or_else(|e| panic!("{e}"));
        let taken = destination("~/code", "app", &home).err().unwrap_or_default();
        assert!(taken.contains("already exists"), "{taken}");
        std::fs::create_dir_all(home.join("empty/app")).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(destination("~/empty", "app", &home), Ok(home.join("empty/app")));
        assert!(!home.join("empty/app").exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn clones_with_progress_and_leaves_nothing_when_it_fails() {
        let root = temp("run");
        let repo = origin(&root);
        let path = std::env::var("PATH").unwrap_or_default();
        let plan = Plan {
            urls: vec![repo.to_string_lossy().into_owned()],
            name: "app".into(),
            gh: false,
        };
        let dest = root.join("app");
        let heard = std::sync::Mutex::new(Vec::new());
        clone(&plan, &dest, &path, &|progress| {
            heard.lock().unwrap_or_else(|e| e.into_inner()).push(progress)
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(dest.join("README.md").is_file());
        let heard = heard.into_inner().unwrap_or_default();
        assert!(heard.contains(&Progress::stage("Connecting")), "{heard:?}");

        let missing = Plan {
            urls: vec![root.join("absent").to_string_lossy().into_owned()],
            name: "gone".into(),
            gh: false,
        };
        let error = clone(&missing, &root.join("gone"), &path, &|_| {})
            .err()
            .unwrap_or_default();
        assert!(error.contains("does not exist"), "{error}");
        assert!(!root.join("gone").exists());

        // A folder another clone filled since it was checked is left as it is.
        let taken = root.join("taken");
        std::fs::create_dir_all(&taken).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(taken.join("keep"), "").unwrap_or_else(|e| panic!("{e}"));
        let error = clone(&plan, &taken, &path, &|_| {}).err().unwrap_or_default();
        assert!(error.contains("already exists"), "{error}");
        assert!(taken.join("keep").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An SSH URL the host refuses is cloned again from the next URL; a refusal of the repository itself is not.
    #[cfg(unix)]
    #[test]
    fn falls_back_when_the_ssh_key_is_refused() {
        let root = temp("ssh");
        let repo = origin(&root);
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("{e}"));
        process::write_fake_script(
            &bin.join("ssh"),
            "#!/bin/sh\necho 'git@host: Permission denied (publickey).' >&2\nexit 255\n",
        );
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        let plan = Plan {
            urls: vec!["git@host:acme/app.git".into(), repo.to_string_lossy().into_owned()],
            name: "app".into(),
            gh: false,
        };
        let heard = std::sync::Mutex::new(Vec::new());
        let dest = root.join("app");
        clone(&plan, &dest, &path, &|progress| {
            heard.lock().unwrap_or_else(|e| e.into_inner()).push(progress)
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(dest.join("README.md").is_file());
        let heard = heard.into_inner().unwrap_or_default();
        assert!(heard.iter().any(|p| p.stage.contains("HTTPS")), "{heard:?}");

        let only_ssh = Plan {
            urls: vec!["git@host:acme/app.git".into()],
            name: "app".into(),
            gh: false,
        };
        let error = clone(&only_ssh, &root.join("other"), &path, &|_| {})
            .err()
            .unwrap_or_default();
        assert!(error.contains("refused this machine's SSH key"), "{error}");
        assert!(error.contains("Permission denied (publickey)"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn falls_back_when_the_ssh_port_is_refused() {
        let root = temp("ssh-port");
        let repo = origin(&root);
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("{e}"));
        process::write_fake_script(
            &bin.join("ssh"),
            "#!/bin/sh\necho 'ssh: connect to host host port 22: Connection refused' >&2\nexit 255\n",
        );
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        let plan = Plan {
            urls: vec!["git@host:acme/app.git".into(), repo.to_string_lossy().into_owned()],
            name: "app".into(),
            gh: false,
        };
        let dest = root.join("app");
        clone(&plan, &dest, &path, &|_| {}).unwrap_or_else(|e| panic!("{e}"));
        assert!(dest.join("README.md").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A clone that goes quiet is stopped.
    #[cfg(unix)]
    #[test]
    fn a_stalled_clone_is_stopped() {
        let root = temp("stall");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("{e}"));
        process::write_fake_script(&bin.join("ssh"), "#!/bin/sh\nexec sleep 30\n");
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        let plan = Plan {
            urls: vec!["git@host:acme/app.git".into()],
            name: "app".into(),
            gh: false,
        };
        let started = Instant::now();
        let error = clone_within(
            &plan,
            &root.join("app"),
            &path,
            Duration::from_millis(500),
            None,
            &|_| {},
        )
        .err()
        .unwrap_or_default();
        assert!(error.contains("was stopped"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        assert!(!root.join("app").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `owner/repo` is looked up with the GitHub CLI: SSH first when that is the person's protocol.
    #[cfg(unix)]
    #[test]
    fn plans_a_github_name_with_the_github_cli() {
        let root = temp("gh");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("{e}"));
        let base = std::env::var("PATH").unwrap_or_default();
        let source = Source::GitHub {
            owner: "acme".into(),
            repo: "app".into(),
        };
        // No GitHub CLI: HTTPS, for a public repository.
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).unwrap_or_else(|e| panic!("{e}"));
        let without = plan(&source, &empty.to_string_lossy()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(without.urls, ["https://github.com/acme/app.git"]);
        assert!(!without.gh);

        process::write_fake_script(
            &bin.join("gh"),
            r#"#!/bin/sh
case "$1 $2" in
  "repo view") [ "$3" = acme/app ] || { echo "GraphQL: Could not resolve to a Repository with the name '$3'." >&2; exit 1; }
    echo '{"name":"App","url":"https://github.com/Acme/App","sshUrl":"git@github.com:Acme/App.git"}' ;;
  "config get") echo ssh ;;
esac
"#,
        );
        let path = format!("{}:{base}", bin.display());
        let with = plan(&source, &path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            with,
            Plan {
                urls: vec![
                    "git@github.com:Acme/App.git".into(),
                    "https://github.com/Acme/App.git".into()
                ],
                name: "App".into(),
                gh: true,
            }
        );
        let other = Source::GitHub {
            owner: "acme".into(),
            repo: "nope".into(),
        };
        let error = plan(&other, &path).err().unwrap_or_default();
        assert!(error.contains("Could not resolve"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn says_what_failed_with_what_to_do() {
        let said = "Cloning into 'app'...\nremote: Repository not found.\nfatal: repository 'https://github.com/a/b/' not found";
        assert_eq!(
            failure("https://github.com/a/b", said),
            "repository 'https://github.com/a/b/' not found Check the name, and that `gh auth login` signed you in to \
             an account that can read it."
        );
        assert_eq!(failure("https://host/x", "fatal: unable to access"), "unable to access");
    }
}
