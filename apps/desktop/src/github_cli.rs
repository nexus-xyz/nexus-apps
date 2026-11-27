//! The GitHub CLI missing or signed out, as a setup line: the pull request page and the pull request form say what is
//! missing and run its fix in the Workbench terminal, where the person sees `gh`'s one-time code. Board P8 of the
//! "Desktop · Setup that blocks an action" design. Any other failure of a `gh` call stays a failure ([`failed`]).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::{App, ClipboardItem, Window};
use nexus_pull_requests::pull_page::{self, ErrorKind, PullError};
use nexus_workbench::source_host::Host;

use crate::setup::{Fix, FixKey, SetupLine, TerminalStep, Tone};

/// The official install guide, for a computer whose package manager is not known here.
pub const GUIDE: &str = "https://github.com/cli/cli#installation";
/// The browser sign-in: `gh` prints a one-time code in the terminal and opens github.com.
const SIGN_IN: &str = "gh auth login --web";

/// How this computer installs the GitHub CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// A command the Workbench terminal runs.
    Run(&'static str),
    /// No package manager known here: the official guide.
    Guide,
}

/// What blocks a `gh` call that setup fixes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Missing(Install),
    SignedOut,
}

impl Block {
    /// The core's words for a failed `gh` call (`Host::needs`, `Host::error`); None for any other failure. It looks for
    /// a package manager when `gh` is missing: call it where the failure arrives, not in `render`.
    pub fn of_text(text: &str) -> Option<Self> {
        if Host::GitHub.missing(text) {
            Some(Block::Missing(install()))
        } else if Host::GitHub.signed_out(text) {
            Some(Block::SignedOut)
        } else {
            None
        }
    }

    /// The pull request page's failure; None for any other. As [`Block::of_text`], not in `render`.
    pub fn of_pull(error: &PullError) -> Option<Self> {
        match error.kind {
            ErrorKind::SignedOut => Some(Block::SignedOut),
            // A GitLab repository is refused with this kind too: it needs no GitHub CLI.
            ErrorKind::Unavailable if error.message != pull_page::GITLAB => Some(Block::Missing(install())),
            _ => None,
        }
    }

    /// The fix's key in `scope` (`"pr"`): its run state, and what resumes when it is done.
    pub fn key(&self, scope: &str) -> FixKey {
        match self {
            Block::Missing(_) => format!("{scope}:gh-install").into(),
            Block::SignedOut => format!("{scope}:gh-auth").into(),
        }
    }

    /// Whether the line's fix runs here, rather than on the web.
    pub fn runs_here(&self) -> bool {
        !matches!(self, Block::Missing(Install::Guide))
    }

    /// The line in `scope`; `doing` ends its first sentence: "Sign in to GitHub to see this pull request."
    pub fn line(&self, scope: &str, doing: &str) -> SetupLine {
        let key = self.key(scope);
        match self {
            Block::SignedOut => SetupLine::new(key, Lucide::Github, format!("Sign in to GitHub to {doing}."))
                .why("Nexus uses the GitHub CLI on this computer.")
                .fix("Sign in to GitHub", step(SIGN_IN, "GitHub sign-in")),
            Block::Missing(install) => {
                let line = SetupLine::new(key, Lucide::Github, format!("Install the GitHub CLI to {doing}."));
                match install {
                    Install::Run(command) => line
                        .why("Nexus uses it to talk to GitHub.")
                        .fix("Install GitHub CLI", step(command, "GitHub CLI install")),
                    Install::Guide => line
                        .why("Install it, then try again.")
                        .guide("Install GitHub CLI", GUIDE),
                }
            }
        }
    }
}

fn step(command: &str, name: &str) -> Fix {
    let mut step = TerminalStep::new(command);
    step.name = Some(name.into());
    Fix::RunInTerminal(step)
}

/// How this computer installs the GitHub CLI, by the package managers on the `PATH` agents get.
pub fn install() -> Install {
    let path = crate::code_run::agent_env().remove("PATH").unwrap_or_default();
    install_for(std::env::consts::OS, |program| {
        nexus_workbench::clone::on_path(program, &path)
    })
}

/// [`install`] on `os` (`std::env::consts::OS`), where `has` says whether a program is installed.
pub fn install_for(os: &str, has: impl Fn(&str) -> bool) -> Install {
    let command = match os {
        "macos" => has("brew").then_some("brew install gh"),
        "windows" => Some("winget install --id GitHub.cli"),
        _ => [
            ("dnf", "sudo dnf install -y gh"),
            ("apt", "sudo apt install -y gh"),
            ("pacman", "sudo pacman -S --noconfirm github-cli"),
        ]
        .into_iter()
        .find(|(manager, _)| has(manager))
        .map(|(_, command)| command),
    };
    command.map_or(Install::Guide, Install::Run)
}

/// Any other failure: the red ✕, `title`, the failure's first line, Try again (`retry`) and Copy (the whole text).
pub fn failed(
    key: impl Into<FixKey>,
    title: &str,
    text: &str,
    retry: impl Fn(&mut Window, &mut App) + 'static,
) -> SetupLine {
    let all = text.to_string();
    SetupLine::new(key, Lucide::X, title.to_string())
        .tone(Tone::Failed)
        .why(first_line(text))
        .fix("Try again", Fix::act(retry))
        .fix(
            "Copy",
            Fix::act(move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(all.clone()))),
        )
}

/// The first line with words on it.
pub fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("GitHub gave no reason.")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn installs_with_the_computers_package_manager() {
        let with = |programs: &'static [&'static str]| move |program: &str| programs.contains(&program);
        assert_eq!(install_for("macos", with(&["brew"])), Install::Run("brew install gh"));
        assert_eq!(install_for("macos", with(&[])), Install::Guide);
        assert_eq!(
            install_for("windows", with(&[])),
            Install::Run("winget install --id GitHub.cli")
        );
        assert_eq!(
            install_for("linux", with(&["dnf"])),
            Install::Run("sudo dnf install -y gh")
        );
        assert_eq!(
            install_for("linux", with(&["apt"])),
            Install::Run("sudo apt install -y gh")
        );
        assert_eq!(
            install_for("linux", with(&["pacman"])),
            Install::Run("sudo pacman -S --noconfirm github-cli")
        );
        assert_eq!(install_for("linux", with(&["zypper"])), Install::Guide);
    }

    #[test]
    fn tells_a_missing_or_signed_out_cli_from_any_other_failure() {
        let missing = Host::GitHub.needs("Opening a pull request");
        assert!(matches!(Block::of_text(&missing), Some(Block::Missing(_))));
        let signed_out = Host::GitHub.error("To get started with GitHub CLI, please run:  gh auth login");
        assert_eq!(Block::of_text(&signed_out), Some(Block::SignedOut));
        assert_eq!(Block::of_text("! [rejected] main -> main (fetch first)"), None);

        let page = |kind, message: &str| Block::of_pull(&PullError::new(kind, message));
        assert_eq!(page(ErrorKind::SignedOut, "x"), Some(Block::SignedOut));
        assert!(matches!(
            page(ErrorKind::Unavailable, nexus_pull_requests::pull_request::UNAVAILABLE),
            Some(Block::Missing(_))
        ));
        assert_eq!(page(ErrorKind::Unavailable, pull_page::GITLAB), None);
        assert_eq!(page(ErrorKind::Github, "HTTP 502"), None);
    }

    #[test]
    fn keys_each_fix_by_its_place() {
        assert_eq!(Block::SignedOut.key("pr").as_ref(), "pr:gh-auth");
        assert_eq!(
            Block::Missing(Install::Guide).key("pr-form").as_ref(),
            "pr-form:gh-install"
        );
        assert!(!Block::Missing(Install::Guide).runs_here());
        assert!(Block::Missing(Install::Run("brew install gh")).runs_here());
        assert_eq!(first_line("\n  error: HTTP 502\nmore"), "error: HTTP 502");
        assert_eq!(first_line(""), "GitHub gave no reason.");
    }

    #[test]
    fn the_sign_in_runs_in_a_named_terminal() {
        let Fix::RunInTerminal(sign_in) = step(SIGN_IN, "GitHub sign-in") else {
            panic!("a terminal step");
        };
        assert_eq!(sign_in.command, "gh auth login --web");
        assert_eq!(sign_in.name.as_deref(), Some("GitHub sign-in"));
    }
}
