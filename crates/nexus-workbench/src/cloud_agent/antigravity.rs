//! Antigravity never runs on a cloud machine: it runs on the Google AI plan only, whose sign-in stays on this computer.

use nexus_agents::Antigravity;

use super::{Check, CloudAgent, FirstRun, InstructionsFile};
use crate::event::Via;

impl CloudAgent for Antigravity {
    fn checks(&self) -> &'static [Check] {
        &[]
    }

    fn refused(&self, _via: Via) -> Option<&'static str> {
        Some(
            "Antigravity doesn't run on Cloud. It runs on your Google AI plan only, and that plan's sign-in stays on \
             this computer.",
        )
    }

    fn instructions(&self) -> Option<InstructionsFile> {
        None
    }

    fn token_var(&self) -> Option<&'static str> {
        None
    }

    fn first_run(&self) -> Option<FirstRun> {
        None
    }
}
