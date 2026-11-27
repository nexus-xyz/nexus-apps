//! Nexus's own agent's session: `nexus __agent`, the nexus CLI's agent with no screen ([`crate::nexus`]). It is not a
//! coding agent, so it has no type in `nexus_agents`: [`Nexus`] is its type here.

use serde_json::Value;

use super::{
    AccountFolders, AppTools, EVERY_MODE, Harness, Instructions, LEGACY_COMPACTION_INSTRUCTIONS_REFUSAL,
    LEGACY_TASK_CONTROL_HINTS, LinkedPolicy, McpDelivery, McpStatus, ModeSettings, PlanDepth, PlanFuture, PlanProbe,
    PlanSeen, RevertCursor, RunFacts, SessionFeatures, TaskControlHints, effort_under,
};
use crate::driver::{Protocol, Start};
use crate::event::{AgentCapabilities, Effort, PermissionMode, Via};
use crate::mcp_servers::{McpServer, Values};

/// The approval level `nexus __agent` takes for a permission mode: Auto and Full access run without asking, Accept
/// edits runs edits, Plan refuses every edit and command (`read-only`). A CLI from before `read-only` reads
/// it as `ask`, as Ask first.
pub(crate) fn approval_level(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Auto | PermissionMode::BypassPermissions => "auto",
        PermissionMode::AcceptEdits => "auto-edit",
        PermissionMode::Plan => "read-only",
        PermissionMode::Default => "ask",
    }
}

/// Nexus's own agent, which only sessions run.
#[derive(Debug, Clone, Copy)]
pub struct Nexus;

impl Harness for Nexus {
    fn args(&self, start: &Start, _run: &RunFacts) -> Vec<String> {
        crate::nexus::args(start)
    }

    fn protocol(&self, start: &Start) -> Box<dyn Protocol> {
        Box::new(crate::nexus::Driver::new(start))
    }

    /// Nexus's agent reports no task controls.
    fn capabilities(&self, _version: Option<&str>) -> Option<AgentCapabilities> {
        None
    }

    fn live_capabilities(&self, version: Option<&str>, _reported_commands: Option<&[String]>) -> AgentCapabilities {
        AgentCapabilities {
            version: version.map(String::from),
            ..AgentCapabilities::default()
        }
    }

    fn session_features(&self) -> SessionFeatures {
        SessionFeatures {
            native_commands: None,
            compact_instructions_refusal: Some(LEGACY_COMPACTION_INSTRUCTIONS_REFUSAL),
            live_rename: false,
            reads_account: false,
            discovers_skills: false,
            effort_selection: true,
            cursor_starts_turn: false,
            local_only: false,
            deny_reason_as_message: false,
        }
    }

    fn task_control_hints(&self) -> TaskControlHints {
        LEGACY_TASK_CONTROL_HINTS
    }

    fn linked_policy(&self) -> LinkedPolicy {
        LinkedPolicy {
            owes_turn_after_task: false,
            children_ready_to_count: false,
        }
    }

    fn child_effort(&self, effort: Option<Effort>, ceiling: Option<Effort>) -> Option<Effort> {
        effort_under(effort, ceiling)
    }

    /// None offered when the model's own list is unknown: `nexus __agent` takes no effort. Its arguments carry none,
    /// and it acknowledges an effort change without applying it.
    fn usual_efforts(&self) -> &'static [Effort] {
        &[]
    }

    fn label(&self) -> &'static str {
        "Nexus"
    }

    fn log_title(&self) -> &'static str {
        "Nexus agent"
    }

    fn fault_component(&self) -> &'static str {
        "agent_spawn.nexus"
    }

    /// Gateway takes the policy id itself.
    fn native_model(&self, id: &str) -> String {
        id.to_string()
    }

    /// Nexus's own agent asks the workbench, which answers in Auto and Full access. Its asks are Nexus's own tools'.
    fn answers_asks(&self) -> bool {
        true
    }

    /// No handoff carries a Nexus conversation.
    fn switches(&self) -> bool {
        false
    }

    fn syncs(&self) -> bool {
        true
    }

    /// The Sandbox switch is agy's; Nexus's agent reads only the mode.
    fn mode_settings(&self, mode: PermissionMode, _run: &RunFacts) -> ModeSettings {
        ModeSettings(vec![("approvals", approval_level(mode).into())])
    }

    fn offered_modes(&self) -> &'static [PermissionMode] {
        &EVERY_MODE
    }

    fn default_mode(&self) -> PermissionMode {
        PermissionMode::Auto
    }

    fn read_plan(&self, _probe: PlanProbe, _depth: PlanDepth) -> Option<PlanFuture> {
        None
    }

    fn plan_tier(&self, _name: &str) -> Option<&'static str> {
        None
    }

    fn observed_plan(&self, _observation: &Value) -> PlanSeen {
        PlanSeen::Keep
    }

    fn allows_subscription_fork(&self) -> bool {
        false
    }

    fn mcp_servers(&self, _servers: &[McpServer], _values: &Values) -> McpDelivery {
        McpDelivery::NotOffered
    }

    fn mcp_launcher(
        &self,
        _program: &str,
        _servers: &[McpServer],
        _values: &Values,
    ) -> Option<super::LauncherDelivery> {
        None
    }

    fn app_tools(&self) -> AppTools {
        AppTools::ClaudeOptions {
            reads_project_mcp: false,
            disallow_native_shell: false,
        }
    }

    fn mcp_status(&self) -> McpStatus {
        McpStatus::RUNTIME
    }

    fn instructions(&self) -> Instructions {
        Instructions::OwnPrompt
    }

    fn accounts(&self) -> Option<AccountFolders> {
        None
    }

    fn plan_payer(&self) -> Option<&'static str> {
        None
    }

    fn revert_cursor(&self) -> RevertCursor {
        RevertCursor::LastKept
    }

    fn launch_refusal(&self, _via: Via, _home: Option<&str>) -> Result<(), String> {
        Ok(())
    }
}
