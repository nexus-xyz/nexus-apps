//! The variables a stdio MCP server never gets from Claude Code when it starts through Nexus's launcher
//! ([`crate::mcp_launch`]). Claude Code starts its stdio servers with its whole environment, and Nexus puts the
//! person's Nexus token and the run's credentials there. The list holds Nexus's own credential names and the names
//! Nexus sets, or removes, for Claude Code, so the person's own variables still reach their servers. That includes
//! what a host adds from the person's own files, such as nexus-host's `agent.env` (`GH_TOKEN`). A server's own declared
//! values are added after the removal, so a server still gets a value it declares under one of these names.

use nexus_agents::{AgentT, Claude};

/// Nexus's own run variables, by prefix: the app socket's address and credential, the built-in server's Gateway and
/// token, the remote servers' header values (`mcp_servers`), and the browser server's socket and credential.
const NEXUS_PREFIXES: [&str; 2] = ["NEXUS_MCP_", "NEXUS_BROWSER_"];

/// Credential names outside Claude Code's catalogue entry: Nexus's keys for its other agents, which hold the Nexus token
/// wherever they are set, and what Nexus or Claude Code sets for a Claude Code run.
const NAMES: [&str; 5] = [
    // Nexus's key for Codex and Nexus's own agent (nexus-local-agents' `CODEX_KEY_ENV`, `nexus::launch`).
    nexus_local_agents::agents::CODEX_KEY_ENV,
    // Nexus's key for OpenCode.
    nexus_local_agents::agents::OPENCODE_KEY_ENV,
    // On a plan, `Authorization=Bearer <Nexus token>` for Claude Code's usage logs (nexus-local-agents' `on_plan`).
    "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
    // Claude Code's own credential for its messaging socket, which it sets itself.
    "CLAUDE_CODE_MESSAGING_TOKEN",
    // The orchestration server's bearer token (`orchestra::inject`).
    crate::orchestra::TOKEN_VAR,
];

/// Whether a server started through the launcher for Claude Code loses the variable `name`. Windows compares names
/// without case, as its environment does.
pub fn stripped(name: &str) -> bool {
    let entry = Claude.entry();
    let same = |listed: &str| {
        if cfg!(windows) {
            listed.eq_ignore_ascii_case(name)
        } else {
            listed == name
        }
    };
    // The prefixes are upper case.
    let prefixed = |prefix: &str| {
        if cfg!(windows) {
            name.to_ascii_uppercase().starts_with(prefix)
        } else {
            name.starts_with(prefix)
        }
    };
    // What would send Claude Code past Nexus, which Nexus removes or sets on every route, and its plan's token.
    entry.billing.plan_overrides.iter().any(|listed| same(listed))
        || entry.sign_in.token_var.is_some_and(same)
        || NAMES.iter().any(|listed| same(listed))
        || NEXUS_PREFIXES.iter().any(|prefix| prefixed(prefix))
}

#[cfg(test)]
#[path = "credential_env_tests.rs"]
mod tests;
