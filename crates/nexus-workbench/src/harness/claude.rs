//! Claude Code's session: its stream-json control protocol ([`crate::claude`]).

use nexus_agents::{Agent, AgentT, Claude};
use nexus_local_agents::{LocalAgentT, Login, login_with};
use serde_json::{Map, Value, json};

use super::{
    AccountFolders, AppTools, EVERY_MODE, Harness, Instructions, LEGACY_TASK_CONTROL_HINTS, LauncherDelivery,
    LinkedPolicy, McpDelivery, McpStatus, ModeSettings, NativeCommands, PlanDepth, PlanFuture, PlanProbe, PlanSeen,
    RevertCursor, RunFacts, SessionFeatures, TaskControlHints, effort_under,
};
use crate::driver::{Protocol, Start, semver};
use crate::event::{AgentCapabilities, Effort, PermissionMode, SessionAgent, Via};
use crate::mcp_servers::{self, McpServer, Values};
use crate::plans::{self, PlanUsage};

/// Claude Code's `--mcp-config` for the enabled servers. A local server is itself, with its declared values as `${KEY}`
/// from the environment; with `launcher`, it is `launcher --mcp-launch=<name>` with none. A remote server's headers
/// are `${NEXUS_MCP_<i>_<j>}` either way.
fn mcp_config(servers: &[McpServer], launcher: Option<&str>) -> Vec<String> {
    let mut config = Map::new();
    for (i, server) in mcp_servers::enabled(servers) {
        if let Some(command) = &server.command {
            let entry = match launcher {
                Some(program) => json!({
                    "type": "stdio", "command": program, "args": [mcp_servers::launch_arg(&server.name)], "env": {},
                }),
                None => {
                    let env: Map<_, _> = server
                        .secrets
                        .iter()
                        .map(|key| (key.clone(), Value::from(format!("${{{key}}}"))))
                        .collect();
                    json!({"type": "stdio", "command": command, "args": server.args, "env": env})
                }
            };
            config.insert(server.name.clone(), entry);
        } else if let Some(url) = &server.url {
            let headers: Map<_, _> = server
                .secrets
                .iter()
                .enumerate()
                .map(|(j, key)| {
                    let var = mcp_servers::header_var(i, j);
                    (key.clone(), Value::from(format!("${{{var}}}")))
                })
                .collect();
            config.insert(
                server.name.clone(),
                json!({"type": "http", "url": url, "headers": headers}),
            );
        }
    }
    // With "=" after the variadic option, it never consumes the next argument.
    if config.is_empty() {
        Vec::new()
    } else {
        vec![format!("--mcp-config={}", json!({ "mcpServers": config }))]
    }
}

async fn read_tier(probe: PlanProbe) -> PlanUsage {
    let PlanProbe {
        program,
        mut env,
        account,
        dir,
        cwd,
    } = probe;
    let cwd = cwd.unwrap_or_else(std::env::temp_dir);
    if let Some(dir) = dir {
        env.insert("CLAUDE_CONFIG_DIR".into(), dir.to_string_lossy().into_owned());
    }
    let result = tokio::task::spawn_blocking(move || login_with(Agent::Claude, &program, &env, &cwd)).await;
    let mut usage = PlanUsage::signed_out(SessionAgent::Claude, account);
    match result {
        Ok(Some((Login::Plan, plan, email))) => {
            usage.signed_in = true;
            usage.plan = plan;
            usage.email = email;
        }
        Ok(Some((Login::ApiKey | Login::SignedOut, _, _))) => {}
        Ok(Some((Login::Unknown, _, _)) | None) => {
            usage.error = Some("Claude Code did not report its sign-in status.".into());
        }
        Err(error) => usage.error = Some(error.to_string()),
    }
    usage
}

/// The oldest version whose task controls Nexus has recorded.
const CONTROLS: (u64, u64, u64) = (2, 1, 292);

const NATIVE_COMMANDS: NativeCommands = NativeCommands {
    compact: Some("compact"),
};

/// A Nexus mode as Claude Code's own permission mode: `--permission-mode` at launch, and `set_permission_mode` while it
/// runs. Claude Code's own modes decide what runs unasked, its classifier included in `auto`.
pub(crate) fn permission_mode(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "default",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::Plan => "plan",
        PermissionMode::Auto => "auto",
        PermissionMode::BypassPermissions => "bypassPermissions",
    }
}

impl Harness for Claude {
    fn args(&self, start: &Start, _run: &RunFacts) -> Vec<String> {
        let mut args = crate::claude::args(
            start.mode,
            start.model,
            start.effort,
            start.native_id,
            start.resume,
            start.rewind.filter(|_| start.resume),
            start.instructions,
            start.name,
        );
        if start.instructions.is_none()
            && let Some(prompt) = start.prompt
            && let Some(index) = args.iter().position(|arg| arg == "--append-system-prompt")
        {
            args[index + 1] = prompt.to_string();
        }
        if start.resume && start.fork {
            args.push("--fork-session".into());
        }
        if start.fast {
            args.extend(["--settings".into(), crate::claude::FAST_SETTINGS.into()]);
        }
        args
    }

    fn protocol(&self, _start: &Start) -> Box<dyn Protocol> {
        Box::new(crate::claude::Translator::default())
    }

    /// A foreground nested agent's `background_tasks` is not recorded: only commands move.
    fn capabilities(&self, version: Option<&str>) -> Option<AgentCapabilities> {
        let parsed = version.and_then(semver)?;
        let kinds = |kinds: &[&str]| kinds.iter().map(|kind| kind.to_string()).collect();
        (parsed >= CONTROLS).then(|| AgentCapabilities {
            version: version.map(String::from),
            stop: kinds(&["agent", "shell"]),
            background: kinds(&["shell"]),
            output: kinds(&["shell"]),
            interrupt_spares_agents: crate::claude::PER_TASK_STOP,
            ..AgentCapabilities::default()
        })
    }

    fn live_capabilities(&self, version: Option<&str>, reported_commands: Option<&[String]>) -> AgentCapabilities {
        let mut capabilities = self.capabilities(version).unwrap_or_default();
        capabilities.version = version.map(String::from);
        if let Some(commands) = reported_commands {
            NATIVE_COMMANDS.apply(&mut capabilities, commands);
        }
        capabilities
    }

    fn session_features(&self) -> SessionFeatures {
        SessionFeatures {
            native_commands: Some(NATIVE_COMMANDS),
            compact_instructions_refusal: None,
            live_rename: true,
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
            owes_turn_after_task: true,
            children_ready_to_count: false,
        }
    }

    fn child_effort(&self, effort: Option<Effort>, ceiling: Option<Effort>) -> Option<Effort> {
        match ceiling {
            Some(Effort::Disabled | Effort::Minimal) => effort_under(effort, Some(Effort::Low)),
            Some(Effort::Ultra) => effort,
            ceiling => effort_under(effort, ceiling),
        }
    }

    /// Offered when the model's own list is unknown: every level from Low to Max. Ultra is Codex's only.
    fn usual_efforts(&self) -> &'static [Effort] {
        &[Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh, Effort::Max]
    }

    fn label(&self) -> &'static str {
        self.entry().identity.name
    }

    fn log_title(&self) -> &'static str {
        self.entry().identity.name
    }

    fn fault_component(&self) -> &'static str {
        "agent_spawn.claude"
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

    /// The Sandbox switch is agy's; Claude Code reads only the mode.
    fn mode_settings(&self, mode: PermissionMode, _run: &RunFacts) -> ModeSettings {
        ModeSettings(vec![("--permission-mode", permission_mode(mode).into())])
    }

    fn offered_modes(&self) -> &'static [PermissionMode] {
        &EVERY_MODE
    }

    fn default_mode(&self) -> PermissionMode {
        PermissionMode::Auto
    }

    fn read_plan(&self, probe: PlanProbe, depth: PlanDepth) -> Option<PlanFuture> {
        match depth {
            PlanDepth::Limits => Some(Box::pin(plans::claude(
                probe.program,
                probe.env,
                probe.account,
                probe.dir,
            ))),
            PlanDepth::TierOnly => Some(Box::pin(read_tier(probe))),
        }
    }

    fn plan_tier(&self, name: &str) -> Option<&'static str> {
        Some(match name {
            "free" => "free",
            "pro" => "pro",
            "max" => "max",
            "team" => "team",
            "enterprise" => "enterprise",
            _ => "unknown",
        })
    }

    fn observed_plan(&self, observation: &Value) -> PlanSeen {
        match observation.get("plan").and_then(Value::as_str) {
            Some(plan) => PlanSeen::Remember(plan.to_owned()),
            None => PlanSeen::Keep,
        }
    }

    fn allows_subscription_fork(&self) -> bool {
        true
    }

    fn mcp_servers(&self, servers: &[McpServer], values: &Values) -> McpDelivery {
        McpDelivery::Args {
            args: mcp_config(servers, None),
            env: mcp_servers::launch_env(servers, values),
        }
    }

    /// Claude Code starts its stdio servers with its whole environment, the Nexus token included: each starts through
    /// the launcher, which removes the credential variables and adds the server's own values from the app.
    fn mcp_launcher(&self, program: &str, servers: &[McpServer], values: &Values) -> Option<LauncherDelivery> {
        Some(LauncherDelivery {
            args: mcp_config(servers, Some(program)),
            env: mcp_servers::header_env(servers, values),
        })
    }

    fn app_tools(&self) -> AppTools {
        AppTools::ClaudeOptions {
            reads_project_mcp: true,
            disallow_native_shell: true,
        }
    }

    fn mcp_status(&self) -> McpStatus {
        McpStatus {
            page_key: "mcpServers",
            status_keys: &["status"],
            reauthentication_failure: None,
        }
    }

    fn instructions(&self) -> Instructions {
        Instructions::All { file: true }
    }

    fn accounts(&self) -> Option<AccountFolders> {
        Some(AccountFolders {
            shared: &[
                "settings.json",
                "CLAUDE.md",
                "skills",
                "agents",
                "commands",
                "plugins",
                "projects",
            ],
            own_login: "Claude Code's own login",
            auto_rotate_by_default: true,
            moves_running_session: true,
        })
    }

    fn plan_payer(&self) -> Option<&'static str> {
        Some(self.entry().identity.plan)
    }

    fn revert_cursor(&self) -> RevertCursor {
        RevertCursor::LastKept
    }

    fn launch_refusal(&self, _via: Via, _home: Option<&str>) -> Result<(), String> {
        Ok(())
    }
}
