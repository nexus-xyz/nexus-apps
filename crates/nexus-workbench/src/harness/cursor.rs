//! Cursor sessions use the shared ACP protocol on the person's Cursor plan.

use std::borrow::Cow;
use std::time::Duration;

use nexus_agents::{AgentT, Cursor};
use serde_json::{Value, json};

use super::{
    AccountFolders, AppTools, Harness, Instructions, LinkedPolicy, McpDelivery, McpStatus, ModeSettings, PlanDepth,
    PlanFuture, PlanProbe, PlanSeen, RevertCursor, RunFacts, SessionFeatures, TaskControlHints, effort_under,
};
use crate::acp::{Driver, ModeMapping, Profile, ToolPresentation};
use crate::driver::{Protocol, Start};
use crate::event::{AgentCapabilities, Effort, PermissionMode, Via};
use crate::mcp_servers::{McpServer, Values};

pub(crate) const ACP_PROFILE: Profile = Profile {
    name: Cow::Borrowed("Cursor"),
    sign_in: Some(Cow::Borrowed("cursor_login")),
    sign_in_help: Cow::Borrowed("Run `cursor-agent login` in a terminal"),
    meta: || json!({}),
    modes: Some(ModeMapping {
        native: mode_id,
        observed: |id| match id {
            Some("agent") => Some(PermissionMode::AcceptEdits),
            Some("plan") => Some(PermissionMode::Plan),
            _ => None,
        },
        requested: |mode, _| mode_id(mode),
        confirm_opening: true,
    }),
    auto: None,
    reports_auto_offered: false,
    levels: None,
    tool_presentation: Some(ToolPresentation {
        edit_text,
        output: tool_output,
    }),
    own_turn_quiet: Duration::from_secs(30),
};

fn mode_id(mode: PermissionMode) -> Option<&'static str> {
    match mode {
        PermissionMode::AcceptEdits => Some("agent"),
        PermissionMode::Plan => Some("plan"),
        PermissionMode::Default | PermissionMode::Auto | PermissionMode::BypassPermissions => None,
    }
}

fn edit_text(diff: Option<&Value>) -> (Option<&str>, Option<&str>) {
    let old = diff
        .and_then(|diff| diff["oldText"].as_str())
        .filter(|&old| old != "-- /dev/null");
    let new = diff
        .and_then(|diff| diff["newText"].as_str())
        .map(|text| match text.split_once('\n') {
            Some((head, rest)) if head.starts_with("++ b/") => rest,
            _ => text,
        });
    (old, new)
}

fn tool_output(raw: &Value) -> Option<Vec<String>> {
    let map = raw.as_object()?;
    let keys = ["stdout", "stderr", "content"];
    keys.iter().any(|key| map.contains_key(*key)).then(|| {
        keys.into_iter()
            .filter_map(|key| map.get(key).and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .map(String::from)
            .collect()
    })
}

impl Harness for Cursor {
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
            compact_instructions_refusal: Some("Cursor does not offer compaction in Nexus."),
            live_rename: false,
            reads_account: false,
            discovers_skills: false,
            effort_selection: false,
            cursor_starts_turn: false,
            local_only: true,
            deny_reason_as_message: true,
            steers_mid_turn: false,
        }
    }

    fn task_control_hints(&self) -> TaskControlHints {
        TaskControlHints {
            update_to_stop: "Cursor does not offer task stop in Nexus.",
            cannot_background: "Cursor does not offer background controls in Nexus.",
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

    /// None offered when the model's own list is unknown: Nexus does not set Cursor's reasoning
    /// (`effort_selection: false`).
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
        "agent_spawn.cursor"
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

    fn mode_settings(&self, mode: PermissionMode, _run: &RunFacts) -> ModeSettings {
        ModeSettings(mode_id(mode).map(|id| ("modeId", id.to_owned())).into_iter().collect())
    }

    fn offered_modes(&self) -> &'static [PermissionMode] {
        &[PermissionMode::AcceptEdits, PermissionMode::Plan]
    }

    fn default_mode(&self) -> PermissionMode {
        PermissionMode::AcceptEdits
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
        Some(self.entry().identity.plan)
    }

    fn revert_cursor(&self) -> RevertCursor {
        RevertCursor::NotOffered
    }

    fn launch_refusal(&self, via: Via, _home: Option<&str>) -> Result<(), String> {
        match via {
            Via::Nexus => Err(self.entry().route_unsupported()),
            Via::Subscription => Ok(()),
        }
    }
}
