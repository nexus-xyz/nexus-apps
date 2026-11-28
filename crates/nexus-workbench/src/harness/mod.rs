//! What a session's agent does through its session protocol, one file per agent. [`Harness`] is this crate's domain
//! trait, as `LocalAgentT` is nexus-local-agents': each coding agent's type in `nexus_agents` implements it here, and
//! Nexus's own agent implements it on [`Nexus`]. [`harness`] is the one dispatch. `LocalAgentT` stays what running the
//! agent's own commands tells us; `Harness` is what its session tells us.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use serde_json::Value;

use crate::driver::{Protocol, Start};
use crate::event::{AgentCapabilities, Effort, PermissionMode, SessionAgent, Via};
use crate::mcp_servers::{McpServer, Values};
use crate::plans::PlanUsage;

pub(crate) mod antigravity;
pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod cursor;
pub(crate) mod nexus;
mod opencode;

pub use antigravity::args as legacy_acp_args;
pub use nexus::Nexus;

/// Every Nexus mode, in the default picker order.
pub const EVERY_MODE: [PermissionMode; 5] = [
    PermissionMode::Default,
    PermissionMode::AcceptEdits,
    PermissionMode::Plan,
    PermissionMode::Auto,
    PermissionMode::BypassPermissions,
];

/// A Nexus mode as the agent's own settings, by the agent's own names: `("--permission-mode", "auto")`,
/// `("approvalPolicy", "on-request")`. A setting the mode has no value for is not listed. The driver sends the typed
/// mapping in the agent's file, each setting when its protocol takes it; this is that mapping as one record, for the
/// review rule and the mode table's test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeSettings(pub Vec<(&'static str, String)>);

/// What one run's arguments read besides [`Start`]. Only Antigravity reads them. `wb_overview` gives the store
/// owner's, so an app over the background service reports what the service runs ([`crate::runs_as`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunFacts {
    /// Antigravity's Sandbox switch ([`crate::Host::antigravity_sandbox`]).
    pub sandbox: bool,
    /// Whether agy offers its `auto` mode, as its last run said ([`crate::driver::Out::AutoOffered`]); None when no
    /// run has said.
    pub auto_offered: Option<bool>,
}

/// A plan read's executable, subscription environment, and login.
pub struct PlanProbe {
    pub program: PathBuf,
    pub env: HashMap<String, String>,
    pub account: Option<String>,
    /// The extra login's folder, when selected.
    pub dir: Option<PathBuf>,
    /// The CLI config folder for a tier-only status command.
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub enum PlanDepth {
    /// The login and its usage limits, as Settings displays them.
    Limits,
    /// The plan tier, as the CLI sends it on a session open.
    TierOnly,
}

pub type PlanFuture = Pin<Box<dyn Future<Output = PlanUsage> + Send + 'static>>;

/// How a running session's observation changes the last known tier.
#[derive(Debug, PartialEq, Eq)]
pub enum PlanSeen {
    Remember(String),
    Forget,
    Keep,
}

/// How the configured MCP servers and their secrets reach the agent.
pub enum McpDelivery {
    Args {
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    /// In the session protocol, as the launcher's entries ([`crate::mcp_servers::session_entries`]): the servers'
    /// values stay with the app.
    Session(crate::mcp_servers::SessionServers),
    NotOffered,
}

/// What an agent gets for servers started through the launcher ([`Harness::mcp_launcher`]).
pub struct LauncherDelivery {
    /// Arguments to put before its own: the config, which names the launcher for each local server.
    pub args: Vec<String>,
    /// Variables for its environment: only the remote servers' header values.
    pub env: Vec<(String, String)>,
}

/// How the app's session-scoped server reaches the agent.
#[derive(Clone, Copy)]
pub enum AppTools {
    ClaudeOptions {
        reads_project_mcp: bool,
        disallow_native_shell: bool,
    },
    CodexOverride,
    NotOffered,
}

impl AppTools {
    pub fn offered(self) -> bool {
        !matches!(self, Self::NotOffered)
    }
}

/// Fields the protocol reports for MCP inventory and runtime health.
#[derive(Clone, Copy)]
pub struct McpStatus {
    pub page_key: &'static str,
    pub status_keys: &'static [&'static str],
    pub reauthentication_failure: Option<&'static str>,
}

impl McpStatus {
    pub(crate) const RUNTIME: Self = Self {
        page_key: "data",
        status_keys: &["runtimeStatus", "status"],
        reauthentication_failure: None,
    };

    pub(crate) fn reported(self, server: &Value) -> Option<&str> {
        let status = self.status_keys.iter().find_map(|key| server.get(*key))?.as_str()?;
        if status == "failed"
            && self
                .reauthentication_failure
                .is_some_and(|reason| server.get("failureReason").and_then(Value::as_str) == Some(reason))
        {
            return Some("needs-auth");
        }
        Some(status)
    }
}

pub enum Instructions {
    All {
        file: bool,
    },
    /// Only the guidance for the tools a session-protocol agent gets ([`crate::driver::session_tool_instructions`]).
    SessionTools,
    /// The agent builds its prompt itself.
    OwnPrompt,
}

impl Instructions {
    /// The prompt text the workbench supplies.
    /// Callers never send the empty result for `Self::OwnPrompt`.
    pub(crate) fn text(self) -> String {
        match self {
            Self::All { .. } => crate::driver::instructions(),
            Self::SessionTools => crate::driver::session_tool_instructions(),
            Self::OwnPrompt => String::new(),
        }
    }
}

/// The folders and rotation policy for an agent with extra subscription accounts.
#[derive(Clone, Copy)]
pub struct AccountFolders {
    pub shared: &'static [&'static str],
    pub own_login: &'static str,
    pub auto_rotate_by_default: bool,
    /// Whether an existing conversation can move to another account.
    pub moves_running_session: bool,
}

#[derive(Clone, Copy)]
pub enum RevertCursor {
    /// The last message of the last retained turn.
    LastKept,
    /// The first turn to drop, including every turn after it.
    FirstDropped,
    /// The driver cannot rewind its native conversation to a cursor.
    NotOffered,
}

/// Operations the session protocol supports.
#[derive(Clone, Copy)]
pub struct SessionFeatures {
    pub native_commands: Option<NativeCommands>,
    pub compact_instructions_refusal: Option<&'static str>,
    pub live_rename: bool,
    pub reads_account: bool,
    pub discovers_skills: bool,
    /// Whether the composer offers reasoning effort.
    pub effort_selection: bool,
    /// A cursor can mark an agent-started turn.
    pub cursor_starts_turn: bool,
    /// Its sessions never run on a remote server: only on this computer, in the app or its background service.
    pub local_only: bool,
    /// The agent's answer to an ask carries no text (ACP): a denial's reason goes to it as the person's own queued
    /// message ([`crate::Workbench::answer`]).
    pub deny_reason_as_message: bool,
    /// The driver takes a message mid-turn (Claude Code at its next step, Codex as a steer). Only these agents get
    /// `terminal_run`'s `watch` option, since its exit notice must reach the turn under way.
    pub steers_mid_turn: bool,
}

/// How a process reports its native command inventory.
#[derive(Clone, Copy)]
pub struct NativeCommands {
    /// The reported command that enables the typed Compact action, when there is one.
    pub compact: Option<&'static str>,
}

impl NativeCommands {
    pub(crate) fn apply(self, capabilities: &mut AgentCapabilities, commands: &[String]) {
        if let Some(compact) = self.compact {
            capabilities.compact = commands.iter().any(|command| command == compact);
        }
        capabilities.commands = Some(commands.to_vec());
    }
}

#[derive(Clone, Copy)]
pub struct TaskControlHints {
    pub update_to_stop: &'static str,
    pub cannot_background: &'static str,
}

// Existing UI copy for agents that used the shared fallback.
const LEGACY_TASK_CONTROL_HINTS: TaskControlHints = TaskControlHints {
    update_to_stop: "Update Claude Code to 2.1.292 or later to stop one sub-agent.",
    cannot_background: "Only a command can move to the background.",
};
const LEGACY_COMPACTION_INSTRUCTIONS_REFUSAL: &str = "Codex compaction accepts no instructions.";

#[derive(Clone, Copy)]
pub struct LinkedPolicy {
    /// The lead can start a turn when a background task ends between turns.
    pub owes_turn_after_task: bool,
    /// Wait for the native child to be ready before counting it for spend-cap cleanup.
    pub children_ready_to_count: bool,
}

/// The trait every session agent implements, in `src/harness/<agent>.rs`. No method has a default body: a new agent
/// does not build until its file says each one, and a capability it lacks is a stated value.
pub trait Harness: Sync {
    /// The agent's own arguments for one run.
    fn args(&self, start: &Start, run: &RunFacts) -> Vec<String>;
    /// The protocol for one run of its process.
    fn protocol(&self, start: &Start) -> Box<dyn Protocol>;
    /// What a running agent of this version accepts. None: no task control, for an older or unknown version or an
    /// agent with none.
    fn capabilities(&self, version: Option<&str>) -> Option<AgentCapabilities>;
    /// The capabilities published for an initialized process.
    fn live_capabilities(&self, version: Option<&str>, reported_commands: Option<&[String]>) -> AgentCapabilities;
    /// Which typed operations and observations this protocol supports.
    fn session_features(&self) -> SessionFeatures;
    /// What the controls say when a task operation is unavailable.
    fn task_control_hints(&self) -> TaskControlHints;
    /// How native tasks affect linked-session progress and spend control.
    fn linked_policy(&self) -> LinkedPolicy;
    /// Clamp the requested effort using this agent's supported levels.
    fn child_effort(&self, effort: Option<Effort>, ceiling: Option<Effort>) -> Option<Effort>;
    /// The reasoning levels the composers offer when the model's own list is unknown. Empty: they offer none.
    fn usual_efforts(&self) -> &'static [Effort];
    /// The agent as people name it.
    fn label(&self) -> &'static str;
    /// The agent as a CLI run's thread title names it ([`crate::cli_log`]).
    fn log_title(&self) -> &'static str;
    /// Where the app's client errors report that it did not start ([`crate::faults`]).
    fn fault_component(&self) -> &'static str;
    /// A policy id (`openai/gpt-6.1-terra`) as the agent names the model (`gpt-6.1-terra`).
    fn native_model(&self, id: &str) -> String;
    /// Whether the workbench may answer this agent's permission asks, in a mode that runs tools unasked
    /// ([`crate::driver::answers_ask`]). A coding agent maps each mode to its own setting, so an ask that still comes
    /// is the person's.
    fn answers_asks(&self) -> bool;
    /// Whether a session may switch to or from this agent: its conversation is one the handoff carries
    /// ([`crate::agent_switch`]).
    fn switches(&self) -> bool;
    /// Whether Gateway's session store takes its sessions: the agents its `SyncedAgent` names
    /// ([`crate::session_sync`]).
    fn syncs(&self) -> bool;
    /// The agent's own settings for a Nexus mode. Each agent maps every mode to its own mechanism; Nexus only picks
    /// the setting.
    fn mode_settings(&self, mode: PermissionMode, run: &RunFacts) -> ModeSettings;
    /// The Nexus modes the agent supports.
    fn offered_modes(&self) -> &'static [PermissionMode];
    /// The mode a new desktop Code session uses when the person has no saved choice.
    fn default_mode(&self) -> PermissionMode;
    /// A plan probe, if the agent supports this depth. The probe starts when polled on a Tokio runtime.
    fn read_plan(&self, probe: PlanProbe, depth: PlanDepth) -> Option<PlanFuture>;
    /// The contract tier for a normalized plan name, if the agent reports tiers.
    fn plan_tier(&self, name: &str) -> Option<&'static str>;
    /// What the running agent's account or plan observation says about its last known tier.
    fn observed_plan(&self, observation: &Value) -> PlanSeen;
    /// Whether a fork may select this agent's subscription.
    fn allows_subscription_fork(&self) -> bool;
    /// The configured MCP servers and secrets to send at launch.
    fn mcp_servers(&self, servers: &[McpServer], values: &Values) -> McpDelivery;
    /// The configured MCP servers when each local one starts through Nexus's launcher (`program --mcp-launch=<name>`,
    /// [`crate::mcp_servers::launcher_config`]): arguments and variables, with no local server's value in either. None
    /// when the agent does not start its servers so.
    fn mcp_launcher(&self, program: &str, servers: &[McpServer], values: &Values) -> Option<LauncherDelivery>;
    /// How to install the app's session-scoped server.
    fn app_tools(&self) -> AppTools;
    /// How the protocol names its MCP health fields.
    fn mcp_status(&self) -> McpStatus;
    /// Which workbench instructions the agent gets and whether it reads them from a file.
    fn instructions(&self) -> Instructions;
    /// Extra subscription accounts and how they rotate.
    fn accounts(&self) -> Option<AccountFolders>;
    /// The plan payer name shown in the composer.
    fn plan_payer(&self) -> Option<&'static str>;
    /// The cursor this agent can use to rewind its native conversation.
    fn revert_cursor(&self) -> RevertCursor;
    /// Per-agent requirements checked before preparing a coding agent's launch.
    fn launch_refusal(&self, via: Via, home: Option<&str>) -> Result<(), String>;
}

/// `effort` under an ancestor at `ceiling`, never above it. None, the model's default, has no known level: under an
/// explicit effort it becomes that effort, and under the default a session may pick a lower effort (low or medium),
/// never a higher one.
pub(crate) fn effort_under(effort: Option<Effort>, ceiling: Option<Effort>) -> Option<Effort> {
    match (effort, ceiling) {
        (Some(effort), Some(ceiling)) => Some(effort.min(ceiling)),
        (None, ceiling) => ceiling,
        (Some(effort), None) => (effort <= Effort::Medium).then_some(effort),
    }
}

/// Each session agent supported by this build has its own harness.
pub fn harness(agent: SessionAgent) -> &'static dyn Harness {
    match agent {
        SessionAgent::Claude => &nexus_agents::Claude,
        SessionAgent::Codex => &nexus_agents::Codex,
        SessionAgent::Antigravity => &nexus_agents::Antigravity,
        #[cfg(feature = "cursor")]
        SessionAgent::Cursor => &nexus_agents::Cursor,
        #[cfg(feature = "opencode")]
        SessionAgent::Opencode => &nexus_agents::Opencode,
        SessionAgent::Nexus => &Nexus,
    }
}

#[cfg(test)]
mod mode_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_compaction_and_commands_keep_their_own_support_rules() {
        let claude = harness(SessionAgent::Claude);
        let compact = vec!["compact".to_owned()];
        let old = claude.live_capabilities(Some("2.1.291"), Some(&compact));
        assert!(old.stop.is_empty());
        assert!(old.compact);
        assert_eq!(old.commands, Some(compact.clone()));

        let empty = claude.live_capabilities(None, Some(&[]));
        assert!(!empty.compact);
        assert_eq!(empty.commands, Some(vec![]));
        assert_eq!(claude.live_capabilities(None, None).commands, None);

        let codex = harness(SessionAgent::Codex);
        for (version, expected) in [(None, false), (Some("0.163.9"), false), (Some("0.164.0-alpha.1"), true)] {
            let caps = codex.live_capabilities(version, Some(&compact));
            assert_eq!(caps.compact, expected);
            assert_eq!(caps.commands, None);
            assert_eq!(caps.version.as_deref(), version);
        }

        for agent in [SessionAgent::Antigravity, SessionAgent::Nexus] {
            let caps = harness(agent).live_capabilities(Some("1.0.0"), Some(&compact));
            assert!(!caps.compact);
            assert_eq!(caps.commands, None);
            assert_eq!(caps.version.as_deref(), Some("1.0.0"));
        }
    }

    /// No method of [`Harness`] has a default body: each agent's file writes every one.
    #[test]
    fn harness_trait_has_no_default_bodies() {
        let source = include_str!("mod.rs");
        let start = source
            .find("pub trait Harness: Sync {")
            .unwrap_or_else(|| panic!("no Harness trait"));
        let body = &source[start..];
        let end = body.find("\n}\n").unwrap_or_else(|| panic!("the trait does not close"));
        let body = &body[body.find('{').map_or(0, |at| at + 1)..end];
        for line in body.lines().map(str::trim).filter(|line| !line.starts_with("//")) {
            assert!(!line.contains('{'), "a Harness method has a default body: {line}");
        }
        assert!(body.matches("fn ").count() >= 10, "the trait's methods were not read");
    }

    /// Each agent reaches its own harness: the names the app and the CLI log show, and the model id it takes.
    #[test]
    fn every_session_agent_has_a_harness() {
        // (label, CLI log title, a policy model id, that id as the agent takes it)
        let expected = |agent| match agent {
            SessionAgent::Claude => (
                "Claude Code",
                "Claude Code",
                "anthropic/claude-fable-5.1",
                "claude-fable-5-1",
            ),
            SessionAgent::Codex => ("Codex", "Codex", "openai/gpt-6.1-terra", "gpt-6.1-terra"),
            SessionAgent::Nexus => (
                "Nexus",
                "Nexus agent",
                "anthropic/claude-fable-5.1",
                "anthropic/claude-fable-5.1",
            ),
            SessionAgent::Antigravity => ("Antigravity", "Antigravity", "gemini-3.8-flash", "gemini-3.8-flash"),
            #[cfg(feature = "cursor")]
            SessionAgent::Cursor => ("Cursor", "Cursor", "default", "default"),
            #[cfg(feature = "opencode")]
            SessionAgent::Opencode => ("OpenCode", "OpenCode", "fake/model", "fake/model"),
        };
        for agent in SessionAgent::ALL {
            let (label, title, policy_id, native) = expected(agent);
            let harness = harness(agent);
            assert_eq!(harness.label(), label, "{agent:?}");
            assert_eq!(harness.log_title(), title, "{agent:?}");
            assert_eq!(harness.native_model(policy_id), native, "{agent:?}");
            assert_eq!(
                harness.fault_component(),
                format!("agent_spawn.{}", agent.as_str()),
                "{agent:?}"
            );
        }
    }

    /// The levels each agent's composers offer when the model's own list is unknown.
    #[test]
    fn usual_efforts_per_agent() {
        use Effort::{High, Low, Max, Medium, Ultra, Xhigh};
        for agent in SessionAgent::ALL {
            let expected: &[Effort] = match agent {
                SessionAgent::Claude => &[Low, Medium, High, Xhigh, Max],
                SessionAgent::Codex => &[Low, Medium, High, Xhigh, Max, Ultra],
                SessionAgent::Antigravity => &[Low, Medium, High],
                SessionAgent::Nexus => &[],
                #[cfg(feature = "cursor")]
                SessionAgent::Cursor => &[],
                #[cfg(feature = "opencode")]
                SessionAgent::Opencode => &[],
            };
            assert_eq!(harness(agent).usual_efforts(), expected, "{agent:?}");
        }
    }

    #[test]
    fn session_agents_offer_their_supported_plan_reads() {
        for agent in SessionAgent::ALL {
            let (limits, tier, fork) = match agent {
                SessionAgent::Claude | SessionAgent::Codex => (true, true, true),
                SessionAgent::Antigravity => (true, false, true),
                SessionAgent::Nexus => (false, false, false),
                #[cfg(feature = "cursor")]
                SessionAgent::Cursor => (false, false, false),
                #[cfg(feature = "opencode")]
                SessionAgent::Opencode => (false, false, false),
            };
            let harness = harness(agent);
            for (depth, expected) in [(PlanDepth::Limits, limits), (PlanDepth::TierOnly, tier)] {
                let probe = PlanProbe {
                    program: PathBuf::new(),
                    env: HashMap::new(),
                    account: None,
                    dir: None,
                    cwd: None,
                };
                assert_eq!(
                    harness.read_plan(probe, depth).is_some(),
                    expected,
                    "{agent:?} {depth:?}"
                );
            }
            assert_eq!(harness.allows_subscription_fork(), fork, "{agent:?}");
        }
    }

    #[test]
    fn observed_plans_preserve_the_account_update_rules() {
        use serde_json::json;

        let claude = harness(SessionAgent::Claude);
        assert_eq!(
            claude.observed_plan(&json!({"plan": "Claude Max"})),
            PlanSeen::Remember("Claude Max".into())
        );
        assert_eq!(claude.observed_plan(&json!({"mcp": []})), PlanSeen::Keep);

        let codex = harness(SessionAgent::Codex);
        for (observation, seen) in [
            (
                json!({"account": {"type": "chatgpt", "planType": "plus"}}),
                PlanSeen::Remember("plus".into()),
            ),
            (json!({"account": {"type": "chatgpt"}}), PlanSeen::Keep),
            (json!({"account": {"type": "apiKey"}}), PlanSeen::Forget),
            (json!({"account": null}), PlanSeen::Keep),
            (json!({"mcpUpdate": {}}), PlanSeen::Keep),
        ] {
            assert_eq!(codex.observed_plan(&observation), seen, "{observation}");
        }
    }

    #[test]
    fn only_mid_turn_steering_agents_enable_steers_mid_turn() {
        for agent in SessionAgent::ALL {
            let expected = matches!(agent, SessionAgent::Claude | SessionAgent::Codex);
            assert_eq!(harness(agent).session_features().steers_mid_turn, expected, "{agent:?}");
        }
    }
}
