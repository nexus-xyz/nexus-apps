//! Where the workbench reports an agent that would not start, for the app's client errors. The workbench
//! sends nothing itself: the shell sets a reporter at start (`nexus_desktop_core::signals::install`), which queues
//! the error for the Gateway. Only names from this code leave: the agent and a fixed code, never the error's text or
//! a path.

use std::sync::OnceLock;

use crate::event::SessionAgent;

/// The shell's reporter: the place (`agent_spawn.claude`) and the code (`not_found`).
pub type Reporter = fn(component: &'static str, code: &'static str);

static REPORTER: OnceLock<Reporter> = OnceLock::new();

/// Sets the reporter. The first call wins.
pub fn report_to(reporter: Reporter) {
    let _ = REPORTER.set(reporter);
}

/// An agent did not start: its launch was refused, or its program could not run.
pub fn agent_failed(agent: SessionAgent, code: &'static str) {
    if let Some(reporter) = REPORTER.get() {
        reporter(component(agent), code);
    }
}

fn component(agent: SessionAgent) -> &'static str {
    crate::harness::harness(agent).fault_component()
}

/// A failed spawn's code, by the kind of the OS error.
pub fn io_code(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "not_found",
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        _ => "spawn_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_agent_and_a_fixed_code() {
        assert_eq!(component(SessionAgent::Codex), "agent_spawn.codex");
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(io_code(&missing), "not_found");
        let other = std::io::Error::other("text that never leaves");
        assert_eq!(io_code(&other), "spawn_failed");
    }
}
