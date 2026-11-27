//! The pull request of a session that runs on a person's own server, read on this computer: the server's `origin`
//! remote and the session's branch name the repository and the pull request (`gh pr view <branch> --repo <owner/repo>`),
//! with this computer's `gh` and sign-in. The server needs no `gh`. A GitLab repository is not read this way yet.

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use nexus_workbench::source_host::Host;

use crate::pull_request::{self, PullRequestStatus};

const READ_DEADLINE: Duration = Duration::from_secs(7);
/// Why a server session's merge request is not read.
pub const GITLAB: &str = "A server session's merge request is not watched yet: its repository is on GitLab.";
/// Why a server session's pull request is not read: its project has no remote this names.
pub const NO_REMOTE: &str = "The server session's repository has no GitHub remote.";
/// Why a server session's pull request is not read: neither a link nor a branch names it.
pub const NO_BRANCH: &str = "The server session has no branch or linked pull request.";

/// A session's repository on another computer: its `origin` remote, the session's branch, and its host.
#[derive(Debug, Clone, PartialEq)]
pub struct Repository {
    pub remote: String,
    pub branch: Option<String>,
    pub host: Host,
}

/// The repository `remote` names for `gh --repo`: `owner/repo` on github.com, `host/owner/repo` on another GitHub host.
/// None for a local path, or a URL without exactly an owner and a name.
pub fn repo_name(remote: &str) -> Option<String> {
    let remote = remote.trim();
    let (host, path) = match remote.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        // `git@host:owner/repo.git`: scp-like, the host before the first `:`, with no `/` before it.
        None => {
            let (host, path) = remote.split_once(':')?;
            if host.contains('/') || host.contains('\\') || host.len() < 2 {
                return None;
            }
            (host, path)
        }
    };
    let host = host.rsplit('@').next()?.split(':').next()?.to_ascii_lowercase();
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts: Vec<&str> = path.split('/').collect();
    let [owner, name] = parts.as_slice() else {
        return None;
    };
    let valid = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if host.is_empty() || !valid(owner) || !valid(name) {
        return None;
    }
    Some(match host.as_str() {
        "github.com" => format!("{owner}/{name}"),
        _ => format!("{host}/{owner}/{name}"),
    })
}

/// Reads the pull request `target` names (its URL or number), else the branch's, of `repository`, with this computer's
/// `gh` in `folder` (any folder: `--repo` names the repository).
pub async fn read(
    repository: &Repository,
    folder: &Path,
    path: &str,
    target: Option<&str>,
) -> Result<PullRequestStatus, String> {
    read_with(repository, OsStr::new("gh"), folder, path, target, READ_DEADLINE).await
}

pub async fn read_with(
    repository: &Repository,
    program: &OsStr,
    folder: &Path,
    path: &str,
    target: Option<&str>,
    deadline: Duration,
) -> Result<PullRequestStatus, String> {
    if repository.host == Host::GitLab {
        return Err(GITLAB.into());
    }
    let repo = repo_name(&repository.remote).ok_or(NO_REMOTE)?;
    let target = target.or(repository.branch.as_deref()).ok_or(NO_BRANCH)?;
    pull_request::view(
        program,
        folder,
        path,
        vec!["pr", "view", target, "--repo", &repo],
        deadline,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_repository_of_a_remote_for_gh() {
        for (remote, name) in [
            ("https://github.com/acme/nexus.git", Some("acme/nexus")),
            ("https://github.com/acme/nexus", Some("acme/nexus")),
            ("git@github.com:acme/nexus.git", Some("acme/nexus")),
            ("ssh://git@github.com/acme/nexus.git", Some("acme/nexus")),
            ("https://token@GitHub.com/acme/nexus.git/", Some("acme/nexus")),
            ("https://ghe.example.com/team/app.git", Some("ghe.example.com/team/app")),
            (
                "ssh://git@ghe.example.com:2222/team/app.git",
                Some("ghe.example.com/team/app"),
            ),
            ("https://github.com/only-owner", None),
            ("https://gitlab.com/group/sub/project.git", None),
            ("https://github.com/../etc", None),
            ("/srv/git/project.git", None),
            ("C:\\repos\\project", None),
        ] {
            assert_eq!(repo_name(remote).as_deref(), name, "{remote}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reads_the_branchs_pull_request_by_the_repositorys_name() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("nexus-remote-pr-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("scratch folder");
        // A stand-in `gh` that writes its arguments next to it and answers one open pull request.
        let gh = dir.join("gh");
        let seen = dir.join("args");
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho '{{\"number\":7,\"title\":\"T\",\"url\":\"https://github.com/o/r/pull/7\",\"isDraft\":false,\"state\":\"OPEN\",\"statusCheckRollup\":[],\"reviewDecision\":null,\"latestReviews\":[],\"mergeable\":\"MERGEABLE\",\"comments\":[],\"headRefName\":\"feat\",\"baseRefName\":\"main\"}}'\n",
                seen.display()
            ),
        )
        .expect("stand-in gh");
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).expect("executable");
        let repository = Repository {
            remote: "git@github.com:o/r.git".into(),
            branch: Some("feat".into()),
            host: Host::GitHub,
        };
        let status = read_with(&repository, gh.as_os_str(), &dir, "/usr/bin:/bin", None, READ_DEADLINE)
            .await
            .expect("read");
        assert_eq!(status.number, 7);
        let args = std::fs::read_to_string(&seen).expect("arguments");
        assert!(args.starts_with("pr\nview\nfeat\n--repo\no/r\n--json\n"), "{args}");
        // A linked pull request is read by its link, in the same repository.
        read_with(
            &repository,
            gh.as_os_str(),
            &dir,
            "/usr/bin:/bin",
            Some("https://github.com/o/r/pull/7"),
            READ_DEADLINE,
        )
        .await
        .expect("read by link");
        let args = std::fs::read_to_string(&seen).expect("arguments");
        assert!(
            args.starts_with("pr\nview\nhttps://github.com/o/r/pull/7\n--repo\no/r\n"),
            "{args}"
        );
        let gitlab = Repository {
            host: Host::GitLab,
            ..repository.clone()
        };
        assert_eq!(
            read_with(&gitlab, gh.as_os_str(), &dir, "", None, READ_DEADLINE).await,
            Err(GITLAB.into())
        );
        let no_branch = Repository {
            branch: None,
            ..repository
        };
        assert_eq!(
            read_with(&no_branch, gh.as_os_str(), &dir, "", None, READ_DEADLINE).await,
            Err(NO_BRANCH.into())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
