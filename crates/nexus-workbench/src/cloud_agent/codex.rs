//! Codex on a cloud machine: it runs there through Nexus, so it needs no Claude token, only GitHub to clone and push.

use nexus_agents::{Agent, Codex};

use super::{Check, CloudAgent, FirstRun, InstructionsFile};
use crate::event::Via;

impl CloudAgent for Codex {
    fn checks(&self) -> &'static [Check] {
        &[Check::Github]
    }

    /// A ChatGPT plan's sign-in can't go to a cloud machine.
    fn refused(&self, via: Via) -> Option<&'static str> {
        match via {
            Via::Subscription => Some(
                "Codex on Cloud runs on Team API. Your ChatGPT plan can't sign in on a cloud machine, so its sign-in \
                 stays on this computer.",
            ),
            Via::Nexus => None,
        }
    }

    /// `$CODEX_HOME/AGENTS.md`, else `~/.codex/AGENTS.md`.
    fn instructions(&self) -> Option<InstructionsFile> {
        let profile = &Agent::Codex.entry().profile;
        Some(InstructionsFile {
            var: profile.var,
            dir: profile.dir,
            name: profile.instructions?,
            report: "codexMd",
        })
    }

    /// None: a ChatGPT plan never goes to a cloud machine until Nexus can use Sign in with ChatGPT.
    fn token_var(&self) -> Option<&'static str> {
        Agent::Codex.entry().sign_in.token_var
    }

    fn first_run(&self) -> Option<FirstRun> {
        None
    }
}
