//! Nexus's own agent on a cloud machine: it runs there on either route and needs what Claude Code needs. It reads no
//! instructions file of its own, and its sign-in is the person's Nexus one.

use super::{Check, CloudAgent, FirstRun, InstructionsFile};
use crate::event::Via;
use crate::harness::Nexus;

impl CloudAgent for Nexus {
    fn checks(&self) -> &'static [Check] {
        &[Check::Claude, Check::Github]
    }

    fn refused(&self, _via: Via) -> Option<&'static str> {
        None
    }

    fn instructions(&self) -> Option<InstructionsFile> {
        None
    }

    fn token_var(&self) -> Option<&'static str> {
        None
    }

    /// None: Claude Code's marks come with Claude Code's own file.
    fn first_run(&self) -> Option<FirstRun> {
        None
    }
}
