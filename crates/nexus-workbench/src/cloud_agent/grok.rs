//! Grok never runs on a cloud machine: it runs on this computer only.

use nexus_agents::Grok;

use super::{Check, CloudAgent, FirstRun, InstructionsFile};
use crate::event::Via;

impl CloudAgent for Grok {
    fn checks(&self) -> &'static [Check] {
        &[]
    }

    fn refused(&self, _via: Via) -> Option<&'static str> {
        Some("Grok runs on this computer only.")
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
