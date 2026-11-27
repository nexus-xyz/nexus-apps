//! Where a repository's pull requests live, and the command-line tool that reaches them: GitHub through
//! `gh`, GitLab (gitlab.com and self-managed) through `glab`, each with the person's own sign-in, never read here.
//!
//! The host comes from the repository's `origin` remote ([`Host::of_repo`]): `gitlab.com`, or a host name with a
//! `gitlab` label (`gitlab.example.com`), is GitLab. A self-managed GitLab under another name is set per repository
//! with `git config nexus.host gitlab`. Everything else stays GitHub, as before. GitLab calls a pull request a merge
//! request and numbers it `!123`: [`Host`] carries those words for the app's messages, and the windows get the host
//! with each project (`Project::host`) and each pull request. T3 Code tells hosts apart the same way
//! (packages/source-control-gitlab/src/shared/definition.ts).
//!
//! Each host's own commands and answers live in its module: GitHub's in `git.rs` and the desktop's `pull_request.rs`,
//! GitLab's in [`crate::gitlab`]. Another host, such as Bitbucket, is one more variant here and one more module.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// A source-control host whose pull requests the app opens, checks out for review and watches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Host {
    #[default]
    #[serde(rename = "github")]
    GitHub,
    #[serde(rename = "gitlab")]
    GitLab,
}

impl Host {
    /// For `skip_serializing_if`: GitHub is the default, so older windows and files read as before.
    pub fn is_github(&self) -> bool {
        *self == Host::GitHub
    }

    /// The host a remote URL names, when it is one this knows by name: `https://`, `ssh://` and `git@host:path`.
    pub fn of_url(url: &str) -> Option<Host> {
        let name = host_name(url)?;
        if name == "github.com" {
            Some(Host::GitHub)
        } else if name == "gitlab.com" || name.split('.').any(|label| label == "gitlab") {
            Some(Host::GitLab)
        } else {
            None
        }
    }

    /// The host of the repository at `path` (a worktree too): its `nexus.host` setting, else its first remote's
    /// (`origin` first), else GitHub. Reads the config file without running git.
    pub fn of_repo(path: &Path) -> Host {
        let config = crate::git::config_text(path).unwrap_or_default();
        if let Some(set) = crate::git::config_value(&config, "nexus", "host") {
            match set.to_ascii_lowercase().as_str() {
                "gitlab" => return Host::GitLab,
                "github" => return Host::GitHub,
                _ => {}
            }
        }
        crate::git::remotes(path)
            .first()
            .and_then(|url| Host::of_url(url))
            .unwrap_or_default()
    }

    /// The command-line tool: `gh` or `glab`.
    pub fn program(self) -> &'static str {
        match self {
            Host::GitHub => "gh",
            Host::GitLab => "glab",
        }
    }

    /// `GitHub` or `GitLab`.
    pub fn name(self) -> &'static str {
        match self {
            Host::GitHub => "GitHub",
            Host::GitLab => "GitLab",
        }
    }

    /// `GitHub CLI` or `GitLab CLI`.
    pub fn cli(self) -> &'static str {
        match self {
            Host::GitHub => "GitHub CLI",
            Host::GitLab => "GitLab CLI",
        }
    }

    /// `pull request` or `merge request`.
    pub fn request(self) -> &'static str {
        match self {
            Host::GitHub => "pull request",
            Host::GitLab => "merge request",
        }
    }

    /// `Pull request` or `Merge request`, to start a sentence.
    pub fn request_title(self) -> &'static str {
        match self {
            Host::GitHub => "Pull request",
            Host::GitLab => "Merge request",
        }
    }

    /// How the host writes request `number`: `#42` or `!42`.
    pub fn reference(self, number: u64) -> String {
        match self {
            Host::GitHub => format!("#{number}"),
            Host::GitLab => format!("!{number}"),
        }
    }

    /// The tool's sign-in command: `gh auth login` or `glab auth login`.
    pub fn sign_in(self) -> String {
        format!("{} auth login", self.program())
    }

    /// The answer when the tool can't be run: `{what} needs the GitLab CLI: install glab, run …`.
    pub fn needs(self, what: &str) -> String {
        format!(
            "{what} needs the {}: install {}, run `{}`, then try again.",
            self.cli(),
            self.program(),
            self.sign_in()
        )
    }

    /// Whether `text` says the tool can't be run here: [`Host::needs`]'s words.
    pub fn missing(self, text: &str) -> bool {
        text.contains(&format!("needs the {}: install {}", self.cli(), self.program()))
    }

    /// Whether `text` says the tool is not signed in: its own words, or [`Host::error`]'s. A text that says the tool
    /// is missing names the sign-in too: check [`Host::missing`] first.
    pub fn signed_out(self, text: &str) -> bool {
        let lower = text.to_lowercase();
        match self {
            Host::GitHub => text.contains("gh auth login") || text.contains("not logged"),
            Host::GitLab => {
                lower.contains("glab auth login")
                    || lower.contains("401 unauthorized")
                    || lower.contains("not authenticated")
                    || lower.contains("has not been authenticated")
            }
        }
    }

    /// What the tool said went wrong, with the one fix people need most often named.
    pub fn error(self, stderr: &str) -> String {
        let text = stderr.trim();
        if self.signed_out(text) {
            format!(
                "The {} isn't signed in: run `{}`, then try again.",
                self.cli(),
                self.sign_in()
            )
        } else {
            text.to_string()
        }
    }
}

/// The lower-cased host name of a remote URL, without its user and port; None when it names none (a local path).
fn host_name(url: &str) -> Option<String> {
    let url = url.trim();
    let authority = match url.split_once("://") {
        Some((_, rest)) => rest.split('/').next().unwrap_or_default(),
        // `git@host:group/project.git`: scp-like, the host before the first `:`, with no `/` before it.
        None => {
            let (host, _) = url.split_once(':')?;
            if host.contains('/') || host.contains('\\') || host.len() < 2 {
                return None;
            }
            host
        }
    };
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = match host.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_the_host_from_the_remotes_url() {
        for (url, host) in [
            ("https://github.com/nexus-xyz/nexus.git", Some(Host::GitHub)),
            ("git@github.com:nexus-xyz/nexus.git", Some(Host::GitHub)),
            ("https://gitlab.com/group/project.git", Some(Host::GitLab)),
            ("git@gitlab.com:group/sub/deeper/project.git", Some(Host::GitLab)),
            (
                "ssh://git@gitlab.example.com:2222/group/project.git",
                Some(Host::GitLab),
            ),
            ("https://oauth2@GitLab.Corp.Example/team/project", Some(Host::GitLab)),
            ("gitlab.internal:team/project.git", Some(Host::GitLab)),
            ("https://git.example.com/team/project.git", None),
            ("https://notgitlab.com/team/project.git", None),
            ("https://github.com.evil.example/team/project.git", None),
            ("/srv/git/project.git", None),
            ("C:\\repos\\project", None),
            ("../project", None),
        ] {
            assert_eq!(Host::of_url(url), host, "{url}");
        }
    }

    #[test]
    fn uses_the_hosts_own_words() {
        assert_eq!(Host::GitHub.reference(42), "#42");
        assert_eq!(Host::GitLab.reference(42), "!42");
        assert_eq!(Host::GitLab.request(), "merge request");
        assert_eq!(
            Host::GitHub.needs("This"),
            "This needs the GitHub CLI: install gh, run `gh auth login`, then try again."
        );
        assert_eq!(
            Host::GitLab.needs("Opening a merge request"),
            "Opening a merge request needs the GitLab CLI: install glab, run `glab auth login`, then try again."
        );
        assert!(
            Host::GitLab
                .error("ERROR: 401 Unauthorized")
                .contains("run `glab auth login`")
        );
        assert_eq!(Host::GitLab.error(" 404 Not Found \n"), "404 Not Found");
        let missing = Host::GitHub.needs("Opening a pull request");
        assert!(Host::GitHub.missing(&missing) && !Host::GitLab.missing(&missing));
        let signed_out = Host::GitHub.error("To get started with GitHub CLI, please run:  gh auth login");
        assert!(Host::GitHub.signed_out(&signed_out) && !Host::GitHub.missing(&signed_out));
        assert!(!Host::GitHub.signed_out("pull request create failed: no commits"));
        assert_eq!(serde_json::to_string(&Host::GitLab).unwrap_or_default(), "\"gitlab\"");
    }

    #[test]
    fn reads_the_host_of_a_repository_and_its_setting() {
        let root = std::env::temp_dir().join(format!("nexus-host-{}", uuid::Uuid::new_v4().simple()));
        let git = root.join(".git");
        std::fs::create_dir_all(&git).expect("git folder");
        let write = |text: &str| std::fs::write(git.join("config"), text).expect("config");
        write(
            "[remote \"upstream\"]\n\turl = https://github.com/a/b.git\n[remote \"origin\"]\n\turl = git@gitlab.com:a/b.git\n",
        );
        assert_eq!(Host::of_repo(&root), Host::GitLab);
        write("[remote \"origin\"]\n\turl = https://git.corp.example/a/b.git\n");
        assert_eq!(Host::of_repo(&root), Host::GitHub);
        write("[remote \"origin\"]\n\turl = https://git.corp.example/a/b.git\n[nexus]\n\thost = GitLab\n");
        assert_eq!(Host::of_repo(&root), Host::GitLab);
        assert_eq!(Host::of_repo(&root.join("absent")), Host::GitHub);
        let _ = std::fs::remove_dir_all(&root);
    }
}
