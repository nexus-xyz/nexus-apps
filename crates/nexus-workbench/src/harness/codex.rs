//! Codex's session: its app server's JSON-RPC protocol ([`crate::codex`]).

use nexus_agents::AgentT;
use nexus_agents::Codex;
use nexus_local_agents::LocalAgentT;

use serde_json::{Value, json};

use super::{
    AccountFolders, AppTools, EVERY_MODE, Harness, Instructions, LEGACY_COMPACTION_INSTRUCTIONS_REFUSAL, LinkedPolicy,
    McpDelivery, McpStatus, ModeSettings, PlanDepth, PlanFuture, PlanProbe, PlanSeen, RevertCursor, RunFacts,
    SessionFeatures, TaskControlHints, effort_under,
};
use crate::driver::{Protocol, Start, semver};
use crate::event::{AgentCapabilities, Effort, PermissionMode, Via};
use crate::mcp_servers::{self, McpServer, Values};
use crate::plans;

/// The oldest version whose task controls Nexus has recorded.
const CONTROLS: (u64, u64, u64) = (0, 164, 0);

/// A permission mode as Codex's approval policy and sandbox: Ask first and Auto-accept edits ask before any command it
/// does not trust and any edit (Codex has no policy that asks before commands but not edits); Auto writes in
/// the folder and asks only to go outside it, a reviewer model answering ([`reviewer`]); Plan only reads.
pub(crate) fn policy(mode: PermissionMode) -> (&'static str, &'static str) {
    match mode {
        PermissionMode::Default | PermissionMode::AcceptEdits => ("untrusted", "workspace-write"),
        PermissionMode::Auto => ("on-request", "workspace-write"),
        PermissionMode::Plan => ("on-request", "read-only"),
        PermissionMode::BypassPermissions => ("never", "danger-full-access"),
    }
}

/// Who answers Codex's approval requests: its auto review in Auto, else the person. It is sent only when Auto is on or
/// is being left (a thread's open, a turn's settings), so a Codex too old to know the field still runs the other modes.
pub(crate) fn reviewer(mode: PermissionMode) -> &'static str {
    if mode == PermissionMode::Auto {
        "auto_review"
    } else {
        "user"
    }
}

/// The sandbox as `turn/start` takes it.
pub(crate) fn sandbox_policy(mode: PermissionMode) -> Value {
    match policy(mode).1 {
        "read-only" => json!({"type": "readOnly", "networkAccess": false}),
        "workspace-write" => json!({"type": "workspaceWrite", "writableRoots": [], "networkAccess": false,
            "excludeTmpdirEnvVar": false, "excludeSlashTmp": false}),
        _ => json!({"type": "dangerFullAccess"}),
    }
}

/// Codex's collaboration mode for a turn: `plan` in Plan, else `default`.
pub(crate) fn collaboration_mode(mode: PermissionMode) -> &'static str {
    if mode == PermissionMode::Plan {
        "plan"
    } else {
        "default"
    }
}

impl Harness for Codex {
    fn args(&self, start: &Start, _run: &RunFacts) -> Vec<String> {
        crate::codex::args(start.model, start.effort)
    }

    fn protocol(&self, start: &Start) -> Box<dyn Protocol> {
        Box::new(crate::codex::Driver::new(start))
    }

    /// Codex stops a child by its turn. Whether its own Interrupt spares children is not recorded yet: said not to. It
    /// has no background commands to move or read.
    fn capabilities(&self, version: Option<&str>) -> Option<AgentCapabilities> {
        let parsed = version.and_then(semver)?;
        (parsed >= CONTROLS).then(|| AgentCapabilities {
            version: version.map(String::from),
            stop: vec!["agent".to_string()],
            ..AgentCapabilities::default()
        })
    }

    fn live_capabilities(&self, version: Option<&str>, _reported_commands: Option<&[String]>) -> AgentCapabilities {
        AgentCapabilities {
            version: version.map(String::from),
            // App-server 0.164.0 defines ThreadCompactStartParams. This gate is independent of task controls.
            compact: version.and_then(semver).is_some_and(|version| version >= (0, 164, 0)),
            ..self.capabilities(version).unwrap_or_default()
        }
    }

    fn session_features(&self) -> SessionFeatures {
        SessionFeatures {
            native_commands: None,
            compact_instructions_refusal: Some(LEGACY_COMPACTION_INSTRUCTIONS_REFUSAL),
            live_rename: false,
            reads_account: true,
            discovers_skills: true,
            effort_selection: true,
            cursor_starts_turn: true,
            local_only: false,
            deny_reason_as_message: false,
            steers_mid_turn: true,
        }
    }

    fn task_control_hints(&self) -> TaskControlHints {
        TaskControlHints {
            update_to_stop: "Update Codex to 0.164.0 or later to stop one sub-agent.",
            cannot_background: "Codex sub-agents always run beside the session.",
        }
    }

    fn linked_policy(&self) -> LinkedPolicy {
        LinkedPolicy {
            owes_turn_after_task: false,
            children_ready_to_count: true,
        }
    }

    fn child_effort(&self, effort: Option<Effort>, ceiling: Option<Effort>) -> Option<Effort> {
        effort_under(effort, ceiling)
    }

    /// Offered when the model's own list is unknown: every level from Low to Ultra.
    fn usual_efforts(&self) -> &'static [Effort] {
        &[
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max,
            Effort::Ultra,
        ]
    }

    fn label(&self) -> &'static str {
        self.entry().identity.name
    }

    fn log_title(&self) -> &'static str {
        self.entry().identity.name
    }

    fn fault_component(&self) -> &'static str {
        "agent_spawn.codex"
    }

    fn native_model(&self, id: &str) -> String {
        self.provider_model_id(id)
    }

    fn answers_asks(&self) -> bool {
        false
    }

    fn switches(&self) -> bool {
        true
    }

    fn syncs(&self) -> bool {
        true
    }

    /// The Sandbox switch is agy's; Codex reads only the mode. The thread's open carries the policy and the sandbox,
    /// and each turn after a change carries them again, the sandbox as `sandboxPolicy`. The reviewer goes only into or
    /// out of Auto. The collaboration mode goes on a turn, once the model is known, in Plan or on a change
    /// ([`crate::codex`]).
    fn mode_settings(&self, mode: PermissionMode, _run: &RunFacts) -> ModeSettings {
        let (approval, sandbox) = policy(mode);
        ModeSettings(vec![
            ("approvalPolicy", approval.into()),
            ("sandbox", sandbox.into()),
            ("approvalsReviewer", reviewer(mode).into()),
            ("collaborationMode", collaboration_mode(mode).into()),
        ])
    }

    fn offered_modes(&self) -> &'static [PermissionMode] {
        &EVERY_MODE
    }

    fn default_mode(&self) -> PermissionMode {
        PermissionMode::Auto
    }

    fn read_plan(&self, probe: PlanProbe, _depth: PlanDepth) -> Option<PlanFuture> {
        Some(Box::pin(plans::codex(
            probe.program,
            probe.env,
            probe.account,
            probe.dir,
        )))
    }

    fn plan_tier(&self, name: &str) -> Option<&'static str> {
        Some(match name {
            "free" => "free",
            "plus" => "plus",
            "pro" => "pro",
            "team" | "business" => "team",
            "enterprise" | "edu" => "enterprise",
            _ => "unknown",
        })
    }

    fn observed_plan(&self, observation: &Value) -> PlanSeen {
        let account = observation.get("account");
        match account.and_then(|account| account.get("type")).and_then(Value::as_str) {
            Some("chatgpt") => match account
                .and_then(|account| account.get("planType"))
                .and_then(Value::as_str)
            {
                Some(plan) => PlanSeen::Remember(plan.to_owned()),
                None => PlanSeen::Keep,
            },
            Some(_) => PlanSeen::Forget,
            None => PlanSeen::Keep,
        }
    }

    fn allows_subscription_fork(&self) -> bool {
        true
    }

    fn mcp_servers(&self, servers: &[McpServer], values: &Values) -> McpDelivery {
        let configs = mcp_servers::enabled(servers).filter_map(|(i, server)| {
            let name = &server.name;
            if let Some(command) = &server.command {
                let timeout = server
                    .tool_timeout_sec
                    .map(|seconds| format!(",tool_timeout_sec={seconds}"))
                    .unwrap_or_default();
                Some(format!(
                    "mcp_servers.{name}={{command={},args={},env_vars={}{timeout}}}",
                    Value::from(command.as_str()),
                    json!(server.args),
                    json!(server.secrets)
                ))
            } else if let Some(url) = &server.url {
                let headers = server
                    .secrets
                    .iter()
                    .enumerate()
                    .map(|(j, header)| {
                        format!(
                            "{}={}",
                            Value::from(header.as_str()),
                            Value::from(mcp_servers::header_var(i, j))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                Some(format!(
                    "mcp_servers.{name}={{url={},env_http_headers={{{headers}}}}}",
                    Value::from(url.as_str())
                ))
            } else {
                None
            }
        });
        // These overrides go before app-server, where Codex reads them.
        let args = configs.flat_map(|config| ["-c".to_string(), config]).collect();
        McpDelivery::Args {
            args,
            env: mcp_servers::launch_env(servers, values),
        }
    }

    /// Codex starts its stdio servers with a base set of variables, not its own environment.
    fn mcp_launcher(
        &self,
        _program: &str,
        _servers: &[McpServer],
        _values: &Values,
    ) -> Option<super::LauncherDelivery> {
        None
    }

    fn app_tools(&self) -> AppTools {
        AppTools::CodexOverride
    }

    fn mcp_status(&self) -> McpStatus {
        McpStatus {
            reauthentication_failure: Some("reauthenticationRequired"),
            ..McpStatus::RUNTIME
        }
    }

    fn instructions(&self) -> Instructions {
        Instructions::All { file: false }
    }

    fn accounts(&self) -> Option<AccountFolders> {
        Some(AccountFolders {
            shared: &["config.toml", "AGENTS.md", "prompts", "skills", "plugins", "rules"],
            own_login: "Codex's own login",
            auto_rotate_by_default: false,
            moves_running_session: false,
        })
    }

    fn plan_payer(&self) -> Option<&'static str> {
        Some(self.entry().identity.name)
    }

    fn revert_cursor(&self) -> RevertCursor {
        RevertCursor::FirstDropped
    }

    fn launch_refusal(&self, _via: Via, _home: Option<&str>) -> Result<(), String> {
        Ok(())
    }
}
