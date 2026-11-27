//! `nexus claude` and `nexus codex` (`agents.ts`): the real agent on the route the person chose (see
//! [`choose_route`]), its session recorded in Nexus Gateway, and its exit code passed through. `nexus agy` starts
//! Antigravity the same way, on the person's Google AI plan only: it has no Gateway route.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nexus_client::base::sha256_hex;
use serde_json::{Value, json};

use nexus_client::agent_entries::{Agent, AgentEntry, Platform};
use nexus_client::agent_setup::{SetupOptions, agent_entry, payer_label, route_of, setup_steps};
pub use nexus_client::agents::CODEX_KEY_ENV;
use nexus_client::agents::{
    LAUNCHER_VARS, LaunchOptions, MissingBinary, Route, agent_launch, is_auto_name, named_model, provider_model_id,
    replace_model, route_supported, route_variable, search_allowed,
};
use nexus_client::commit_trailer::{self, Surface};
use nexus_client::local_agents::agents::{CLAUDE_NO_CONVERSATION, FILE_CLOCK_SLACK, codex_conversation, local};
use nexus_client::local_agents::{LocalAgent, Login, command as agent_command, login_with};
use nexus_client::staff::is_employee;

use crate::Context;
use crate::antigravity_usage;
use crate::cli::{cli_client, cli_client_with_timeout};
use crate::client::enc;
use crate::config::{Config, create_private_dir, ui_url, write_private};
use crate::error::{CliError, Error, Result};
use crate::json::{js_string, n, obj, pretty, s, t};
use crate::session_log;
use crate::skills_delivery;
use crate::style::{Style, js_number, pad_end, quote, to_fixed};
use crate::usage_signals;
use nexus_workbench::event::{TurnStatus, Via};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LauncherArgs {
    pub json: bool,
    pub help: bool,
    /// The route Nexus's own flags asked for (`--subscription`, `--api`, `--via <route>`), as given.
    pub route: Option<String>,
    /// Two different routes were asked for.
    pub conflict: bool,
    pub team: Option<String>,
    /// `--recording [<id>|latest]`: start on a screen recording from Nexus for Mac ("latest" when no id follows).
    pub recording: Option<String>,
    /// `--no-skills`: start the agent without writing the person's skills into its skills folder.
    pub no_skills: bool,
    /// Everything for the agent, unchanged and in order.
    pub forwarded: Vec<String>,
}

/// A recording's id (its folder's name, a time: `2026-10-19T14-03-22`) or `latest`: what `--recording` may take.
pub fn is_recording_id(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    if value == "latest" {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() > 11
        && bytes[..10].iter().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                *b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
        && bytes[10] == b'T'
        && bytes[11..].iter().all(|b| b.is_ascii_digit() || *b == b'-')
}

/// The agent's arguments with a recording (`withRecording` in agents.ts), as its `LocalAgentT::with_recording` takes
/// the recording's prompt and contact sheet.
pub fn with_recording(agent: Agent, args: &[String], recording: &crate::recordings::Recording) -> Vec<String> {
    let prompt = crate::recordings::prompt(recording);
    let sheet = recording.path.join("contact-sheet.png");
    local(agent).with_recording(args, &prompt, &sheet)
}

/// What every coding session is told about Nexus pages (`PAGE_INSTRUCTIONS` in agents.ts; both are pinned to
/// page-instructions.txt). `nexus pages publish` and `update` print the viewer link the agent passes on.
pub const PAGE_INSTRUCTIONS: &str = "Nexus pages: when your response creates a significant, self-contained output of roughly 15 or more lines that is easier to view than read, publish it as a page when the user asks or when it would be useful. Write the output to an .html or .md file, then run `nexus pages publish <file>` and print the viewer link from its `View it:` line. For a tracker, a budget, a table to fill in or a spreadsheet, write a sheet instead: a .sheet.json file such as {\"tabs\":[{\"id\":\"t1\",\"name\":\"Budget\",\"cells\":{\"A1\":{\"value\":\"Cost\"},\"A2\":{\"value\":4200,\"format\":{\"number\":\"currency\"}},\"A3\":{\"formula\":\"=SUM(A2:A2)\"}}}]}, published the same way; it opens as a spreadsheet people edit together. For a design (screens, mockups, a UI to iterate on), mark each screen of an html or jsx page as an artboard, `<section data-artboard=\"Home\" data-width=\"1440\" data-height=\"900\">` (390 by 844 for a phone): the page then opens as a design canvas the reader zooms and pans, with the screens that share a parent in one row. Give a board `data-id=\"L1\"` and `data-status` (final, proposed or superseded), name its parts with `data-layer`, and group boards into canvas pages with `<section data-page=\"Mobile\">`. To offer alternatives to a board, add boards in a new row under its row, each with `data-status=\"proposed\"`, `data-variant-of=\"L1\"` (the original's data-id) and `data-note`, one line on what it tries: the reader keeps one, which replaces the original. Declare a few tweaks the reader tunes live: CSS variables on `:root` with a comment, such as `--gap: 24px; /* @tweak range 8 64 4 \"Gap\" */`, `--accent: #2F6FEB; /* @tweak color \"Accent\" */` or `--density: cozy; /* @tweak choice compact|cozy|roomy */`, and use them in the boards. Before you write a design, run `nexus design-system show` (or `show <name>`): it prints the organization's design system, the tag that links a page to it, its tokens as CSS custom properties and its notes. Put the tag in the design's `<head>`, and use its `var(--name)` values for colours, font sizes, spacing and radii; the page is served with them. After you publish a design, run `nexus pages check <page-id>`: it lists values off the tokens and text below WCAG AA contrast, and changes nothing. Design process: before you draw, say your plan in one line (the feeling, the palette with a job for each color, the type, the layout). Build one visual group at a time. After each write, run `nexus pages check <page-id>`, then look at a rendered board (`nexus pages export <page-id> --format png --board <board>`, or `nexus pages frame <page-id>`) before you continue, and fix overflow, clipped text and misaligned rows. Use the design system's tokens, never raw colors, sizes or spacing. Finish with a refinement pass against this looks-generated list and fix every hit: a headline with one recolored word; formula copy such as 'X, but for Y'; an eyebrow label on every section; boxes inside boxes; a floating toast or badge for decoration; fake proof (invented logos, numbers or quotes); flat surfaces with no light, depth or material; navy with electric purple, or gradients with heavy shadows; details that differ between like elements (radii, icon sizes, gaps). Say you looked only at what you rendered after your last change. For a slide deck (a talk, a pitch, a walkthrough), make each slide of an html or jsx page a `<section data-slide=\"Title\">` laid out for 1920 by 1080 (`data-width` and `data-height` change it), with its speaker notes in an `<aside data-notes>` inside it: the page then opens as a deck the reader steps through with the arrow keys and presents full screen, so write no navigation script of your own. For a plan, a brainstorm, a map or a sketch (a timeline with owners and dates, columns, a flow, a mind map, sticky notes), publish a board: write its layout as JSON to a .board.json file, such as `{\"layout\": \"timeline\", \"title\": \"Q1\", \"items\": [{\"id\": \"ga\", \"text\": \"GA\", \"owner\": \"Ana\", \"start\": \"2027-03-15\"}]}` (or `columns`, `flow`, `mindmap` or `grid`), and Nexus lays it out into shapes the team edits together. To change the plan, run `nexus pages update` with the changed layout: the shapes people drew stay. For requested changes to an existing page, write the revised file and run `nexus pages update <page-id> <file>` so it becomes a new version of the same page, then print the viewer link from its output. To check how a page renders, run `nexus pages frame <page-id>` and open the URL it prints with your own browser tool; it needs no sign-in and works for five minutes. Pages are private until shared. Use `nexus pages share` only when the user asks to share. Never paste a long page into chat when publishing it would be clearer.";

/// The agent's arguments with the page instructions (`withPages` in agents.ts), added as its
/// `LocalAgentT::with_instructions` adds instructions.
pub fn with_pages(agent: Agent, args: &[String]) -> Vec<String> {
    with_instructions(agent, args, PAGE_INSTRUCTIONS)
}

/// Connector discovery stays available through the explicit MCP setup or the authenticated CLI fallback.
fn session_instructions(employee: bool) -> String {
    let connectors = format!(
        "If this session exposes Nexus MCP tools: {} If those tools are absent, use the authenticated CLI: `nexus connectors --json` lists services and operation names. \
         For calls, start `nexus mcp` and send newline-delimited JSON-RPC over stdin: initialize, notifications/initialized, \
         then tools/call with the tool name and arguments. First call describeConnector for the service to inspect its operation schemas. Await each request response; notifications have no response. \
         This uses the saved Nexus sign-in and the same Gateway permissions. Do not read credentials or change global configuration. \
         Attaching Nexus MCP to the client is an optional setup improvement, not a prerequisite for this CLI fallback.",
        nexus_client::mcp::CONNECTOR_INSTRUCTIONS
    );
    let mut text = format!("{connectors}\n\n{PAGE_INSTRUCTIONS}");
    if employee {
        text.push_str(&format!("\n\n{}", commit_trailer::INSTRUCTIONS));
    }
    text
}

/// The agent's arguments with `text` added to its instructions, as [`with_pages`] adds the page instructions, by its
/// `LocalAgentT::with_instructions`. An agent that takes none ([`takes_instructions`]) gets its arguments unchanged.
pub fn with_instructions(agent: Agent, args: &[String], text: &str) -> Vec<String> {
    local(agent).with_instructions(args, text)
}

/// Nexus's own flags come first; the first argument that is not one of them, or everything after `--`, goes to the
/// agent unchanged. `nexus claude -- --help` shows Claude Code's own help.
pub fn parse_launcher_args(argv: &[String]) -> LauncherArgs {
    let mut parsed = LauncherArgs::default();
    let ask = |parsed: &mut LauncherArgs, route: &str| {
        if parsed.route.as_deref().is_some_and(|asked| asked != route) {
            parsed.conflict = true;
        }
        parsed.route = Some(route.to_string());
    };
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        if arg == "--" {
            parsed.forwarded = argv[i + 1..].to_vec();
            break;
        } else if arg == "--json" {
            parsed.json = true;
        } else if arg == "--no-color" {
        } else if arg == "--subscription" {
            ask(&mut parsed, "subscription");
        } else if arg == "--api" || arg == "--gateway" {
            ask(&mut parsed, "gateway");
        } else if arg == "--via" && i + 1 < argv.len() {
            i += 1;
            ask(&mut parsed, &argv[i]);
        } else if let Some(route) = arg.strip_prefix("--via=") {
            ask(&mut parsed, route);
        } else if arg == "--team" && i + 1 < argv.len() {
            i += 1;
            parsed.team = Some(argv[i].clone());
        } else if let Some(team) = arg.strip_prefix("--team=") {
            parsed.team = Some(team.to_string());
        } else if arg == "--recording" {
            if is_recording_id(argv.get(i + 1).map(String::as_str)) {
                i += 1;
                parsed.recording = Some(argv[i].clone());
            } else {
                parsed.recording = Some("latest".into());
            }
        } else if let Some(recording) = arg.strip_prefix("--recording=") {
            parsed.recording = Some(if recording.is_empty() { "latest" } else { recording }.to_string());
        } else if arg == "--no-skills" {
            parsed.no_skills = true;
        } else if arg == "--help" || arg == "-h" {
            parsed.help = true;
        } else {
            parsed.forwarded = argv[i..].to_vec();
            break;
        }
        i += 1;
    }
    parsed
}

/// A run's route and where it came from: `flag`, `env`, or Gateway's `preference`, `default` or `organization`.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteChoice {
    pub route: Route,
    pub from: String,
}

/// The route for one run (see the header); `entry` is Gateway's answer for the agent, when there is one.
pub fn choose_route(
    agent: Agent,
    flag: Option<Route>,
    variable_value: Option<&str>,
    entry: Option<&Value>,
) -> Result<RouteChoice> {
    let choice = |route: Route, from: &str| RouteChoice {
        route,
        from: from.to_string(),
    };
    if let Some(route) = flag {
        return Ok(choice(route, "flag"));
    }
    let value = variable_value
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    if !value.is_empty() {
        let Some(route) = Route::parse(&value) else {
            return Err(CliError::hint(
                format!("{} is {}, not a route.", route_variable(agent), quote(&value)),
                "Set it to gateway or subscription, or unset it to use your saved route (`nexus agents`).",
            )
            .into());
        };
        return Ok(choice(route, "env"));
    }
    if let Some(entry) = entry {
        let route = s(entry, "route").and_then(Route::parse).unwrap_or(Route::Gateway);
        return Ok(choice(route, s(entry, "from").unwrap_or("default")));
    }
    Ok(choice(Route::Subscription, "default"))
}

/// Refuses a route the agent's entry does not support ([`route_supported`]: Gateway without a binding), however it
/// was chosen: a flag, the variable, or a saved or organization route in Gateway's answer (`requireRoute` in
/// agents.ts). A flag or the variable skips `/v1/agents/access`, so the entry decides, not Gateway's answer. A usage
/// error (exit 2).
pub fn require_route(entry: &AgentEntry, route: Route) -> Result<()> {
    if !route_supported(entry, route) {
        return Err(CliError::usage(entry.route_unsupported(), None)
            .with_code("route_unsupported")
            .into());
    }
    Ok(())
}

/// What a route choice says in a launch line: nothing for a flag or the default.
fn source_note(agent: Agent, choice: &RouteChoice) -> Option<String> {
    match choice.from.as_str() {
        "env" => Some(format!("from {}", route_variable(agent))),
        "preference" => Some("your saved route (nexus agents)".into()),
        _ => None,
    }
}

/// The warning when Gateway's answer, not a flag or the variable, puts a run on the organization's budget. Gateway's
/// default is the plan, so a default of `gateway` means the organization turned subscription mode off.
fn fallback_line(agent: Agent, choice: &RouteChoice, plan_name: &str) -> Option<String> {
    if choice.route != Route::Gateway {
        return None;
    }
    let command = agent.command();
    let why = match choice.from.as_str() {
        "organization" | "default" => "Your organization turned subscription mode off.".to_string(),
        "preference" => {
            format!(
                "Your saved route is gateway; to use your {plan_name} plan: nexus agents use {command} subscription."
            )
        }
        _ => return None,
    };
    Some(format!("Through Gateway: paid from your organization's budget. {why}"))
}

fn usage(agent: Agent) -> String {
    let command = agent.command();
    if agent.entry().billing.gateway.is_none() {
        return format!(
            "nexus {command} [--json] [--subscription] [--team <slug>] [--] [{} args…]",
            agent.entry().identity.executable
        );
    }
    format!(
        "nexus {command} [--json] [--subscription | --api | --via <route>] [--team <slug>] [--recording [<id>|latest]] [--no-skills] [--] [{command} args…]"
    )
}

/// Whether the agent takes Nexus's instructions for a run (the pages, the commit trailer): see [`with_instructions`].
fn takes_instructions(agent: Agent) -> bool {
    local(agent).takes_instructions()
}

/// How often the CLI tells Gateway the agent still runs; Gateway shows a session with none for 15 minutes as lost.
/// `NEXUS_SESSION_HEARTBEAT_MS` sets another interval (tests).
const HEARTBEAT: Duration = Duration::from_secs(5 * 60);

/// Runs the shared async model discovery from this synchronous CLI, including callers already inside a Tokio runtime.
fn installed_models(
    agent: Agent,
    bin: &Path,
    path_env: &str,
    env: &HashMap<String, String>,
) -> Vec<nexus_client::local_agents::models::Model> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()?;
                runtime.block_on(local(agent).models(bin, path_env, None, Some(env)))
            })
            .join()
            .ok()
            .flatten()
            .unwrap_or_default()
    })
}

fn catalog_model_key(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).replace('.', "-").to_lowercase()
}

/// Known catalog model ids from `/v1/models?status=all`, so `/v1/agents/route` only receives ids Gateway knows.
fn known_catalog_models(core: &crate::client::Client) -> std::collections::HashSet<String> {
    let Ok(models) = core.get("/v1/models?status=all") else {
        return std::collections::HashSet::new();
    };
    let mut known = std::collections::HashSet::new();
    for model in models.get("data").and_then(Value::as_array).into_iter().flatten() {
        for field in ["id", "provider_model"] {
            if let Some(id) = model.get(field).and_then(Value::as_str) {
                known.insert(catalog_model_key(id));
            }
        }
    }
    known
}

/// Routes `auto` through `POST /v1/agents/route`, preferring the person's plan on `Route::Subscription`.
/// Returns `Ok(Some(provider_model))` when routed, `Ok(None)` to fall back to `/v1/agents/check` (older Gateway,
/// unknown catalog, or `stop`), and `Err` when a plan launch would require paid fallback or subscriptions are off.
fn route_auto(
    core: &crate::client::Client,
    agent: Agent,
    route: Route,
    team: Option<&str>,
    bin: &Path,
    env: &HashMap<String, String>,
) -> Result<Option<String>> {
    use nexus_client::auto_route;
    let on_plan = route == Route::Subscription;
    let path_env = env.get("PATH").map(String::as_str).unwrap_or_default();
    let plan = if on_plan {
        let installed = installed_models(agent, bin, path_env, env);
        let known = known_catalog_models(core);
        Some(auto_route::plan_models(
            installed
                .iter()
                .map(|m| (m.id.as_str(), m.resolved_id.as_deref(), m.name.as_str())),
            |id| known.contains(&catalog_model_key(id)),
            None,
            0,
        ))
    } else {
        None
    };
    if !auto_route::routable(on_plan, plan.as_deref()) {
        return Ok(None);
    }
    let mut body = auto_route::body(agent.as_str(), plan.as_deref());
    if let Some(team) = team.filter(|t| !t.is_empty()) {
        body["team"] = json!(team);
    }
    let Ok(raw) = core.post("/v1/agents/route", body) else {
        return Ok(None);
    };
    let Some(answer) = auto_route::parse(&raw) else {
        return Ok(None);
    };
    let command = agent.entry().identity.id;
    let plan_name = agent.entry().identity.plan;
    match answer {
        auto_route::Answer::UsePlan { model, .. } => Ok(Some(model)),
        auto_route::Answer::AskToPay { model, .. } if !on_plan => Ok(Some(model)),
        auto_route::Answer::AskToPay { name, .. } => {
            if let Ok(settings) = core.get("/v1/orgs/settings")
                && settings.get("agentSubscriptions").and_then(Value::as_bool) == Some(false)
            {
                return Err(CliError::hint(
                    "Your organization does not allow sessions on a personal subscription.",
                    format!("Run `nexus {command} --via gateway --model auto`, or ask an admin to enable `agentSubscriptions`."),
                )
                .with_code("subscriptions_disabled")
                .into());
            }
            Err(CliError::hint(
                format!(
                    "Your {plan_name} plan has no room left for this task. Auto would run {} on your organization's API.",
                    name.rsplit('/').next().unwrap_or(&name)
                ),
                format!("Run `nexus {command} --via gateway --model auto`, or pass `--model <id>`."),
            )
            .into())
        }
        auto_route::Answer::Stop { .. } => Ok(None),
    }
}

/// Runs the shared async probe from this synchronous CLI, including callers already inside a Tokio runtime.
fn model_catalog(agent: Agent, bin: &Path, env: &HashMap<String, String>, dir: &Path) -> Option<PathBuf> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()?;
                runtime.block_on(nexus_client::local_agents::agents::local(agent).model_catalog(bin, env, dir))
            })
            .join()
            .ok()
            .flatten()
    })
}

/// The arguments for `bin`, with a system prompt over several lines moved to a file in `dir` when `bin` is a batch file.
/// npm installs Claude Code on Windows as `claude.cmd`, and Rust refuses to start a batch file with a line break in an
/// argument (CVE-2024-24576). A recording's prompt, or the pages joined to the person's own, has them. Claude Code reads
/// the file through `--append-system-prompt-file`. Any other program, or a prompt on one line, keeps its arguments; so
/// does a prompt that can't be written, and the start then fails with Rust's own error.
pub fn prompt_in_file(bin: &Path, args: Vec<String>, dir: &Path) -> Vec<String> {
    prompt_in_file_for_store(bin, args, dir, None)
}

fn prompt_in_file_for_store(
    bin: &Path,
    args: Vec<String>,
    dir: &Path,
    memory: Option<&nexus_agent_memory::Store>,
) -> Vec<String> {
    let batch = bin
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    if !batch {
        return args;
    }
    let own = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (at, prompt, joined) = match args[..own].iter().enumerate().find_map(|(i, arg)| {
        if arg == "--append-system-prompt" {
            args.get(i + 1).filter(|_| i + 1 < own).map(|p| (i, p.as_str(), false))
        } else {
            arg.strip_prefix("--append-system-prompt=").map(|p| (i, p, true))
        }
    }) {
        Some(found) if found.1.contains(['\r', '\n']) => found,
        _ => return args,
    };
    let path = if let Some(store) = memory {
        match store.prompt_file(prompt) {
            Ok(path) => path,
            Err(_) => return args,
        }
    } else {
        let name = format!("system-prompt-{}.md", &sha256_hex(&[prompt.as_bytes()])[..16]);
        let path = dir.join(&name);
        let written = || std::fs::read(&path).is_ok_and(|bytes| bytes == prompt.as_bytes());
        if !written() {
            // Renamed into place, so an agent started at the same moment never reads half a file.
            let partial = dir.join(format!("{name}.{}.tmp", std::process::id()));
            let saved = create_private_dir(dir)
                .and_then(|()| write_private(&partial, prompt.as_bytes()))
                .and_then(|()| std::fs::rename(&partial, &path));
            if saved.is_err() {
                let _ = std::fs::remove_file(&partial);
                if !written() {
                    return args;
                }
            }
        }
        path
    };
    let file = path.to_string_lossy().into_owned();
    let mut out = args;
    if joined {
        out[at] = format!("--append-system-prompt-file={file}");
    } else {
        out.splice(at..at + 2, ["--append-system-prompt-file".to_string(), file]);
    }
    out
}

fn launch_instructions(
    agent: Agent,
    employee: bool,
    cwd: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> std::io::Result<(String, Option<nexus_agent_memory::Store>)> {
    let mut text = session_instructions(employee);
    if !takes_instructions(agent) {
        return Ok((text, None));
    }
    let Ok(store) = nexus_agent_memory::Store::from_env(env) else {
        return Ok((text, None));
    };
    let guidance = nexus_agent_memory::tools::launch_guidance(crate::mcp::MEMORY_SERVER, agent == Agent::Claude);
    let notes = store.instructions(cwd, Some(&guidance))?;
    if notes.is_empty() {
        return Ok((text, None));
    }
    text.push_str("\n\n");
    text.push_str(&notes);
    Ok((text, Some(store)))
}

fn configure_memory_launch(
    memory: Option<&nexus_agent_memory::Store>,
    agent: Agent,
    executable: Option<&Path>,
    cwd: &Path,
    session_id: &str,
    forwarded: &mut Vec<String>,
    env: &mut HashMap<String, String>,
) -> Result<()> {
    if let Some(store) = memory {
        let program = executable
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_exe().ok())
            .ok_or_else(|| CliError::new("Could not find the Nexus executable for shared memory."))?;
        let mcp = crate::mcp::memory_launch_args(agent, &program, store.root(), cwd, session_id);
        forwarded.splice(0..0, mcp);
        env.insert(nexus_agent_memory::tools::AGENT_ENV.into(), agent.as_str().into());
        env.insert(nexus_agent_memory::tools::SESSION_ENV.into(), session_id.to_string());
    }
    Ok(())
}

/// A catalog entry can ship before its local session integration.
pub(crate) fn launch_available(agent: Agent) -> bool {
    agent.entry().session.cli_launcher
}

pub fn launch_agent(agent: Agent, argv: &[String], ctx: &Context, config: &Config, style: &Style) -> Result<i32> {
    if let Some(reason) = agent.entry().platform_refusal(Platform::current()) {
        return Err(CliError::usage(reason, None).into());
    }
    if !launch_available(agent) {
        return Err(CliError::usage(
            format!(
                "{} launch is unavailable in this Nexus build.",
                agent.entry().identity.name
            ),
            None,
        )
        .into());
    }
    let args = parse_launcher_args(argv);
    let agent_label = agent.entry().identity.name;
    let command = agent.command();
    let cwd = ctx.cwd.clone();
    let client = |key: &str| cli_client(&config.url, Some(key));
    let catalog = agent_entry(None, agent);
    let plan = route_of(&catalog, Route::Subscription).cloned().unwrap_or_default();
    let plan_name = js_string(plan.get("plan"));
    let plan_login = js_string(plan.get("login"));

    if args.help {
        let (policy, access) = match config.api_key.as_deref() {
            Some(key) => {
                let core = client(key);
                // No policy applies to an agent without a Gateway route, so its help does not ask for one.
                let policy = agent
                    .entry()
                    .billing
                    .gateway
                    .and_then(|_| core.get("/v1/agents/policy").ok())
                    .and_then(|r| r.get("agents").and_then(|agents| agents.get(agent.as_str())).cloned())
                    .filter(|p| !p.is_null());
                let access = core
                    .get("/v1/agents/access")
                    .ok()
                    .map(|answer| agent_entry(Some(&answer), agent));
                (policy, access)
            }
            None => (None, None),
        };
        if args.json {
            ctx.io.out(&pretty(&obj(vec![
                ("agent", Some(json!(agent.as_str()))),
                ("usage", Some(json!(usage(agent)))),
                ("core", Some(json!(config.url))),
                ("policy", policy),
                ("route", access.as_ref().and_then(|a| a.get("route").cloned())),
                ("routeFrom", access.as_ref().and_then(|a| a.get("from").cloned())),
            ])));
        } else {
            for line in launcher_help(style, agent, &config.url, policy.as_ref(), access.as_ref()) {
                ctx.io.out(&line);
            }
        }
        return Ok(0);
    }

    if args.conflict {
        return Err(CliError::hint(
            format!("Name one route for {agent_label}: --subscription, --api and --via ask for different ones."),
            format!("`nexus {command} --via gateway|subscription`."),
        )
        .into());
    }
    if args.recording.is_some() && !local(agent).takes_recording() {
        return Err(CliError::usage(
            format!("--recording is not available for {agent_label}."),
            Some("Run without --recording."),
        )
        .into());
    }
    let flag = match args.route.as_deref() {
        None => None,
        Some(asked) => Some(Route::parse(asked).ok_or_else(|| {
            CliError::hint(
                format!("{} is not a route.", quote(asked)),
                "Use --via gateway or --via subscription (or --subscription, --api).",
            )
        })?),
    };

    let found = nexus_client::agents::find_agent(agent, &ctx.env, cfg!(windows), &cwd).map_err(
        |MissingBinary { variable, path }| {
            CliError::hint(
                format!("{variable} points to {}, which does not exist.", path.display()),
                format!(
                    "Unset it to use the `{}` on your PATH, or point it at the {} binary.",
                    agent.entry().identity.executable,
                    agent.entry().identity.name
                ),
            )
        },
    )?;
    let Some(bin) = found else {
        return Err(CliError::hint(
            format!(
                "nexus {command} runs {agent_label}, and `{}` is not on your PATH.",
                agent.entry().identity.executable
            ),
            match agent.entry().billing.gateway {
                Some(_) => format!(
                    "Install it with `{}`, then run `nexus {command}` again (Nexus does not install it for you). To use your own {plan_name} plan instead of Gateway, also run `{plan_login}` once. `nexus agents` shows what is set up.",
                    agent.entry().install.command
                ),
                None => format!(
                    "Install it with `{}`, then run `nexus {command}` again (Nexus does not install it for you). `nexus agents` shows what is set up.",
                    agent.entry().install.command
                ),
            },
        )
        .into());
    };
    let Some(api_key) = config.api_key.as_deref() else {
        if crate::byo::enabled(ctx) {
            return launch_without_account(agent, &args, flag, &bin, ctx, config, style);
        }
        return Err(CliError::hint(
            "You are not signed in to Nexus.",
            format!(
                "Run `nexus auth login --url {}` first: Nexus records every {agent_label} session, on Gateway or on your own plan.",
                config.url
            ),
        )
        .into());
    };
    let core = client(api_key);

    // Gateway is asked for the saved route only when neither a flag nor the variable decides.
    let variable_value = ctx.var(&route_variable(agent));
    let entry = if flag.is_some() || variable_value.as_deref().is_some_and(|v| !v.trim().is_empty()) {
        None
    } else {
        core.get("/v1/agents/access")
            .ok()
            .map(|answer| agent_entry(Some(&answer), agent))
    };
    let choice = choose_route(agent, flag, variable_value.as_deref(), entry.as_ref())?;
    // Gateway's answer puts an agent without a Gateway route on the gateway route only when subscription mode is off: it
    // refuses saving that route for it. Its plan is refused too (`subscriptions_disabled`), so the fix is the
    // organization's setting, not `--subscription`.
    let from_gateway = !matches!(choice.from.as_str(), "flag" | "env");
    if choice.route == Route::Gateway && from_gateway && agent.entry().billing.gateway.is_none() {
        return Err(CliError::hint(
            format!("{agent_label} is not available: subscription mode is off for your organization."),
            "An owner or admin turns it on with `nexus org settings set agentSubscriptions true`.",
        )
        .with_code("subscriptions_disabled")
        .into());
    }
    require_route(agent.entry(), choice.route)?;
    let route = choice.route;
    let asked = named_model(agent, &args.forwarded);
    // Gateway chooses `auto` within the organization's policies, and no policy applies to an agent without a Gateway
    // route (`/v1/agents/check` refuses it), so its model is the one the person names, or its own default.
    let plan_only = agent.entry().billing.gateway.is_none();
    if plan_only && is_auto_name(asked.as_deref()) {
        return Err(CliError::usage(
            format!(
                "--model {} is not available for {agent_label}.",
                asked.as_deref().unwrap_or_default()
            ),
            Some(&format!(
                "Run with --model <id> (`{} models` lists them), or without --model.",
                agent.entry().identity.executable
            )),
        )
        .into());
    }
    // The account the plan's run is on, for the line before it starts: a wrong profile shows at once.
    let mut account = None;
    if route == Route::Subscription {
        // Signed in to the plan? A run that is not would stop at the agent's own login, or bill something else.
        // `None`: the agent has no sign-in check, and nothing ran.
        let checked = login_with(agent, &bin, &ctx.env, &cwd);
        // Claude Code's check names the plan; keep its tier for this session and the next ones.
        if agent == Agent::Claude
            && let Some((login, plan, _)) = &checked
        {
            match login {
                Login::Plan => {
                    usage_signals::save_plan(&ctx.config_path, agent, Some(plan.as_deref().unwrap_or("unknown")))
                }
                Login::ApiKey | Login::SignedOut => usage_signals::save_plan(&ctx.config_path, agent, None),
                Login::Unknown => {}
            }
        }
        if let Some(warning) = check_plan_login(agent, checked.as_ref().map(|(login, _, _)| *login))?
            && !args.json
        {
            ctx.io.err(&format!("{} {warning}", style.warn));
        }
        account = checked.and_then(|(_, _, signed_in_as)| signed_in_as);
    }
    if let Some(line) = fallback_line(agent, &choice, &plan_name)
        && !args.json
    {
        ctx.io.err(&format!("{} {line}", style.warn));
    }

    // --team, else NEXUS_TEAM; Gateway checks the person is in it.
    let team = args
        .team
        .clone()
        .or_else(|| ctx.var("NEXUS_TEAM").filter(|t| !t.is_empty()));
    // `--recording`: found before anything is opened, so a wrong id stops here.
    let recording = match args.recording.as_deref() {
        Some(which) => Some(crate::recordings::find(ctx, Some(which))?),
        None => None,
    };
    let mut forwarded = match &recording {
        Some(recording) => with_recording(agent, &args.forwarded, recording),
        None => args.forwarded.clone(),
    };
    // The commits the session makes in a nexus-xyz repository name Nexus as a co-author. Only for Nexus's
    // own people while Nexus tries it on its own work first. The agent is told, or it offers to undo it.
    // An agent that takes no instructions gets no trailer either: it would not know where the trailer came from.
    let employee = core.get("/v1/tenants/me").is_ok_and(|me| is_employee(&me)) && takes_instructions(agent);
    let (instructions, memory) = launch_instructions(agent, employee, &cwd, &ctx.env_fn()).unwrap_or_else(|error| {
        ctx.io
            .err(&format!("{} Could not load shared memory: {error}", style.warn));
        (session_instructions(employee), None)
    });
    forwarded = with_instructions(agent, &forwarded, &instructions);
    let mut named = asked.clone();
    let model: Option<String>;
    let check_body = |model: Option<&str>| {
        obj(vec![
            ("agent", Some(json!(agent.as_str()))),
            ("model", model.map(|m| json!(m))),
            ("team", team.as_ref().filter(|t| !t.is_empty()).map(|t| json!(t))),
        ])
    };
    // A subscription run chooses its model the same way: the choice is a policy lookup, not a call through Gateway.
    if plan_only {
        model = asked.clone();
    } else if is_auto_name(asked.as_deref()) {
        let provider_model = match route_auto(&core, agent, route, team.as_deref(), &bin, &ctx.env)? {
            Some(chosen) => chosen,
            None => {
                let check = core.post("/v1/agents/check", check_body(Some("nexus/auto")))?;
                let provider_model = s(&check, "providerModel").filter(|m| !m.is_empty());
                let (true, Some(provider_model)) = (t(&check, "allowed"), provider_model) else {
                    return Err(CliError::hint(
                        format!(
                            "Gateway has no model for {agent_label}: {}.",
                            js_string(check.get("reason"))
                        ),
                        format!("See `nexus policy test --agent {agent}`."),
                    )
                    .into());
                };
                provider_model.to_string()
            }
        };
        model = Some(provider_model.clone());
        forwarded = replace_model(agent, &forwarded, &provider_model);
        named = model.clone();
    } else if asked.is_some() {
        model = asked.clone();
    } else {
        let check = core.post("/v1/agents/check", check_body(None)).ok();
        model = check.and_then(|check| {
            let chosen = check
                .pointer("/defaultFrom/level")
                .and_then(Value::as_str)
                .is_some_and(|level| ["organization", "team", "agent"].contains(&level));
            if !(chosen && t(&check, "allowed")) {
                return None;
            }
            s(&check, "providerModel").map(String::from).or_else(|| {
                s(&check, "model")
                    .filter(|m| !m.is_empty())
                    .map(|m| provider_model_id(agent, m))
            })
        });
    }
    let person = current_user()
        .or_else(|| ctx.var("USER"))
        .or_else(|| ctx.var("USERNAME"))
        .filter(|u| !u.is_empty());
    let base = cwd
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    // On the plan Codex keeps its own provider, and OpenAI's backend reads Responses Lite itself.
    let model_catalog = if route == Route::Gateway {
        model_catalog(
            agent,
            &bin,
            &ctx.env,
            ctx.config_path.parent().unwrap_or(Path::new(".")),
        )
    } else {
        None
    };
    // The person's own plan tier, as this machine last read it (never the raw string); read again on its
    // own thread when it is a day old.
    let plan = usage_signals::saved_plan(&ctx.config_path, agent);
    usage_signals::refresh_plan(&ctx.config_path, agent, &bin, &ctx.env);
    let session = core.post(
        "/v1/agents/sessions",
        obj(vec![
            ("agent", Some(json!(agent.as_str()))),
            ("via", Some(json!(route.via()))),
            ("model", model.as_ref().map(|m| json!(m))),
            ("person", person.map(|p| json!(p))),
            ("cwd", Some(json!(base))),
            ("team", team.as_ref().filter(|t| !t.is_empty()).map(|t| json!(t))),
            ("plan", plan.map(|p| json!(p))),
        ]),
    )?;
    if agent == Agent::Cursor && session["id"].as_str().is_none_or(str::is_empty) {
        return Err(CliError::usage("Nexus Gateway did not return a session ID.", None).into());
    }
    let session_id = js_string(session.get("id"));
    // The person's skills: written once the session exists (their records name it) and before the agent starts. A failure
    // stops the launch and closes the session, as an agent that could not start does; `--no-skills` starts without them.
    let skills = if args.no_skills {
        None
    } else {
        let delivered = skills_delivery::deliver(ctx, &core, agent).and_then(|delivery| {
            if let Some(delivery) = &delivery {
                skills_delivery::record(&core, agent, &session_id, delivery)?;
            }
            Ok(delivery)
        });
        match delivered {
            Ok(delivery) => delivery,
            Err(error) => {
                let closed = core.post(
                    &format!("/v1/agents/sessions/{}/end", enc(&session_id)),
                    obj(vec![("exitCode", Some(json!(1))), ("durationMs", Some(json!(0)))]),
                );
                if let Err(closing) = closed {
                    ctx.io.err(&format!(
                        "{} Could not close session {session_id} in Nexus Gateway: {}",
                        style.warn,
                        describe(&closing)
                    ));
                }
                return Err(error);
            }
        }
    };
    // The conversation's own id, so Nexus UI can show the command that resumes it.
    let conversation = match agent {
        Agent::Claude => claude_conversation(&forwarded),
        Agent::Codex => codex_resumed(&forwarded),
        Agent::Antigravity => antigravity_resumed(&forwarded),
        Agent::Cursor => resumed_with(&forwarded, "--resume"),
        Agent::Opencode => Conversation::Unknown,
    };
    if let Conversation::New(id) = &conversation {
        forwarded.splice(0..0, ["--session-id".to_string(), id.clone()]);
    }
    let session_team = s(&session, "team").filter(|t| !t.is_empty()).map(String::from);
    // Codex through Gateway searches when the organization lets it. A settings read that fails keeps search off.
    let web_search = agent == Agent::Codex
        && route == Route::Gateway
        && core
            .get("/v1/orgs/settings")
            .is_ok_and(|settings| search_allowed(&settings));
    let mut env = ctx.env.clone();
    configure_memory_launch(
        memory.as_ref(),
        agent,
        ctx.exe_path.as_deref(),
        &cwd,
        &session_id,
        &mut forwarded,
        &mut env,
    )?;
    if employee {
        commit_trailer::mark(&mut env, Surface::Cli);
    }
    let (launch_args, mut launch_env) = agent_launch(
        agent,
        LaunchOptions {
            route,
            core_url: &config.url,
            api_key,
            session_id: &session_id,
            team: session_team.as_deref(),
            model: if named.is_some() { None } else { model.as_deref() },
            model_catalog: model_catalog.as_deref(),
            web_search,
            args: forwarded,
            env,
        },
    );
    match agent {
        // Pages replace Claude Code's own artifacts in a Nexus session.
        Agent::Claude => {
            launch_env.insert("CLAUDE_CODE_DISABLE_ARTIFACT".into(), "1".into());
        }
        Agent::Codex | Agent::Antigravity | Agent::Cursor | Agent::Opencode => {}
    }
    let launch_args = prompt_in_file_for_store(
        &bin,
        launch_args,
        ctx.config_path.parent().unwrap_or(Path::new(".")),
        memory.as_ref(),
    );
    if !args.json {
        ctx.io.err(&style.dim(&launch_line(
            style,
            agent,
            &choice,
            &session,
            &config.url,
            model.is_some(),
            account.as_deref(),
        )));
        if let Some(text) = skills
            .as_ref()
            .and_then(|delivery| skills_delivery::line(style, agent, delivery))
        {
            ctx.io.err(&style.dim(&text));
        }
    }

    let mut log_start = session_log_start(
        agent,
        ctx,
        config,
        style,
        args.json,
        SessionLogRun {
            route,
            api_key,
            session_id: &session_id,
            model: named.as_deref().or(model.as_deref()),
            conversation: &conversation,
            env: &launch_env,
        },
    );

    // With Phone control on, Codex's TUI runs as a client of an app-server the CLI owns, so the phone can
    // direct the run. Otherwise, or when that server cannot run, Codex runs as before.
    #[cfg(unix)]
    let codex_server = if agent == Agent::Codex && log_start.is_some() {
        crate::codex_remote::for_run(ctx, &core, &config.url, &bin, &launch_args, &launch_env, &cwd, |line| {
            if !args.json {
                ctx.io.err(&style.dim(line));
            }
        })
    } else {
        None
    };
    #[cfg(unix)]
    let (launch_args, codex_socket) = match &codex_server {
        Some(server) => (
            crate::codex_remote::tui_args(server.socket(), &launch_args),
            Some(server.socket().to_path_buf()),
        ),
        None => (launch_args, None),
    };
    #[cfg(not(unix))]
    let codex_socket = None;
    if let Some(start) = log_start.as_mut() {
        start.codex = codex_socket;
    }
    let log = log_start.map(session_log::Log::start);
    // Antigravity's calls are counted from its conversation file after it exits, so what the file held before it
    // starts is read now.
    let agy_before = antigravity_usage::counts_from_conversation(agent).then(|| {
        antigravity_usage::Before::take(
            agent_profile(ctx, agent).map(|profile| profile.join("conversations")),
            match &conversation {
                Conversation::Known(id) => Some(id.as_str()),
                Conversation::New(_) | Conversation::Unknown => None,
            },
            continues_latest(&launch_args),
        )
    });
    let started = Instant::now();
    let started_at = std::time::SystemTime::now();
    let every = ctx
        .var("NEXUS_SESSION_HEARTBEAT_MS")
        .and_then(|ms| ms.parse().ok())
        .map_or(HEARTBEAT, Duration::from_millis);
    let beating = Heartbeat::start(&core, &session_id, every);
    // Active minutes while the person uses the terminal (Unix; see `usage_signals`).
    let activity = usage_signals::ActivityBeat::start(&config.url, api_key);
    let ran = run(&bin, &launch_args, &launch_env, &cwd, ctx.interactive);
    #[cfg(unix)]
    drop(codex_server);
    activity.stop();
    beating.stop();
    let (result, launch_error) = match ran {
        Ok(result) => (result, None),
        Err(error) => {
            let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
                126
            } else {
                127
            };
            usage_signals::record_error(
                &ctx.config_path,
                usage_signals::ErrorKind::Agent,
                "agent_spawn",
                spawn_code(error.kind()),
            );
            (
                (code, None),
                Some(CliError::hint(
                    format!("Could not start {}: {error}.", bin.display()),
                    format!(
                        "Check that {agent_label} is installed and runs (`{} --version`).",
                        agent.entry().identity.executable
                    ),
                )),
            )
        }
    };
    let (exit_code, signal) = result;
    let elapsed = started.elapsed().as_millis() as f64;
    let conversation = match conversation {
        Conversation::New(id) | Conversation::Known(id) => Some(id),
        Conversation::Unknown => match agent {
            Agent::Claude | Agent::Cursor | Agent::Opencode => None,
            Agent::Codex => agent_profile(ctx, agent).and_then(|home| codex_conversation(&home, &cwd, started_at)),
            Agent::Antigravity => agent_profile(ctx, agent)
                .and_then(|profile| antigravity_conversation(&profile, started_at, continues_latest(&launch_args))),
        },
    };
    // Reported before `/end`, with the key that opened the session. Whatever happens, the session still ends.
    let agy_usage = match &agy_before {
        Some(before) if launch_error.is_none() => {
            let (url, key) = (config.url.clone(), api_key.to_string());
            let connect = move |timeout| cli_client_with_timeout(&url, Some(&key), timeout);
            Some(antigravity_usage::report(
                connect,
                &session_id,
                before,
                conversation.as_deref(),
            ))
        }
        _ => None,
    };
    let ended = match core.post(
        &format!("/v1/agents/sessions/{}/end", enc(&session_id)),
        obj(vec![
            ("exitCode", Some(json!(exit_code))),
            ("durationMs", Some(json!(elapsed))),
            ("signal", signal.as_ref().map(|s| json!(s))),
            ("agentSessionId", conversation.clone().map(|id| json!(id))),
        ]),
    ) {
        Ok(ended) => Some(ended),
        Err(error) => {
            ctx.io.err(&format!(
                "{} Could not close session {session_id} in Nexus Gateway: {}",
                style.warn,
                describe(&error)
            ));
            None
        }
    };
    let synced = log.map(|log| {
        let interrupted = signal.is_some() || exit_code == 130 || exit_code == 143 || exit_code == 0;
        log.finish(session_log::Finish {
            status: if interrupted {
                TurnStatus::Interrupted
            } else {
                TurnStatus::Error
            },
            conversation: conversation.clone(),
        })
    });
    if let Some(error) = launch_error {
        return Err(error.into());
    }

    let ui = config
        .ui_url
        .clone()
        .unwrap_or_else(|| ui_url(&config.url, &ctx.env_fn()));
    let link = crate::artifacts::ui_link(
        &Config {
            ui_url: Some(ui),
            ..config.clone()
        },
        &format!("/overview?session={}#agent-sessions", enc(&session_id)),
    );
    let duration = ended
        .as_ref()
        .and_then(|e| e.get("durationMs"))
        .and_then(Value::as_f64)
        .unwrap_or(started.elapsed().as_millis() as f64);
    if args.json {
        let mut summary = json!({
            "session": ended.clone().unwrap_or(session.clone()),
            "exitCode": exit_code,
            "durationMs": duration,
            "route": { "route": route.as_str(), "from": choice.from },
            "url": link,
        });
        if let Some(delivery) = &skills {
            summary["skills"] = skills_delivery::json_of(delivery);
        }
        if let Some(synced) = &synced {
            summary["sessionSync"] = sync_json(synced);
        }
        ctx.io.out(&pretty(&summary));
    } else {
        let took = format_duration(duration);
        let interrupted = signal.is_some() || exit_code == 130 || exit_code == 143;
        let how = match &signal {
            Some(signal) => format!("stopped by {signal}"),
            None if interrupted => format!("was interrupted (exit {exit_code})"),
            None => format!("exited with code {exit_code}"),
        };
        let mark = if exit_code == 0 {
            &style.ok
        } else if interrupted {
            &style.warn
        } else {
            &style.fail
        };
        let spent = if route == Route::Subscription && !agent.entry().billing.accepts_subscription_usage {
            "usage unavailable".into()
        } else if route == Route::Subscription {
            // A run whose usage `nexus agy` could not read says only that: the reason is nothing the person can act on.
            let unknown = agent == Agent::Antigravity && !matches!(agy_usage, Some(antigravity_usage::Usage::Known(_)));
            match ended.as_ref().and_then(|e| e.get("usage")).filter(|u| !u.is_null()) {
                _ if unknown => "usage unknown".into(),
                Some(usage) => usage_line(style, usage),
                None => "no usage reported yet".into(),
            }
        } else {
            let access = ended
                .as_ref()
                .and_then(|e| e.get("access"))
                .filter(|a| !a.is_null())
                .or_else(|| session.get("access"));
            match access.and_then(Value::as_str).filter(|a| !a.is_empty()) {
                Some(access) => format!(
                    "paid with {} {} spend: nexus spend",
                    gateway_payer(&catalog, access),
                    style.bullet
                ),
                None => "spend: nexus spend".into(),
            }
        };
        ctx.io.err(&format!(
            "{mark} {command} {how} after {took} {} session {session_id} {} {spent}",
            style.bullet, style.bullet
        ));
        ctx.io.err(&format!("  {} {}", style.arrow, style.underline(&link)));
        if let Some(line) = synced.as_ref().and_then(|synced| sync_line(style, synced)) {
            ctx.io.err(&line);
        }
    }
    Ok(exit_code)
}

/// `nexus claude` and the other launchers with no Nexus account (`NEXUS_BYO_PROVIDERS`): the agent runs on its own
/// sign-in, the person's plan or key, as if they had started it themselves. No Gateway, no session, nothing recorded.
fn launch_without_account(
    agent: Agent,
    args: &LauncherArgs,
    flag: Option<Route>,
    bin: &Path,
    ctx: &Context,
    config: &Config,
    style: &Style,
) -> Result<i32> {
    let agent_label = agent.entry().identity.name;
    let command = agent.command();
    let choice = choose_route(agent, flag, ctx.var(&route_variable(agent)).as_deref(), None)?;
    if choice.route == Route::Gateway {
        return Err(crate::byo::account_required(&format!("`nexus {command}` through Gateway"), ctx, config).into());
    }
    if is_auto_name(named_model(agent, &args.forwarded).as_deref()) {
        return Err(
            crate::byo::account_required("Letting Nexus choose the model (`--model auto`)", ctx, config).into(),
        );
    }
    let forwarded = match args.recording.as_deref() {
        Some(which) => with_recording(agent, &args.forwarded, &crate::recordings::find(ctx, Some(which))?),
        None => args.forwarded.clone(),
    };
    let mut env = ctx.env.clone();
    for name in LAUNCHER_VARS {
        env.remove(*name);
    }
    let launch_args = prompt_in_file(bin, forwarded, ctx.config_path.parent().unwrap_or(Path::new(".")));
    if !args.json {
        ctx.io.err(&style.dim(&format!(
            "{agent_label} on its own sign-in (your plan or key) {} not signed in to Nexus, so nothing is recorded",
            style.bullet
        )));
    }
    let started = Instant::now();
    let (exit_code, _signal) = run(bin, &launch_args, &env, &ctx.cwd, ctx.interactive).map_err(|error| {
        CliError::hint(
            format!("Could not start {}: {error}.", bin.display()),
            format!(
                "Check that {agent_label} is installed and runs (`{} --version`).",
                agent.entry().identity.executable
            ),
        )
    })?;
    if args.json {
        ctx.io.out(&pretty(&json!({
            "signedIn": false,
            "exitCode": exit_code,
            "durationMs": started.elapsed().as_millis() as f64,
            "route": { "route": Route::Subscription.as_str(), "from": choice.from },
        })));
    }
    Ok(exit_code)
}

/// What the session log needs from the launch.
struct SessionLogRun<'a> {
    route: Route,
    api_key: &'a str,
    session_id: &'a str,
    model: Option<&'a str>,
    conversation: &'a Conversation,
    env: &'a HashMap<String, String>,
}

/// The run's admitted session-log configuration. Checks the sync switch, cache policy and device key.
/// When sync is on but unavailable, returns none and says why.
fn session_log_start(
    agent: Agent,
    ctx: &Context,
    config: &Config,
    style: &Style,
    json: bool,
    run: SessionLogRun,
) -> Option<session_log::Start> {
    if !session_log::enabled(ctx) {
        return None;
    }
    let say = |text: &str| {
        if !json {
            ctx.io.err(&style.dim(text));
        }
    };
    if session_log::workbench_agent(agent).is_none() {
        say(&format!(
            "Session sync: not available for {} yet.",
            agent.entry().identity.name
        ));
        return None;
    }
    if !session_log::allow_local_cache() {
        say("Session sync: off here, because your organization keeps nothing on this computer (AllowLocalCache).");
        return None;
    }
    let device = match crate::device_key::stored(ctx, &config.url) {
        Ok(Some(device)) => device,
        Ok(None) => {
            say("Session sync: this computer is not paired yet. Run `nexus devices pair` to sync your sessions.");
            return None;
        }
        Err(error) => {
            say(&format!("Session sync: {}", describe(&error)));
            return None;
        }
    };
    let entry = agent.entry();
    let agent_home = entry
        .profile
        .var
        .and_then(|variable| run.env.get(variable))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| ctx.var("HOME").map(|home| Path::new(&home).join(entry.profile.dir)))?;
    let conversation = match run.conversation {
        Conversation::New(id) | Conversation::Known(id) => Some(id.clone()),
        Conversation::Unknown => None,
    };
    Some(session_log::Start {
        agent,
        via: match run.route {
            Route::Gateway => Via::Nexus,
            Route::Subscription => Via::Subscription,
        },
        gateway: config.url.clone(),
        api_key: run.api_key.to_string(),
        nexus_session: run.session_id.to_string(),
        model: run.model.map(String::from),
        conversation,
        cwd: ctx.cwd.clone(),
        agent_home,
        dir: session_log::dir(ctx),
        device,
        version: crate::VERSION.to_string(),
        codex: None,
    })
}

fn sync_json(outcome: &session_log::Outcome) -> Value {
    match outcome {
        session_log::Outcome::Synced { events } => json!({ "state": "synced", "events": events }),
        session_log::Outcome::Waiting { events, trusted } => {
            json!({ "state": "waiting", "events": events, "trusted": trusted })
        }
        session_log::Outcome::Off(why) => json!({ "state": "off", "reason": why }),
        session_log::Outcome::Nothing => json!({ "state": "nothing" }),
    }
}

fn sync_line(style: &Style, outcome: &session_log::Outcome) -> Option<String> {
    Some(match outcome {
        session_log::Outcome::Synced { .. } => format!(
            "  {} {}",
            style.arrow,
            style.dim("Synced to Nexus UI, end-to-end encrypted.")
        ),
        session_log::Outcome::Waiting { events, trusted: false } => format!(
            "{} {events} events wait on this computer until a trusted device approves it: nexus devices pair",
            style.warn
        ),
        session_log::Outcome::Waiting { events, trusted: true } => format!(
            "{} {events} events wait to sync; your next `nexus claude` or `nexus codex` sends them.",
            style.warn
        ),
        session_log::Outcome::Off(why) => style.dim(&format!("Session sync: off for this run ({why}).")),
        session_log::Outcome::Nothing => return None,
    })
}

/// An error as one line, for a warning that does not stop the run.
fn describe(error: &Error) -> String {
    match error {
        Error::Core(core) => core.message.clone(),
        Error::Cli(cli) => cli.message.clone(),
        Error::Transport(transport) => format!("{transport:?}"),
    }
}

/// Who pays a Gateway session, from the `access` it recorded.
fn gateway_payer(catalog: &Value, access: &str) -> String {
    let provider = route_of(catalog, Route::Gateway).and_then(|view| view.get("provider").cloned());
    payer_label(Some(
        &json!({ "route": "gateway", "access": access, "available": true, "provider": provider }),
    ))
}

/// What a subscription run does with the agent's sign-in (`planLoginCheck` in agents.ts): stop when the profile is not
/// on the plan, go with a warning when the agent did not say or has no check (`None`), go quietly when it is. A signed-out
/// Antigravity goes with a warning too: `agy` asks for its own sign-in when it starts.
fn check_plan_login(agent: Agent, login: Option<Login>) -> std::result::Result<Option<String>, CliError> {
    let agent_label = agent.entry().identity.name;
    let command = agent.command();
    let catalog = agent_entry(None, agent);
    let plan = route_of(&catalog, Route::Subscription).cloned().unwrap_or_default();
    let plan_name = js_string(plan.get("plan"));
    // An agent whose folder no variable moves keeps it under the home folder.
    let variable = agent.entry().profile.var.unwrap_or("HOME");
    let gateway = match agent.entry().billing.gateway {
        Some(_) => {
            format!("run it through Gateway: `nexus {command} --api` (or always: `nexus agents use {command} gateway`)")
        }
        None => format!("run `{}` once to sign in", js_string(plan.get("login"))),
    };
    let Some(login) = login else {
        return Ok(Some(not_checked_line(agent_label)));
    };
    match login {
        Login::Plan => Ok(None),
        Login::SignedOut if agent == Agent::Antigravity => Ok(Some(format!(
            "{agent_label} isn't signed in to your {plan_name} plan here; {} will ask you to sign in.",
            agent.entry().identity.executable
        ))),
        Login::SignedOut => Err(CliError::hint(
            format!("{agent_label} is not signed in to your {plan_name} plan on this machine."),
            format!(
                "Run `{}` once (choose your {plan_name} account{}), then `nexus {command}` again. If another profile is signed in to it, set `{variable}` to that profile. Or {gateway}.",
                js_string(plan.get("login")),
                match agent {
                    Agent::Claude => ", not Console",
                    Agent::Codex | Agent::Antigravity | Agent::Cursor | Agent::Opencode => "",
                }
            ),
        )),
        Login::ApiKey => {
            let local = LocalAgent {
                agent: agent.as_str().into(),
                installed: true,
                path: None,
                version: None,
                login: Some(login),
                plan: None,
            };
            let relogin = setup_steps(
                &catalog,
                Route::Subscription,
                SetupOptions {
                    local: Some(&local),
                    ..SetupOptions::default()
                },
            )
            .into_iter()
            .find(|step| step.id == "plan_relogin")
            .map(|step| step.text)
            .unwrap_or_else(|| format!("{agent_label} is signed in with an API key, not your plan"));
            if agent == Agent::Antigravity {
                let settings = format!("~/{}/settings.json", agent.entry().profile.dir);
                return Err(CliError::hint(
                    format!("{agent_label} is set to use an API key: {settings} sets \"modelProvider\"."),
                    format!("Remove \"modelProvider\" from {settings}, then run `nexus {command}` again."),
                ));
            }
            Err(CliError::hint(
                format!("{relogin}."),
                format!("Or set `{variable}` to the profile that is signed in to your {plan_name} plan, or {gateway}."),
            ))
        }
        Login::Unknown => Ok(Some(format!(
            "Could not check how {agent_label} is signed in (`{}`): this run is on whichever account the profile `{variable}` names (or the default one) is signed in to.",
            match agent {
                Agent::Claude => "claude auth status",
                Agent::Codex => "codex login status",
                Agent::Antigravity => "agy -p /usage",
                Agent::Cursor => "cursor-agent status",
                Agent::Opencode => "opencode auth list",
            }
        ))),
    }
}

/// The warning for an agent whose entry has no sign-in check (`notCheckedLine` in agents.ts): the run goes ahead.
pub fn not_checked_line(agent_label: &str) -> String {
    format!("Sign-in not checked: {agent_label} runs on the account it is signed in to.")
}

/// The line before the agent starts: where its calls go, who pays, and where the route came from.
fn launch_line(
    style: &Style,
    agent: Agent,
    choice: &RouteChoice,
    session: &Value,
    core_url: &str,
    named: bool,
    account: Option<&str>,
) -> String {
    let command = agent.command();
    let catalog = agent_entry(None, agent);
    let from = source_note(agent, choice)
        .map(|note| format!(" {} {note}", style.bullet))
        .unwrap_or_default();
    let session_id = js_string(session.get("id"));
    if choice.route == Route::Subscription {
        let plan = route_of(&catalog, Route::Subscription).cloned().unwrap_or_default();
        // Claude Code and Codex report each call as it ends; Antigravity's calls are counted from its file at the end.
        let usage = match agent {
            Agent::Claude | Agent::Codex => format!("{} reports their usage", agent.entry().identity.name),
            Agent::Antigravity => format!("usage reported when {command} exits"),
            Agent::Cursor | Agent::Opencode => "usage unavailable".to_string(),
        };
        return format!(
            "{} Nexus Gateway {} session {session_id} {} {command} on your {} subscription{}{from} {} model calls go straight to {} (not metered or checked by Gateway; {usage})",
            style.info,
            style.bullet,
            style.bullet,
            js_string(plan.get("plan")),
            account.map(|account| format!(" ({account})")).unwrap_or_default(),
            style.bullet,
            js_string(plan.get("vendor")),
        );
    }
    let payer = s(session, "access")
        .filter(|access| !access.is_empty())
        .map(|access| format!(" {} paid with {}", style.bullet, gateway_payer(&catalog, access)))
        .unwrap_or_default();
    format!(
        "{} Nexus Gateway {} session {session_id} {} {command} on {} {} {}every model call through {core_url} (reserved, metered and checked by Gateway){payer}{from}",
        style.info,
        style.bullet,
        style.bullet,
        if named {
            js_string(session.get("model"))
        } else {
            "its default model".into()
        },
        style.bullet,
        s(session, "team")
            .filter(|team| !team.is_empty())
            .map(|team| format!("team {team} {} ", style.bullet))
            .unwrap_or_default(),
    )
}

#[derive(Debug, PartialEq)]
pub enum Conversation {
    /// Nexus picks the id and passes `--session-id`.
    New(String),
    /// The user named it (`--session-id x`, `--resume x`).
    Known(String),
    /// Claude Code picks it (`--continue`, the `--resume` picker, `--fork-session`), the run starts no conversation
    /// (`mcp list`, `--version`), or it is a new Codex one.
    Unknown,
}

/// Which Claude Code conversation this run is, from its arguments.
pub fn claude_conversation(args: &[String]) -> Conversation {
    let mut named = None;
    for (i, arg) in args.iter().enumerate() {
        if arg == "--" {
            break;
        }
        let next = args.get(i + 1).filter(|n| is_uuid(n)).cloned();
        if CLAUDE_NO_CONVERSATION.contains(&arg.as_str()) {
            return Conversation::Unknown;
        }
        match arg.as_str() {
            "--fork-session" | "--continue" | "-c" => return Conversation::Unknown,
            "--session-id" => named = next.or(named),
            "--resume" | "-r" => match next {
                Some(id) => named = Some(id),
                None => return Conversation::Unknown,
            },
            _ => {
                if let Some(id) = arg
                    .strip_prefix("--session-id=")
                    .or_else(|| arg.strip_prefix("--resume="))
                    .filter(|id| is_uuid(id))
                {
                    named = Some(id.to_string());
                }
            }
        }
    }
    match named {
        Some(id) => Conversation::Known(id),
        None => Conversation::New(uuid::Uuid::new_v4().to_string()),
    }
}

/// `codex resume <id>` (or `codex exec resume <id>`) names its conversation; any other Codex run is found afterwards
/// by `codex_conversation`.
pub fn codex_resumed(args: &[String]) -> Conversation {
    let id = args
        .iter()
        .position(|a| a == "resume")
        .and_then(|i| args.get(i + 1))
        .filter(|id| is_uuid(id));
    id.map_or(Conversation::Unknown, |id| Conversation::Known(id.clone()))
}

/// `agy --conversation <id>` resumes the conversation it names; any other run (a new one, `--continue`) is found
/// afterwards by [`antigravity_conversation`].
pub fn antigravity_resumed(args: &[String]) -> Conversation {
    resumed_with(args, "--conversation")
}

fn resumed_with(args: &[String], flag: &str) -> Conversation {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--" {
            break;
        }
        let named = if arg == flag {
            args.get(i + 1).map(String::as_str)
        } else {
            arg.strip_prefix(flag).and_then(|suffix| suffix.strip_prefix('='))
        };
        if let Some(id) = named {
            return match is_uuid(id) {
                true => Conversation::Known(id.to_string()),
                false => Conversation::Unknown,
            };
        }
    }
    Conversation::Unknown
}

/// Whether `agy`'s arguments resume its latest conversation (`--continue`), before its own `--`.
pub fn continues_latest(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--continue")
}

/// The Antigravity conversation this run used: `agy` keeps each one as `<profile>/conversations/<id>.db` (with SQLite's
/// `-wal` and `-journal` files beside it). A new run's is the one database created since `since`, so an `agy` open in
/// another terminal is not taken for it. A run that `continued` an older one, or a file system without creation times,
/// counts the conversations whose files changed since then. None when no conversation or more than one qualifies: a
/// wrong id would resume someone else's conversation.
pub fn antigravity_conversation(profile: &Path, since: std::time::SystemTime, continued: bool) -> Option<String> {
    let since = since.checked_sub(FILE_CLOCK_SLACK).unwrap_or(since);
    let ids: std::collections::BTreeSet<String> = std::fs::read_dir(profile.join("conversations"))
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let (id, database) = ["", "-wal", "-journal"].iter().find_map(|suffix| {
                let id = name.strip_suffix(suffix)?.strip_suffix(".db")?;
                Some((id.to_string(), suffix.is_empty()))
            })?;
            let meta = entry.metadata().ok()?;
            let at = match (continued, meta.created()) {
                // SQLite makes a `-wal` file anew each time a conversation opens, so for a new one only its database
                // counts.
                (false, Ok(created)) if database => created,
                (false, Ok(_)) => return None,
                _ => meta.modified().ok()?,
            };
            if !is_uuid(&id) {
                return None;
            }
            (meta.is_file() && at >= since).then_some(id)
        })
        .collect();
    let mut ids = ids.into_iter();
    let id = ids.next()?;
    ids.next().is_none().then_some(id)
}

/// The agent's configuration folder as this run sees it: its variable when set, else its folder under the home folder.
fn agent_profile(ctx: &Context, agent: Agent) -> Option<PathBuf> {
    let entry = agent.entry();
    entry
        .profile
        .var
        .and_then(|variable| ctx.var(variable))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(|| ctx.var("HOME").map(|h| Path::new(&h).join(entry.profile.dir)))
}

/// The account's name (`os.userInfo().username`), not `$USER`, which a shell or sudo may have changed.
#[cfg(unix)]
fn current_user() -> Option<String> {
    nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|user| user.name)
}

#[cfg(not(unix))]
fn current_user() -> Option<String> {
    None
}

/// Run the agent with the terminal handed over and wait for it; SIGTERM, SIGHUP and (when stdin is not a terminal)
/// SIGINT are passed on, so the agent ends cleanly and the session is closed after it. On a terminal, Ctrl-C already
/// reaches the agent (the whole foreground process group gets it), so it is not sent twice.
fn run(
    bin: &Path,
    args: &[String],
    env: &HashMap<String, String>,
    cwd: &Path,
    interactive: bool,
) -> std::io::Result<(i32, Option<String>)> {
    let mut command = agent_command(bin);
    command.args(args).env_clear().envs(env).current_dir(cwd);
    run_child(command, interactive)
}

/// A failed start as the contract's `code`: the kind of the error, never its text.
fn spawn_code(kind: std::io::ErrorKind) -> &'static str {
    match kind {
        std::io::ErrorKind::NotFound => "not_found",
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        std::io::ErrorKind::InvalidInput => "invalid_input",
        _ => "other",
    }
}

/// The heartbeats of one session while its agent runs, on their own thread; a failed one is skipped, as the next may
/// get through.
struct Heartbeat {
    stop: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

impl Heartbeat {
    fn start(core: &crate::client::Client, session_id: &str, every: Duration) -> Heartbeat {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let core = core.clone();
        let path = format!("/v1/agents/sessions/{}/heartbeat", enc(session_id));
        let thread = std::thread::spawn(move || {
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(every) {
                let _ = core.post(&path, json!({}));
            }
        });
        Heartbeat { stop, thread }
    }

    /// Stops the beats and waits for one in flight, so none lands after the session ends.
    fn stop(self) {
        drop(self.stop);
        let _ = self.thread.join();
    }
}

#[cfg(unix)]
fn run_child(mut command: std::process::Command, interactive: bool) -> std::io::Result<(i32, Option<String>)> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    use signal_hook::iterator::Signals;
    use std::os::unix::process::ExitStatusExt;

    let mut signals = Signals::new([SIGTERM, SIGHUP, SIGINT])?;
    let handle = signals.handle();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            handle.close();
            return Err(error);
        }
    };
    let pid = Pid::from_raw(child.id() as i32);
    let relay = std::thread::spawn(move || {
        for signal in signals.forever() {
            if signal == SIGINT && interactive {
                continue;
            }
            if let Ok(signal) = Signal::try_from(signal) {
                let _ = kill(pid, signal);
            }
        }
    });
    let status = child.wait();
    handle.close();
    let _ = relay.join();
    let status = status?;
    if let Some(signal) = status.signal() {
        let name = Signal::try_from(signal)
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|_| signal.to_string());
        return Ok((128 + signal, Some(name)));
    }
    Ok((status.code().unwrap_or(1), None))
}

#[cfg(not(unix))]
fn run_child(mut command: std::process::Command, _interactive: bool) -> std::io::Result<(i32, Option<String>)> {
    let status = command.spawn()?.wait()?;
    Ok((status.code().unwrap_or(1), None))
}

/// A duration humanised to the largest unit that fits: `4h 36m`, `12m 03s`, `8.4s`, `740ms`.
fn format_duration(ms: f64) -> String {
    if ms < 1000.0 {
        return format!("{}ms", js_number(ms));
    }
    let total_seconds = crate::style::js_round(ms / 1000.0);
    let h = (total_seconds / 3600.0).floor();
    let m = ((total_seconds % 3600.0) / 60.0).floor();
    let s = (total_seconds % 60.0) as i64;
    if h > 0.0 {
        format!("{}h {:02}m", js_number(h), m as i64)
    } else if total_seconds >= 60.0 {
        format!("{}m {s:02}s", js_number(m))
    } else {
        format!("{}s", to_fixed(ms / 1000.0, 1))
    }
}

/// The subscription usage summary: calls, compacted in→out tokens with cache share, the models used, and list cost,
/// with the calls it leaves unpriced. With every call unpriced there is no list cost to show, only the unpriced count.
fn usage_line(style: &Style, usage: &Value) -> String {
    let input_tokens = n(usage, "inputTokens") + n(usage, "cacheReadTokens") + n(usage, "cacheCreationTokens");
    let cached = if input_tokens > 0.0 {
        format!(
            " ({}% cached)",
            js_number(crate::style::js_round(
                n(usage, "cacheReadTokens") / input_tokens * 100.0
            ))
        )
    } else {
        String::new()
    };
    let names: Vec<&str> = usage
        .get("models")
        .and_then(Value::as_array)
        .map(|models| models.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let models = if names.is_empty() {
        String::new()
    } else {
        format!("{} {} ", names.join(", "), style.bullet)
    };
    let cost = n(usage, "estimatedCostUsd");
    let (calls, unpriced) = (n(usage, "calls"), n(usage, "unpricedCalls"));
    let priced = if calls > 0.0 && unpriced == calls {
        format!("{} calls unpriced", js_number(unpriced))
    } else if unpriced > 0.0 {
        format!(
            "~${} list, {} calls unpriced",
            to_fixed(cost, if cost < 0.01 { 4 } else { 2 }),
            js_number(unpriced)
        )
    } else {
        format!("~${} list", to_fixed(cost, if cost < 0.01 { 4 } else { 2 }))
    };
    format!(
        "{} calls {} {} {} {} tokens{cached} {} {models}{priced}, on your subscription",
        js_number(calls),
        style.bullet,
        compact_tokens(input_tokens),
        style.g.arrow,
        compact_tokens(n(usage, "outputTokens")),
        style.bullet,
    )
}

/// A token count with an SI suffix: `28.7M`, `182.2K`, `940`. One decimal, a trailing `.0` dropped.
/// The thresholds sit at the point where `to_fixed(_, 1)` rounds up into the next suffix, so a count
/// like 999_990 prints `1M` rather than `1000K`.
fn compact_tokens(count: f64) -> String {
    if count >= 999_950.0 {
        format!("{}M", trim1(count / 1_000_000.0))
    } else if count >= 999.95 {
        format!("{}K", trim1(count / 1_000.0))
    } else {
        js_number(count)
    }
}

fn trim1(value: f64) -> String {
    let text = to_fixed(value, 1);
    text.strip_suffix(".0").map(str::to_string).unwrap_or(text)
}

fn launcher_help(
    style: &Style,
    agent: Agent,
    core_url: &str,
    policy: Option<&Value>,
    access: Option<&Value>,
) -> Vec<String> {
    if agent.entry().billing.gateway.is_none() {
        return plan_only_help(style, agent, access);
    }
    let agent_label = agent.entry().identity.name;
    let command = agent.command();
    let catalog = agent_entry(None, agent);
    let plan = route_of(&catalog, Route::Subscription).cloned().unwrap_or_default();
    let (plan_name, plan_login) = (js_string(plan.get("plan")), js_string(plan.get("login")));
    let how = match agent {
        Agent::Claude => vec![
            format!(
                "  Through Gateway: starts {} with ANTHROPIC_BASE_URL={core_url}/v1/anthropic and your Nexus key",
                style.cyan("claude")
            ),
            "  as ANTHROPIC_AUTH_TOKEN; your own ANTHROPIC_API_KEY is not passed to it.".to_string(),
        ],
        Agent::Codex => vec![
            format!(
                "  Through Gateway: starts {} with a {} model provider for this run only (-c overrides:",
                style.cyan("codex"),
                style.cyan("nexus")
            ),
            format!("  {core_url}/v1/openai/v1, the Responses API); ~/.codex/config.toml is not changed."),
        ],
        Agent::Antigravity | Agent::Cursor | Agent::Opencode => Vec::new(),
    };
    let route = match access {
        Some(access) => format!(
            "  {}  {} {}",
            style.dim("Route"),
            if s(access, "route") == Some("gateway") {
                "Gateway".to_string()
            } else {
                format!("your {plan_name} plan")
            },
            style.dim(&format!(
                "({}; change it: nexus agents use {command} gateway|subscription)",
                match s(access, "from") {
                    Some("preference") => "your saved route",
                    Some("organization") => "your organization turned subscription mode off",
                    _ => "the default",
                }
            ))
        ),
        None => format!(
            "  {}",
            style.dim("Sign in (nexus auth login) to see your saved route: nexus agents.")
        ),
    };
    let mut lines = vec![
        format!(
            "{} {}",
            style.bold(&format!("nexus {command}")),
            style.dim(&format!(
                "· {agent_label} through Nexus Gateway, or on your own {plan_name} plan"
            ))
        ),
        String::new(),
        format!("{}  {}", style.bold("Usage"), usage(agent)),
        String::new(),
    ];
    lines.extend(how);
    lines.extend([
        "  Every model call is reserved against your organization's budget, metered, checked against its".to_string(),
        "  policy by Gateway, and attributed to you and to a session you can see in Nexus UI.".to_string(),
        format!(
            "  On your {plan_name} plan (--subscription), {agent_label} keeps its own login ({plan_login}): its calls go straight"
        ),
        format!(
            "  to {}, and Nexus records the session and the usage {agent_label} reports.",
            js_string(plan.get("vendor"))
        ),
        format!(
            "  Arguments after Nexus's own flags (or after --) go to {agent_label} unchanged; {}",
            style.cyan(&format!("nexus {command} -- --help"))
        ),
        format!("  shows {agent_label}'s own help."),
        String::new(),
        format!(
            "  {}A JSON summary on stdout when {agent_label} exits",
            pad_end("--json", 16)
        ),
        format!(
            "  {}Use your {plan_name} plan (`{plan_login}`), not Gateway: calls are not metered or checked",
            pad_end("--subscription", 16)
        ),
        format!(
            "  {}Through Gateway, metered and checked, whatever route you saved (was --gateway)",
            pad_end("--api", 16)
        ),
        format!(
            "  {}gateway or subscription (also {}=<route>); default: your saved route",
            pad_end("--via <route>", 16),
            route_variable(agent)
        ),
        format!(
            "  {}The team this run's spend goes to (also NEXUS_TEAM); default: your default team",
            pad_end("--team <slug>", 16)
        ),
        format!(
            "  {}Start on a screen recording from Nexus for Mac (⌥⇧S): the latest, or one by id (nexus recordings)",
            pad_end("--recording", 16)
        ),
        format!(
            "  {}Start without your skills: Nexus does not write them to {agent_label}'s skills folder (nexus skills)",
            pad_end("--no-skills", 16)
        ),
        format!(
            "  {}{agent_label}'s own flag, read by Nexus: run on the model Gateway chooses within your policies",
            pad_end("--model auto", 16)
        ),
        String::new(),
        route,
        match policy {
            Some(policy) => format!(
                "  {}  {} {}",
                style.dim("Default model"),
                js_string(policy.get("defaultModel")),
                style.dim(&format!(
                    "(when you name none; allowed: {})",
                    crate::json::a(policy, "allowedModels")
                        .iter()
                        .map(|m| js_string(Some(m)))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            ),
            None => format!(
                "  {}",
                style.dim("Sign in (nexus auth login) to see your organization's default model.")
            ),
        },
        String::new(),
        format!(
            "  {} {agent_label}: {} {}",
            style.dim("Needs"),
            agent.entry().install.command,
            style.dim("(nexus agents shows what is set up)")
        ),
        format!(
            "  {}  nexus {command} {}",
            style.dim("Example"),
            agent.entry().session.example
        ),
    ]);
    lines
}

/// `nexus <agent> --help` for an agent without a Gateway route: what runs and what does not, with no Gateway flags, no
/// policy and no `auto` model.
fn plan_only_help(style: &Style, agent: Agent, access: Option<&Value>) -> Vec<String> {
    let entry = agent.entry();
    let (label, executable) = (entry.identity.name, entry.identity.executable);
    let command = agent.command();
    let route = match access {
        Some(access) if s(access, "route") == Some("gateway") => format!(
            "  {}  {}",
            style.dim("Route"),
            style.dim("none: subscription mode is off for your organization")
        ),
        Some(_) => format!("  {}  your {} plan", style.dim("Route"), entry.identity.plan),
        None => format!(
            "  {}",
            style.dim("Sign in (nexus auth login) to see whether your organization allows your own plan.")
        ),
    };
    vec![
        format!(
            "{} {}",
            style.bold(&format!("nexus {command}")),
            style.dim(&format!("· {label} on your own {} plan", entry.identity.plan))
        ),
        String::new(),
        format!("{}  {}", style.bold("Usage"), usage(agent)),
        String::new(),
        format!(
            "  Starts {} on your {} plan: its calls go straight to {}. Nexus records the session, not its usage.",
            style.cyan(executable),
            entry.identity.plan,
            entry.identity.vendor
        ),
        "  --api, --via gateway, --model auto and --recording are not available.".to_string(),
        format!("  {} are not passed to it.", entry.billing.plan_overrides.join(", ")),
        format!(
            "  It does not start while ~/{}/settings.json sets \"modelProvider\": remove that key.",
            entry.profile.dir
        ),
        format!(
            "  Arguments after Nexus's own flags (or after --) go to {label} unchanged; {}",
            style.cyan(&format!("nexus {command} -- --help"))
        ),
        format!("  shows {label}'s own help."),
        String::new(),
        format!("  {}A JSON summary on stdout when {label} exits", pad_end("--json", 16)),
        format!(
            "  {}Run on your {} plan (the default; sign in with `{}`)",
            pad_end("--subscription", 16),
            entry.identity.plan,
            entry.sign_in.login
        ),
        format!(
            "  {}The team this session is attributed to (also NEXUS_TEAM); default: your default team",
            pad_end("--team <slug>", 16)
        ),
        format!(
            "  {}{label}'s own flag: the model {executable} runs, by its id",
            pad_end("--model <id>", 16)
        ),
        String::new(),
        route,
        String::new(),
        format!(
            "  {} {label}: {} {}",
            style.dim("Needs"),
            entry.install.command,
            style.dim("(nexus agents shows what is set up)")
        ),
        format!("  {}  nexus {command} {}", style.dim("Example"), entry.session.example),
        format!(
            "  {}   nexus {command} -- {}",
            style.dim("Resume"),
            entry.session.resume.replace("{id}", "<id>")
        ),
    ]
}

/// A UUID in its 36-character hyphenated form, the only form both CLIs send, as Gateway checks it
/// (`nexus_control::agents::is_uuid`). Nothing looser: an agent's conversation id ends up in a resume command people
/// paste into a shell. `Uuid::try_parse` also takes the simple, `{…}` and `urn:uuid:…` forms, but none of 36 characters.
pub fn is_uuid(text: &str) -> bool {
    text.len() == 36 && uuid::Uuid::try_parse(text).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uuid_is_the_hyphenated_form_only() {
        assert!(is_uuid("0f8fad5b-d9cb-469f-a165-70867728950e"));
        assert!(is_uuid("0F8FAD5B-D9CB-469F-A165-70867728950E"));
        for text in [
            "0f8fad5bd9cb469fa16570867728950e",
            "{0f8fad5b-d9cb-469f-a165-70867728950e}",
            "urn:uuid:0f8fad5b-d9cb-469f-a165-70867728950e",
            "0f8fad5b-d9cb-469f-a165-70867728950",
            "0f8fad5bd-9cb-469f-a165-70867728950e",
            "0f8fad5b-d9cb-469f-a165-70867728950g",
            "0f8fad5b-d9cb-469f-a165-7086772895 e",
            "--dangerously-skip-permissions-xxxxx",
            "",
        ] {
            assert!(!is_uuid(text), "{text}");
        }
    }

    #[test]
    fn usage_line_leaves_out_the_list_cost_only_when_every_call_is_unpriced() {
        let style = Style::new(false);
        let usage = |unpriced: u64, cost: f64, models: Value| {
            json!({
                "calls": 2, "inputTokens": 18_067, "cacheReadTokens": 0, "cacheCreationTokens": 0, "outputTokens": 200,
                "estimatedCostUsd": cost, "unpricedCalls": unpriced, "models": models,
            })
        };
        assert_eq!(
            usage_line(&style, &usage(2, 0.0, json!([]))),
            "2 calls · 18.1K → 200 tokens (0% cached) · 2 calls unpriced, on your subscription"
        );
        assert_eq!(
            usage_line(&style, &usage(2, 0.0, json!(["gpt-9-preview"]))),
            "2 calls · 18.1K → 200 tokens (0% cached) · gpt-9-preview · 2 calls unpriced, on your subscription"
        );
        // One priced call: the list cost stays.
        assert_eq!(
            usage_line(&style, &usage(1, 0.0042, json!(["gpt-9-preview"]))),
            "2 calls · 18.1K → 200 tokens (0% cached) · gpt-9-preview · ~$0.0042 list, 1 calls unpriced, on your subscription"
        );
        assert_eq!(
            usage_line(&style, &usage(0, 0.0042, json!([]))),
            "2 calls · 18.1K → 200 tokens (0% cached) · ~$0.0042 list, on your subscription"
        );
    }

    /// Antigravity's only sign-in state that stops a run is its API-key mode, and the fix names the file; signed out,
    /// or with no check, the run goes with a warning. Neither offers Gateway, which it does not have.
    #[test]
    fn antigravity_stops_only_on_its_api_key_mode() {
        let api_key = check_plan_login(Agent::Antigravity, Some(Login::ApiKey)).expect_err("stops");
        assert_eq!(
            api_key.message,
            "Antigravity is set to use an API key: ~/.gemini/antigravity-cli/settings.json sets \"modelProvider\"."
        );
        assert_eq!(
            api_key.hint.as_deref(),
            Some("Remove \"modelProvider\" from ~/.gemini/antigravity-cli/settings.json, then run `nexus agy` again.")
        );
        // `agy` shows its own sign-in prompt, so a signed-out profile is one warning line, not a stop.
        let signed_out = check_plan_login(Agent::Antigravity, Some(Login::SignedOut)).expect("goes");
        assert_eq!(
            signed_out.as_deref(),
            Some("Antigravity isn't signed in to your Google AI plan here; agy will ask you to sign in.")
        );
        let unchecked = check_plan_login(Agent::Antigravity, None).expect("goes");
        assert!(unchecked.is_some_and(|warning| warning.starts_with("Sign-in not checked: Antigravity")));
    }

    #[test]
    fn antigravity_resumes_only_a_conversation_named_by_its_id() {
        let id = "0b9c8a1e-6f4d-4b2a-9c3e-1d2f3a4b5c6d";
        assert_eq!(
            antigravity_resumed(&v(&["--conversation", id, "-p", "hi"])),
            Conversation::Known(id.into())
        );
        assert_eq!(
            antigravity_resumed(&v(&[&format!("--conversation={id}")])),
            Conversation::Known(id.into())
        );
        assert_eq!(antigravity_resumed(&v(&["--continue"])), Conversation::Unknown);
        assert_eq!(
            antigravity_resumed(&v(&["--conversation", "latest"])),
            Conversation::Unknown
        );
        assert_eq!(
            antigravity_resumed(&v(&["--", "--conversation", id])),
            Conversation::Unknown
        );
        assert!(continues_latest(&v(&["--continue", "-p", "hi"])));
        assert!(!continues_latest(&v(&["-p", "--", "--continue"])));
    }

    #[test]
    fn antigravity_conversation_is_the_one_file_changed_since_the_run_began() -> std::io::Result<()> {
        let profile = std::env::temp_dir().join(format!("nexus-agy-conversations-{}", uuid::Uuid::new_v4()));
        let folder = profile.join("conversations");
        assert_eq!(
            antigravity_conversation(&profile, std::time::SystemTime::now(), false),
            None
        );
        std::fs::create_dir_all(&folder)?;
        let older = "11111111-1111-4111-8111-111111111111";
        std::fs::write(folder.join(format!("{older}.db")), b"")?;
        // Written before `since` by more than the clock slack, so it is never this run's.
        std::thread::sleep(FILE_CLOCK_SLACK * 2);
        let since = std::time::SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(antigravity_conversation(&profile, since, false), None);
        let id = "0b9c8a1e-6f4d-4b2a-9c3e-1d2f3a4b5c6d";
        std::fs::write(folder.join(format!("{id}.db")), b"")?;
        // Its SQLite files are the same conversation; a name that is not a conversation id is none.
        std::fs::write(folder.join(format!("{id}.db-wal")), b"")?;
        std::fs::write(folder.join("notes.db"), b"")?;
        // An `agy` in another terminal writes to an older conversation meanwhile.
        std::fs::write(folder.join(format!("{older}.db-wal")), b"")?;
        let created = antigravity_conversation(&profile, since, false);
        let changed = antigravity_conversation(&profile, since, true);
        // A file system without creation times counts changes, and then the two cannot be told apart.
        if std::fs::metadata(folder.join(format!("{id}.db")))?.created().is_ok() {
            assert_eq!(created.as_deref(), Some(id));
        } else {
            assert_eq!(created, None);
        }
        assert_eq!(changed, None);
        std::fs::write(folder.join("7f2c1d9e-3b4a-4c5d-8e6f-0a1b2c3d4e5f.db"), b"")?;
        assert_eq!(antigravity_conversation(&profile, since, false), None);
        let _ = std::fs::remove_dir_all(&profile);
        Ok(())
    }

    #[test]
    fn a_subscription_run_stops_unless_the_profile_is_on_the_plan() {
        assert_eq!(check_plan_login(Agent::Claude, Some(Login::Plan)).ok(), Some(None));
        let signed_out = check_plan_login(Agent::Claude, Some(Login::SignedOut)).expect_err("stops");
        assert!(
            signed_out
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("set `CLAUDE_CONFIG_DIR` to that profile"))
        );
        let codex_signed_out = check_plan_login(Agent::Codex, Some(Login::SignedOut)).expect_err("stops");
        assert!(codex_signed_out.message.starts_with("Codex is not signed in"));
        let api_key = check_plan_login(Agent::Codex, Some(Login::ApiKey)).expect_err("stops");
        assert!(
            api_key.message.starts_with("Codex is signed in with an API key"),
            "{}",
            api_key.message
        );
        assert!(
            api_key
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("`CODEX_HOME`"))
        );
        // An agent that does not say is not stopped: the run goes, with a warning.
        let unknown = check_plan_login(Agent::Claude, Some(Login::Unknown)).expect("goes");
        assert!(unknown.is_some_and(|warning| warning.contains("`claude auth status`")));
    }

    /// Every message a subscription run prints for its sign-in, whole, for each agent.
    #[test]
    fn sign_in_messages_for_each_agent() {
        let stop = |message: &str, hint: &str| {
            Err(CliError {
                message: message.into(),
                hint: Some(hint.into()),
                exit_code: 1,
                code: None,
            })
        };
        let cases = [
            (Agent::Claude, Some(Login::Plan), Ok(None)),
            (Agent::Codex, Some(Login::Plan), Ok(None)),
            (
                Agent::Claude,
                Some(Login::SignedOut),
                stop(
                    "Claude Code is not signed in to your Claude plan on this machine.",
                    "Run `claude auth login` once (choose your Claude account, not Console), then `nexus claude` again. If another profile is signed in to it, set `CLAUDE_CONFIG_DIR` to that profile. Or run it through Gateway: `nexus claude --api` (or always: `nexus agents use claude gateway`).",
                ),
            ),
            (
                Agent::Codex,
                Some(Login::SignedOut),
                stop(
                    "Codex is not signed in to your ChatGPT plan on this machine.",
                    "Run `codex login` once (choose your ChatGPT account), then `nexus codex` again. If another profile is signed in to it, set `CODEX_HOME` to that profile. Or run it through Gateway: `nexus codex --api` (or always: `nexus agents use codex gateway`).",
                ),
            ),
            (
                Agent::Claude,
                Some(Login::ApiKey),
                stop(
                    "Claude Code is signed in with an API key, which bills the API, not your plan: sign out (claude auth logout), then sign in with your Claude account.",
                    "Or set `CLAUDE_CONFIG_DIR` to the profile that is signed in to your Claude plan, or run it through Gateway: `nexus claude --api` (or always: `nexus agents use claude gateway`).",
                ),
            ),
            (
                Agent::Codex,
                Some(Login::ApiKey),
                stop(
                    "Codex is signed in with an API key, which bills the API, not your plan: sign out (codex logout), then sign in with your ChatGPT account.",
                    "Or set `CODEX_HOME` to the profile that is signed in to your ChatGPT plan, or run it through Gateway: `nexus codex --api` (or always: `nexus agents use codex gateway`).",
                ),
            ),
            // The Unknown message names `claude auth status`, not the entry's `login_status` (`--text`).
            (
                Agent::Claude,
                Some(Login::Unknown),
                Ok(Some(
                    "Could not check how Claude Code is signed in (`claude auth status`): this run is on whichever account the profile `CLAUDE_CONFIG_DIR` names (or the default one) is signed in to.".to_string(),
                )),
            ),
            (
                Agent::Codex,
                Some(Login::Unknown),
                Ok(Some(
                    "Could not check how Codex is signed in (`codex login status`): this run is on whichever account the profile `CODEX_HOME` names (or the default one) is signed in to.".to_string(),
                )),
            ),
            (
                Agent::Claude,
                None,
                Ok(Some(
                    "Sign-in not checked: Claude Code runs on the account it is signed in to.".to_string(),
                )),
            ),
            (
                Agent::Codex,
                None,
                Ok(Some(
                    "Sign-in not checked: Codex runs on the account it is signed in to.".to_string(),
                )),
            ),
            // Antigravity has no Gateway route and no profile variable: signed out it goes, as agy asks itself.
            (Agent::Antigravity, Some(Login::Plan), Ok(None)),
            (
                Agent::Antigravity,
                Some(Login::SignedOut),
                Ok(Some(
                    "Antigravity isn't signed in to your Google AI plan here; agy will ask you to sign in.".to_string(),
                )),
            ),
            (
                Agent::Antigravity,
                Some(Login::ApiKey),
                stop(
                    "Antigravity is set to use an API key: ~/.gemini/antigravity-cli/settings.json sets \"modelProvider\".",
                    "Remove \"modelProvider\" from ~/.gemini/antigravity-cli/settings.json, then run `nexus agy` again.",
                ),
            ),
            (
                Agent::Antigravity,
                Some(Login::Unknown),
                Ok(Some(
                    "Could not check how Antigravity is signed in (`agy -p /usage`): this run is on whichever account the profile `HOME` names (or the default one) is signed in to.".to_string(),
                )),
            ),
            (
                Agent::Antigravity,
                None,
                Ok(Some(
                    "Sign-in not checked: Antigravity runs on the account it is signed in to.".to_string(),
                )),
            ),
        ];
        for (agent, login, expected) in cases {
            assert_eq!(check_plan_login(agent, login), expected, "{agent} {login:?}");
        }
    }

    /// The pages and commit-trailer instructions as each agent gets them, whole.
    #[test]
    fn instructions_for_each_agent() {
        // The CLI's golden launches pin the whole text one of Nexus's own people gets; anyone else gets it without the
        // trailer instructions.
        let both = session_instructions(true);
        assert_eq!(
            format!("{}\n\n{}", session_instructions(false), commit_trailer::INSTRUCTIONS),
            both
        );
        assert!(session_instructions(false).ends_with(&format!("\n\n{PAGE_INSTRUCTIONS}")));
        let developer = |text: &str| format!("developer_instructions={}", serde_json::Value::from(text));
        assert_eq!(
            with_instructions(
                Agent::Claude,
                &v(&["-p", "hi", "--", "--append-system-prompt=x"]),
                &both
            ),
            vec![
                "--append-system-prompt".to_string(),
                both.clone(),
                "-p".into(),
                "hi".into(),
                "--".into(),
                "--append-system-prompt=x".into()
            ]
        );
        assert_eq!(
            with_instructions(
                Agent::Claude,
                &v(&["--append-system-prompt", "mine", "-p", "hi"]),
                &both
            ),
            vec![
                "--append-system-prompt".to_string(),
                format!("mine\n\n{both}"),
                "-p".into(),
                "hi".into()
            ]
        );
        // A flag at the end with nothing after it is not joined: the instructions go first.
        assert_eq!(
            with_instructions(Agent::Claude, &v(&["-p", "--append-system-prompt"]), &both),
            vec![
                "--append-system-prompt".to_string(),
                both.clone(),
                "-p".into(),
                "--append-system-prompt".into()
            ]
        );
        assert_eq!(
            with_instructions(Agent::Claude, &v(&["auth", "status"]), &both),
            v(&["auth", "status"])
        );
        // Codex gets them first, whatever follows, even a run that starts no conversation.
        for args in [v(&["exec", "hi"]), v(&["--version"]), v(&["--", "x"]), Vec::new()] {
            let mut expected = vec!["-c".to_string(), developer(&both)];
            expected.extend(args.iter().cloned());
            assert_eq!(with_instructions(Agent::Codex, &args, &both), expected);
        }
        assert_eq!(
            with_instructions(Agent::Antigravity, &v(&["-p", "hi"]), &both),
            v(&["-p", "hi"])
        );
        assert!(takes_instructions(Agent::Claude) && takes_instructions(Agent::Codex));
        assert!(!takes_instructions(Agent::Antigravity));
        // The trailer instructions, whole.
        assert_eq!(
            commit_trailer::INSTRUCTIONS,
            "Nexus co-author: when a commit you make in a nexus-xyz repository ends with `Co-authored-by: Nexus CLI` or `Co-authored-by: Nexus Desktop` in place of your own co-author line, a Nexus git hook did that on purpose, so Nexus can count the work done in it. Keep the commit as it is: do not amend it, skip the hook, or offer to restore your line."
        );
    }

    /// The conversation ids each agent's arguments name, beyond the cases above.
    #[test]
    fn conversation_ids_from_the_arguments() {
        let id = "0b9c8a1e-6f4d-4b2a-9c3e-1d2f3a4b5c6d";
        let upper = "0B9C8A1E-6F4D-4B2A-9C3E-1D2F3A4B5C6D";
        assert_eq!(
            claude_conversation(&v(&["--session-id", id, "-p", "hi"])),
            Conversation::Known(id.into())
        );
        assert_eq!(
            claude_conversation(&v(&[&format!("--resume={upper}")])),
            Conversation::Known(upper.into())
        );
        // The last id named wins; a `--session-id` with no id keeps the one before.
        assert_eq!(
            claude_conversation(&v(&[
                "--session-id",
                "11111111-1111-4111-8111-111111111111",
                "--resume",
                id
            ])),
            Conversation::Known(id.into())
        );
        assert_eq!(
            claude_conversation(&v(&["--resume", id, "--session-id", "x"])),
            Conversation::Known(id.into())
        );
        assert_eq!(claude_conversation(&v(&["--continue"])), Conversation::Unknown);
        assert_eq!(claude_conversation(&v(&["-r"])), Conversation::Unknown);
        // After `--`, nothing counts.
        assert!(matches!(
            claude_conversation(&v(&["-p", "hi", "--", "--resume", id])),
            Conversation::New(_)
        ));
        assert!(matches!(
            claude_conversation(&v(&["-p", "hi", "--", "mcp"])),
            Conversation::New(_)
        ));
        let Conversation::New(new) = claude_conversation(&v(&["--session-id", "x"])) else {
            panic!("a new conversation");
        };
        assert!(is_uuid(&new) && new != "x");

        // Codex reads the first `resume`, with no `--`.
        assert_eq!(
            codex_resumed(&v(&["exec", "--", "resume", upper])),
            Conversation::Known(upper.into())
        );
        assert_eq!(codex_resumed(&v(&["resume", "x", "resume", id])), Conversation::Unknown);
        assert_eq!(codex_resumed(&v(&["resume"])), Conversation::Unknown);
        assert_eq!(codex_resumed(&v(&[])), Conversation::Unknown);

        // Antigravity reads the first `--conversation`.
        assert_eq!(antigravity_resumed(&v(&["--conversation="])), Conversation::Unknown);
        assert_eq!(antigravity_resumed(&v(&["--conversation"])), Conversation::Unknown);
        assert_eq!(
            antigravity_resumed(&v(&["--conversation", "x", "--conversation", id])),
            Conversation::Unknown
        );
        assert_eq!(
            antigravity_resumed(&v(&["--conversation", upper])),
            Conversation::Known(upper.into())
        );
    }

    #[test]
    fn agent_without_a_sign_in_check_goes_with_a_warning() {
        let warning = check_plan_login(Agent::Claude, None).expect("goes");
        assert_eq!(warning, Some(not_checked_line("Claude Code")));
        assert!(not_checked_line("Antigravity").starts_with("Sign-in not checked: Antigravity "));
    }

    #[test]
    fn entry_without_gateway_refuses_the_gateway_route() {
        let claude = Agent::Claude.entry();
        let unbound = AgentEntry {
            billing: nexus_client::agent_entries::Billing {
                gateway: None,
                ..claude.billing
            },
            ..*claude
        };
        // `--api` and `--via gateway` are a flag; the variable and Gateway's answer choose the same route.
        let flag = parse_launcher_args(&["--api".to_string()]).route;
        let route = Route::parse(flag.as_deref().unwrap_or_default()).expect("a route");
        let chosen = [
            choose_route(Agent::Claude, Some(route), None, None),
            choose_route(Agent::Claude, None, Some("gateway"), None),
            choose_route(
                Agent::Claude,
                None,
                None,
                Some(&json!({ "route": "gateway", "from": "preference" })),
            ),
            choose_route(
                Agent::Claude,
                None,
                None,
                Some(&json!({ "route": "gateway", "from": "organization" })),
            ),
        ];
        for choice in chosen {
            let refused = require_route(&unbound, choice.expect("a route").route).expect_err("refused");
            let Error::Cli(refused) = refused else {
                panic!("{refused:?}");
            };
            assert_eq!(refused.code, Some("route_unsupported"));
            assert_eq!(refused.exit_code, 2);
            assert_eq!(
                refused.message,
                "Claude Code runs only on your Claude plan: run `nexus claude --subscription`"
            );
        }
        assert!(require_route(&unbound, Route::Subscription).is_ok());
        // Claude Code and Codex keep both routes; Antigravity has only its plan.
        for agent in [Agent::Claude, Agent::Codex] {
            for route in Route::ALL {
                assert!(require_route(agent.entry(), route).is_ok(), "{agent} {route:?}");
            }
        }
        assert!(require_route(Agent::Antigravity.entry(), Route::Gateway).is_err());
        assert!(require_route(Agent::Antigravity.entry(), Route::Subscription).is_ok());
    }

    #[test]
    fn claude_conversation_is_new_unless_the_user_named_or_left_it_to_claude() {
        let v = |args: &[&str]| args.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        let id = "0b9c8a1e-6f4d-4b2a-9c3e-1d2f3a4b5c6d";
        assert!(matches!(claude_conversation(&v(&["-p", "hi"])), Conversation::New(_)));
        assert_eq!(
            claude_conversation(&v(&["--resume", id])),
            Conversation::Known(id.into())
        );
        assert_eq!(
            claude_conversation(&v(&["-r", id, "go on"])),
            Conversation::Known(id.into())
        );
        assert_eq!(
            claude_conversation(&v(&[&format!("--session-id={id}")])),
            Conversation::Known(id.into())
        );
        assert_eq!(claude_conversation(&v(&["--resume"])), Conversation::Unknown);
        assert_eq!(
            claude_conversation(&v(&["--resume", "fix the bug"])),
            Conversation::Unknown
        );
        assert_eq!(claude_conversation(&v(&["-c"])), Conversation::Unknown);
        assert_eq!(
            claude_conversation(&v(&["--resume", id, "--fork-session"])),
            Conversation::Unknown
        );
        // Runs that write no conversation get no `--session-id` and no resume command.
        assert_eq!(claude_conversation(&v(&["mcp", "list"])), Conversation::Unknown);
        assert_eq!(claude_conversation(&v(&["doctor"])), Conversation::Unknown);
        assert_eq!(claude_conversation(&v(&["--version"])), Conversation::Unknown);
        assert_eq!(
            claude_conversation(&v(&["--model", "claude-opus-5-5", "mcp", "list"])),
            Conversation::Unknown
        );
        assert!(matches!(
            claude_conversation(&v(&["-p", "update the readme"])),
            Conversation::New(_)
        ));
        // Only the hyphenated form, as Gateway accepts it.
        assert!(matches!(
            claude_conversation(&v(&["--resume", &format!("{{{id}}}")])),
            Conversation::Unknown
        ));
    }

    #[test]
    fn codex_conversation_is_the_one_this_run_started_in_this_folder() -> std::io::Result<()> {
        let id = "0b9c8a1e-6f4d-4b2a-9c3e-1d2f3a4b5c6d";
        assert_eq!(codex_resumed(&v(&["resume", id])), Conversation::Known(id.into()));
        assert_eq!(
            codex_resumed(&v(&["exec", "resume", id, "go on"])),
            Conversation::Known(id.into())
        );
        assert_eq!(codex_resumed(&v(&["resume", "--last"])), Conversation::Unknown);
        assert_eq!(codex_resumed(&v(&["fix the bug"])), Conversation::Unknown);

        let root = std::env::temp_dir().join(format!("nexus-codex-{}", uuid::Uuid::new_v4()));
        let (home, here) = (root.join("home"), root.join("cwd"));
        let day = home.join("sessions/2026/10/19");
        std::fs::create_dir_all(&day)?;
        std::fs::create_dir_all(&here)?;
        let since = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        let at = |secs: i64| time::OffsetDateTime::from(since) + time::Duration::seconds(secs);
        // (file, id, folder, started, last written), in seconds from `since`.
        for (name, id, cwd, begun, touched) in [
            ("mine", "22222222-2222-4222-8222-222222222222", &here, 1, 2),
            // Another Codex in this folder, started before this run, wrote last.
            ("other", "11111111-1111-4111-8111-111111111111", &here, -60, 3),
            ("elsewhere", "33333333-3333-4333-8333-333333333333", &root, 1, 4),
        ] {
            let begun = at(begun)
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(std::io::Error::other)?;
            let meta = json!({ "type": "session_meta", "payload": { "id": id, "cwd": cwd, "timestamp": begun } });
            let path = day.join(format!("rollout-{name}.jsonl"));
            std::fs::write(&path, format!("{meta}\n{{}}\n"))?;
            std::fs::File::options()
                .write(true)
                .open(&path)?
                .set_modified(at(touched).into())?;
        }
        assert_eq!(
            codex_conversation(&home, &here, since).as_deref(),
            Some("22222222-2222-4222-8222-222222222222")
        );
        assert_eq!(codex_conversation(&root.join("missing"), &here, since), None);
        std::fs::remove_dir_all(&root)
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn durations_and_token_counts_match_the_typescript_helpers() {
        assert_eq!(format_duration(740.0), "740ms");
        assert_eq!(format_duration(8_400.0), "8.4s");
        assert_eq!(format_duration(72_000.0), "1m 12s");
        assert_eq!(format_duration(723_000.0), "12m 03s");
        assert_eq!(format_duration(14_700_000.0), "4h 05m");
        assert_eq!(format_duration(16_610_000.0), "4h 36m");

        assert_eq!(compact_tokens(940.0), "940");
        assert_eq!(compact_tokens(182_194.0), "182.2K");
        assert_eq!(compact_tokens(28_708_768.0), "28.7M");
        assert_eq!(compact_tokens(2_000_000.0), "2M");
        assert_eq!(compact_tokens(999_949.0), "999.9K");
        assert_eq!(compact_tokens(999_990.0), "1M");
    }

    #[test]
    fn forwards_everything_after_nexus_flags() {
        assert_eq!(
            parse_launcher_args(&v(&["-p", "hello", "--model", "x"])).forwarded,
            v(&["-p", "hello", "--model", "x"])
        );
        let parsed = parse_launcher_args(&v(&["--json", "--", "--help", "--json"]));
        assert!(parsed.json && !parsed.help);
        assert_eq!(parsed.forwarded, v(&["--help", "--json"]));
        assert!(parse_launcher_args(&v(&["--help"])).help);
        assert_eq!(
            parse_launcher_args(&v(&["--team=web", "x"])).team.as_deref(),
            Some("web")
        );
    }

    #[test]
    fn reads_the_route_flags() {
        let parsed = parse_launcher_args(&v(&["--team", "web", "--via=subscription", "--json", "-p", "hi"]));
        assert_eq!(
            (parsed.route.as_deref(), parsed.conflict, parsed.json),
            (Some("subscription"), false, true)
        );
        assert_eq!(parsed.forwarded, v(&["-p", "hi"]));
        assert!(!parse_launcher_args(&v(&["--api", "--via", "gateway"])).conflict);
        assert!(parse_launcher_args(&v(&["--api", "--subscription"])).conflict);
        assert_eq!(
            parse_launcher_args(&v(&["--gateway"])).route.as_deref(),
            Some("gateway")
        );
        // `--via` with nothing after it, or after the agent's first argument, is the agent's.
        assert_eq!(parse_launcher_args(&v(&["--via"])).forwarded, v(&["--via"]));
        let late = parse_launcher_args(&v(&["-p", "--subscription"]));
        assert_eq!((late.route, late.forwarded), (None, v(&["-p", "--subscription"])));
    }

    #[test]
    fn a_flag_wins_then_the_variable_then_the_saved_route() {
        let saved = json!({ "route": "subscription", "from": "preference" });
        let choose = |flag, variable| choose_route(Agent::Claude, flag, variable, Some(&saved)).expect("a route");
        assert_eq!(choose(Some(Route::Gateway), Some("subscription")).from, "flag");
        let env = choose(None, Some(" Gateway "));
        assert_eq!((env.route, env.from.as_str()), (Route::Gateway, "env"));
        let preference = choose(None, Some("  "));
        assert_eq!(
            (preference.route, preference.from.as_str()),
            (Route::Subscription, "preference")
        );
        let default = choose_route(Agent::Claude, None, None, None).expect("a route");
        assert_eq!((default.route, default.from.as_str()), (Route::Subscription, "default"));
        assert!(choose_route(Agent::Claude, None, Some("nope"), None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn writes_the_codex_catalog_named_by_its_content() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("nexus-codex-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        // Larger than a pipe holds, as Codex's own catalog is.
        let big = json!({ "models": [{ "slug": "gpt-6-sol", "use_responses_lite": true, "base_instructions": "x".repeat(300_000) }] });
        std::fs::write(dir.join("catalog.json"), big.to_string()).expect("catalog");
        let stand_in = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            path
        };
        let codex = stand_in(
            "codex",
            &format!(
                "[ \"$*\" = \"debug models --bundled\" ] || exit 2\ncat '{}'",
                dir.join("catalog.json").display()
            ),
        );
        let env: HashMap<String, String> = [("PATH".to_string(), "/bin:/usr/bin".to_string())]
            .into_iter()
            .collect();
        let out = dir.join("config");
        let path = model_catalog(Agent::Codex, &codex, &env, &out).expect("written");
        assert!(path.starts_with(&out));
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(written["models"][0]["use_responses_lite"], json!(false));
        let contents = std::fs::read(&path).expect("catalog bytes");
        let expected = format!("codex-models-{}.json", &sha256_hex(&[&contents])[..16]);
        assert_eq!(path.file_name().and_then(|name| name.to_str()), Some(expected.as_str()));
        assert_eq!(model_catalog(Agent::Codex, &codex, &env, &out), Some(path));

        // A Codex that cannot print its catalog (the parity stand-in exits 3) runs without one.
        let failing = stand_in("failing", "echo \"args: $*\"\nexit 3");
        assert_eq!(model_catalog(Agent::Codex, &failing, &env, &dir.join("none")), None);
        assert!(!dir.join("none").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod recording_tests {
    use super::*;
    use crate::recordings::Recording;

    /// `claude.cmd` (npm on Windows) can't be started with a line break in an argument: a system prompt
    /// over several lines goes to a file, in either spelling. Nothing else changes.
    #[test]
    fn a_batch_file_reads_a_multi_line_system_prompt_from_a_file() {
        let strings = |items: &[&str]| items.iter().map(|item| item.to_string()).collect::<Vec<_>>();
        let dir = std::env::temp_dir().join(format!("nexus-cli-prompt-{}", uuid::Uuid::new_v4().simple()));
        let cmd = Path::new(r"C:\Users\me\AppData\Roaming\npm\claude.CMD");
        let joined = with_pages(Agent::Claude, &strings(&["--append-system-prompt", "mine", "-p", "hi"]));
        let out = prompt_in_file(cmd, joined.clone(), &dir);
        assert_eq!(out.len(), joined.len());
        assert_eq!(out[0], "--append-system-prompt-file");
        assert_eq!(std::fs::read_to_string(&out[1]).ok().as_ref(), Some(&joined[1]));
        assert_eq!(out[2..], joined[2..]);
        assert!(out.iter().all(|arg| !arg.contains(['\r', '\n'])));
        // Written once, then reused.
        assert_eq!(prompt_in_file(cmd, joined.clone(), &dir), out);
        let equals = strings(&["--append-system-prompt=a\nb", "-p", "hi"]);
        let out = prompt_in_file(Path::new("claude.bat"), equals, &dir);
        let file = out[0]
            .strip_prefix("--append-system-prompt-file=")
            .unwrap_or_else(|| panic!("{out:?}"));
        assert_eq!(std::fs::read_to_string(file).ok().as_deref(), Some("a\nb"));
        // A native build, a prompt on one line, or text after `--` stays as it is.
        assert_eq!(prompt_in_file(Path::new("claude.exe"), joined.clone(), &dir), joined);
        assert_eq!(prompt_in_file(Path::new("claude"), joined.clone(), &dir), joined);
        let one = with_pages(Agent::Claude, &strings(&["-p", "hi"]));
        assert_eq!(prompt_in_file(cmd, one.clone(), &dir), one);
        let after = strings(&["-p", "--", "--append-system-prompt", "a\nb"]);
        assert_eq!(prompt_in_file(cmd, after.clone(), &dir), after);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn customers_get_connector_discovery_without_claiming_an_attached_server() {
        let customer = session_instructions(false);
        assert!(customer.contains("If this session exposes Nexus MCP tools"));
        assert!(customer.contains(nexus_client::mcp::CONNECTOR_INSTRUCTIONS));
        assert!(customer.contains("nexus connectors --json"));
        assert!(customer.contains(PAGE_INSTRUCTIONS));
        assert!(!customer.contains(commit_trailer::INSTRUCTIONS));
        assert!(session_instructions(true).starts_with(&customer));
        assert!(session_instructions(true).contains(commit_trailer::INSTRUCTIONS));
    }

    #[test]
    fn every_session_is_told_about_pages() {
        let p = PAGE_INSTRUCTIONS;
        let strings = |items: &[&str]| items.iter().map(|item| item.to_string()).collect::<Vec<_>>();
        assert_eq!(
            with_pages(Agent::Claude, &strings(&["-p", "hi"])),
            strings(&["--append-system-prompt", p, "-p", "hi"])
        );
        // Claude Code keeps one system-prompt addition, so pages join the one already there.
        assert_eq!(
            with_pages(Agent::Claude, &strings(&["--append-system-prompt", "rec", "-p", "hi"])),
            vec![
                "--append-system-prompt".to_string(),
                format!("rec\n\n{p}"),
                "-p".into(),
                "hi".into()
            ]
        );
        assert_eq!(
            with_pages(Agent::Claude, &strings(&["--append-system-prompt=mine"])),
            vec![format!("--append-system-prompt=mine\n\n{p}")]
        );
        assert_eq!(
            with_pages(Agent::Claude, &strings(&["mcp", "list"])),
            strings(&["mcp", "list"])
        );
        assert_eq!(
            with_pages(Agent::Claude, &strings(&["--version"])),
            strings(&["--version"])
        );
        // A prompt word that is also a subcommand keeps them.
        assert_eq!(
            with_pages(Agent::Claude, &strings(&["-p", "fix", "the", "logs"])),
            strings(&["--append-system-prompt", p, "-p", "fix", "the", "logs"])
        );
        // One wording, pinned with the TypeScript CLI's.
        assert_eq!(
            p,
            include_str!("../tests/fixtures/interfaces/page-instructions.txt").trim_end()
        );
        assert_eq!(
            with_pages(Agent::Codex, &strings(&["exec", "hi"])),
            vec![
                "-c".to_string(),
                format!("developer_instructions={}", serde_json::to_string(p).expect("json")),
                "exec".into(),
                "hi".into()
            ]
        );
    }

    #[test]
    fn claude_is_told_how_to_read_it_and_codex_gets_the_contact_sheet() {
        let dir = std::env::temp_dir().join(format!("nexus-cli-agent-recording-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("contact-sheet.png"), "png").expect("write");
        let recording = Recording {
            id: "2026-10-19T14-03-22".into(),
            path: dir.clone(),
            created_at: None,
            duration: 42.4,
            frames: 3,
            narrated: true,
            said: None,
        };
        let prompt = crate::recordings::prompt(&recording);
        let sheet = dir.join("contact-sheet.png").display().to_string();
        let strings = |items: &[&str]| items.iter().map(|item| item.to_string()).collect::<Vec<_>>();
        let claude = with_recording(Agent::Claude, &[], &recording);
        assert_eq!(claude[..2], [String::from("--append-system-prompt"), prompt.clone()]);
        assert_eq!(claude.len(), 3);
        assert_eq!(
            with_recording(Agent::Claude, &strings(&["-p", "fix it"]), &recording),
            vec![
                "--append-system-prompt".to_string(),
                prompt.clone(),
                "-p".into(),
                "fix it".into()
            ]
        );
        assert_eq!(
            with_recording(Agent::Codex, &[], &recording),
            vec!["-i".to_string(), sheet.clone(), prompt]
        );
        assert_eq!(
            with_recording(Agent::Codex, &strings(&["exec", "fix it"]), &recording),
            vec!["exec".to_string(), "-i".into(), sheet, "fix it".into()]
        );
        assert!(is_recording_id(Some("latest")) && is_recording_id(Some("2026-10-19T14-03-22")));
        assert!(!is_recording_id(Some("fix it")) && !is_recording_id(None));
        let parsed = parse_launcher_args(&strings(&["--recording", "-p", "x"]));
        assert_eq!(
            (parsed.recording.as_deref(), parsed.forwarded.len()),
            (Some("latest"), 2)
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod shared_memory_launch_tests {
    use super::*;

    #[test]
    fn enabled_memory_launch_points_claude_at_the_cli_mcp_server() {
        let fixture = std::env::temp_dir().join(format!("nexus-cli-memory-prompt-{}", uuid::Uuid::new_v4()));
        let env = |name: &str| match name {
            "HOME" | "XDG_DATA_HOME" | "LOCALAPPDATA" => Some(fixture.display().to_string()),
            _ => None,
        };
        let store = nexus_agent_memory::Store::from_env(&env).expect("store");
        store.set_enabled(true).expect("enable shared memory");

        let (prompt, memory) = launch_instructions(Agent::Claude, false, &fixture, &env).expect("launch");
        assert!(memory.is_some());
        let (instruction, _) = prompt.split_once("<nexus-shared-memory>").expect("notes block");
        assert!(instruction.contains("Use the `nexus_memory` MCP server by default"));
        assert!(instruction.contains("select:mcp__nexus_memory__remember"));
        std::fs::remove_dir_all(fixture).expect("remove fixture");
    }

    #[test]
    fn disabled_memory_keeps_main_launch_arguments_and_environment() {
        let cwd = std::env::current_dir().expect("fixture working directory");
        let data = cwd.join("unused-memory-fixture");
        let env = |name: &str| match name {
            "HOME" | "XDG_DATA_HOME" | "LOCALAPPDATA" => Some(data.display().to_string()),
            _ => None,
        };
        for agent in [Agent::Claude, Agent::Codex] {
            let forward = vec!["hello".to_string()];
            let original_env = HashMap::from([("PATH".into(), "fixture-path".into())]);
            let launch = |args, env| {
                agent_launch(
                    agent,
                    LaunchOptions {
                        route: Route::Subscription,
                        core_url: "https://example.test",
                        api_key: "fixture",
                        session_id: "session",
                        team: None,
                        model: None,
                        model_catalog: None,
                        web_search: false,
                        args,
                        env,
                    },
                )
            };
            let expected = launch(
                with_instructions(agent, &forward, &session_instructions(false)),
                original_env.clone(),
            );
            let (instructions, store) = launch_instructions(agent, false, &cwd, &env).expect("disabled launch");
            let mut args = with_instructions(agent, &forward, &instructions);
            let mut child_env = original_env.clone();
            configure_memory_launch(store.as_ref(), agent, None, &cwd, "session", &mut args, &mut child_env)
                .expect("memory configuration");
            assert_eq!(launch(args, child_env), expected);
            assert!(!data.exists(), "disabled memory must not create a data directory");
        }
    }
}
