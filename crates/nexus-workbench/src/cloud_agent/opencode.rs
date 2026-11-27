//! OpenCode on a cloud machine: it runs there through Nexus credits, so it needs GitHub to clone and push.

use nexus_agents::Opencode;

use super::{Check, CloudAgent, FirstRun, InstructionsFile};
use crate::event::Via;

impl CloudAgent for Opencode {
    fn checks(&self) -> &'static [Check] {
        &[Check::Github]
    }

    /// OpenCode on Cloud runs on Nexus credits (Team API), not a local configured provider.
    fn refused(&self, via: Via) -> Option<&'static str> {
        match via {
            Via::Subscription => {
                Some("OpenCode on Cloud runs on Team API. Switch to Team API or run OpenCode on this computer.")
            }
            Via::Nexus => None,
        }
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
