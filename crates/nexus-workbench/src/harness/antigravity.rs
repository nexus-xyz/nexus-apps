//! Antigravity's session: `agy acp`, the Agent Client Protocol ([`crate::acp`]).
//! The run's mode and Sandbox switch decide its launch flags. Its ACP mode follows T3 Code's fixed mapping
//! ([`acp_mode_id`]), always set as the conversation opens; a refusal ends the run.
//! The recorded ACP fixtures pin mode ordering, settings, read receipts, and idle turns.

use nexus_agents::AgentT;
use nexus_agents::Antigravity;
use nexus_local_agents::LocalAgentT;

use std::borrow::Cow;
use std::time::Duration;

use serde_json::{Value, json};

use super::{
    AccountFolders, AppTools, EVERY_MODE, Harness, Instructions, LEGACY_COMPACTION_INSTRUCTIONS_REFUSAL,
    LEGACY_TASK_CONTROL_HINTS, LinkedPolicy, McpDelivery, McpStatus, ModeSettings, PlanDepth, PlanFuture, PlanProbe,
    PlanSeen, RevertCursor, RunFacts, SessionFeatures, TaskControlHints, effort_under,
};
use crate::acp::{Driver, ModeMapping, Profile, ThinkingLevels};
use crate::driver::{Protocol, Start};
use crate::event::{AgentCapabilities, Effort, PermissionMode, Via};
use crate::mcp_servers::{McpServer, Values};
use crate::plans;

/// The sign-in agy offers for a personal Google account.
const SIGN_IN: &str = "oauth-personal";

/// How long agy stays silent, with no tool call running and no approval open, before a turn it started itself counts
/// as ended: agy says nothing when such a turn ends ([`Protocol::quiet`]).
pub(crate) const OWN_TURN_QUIET: Duration = Duration::from_secs(30);

/// agy's thinking levels: low, medium and high, and xhigh for the models `agy models` lists at it.
fn thought_level(effort: Effort) -> Option<&'static str> {
    match effort {
        Effort::Low => Some("low"),
        Effort::Medium => Some("medium"),
        Effort::High => Some("high"),
        Effort::Xhigh => Some("xhigh"),
        _ => None,
    }
}

/// The arguments for one run in `mode`: agy's global flags, then `acp`. `sandbox`: the Sandbox switch, which adds
/// `--sandbox` in every mode. `_auto` (whether agy offers its `auto` mode) does not change them: Auto runs as Ask
/// first, as in T3 Code. agy reads them as it starts, for that run. The conversation's id goes in `session/load`, not
/// on the command line.
pub fn args(mode: PermissionMode, sandbox: bool, _auto: Option<bool>) -> Vec<String> {
    let mut args = Vec::new();
    if sandbox {
        args.push("--sandbox".to_string());
    }
    if let Some(permission) = tool_permission(mode) {
        args.push("--settings".to_string());
        args.push(json!({ "toolPermission": permission }).to_string());
    }
    args.push("acp".to_string());
    args
}

/// The page guidance's line for agents whose tool answers reach the app, which then shows a published page as a card.
const PAGE_CARD_LINE: &str = "The app shows published pages as cards, so there is no viewer link to print.";
/// agy forwards no tool answer to the app (agy 1.5.15: an MCP tool's `tool_call_update` has no content), so the app
/// finds a page only in the reply; the built-in server adds the link to the answer (nexus-desktop-mcp's
/// `with_view_link`).
const PAGE_LINK_LINE: &str = "After you create or update a page, print the viewer link from the tool answer's `View it:` line in your reply: the app shows the page as a card from that link.";

/// The workbench's instructions as agy gets them, appended to its system prompt.
pub(crate) fn instructions() -> String {
    Antigravity
        .instructions()
        .text()
        .replace(PAGE_CARD_LINE, PAGE_LINK_LINE)
}

pub(crate) const ACP_PROFILE: Profile = Profile {
    name: Cow::Borrowed("Antigravity"),
    sign_in: Some(Cow::Borrowed(SIGN_IN)),
    sign_in_help: Cow::Borrowed("Run `agy` in a terminal and sign in with Google"),
    meta: || json!({ "antigravity": { "systemPrompt": { "append": instructions() } } }),
    modes: Some(ModeMapping {
        native: |mode| Some(acp_mode_id(mode)),
        observed: mode_of,
        confirm_opening: false,
        requested: |mode, _| Some(acp_mode_id(mode)),
        config_option: true,
    }),
    auto: None,
    reports_auto_offered: false,
    levels: Some(ThinkingLevels {
        option: "thought_level",
        value: thought_level,
        unsupported: Cow::Borrowed("Antigravity thinks at low, medium, high or extra high."),
    }),
    own_turn_quiet: OWN_TURN_QUIET,
    tool_presentation: None,
};

/// agy's `toolPermission` for `mode`, passed in `--settings` for that run only ([`args`]). Auto asks, as Ask first
/// does. None: the run passes none.
pub(crate) fn tool_permission(mode: PermissionMode) -> Option<&'static str> {
    match mode {
        PermissionMode::Default | PermissionMode::AcceptEdits | PermissionMode::Auto => Some("request-review"),
        PermissionMode::Plan => None,
        PermissionMode::BypassPermissions => Some("always-proceed"),
    }
}

/// The session's mode for agy's mode `id`, as a session's answer names the one it runs. None for one it does not name
/// or this client does not know: it may be looser than Ask first, so the requested mode is always sent ([`crate::acp`]).
pub(crate) fn mode_of(id: Option<&str>) -> Option<PermissionMode> {
    match id {
        Some("default") => Some(PermissionMode::Default),
        Some("accept-edits") => Some(PermissionMode::AcceptEdits),
        Some("auto") => Some(PermissionMode::Auto),
        Some("plan") => Some(PermissionMode::Plan),
        _ => None,
    }
}

/// The ACP mode to set for `mode`: T3 Code's fixed mapping (`antigravityPermissionMode`), in the ids agy 1.5.8
/// offers. Auto and Ask first set `default`. Auto-accept edits sets `accept-edits`. Full access sets T3's `yolo`,
/// which agy 1.5.8 does not offer: it sets `accept-edits`, and `always-proceed` ([`tool_permission`]) lets every
/// command run. agy's own `auto` is never set, so no plan or account can refuse it.
pub(crate) fn acp_mode_id(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default | PermissionMode::Auto => "default",
        PermissionMode::AcceptEdits | PermissionMode::BypassPermissions => "accept-edits",
        PermissionMode::Plan => "plan",
    }
}

impl Harness for Antigravity {
    /// The Sandbox switch and whether agy offers `auto` decide its flags ([`args`]).
    fn args(&self, start: &Start, run: &RunFacts) -> Vec<String> {
        args(start.mode, run.sandbox, run.auto_offered)
    }

    fn protocol(&self, start: &Start) -> Box<dyn Protocol> {
        Box::new(Driver::new(start, ACP_PROFILE))
    }

    /// agy reports no task controls over ACP.
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
            local_only: true,
            deny_reason_as_message: true,
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

    /// agy's levels run from low to xhigh ([`thought_level`]). A ceiling below them (Codex's `none` or `minimal`)
    /// becomes low. One above them (`max`, `ultra`) is no ceiling: every level agy has is under it.
    fn child_effort(&self, effort: Option<Effort>, ceiling: Option<Effort>) -> Option<Effort> {
        match ceiling {
            Some(Effort::Disabled | Effort::Minimal) => effort_under(effort, Some(Effort::Low)),
            Some(Effort::Max | Effort::Ultra) => effort,
            ceiling => effort_under(effort, ceiling),
        }
    }

    /// Offered when the model's own list is unknown: the levels of agy's `thought_level` that every agy model takes
    /// ([`thought_level`]). Only some take xhigh.
    fn usual_efforts(&self) -> &'static [Effort] {
        &[Effort::Low, Effort::Medium, Effort::High]
    }

    fn label(&self) -> &'static str {
        self.entry().identity.name
    }

    fn log_title(&self) -> &'static str {
        self.entry().identity.name
    }

    fn fault_component(&self) -> &'static str {
        "agent_spawn.antigravity"
    }

    fn native_model(&self, id: &str) -> String {
        self.provider_model_id(id)
    }

    fn answers_asks(&self) -> bool {
        false
    }

    /// No handoff carries an agy conversation.
    fn switches(&self) -> bool {
        false
    }

    /// agy runs on the person's Google plan with no Gateway route, so its sessions stay on this computer, as the CLI
    /// keeps no log of them.
    fn syncs(&self) -> bool {
        false
    }

    /// The flags follow what agy's last run said of `auto` ([`RunFacts`]); the ACP mode follows the running session's
    /// answer. Both read `run.auto_offered` here.
    fn mode_settings(&self, mode: PermissionMode, run: &RunFacts) -> ModeSettings {
        let mut settings = vec![("modeId", acp_mode_id(mode).to_string())];
        if let Some(permission) = tool_permission(mode) {
            settings.push(("toolPermission", permission.to_string()));
        }
        if run.sandbox {
            settings.push(("--sandbox", "on".to_string()));
        }
        ModeSettings(settings)
    }

    fn offered_modes(&self) -> &'static [PermissionMode] {
        &EVERY_MODE
    }

    fn default_mode(&self) -> PermissionMode {
        PermissionMode::Auto
    }

    fn read_plan(&self, probe: PlanProbe, depth: PlanDepth) -> Option<PlanFuture> {
        match depth {
            PlanDepth::Limits => Some(Box::pin(plans::antigravity(probe.program, probe.env))),
            PlanDepth::TierOnly => None,
        }
    }

    // ACP does not pass on the tier agy reports in its authenticate response.
    fn plan_tier(&self, _name: &str) -> Option<&'static str> {
        None
    }

    fn observed_plan(&self, _observation: &Value) -> PlanSeen {
        PlanSeen::Keep
    }

    fn allows_subscription_fork(&self) -> bool {
        true
    }

    /// agy takes local servers in ACP's `session/new` and `session/load`, and saves their environment in plain text in
    /// its conversation file: each starts through the launcher, and its values stay with the app. The built-in server
    /// is told it serves agy ([`crate::mcp_servers::ANTIGRAVITY_CLIENT`]): one type per tool property for its Gemini
    /// models, and the viewer link in a page answer, which agy does not forward to the app.
    fn mcp_servers(&self, servers: &[McpServer], values: &Values) -> McpDelivery {
        use crate::mcp_servers::{ANTIGRAVITY_CLIENT, CLIENT_VAR, session_servers};
        let mut session = session_servers(servers, values);
        session.builtin_env(CLIENT_VAR, ANTIGRAVITY_CLIENT);
        McpDelivery::Session(session)
    }

    /// agy takes its servers in its session, already through the launcher ([`McpDelivery::Session`]).
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
        Instructions::SessionTools
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

    fn launch_refusal(&self, via: Via, home: Option<&str>) -> Result<(), String> {
        use nexus_client::local_agents::{Login, antigravity_login};
        if via != crate::event::Via::Subscription {
            return Err("Antigravity runs on your Google AI plan only. Start the session on your plan.".into());
        }
        if antigravity_login(home) == Some(Login::ApiKey) {
            let dir = nexus_agents::Agent::Antigravity.entry().profile.dir;
            let settings = format!("~/{dir}/settings.json");
            return Err(format!(
                "Antigravity is set to use an API key: {settings} sets \"modelProvider\". Remove \"modelProvider\" from \
                 {settings}, then start the session again."
            ));
        }
        Ok(())
    }
}
