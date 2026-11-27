//! Claude Code on a cloud machine: it runs there on either route, with the person's Claude token from this computer.

use nexus_agents::{Agent, Claude};

use super::{Check, CloudAgent, FirstRun, InstructionsFile};
use crate::event::Via;

impl CloudAgent for Claude {
    fn checks(&self) -> &'static [Check] {
        &[Check::Claude, Check::Github]
    }

    fn refused(&self, _via: Via) -> Option<&'static str> {
        None
    }

    /// `~/.claude/CLAUDE.md`. The machine's setup does not honor `CLAUDE_CONFIG_DIR`: nothing sets it there.
    fn instructions(&self) -> Option<InstructionsFile> {
        let profile = &Agent::Claude.entry().profile;
        Some(InstructionsFile {
            var: None,
            dir: profile.dir,
            name: profile.instructions?,
            report: "claudeMd",
        })
    }

    fn token_var(&self) -> Option<&'static str> {
        Agent::Claude.entry().sign_in.token_var
    }

    /// `~/.claude.json`: onboarding done, so Claude Code starts without its welcome, and its own updates off.
    fn first_run(&self) -> Option<FirstRun> {
        Some(FirstRun {
            file: ".claude.json",
            marks: &[("hasCompletedOnboarding", true), ("autoUpdates", false)],
            report: "claude",
        })
    }
}
