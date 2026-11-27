//! A cloud machine's own setup at boot: its session's repository cloned, or its saved copy brought up to
//! date, and its `.agents/setup` run, with no app connected. When Gateway created the machine for a session
//! on a GitHub repository, for a person GitHub lets read it, its `cloud.json` names the repository and a grant. The
//! host starts this once at boot (`Api::begin_cloud_setup`), and asks Gateway for a GitHub App token that may only read
//! that repository ([`TokenSource`]). The token stays in memory and in git's environment ([`clone::GithubAuth`]); it is
//! never written.
//!
//! Where it stands lives here, on the machine: `idle`, `cloning` (git's stage and percent), `setup`, `ready` (its
//! project) or `failed` (why). The apps read it (`wb_cloud_setup`) and wait for it instead of cloning, so a closed app
//! or a dropped connection never stops it. A failure waits for the person's Try again (`wb_cloud_setup_retry`), which
//! runs it again with a new token. A failed `.agents/setup` (an exit other than 0, the time limit, git config it
//! planted) fails it too, with why; its clone stays, and Try again runs only `.agents/setup`. An app's own clone of the same repository (`wb_project_clone`, an older app) waits
//! for it too and answers its project, so the repository is never cloned twice.
//!
//! The state is saved in a file under the host's folder at each step, so a host that restarts (a wake, a crash) knows
//! it: a ready or failed setup stays so, and one that stopped midway runs again from its step. A clone cut off midway
//! is removed first.
//!
//! `idle` means the machine has no setup of its own: no `cloud.json` setup (the feature is off, or the person's GitHub
//! access was not proven), an older host, or Gateway withdrew the grant. The apps then clone as before.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::clone::{self, GithubAuth, Progress};
use crate::event::Project;

/// Where the repository goes on the machine: the home folder, as the apps' cloud clone (`CLONE_PARENT`).
pub const PARENT: &str = "~";

/// The stage the apps show while `.agents/setup` runs.
pub const RUNNING_SETUP: &str = "Running setup";

/// Why a token could not be had from Gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// Gateway no longer gives one for this machine (the grant ran out or is not this machine's): the setup goes back
    /// to `idle`, and the app clones with the person's own sign-in.
    Withdrawn(String),
    /// Anything else: the setup fails with this, and Try again asks again.
    Failed(String),
}

/// Gives the setup its GitHub token: the host asks Gateway with the machine's own Nexus token and the grant.
pub trait TokenSource: Send + Sync {
    fn token(&self) -> BoxFuture<'static, Result<String, TokenError>>;

    /// The setup is ready: Gateway ends the grant, so nothing on the machine can ask for a token any more. A failure
    /// is only logged by the caller's source: the grant also runs out by itself.
    fn done(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

/// The setup's step.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Idle,
    Cloning(Progress),
    Setup,
    Ready { project_id: String },
    Failed { reason: String },
}

/// What `wb_cloud_setup` answers (contract.ts `CloudSetup`): the state, its repository (`owner/name`), git's stage
/// and percent while it clones, the project once ready, and why it failed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub state: &'static str,
    pub repo: Option<String>,
    pub stage: Option<String>,
    pub percent: Option<u8>,
    pub project: Option<Project>,
    pub reason: Option<String>,
    /// Why the last `.agents/resume` failed or timed out, for the status line. None when it passed or none ran. It never
    /// changes `state`: a failed resume does not block the machine.
    pub resume: Option<String>,
}

/// What `.agents/resume` is given: the agent's own environment, once the person's credentials are in place on the
/// machine; None while they are not (the hook then waits for them).
pub type ResumeEnv = Arc<dyn Fn() -> Option<Vec<(String, String)>> + Send + Sync>;

/// The saved state: the step, and where the repository is once its folder was chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    repo: String,
    state: String,
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    /// Whether the clone (or the saved copy) was done at `path`: a failure after it was `.agents/setup`'s, and Try
    /// again runs only that.
    #[serde(default)]
    cloned: bool,
}

#[derive(Default)]
struct Inner {
    /// GitHub's `owner/name`, as Gateway gave it.
    repo: Option<String>,
    phase: Option<Phase>,
    /// The repository's folder once chosen.
    path: Option<PathBuf>,
    /// The clone at `path` is done: what runs now is `.agents/setup`.
    cloned: bool,
    file: Option<PathBuf>,
    token: Option<Arc<dyn TokenSource>>,
    /// The agent's environment for `.agents/resume`; set by the host.
    resume_env: Option<ResumeEnv>,
    /// Why the last `.agents/resume` failed.
    resume: Option<String>,
    /// A `.agents/resume` runs now: a second trigger waits for it instead of running beside it.
    resuming: bool,
}

/// The machine's setup: one per host. Cloned, it is the same.
#[derive(Clone, Default)]
pub struct Setup(Arc<Mutex<Inner>>);

/// How a run starts: from the clone, or with the clone done (a run cut off during `.agents/setup`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    Clone,
    SetupAt(PathBuf),
}

impl Setup {
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Takes the setup of `repo`, saved in `file`, with its token from `token`. What the file says decides the start:
    /// None when there is nothing to run (it was ready or failed, or a setup was taken already).
    pub fn claim(&self, repo: &str, file: PathBuf, token: Arc<dyn TokenSource>, home: &Path) -> Option<Start> {
        let mut inner = self.inner();
        if inner.repo.is_some() {
            return None;
        }
        let saved = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str::<Saved>(&text).ok())
            .filter(|saved| saved.repo == repo);
        inner.repo = Some(repo.to_string());
        inner.file = Some(file);
        inner.token = Some(token);
        let start = match saved {
            Some(Saved {
                state,
                project_id: Some(project_id),
                path,
                ..
            }) if state == "ready" => {
                inner.path = path;
                inner.phase = Some(Phase::Ready { project_id });
                None
            }
            Some(Saved {
                state,
                reason,
                path,
                cloned,
                ..
            }) if state == "failed" => {
                inner.path = path;
                inner.cloned = cloned;
                inner.phase = Some(Phase::Failed {
                    reason: reason.unwrap_or_else(|| "The setup failed.".into()),
                });
                None
            }
            Some(Saved {
                state,
                path: Some(path),
                ..
            }) if state == "setup" && path.join(".git").is_dir() => Some(Start::SetupAt(path)),
            Some(Saved {
                state,
                path: Some(path),
                ..
            }) if state == "cloning" => {
                // A clone cut off midway: only a folder of the home's that the setup chose is removed.
                if inside_home(&path, home) {
                    let _ = std::fs::remove_dir_all(&path);
                }
                Some(Start::Clone)
            }
            _ => Some(Start::Clone),
        };
        if start.is_some() {
            inner.phase = Some(Phase::Cloning(Progress::stage("Finding the repository")));
        }
        start
    }

    /// Gives `.agents/resume` the agent's environment (the host's, with the person's credentials).
    pub fn set_resume_env(&self, env: ResumeEnv) {
        self.inner().resume_env = Some(env);
    }

    /// Takes the run of `.agents/resume`: the folder it runs in and the environment it gets, or None when it must not
    /// run now (the setup is not ready, the person's credentials are not in place, or one runs already).
    fn take_resume(&self) -> Option<(PathBuf, Vec<(String, String)>)> {
        let mut inner = self.inner();
        if inner.resuming || !matches!(inner.phase, Some(Phase::Ready { .. })) {
            return None;
        }
        let path = inner.path.clone()?;
        let env = inner.resume_env.clone()?();
        let env = env?;
        inner.resuming = true;
        Some((path, env))
    }

    fn resumed(&self, outcome: Result<(), String>) {
        let mut inner = self.inner();
        inner.resuming = false;
        inner.resume = outcome.err();
    }

    /// The repository, while the machine has a setup of its own.
    pub fn repo(&self) -> Option<String> {
        let inner = self.inner();
        match inner.phase {
            None | Some(Phase::Idle) => None,
            Some(_) => inner.repo.clone(),
        }
    }

    pub(crate) fn token_source(&self) -> Option<Arc<dyn TokenSource>> {
        self.inner().token.clone()
    }

    /// Whether the setup clones or runs `.agents/setup` now.
    pub fn running(&self) -> bool {
        matches!(self.inner().phase, Some(Phase::Cloning(_) | Phase::Setup))
    }

    /// From failed to running again (Try again), and where the run starts: only `.agents/setup` when that is what
    /// failed and the clone is still there, else the clone. None when it had not failed.
    pub(crate) fn restart(&self) -> Option<Start> {
        let mut inner = self.inner();
        if !matches!(inner.phase, Some(Phase::Failed { .. })) {
            return None;
        }
        let start = match inner.path.clone() {
            Some(path) if inner.cloned && path.join(".git").is_dir() => {
                inner.phase = Some(Phase::Setup);
                Start::SetupAt(path)
            }
            _ => {
                inner.phase = Some(Phase::Cloning(Progress::stage("Finding the repository")));
                Start::Clone
            }
        };
        Some(start)
    }

    /// git's progress while it clones.
    pub fn progress(&self, progress: Progress) {
        let mut inner = self.inner();
        if matches!(inner.phase, Some(Phase::Cloning(_))) {
            inner.phase = Some(Phase::Cloning(progress));
        }
    }

    /// The folder the clone goes in, saved before git starts, so a restart can remove a clone cut off midway.
    /// The saved copy's folder too, before it moves there: a restart removes a copy cut off in its refresh.
    pub fn cloning_at(&self, path: &Path) {
        let mut inner = self.inner();
        inner.path = Some(path.to_path_buf());
        inner.cloned = false;
        save(&inner, "cloning");
    }

    pub fn setting_up(&self, path: &Path) {
        let mut inner = self.inner();
        inner.path = Some(path.to_path_buf());
        inner.cloned = true;
        inner.phase = Some(Phase::Setup);
        save(&inner, "setup");
    }

    pub fn ready(&self, project_id: String) {
        let mut inner = self.inner();
        inner.phase = Some(Phase::Ready { project_id });
        save(&inner, "ready");
    }

    pub fn fail(&self, reason: String) {
        let mut inner = self.inner();
        eprintln!("nexus-workbench: the machine's setup failed: {reason}");
        inner.phase = Some(Phase::Failed { reason });
        save(&inner, "failed");
    }

    /// No setup of the machine's own any more: the apps clone as before. Its file goes.
    pub fn withdraw(&self, why: &str) {
        let mut inner = self.inner();
        eprintln!("nexus-workbench: the machine's setup was withdrawn: {why}");
        inner.phase = Some(Phase::Idle);
        // Saved, not removed: the machine's preparation script holds the person's secrets back
        // until the setup is ready, failed or withdrawn.
        save(&inner, "withdrawn");
    }

    /// The project's id once ready.
    pub fn ready_project(&self) -> Option<String> {
        match &self.inner().phase {
            Some(Phase::Ready { project_id }) => Some(project_id.clone()),
            _ => None,
        }
    }

    /// Whether the clone stands at its folder after the setup went past it (a failed `.agents/setup` keeps it).
    pub fn kept_clone(&self) -> bool {
        let inner = self.inner();
        inner.cloned && inner.path.as_ref().is_some_and(|path| path.join(".git").is_dir())
    }

    /// Why it failed, when it did.
    pub fn failure(&self) -> Option<String> {
        match &self.inner().phase {
            Some(Phase::Failed { reason }) => Some(reason.clone()),
            _ => None,
        }
    }

    /// git's progress while it clones; the setup's stage while `.agents/setup` runs.
    pub fn stage(&self) -> Option<Progress> {
        match &self.inner().phase {
            Some(Phase::Cloning(progress)) => Some(progress.clone()),
            Some(Phase::Setup) => Some(Progress::stage(RUNNING_SETUP)),
            _ => None,
        }
    }

    /// Where it stands, with `project` to read the ready one's project.
    pub fn view(&self, project: impl FnOnce(&str) -> Option<Project>) -> View {
        let (phase, repo) = {
            let inner = self.inner();
            (inner.phase.clone().unwrap_or(Phase::Idle), inner.repo.clone())
        };
        let mut view = View {
            state: "idle",
            repo: None,
            stage: None,
            percent: None,
            project: None,
            reason: None,
            resume: self.inner().resume.clone(),
        };
        if phase == Phase::Idle {
            return view;
        }
        view.repo = repo;
        match phase {
            Phase::Idle => {}
            Phase::Cloning(progress) => {
                view.state = "cloning";
                view.stage = Some(progress.stage);
                view.percent = progress.percent;
            }
            Phase::Setup => {
                view.state = "setup";
                view.stage = Some(RUNNING_SETUP.into());
            }
            Phase::Ready { project_id } => match project(&project_id) {
                Some(found) => {
                    view.state = "ready";
                    view.project = Some(found);
                }
                // The person removed its project since: the machine has no setup of its own to wait for.
                None => return View { repo: None, ..view },
            },
            Phase::Failed { reason } => {
                view.state = "failed";
                view.reason = Some(reason);
            }
        }
        view
    }
}

/// Saves the state as `state`. A failure is only logged: the setup goes on, and a restart runs it again.
fn save(inner: &Inner, state: &str) {
    let (Some(file), Some(repo)) = (&inner.file, &inner.repo) else {
        return;
    };
    let saved = Saved {
        repo: repo.clone(),
        state: state.into(),
        path: inner.path.clone(),
        project_id: match &inner.phase {
            Some(Phase::Ready { project_id }) => Some(project_id.clone()),
            _ => None,
        },
        reason: match &inner.phase {
            Some(Phase::Failed { reason }) => Some(reason.clone()),
            _ => None,
        },
        cloned: inner.cloned,
    };
    let text = serde_json::to_string_pretty(&saved).unwrap_or_default();
    if let Some(folder) = file.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    if let Err(error) = std::fs::write(file, format!("{text}\n")) {
        eprintln!(
            "nexus-workbench: could not save the setup's state in {}: {error}",
            file.display()
        );
    }
}

/// Runs the repository's `.agents/resume` once the setup is ready and the person's credentials are in place, for at most
/// `limit`, and records why it failed for the status line ([`View::resume`]). Blocks: run it off the async threads.
/// Answers whether it ran. Nothing here fails the machine: a failure or a timeout is only reported.
pub fn resume(setup: &Setup, home: &Path, limit: std::time::Duration) -> bool {
    let Some((path, env)) = setup.take_resume() else {
        return false;
    };
    setup.resumed(clone::run_agents_resume(&path, home, &env, limit));
    true
}

/// Whether `path`, read from the saved state on disk, is a folder strictly inside `home` that a recursive delete may
/// take. `Path::starts_with` compares parts, so `<home>/../x` would pass it: any part that is not a plain name (`..`,
/// `.`, a root) is refused first.
fn inside_home(path: &Path, home: &Path) -> bool {
    use std::path::Component;
    let Ok(rest) = path.strip_prefix(home) else {
        return false;
    };
    !rest.as_os_str().is_empty() && rest.components().all(|part| matches!(part, Component::Normal(_)))
}

/// `repo` (`owner/name`, from Gateway's `cloud.json`) split in two, when both are GitHub names ([`clone::github_name`]):
/// a name like `..` would reach outside the clone's folder, and the owner goes into the clone's URL.
fn owner_and_name(repo: &str) -> Option<(&str, &str)> {
    let (owner, name) = repo.split_once('/')?;
    (clone::github_name(owner) && clone::github_name(name) && !owner.starts_with('.')).then_some((owner, name))
}

/// The clone's plan for GitHub's `repo` (`owner/name`): over HTTPS, signed in with the setup's token, not the CLI.
pub fn plan(repo: &str) -> Option<clone::Plan> {
    let (_, name) = owner_and_name(repo)?;
    Some(clone::Plan {
        urls: vec![format!("https://github.com/{repo}.git")],
        name: name.to_string(),
        gh: false,
    })
}

/// The source the apps' clone reads `repo` as.
pub fn source(repo: &str) -> Option<clone::Source> {
    let (owner, name) = owner_and_name(repo)?;
    Some(clone::Source::GitHub {
        owner: owner.into(),
        repo: name.into(),
    })
}

/// Clones `repo` into the home folder with `auth`, or takes its saved copy (the clone alone), and runs its `.agents/setup`
/// either way. From `start`: a run cut off during `.agents/setup`, or a Try again after it failed, runs only that again,
/// and needs no `auth`. When the clone step fails because an app's own clone took the folder (`known` says it is a
/// project), that clone is the repository's, and nothing more runs. Answers the repository's folder, or why it failed:
/// a failed `.agents/setup` fails the run, whatever `known` says, and its clone stays.
pub fn run(
    setup: &Setup,
    repo: &str,
    start: Start,
    home: &Path,
    path: &str,
    auth: Option<&GithubAuth>,
    known: &dyn Fn(&Path) -> bool,
) -> Result<PathBuf, String> {
    let warm = |source: &clone::Source, chosen: &dyn Fn(&Path), progress: &dyn Fn(Progress)| {
        clone::warm(source, PARENT, home, path, auth, chosen, progress)
    };
    let agents = |dest: &Path| clone::agents_setup(dest, home, path, &|_| {});
    run_with(setup, repo, start, home, path, auth, known, &warm, &agents)
}

/// Takes the saved copy of a source, when there is one, telling its folder before it moves there.
type Warm<'a> = &'a dyn Fn(&clone::Source, &dyn Fn(&Path), &dyn Fn(Progress)) -> Option<PathBuf>;

/// [`run`], with the saved copy's taker and the setup's runner given.
#[allow(clippy::too_many_arguments)]
fn run_with(
    setup: &Setup,
    repo: &str,
    start: Start,
    home: &Path,
    path: &str,
    auth: Option<&GithubAuth>,
    known: &dyn Fn(&Path) -> bool,
    warm: Warm,
    agents: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let dest = match start {
        Start::SetupAt(dest) => dest,
        Start::Clone => {
            let cloned = (|| {
                let source = source(repo).ok_or_else(|| format!("{repo} is not a GitHub repository."))?;
                let progress = |progress: Progress| setup.progress(progress);
                // The copy's folder is saved before its refresh, as a clone's: a restart midway removes it and clones.
                if let Some(dest) = warm(&source, &|dest| setup.cloning_at(dest), &progress) {
                    return Ok(dest);
                }
                let plan = plan(repo).ok_or_else(|| format!("{repo} is not a GitHub repository."))?;
                let auth = auth.ok_or_else(|| format!("No access to {repo} for the clone."))?;
                let dest = clone::destination(PARENT, &plan.name, home)?;
                setup.cloning_at(&dest);
                clone::clone_with(&plan, &dest, path, auth, &progress)?;
                Ok(dest)
            })();
            match cloned {
                Ok(dest) => dest,
                Err(error) => {
                    // An app's own clone took the folder meanwhile: its project is this repository's.
                    let theirs = home.join(repo.rsplit('/').next().unwrap_or(repo));
                    return if known(&theirs) { Ok(theirs) } else { Err(error) };
                }
            }
        }
    };
    setup.setting_up(&dest);
    agents(&dest)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_path_with_dot_dot_is_never_inside_the_home() {
        let home = Path::new("/home/me");
        assert!(inside_home(Path::new("/home/me/code/app"), home));
        assert!(!inside_home(Path::new("/home/me"), home));
        assert!(!inside_home(Path::new("/home/me/../other"), home));
        assert!(!inside_home(Path::new("/home/me/code/../../other"), home));
        assert!(!inside_home(Path::new("/etc"), home));
    }

    #[test]
    fn a_repo_whose_parts_are_not_github_names_has_no_plan_or_source() {
        assert!(plan("acme/app").is_some());
        assert!(source("acme/app").is_some());
        for bad in [
            "acme/../../x",
            "acme/..",
            "../app",
            "-x/app",
            "acme/-app",
            "acme/a b",
            "acme",
            "/app",
            "acme/",
        ] {
            assert!(plan(bad).is_none(), "{bad}");
            assert!(source(bad).is_none(), "{bad}");
        }
    }

    struct NoToken;

    impl TokenSource for NoToken {
        fn token(&self) -> BoxFuture<'static, Result<String, TokenError>> {
            Box::pin(async { Err(TokenError::Failed("none".into())) })
        }
    }

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wb-cloud-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        dir
    }

    fn project(id: &str) -> Project {
        serde_json::from_value(
            serde_json::json!({ "id": id, "name": "web", "path": "/home/user/web", "branch": "main", "addedAt": 0 }),
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    /// Writes an executable `.agents/setup` saying `body` in `repo`, from another process (see clone.rs's test).
    #[cfg(unix)]
    fn script(repo: &Path, body: &str) {
        std::fs::create_dir_all(repo.join(".agents")).unwrap_or_else(|e| panic!("{e}"));
        let script = repo.join(clone::AGENTS_SETUP);
        let ok = crate::process::command("sh")
            .args(["-c", "printf '%s' \"$1\" > \"$2\" && chmod 755 \"$2\"", "sh", body])
            .arg(&script)
            .status()
            .is_ok_and(|status| status.success());
        assert!(ok, "could not write the script");
    }

    #[cfg(unix)]
    #[test]
    fn a_saved_copy_runs_its_agents_setup_once() {
        let home = dir("warm-setup");
        let copy = home.join("web");
        std::fs::create_dir_all(copy.join(".git")).unwrap_or_else(|e| panic!("{e}"));
        script(&copy, "#!/bin/sh\necho x >> ran\n");
        let auth = GithubAuth::new("ghs_secret123").unwrap_or_else(|| panic!("plain"));
        let path = std::env::var("PATH").unwrap_or_default();
        let file = home.join(".nexus/host/setup.json");
        let setup = Setup::default();
        assert_eq!(
            setup.claim("acme/web", file.clone(), Arc::new(NoToken), &home),
            Some(Start::Clone)
        );
        // What the saved state said while the copy was being taken: its folder, still cloning.
        let midway = std::sync::Mutex::new(String::new());
        let taken = copy.clone();
        let warm = |_: &clone::Source, chosen: &dyn Fn(&Path), _: &dyn Fn(Progress)| {
            chosen(&taken);
            *midway.lock().unwrap_or_else(|e| panic!("{e}")) = std::fs::read_to_string(&file).unwrap_or_default();
            Some(taken.clone())
        };
        let agents =
            |dest: &Path| clone::run_agents_setup(dest, &home, &path, std::time::Duration::from_secs(30), &|_| {});
        let dest = run_with(
            &setup,
            "acme/web",
            Start::Clone,
            &home,
            &path,
            Some(&auth),
            &|_| false,
            &warm,
            &agents,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(dest, copy);
        let ran = std::fs::read_to_string(copy.join("ran")).unwrap_or_default();
        assert_eq!(ran.lines().count(), 1, "the copy's .agents/setup ran once: {ran:?}");

        // A host that restarted during the copy's refresh removes the half-updated copy and clones again.
        let midway = midway.into_inner().unwrap_or_else(|e| panic!("{e}"));
        assert!(midway.contains("\"state\": \"cloning\""), "{midway}");
        std::fs::write(&file, midway).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            Setup::default().claim("acme/web", file, Arc::new(NoToken), &home),
            Some(Start::Clone)
        );
        assert!(!copy.exists(), "the copy cut off in its refresh is removed");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_agents_setup_fails_the_run_and_try_again_runs_only_it() {
        let home = dir("setup-fails");
        let copy = home.join("web");
        std::fs::create_dir_all(copy.join(".git")).unwrap_or_else(|e| panic!("{e}"));
        script(&copy, "#!/bin/sh\necho x >> tries\necho npm broke >&2\nexit 1\n");
        let auth = GithubAuth::new("ghs_secret123").unwrap_or_else(|| panic!("plain"));
        let path = std::env::var("PATH").unwrap_or_default();
        let file = home.join(".nexus/host/setup.json");
        let setup = Setup::default();
        let start = setup
            .claim("acme/web", file.clone(), Arc::new(NoToken), &home)
            .unwrap_or_else(|| panic!("nothing to run"));
        let takes = std::sync::atomic::AtomicUsize::new(0);
        let taken = copy.clone();
        let warm = |_: &clone::Source, chosen: &dyn Fn(&Path), _: &dyn Fn(Progress)| {
            takes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            chosen(&taken);
            Some(taken.clone())
        };
        let agents =
            |dest: &Path| clone::run_agents_setup(dest, &home, &path, std::time::Duration::from_secs(30), &|_| {});

        // `exit 1`: the run answers why, as `run_cloud_setup` then fails the setup, and the clone stays.
        let why = run_with(
            &setup,
            "acme/web",
            start,
            &home,
            &path,
            Some(&auth),
            &|_| false,
            &warm,
            &agents,
        )
        .err()
        .unwrap_or_default();
        assert!(why.contains(".agents/setup failed: npm broke"), "{why}");
        setup.fail(why.clone());
        let view = setup.view(|_| None);
        assert_eq!((view.state, view.reason.as_deref()), ("failed", Some(why.as_str())));
        assert!(copy.join(".git").is_dir() && setup.kept_clone());

        // A failed `.agents/setup` stays failed even when its folder is a project now (the person added it).
        let again = run_with(
            &setup,
            "acme/web",
            Start::SetupAt(copy.clone()),
            &home,
            &path,
            None,
            &|_| true,
            &warm,
            &agents,
        );
        setup.fail(again.err().unwrap_or_else(|| panic!("a failed setup ended well")));

        // Try again, after a host restart too, runs only `.agents/setup` on the clone, with no token, and ends ready.
        let restarted = Setup::default();
        assert_eq!(
            restarted.claim("acme/web", file.clone(), Arc::new(NoToken), &home),
            None
        );
        let start = restarted.restart().unwrap_or_else(|| panic!("not failed"));
        assert_eq!(start, Start::SetupAt(copy.clone()));
        assert!(restarted.running());
        script(&copy, "#!/bin/sh\necho x >> tries\n");
        let dest = run_with(
            &restarted,
            "acme/web",
            start,
            &home,
            &path,
            None,
            &|_| false,
            &warm,
            &agents,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(dest, copy);
        restarted.ready("p1".into());
        assert_eq!(
            takes.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the copy was taken once"
        );
        let tries = std::fs::read_to_string(copy.join("tries")).unwrap_or_default();
        assert_eq!(tries.lines().count(), 3, "{tries:?}");
        assert_eq!(restarted.view(|id| Some(project(id))).state, "ready");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_clone_that_finds_the_apps_own_clone_takes_it_and_runs_nothing() {
        let home = dir("theirs");
        let theirs = home.join("web");
        std::fs::create_dir_all(theirs.join(".git")).unwrap_or_else(|e| panic!("{e}"));
        let path = std::env::var("PATH").unwrap_or_default();
        let warm = |_: &clone::Source, _: &dyn Fn(&Path), _: &dyn Fn(Progress)| None;
        let ran = std::sync::atomic::AtomicUsize::new(0);
        let agents = |_: &Path| {
            ran.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("must not run".to_string())
        };
        let auth = GithubAuth::new("ghs_secret123").unwrap_or_else(|| panic!("plain"));
        // Not a project: the folder is in the way, and the run says so.
        let why = run_with(
            &Setup::default(),
            "acme/web",
            Start::Clone,
            &home,
            &path,
            Some(&auth),
            &|_| false,
            &warm,
            &agents,
        )
        .err()
        .unwrap_or_default();
        assert!(why.contains("already exists"), "{why}");
        // The app's own clone, a project: it is the repository's, and its setup is not run here.
        let known = |at: &Path| at == theirs;
        let dest = run_with(
            &Setup::default(),
            "acme/web",
            Start::Clone,
            &home,
            &path,
            Some(&auth),
            &known,
            &warm,
            &agents,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(dest, theirs);
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_machine_without_a_setup_is_idle() {
        let setup = Setup::default();
        let view = setup.view(|_| None);
        assert_eq!(view.state, "idle");
        assert_eq!(view.repo, None);
        assert!(!setup.running());
    }

    #[test]
    fn a_setup_is_taken_once_and_says_each_step() {
        let home = dir("steps");
        let file = home.join(".nexus/host/setup.json");
        let setup = Setup::default();
        assert_eq!(
            setup.claim("acme/web", file.clone(), Arc::new(NoToken), &home),
            Some(Start::Clone)
        );
        // A second start (a second boot task, a reconnect) takes nothing.
        assert_eq!(setup.claim("acme/web", file.clone(), Arc::new(NoToken), &home), None);
        assert!(setup.running());
        setup.progress(Progress {
            stage: "Receiving objects".into(),
            percent: Some(43),
        });
        let view = setup.view(|_| None);
        assert_eq!(
            (view.state, view.stage.as_deref(), view.percent),
            ("cloning", Some("Receiving objects"), Some(43))
        );
        assert_eq!(view.repo.as_deref(), Some("acme/web"));
        setup.setting_up(&home.join("web"));
        assert_eq!(setup.view(|_| None).stage.as_deref(), Some(RUNNING_SETUP));
        setup.ready("p1".into());
        let view = setup.view(|id| Some(project(id)));
        assert_eq!(view.state, "ready");
        assert_eq!(view.project.map(|p| p.id).as_deref(), Some("p1"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_restarted_host_knows_where_its_setup_stood() {
        let home = dir("restart");
        let file = home.join(".nexus/host/setup.json");
        // Ready stays ready, and runs nothing.
        let first = Setup::default();
        first.claim("acme/web", file.clone(), Arc::new(NoToken), &home);
        first.ready("p1".into());
        let again = Setup::default();
        assert_eq!(again.claim("acme/web", file.clone(), Arc::new(NoToken), &home), None);
        assert_eq!(again.ready_project().as_deref(), Some("p1"));
        // Failed stays failed until Try again, with why.
        again.fail("GitHub refused the clone.".into());
        let failed = Setup::default();
        assert_eq!(failed.claim("acme/web", file.clone(), Arc::new(NoToken), &home), None);
        let view = failed.view(|_| None);
        assert_eq!(
            (view.state, view.reason.as_deref()),
            ("failed", Some("GitHub refused the clone."))
        );
        assert_eq!(failed.restart(), Some(Start::Clone));
        assert!(failed.running());
        assert_eq!(failed.restart(), None);
        // A clone cut off midway is removed and cloned again.
        let partial = home.join("web");
        std::fs::create_dir_all(partial.join(".git")).unwrap_or_else(|e| panic!("{e}"));
        failed.cloning_at(&partial);
        let restarted = Setup::default();
        assert_eq!(
            restarted.claim("acme/web", file.clone(), Arc::new(NoToken), &home),
            Some(Start::Clone)
        );
        assert!(!partial.exists());
        // One cut off in `.agents/setup` runs only that again.
        std::fs::create_dir_all(partial.join(".git")).unwrap_or_else(|e| panic!("{e}"));
        restarted.setting_up(&partial);
        let resumed = Setup::default();
        assert_eq!(
            resumed.claim("acme/web", file.clone(), Arc::new(NoToken), &home),
            Some(Start::SetupAt(partial.clone()))
        );
        // Another repository's saved state is not this one's.
        let other = Setup::default();
        assert_eq!(
            other.claim("acme/api", file, Arc::new(NoToken), &home),
            Some(Start::Clone)
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_withdrawn_setup_leaves_the_clone_to_the_app() {
        let home = dir("withdrawn");
        let file = home.join(".nexus/host/setup.json");
        let setup = Setup::default();
        setup.claim("acme/web", file.clone(), Arc::new(NoToken), &home);
        setup.cloning_at(&home.join("web"));
        setup.withdraw("the grant ran out");
        assert_eq!(setup.view(|_| None).state, "idle");
        assert_eq!(setup.repo(), None);
        // Its state is saved, so the machine's preparation can tell a withdrawn setup from one still running.
        let saved = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(saved.contains("\"state\": \"withdrawn\""), "{saved}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_ready_setup_whose_project_was_removed_is_idle() {
        let home = dir("removed");
        let setup = Setup::default();
        setup.claim("acme/web", home.join("setup.json"), Arc::new(NoToken), &home);
        setup.ready("p1".into());
        assert_eq!(setup.view(|_| None).state, "idle");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_token_never_shows() {
        let auth = GithubAuth::new("ghs_secret123").unwrap_or_else(|| panic!("plain"));
        assert!(!format!("{auth:?}").contains("secret"));
        assert_eq!(GithubAuth::new("a b"), None);
        assert_eq!(GithubAuth::new(""), None);
    }

    /// Writes an executable `.agents/resume` saying `body` in `repo`.
    #[cfg(unix)]
    fn resume_script(repo: &Path, body: &str) {
        std::fs::create_dir_all(repo.join(".agents")).unwrap_or_else(|e| panic!("{e}"));
        let script = repo.join(clone::AGENTS_RESUME);
        let ok = crate::process::command("sh")
            .args(["-c", "printf '%s' \"$1\" > \"$2\" && chmod 755 \"$2\"", "sh", body])
            .arg(&script)
            .status()
            .is_ok_and(|status| status.success());
        assert!(ok, "could not write the script");
    }

    /// A ready setup of `acme/web` at `home/web`, whose resume env is `env` (None: no personal credentials yet).
    #[cfg(unix)]
    fn ready_with(home: &Path, env: Option<Vec<(String, String)>>) -> (Setup, PathBuf) {
        let repo = home.join("web");
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
        let setup = Setup::default();
        setup.claim("acme/web", home.join("setup.json"), Arc::new(NoToken), home);
        setup.cloning_at(&repo);
        setup.ready("p1".into());
        setup.set_resume_env(Arc::new(move || env.clone()));
        (setup, repo)
    }

    #[cfg(unix)]
    #[test]
    fn resume_runs_with_the_agents_environment_once_credentials_are_in() {
        let home = dir("resume-env");
        let personal = vec![
            ("PATH".to_string(), std::env::var("PATH").unwrap_or_default()),
            ("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-ant-oat-test".to_string()),
        ];
        let (setup, repo) = ready_with(&home, Some(personal));
        resume_script(&repo, "#!/bin/sh\nenv > env.txt\n");
        let limit = std::time::Duration::from_secs(10);
        assert!(resume(&setup, &home, limit));
        let seen = std::fs::read_to_string(repo.join("env.txt")).unwrap_or_default();
        assert!(seen.contains("CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat-test"), "{seen}");
        assert_eq!(setup.view(|_| None).resume, None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn resume_waits_for_credentials_and_for_a_ready_setup() {
        let home = dir("resume-wait");
        let limit = std::time::Duration::from_secs(10);
        // No personal credentials yet: the hook does not run.
        let (setup, repo) = ready_with(&home, None);
        resume_script(&repo, "#!/bin/sh\ntouch ran\n");
        assert!(!resume(&setup, &home, limit));
        assert!(!repo.join("ran").exists());
        // A setup still running: it does not run either.
        let other = dir("resume-wait-setup");
        let running = Setup::default();
        running.claim("acme/web", other.join("setup.json"), Arc::new(NoToken), &other);
        running.set_resume_env(Arc::new(|| Some(Vec::new())));
        assert!(!resume(&running, &other, limit));
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_or_slow_resume_shows_in_the_status_line_and_blocks_nothing() {
        let home = dir("resume-fail");
        let env = vec![("PATH".to_string(), std::env::var("PATH").unwrap_or_default())];
        let (setup, repo) = ready_with(&home, Some(env));
        resume_script(&repo, "#!/bin/sh\necho no login >&2\nexit 3\n");
        assert!(resume(&setup, &home, std::time::Duration::from_secs(10)));
        let view = setup.view(|_| Some(project("p1")));
        assert_eq!(view.state, "ready", "a failed resume leaves the setup ready");
        assert!(
            view.resume
                .as_deref()
                .is_some_and(|why| why.contains("failed: no login")),
            "{view:?}"
        );
        // A slow one is stopped at the limit, and its leftovers end with it.
        resume_script(&repo, "#!/bin/sh\nsleep 5\n");
        let started = std::time::Instant::now();
        assert!(resume(&setup, &home, std::time::Duration::from_millis(300)));
        assert!(started.elapsed() < std::time::Duration::from_secs(4));
        let view = setup.view(|_| Some(project("p1")));
        assert!(
            view.resume.as_deref().is_some_and(|why| why.contains("did not finish")),
            "{view:?}"
        );
        // The next good run clears the note.
        resume_script(&repo, "#!/bin/sh\nexit 0\n");
        assert!(resume(&setup, &home, std::time::Duration::from_secs(10)));
        assert_eq!(setup.view(|_| Some(project("p1"))).resume, None);
        let _ = std::fs::remove_dir_all(&home);
    }
}
