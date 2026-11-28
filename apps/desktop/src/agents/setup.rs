//! A coding agent that is not installed or not signed in, as a setup line (`crate::setup`, board P3 of "Desktop ·
//! Setup that blocks an action"). Install and the agent's own sign-in run in the Workbench terminal, where the person
//! sees them; an agent that Nexus signs in itself (Claude, Codex) uses the browser sign-in of Settings › Coding agents.
//! "Coding agents ›" opens that agent's section. Never red: this is setup, not a failure.

use gpui_kit::assets::IconName as Lucide;
use nexus_workbench::event::SessionAgent;

use crate::pages::PageId;
use crate::pages::subscriptions::login::Mode;
use crate::setup::{Fix, FixKey, SetupLine, SignIn, TerminalStep};

/// What an agent lacks, read from an error's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gap {
    /// Not installed. `here`: on this computer, where the Workbench can install it.
    Missing { here: bool },
    /// Not signed in to its own account.
    SignedOut,
}

/// What an agent lacks, when `error` says it: the launch's "… is not installed on this computer. Install it with …",
/// nexus-host's "… on this machine", and the agent's own "… is not signed in (…)" (`acp.rs`) or "… is not signed in.
/// Run `agy` …". A server's Nexus sign-in ("This machine is not signed in to Nexus") is not the agent's.
pub(crate) fn gap(error: &str) -> Option<Gap> {
    if error.contains(" is not installed on this computer") {
        Some(Gap::Missing { here: true })
    } else if error.contains(" is not installed on this machine") {
        Some(Gap::Missing { here: false })
    } else if error.contains(" is not signed in (") || error.contains(" is not signed in. Run ") {
        Some(Gap::SignedOut)
    } else {
        None
    }
}

/// The anchor of the agent's section in Settings › Coding agents (`pages/agents.rs`).
pub(crate) fn anchor(agent: SessionAgent) -> &'static str {
    match agent {
        SessionAgent::Claude => "agent-claude",
        SessionAgent::Codex => "agent-codex",
        SessionAgent::Nexus => "agent-nexus",
        SessionAgent::Antigravity => "agent-antigravity",
        SessionAgent::Cursor => "agent-cursor",
        SessionAgent::Opencode => "agent-opencode",
        SessionAgent::Grok => "agent-grok",
    }
}

/// The vendor's install command, for a coding agent.
pub(crate) fn install_command(agent: SessionAgent) -> Option<&'static str> {
    agent.coding().map(|coding| coding.entry().install.command)
}

/// The agent's own sign-in command, run in a terminal (`agy`, `codex login`).
pub(crate) fn login_command(agent: SessionAgent) -> Option<&'static str> {
    agent.coding().map(|coding| coding.entry().sign_in.login)
}

/// Whether Nexus signs the agent in itself, from Settings › Coding agents (`connect_on_open`).
pub(crate) fn signs_in_here(agent: SessionAgent) -> bool {
    super::ui(agent).login_args(None, Mode::Browser).is_ok()
}

/// "Coding agents ›": the agent's section in Settings.
pub(crate) fn open_agent(agent: SessionAgent) -> Fix {
    Fix::open_at(PageId::Agents, anchor(agent))
}

/// Install `command` in the Workbench terminal, in a tab named after the agent.
pub(crate) fn install_fix(agent: SessionAgent, command: impl Into<String>) -> Fix {
    let mut step = TerminalStep::new(command);
    step.name = Some(format!("Install {}", nexus_workbench::driver::label(agent)));
    Fix::RunInTerminal(step)
}

/// The agent's sign-in: Nexus's browser sign-in where it has one, else its own command in the Workbench terminal.
pub(crate) fn sign_in_fix(agent: SessionAgent) -> Option<Fix> {
    if signs_in_here(agent) {
        return Some(Fix::SignIn(SignIn::Agent(agent)));
    }
    let mut step = TerminalStep::new(login_command(agent)?);
    step.name = Some(format!("Sign in to {}", nexus_workbench::driver::label(agent)));
    Some(Fix::RunInTerminal(step))
}

/// "Codex isn't installed on this computer.", with Install when the Workbench can run `install`, and "Coding agents ›"
/// when `settings` (the agent is this computer's). The command shows under the line with Copy
/// (`workspace/agent_setup.rs`).
pub(crate) fn missing_line(
    key: impl Into<FixKey>,
    agent: SessionAgent,
    place: &str,
    install: Option<&str>,
    settings: bool,
) -> SetupLine {
    let name = nexus_workbench::driver::label(agent);
    let mut line = SetupLine::new(
        key,
        Lucide::SquareTerminal,
        format!("{name} isn't installed on {place}."),
    );
    match install {
        Some(command) => line = line.fix("Install", install_fix(agent, command)),
        None => line = line.why("Install it there, then try again."),
    }
    if settings {
        line = line.fix("Coding agents ›", open_agent(agent));
    }
    line
}

/// "Antigravity isn't signed in.", with `sign_in` (the agent's own, [`sign_in_fix`], when `None`), then "Coding
/// agents ›" when `settings` and Sign in does not open them already.
pub(crate) fn signed_out_line(
    key: impl Into<FixKey>,
    agent: SessionAgent,
    what: String,
    sign_in: Option<Fix>,
    settings: bool,
) -> SetupLine {
    let here = signs_in_here(agent);
    let mut line = SetupLine::new(key, Lucide::KeyRound, what).why(if here {
        "Sign-in opens your browser. Nexus never sees the password."
    } else {
        "Sign-in runs in the terminal. Nexus never sees the password."
    });
    if let Some(fix) = sign_in.or_else(|| sign_in_fix(agent)) {
        line = line.fix("Sign in", fix);
    }
    if settings && !here {
        line = line.fix("Coding agents ›", open_agent(agent));
    }
    line
}

/// Whether a press of `fix` starts something whose end resumes the blocked action: a terminal step or a sign-in.
pub(crate) fn resumes(fix: &Fix) -> bool {
    matches!(fix, Fix::RunInTerminal(_) | Fix::SignIn(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_errors_name_what_the_agent_lacks() {
        assert_eq!(
            gap("Codex is not installed on this computer. Install it with `npm i -g @openai/codex`, then try again."),
            Some(Gap::Missing { here: true })
        );
        assert_eq!(
            gap("Codex is not installed on this machine. Install it with `npm i -g @openai/codex`, then try again."),
            Some(Gap::Missing { here: false })
        );
        assert_eq!(
            gap("Antigravity is not signed in. Run `agy` to sign in."),
            Some(Gap::SignedOut)
        );
        assert_eq!(
            gap("Cursor is not signed in (auth required). Run `cursor-agent login` in a terminal, then send again."),
            Some(Gap::SignedOut)
        );
        // A server's Nexus sign-in is P4's, and anything else stays an error.
        assert_eq!(
            gap("This machine is not signed in to Nexus, so Codex can't start here."),
            None
        );
        assert_eq!(gap("Codex could not start: the folder is gone."), None);
    }

    #[test]
    fn claude_and_codex_sign_in_here_and_the_others_in_the_terminal() {
        assert!(matches!(
            sign_in_fix(SessionAgent::Codex),
            Some(Fix::SignIn(SignIn::Agent(SessionAgent::Codex)))
        ));
        assert!(matches!(
            sign_in_fix(SessionAgent::Claude),
            Some(Fix::SignIn(SignIn::Agent(SessionAgent::Claude)))
        ));
        let Some(Fix::RunInTerminal(step)) = sign_in_fix(SessionAgent::Antigravity) else {
            panic!("agy signs in in the terminal");
        };
        assert_eq!(step.command, "agy");
        assert_eq!(step.name.as_deref(), Some("Sign in to Antigravity"));
        let Some(Fix::RunInTerminal(step)) = sign_in_fix(SessionAgent::Cursor) else {
            panic!("cursor-agent signs in in the terminal");
        };
        assert_eq!(step.command, "cursor-agent login");
        // Nexus's own agent signs in with Nexus.
        assert!(sign_in_fix(SessionAgent::Nexus).is_none());
    }

    #[test]
    fn install_runs_the_vendor_command_in_a_named_tab() {
        let command = install_command(SessionAgent::Codex).expect("Codex installs with its vendor's command");
        let Fix::RunInTerminal(step) = install_fix(SessionAgent::Codex, command) else {
            panic!("install runs in the terminal");
        };
        assert_eq!(step.command, command);
        assert_eq!(step.name.as_deref(), Some("Install Codex"));
        assert_eq!(step.cwd, None);
        assert!(install_command(SessionAgent::Nexus).is_none());
    }

    #[test]
    fn every_agent_has_its_own_anchor() {
        let anchors: std::collections::HashSet<_> = SessionAgent::ALL.into_iter().map(anchor).collect();
        assert_eq!(anchors.len(), SessionAgent::ALL.len());
        assert!(matches!(
            open_agent(SessionAgent::Cursor),
            Fix::Open {
                page: PageId::Agents,
                focus: Some("agent-cursor"),
                ..
            }
        ));
    }
}
