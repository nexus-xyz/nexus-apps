//! Each session agent's cloud rules, one file per agent: whether it runs on a cloud machine and on which route, which
//! sign-ins a new machine needs from this computer for it, where it reads the app's instructions block there, the
//! variable its sign-in token goes in, and the first-run marks it needs there. [`CloudAgent`] is this crate's domain
//! trait, as [`crate::harness::Harness`] is: each coding agent's type in `nexus_agents` implements it here, and Nexus's own agent implements it on
//! [`crate::harness::Nexus`]. [`cloud_agent`] is the one dispatch.
//!
//! The cloud machine's SSH setup script, nexus-host's `host_provision` and the app's Cloud screens read
//! it, so an agent that reaches cloud machines adds its file here instead of a match in each. Paths, file names and
//! token names come from the agent's `nexus_agents` entry.

use crate::event::{SessionAgent, SessionKind, Via};
use crate::harness::Nexus;

mod antigravity;
mod claude;
mod codex;
mod cursor;
mod nexus;
mod opencode;

/// Settings › Cloud's steps: the sign-ins a new machine gets from this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Claude,
    Github,
}

impl Check {
    pub fn name(self) -> &'static str {
        match self {
            Check::Claude => "Claude",
            Check::Github => "GitHub",
        }
    }
}

/// Where an agent reads the app's block of instructions on a cloud machine: `<folder>/<name>`, the folder being
/// `$<var>` when the variable is set and not empty, else `~/<dir>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstructionsFile {
    /// The variable that moves the folder, when the machine's setup honors it.
    pub var: Option<&'static str>,
    /// The folder under the home folder.
    pub dir: &'static str,
    /// The file's name in it.
    pub name: &'static str,
    /// The setup's report field for this file (`claudeMd`).
    pub report: &'static str,
}

/// The JSON file where an agent keeps its first-run state on a cloud machine, `~/<file>`, and the marks the machine's
/// setup sets in it so the agent starts without its first-run screens. The rest of the file is the person's and stays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstRun {
    /// The file under the home folder (`.claude.json`).
    pub file: &'static str,
    /// Each top-level field and the value it must hold. Field names are plain identifiers: the setup script writes
    /// them into Python and JavaScript as they are.
    pub marks: &'static [(&'static str, bool)],
    /// The setup's report field for this file (`claude`).
    pub report: &'static str,
}

/// The trait every session agent implements, in `src/cloud_agent/<agent>.rs`. No method has a default body: a new
/// agent does not build until its file says each rule.
pub trait CloudAgent: Sync {
    /// The steps a cloud session with this agent needs from this computer (`checksFor`).
    fn checks(&self) -> &'static [Check];
    /// Why a new cloud session with this agent paid by `via` can't start; None when it can (`cloudPlanRefused`).
    fn refused(&self, via: Via) -> Option<&'static str>;
    /// The file the app's instructions block goes in on a cloud machine; None: the agent gets none.
    fn instructions(&self) -> Option<InstructionsFile>;
    /// The variable in the machine's `agent.env` that its sign-in token from this computer goes in; None: none goes.
    fn token_var(&self) -> Option<&'static str>;
    /// The first-run marks a cloud machine's setup sets for the agent; None: the agent needs none.
    fn first_run(&self) -> Option<FirstRun>;
}

/// The agent's [`CloudAgent`], by its kind: a coding agent's through `nexus_agents::of!`, and Nexus's own.
pub fn cloud_agent(agent: SessionAgent) -> &'static dyn CloudAgent {
    match agent.kind() {
        SessionKind::Coding(coding) => coding_cloud_agent(coding),
        SessionKind::Nexus => &Nexus,
    }
}

/// A coding agent's [`CloudAgent`], whether or not this build runs its sessions.
pub fn coding_cloud_agent(agent: nexus_agents::Agent) -> &'static dyn CloudAgent {
    nexus_agents::of!(agent, dyn CloudAgent)
}

/// Every instructions file a cloud machine's setup writes, in the order it writes and reports them: the coding agents
/// in `nexus_agents::Agent::ALL`'s order. Nexus's own agent has none.
pub fn instructions_files() -> Vec<InstructionsFile> {
    nexus_agents::Agent::ALL
        .into_iter()
        .filter_map(|agent| coding_cloud_agent(agent).instructions())
        .collect()
}

/// Every first-run file a cloud machine's setup marks, in the order it marks and reports them: the coding agents in
/// `nexus_agents::Agent::ALL`'s order. Nexus's own agent has none.
pub fn first_run_files() -> Vec<FirstRun> {
    nexus_agents::Agent::ALL
        .into_iter()
        .filter_map(|agent| coding_cloud_agent(agent).first_run())
        .collect()
}

/// Every variable of `agent.env` that holds an agent's sign-in token ([`CloudAgent::token_var`]).
pub fn token_vars() -> Vec<&'static str> {
    nexus_agents::Agent::ALL
        .into_iter()
        .filter_map(|agent| coding_cloud_agent(agent).token_var())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_agents::Agent;

    const CODEX_ON_A_PLAN: &str = "Codex on Cloud runs on Team API. Your ChatGPT plan can't sign in on a cloud machine, \
                                   so its sign-in stays on this computer.";
    const ANTIGRAVITY: &str = "Antigravity doesn't run on Cloud. It runs on your Google AI plan only, and that plan's sign-in stays on this \
         computer.";

    const OPENCODE_ON_A_PLAN: &str =
        "OpenCode on Cloud runs on Team API. Switch to Team API or run OpenCode on this computer.";

    /// Every agent's cloud rules, as they were when they were matches in three crates.
    #[test]
    fn each_agents_cloud_rules() {
        use Check::{Claude, Github};
        let claude = coding_cloud_agent(Agent::Claude);
        let codex = coding_cloud_agent(Agent::Codex);
        let antigravity = coding_cloud_agent(Agent::Antigravity);
        let cursor = coding_cloud_agent(Agent::Cursor);
        let opencode = coding_cloud_agent(Agent::Opencode);
        let nexus = cloud_agent(SessionAgent::Nexus);

        assert_eq!(claude.checks(), &[Claude, Github]);
        assert_eq!(nexus.checks(), &[Claude, Github]);
        assert_eq!(codex.checks(), &[Github]);
        assert_eq!(opencode.checks(), &[Github]);
        for agent in [antigravity, cursor] {
            assert!(agent.checks().is_empty());
        }

        for via in [Via::Nexus, Via::Subscription] {
            assert_eq!(claude.refused(via), None);
            assert_eq!(nexus.refused(via), None);
            assert_eq!(antigravity.refused(via), Some(ANTIGRAVITY));
            assert_eq!(cursor.refused(via), Some("Cursor runs on this computer only."));
        }
        assert_eq!(codex.refused(Via::Nexus), None);
        assert_eq!(codex.refused(Via::Subscription), Some(CODEX_ON_A_PLAN));
        assert_eq!(opencode.refused(Via::Nexus), None);
        assert_eq!(opencode.refused(Via::Subscription), Some(OPENCODE_ON_A_PLAN));

        assert_eq!(
            instructions_files(),
            vec![
                InstructionsFile {
                    var: None,
                    dir: ".claude",
                    name: "CLAUDE.md",
                    report: "claudeMd",
                },
                InstructionsFile {
                    var: Some("CODEX_HOME"),
                    dir: ".codex",
                    name: "AGENTS.md",
                    report: "codexMd",
                },
            ]
        );
        assert_eq!(nexus.instructions(), None);

        assert_eq!(claude.token_var(), Some("CLAUDE_CODE_OAUTH_TOKEN"));
        for agent in [codex, antigravity, cursor, opencode, nexus] {
            assert_eq!(agent.token_var(), None);
        }
        assert_eq!(token_vars(), vec!["CLAUDE_CODE_OAUTH_TOKEN"]);

        assert_eq!(
            first_run_files(),
            vec![FirstRun {
                file: ".claude.json",
                marks: &[("hasCompletedOnboarding", true), ("autoUpdates", false)],
                report: "claude",
            }]
        );
        assert_eq!(nexus.first_run(), None);
        for file in first_run_files() {
            assert!(!file.marks.is_empty(), "{file:?}");
            for (field, _) in file.marks {
                let plain = field.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                    && field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                assert!(plain, "{field} is not a plain identifier");
            }
        }
        assert_eq!(Claude.name(), "Claude");
        assert_eq!(Github.name(), "GitHub");
    }

    /// The session agents reach the same rules as their coding agents.
    #[test]
    fn session_agents_dispatch_to_their_coding_agents() {
        for agent in [SessionAgent::Claude, SessionAgent::Codex, SessionAgent::Antigravity] {
            let coding = agent.coding().unwrap_or_else(|| panic!("{agent:?} is a coding agent"));
            assert_eq!(cloud_agent(agent).checks(), coding_cloud_agent(coding).checks());
            assert_eq!(
                cloud_agent(agent).refused(Via::Subscription),
                coding_cloud_agent(coding).refused(Via::Subscription)
            );
        }
    }

    #[test]
    fn cloud_agent_trait_has_no_default_bodies() {
        let source = include_str!("mod.rs");
        let start = source
            .find("pub trait CloudAgent: Sync {")
            .unwrap_or_else(|| panic!("no CloudAgent trait"));
        let body = &source[start..];
        let end = body.find("\n}\n").unwrap_or_else(|| panic!("the trait does not close"));
        let body = &body[body.find('{').map_or(0, |at| at + 1)..end];
        for line in body.lines().map(str::trim).filter(|line| !line.starts_with("//")) {
            assert!(!line.contains('{'), "a CloudAgent method has a default body: {line}");
        }
        assert_eq!(body.matches("fn ").count(), 5, "the trait's methods were not read");
    }
}
