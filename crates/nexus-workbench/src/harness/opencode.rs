//! OpenCode sessions use the shared ACP protocol with OpenCode's provider configuration.

use std::borrow::Cow;
use std::time::Duration;

use nexus_agents::{AgentT, Opencode};
use serde_json::{Value, json};

use super::{
    AccountFolders, AppTools, Harness, Instructions, LinkedPolicy, McpDelivery, McpStatus, ModeSettings, PlanDepth,
    PlanFuture, PlanProbe, PlanSeen, RevertCursor, RunFacts, SessionFeatures, TaskControlHints, effort_under,
};
use crate::acp::{Driver, Profile};
use crate::driver::{Protocol, Start};
use crate::event::{AgentCapabilities, Effort, PermissionMode, Via};
use crate::mcp_servers::{McpServer, Values};

const ACP_PROFILE: Profile = Profile {
    name: Cow::Borrowed("OpenCode"),
    sign_in: None,
    subscription_auth: None,
    sign_in_help: Cow::Borrowed("Configure a provider with `opencode auth login` in a terminal"),
    meta: || json!({}),
    modes: None,
    auto: None,
    reports_auto_offered: false,
    levels: None,
    tool_presentation: None,
    own_turn_quiet: Duration::from_secs(30),
};

impl Harness for Opencode {
    fn args(&self, _start: &Start, _run: &RunFacts) -> Vec<String> {
        vec!["acp".into()]
    }

    fn protocol(&self, start: &Start) -> Box<dyn Protocol> {
        Box::new(Driver::new(start, ACP_PROFILE))
    }

    fn capabilities(&self, _version: Option<&str>) -> Option<AgentCapabilities> {
        None
    }

    fn live_capabilities(&self, version: Option<&str>, _commands: Option<&[String]>) -> AgentCapabilities {
        AgentCapabilities {
            version: version.map(String::from),
            ..AgentCapabilities::default()
        }
    }

    fn session_features(&self) -> SessionFeatures {
        SessionFeatures {
            native_commands: None,
            compact_instructions_refusal: Some("OpenCode does not offer compaction in Nexus."),
            live_rename: false,
            reads_account: false,
            discovers_skills: false,
            effort_selection: false,
            cursor_starts_turn: false,
            local_only: false,
            deny_reason_as_message: false,
            steers_mid_turn: false,
        }
    }

    fn task_control_hints(&self) -> TaskControlHints {
        TaskControlHints {
            update_to_stop: "OpenCode does not offer task stop in Nexus.",
            cannot_background: "OpenCode does not offer background controls in Nexus.",
        }
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

    fn usual_efforts(&self) -> &'static [Effort] {
        &[]
    }

    fn label(&self) -> &'static str {
        self.entry().identity.name
    }

    fn log_title(&self) -> &'static str {
        self.label()
    }

    fn fault_component(&self) -> &'static str {
        "agent_spawn.opencode"
    }

    fn native_model(&self, id: &str) -> String {
        id.to_owned()
    }

    fn answers_asks(&self) -> bool {
        false
    }

    fn switches(&self) -> bool {
        false
    }

    fn syncs(&self) -> bool {
        false
    }

    fn mode_settings(&self, _mode: PermissionMode, _run: &RunFacts) -> ModeSettings {
        ModeSettings(vec![])
    }

    fn offered_modes(&self) -> &'static [PermissionMode] {
        &[PermissionMode::Default]
    }

    fn default_mode(&self) -> PermissionMode {
        PermissionMode::Default
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
        AppTools::NotOffered
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
        RevertCursor::NotOffered
    }

    fn launch_refusal(&self, _via: Via, _home: Option<&str>) -> Result<(), String> {
        Ok(())
    }
}
