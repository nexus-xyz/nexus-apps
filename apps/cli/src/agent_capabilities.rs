//! What each coding agent can do, computed from the code that does it: its entry (`nexus_agents`), its launch on
//! this machine (`LocalAgentT`, through `agent_launch`) and this CLI's usage readers. No value here is declared per
//! agent. It lives in this crate because the conversation-file reader (`antigravity_usage`) is this crate's.
//!
//! `kit/fixtures/agent-capabilities.json` holds these values with the entry's facts that kit's `AGENT_CATALOG`
//! copies. A test here writes it, and `kit/src/agent-capabilities.test.ts` holds the catalog to it.

use std::collections::HashMap;

use nexus_client::agent_entries::Agent;
use nexus_client::agents::{LaunchOptions, Route, agent_launch};
use nexus_client::local_agents::agents::local;

use crate::antigravity_usage;

/// How a run on the person's plan reports its calls' usage to its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageSource {
    /// The agent exports each call as an OpenTelemetry log to the session's usage endpoint while it runs.
    Otel,
    /// The CLI reads the run's calls from the agent's conversation file after it exits (`antigravity_usage`).
    ConversationDb,
}

impl UsageSource {
    pub fn as_str(self) -> &'static str {
        match self {
            UsageSource::Otel => "otel",
            UsageSource::ConversationDb => "conversation_db",
        }
    }
}

/// An agent's capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// The provider Gateway calls for it: the entry's `billing.gateway`. `None` when it runs only on the person's plan.
    pub gateway: Option<&'static str>,
    /// How a run on the person's plan reports its usage; `None` when it reports none.
    pub usage_reporting: Option<UsageSource>,
    /// Whether its runs keep a session log: it writes a transcript that this CLI's readers read
    /// ([`crate::session_log::workbench_agent`]).
    pub session_log: bool,
    /// Whether its launcher starts a conversation on a prompt given as its first argument
    /// (`LocalAgentT::takes_prompt_argument`).
    pub prompt_argument: bool,
}

/// `agent`'s capabilities.
pub fn capabilities(agent: Agent) -> Capabilities {
    Capabilities {
        gateway: agent.entry().billing.gateway.map(|binding| binding.provider),
        usage_reporting: usage_reporting(agent),
        session_log: crate::session_log::workbench_agent(agent).is_some(),
        prompt_argument: local(agent).takes_prompt_argument(),
    }
}

/// The agents whose capabilities `has` says yes to, by id, joined by `sep` (`claude|codex`), in the catalog's order.
pub fn agent_ids(has: impl Fn(&Capabilities) -> bool, sep: &str) -> String {
    Agent::ALL
        .into_iter()
        .filter(|&agent| has(&capabilities(agent)))
        .map(Agent::as_str)
        .collect::<Vec<_>>()
        .join(sep)
}

/// The conversation-file reader when the run uses it ([`antigravity_usage::counts_from_conversation`]), else
/// OpenTelemetry when the agent's launch exports to the session ([`exports_to_session`]).
fn usage_reporting(agent: Agent) -> Option<UsageSource> {
    if antigravity_usage::counts_from_conversation(agent) {
        return Some(UsageSource::ConversationDb);
    }
    exports_to_session(agent).then_some(UsageSource::Otel)
}

/// Whether the agent's launch on the person's plan points an OpenTelemetry exporter at the session's usage endpoint.
/// It launches with a stand-in session and an empty environment, as `agent_launch` does for `nexus <agent>`, and looks
/// for that endpoint in an `OTEL_*` variable (Claude Code's `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`) or an `otel.*`
/// override (Codex's `-c otel.exporter=`). The empty environment leaves out Claude Code's opt-out
/// (`CLAUDE_NO_USAGE_ENV`), so this is what a run does by default.
fn exports_to_session(agent: Agent) -> bool {
    const CORE: &str = "https://gateway.invalid";
    const SESSION: &str = "ags_capabilities";
    let usage = format!("{CORE}/v1/agents/sessions/{SESSION}/usage");
    let (args, env) = agent_launch(
        agent,
        LaunchOptions {
            route: Route::Subscription,
            core_url: CORE,
            api_key: "nx_capabilities",
            session_id: SESSION,
            team: None,
            model: None,
            model_catalog: None,
            web_search: false,
            args: Vec::new(),
            env: HashMap::new(),
        },
    );
    let in_env = env
        .iter()
        .any(|(name, value)| name.starts_with("OTEL_") && *value == usage);
    let in_args = args.iter().any(|arg| arg.starts_with("otel.") && arg.contains(&usage));
    in_env || in_args
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{Value, json};

    use super::*;

    /// The variable that rewrites the fixture instead of comparing it.
    const BLESS: &str = "AGENT_CAPABILITIES_BLESS";

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/kit/fixtures/agent-capabilities.json")
    }

    /// One agent as the fixture holds it: its offline answer as the Rust clients build it (`agent_catalog`, the shape
    /// of kit's `AgentAccessEntry`), the entry's facts that kit's `AgentCatalogEntry` adds, and its capabilities.
    fn row(agent: Agent, access: Value) -> Value {
        let entry = agent.entry();
        let capabilities = capabilities(agent);
        // kit keeps the arguments before the id (`--resume`); the entry writes the id's place (`--resume {id}`).
        let resume = entry
            .session
            .resume
            .strip_suffix(" {id}")
            .expect("the entry's resume arguments end with the id");
        json!({
            "access": access,
            "executable": entry.identity.executable,
            "cliLauncher": entry.session.has_cli_launcher(),
            "configuredProvider": entry.billing.configured_provider,
            "logout": entry.sign_in.logout,
            "resume": resume,
            "capabilities": {
                "gateway": capabilities.gateway,
                "usageReporting": capabilities.usage_reporting.map(UsageSource::as_str),
                "sessionLog": capabilities.session_log,
                "promptArgument": capabilities.prompt_argument,
            },
        })
    }

    fn fixture() -> String {
        let access = nexus_client::agent_setup::agent_catalog();
        assert_eq!(access.len(), Agent::ALL.len(), "one offline entry per agent");
        let agents: Vec<Value> = Agent::ALL
            .into_iter()
            .zip(access)
            .map(|(agent, access)| row(agent, access))
            .collect();
        let fixture = json!({
            "regenerate": format!("{BLESS}=1 cargo test --locked -p nexus-cli --lib agent_capabilities"),
            "agents": agents,
        });
        serde_json::to_string_pretty(&fixture).expect("JSON") + "\n"
    }

    /// `kit/fixtures/agent-capabilities.json` is what Rust computes. With `AGENT_CAPABILITIES_BLESS=1` the test
    /// writes it; without, a difference fails and names that command.
    #[test]
    fn kit_fixture_matches_the_computed_capabilities() {
        let path = fixture_path();
        let computed = fixture();
        if std::env::var(BLESS).is_ok_and(|value| value == "1") {
            std::fs::write(&path, &computed).expect("write the fixture");
            return;
        }
        // A missing file fails here by name, not as a stale one: CI's sparse checkout must list it.
        let stored = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert!(
            stored == computed,
            "{} is stale. Regenerate it from the repository root: {BLESS}=1 cargo test --locked -p nexus-cli --lib \
             agent_capabilities\n--- computed\n{computed}",
            path.display()
        );
    }

    /// The values the issue names: Claude Code and Codex export over OpenTelemetry, agy's usage is read from its
    /// conversation file. Antigravity and Cursor use only their plans.
    #[test]
    fn usage_sources_and_gateway_routes() {
        let of = |agent| {
            let capabilities = capabilities(agent);
            (capabilities.gateway, capabilities.usage_reporting)
        };
        assert_eq!(of(Agent::Claude), (Some("anthropic"), Some(UsageSource::Otel)));
        assert_eq!(of(Agent::Codex), (Some("openai"), Some(UsageSource::Otel)));
        assert_eq!(of(Agent::Antigravity), (None, Some(UsageSource::ConversationDb)));
        assert_eq!(of(Agent::Cursor), (None, None));
        assert_eq!(of(Agent::Grok), (None, None));
        assert_eq!(of(Agent::Opencode), (None, None));
    }

    /// Claude Code and Codex keep a session log: their transcripts have readers. Antigravity's conversations are
    /// databases, and Cursor keeps no transcript.
    #[test]
    fn session_logs_follow_the_transcript_readers() {
        assert_eq!(agent_ids(|capabilities| capabilities.session_log, "|"), "claude|codex");
    }

    /// These launchers accept the prompt that Review hands off.
    #[test]
    fn launchers_accepting_a_prompt_argument() {
        assert_eq!(
            agent_ids(|capabilities| capabilities.prompt_argument, "|"),
            "claude|codex|grok"
        );
    }

    /// The OpenTelemetry probe is not true for every agent: agy's launch exports nothing to the session.
    #[test]
    fn agy_launch_exports_nothing_to_the_session() {
        assert!(!exports_to_session(Agent::Antigravity));
        assert!(exports_to_session(Agent::Claude));
        assert!(exports_to_session(Agent::Codex));
    }
}
