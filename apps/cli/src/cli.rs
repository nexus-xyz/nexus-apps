//! The `nexus` command line (`cli.ts`): built-in commands, and product commands discovered from the module manifests
//! Gateway serves. Output is colored with symbols on a terminal and plain otherwise; `--json` prints only JSON on stdout.
//! Errors are one short line and a fix on stderr, with a non-zero exit.

use std::path::Path;
use std::time::{Duration, Instant};

use nexus_client::agent_entries::Agent;
use nexus_client::device::{self, Poll};
use serde_json::{Value, json};

use crate::agent_capabilities::{agent_ids, capabilities};
use crate::args::{Args, parse_argv};
use crate::catalog::{
    AUTH_COMMANDS, AUTH_TOKEN_COMMANDS, HelpGroup, MEMBER_COMMANDS, POLICY_VERBS, command_catalog, find_command,
    help_groups,
};
use crate::client::{CLI_VERSION_HEADER, Client, ClientOptions, enc};
use crate::config::{
    Config, DEFAULT_URL, GATEWAY_URL, environment, load_config, save_config, save_default_config, ui_url,
};
use crate::error::{CliError, Error, Result, Transport, error_text, to_cli_error, usage};
use crate::json::{a, compact, js_string, n, n_opt, obj, pretty, s, t};
use crate::style::{
    Row, Style, ago, brand, budget_bar, capitalize, col, flex, js_len, kv, level_tone, list, meta, meter, pad_end,
    pairs, provider_mark, right, rows_of, seconds, style_for, to_fixed, usd,
};
use crate::usage_signals;
use crate::{
    Context, VERSION, agent_routes, agent_share, agents, apps, artifacts, audit, billing, budgets, chat, completion,
    connections, design_systems, exports, jobs, machines, mcp, members, models, notifications, org, pages, routines,
    search, service_accounts, skills, slack, spend, teams, tokens, update,
};

/// Every Gateway client the CLI makes: idempotent, retried writes, `x-nexus-client: cli/<version>` and
/// `x-nexus-cli-version`.
pub fn cli_client(url: &str, api_key: Option<&str>) -> Client {
    cli_client_with_timeout(url, api_key, Duration::from_secs(120))
}

/// [`cli_client`] with each call limited to `timeout`.
pub fn cli_client_with_timeout(url: &str, api_key: Option<&str>, timeout: Duration) -> Client {
    Client::new(ClientOptions {
        api_key: api_key.map(String::from),
        headers: vec![(CLI_VERSION_HEADER.to_string(), VERSION.to_string())],
        idempotency: true,
        client: Some(format!("cli/{VERSION}")),
        timeout,
        ..ClientOptions::new(url)
    })
}

/// What a command needs: Gateway, the style, `--json`, the arguments and the run's context.
pub struct Cmd<'a> {
    pub core: Client,
    pub style: &'a Style,
    pub json: bool,
    pub args: &'a Args,
    pub ctx: &'a Context,
    pub config: &'a Config,
}

impl Cmd<'_> {
    pub fn out(&self, line: impl AsRef<str>) {
        self.ctx.io.out(line.as_ref());
    }

    pub fn lines(&self, lines: impl IntoIterator<Item = String>) {
        for line in lines {
            self.ctx.io.out(&line);
        }
    }

    pub fn print(&self, value: &Value) -> i32 {
        self.ctx.io.out(&pretty(value));
        0
    }

    pub fn flag(&self, name: &str) -> Option<&str> {
        self.args.flag(name)
    }

    pub fn has(&self, name: &str) -> bool {
        self.args.has(name)
    }

    /// A flag's value when it is set and not empty (`flags.x` is truthy), owned.
    pub fn flag_set_str(&self, name: &str) -> Option<String> {
        self.args.flag_set(name).map(String::from)
    }

    /// A local file, relative to the working directory (`nexus team import`, `nexus policy set --from`).
    pub fn read_file(&self, path: &str) -> std::io::Result<String> {
        std::fs::read_to_string(self.ctx.cwd.join(path))
    }
}

pub fn run_cli(argv: &[String], ctx: &Context) -> i32 {
    // Hidden: the shell-completion helper `nexus completion <shell>` installs the glue that calls this.
    // It walks the local command catalog only — no config, no Gateway, no sign-in — so it runs first.
    // Hidden: the agent with no screen, for programs (the desktop's Code workbench): JSON lines on stdin and stdout.
    if argv.first().map(String::as_str) == Some("__agent") {
        return crate::tui::serve_agent(&argv[1..], ctx);
    }
    if argv.first().map(String::as_str) == Some("__memory_mcp") {
        return crate::mcp::serve_memory(&argv[1..], ctx);
    }
    if argv.first().map(String::as_str) == Some("__complete") {
        for line in completion::complete(&argv[1..], VERSION) {
            ctx.io.out(&line);
        }
        return 0;
    }
    let args = parse_argv(argv);
    let style = style_for(ctx.color, &ctx.env_fn(), ctx.columns);
    // `nexus [--json] [--no-color] claude|codex|agy …`: everything after Nexus's own flags belongs to the agent.
    let agent_at = argv.iter().position(|arg| arg != "--json" && arg != "--no-color");
    let agent = agent_at
        .and_then(|at| argv.get(at))
        .and_then(|arg| Agent::from_command(arg))
        .filter(|&agent| agents::launch_available(agent));
    let json = match (&agent, agent_at) {
        (Some(_), Some(at)) => argv[..at].iter().any(|arg| arg == "--json") || launcher_wants_json(&argv[at + 1..]),
        _ => args.has("json"),
    };
    let saved = load_config(&ctx.config_path, &ctx.env_fn());
    let mut config = saved.clone();
    config.ui_url = Some(ui_url(
        if saved.url.is_empty() { DEFAULT_URL } else { &saved.url },
        &ctx.env_fn(),
    ));

    let result = (|| -> Result<i32> {
        if let (Some(agent), Some(at)) = (agent, agent_at) {
            let mut launch: Vec<String> = argv[..at].iter().filter(|arg| *arg == "--json").cloned().collect();
            launch.extend(argv[at + 1..].iter().cloned());
            return agents::launch_agent(agent, &launch, ctx, &config, &style);
        }
        dispatch(&args, json, &style, ctx, &saved, &config)
    })();

    match result {
        Ok(code) => code,
        Err(error) => {
            let failure = to_cli_error(&error, &config.url);
            // A failed sign-in or update is queued for the next run to report, as codes only.
            if agent.is_none() && failure.exit_code != 2 {
                let words: Vec<&str> = args.positionals.iter().take(2).map(String::as_str).collect();
                let kind = match words.as_slice() {
                    ["login", ..] | ["auth", "login", ..] => Some((usage_signals::ErrorKind::SignIn, "sign_in")),
                    ["update", ..] => Some((usage_signals::ErrorKind::Update, "updater")),
                    _ => None,
                };
                if let Some((kind, component)) = kind {
                    usage_signals::record_error(&ctx.config_path, kind, component, &error_code(&error));
                }
            }
            if json {
                ctx.io.err(&compact(&json_error(&error, &failure)));
            } else {
                ctx.io.err(&error_text(&style, &failure));
            }
            if ctx.var("NEXUS_DEBUG").as_deref() == Some("1") && !matches!(error, Error::Cli(_)) {
                ctx.io.err(&format!("{error:?}"));
            }
            failure.exit_code
        }
    }
}

/// A failure with `--json`: its message and hint, and the code Gateway or the CLI gave it, if any.
fn json_error(error: &Error, failure: &CliError) -> Value {
    json!({ "error": obj(vec![
        ("message", Some(json!(failure.message))),
        ("hint", failure.hint.as_ref().map(|hint| json!(hint))),
        ("code", match error {
            Error::Core(error) => error.code.as_ref().map(|code| json!(code)),
            Error::Cli(error) => error.code.map(|code| json!(code)),
            _ => None,
        }),
    ]) })
}

/// A failure as the contract's `code`: Gateway's own error code, else its status, else how the request failed. Never
/// the message.
fn error_code(error: &Error) -> String {
    match error {
        Error::Core(core) => core
            .code
            .as_deref()
            .map(|code| usage_signals::name_of(code, "other"))
            .filter(|code| code != "other")
            .unwrap_or_else(|| format!("http_{}", core.status)),
        Error::Transport(Transport::Timeout) => "timeout".into(),
        Error::Transport(Transport::Unreachable(_)) => "unreachable".into(),
        Error::Transport(Transport::Other(_)) => "transport".into(),
        Error::Cli(_) => "refused".into(),
    }
}

fn dispatch(args: &Args, json: bool, style: &Style, ctx: &Context, saved: &Config, config: &Config) -> Result<i32> {
    let out = |line: String| ctx.io.out(&line);
    let print = |value: &Value| {
        ctx.io.out(&pretty(value));
        0
    };
    let command = args.positionals.first().map(String::as_str);
    let rest: Vec<String> = args.positionals.iter().skip(1).cloned().collect();
    let rest0 = rest.first().map(String::as_str);

    // `--help` never runs a command. A command with a help screen of its own prints it below, alone or
    // with one argument; a subcommand, more arguments or any other command prints its catalog entry here, before
    // sign-in and with no request.
    // `nexus agent ask|chat` is Nexus, the personal agent; any other `nexus agent` is still `nexus agents`.
    let agent_chat =
        command == Some("agent") && rest0.is_some_and(|verb| crate::agent_chat::AGENT_CHAT_VERBS.contains(&verb));
    if let Some(command) = command.filter(|_| args.has("help")) {
        let alias = HELP_ALIASES
            .iter()
            .find(|(alias, _)| *alias == command && !agent_chat)
            .map_or(command, |(_, name)| *name);
        let words: Vec<String> = alias.split(' ').map(String::from).chain(rest.iter().cloned()).collect();
        let catalog = command_catalog(VERSION);
        let spec = find_command(a(&catalog, "commands"), &words).filter(|spec| {
            !OWN_HELP.contains(&command) || s(spec, "name").is_some_and(|name| name.contains(' ')) || rest.len() > 1
        });
        if let Some(spec) = spec {
            if json {
                return Ok(print(spec));
            }
            for line in command_help(style, spec) {
                out(line);
            }
            return Ok(0);
        }
    }

    if args.has("version") || command == Some("version") {
        if json {
            return Ok(print(&json!({ "version": VERSION })));
        }
        out(format!("nexus {VERSION}"));
        return Ok(0);
    }

    if command.is_none() || command == Some("help") {
        // Every built-in command, flag and exit code, for scripts and the docs.
        if json {
            return Ok(print(&command_catalog(VERSION)));
        }
        // `nexus` alone: who you are and where to start; `nexus help`: every command.
        if command.is_none() && !args.has("help") {
            let me = config
                .api_key
                .as_ref()
                .and_then(|key| cli_client(&config.url, Some(key)).get("/v1/tenants/me").ok());
            // On a terminal, a session in the style of Codex's; piped, the home screen as before.
            if crate::tui::wanted(ctx, config, me.as_ref()) {
                let budget = config
                    .api_key
                    .as_ref()
                    .and_then(|key| cli_client(&config.url, Some(key)).get("/v1/spend").ok())
                    .map(|spend| left_text(&spend));
                let (org, who) = identity(config, me.as_ref());
                return crate::tui::run(ctx, config, style, &[org, who], budget);
            }
            for line in home_lines(style, config, me.as_ref()) {
                out(line);
            }
            return Ok(0);
        }
        let modules = match &config.api_key {
            Some(key) => cli_client(&config.url, Some(key))
                .get("/v1/modules")
                .ok()
                .map(|r| a(&r, "modules").to_vec()),
            None => None,
        };
        for line in main_help(style, config, modules.as_deref()) {
            out(line);
        }
        return Ok(0);
    }
    let command = command.unwrap_or_default();

    // Update this CLI from the Gateway it talks to; needs no sign-in.
    if command == "update" {
        let core_url = if config.url.is_empty() {
            DEFAULT_URL
        } else {
            &config.url
        };
        return update::update_command(ctx, core_url, args, json, style);
    }

    // Screen recordings from Nexus for Mac, on this computer: no sign-in, nothing sent.
    if command == "recordings" || command == "recording" {
        let help = ["help".to_string()];
        let words = if args.has("help") { &help[..] } else { &rest[..] };
        return crate::recordings::recordings_command(words, ctx, args, json, style);
    }

    // Code sessions in Nexus for desktop, through its outside access: no sign-in, nothing sent to Gateway.
    if command == "sessions" || command == "session" {
        let help = ["help".to_string()];
        let words = if args.has("help") { &help[..] } else { &rest[..] };
        return crate::sessions::sessions_command(words, ctx, args, json, style);
    }

    if command == "memory" {
        let help = ["help".to_string()];
        let words = if args.has("help") { &help[..] } else { &rest[..] };
        return crate::shared_memory::command(words, ctx, args, json);
    }

    // Print a shell-completion script to source; needs no sign-in. The hidden `__complete`
    // helper the script calls is handled earlier, in run_cli.
    if command == "completion" {
        // `nexus completion help` / `--help`: print usage and exit 0, like every other command.
        if rest0 == Some("help") || args.has("help") {
            out(format!(
                "Usage: nexus completion <{}|install>",
                completion::SHELLS.replace(", ", "|")
            ));
            out("Source what it prints, e.g. `source <(nexus completion bash)` (fish: `nexus completion fish | source`).".to_string());
            out("`nexus completion install` turns it on for every shell you use; the installer and `nexus update` run it.".to_string());
            return Ok(0);
        }
        if rest0 == Some("install") {
            let done = completion::install(&ctx.env)
                .map_err(|error| CliError::new(format!("Could not turn on tab completion: {error}")))?;
            if done.is_empty() {
                return Err(CliError::new("HOME is not set, so there is no shell startup file to change.").into());
            }
            for setup in done {
                out(format!("{} {}", style.ok, setup.line));
            }
            out("Open a new terminal to use it.".to_string());
            return Ok(0);
        }
        match completion::script(rest0.unwrap_or("")) {
            Some(text) => {
                for line in text.lines() {
                    out(line.to_string());
                }
                return Ok(0);
            }
            None => {
                return usage(
                    format!(
                        "Usage: nexus completion <{}|install>",
                        completion::SHELLS.replace(", ", "|")
                    ),
                    Some(
                        "Source what it prints, e.g. `source <(nexus completion bash)` (fish: `nexus completion fish | source`).",
                    ),
                );
            }
        }
    }

    // `nexus auth login|logout|status|tokens`, like `gh auth`; `login` and `logout` stay as hidden aliases.
    let auth_action = if command == "auth" {
        rest0
    } else if command == "login" || command == "logout" {
        Some(command)
    } else {
        None
    };
    if command == "auth"
        && (auth_action.is_none() || auth_action == Some("help") || (args.has("help") && rest.get(1).is_none()))
    {
        if json {
            let row = |prefix: &str, (name, text): &(&str, &str)| json!({ "command": format!("nexus auth {prefix}{}", name.split(' ').next().unwrap_or("")), "description": text });
            let commands: Vec<Value> = AUTH_COMMANDS
                .iter()
                .map(|r| row("", r))
                .chain(AUTH_TOKEN_COMMANDS.iter().map(|r| row("tokens ", r)))
                .collect();
            return Ok(print(&json!({ "commands": commands })));
        }
        for line in auth_help(style) {
            out(line);
        }
        return Ok(0);
    }
    match auth_action {
        Some("signup") => {
            if args.flag_set("name").is_none_or(|name| name.trim().is_empty())
                || args.flag("key").is_some()
                || args.has("with-token")
            {
                return usage(
                    "Give the new organization's name without an existing token.",
                    Some("Usage: nexus auth signup --name <name> [--email <email>] [--dry-run]"),
                );
            }
            return login(args, config, ctx, style, json);
        }
        Some("login") => return login(args, config, ctx, style, json),
        Some("logout") => {
            // Revoke the token on Gateway too (best effort). An API key from --with-token is only forgotten.
            let revoked = match (&config.api_key, config.kind.as_deref()) {
                (Some(key), kind) if kind != Some("api") => cli_client(&config.url, Some(key))
                    .post("/v1/auth/logout", json!({}))
                    .map(|r| t(&r, "revoked"))
                    .unwrap_or(false),
                _ => false,
            };
            save(ctx, &Config::with_url(&config.url))?;
            if json {
                return Ok(print(&json!({ "loggedOut": true, "url": config.url })));
            }
            out(format!(
                "{} Logged out of {}. Your token is {}removed from {}.",
                style.ok,
                config.url,
                if revoked { "revoked and " } else { "" },
                ctx.config_path.display()
            ));
            return Ok(0);
        }
        Some("status") => return auth_status(config, ctx, style, json),
        Some("delete-account") => {
            return crate::account::delete_account(
                |name| args.has(name),
                config,
                ctx,
                style,
                json,
                args.flag("confirm"),
                || save(ctx, &Config::with_url(&config.url)),
            );
        }
        _ => {}
    }
    // `nexus auth tokens` needs a sign-in: it runs with the other API commands, below.
    if command == "auth" && auth_action != Some("tokens") {
        return usage(
            format!("Unknown command `nexus auth {}`.", js_opt(auth_action)),
            Some(
                "Use `nexus auth login`, `nexus auth logout`, `nexus auth status`, `nexus auth tokens` or `nexus auth delete-account`.",
            ),
        );
    }

    if (command == "team" || command == "teams")
        && (rest0 == Some("help") || (args.has("help") && rest.get(1).is_none()))
    {
        if json {
            return Ok(print(
                &json!({ "usage": "nexus team <command>", "commands": ["list", "tree", "show", "create", "move", "rename", "delete", "add", "remove", "pending", "import", "use"] }),
            ));
        }
        for line in teams::team_help(style) {
            out(line);
        }
        return Ok(0);
    }
    if command == "audit" && (rest0 == Some("help") || args.has("help")) {
        if json {
            return Ok(print(
                &json!({ "usage": "nexus audit [--since 7d] [--until T] [--action A] [--actor EMAIL] [--team SLUG] [--target ID] [--limit N] [--cursor C] [--json] [--csv]" }),
            ));
        }
        for line in audit::audit_help(style) {
            out(line);
        }
        return Ok(0);
    }
    if command == "search" && args.has("help") {
        if json {
            return Ok(print(
                &json!({ "usage": "nexus search \"<query>\" [--type chats|artifacts|people|teams|models] [--limit N] [--json]" }),
            ));
        }
        for line in search::search_help(style) {
            out(line);
        }
        return Ok(0);
    }
    if command == "policy" && (rest0 == Some("help") || args.has("help")) {
        if json {
            return Ok(print(&json!({
                "usage": format!("nexus policy {}", POLICY_VERBS.join("|")),
                "commands": POLICY_VERBS,
            })));
        }
        for line in models::policy_help(style) {
            out(line);
        }
        return Ok(0);
    }
    if command == "budgets" && (rest0 == Some("help") || args.has("help")) {
        if json {
            return Ok(print(
                &json!({ "usage": "nexus budgets <command>", "commands": ["list", "show", "default", "set", "managers"] }),
            ));
        }
        for line in budgets::budgets_help(style) {
            out(line);
        }
        return Ok(0);
    }
    if command == "billing" && (rest0 == Some("help") || args.has("help")) {
        if json {
            let commands = vec![
                "show",
                "card",
                "statement",
                "statements",
                "receipts",
                "portal",
                "credits",
                "auto-topup",
                "after-the-fact",
            ];
            #[cfg(feature = "plans")]
            let commands = {
                let mut commands = commands;
                commands.splice(2..2, ["plan", "plans"]);
                commands
            };
            return Ok(print(
                &json!({ "usage": "nexus billing <command>", "commands": commands }),
            ));
        }
        for line in billing::billing_help(style) {
            out(line);
        }
        return Ok(0);
    }
    if (command == "pages" || command == "page")
        && (rest0 == Some("help") || (args.has("help") && rest.get(1).is_none()))
    {
        if json {
            return Ok(print(
                &json!({ "usage": "nexus pages <command>", "commands": pages::page_command_names() }),
            ));
        }
        for line in pages::pages_help(style) {
            out(line);
        }
        return Ok(0);
    }
    if (command == "design-system" || command == "design-systems")
        && (rest0 == Some("help") || (args.has("help") && rest.get(1).is_none()))
    {
        if json {
            return Ok(print(&json!({
                "usage": "nexus design-system <command>",
                "commands": design_systems::design_system_command_names(),
            })));
        }
        for line in design_systems::design_system_help(style) {
            out(line);
        }
        return Ok(0);
    }
    if (command == "skills" || command == "skill")
        && (rest0 == Some("help") || (args.has("help") && rest.get(1).is_none()))
    {
        if json {
            return Ok(print(
                &json!({ "usage": "nexus skills <command>", "commands": skills::skill_command_names() }),
            ));
        }
        for line in skills::skills_help(style) {
            out(line);
        }
        return Ok(0);
    }
    if command == "artifacts" && (rest0 == Some("help") || (args.has("help") && rest.get(1).is_none())) {
        if json {
            return Ok(print(
                &json!({ "usage": "nexus artifacts <command>", "commands": artifacts::artifact_command_names() }),
            ));
        }
        for line in artifacts::artifacts_help(style) {
            out(line);
        }
        return Ok(0);
    }
    // `nexus org` (`nexus orgs` is the same command): your organizations, switching, and the settings.
    if (command == "org" || command == "orgs")
        && (rest0 == Some("help") || rest.get(1).map(String::as_str) == Some("help") || args.has("help"))
    {
        if json {
            return Ok(print(
                &json!({ "usage": "nexus org <command>", "commands": org::org_command_names() }),
            ));
        }
        for line in org::org_help(style) {
            out(line);
        }
        return Ok(0);
    }
    // Devices for encrypted session sync: this computer, pairing, revoking.
    if (command == "devices" || command == "device")
        && (rest0 == Some("help") || (args.has("help") && rest.get(1).is_none()))
    {
        if json {
            return Ok(print(
                &json!({ "usage": "nexus devices <command>", "commands": ["list", "pair", "revoke", "control"] }),
            ));
        }
        for line in crate::devices::devices_help(style) {
            out(line);
        }
        return Ok(0);
    }
    if command == "members" && (rest0 == Some("help") || (args.has("help") && rest.get(1).is_none())) {
        if json {
            let commands: Vec<&str> = MEMBER_COMMANDS
                .iter()
                .map(|(name, _)| name.split(' ').next().unwrap_or(""))
                .collect();
            return Ok(print(
                &json!({ "usage": "nexus members <command>", "commands": commands }),
            ));
        }
        for line in members::members_help(style) {
            out(line);
        }
        return Ok(0);
    }
    if command == "service-accounts" && (rest0 == Some("help") || args.has("help")) {
        if json {
            return Ok(print(&json!({
                "usage": "nexus service-accounts <command>",
                "commands": service_accounts::service_account_command_names(),
            })));
        }
        for line in service_accounts::service_accounts_help(style) {
            out(line);
        }
        return Ok(0);
    }
    if command == "slack"
        && (rest0 == Some("help") || rest.get(1).map(String::as_str) == Some("help") || args.has("help"))
    {
        if json {
            return Ok(print(&json!({
                "usage": "nexus slack <command>",
                "commands": slack::slack_command_names(),
            })));
        }
        for line in slack::slack_help(style) {
            out(line);
        }
        return Ok(0);
    }

    // Coding agents: what this machine has of each, the route each takes, who pays, and what is left to set up.
    // Signed out, this machine only.
    // `--csv` and `--jsonl` are the organization's agent sessions: signed in, below, and only on
    // `nexus agents` alone; help still wins.
    let help = rest0 == Some("help") || args.has("help");
    let file = args.has("csv") || args.has("jsonl");
    // `nexus agents share` shares a Claude Code session: signed in, with the API commands below.
    let agent_share = rest0 == Some("share") && !help;
    if (command == "agents" || command == "agent") && !(file && rest.is_empty() && !help) && !agent_share && !agent_chat
    {
        if help {
            if json {
                return Ok(print(&json!({
                    "usage": "nexus agents <command>",
                    "commands": agent_routes::agent_command_names(),
                })));
            }
            for line in agent_routes::agents_help(style) {
                out(line);
            }
            return Ok(0);
        }
        if file {
            return usage(
                "--csv and --jsonl write the organization's agent sessions: use them with nexus agents alone.",
                None,
            );
        }
        return agent_routes::agents_command(&rest, ctx, config, style, json);
    }

    let Some(api_key) = &config.api_key else {
        // Without an account (NEXUS_BYO_PROVIDERS): chat says how to get one or use an agent instead; any other
        // command says it needs an account, and where to make one.
        if crate::byo::enabled(ctx) {
            if command == "chat" || agent_chat {
                return Err(crate::byo::chat_requires_account(ctx, config).into());
            }
            let what = match rest0.filter(|_| command == "auth" || command == "agents" || command == "agent") {
                Some(verb) => format!("`nexus {command} {verb}`"),
                None => format!("`nexus {command}`"),
            };
            return Err(crate::byo::account_required(&what, ctx, config).into());
        }
        return Err(CliError::hint(
            "You are not logged in.",
            format!(
                "Run `nexus auth login --url {}` (in CI: `--with-token`, with an API key on stdin).",
                if config.url.is_empty() {
                    DEFAULT_URL
                } else {
                    &config.url
                }
            ),
        )
        .into());
    };
    let cmd = Cmd {
        core: cli_client(&config.url, Some(api_key)),
        style,
        json,
        args,
        ctx,
        config,
    };

    match command {
        "team" | "teams" => {
            if command == "teams" && rest.is_empty() {
                return teams::team_command(&["list".to_string()], &cmd);
            }
            return teams::team_command(&rest, &cmd);
        }
        // Your API keys and CLI sign-ins: `nexus auth tokens`, the only `auth` command that gets here.
        "auth" => return tokens::tokens_command(rest.get(1..).unwrap_or(&[]), &cmd),
        "members" => return members::members_command(&rest, &cmd),
        // Devices for encrypted session sync: this computer as a trusted device.
        "devices" | "device" => return crate::devices::devices_command(&rest, &cmd),
        "audit" => return audit::audit_command(&cmd),
        // MCP: Gateway's read operations as tools for an agent, on stdio, under this sign-in.
        "mcp" => return mcp::serve(&cmd),
        // Search (⌘K in Nexus UI): what your role may read, grouped and ranked.
        "search" => {
            let ui = config
                .ui_url
                .clone()
                .unwrap_or_else(|| ui_url(&config.url, &ctx.env_fn()));
            return search::search_command(&rest, &cmd, &ui);
        }
        "analytics" => return teams::analytics_command(&cmd, None),
        "budget" => return teams::budget_command(&rest, &cmd),
        "budgets" => return budgets::budgets_command(&rest, &cmd),
        // Billing (Rust gateway, owners): the card on file through Stripe Checkout, an unpaid month, the statements.
        "billing" => return billing::billing_command(&rest, &cmd),
        "policy" => return models::policy_command(&rest, &cmd),
        // Service accounts: callers for CI and scripts, and their keys; owners and admins.
        "service-accounts" => return service_accounts::service_accounts_command(&rest, &cmd),
        // The Nexus Slack app: bind a Slack workspace to the organization, owners and admins; and a channel
        // of it to a team, owners and admins and team leads for their own teams.
        "slack" => return slack::slack_command(&rest, &cmd),
        // Connections: your accounts and the organization's at a connector's vendor.
        "connections" => return connections::connections_command(&cmd),
        "connect" => return connections::connect_command(&rest, &cmd),
        "disconnect" => return connections::disconnect_command(&rest, &cmd),
        // Jobs: a module's verb run in the background, read, cancelled, and its result.
        "jobs" => return jobs::jobs_command(&rest, &cmd),
        // Routines (W4.1): your routines, such as a morning brief, on a schedule.
        "routines" => return routines::routines_command(&rest, &cmd),
        // Nexus Apps: a folder deployed to Vercel on Nexus's team; off unless the Gateway turns it on.
        "apps" => return apps::apps_command(&rest, &cmd),
        // Notifications: your inbox, as Nexus UI's notifications page shows it.
        "notifications" => return notifications::notifications_command(&rest, &cmd),
        // Cloud machines: yours, read and put to sleep, as Nexus for desktop does.
        "machines" => return machines::machines_command(&rest, &cmd),
        // Triage: an agent diagnoses a defect on this computer; the approved report goes to the Nexus team.
        #[cfg(feature = "staff")]
        "triage" => return crate::triage::triage_command(&rest, &cmd),
        "models" => return models::models_command(&rest, &cmd, saved.chat_model()),
        "chat" => return chat::chat_command(&rest, &cmd, saved.chat_session.as_deref()),
        "artifacts" | "artifact" => return artifacts::artifacts_command(&rest, &cmd),
        // Pages: standalone HTML or Markdown in versions that never change.
        "pages" | "page" => return pages::pages_command(&rest, &cmd),
        // Nexus Design's design systems: the tokens and notes designs are made, served and checked with.
        "design-system" | "design-systems" => return design_systems::design_system_command(&rest, &cmd),
        // Skills: a person's own SKILL.md files, in versions that never change.
        "skills" | "skill" => return skills::skills_command(&rest, &cmd),
        // Your organizations (Rust gateway): Personal first, then your companies; switch with a new token. And the
        // organization's settings: chat retention, subscription mode, sharing artifacts outside it.
        "org" | "orgs" => return org::org_command(&rest, &cmd),
        "keys" => return keys(&rest, &cmd),
        // Nexus, the personal agent (Agent Interfaces S2.2): the same one as @nexus in Slack, through the agent API.
        "agent" if agent_chat => return crate::agent_chat::agent_chat_command(&rest, &cmd),
        // A Claude Code session shared as an artifact, from its transcript on this machine.
        "agents" | "agent" if agent_share => return agent_share::agent_share_command(&rest[1..], &cmd),
        "agents" | "agent" => {
            let format = exports::format(&cmd)?.unwrap_or("csv");
            let query = exports::filters(&cmd, exports::SESSION_FILTERS, None);
            return exports::write_file(&cmd, "/v1/agents/sessions/export", format, &query);
        }
        "spend" => {
            if let Some(code) = spend::spend_calls_command(&rest, &cmd)? {
                return Ok(code);
            }
            if let Some(by) = args.flag_set("by") {
                return teams::analytics_command(&cmd, Some(by));
            }
            let spend = cmd.core.get("/v1/spend")?;
            let manifests = cmd
                .core
                .get("/v1/modules")
                .map(|r| a(&r, "modules").to_vec())
                .unwrap_or_default();
            if json {
                return Ok(print(&spend));
            }
            let person = cmd.core.get("/v1/people/me/budget").ok();
            for line in spend_lines(style, &spend, &manifests, person.as_ref()) {
                out(line);
            }
            return Ok(0);
        }
        "modules" => {
            let response = cmd.core.get("/v1/modules")?;
            let modules = a(&response, "modules");
            let unavailable = a(&response, "unavailable");
            if json {
                return Ok(print(
                    &json!({ "modules": response.get("modules"), "unavailable": response.get("unavailable") }),
                ));
            }
            if modules.is_empty() {
                out("No modules are registered with this Gateway.".into());
            } else {
                let rows: Vec<Vec<String>> = modules
                    .iter()
                    .map(|m| {
                        vec![
                            js_opt(m.pointer("/product/slug").and_then(Value::as_str)),
                            js_opt(m.pointer("/capability/slug").and_then(Value::as_str)),
                            format!(
                                "{} › {}",
                                js_opt(m.pointer("/product/name").and_then(Value::as_str)),
                                js_opt(m.pointer("/capability/name").and_then(Value::as_str))
                            ),
                            strings(a(m, "verbs")).join(", "),
                        ]
                    })
                    .collect();
                for line in list(
                    style,
                    &[col("Product"), col("Capability"), col("Name"), flex("Verbs")],
                    rows_of(rows),
                    None,
                ) {
                    out(line);
                }
            }
            for u in unavailable {
                ctx.io.err(&format!(
                    "{} Module at {} is not answering ({}).\n  {} Start it, or remove it from NEXUS_MODULES on Gateway.",
                    style.warn,
                    js_string(u.get("url")),
                    js_string(u.get("error")),
                    style.arrow
                ));
            }
            return Ok(0);
        }
        "connectors" => {
            let response = cmd.core.get("/v1/connectors")?;
            if json {
                return Ok(print(&json!({ "connectors": response.get("connectors") })));
            }
            let rows: Vec<Vec<String>> = a(&response, "connectors")
                .iter()
                .map(|c| {
                    vec![
                        js_string(c.get("name")),
                        a(c, "operations")
                            .iter()
                            .map(|op| js_string(op.get("name")))
                            .collect::<Vec<_>>()
                            .join(", "),
                        js_string(c.get("description")),
                    ]
                })
                .collect();
            for line in list(
                style,
                &[col("Connector"), col("Operations"), flex("Description")],
                rows_of(rows),
                None,
            ) {
                out(line);
            }
            return Ok(0);
        }
        _ => {}
    }

    product_command(command, &rest, &cmd)
}

fn save(ctx: &Context, config: &Config) -> Result<()> {
    save_with(ctx, config, save_config)
}

/// A sign-in from `nexus auth login`: `--url` also makes its Gateway the default, as it always did; without it, the
/// sign-in is saved for the chosen Gateway (`NEXUS_GATEWAY`, else the saved one).
fn save_login(ctx: &Context, args: &Args, config: &Config) -> Result<()> {
    if args.flag("url").is_some() || args.flag("env").is_some() {
        save_with(ctx, config, save_default_config)
    } else {
        save(ctx, config)
    }
}

fn save_with(ctx: &Context, config: &Config, write: fn(&Path, &Config) -> std::io::Result<()>) -> Result<()> {
    write(&ctx.config_path, config).map_err(|error| {
        Error::Cli(CliError::new(format!(
            "Could not write {}: {error}",
            ctx.config_path.display()
        )))
    })
}

/// `String(x)` of an optional string as a template prints it (`undefined` when absent).
pub fn js_opt(value: Option<&str>) -> String {
    value.map(String::from).unwrap_or_else(|| "undefined".into())
}

pub fn strings(values: &[Value]) -> Vec<String> {
    values.iter().map(|value| js_string(Some(value))).collect()
}

fn keys(rest: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = rest.first().map(String::as_str);
    let provider = rest.get(1).filter(|p| !p.is_empty());
    let key = rest.get(2).filter(|k| !k.is_empty());
    if let (Some("add"), Some(provider), Some(key)) = (action, provider, key) {
        let holder = crate::provider_keys::holder(cmd)?;
        let saved = cmd.core.post(
            "/v1/keys/providers",
            crate::provider_keys::with_holder(json!({ "provider": provider, "key": key }), &holder),
        )?;
        if cmd.json {
            return Ok(cmd.print(&saved));
        }
        let last4 = js_string(saved.get("last4"));
        let words = crate::provider_keys::holder_words(&holder);
        cmd.out(if s(&saved, "state") == Some("unreachable") {
            format!(
                "{} Saved your {provider} key {}{last4}{words}, but {provider} could not check it. Add it again later to check it.",
                style.warn,
                style.dim("••••")
            )
        } else if s(&saved, "state") == Some("disabled") {
            format!(
                "{} Saved your {provider} key {}{last4}{words}. It stays disabled: {} to use it.",
                style.warn,
                style.dim("••••"),
                style.cyan(&format!(
                    "nexus keys enable {provider}{}",
                    crate::provider_keys::holder_flags(&holder)
                ))
            )
        } else if holder.is_empty() {
            format!(
                "{} Saved your {provider} key {}{last4}. Encrypted at rest; used for your organization only.",
                style.ok,
                style.dim("••••")
            )
        } else {
            format!(
                "{} Saved your {provider} key {}{last4}{words}. Encrypted at rest; calls{words} use it before the organization's key.",
                style.ok,
                style.dim("••••")
            )
        });
        return Ok(0);
    }
    if let (Some("history"), Some(provider)) = (action, provider) {
        return crate::provider_keys::history(provider, cmd);
    }
    if let (Some("replace"), Some(provider), Some(key)) = (action, provider, key) {
        return crate::provider_keys::change("replace", provider, Some(key), cmd);
    }
    if let (Some(verb @ ("disable" | "enable" | "remove")), Some(provider)) = (action, provider) {
        return crate::provider_keys::change(verb, provider, None, cmd);
    }
    if let (Some("revoke"), Some(id)) = (action, rest.get(1).filter(|id| !id.is_empty())) {
        let dry_run = cmd.has("dry-run");
        let revoked = cmd
            .core
            .post("/v1/auth/scoped-keys/revoke", json!({ "id": id, "dryRun": dry_run }))?;
        if cmd.json {
            return Ok(cmd.print(&revoked));
        }
        let key = js_string(revoked.get("id"));
        cmd.out(if t(&revoked, "already") {
            format!("{} {key} was already revoked.", style.ok)
        } else if dry_run {
            format!(
                "{} Would revoke {key}: the module's key stops working at once. Run without --dry-run to revoke it.",
                style.warn
            )
        } else {
            format!("{} Revoked {key}: the module's key no longer works.", style.ok)
        });
        return Ok(0);
    }
    if action == Some("list") || (action.is_none() && !cmd.has("help")) {
        let response = cmd.core.get("/v1/keys/providers")?;
        // A team's or a person's keys: owners and admins read all, anyone else their teams' and their own.
        let scoped = a(&response, "scopedKeys");
        if cmd.json {
            let mut printed = json!({ "providers": response.get("providers") });
            if !scoped.is_empty() {
                printed["scopedKeys"] = json!(scoped);
            }
            return Ok(cmd.print(&printed));
        }
        let providers = a(&response, "providers");
        if providers.is_empty() && scoped.is_empty() {
            cmd.out(format!(
                "No provider keys yet. Add one: {}",
                style.cyan("nexus keys add anthropic sk-ant-…")
            ));
            return Ok(0);
        }
        let notice = |p: &Value| s(p, "notice").map(String::from);
        let holder_of = |p: &Value| match s(p, "scope") {
            Some("team") => format!("team {}", js_string(p.get("scopeId"))),
            Some("user") => js_string(p.get("scopeId")),
            _ => "organization".to_string(),
        };
        let rows: Vec<Vec<String>> = providers
            .iter()
            .chain(scoped)
            .map(|p| {
                let mut row = vec![
                    js_string(p.get("provider")),
                    format!("••••{}", js_string(p.get("last4"))),
                    ago(s(p, "createdAt").unwrap_or("")),
                    crate::provider_keys::last_used(p),
                    crate::provider_keys::status(style, p),
                ];
                if !scoped.is_empty() {
                    row.insert(1, holder_of(p));
                }
                row
            })
            .collect();
        let mut columns = vec![
            col("Provider"),
            col("Key"),
            col("Added"),
            col("Last used"),
            flex("Status"),
        ];
        if !scoped.is_empty() {
            columns.insert(1, col("Holder"));
        }
        cmd.lines(list(style, &columns, rows_of(rows), None));
        // A stored OpenRouter key is kept and listed, never used; the Status column covers a disabled key.
        let retired = |p: &&Value| !matches!(p.get("state").and_then(Value::as_str), Some("disabled" | "removed"));
        for p in providers.iter().filter(|p| t(p, "unused")).filter(retired) {
            let text = notice(p).unwrap_or_else(|| format!("{} is no longer used", js_string(p.get("provider"))));
            cmd.ctx
                .io
                .err(&format!("{} {text}: Nexus calls model providers directly.", style.warn));
        }
        return Ok(0);
    }
    // The TypeScript CLI is the parity reference and prints `<provider>`. Print the same text. The catalog list of BYOK
    // providers belongs in `nexus providers`, not in this line.
    usage(
        "Usage: nexus keys add <provider> <key> | nexus keys list | nexus keys history <provider> | nexus keys replace <provider> <key> [--dry-run] | nexus keys disable|enable|remove <provider> [--dry-run] | nexus keys revoke <scoped key id> [--dry-run]; --team <slug|id> or --user <me|id> names a team's or a person's key",
        Some(
            "Bring your own provider key (owners and admins); Gateway calls the provider with it for your organization only.",
        ),
    )
}

// ---------------------------------------------------------------- auth

pub use nexus_client::staff::is_employee;

/// `--env dev` is for Nexus employees: the person signed in to production must be one. This only keeps
/// the shortcut internal. `--url` still reaches any Gateway, and each Gateway checks its own sign-ins.
fn require_employee(ctx: &Context) -> Result<()> {
    let refused = || {
        Error::Cli(CliError::hint(
            "--env dev is for Nexus employees.",
            "Sign in to production with your Nexus account first: `nexus auth login --env prod`.",
        ))
    };
    let production = load_config(&ctx.config_path, &|name| {
        (name == "NEXUS_GATEWAY").then(|| GATEWAY_URL.to_string())
    });
    let Some(api_key) = &production.api_key else {
        return Err(refused());
    };
    let me = cli_client(GATEWAY_URL, Some(api_key)).get("/v1/tenants/me")?;
    if is_employee(&me) { Ok(()) } else { Err(refused()) }
}

fn login(args: &Args, config: &Config, ctx: &Context, style: &Style, json: bool) -> Result<i32> {
    let raw = match (args.flag("env"), args.flag("url")) {
        (Some(_), Some(_)) => return usage("Use --url or --env, not both.", None),
        (Some(name), None) => match environment(name) {
            Some(env) => {
                if env.url != GATEWAY_URL {
                    require_employee(ctx)?;
                }
                env.url.to_string()
            }
            None => return usage(format!("Unknown environment: {name}."), Some("Use prod or dev.")),
        },
        (None, url) => url.map(String::from).unwrap_or_else(|| config.url.clone()),
    };
    let url = raw.strip_suffix('/').unwrap_or(&raw).to_string();
    let signup = args.positionals.get(1).is_some_and(|action| action == "signup");
    if signup && args.has("dry-run") {
        if json {
            ctx.io.out(&pretty(&obj(vec![
                ("dryRun", Some(json!(true))),
                ("url", Some(json!(url))),
                ("name", args.flag("name").map(|name| json!(name))),
                (
                    "email",
                    args.flag("email")
                        .filter(|email| !email.is_empty())
                        .map(|email| json!(email)),
                ),
            ])));
        } else {
            ctx.io.out(&format!(
                "Would create organization {} on {url}. Nothing was created.",
                args.flag("name").unwrap_or_default()
            ));
        }
        return Ok(0);
    }
    let ui = ui_url(&url, &ctx.env_fn());
    let out = |line: String| ctx.io.out(&line);
    // `--with-token` reads an API key from stdin (CI); `--key` is its older, hidden spelling. `--name` creates a
    // key-only organization (local development). Otherwise: the browser (device flow).
    let mut api_key = args.flag("key").map(String::from);
    if args.has("with-token") {
        let read = ctx.io.read_stdin().trim().to_string();
        if read.is_empty() {
            return usage(
                "No API key on stdin.",
                Some("Pipe one in: `echo \"$NEXUS_API_KEY\" | nexus auth login --with-token`."),
            );
        }
        api_key = Some(read);
    }
    let api_key = api_key.filter(|key| !key.is_empty());
    if api_key.is_none() && args.flag("name").is_none() {
        return device_login(&url, &ui, args, ctx, style, json);
    }
    let config_path = ctx.config_path.display().to_string();
    let saved_with = |key: &str| Config {
        url: url.clone(),
        api_key: Some(key.to_string()),
        kind: Some("api".into()),
        ..Config::default()
    };
    let created: Option<(String, f64)>;
    let api_key = if let Some(api_key) = api_key {
        let client = cli_client(&url, Some(&api_key));
        let spend = client.get("/v1/spend")?;
        let me = client.get("/v1/tenants/me").ok();
        let tenant = me.as_ref().and_then(|me| me.get("tenant")).filter(|t| !t.is_null());
        let tenant_name = match &me {
            Some(me) => me
                .pointer("/tenant/slug")
                .and_then(Value::as_str)
                .or_else(|| me.pointer("/tenant/name").and_then(Value::as_str))
                .map(String::from)
                .unwrap_or_else(|| "undefined".into()),
            None => "your organization".into(),
        };
        save_login(ctx, args, &with_org(saved_with(&api_key), tenant))?;
        if json {
            ctx.io.out(&pretty(&obj(vec![
                ("url", Some(json!(url))),
                ("tenant", Some(json!({ "name": tenant_name }))),
                ("configPath", Some(json!(config_path))),
                ("remainingUsd", spend.get("remainingUsd").cloned()),
            ])));
            return Ok(0);
        }
        out(format!(
            "{} Logged in to {} as {} {} {}",
            style.ok,
            style.bold(&url),
            style.bold(&tenant_name),
            style.bullet,
            left_text(&spend)
        ));
        created = None;
        api_key
    } else {
        let name = args.flag_set("name").unwrap_or("My organization");
        let result = cli_client(&url, None).post(
            "/v1/tenants",
            obj(vec![
                ("name", Some(json!(name))),
                ("email", args.flag("email").map(|e| json!(e))),
            ]),
        )?;
        let api_key = js_string(result.get("apiKey"));
        let tenant = result.get("tenant").cloned().unwrap_or(Value::Null);
        save_login(ctx, args, &with_org(saved_with(&api_key), Some(&tenant)))?;
        if json {
            ctx.io.out(&pretty(&obj(vec![
                ("url", Some(json!(url))),
                ("tenant", result.get("tenant").cloned()),
                ("apiKey", result.get("apiKey").cloned()),
                ("configPath", Some(json!(config_path))),
                ("ui", Some(json!(format!("{ui}/login#key={api_key}")))),
            ])));
            return Ok(0);
        }
        let budget = n(&tenant, "budgetUsd");
        out(format!(
            "{} Created organization {} on {} {} budget {}{}",
            style.ok,
            style.bold(&js_string(tenant.get("name"))),
            style.bold(&url),
            style.bullet,
            usd(budget),
            if signup {
                String::new()
            } else {
                format!(" {}", style.dim("(local development)"))
            }
        ));
        created = Some((js_string(tenant.get("id")), budget));
        api_key
    };
    let mut entries = Vec::new();
    if let Some((id, _)) = &created {
        entries.push(kv("Organization", id.clone()));
        entries.push(kv(
            "API key",
            format!("{api_key} {}", style.dim("(shown once; saved for you)")),
        ));
    }
    entries.push(kv("Config", config_path));
    entries.push(kv(
        "Nexus UI",
        if created.is_some() {
            format!("{ui}/login#key={api_key}")
        } else {
            format!("{ui}/login")
        },
    ));
    out(String::new());
    for line in pairs(style, &entries) {
        out(line);
    }
    out(String::new());
    out(format!(
        "{} Next: {} lists the products on this Gateway.",
        style.arrow,
        style.cyan("nexus help")
    ));
    Ok(0)
}

/// An organization's segment in Nexus UI's URLs: its slug, else its sign-in provider's org id, else its tenant id
/// (`orgSegment` in `config.ts`).
fn org_segment(tenant: &Value) -> Option<String> {
    ["slug", "clerkOrgId", "id"]
        .into_iter()
        .find_map(|field| s(tenant, field).filter(|value| !value.is_empty()))
        .map(String::from)
}

/// `config` with the organization's segment kept (`org`, for Nexus UI's links), as the TypeScript CLI saves it.
fn with_org(mut config: Config, tenant: Option<&Value>) -> Config {
    if let Some(org) = tenant.and_then(org_segment) {
        config.extra.insert("org".into(), json!(org));
    }
    config
}

/// `nexus auth login`: a code from Gateway, Nexus UI's approval page, and polls until a member approves it.
fn device_login(url: &str, ui: &str, args: &Args, ctx: &Context, style: &Style, json: bool) -> Result<i32> {
    let core = Client::new(ClientOptions {
        api_key: None,
        headers: vec![(CLI_VERSION_HEADER.to_string(), VERSION.to_string())],
        idempotency: false,
        client: Some(format!("cli/{VERSION}")),
        timeout: Duration::from_secs(120),
        ..ClientOptions::new(url)
    });
    let info = |line: String| if json { ctx.io.err(&line) } else { ctx.io.out(&line) };
    let host = ctx.hostname.clone().unwrap_or_else(|| "this machine".into());
    let started = core.post(
        device::START_PATH,
        device::start_body(&format!("nexus CLI on {host}"), Some(device::CLI_CLIENT_ID)),
    )?;
    let user_code = js_string(started.get("user_code"));
    let device_code = js_string(started.get("device_code"));
    let link = s(&started, "verification_uri_complete")
        .map(String::from)
        .unwrap_or_else(|| format!("{ui}/device?code={}", enc(&user_code)));
    info(format!("{} Your one-time code: {}", style.info, style.bold(&user_code)));
    info(format!(
        "  {} Approve it in Nexus UI: {}",
        style.arrow,
        style.cyan(&link)
    ));
    // Gateway's link opens only on its own Gateway or Nexus UI; any other is printed (PT-12).
    if !args.has("no-browser") && crate::same_origin(&link, &[url, ui]) && ctx.io.open_url(&link) {
        info(format!("  {}", style.dim("Opened your browser. Waiting for approval…")));
    } else {
        info(format!(
            "  {}",
            style.dim("Open the link in a browser. Waiting for approval…")
        ));
    }

    let mut interval = n(&started, "interval");
    let deadline = Instant::now() + Duration::from_secs_f64(n(&started, "expires_in").max(0.0));
    let result = (|| -> Result<i32> {
        loop {
            ctx.io.progress(Some(&format!("Waiting for approval of {user_code}…")));
            let wait = ctx.poll_ms.unwrap_or((interval * 1000.0).max(0.0) as u64);
            std::thread::sleep(Duration::from_millis(wait));
            let polled = match core.post(device::TOKEN_PATH, device::token_body(&device_code)) {
                Ok(granted) => Ok(granted),
                Err(Error::Core(error)) => Err(nexus_client::Error::Core(error)),
                Err(Error::Transport(error)) => Err(nexus_client::Error::Transport(error)),
                Err(error) => return Err(error),
            };
            match device::classify(polled, interval, Instant::now() > deadline) {
                Poll::Granted(granted) => {
                    ctx.io.progress(None);
                    let token = js_string(granted.get("access_token"));
                    let saved = with_org(
                        Config {
                            url: url.to_string(),
                            api_key: Some(token),
                            kind: Some("cli".into()),
                            ..Config::default()
                        },
                        granted.get("tenant"),
                    );
                    save_login(ctx, args, &saved)?;
                    let user = granted.get("user").cloned().unwrap_or(Value::Null);
                    let tenant = granted.get("tenant").cloned().unwrap_or(Value::Null);
                    let who = s(&user, "email")
                        .map(String::from)
                        .unwrap_or_else(|| js_string(user.get("id")));
                    let org = s(&tenant, "slug")
                        .map(String::from)
                        .unwrap_or_else(|| js_string(tenant.get("name")));
                    let config_path = ctx.config_path.display().to_string();
                    if json {
                        ctx.io.out(&pretty(&obj(vec![
                            ("url", Some(json!(url))),
                            ("org", granted.get("tenant").cloned()),
                            ("user", granted.get("user").cloned()),
                            ("role", granted.get("role").cloned()),
                            ("expiresAt", granted.get("expires_at").cloned()),
                            ("configPath", Some(json!(config_path))),
                        ])));
                        return Ok(0);
                    }
                    let until = s(&granted, "expires_at")
                        .filter(|at| !at.is_empty())
                        .map(|at| format!(", until {}", at.chars().take(10).collect::<String>()))
                        .unwrap_or_default();
                    ctx.io.out(&format!(
                        "{} Signed in to {} as {} {}",
                        style.ok,
                        style.bold(&org),
                        style.bold(&who),
                        style.dim(&format!("({})", js_string(granted.get("role"))))
                    ));
                    ctx.io.out("");
                    for line in pairs(
                        style,
                        &[
                            kv("Gateway", url),
                            kv("Token", format!("saved to {config_path}{until}")),
                            kv(
                                "Nexus UI",
                                crate::artifacts::ui_link(
                                    &Config {
                                        ui_url: Some(ui.to_string()),
                                        ..saved.clone()
                                    },
                                    "",
                                ),
                            ),
                        ],
                    ) {
                        ctx.io.out(&line);
                    }
                    ctx.io.out("");
                    ctx.io.out(&format!(
                        "{} Next: {} lists the products on this Gateway.",
                        style.arrow,
                        style.cyan("nexus help")
                    ));
                    return Ok(0);
                }
                Poll::SlowDown(next) => interval = next,
                Poll::Denied => {
                    return Err(CliError::hint(
                        "The login was denied in Nexus UI.",
                        "Run `nexus auth login` again to start over.",
                    )
                    .into());
                }
                Poll::Expired => {
                    return Err(CliError::hint(
                        "The login code expired before it was approved.",
                        "Run `nexus auth login` again; codes last ten minutes.",
                    )
                    .into());
                }
                Poll::Pending => {}
                Poll::Failed(error) => return Err(error.into()),
            }
        }
    })();
    ctx.io.progress(None);
    result
}

fn auth_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus auth <command> [options]", style.bold("Usage")),
        String::new(),
    ];
    lines.extend(
        AUTH_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 31)))),
    );
    lines.extend(
        AUTH_TOKEN_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(&format!("tokens {name}"), 31)))),
    );
    lines.push(String::new());
    lines.push(style.dim("Your token is saved in ~/.nexus/factory/config.json (NEXUS_CONFIG), readable by you only."));
    lines.push(style.dim(
        "Another Gateway: sign in once with NEXUS_GATEWAY=<url> nexus auth login, then set NEXUS_GATEWAY=<url> on any command.",
    ));
    lines
}

/// Commands with a help screen of their own, for `--help` alone or with one argument; `tests/help.rs` runs `--help` on
/// every command.
const OWN_HELP: [&str; 31] = [
    "auth",
    "team",
    "teams",
    "audit",
    "search",
    "policy",
    "budgets",
    "billing",
    "artifacts",
    "artifact",
    "pages",
    "page",
    "design-system",
    "design-systems",
    "skills",
    "skill",
    "org",
    "orgs",
    "members",
    "service-accounts",
    "slack",
    "agents",
    "agent",
    "chat",
    "recordings",
    "recording",
    "memory",
    "sessions",
    "session",
    "completion",
    "update",
];

/// The catalog's name for a command's alias.
const HELP_ALIASES: [(&str, &str); 11] = [
    ("design-systems", "design-system"),
    ("teams", "team"),
    ("orgs", "org"),
    ("artifact", "artifacts"),
    ("page", "pages"),
    ("skill", "skills"),
    ("agent", "agents"),
    ("recording", "recordings"),
    ("session", "sessions"),
    ("login", "auth login"),
    ("logout", "auth logout"),
];

/// A command's help from its catalog entry: usage, what it does, its subcommands, flags and examples.
fn command_help(style: &Style, spec: &Value) -> Vec<String> {
    let rows = |items: Vec<(String, String)>| -> Vec<String> {
        let width = (items.iter().map(|(name, _)| js_len(name)).max().unwrap_or(0) + 2).min(40);
        items
            .into_iter()
            .flat_map(|(name, text)| {
                if js_len(&name) + 2 <= width {
                    vec![format!("  {}{text}", style.cyan(&pad_end(&name, width)))]
                } else {
                    vec![format!("  {}", style.cyan(&name)), format!("      {text}")]
                }
            })
            .collect()
    };
    let field = |value: &Value, key: &str| s(value, key).unwrap_or_default().to_string();
    let subcommands: Vec<(String, String)> = a(spec, "subcommands")
        .iter()
        .map(|sub| {
            let usage = field(sub, "usage");
            (
                usage.strip_prefix("nexus ").unwrap_or(&usage).to_string(),
                field(sub, "description"),
            )
        })
        .collect();
    let flags: Vec<(String, String)> = a(spec, "flags")
        .iter()
        .map(|flag| match s(flag, "value") {
            Some(value) => (format!("{} {value}", field(flag, "name")), field(flag, "description")),
            None => (field(flag, "name"), field(flag, "description")),
        })
        .collect();
    let mut lines = vec![format!("{}  {}", style.bold("Usage"), field(spec, "usage"))];
    lines.extend(
        a(spec, "variants")
            .iter()
            .map(|variant| format!("       {}", field(variant, "usage"))),
    );
    lines.push(String::new());
    lines.push(field(spec, "description"));
    if !subcommands.is_empty() {
        lines.extend([String::new(), style.bold("Commands")]);
        lines.extend(rows(subcommands));
    }
    if !flags.is_empty() {
        lines.extend([String::new(), style.bold("Flags")]);
        lines.extend(rows(flags));
    }
    let examples = a(spec, "examples");
    if !examples.is_empty() {
        lines.extend([String::new(), style.bold("Examples")]);
        lines.extend(
            examples
                .iter()
                .filter_map(Value::as_str)
                .map(|example| format!("  {example}")),
        );
    }
    lines
}

/// `nx_abcdef…wxyz` as `nx_abc••••wxyz`: enough to tell keys apart, not to use one.
pub fn mask_key(key: &str) -> String {
    let units: Vec<u16> = key.encode_utf16().collect();
    if units.len() <= 12 {
        return "••••".into();
    }
    format!(
        "{}••••{}",
        String::from_utf16_lossy(&units[..6]),
        String::from_utf16_lossy(&units[units.len() - 4..])
    )
}

/// `nexus auth status`: exit 0 when signed in with a key Gateway accepts, 1 otherwise.
fn auth_status(config: &Config, ctx: &Context, style: &Style, json: bool) -> Result<i32> {
    let out = |line: String| ctx.io.out(&line);
    let ui = config
        .ui_url
        .clone()
        .unwrap_or_else(|| ui_url(&config.url, &ctx.env_fn()));
    let config_path = ctx.config_path.display().to_string();
    let Some(api_key) = &config.api_key else {
        if json {
            out(pretty(
                &json!({ "url": config.url, "loggedIn": false, "configPath": config_path, "ui": ui }),
            ));
            return Ok(1);
        }
        out(format!(
            "{} Not signed in {} Gateway {}",
            style.warn,
            style.bullet,
            style.bold(&config.url)
        ));
        out(format!(
            "  {} Run {}",
            style.arrow,
            style.cyan(&format!("nexus auth login --url {}", config.url))
        ));
        return Ok(1);
    };
    let core = cli_client(&config.url, Some(api_key));
    let (me, spend, accepted) = match core
        .get("/v1/tenants/me")
        .and_then(|me| core.get("/v1/spend").map(|spend| (me, spend)))
    {
        Ok((me, spend)) => (Some(me), Some(spend), true),
        Err(Error::Core(error)) if error.status == 401 => (None, None, false),
        Err(error) => return Err(error),
    };
    let tenant = me
        .as_ref()
        .and_then(|me| me.get("tenant"))
        .filter(|t| !t.is_null())
        .cloned();
    // Keep the organization for Nexus UI's links (a config from before it was kept, or a renamed slug).
    if let Some(org) = tenant.as_ref().and_then(org_segment)
        && config.extra.get("org").and_then(Value::as_str) != Some(org.as_str())
    {
        let mut current = load_config(&ctx.config_path, &ctx.env_fn());
        current.extra.insert("org".into(), json!(org));
        save(ctx, &current)?;
    }
    let key = mask_key(api_key);
    let field = |name: &str| me.as_ref().and_then(|me| me.get(name)).cloned();
    if json {
        let summary = obj(vec![
            ("url", Some(json!(config.url))),
            ("loggedIn", Some(json!(true))),
            ("accepted", Some(json!(accepted))),
            ("tenant", tenant.clone()),
            ("user", field("user")),
            ("role", field("role")),
            ("team", field("team")),
            ("teams", field("teams")),
            ("apiKey", Some(json!(key))),
            ("configPath", Some(json!(config_path))),
            (
                "remainingUsd",
                spend.as_ref().and_then(|spend| spend.get("remainingUsd")).cloned(),
            ),
            ("ui", Some(json!(ui))),
        ]);
        out(pretty(&summary));
        return Ok(if accepted { 0 } else { 1 });
    }
    let user = field("user").filter(crate::json::truthy);
    let role = field("role").filter(crate::json::truthy);
    let user_name = |user: &Value| {
        s(user, "email")
            .map(String::from)
            .unwrap_or_else(|| js_string(user.get("id")))
    };
    if accepted {
        let org = tenant
            .as_ref()
            .and_then(|t| s(t, "slug").or_else(|| s(t, "name")))
            .unwrap_or("your organization")
            .to_string();
        out(format!(
            "{} Signed in to {}{}{}",
            style.ok,
            style.bold(&org),
            user.as_ref()
                .map(|u| format!(" as {}", style.bold(&user_name(u))))
                .unwrap_or_default(),
            role.as_ref()
                .map(|r| format!(" {}", style.dim(&format!("({})", js_string(Some(r))))))
                .unwrap_or_default()
        ));
    } else {
        out(format!(
            "{} Nexus Gateway at {} does not accept the saved key",
            style.fail,
            style.bold(&config.url)
        ));
    }
    out(String::new());
    let mut entries = vec![kv("Gateway", config.url.clone())];
    if let Some(tenant) = &tenant {
        let name = js_string(tenant.get("name"));
        let label = match s(tenant, "slug").filter(|slug| !slug.is_empty()) {
            Some(slug) => format!("{slug} {}", style.dim(&format!("· {name}"))),
            None => name,
        };
        entries.push(kv(
            "Org",
            format!("{label} {}", style.dim(&format!("({})", js_string(tenant.get("id"))))),
        ));
    }
    if let Some(user) = &user {
        entries.push(kv("User", user_name(user)));
    }
    if let Some(role) = &role {
        entries.push(kv("Role", js_string(Some(role))));
    }
    if user.is_some() {
        let me = me.clone().unwrap_or(Value::Null);
        let team = me.get("team").filter(|team| crate::json::truthy(team));
        let teams = a(&me, "teams");
        let text = match team {
            Some(team) => {
                let also = if teams.len() > 1 {
                    let others: Vec<String> = teams
                        .iter()
                        .filter(|other| other.get("id") != team.get("id"))
                        .map(|other| js_string(other.get("slug")))
                        .collect();
                    format!("; also {}", others.join(", "))
                } else {
                    String::new()
                };
                format!(
                    "{} {}",
                    js_string(team.get("slug")),
                    style.dim(&format!("(default{also})"))
                )
            }
            None => style.dim(&if teams.is_empty() {
                "none".to_string()
            } else {
                format!(
                    "none by default (in {}; nexus team use <slug>)",
                    teams
                        .iter()
                        .map(|team| js_string(team.get("slug")))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
        };
        entries.push(kv("Team", text));
    }
    entries.push(kv(
        "Token",
        format!(
            "{key} {}",
            style.dim(if accepted { "(accepted)" } else { "(rejected)" })
        ),
    ));
    if let Some(spend) = &spend {
        entries.push(if n_opt(spend, "budgetUsd").is_none() {
            kv("Left for you", usd(n(spend, "remainingUsd")))
        } else {
            kv(
                "Budget",
                format!("{}  {}", left_text(spend), budget_bar(style, budget_used(spend), 12)),
            )
        });
    }
    entries.push(kv("Config", config_path));
    entries.push(kv("Nexus UI", ui));
    for line in pairs(style, &entries) {
        out(line);
    }
    if !accepted {
        out(String::new());
        out(format!(
            "{} Run {} to sign in again (your token may have expired or been revoked).",
            style.arrow,
            style.cyan(&format!("nexus auth login --url {}", config.url))
        ));
    }
    Ok(if accepted { 0 } else { 1 })
}

/// `--json` among Nexus's own launcher flags (before the agent's first argument), as the launcher reads them.
fn launcher_wants_json(argv: &[String]) -> bool {
    agents::parse_launcher_args(argv).json
}

// ---------------------------------------------------------------- help

/// Manifests by product slug, in the order Gateway lists them.
pub fn products(manifests: &[Value]) -> Vec<(String, Vec<Value>)> {
    let mut by_product: Vec<(String, Vec<Value>)> = Vec::new();
    for manifest in manifests {
        let slug = js_opt(manifest.pointer("/product/slug").and_then(Value::as_str));
        match by_product.iter_mut().find(|(existing, _)| *existing == slug) {
            Some((_, list)) => list.push(manifest.clone()),
            None => by_product.push((slug, vec![manifest.clone()])),
        }
    }
    by_product
}

fn ptr(value: &Value, pointer: &str) -> String {
    js_opt(value.pointer(pointer).and_then(Value::as_str))
}

fn group_lines(style: &Style, group: &HelpGroup) -> Vec<String> {
    let mut lines = vec![String::new(), style.bold(group.title)];
    lines.extend(
        group
            .rows
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 28)))),
    );
    lines
}

pub fn main_help(style: &Style, config: &Config, modules: Option<&[Value]>) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{} {} {} {}",
            style.bold("Nexus"),
            style.dim(&format!("CLI {VERSION}")),
            style.bullet,
            if config.api_key.is_some() {
                format!("signed in to {}", config.url)
            } else {
                "not signed in".into()
            }
        ),
        String::new(),
        format!("{}  nexus <command> [options]", style.bold("Usage")),
    ];
    let groups = help_groups();
    if let Some(first) = groups.first() {
        lines.extend(group_lines(style, first));
    }
    lines.push(String::new());
    lines.push(style.bold("Products"));
    match modules {
        None => lines.push(format!(
            "  {}",
            style.dim(if config.api_key.is_some() {
                "Gateway did not answer; products appear here when it does."
            } else {
                "Log in to list the products on your Gateway."
            })
        )),
        Some([]) => lines.push(format!(
            "  {}",
            style.dim("No modules are registered with this Gateway.")
        )),
        Some(modules) => {
            for (slug, capabilities) in products(modules) {
                let name = capabilities
                    .first()
                    .and_then(|m| m.pointer("/product/name"))
                    .and_then(Value::as_str)
                    .unwrap_or(&slug)
                    .to_string();
                lines.push(format!("  {}{name}", style.cyan(&pad_end(&slug, 28))));
                for manifest in &capabilities {
                    let inputs = a(manifest, "inputs")
                        .iter()
                        .map(|input| format!("--{}", js_string(input.get("name"))))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let verbs = strings(a(manifest, "verbs")).join(", ");
                    let tail = if inputs.is_empty() {
                        String::new()
                    } else {
                        format!(" {} {inputs}", style.bullet)
                    };
                    lines.push(format!(
                        "    {}{}",
                        pad_end(&ptr(manifest, "/capability/slug"), 26),
                        style.dim(&format!("{}: {verbs}{tail}", ptr(manifest, "/capability/name")))
                    ));
                }
            }
        }
    }
    for group in groups.iter().skip(1) {
        lines.extend(group_lines(style, group));
    }
    lines.push(String::new());
    lines.push(format!(
        "{}  --json  --no-wait  --team <slug>  --no-color  -h, --help  -v, --version",
        style.bold("Options")
    ));
    lines.push(style.dim("Run `nexus <product> --help` for a product's capabilities, verbs and inputs."));
    lines
}

fn product_help(style: &Style, slug: &str, capabilities: &[Value]) -> Vec<String> {
    let first = capabilities.first();
    let product_name = first
        .and_then(|m| m.pointer("/product/name"))
        .and_then(Value::as_str)
        .unwrap_or(slug);
    let mut lines = vec![
        format!("{} {}", style.bold(product_name), style.dim(&format!("· nexus {slug}"))),
        String::new(),
        format!(
            "{}  nexus {slug} <verb> [capability] [--input value] [--json] [--no-wait]",
            style.bold("Usage")
        ),
        style.dim(&format!(
            "         Without a capability, the verb runs on the first one that has it ({}).",
            first
                .map(|m| ptr(m, "/capability/slug"))
                .unwrap_or_else(|| "undefined".into())
        )),
    ];
    for manifest in capabilities {
        lines.push(String::new());
        lines.push(format!(
            "{} {}",
            style.bold(&ptr(manifest, "/capability/name")),
            style.dim(&format!("({})", ptr(manifest, "/capability/slug")))
        ));
        lines.push(format!("  {}", ptr(manifest, "/capability/description")));
        lines.push(String::new());
        for verb in strings(a(manifest, "verbs")) {
            let summary = manifest
                .get("operations")
                .and_then(|ops| ops.get(&verb))
                .and_then(|op| op.get("summary"))
                .and_then(Value::as_str)
                .unwrap_or("");
            lines.push(format!("  {}{}", style.cyan(&pad_end(&verb, 9)), style.dim(summary)));
        }
        let inputs = a(manifest, "inputs");
        if !inputs.is_empty() {
            lines.push(String::new());
            lines.push(format!("  {}", style.dim("Inputs")));
            for input in inputs {
                let placeholder = s(input, "placeholder")
                    .filter(|p| !p.is_empty())
                    .map(|p| style.dim(&format!("  e.g. {p}")))
                    .unwrap_or_default();
                lines.push(format!(
                    "  {}{}{}{placeholder}",
                    pad_end(&format!("--{}", js_string(input.get("name"))), 14),
                    js_string(input.get("label")),
                    if t(input, "required") {
                        String::new()
                    } else {
                        style.dim(" (optional)")
                    }
                ));
            }
        }
        lines.push(String::new());
        lines.push(format!("  {}  {}", style.dim("Example"), example_run(manifest)));
    }
    lines
}

fn example_run(manifest: &Value) -> String {
    let inputs = a(manifest, "inputs")
        .iter()
        .filter(|input| t(input, "required"))
        .map(|input| {
            let name = js_string(input.get("name"));
            let placeholder = s(input, "placeholder")
                .map(String::from)
                .unwrap_or_else(|| format!("<{name}>"));
            format!("--{name} {placeholder}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "nexus {} run {}{}",
        ptr(manifest, "/product/slug"),
        ptr(manifest, "/capability/slug"),
        if inputs.is_empty() {
            String::new()
        } else {
            format!(" {inputs}")
        }
    )
}

// ---------------------------------------------------------------- home

/// `nexus` alone: a header (version, organization, you) and the commands to start with.
fn home_lines(style: &Style, config: &Config, me: Option<&Value>) -> Vec<String> {
    let (org, who) = identity(config, me);
    let header: Vec<Option<String>> = if config.api_key.is_some() {
        let mut header = vec![Some(VERSION.to_string()), org.clone(), who.clone()];
        if org.as_deref().unwrap_or("").is_empty() && who.as_deref().unwrap_or("").is_empty() {
            header.push(Some(config.url.clone()));
        }
        header
    } else {
        vec![
            Some(VERSION.to_string()),
            Some("not signed in".into()),
            Some(config.url.clone()),
        ]
    };
    let mut start: Vec<(&str, &str)> = Vec::new();
    if config.api_key.is_none() {
        start.push(("auth login", "Sign in through Nexus UI (or --with-token for CI)"));
    }
    start.extend([
        (
            "chat \"prompt\"",
            "Ask any model you may use; continues your last session",
        ),
        ("models", "The models you may use, with their prices"),
        ("models pick", "Choose the model nexus chat uses"),
        ("spend", "Your budget, what is spent and the latest calls"),
        ("search \"<query>\"", "Your chats, people, teams and models"),
        ("claude", "Claude Code on your saved Gateway or Claude-plan route"),
        ("codex", "Codex on your saved Gateway or ChatGPT-plan route"),
        ("agy", "Antigravity on your Google AI plan"),
    ]);
    let mut lines = vec![format!("{} {}", brand(style), meta(style, &header)), String::new()];
    lines.extend(list(
        style,
        &[col(""), flex("")],
        rows_of(
            start
                .iter()
                .map(|(command, text)| vec![style.accent(&format!("nexus {command}")), style.dim(text)])
                .collect(),
        ),
        None,
    ));
    lines.push(String::new());
    lines.push(format!(
        "  {} {}  {} {}",
        style.dim("Every command:"),
        style.accent("nexus help"),
        style.dim("A product's:"),
        style.accent("nexus <product> --help")
    ));
    lines
}

/// The organization (its slug, else its name, else the one saved at sign-in) and the person (email, else id).
fn identity(config: &Config, me: Option<&Value>) -> (Option<String>, Option<String>) {
    let tenant = me.and_then(|me| me.get("tenant")).filter(|tenant| tenant.is_object());
    let user = me.and_then(|me| me.get("user")).filter(|user| user.is_object());
    let org = match tenant {
        Some(tenant) => s(tenant, "slug")
            .map(String::from)
            .or_else(|| s(tenant, "name").map(String::from)),
        None => config.extra.get("org").and_then(Value::as_str).map(String::from),
    };
    let who = user.map(|user| {
        s(user, "email")
            .map(String::from)
            .unwrap_or_else(|| js_string(user.get("id")))
    });
    (org, who)
}

// ---------------------------------------------------------------- spend

/// `$4 of $5 left` for owners and admins, `$4 left for you` for a member (the Rust gateway sends them no budget).
pub(crate) fn left_text(spend: &Value) -> String {
    match n_opt(spend, "budgetUsd") {
        Some(budget) => format!("{} of {} left", usd(n(spend, "remainingUsd")), usd(budget)),
        None => format!("{} left for you", usd(n(spend, "remainingUsd"))),
    }
}

/// The organization's spend in the budget's current period. The TypeScript gateway sends no `periodSpentUsd`, and
/// its `spentUsd` is that figure.
fn period_spent(spend: &Value) -> f64 {
    n_opt(spend, "periodSpentUsd").unwrap_or_else(|| n(spend, "spentUsd"))
}

fn budget_used(spend: &Value) -> f64 {
    match n(spend, "budgetUsd") {
        budget if budget != 0.0 => period_spent(spend) / budget,
        _ => 0.0,
    }
}

fn spent_label(spend: &Value) -> &'static str {
    if n_opt(spend, "periodSpentUsd").is_none() {
        return "Spent";
    }
    match s(spend, "budgetPeriod") {
        Some("day") => "Spent today",
        Some("week") => "Spent this week",
        _ => "Spent this month",
    }
}

/// The billing glossary's names for the free start and what it paid (kit's `BILLING_TERMS`). A test checks
/// them against `kit/src/fixtures/billing-words.json`, as the TypeScript CLI imports them.
const FREE_START: &str = "Free start";
const PAID_BY_FREE_START: &str = "Paid by the free start";

fn spend_lines(style: &Style, spend: &Value, manifests: &[Value], person: Option<&Value>) -> Vec<String> {
    let names = |product: &str, capability: Option<&str>| -> [String; 2] {
        if product == "core" {
            return ["Nexus Gateway".into(), "direct API calls".into()];
        }
        let found = manifests.iter().find(|m| {
            m.pointer("/product/slug").and_then(Value::as_str) == Some(product)
                && m.pointer("/capability/slug").and_then(Value::as_str) == capability
        });
        [
            found
                .and_then(|m| m.pointer("/product/name").and_then(Value::as_str))
                .unwrap_or(product)
                .to_string(),
            found
                .and_then(|m| m.pointer("/capability/name").and_then(Value::as_str))
                .or(capability)
                .unwrap_or("")
                .to_string(),
        ]
    };
    let budget = n(spend, "budgetUsd");
    let used = budget_used(spend);
    // Your own monthly limit, when you have one: how much of it this month's calls used.
    let limit = person.and_then(|person| n_opt(person, "limitUsd"));
    let mine = person.map(|person| n(person, "spentUsd")).unwrap_or(0.0);
    let your_limit = limit.map(|limit| {
        let share = if limit != 0.0 { mine / limit } else { 1.0 };
        kv(
            "Your limit",
            format!(
                "{} of {} this month  {}",
                usd(mine),
                usd(limit),
                meter(style, share, 10, Some(level_tone(share)))
            ),
        )
    });
    // A member of the Rust gateway gets what is left for them, never the organization's or a team's figures or subsidy.
    let figures = if n_opt(spend, "budgetUsd").is_none() {
        s(spend, "resetsAt")
            .map(|resets| {
                kv(
                    "Resets",
                    format!("{} (UTC)", resets.chars().take(10).collect::<String>()),
                )
            })
            .into_iter()
            .chain(
                n_opt(spend, "subsidizedUsd")
                    .filter(|paid| *paid != 0.0)
                    .map(|paid| kv(PAID_BY_FREE_START, usd(paid))),
            )
            .chain(your_limit)
            .collect::<Vec<_>>()
    } else {
        vec![
            kv("Budget", usd(budget)),
            kv(
                spent_label(spend),
                format!(
                    "{}  {}  {}",
                    usd(period_spent(spend)),
                    budget_bar(style, used, 24),
                    style.dim(&format!("{}%", to_fixed(used * 100.0, 2)))
                ),
            ),
            kv("Reserved", usd(n(spend, "reservedUsd"))),
            kv("Remaining", style.bold(&usd(n(spend, "remainingUsd")))),
        ]
        .into_iter()
        .chain(your_limit)
        .collect()
    };
    let mut lines = vec![
        format!("{} {} {}", style.bold("Spend"), style.bullet, left_text(spend)),
        String::new(),
    ];
    if !figures.is_empty() {
        lines.extend(pairs(style, &figures));
        lines.push(String::new());
    }
    let by_product = a(spend, "byProduct");
    if by_product.is_empty() {
        lines.push(style.dim("Nothing spent yet. Run a product; every model call is metered here."));
        if spend.get("subsidy").is_some_and(Value::is_object) {
            lines.push(String::new());
            lines.extend(subsidy_lines(style, spend));
        }
        if let Some(paid) = spend.get("subsidiesPaid").filter(|paid| paid.is_object()) {
            lines.push(String::new());
            lines.extend(paid_line(style, paid));
        }
        return lines;
    }
    let mut rows: Vec<&Value> = by_product.iter().collect();
    rows.sort_by(|x, y| {
        n(y, "costUsd")
            .partial_cmp(&n(x, "costUsd"))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let [product, capability] = names(s(row, "product").unwrap_or(""), s(row, "capability"));
            vec![product, capability, js_string(row.get("calls")), usd(n(row, "costUsd"))]
        })
        .collect();
    let calls: f64 = rows.iter().map(|row| n(row, "calls")).sum();
    let cost: f64 = rows.iter().fold(0.0, |sum, row| sum + n(row, "costUsd"));
    lines.extend(list(
        style,
        &[col("Product"), col("Capability"), right("Calls"), right("Cost")],
        rows_of(table_rows),
        Some(vec![
            "Total".into(),
            String::new(),
            crate::style::js_number(calls),
            usd(cost),
        ]),
    ));
    // The last few calls, newest first: the mark of the provider that served a settled call, ✕ for a failed one, ! for a refused one.
    let mut recent: Vec<&Value> = a(spend, "recent").iter().collect();
    recent.sort_by(|x, y| s(y, "at").unwrap_or("").cmp(s(x, "at").unwrap_or("")));
    recent.truncate(5);
    if !recent.is_empty() {
        lines.push(String::new());
        let recent_rows: Vec<Row> = recent
            .iter()
            .map(|entry| {
                let model = js_string(entry.get("model"));
                let [product, capability] = names(s(entry, "product").unwrap_or(""), s(entry, "capability"));
                let what = if !capability.is_empty() { capability } else { product };
                let mark = match s(entry, "status") {
                    Some("settled") => provider_mark(style, s(entry, "provider").unwrap_or("")),
                    Some("failed") => style.fail.clone(),
                    _ => style.warn.clone(),
                };
                Row {
                    cells: vec![
                        model,
                        what,
                        crate::style::js_number(n(entry, "promptTokens") + n(entry, "completionTokens")),
                        usd(n(entry, "costUsd")),
                        ago(s(entry, "at").unwrap_or("")),
                    ],
                    mark: Some(mark),
                    ..Row::default()
                }
            })
            .collect();
        lines.extend(list(
            style,
            &[
                col("Recent calls"),
                flex("For"),
                right("Tokens"),
                right("Cost"),
                col("When"),
            ],
            recent_rows,
            None,
        ));
    }
    if spend.get("subsidy").is_some_and(Value::is_object) {
        lines.push(String::new());
        lines.extend(subsidy_lines(style, spend));
    }
    if let Some(paid) = spend.get("subsidiesPaid").filter(|paid| paid.is_object()) {
        lines.push(String::new());
        lines.extend(paid_line(style, paid));
    }
    if a(spend, "recent")
        .iter()
        .any(|entry| s(entry, "provider") == Some("mock"))
    {
        lines.push(String::new());
        lines.push(style.dim("Mock provider: prices are illustrative, not measured."));
    }
    lines
}

fn plural(n: f64, noun: &str) -> String {
    format!(
        "{} {noun}{}",
        crate::style::js_number(n),
        if n == 1.0 { "" } else { "s" }
    )
}

/// The funding organization: what its stored keys paid for other organizations' subsidized calls.
fn paid_line(style: &Style, paid: &Value) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{} {} {}  {}",
            style.bold("Subsidies paid"),
            style.bullet,
            usd(n(paid, "usd")),
            meta(
                style,
                &[
                    Some(plural(n(paid, "calls"), "call")),
                    Some(plural(n(paid, "organizations"), "organization")),
                ]
            )
        ),
        String::new(),
    ];
    lines.extend(pairs(
        style,
        &[kv(
            "Paid with",
            "this organization's stored keys, outside its spend and budget",
        )],
    ));
    lines
}

/// The organization's Nexus subsidy (a personal organization's starting credit), and what it paid.
fn subsidy_lines(style: &Style, spend: &Value) -> Vec<String> {
    let subsidy = spend.get("subsidy").cloned().unwrap_or(Value::Null);
    let granted = n(&subsidy, "grantedUsd");
    let remaining = n(&subsidy, "remainingUsd");
    let used = if granted != 0.0 {
        (granted - remaining) / granted
    } else {
        0.0
    };
    let calls = a(spend, "recent")
        .iter()
        .filter(|entry| s(entry, "fundedBy") == Some("nexus_subsidy"))
        .count();
    let mut lines = vec![
        format!(
            "{} {} {} of {} left  {}",
            style.bold(FREE_START),
            style.bullet,
            usd(remaining),
            usd(granted),
            meta(style, &[Some("from Nexus, not your budget".into())])
        ),
        String::new(),
    ];
    lines.extend(pairs(
        style,
        &[
            kv(
                "Used",
                format!(
                    "{}  {}",
                    usd(granted - remaining),
                    meter(style, used, 10, Some(level_tone(used)))
                ),
            ),
            kv(
                PAID_BY_FREE_START,
                format!(
                    "{}  {}",
                    usd(n(spend, "subsidizedUsd")),
                    meta(style, &[Some(plural(calls as f64, "recent call"))])
                ),
            ),
            kv(
                "Model calls",
                if t(&subsidy, "usableForModelCalls") {
                    "paid from the credit when you have no key of your own for the provider".to_string()
                } else {
                    format!(
                        "{} {}  {}",
                        style.warn,
                        style.yellow("need your own provider key on this Gateway"),
                        meta(style, &[Some("nexus keys add <provider> <key>".into())])
                    )
                },
            ),
        ],
    ));
    lines
}

// ---------------------------------------------------------------- products

const VERBS: &[&str] = &["connect", "run", "report", "apply", "watch", "status"];

struct Resolved {
    manifest: Value,
    verb: String,
    args: Vec<String>,
    input: Vec<(String, String)>,
}

/// `nexus <product> <verb> [capability] [args] [--input value]` (`discovery.ts`).
fn resolve_command(
    product: &str,
    positional: &[String],
    args: &Args,
    manifests: &[Value],
) -> std::result::Result<Resolved, String> {
    let all = products(manifests);
    let Some((_, capabilities)) = all.iter().find(|(slug, _)| slug == product) else {
        let known = all.iter().map(|(slug, _)| slug.clone()).collect::<Vec<_>>().join(", ");
        return Err(format!(
            "unknown command or product \"{product}\" (products: {})",
            if known.is_empty() {
                "none registered".to_string()
            } else {
                known
            }
        ));
    };
    let verb = positional.first().map(String::as_str);
    let rest = positional.get(1..).unwrap_or(&[]);
    let mut verbs: Vec<String> = Vec::new();
    for manifest in capabilities {
        for v in strings(a(manifest, "verbs")) {
            if !verbs.contains(&v) {
                verbs.push(v);
            }
        }
    }
    let Some(verb) = verb.filter(|v| VERBS.contains(v)) else {
        return Err(format!(
            "usage: nexus {product} <{}> [capability] [--input value]",
            verbs.join("|")
        ));
    };
    let has_verb = |m: &Value| a(m, "verbs").iter().any(|v| v.as_str() == Some(verb));
    let explicit = rest.first().filter(|first| !first.is_empty()).and_then(|first| {
        capabilities
            .iter()
            .find(|m| m.pointer("/capability/slug").and_then(Value::as_str) == Some(first))
    });
    let manifest = explicit.or_else(|| capabilities.iter().find(|m| has_verb(m)));
    let Some(manifest) = manifest.filter(|m| has_verb(m)) else {
        let names = capabilities
            .iter()
            .map(|m| ptr(m, "/capability/slug"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "{product} has no capability that supports {verb} (capabilities: {names})"
        ));
    };
    let rest_args = if explicit.is_some() {
        rest.get(1..).unwrap_or(&[]).to_vec()
    } else {
        rest.to_vec()
    };
    let mut input = Vec::new();
    for declared in a(manifest, "inputs") {
        let name = js_string(declared.get("name"));
        if let Some(value) = args.flag(&name) {
            input.push((name, value.to_string()));
        } else if t(declared, "required") && (verb == "run" || verb == "connect") {
            return Err(format!("missing --{name} ({})", js_string(declared.get("label"))));
        }
    }
    Ok(Resolved {
        manifest: manifest.clone(),
        verb: verb.to_string(),
        args: rest_args,
        input,
    })
}

fn input_json(input: &[(String, String)]) -> Value {
    Value::Object(input.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

fn product_command(command: &str, rest: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let ctx = cmd.ctx;
    let response = cmd.core.get("/v1/modules")?;
    let modules = a(&response, "modules");
    let all = products(modules);
    let capabilities = all
        .iter()
        .find(|(slug, _)| slug == command)
        .map(|(_, list)| list.clone());
    if let Some(capabilities) = &capabilities
        && (cmd.has("help") || rest.is_empty())
    {
        if cmd.json {
            return Ok(cmd.print(&obj(vec![
                ("product", capabilities.first().and_then(|m| m.get("product")).cloned()),
                ("capabilities", Some(Value::Array(capabilities.clone()))),
            ])));
        }
        cmd.lines(product_help(style, command, capabilities));
        return Ok(0);
    }
    if cmd.has("help") {
        cmd.lines(main_help(style, cmd.config, Some(modules)));
        return Ok(0);
    }
    // S4.4: `run review --local` reviews the working tree before a pull request exists.
    if (cmd.has("local") || cmd.flag("local").is_some()) && rest.first().map(String::as_str) == Some("run") {
        return crate::local_review::command(command, rest, cmd);
    }
    // A product missing while a module is down is that module, not a typo.
    if capabilities.is_none() {
        let known: Vec<String> = all.iter().map(|(slug, _)| slug.clone()).collect();
        let unavailable = a(&response, "unavailable");
        let local = crate::error::is_local(&cmd.config.url);
        if let Some(error) = crate::unanswered::unanswered_error(command, &known, unavailable, local) {
            return Err(error.into());
        }
    }
    let resolved = match resolve_command(command, rest, cmd.args, modules) {
        Ok(resolved) => resolved,
        Err(message) => {
            let hint = if capabilities.is_some() {
                format!("Run `nexus {command} --help` for its capabilities, verbs and inputs.")
            } else {
                "Run `nexus help` for the commands and the products on this Gateway.".to_string()
            };
            return usage(capitalize(&message), Some(&hint));
        }
    };
    let Resolved {
        manifest,
        verb,
        args,
        input,
    } = resolved;
    let product = ptr(&manifest, "/product/slug");
    let capability = ptr(&manifest, "/capability/slug");
    let capability_name = ptr(&manifest, "/capability/name");

    if verb == "report" {
        let Some(id) = args.first() else {
            let reports = cmd.core.get(&format!("/v1/{product}/{capability}/report"))?;
            if cmd.json {
                return Ok(cmd.print(reports.get("runs").unwrap_or(&Value::Null)));
            }
            let runs = a(&reports, "runs");
            if runs.is_empty() {
                cmd.out(format!(
                    "No {capability_name} runs yet. Start one: {}",
                    style.cyan(&example_run(&manifest))
                ));
                return Ok(0);
            }
            let rows: Vec<Vec<String>> = runs
                .iter()
                .map(|r| {
                    vec![
                        js_string(r.get("id")),
                        status(style, s(r, "status").unwrap_or("")),
                        ago(s(r, "startedAt").unwrap_or("")),
                        usd(n(r, "costUsd")),
                        r.get("input")
                            .and_then(Value::as_object)
                            .map(|input| input.values().map(|v| js_string(Some(v))).collect::<Vec<_>>().join(" "))
                            .unwrap_or_default(),
                    ]
                })
                .collect();
            cmd.lines(list(
                style,
                &[col("Run"), col("Status"), col("Started"), right("Cost"), flex("Input")],
                rows_of(rows),
                None,
            ));
            return Ok(0);
        };
        let report = cmd
            .core
            .get(&format!("/v1/{product}/{capability}/report/{}", enc(id)))?;
        if cmd.json {
            return Ok(cmd.print(&report));
        }
        print_pages(&manifest, &report, cmd, cmd.flag("page"))?;
        return Ok(0);
    }

    if verb == "apply" {
        return apply_command(&product, &capability, &input, cmd);
    }

    if verb == "status" {
        let query = crate::module_status::status_query(&manifest, &input);
        let answer = match cmd.core.get(&format!("/v1/{product}/{capability}/status{query}")) {
            Ok(answer) => answer,
            Err(Error::Core(error)) if error.status == 404 => {
                return Err(CliError {
                    message: crate::module_status::legacy_status_message(&capability_name),
                    hint: Some(format!("Gateway answered: {}", error.message)),
                    exit_code: 1,
                    code: None,
                }
                .into());
            }
            Err(error) => return Err(error),
        };
        if cmd.json {
            return Ok(cmd.print(&answer));
        }
        cmd.lines(crate::module_status::status_lines(style, &answer));
        return Ok(0);
    }

    // The run's spend goes to --team (or NEXUS_TEAM), else your default team.
    let team = cmd.flag("team").map(String::from).or_else(|| ctx.var("NEXUS_TEAM"));
    let mut body = input.clone();
    if let Some(team) = team.filter(|team| !team.is_empty()) {
        body.retain(|(key, _)| key != "team");
        body.push(("team".into(), team));
    }
    let result = cmd
        .core
        .post(&format!("/v1/{product}/{capability}/{verb}"), input_json(&body))?;
    let run_id = s(&result, "id").map(String::from);
    let Some(run_id) = run_id.filter(|_| verb == "run") else {
        if cmd.json {
            return Ok(cmd.print(&result));
        }
        cmd.out(format!("{} {capability_name} {verb}: {}", style.ok, style.bold("done")));
        if let Some(map) = result.as_object() {
            for (key, value) in map {
                let text = if value.is_object() || value.is_array() || value.is_null() {
                    compact(value)
                } else {
                    js_string(Some(value))
                };
                cmd.out(format!("  {}  {text}", style.dim(key)));
            }
        }
        return Ok(0);
    };
    let target = input
        .iter()
        .map(|(_, value)| value.clone())
        .collect::<Vec<_>>()
        .join(" ");
    let product_name = ptr(&manifest, "/product/name");
    if cmd.has("no-wait") {
        if cmd.json {
            return Ok(cmd.print(&result));
        }
        cmd.out(format!(
            "{} {product_name} › {capability_name}: run {run_id} started",
            style.ok
        ));
        cmd.out(format!(
            "  {} Follow it: {}",
            style.arrow,
            style.cyan(&format!("nexus {product} report {capability} {run_id}"))
        ));
        return Ok(0);
    }
    if !cmd.json {
        cmd.out(format!(
            "{} {product_name} › {capability_name}: run {run_id} started",
            style.info
        ));
    }

    let started = Instant::now();
    let polled = (|| -> Result<Value> {
        let mut attempt: u32 = 0;
        loop {
            ctx.io
                .progress(Some(&format!("Running {capability_name} on {target}…")));
            let report = cmd
                .core
                .get(&format!("/v1/{product}/{capability}/report/{}", enc(&run_id)))?;
            if s(&report, "status") != Some("running") {
                return Ok(report);
            }
            // Poll quickly at first (short runs finish in well under a second), then every 1.5 s.
            let wait = ctx
                .poll_ms
                .unwrap_or_else(|| 1500.min(150u64.saturating_mul(1u64 << attempt.min(20))));
            std::thread::sleep(Duration::from_millis(wait));
            attempt += 1;
        }
    })();
    ctx.io.progress(None);
    let report = polled?;
    let succeeded = s(&report, "status") == Some("succeeded");
    if cmd.json {
        cmd.print(&report);
        return Ok(if succeeded { 0 } else { 1 });
    }
    let took = to_fixed(started.elapsed().as_secs_f64(), 1);
    if succeeded {
        cmd.out(format!(
            "{} {capability_name} finished in {took} s {} {target}",
            style.ok, style.bullet
        ));
    } else {
        cmd.out(format!(
            "{} {capability_name} {} after {took} s {} {target}",
            style.fail,
            js_string(report.get("status")),
            style.bullet
        ));
    }
    cmd.out("");
    print_pages(&manifest, &report, cmd, None)?;
    if !succeeded {
        let error = s(&report, "error");
        let failure = CliError {
            message: error.unwrap_or("The run failed.").to_string(),
            hint: run_failure_hint(error),
            exit_code: 1,
            code: None,
        };
        ctx.io.err(&error_text(style, &failure));
        return Ok(1);
    }
    Ok(0)
}

/// `nexus <product> apply <capability> …` (Nexus Review's fix hand-off, S4.3). The prompt is printed alone on stdout,
/// so it pipes into any agent; `--agent <agent>` starts that agent on it in this checkout instead. A bundle with
/// no prompt (a pull request from a fork) is printed for a person to read, and `--agent` refuses it. `--agent` wins
/// over `--json`: with both, the agent starts and no JSON is printed.
fn apply_command(product: &str, capability: &str, input: &[(String, String)], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let agent = match cmd.flag("agent") {
        None => None,
        // The prompt goes to the agent as its first argument, so only an agent whose launcher takes one starts
        // (`LocalAgentT::takes_prompt_argument`).
        Some(name) => match Agent::parse(name).filter(|&agent| capabilities(agent).prompt_argument) {
            Some(agent) => Some(agent),
            None => {
                return usage(
                    format!(
                        "--agent must be {}, not {name}",
                        agent_ids(|capabilities| capabilities.prompt_argument, " or ")
                    ),
                    Some("Leave it out to print the prompt for any other agent."),
                );
            }
        },
    };
    let result = cmd
        .core
        .post(&format!("/v1/{product}/{capability}/apply"), input_json(input))?;
    let prompt = s(&result, "prompt").filter(|prompt| !prompt.is_empty());
    if cmd.json && agent.is_none() {
        return Ok(cmd.print(&result));
    }
    let Some(prompt) = prompt else {
        let reason = s(&result, "withheld_reason").unwrap_or("Review gave no prompt for this finding.");
        if agent.is_some() {
            return Err(Error::Cli(CliError {
                message: format!("No agent started. {reason}"),
                hint: Some("Run it without --agent to read the finding.".into()),
                exit_code: 1,
                code: None,
            }));
        }
        cmd.out(format!("{} {}", style.warn, reason));
        cmd.out("");
        for key in [
            "finding",
            "pr_url",
            "head_sha",
            "path",
            "line_start",
            "line_end",
            "rule_id",
            "title",
        ] {
            if let Some(value) = result.get(key).filter(|value| !value.is_null()) {
                cmd.out(format!("  {}  {}", style.dim(key), js_string(Some(value))));
            }
        }
        if let Some(detail) = s(&result, "detail").filter(|detail| !detail.is_empty()) {
            cmd.out("");
            cmd.out(detail);
        }
        return Ok(0);
    };
    if t(&result, "moved") {
        cmd.ctx.io.err(&format!(
            "{} The pull request moved past {} since this finding: the agent stops if the lines changed.",
            style.warn,
            js_string(result.get("head_sha"))
        ));
    }
    match agent {
        None => {
            cmd.out(prompt);
            Ok(0)
        }
        Some(agent) => {
            let launch = vec!["--".to_string(), prompt.to_string()];
            agents::launch_agent(agent, &launch, cmd.ctx, cmd.config, style)
        }
    }
}

fn run_footer(manifest: &Value, report: &Value, cmd: &Cmd, extra: Vec<(String, String)>) -> Vec<String> {
    let style = cmd.style;
    let link = artifacts::ui_link(
        cmd.config,
        &format!(
            "/products/{}?capability={}&run={}",
            ptr(manifest, "/product/slug"),
            ptr(manifest, "/capability/slug"),
            js_string(report.get("id"))
        ),
    );
    let mut entries = vec![
        kv(
            "Run",
            format!(
                "{} {}",
                js_string(report.get("id")),
                style.dim(&format!(
                    "({})",
                    seconds(s(report, "startedAt").unwrap_or(""), s(report, "finishedAt"))
                ))
            ),
        ),
        kv(
            "Spend",
            format!(
                "{} {}",
                usd(n(report, "costUsd")),
                style.dim("model spend for this run")
            ),
        ),
    ];
    if let Some(coverage) = report.get("coverage").and_then(Value::as_f64) {
        entries.push(kv(
            "Coverage",
            format!(
                "{} {}",
                percent(coverage),
                style.dim("of the files in scope, cited by the pages")
            ),
        ));
    }
    entries.extend(extra);
    entries.push(kv("Open", style.underline(&link)));
    let mut lines = vec![String::new()];
    lines.extend(pairs(style, &entries));
    lines
}

/// A share from 0 to 1 as a percentage: one decimal under 10%, as Nexus UI shows it.
fn percent(share: f64) -> String {
    let value = share * 100.0;
    if value < 10.0 {
        format!("{value:.1}%")
    } else {
        format!("{}%", value.round())
    }
}

fn print_pages(manifest: &Value, report: &Value, cmd: &Cmd, page: Option<&str>) -> Result<()> {
    let style = cmd.style;
    let pages = a(report, "pages");
    if let Some(page) = page.filter(|page| !page.is_empty()) {
        let Some(found) = pages.iter().find(|candidate| s(candidate, "slug") == Some(page)) else {
            let slugs = pages
                .iter()
                .map(|p| js_string(p.get("slug")))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CliError::hint(
                format!("No page \"{page}\" in run {}.", js_string(report.get("id"))),
                format!("Pages: {}.", if slugs.is_empty() { "none".to_string() } else { slugs }),
            )
            .into());
        };
        cmd.out(js_string(found.get("markdown")));
        return Ok(());
    }
    if !pages.is_empty() {
        cmd.out(format!("  {}", style.bold(&format!("{} pages generated", pages.len()))));
        cmd.out("");
        let width = pages
            .iter()
            .map(|p| crate::style::js_len(&js_string(p.get("slug"))))
            .max()
            .unwrap_or(0);
        for p in pages {
            cmd.out(format!(
                "  {}  {}",
                style.cyan(&pad_end(&js_string(p.get("slug")), width)),
                js_string(p.get("title"))
            ));
        }
    }
    let extra = match pages.first() {
        Some(first) => vec![kv(
            "Read",
            format!(
                "nexus {} report {} {} --page {}",
                ptr(manifest, "/product/slug"),
                ptr(manifest, "/capability/slug"),
                js_string(report.get("id")),
                js_string(first.get("slug"))
            ),
        )],
        None => vec![],
    };
    cmd.lines(run_footer(manifest, report, cmd, extra));
    Ok(())
}

fn status(style: &Style, value: &str) -> String {
    match value {
        "succeeded" => format!("{} succeeded", style.ok),
        "failed" => format!("{} {}", style.fail, style.red("failed")),
        "running" => format!("{} running", style.yellow("…")),
        other => other.to_string(),
    }
}

/// Whether a budget refusal's message left anything: the figure in its "and $… is left". A run keeps
/// only the 402's message, not its `remainingUsdDecimal`. None when it does not say. `cli.ts` reads it the same way.
fn anything_left(message: &str) -> Option<bool> {
    let (_, after) = message.split_once(" and $")?;
    let figure: String = after.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let clause = after.split(';').next().unwrap_or_default();
    let exact =
        !figure.is_empty() && !figure.starts_with('.') && !figure.ends_with('.') && figure.matches('.').count() <= 1;
    if !exact || !clause.contains(" is left") {
        return None;
    }
    Some(figure.chars().any(|c| matches!(c, '1'..='9')))
}

/// A run's hint for its stored error. A budget refusal's first sentence follows what was left, as Nexus UI's title
/// does: the call may cost more than what is left, nothing is left, or (unsaid) the budget does not
/// cover the call.
fn run_failure_hint(error: Option<&str>) -> Option<String> {
    let text = error.unwrap_or_default();
    let lower = text.to_lowercase();
    if lower.contains("unpaid nexus invoice") {
        return Some(billing::PAST_DUE_HINT.into());
    }
    let titled = |some: &str, none: &str, unsaid: &str| match anything_left(text) {
        Some(true) => some.to_string(),
        Some(false) => none.to_string(),
        None => unsaid.to_string(),
    };
    let product = lower.trim_start();
    if product.starts_with("product budget:") || product.starts_with("product limit:") {
        let title = titled(
            "This call may cost more than this product's limit has left.",
            "Nothing is left of this product's limit.",
            "This product's limit doesn't cover this call.",
        );
        return Some(format!(
            "{title} See `nexus spend`; an owner or admin can raise the product's limit through the API."
        ));
    }
    if lower.contains("person limit") {
        let title = titled(
            "This call may cost more than your monthly limit has left.",
            "Nothing is left of your monthly limit.",
            "Your monthly spend limit doesn't cover this call.",
        );
        return Some(format!(
            "{title} See `nexus budgets show me`; an owner, or an admin an owner allows, can raise it."
        ));
    }
    if lower.contains("budget") {
        let title = titled(
            "This call may cost more than the organization's budget has left.",
            "Nothing is left of the organization's budget.",
            "The organization's budget doesn't cover this call.",
        );
        return Some(format!(
            "{title} See `nexus spend`; its owner or an admin can raise it."
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run's budget hint follows what its 402's message left, and never calls money left "used up".
    #[test]
    fn a_run_budget_hint_says_what_was_left() {
        let hint = |error: &str| run_failure_hint(Some(error)).expect("a budget hint");
        let org = |left: &str| {
            hint(&format!(
                "this call may cost up to $0.042110, and {left} of this month's budget is left; it resets on 2026-12-01 (UTC)"
            ))
        };
        let spend = " See `nexus spend`; its owner or an admin can raise it.";
        assert_eq!(
            org("$0.010000"),
            format!("This call may cost more than the organization's budget has left.{spend}")
        );
        assert_eq!(
            org("$0.00"),
            format!("Nothing is left of the organization's budget.{spend}")
        );
        assert_eq!(
            hint("the budget refused it"),
            format!("The organization's budget doesn't cover this call.{spend}")
        );
        let person = |left: &str| {
            hint(&format!(
                "person limit: this call may cost up to $0.004096, and {left} of your $1.00 monthly limit is left (set for you); it resets on 2026-12-01 (UTC)"
            ))
        };
        let me = " See `nexus budgets show me`; an owner, or an admin an owner allows, can raise it.";
        assert_eq!(
            person("$0.50"),
            format!("This call may cost more than your monthly limit has left.{me}")
        );
        assert_eq!(person("$0.00"), format!("Nothing is left of your monthly limit.{me}"));
        assert_eq!(
            hint("person limit: refused"),
            format!("Your monthly spend limit doesn't cover this call.{me}")
        );
        for odd in ["$.5", "$1.", "$1.2.3", "$x"] {
            assert_eq!(
                person(odd),
                format!("Your monthly spend limit doesn't cover this call.{me}"),
                "{odd}"
            );
        }
        for text in [org("$0.01"), org("$0"), person("$1.00"), person("$0.00")] {
            assert!(!text.contains("used up") && !text.contains("reached"), "{text}");
        }
        assert_eq!(run_failure_hint(Some("the model timed out")), None);
    }

    /// A product budget's refusal reads as the product's, not the organization's.
    #[test]
    fn a_run_product_budget_hint_names_the_product() {
        let product = |left: &str| {
            run_failure_hint(Some(&format!(
                "product budget: this call may cost up to $1.000000, and {left} is left of the monthly budget of review run; it resets on 2026-12-01 (UTC)"
            )))
            .expect("a budget hint")
        };
        let api = " See `nexus spend`; an owner or admin can raise the product's limit through the API.";
        assert_eq!(
            product("$0.25"),
            format!("This call may cost more than this product's limit has left.{api}")
        );
        assert_eq!(
            product("$0.00"),
            format!("Nothing is left of this product's limit.{api}")
        );
        assert_eq!(
            run_failure_hint(Some("product limit: refused")).as_deref(),
            Some(format!("This product's limit doesn't cover this call.{api}").as_str())
        );
    }

    #[test]
    fn json_error_carries_the_clis_own_code() {
        let claude = Agent::Claude.entry();
        let unbound = nexus_client::agent_entries::AgentEntry {
            billing: nexus_client::agent_entries::Billing {
                gateway: None,
                ..claude.billing
            },
            ..*claude
        };
        let refused =
            crate::agents::require_route(&unbound, nexus_client::agents::Route::Gateway).expect_err("refused");
        let failure = to_cli_error(&refused, DEFAULT_URL);
        // What the TypeScript CLI prints for the same refusal (cli.test.ts, "route_unsupported").
        assert_eq!(
            compact(&json_error(&refused, &failure)),
            r#"{"error":{"message":"Claude Code runs only on your Claude plan: run `nexus claude --subscription`","code":"route_unsupported"}}"#
        );
        let plain = Error::Cli(CliError::hint("No.", "Fix it."));
        assert_eq!(
            compact(&json_error(&plain, &to_cli_error(&plain, DEFAULT_URL))),
            r#"{"error":{"message":"No.","hint":"Fix it."}}"#
        );
    }

    /// The spend page's billing words are the glossary's.
    #[test]
    fn spend_billing_words_match_the_kit_glossary() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../packages/kit/src/fixtures/billing-words.json"))
                .expect("fixture");
        assert_eq!(fixture["terms"]["freeStart"].as_str(), Some(FREE_START));
        assert_eq!(fixture["terms"]["paidByFreeStart"].as_str(), Some(PAID_BY_FREE_START));
    }

    #[test]
    fn masks_keys() {
        assert_eq!(mask_key("nx_0123456789abcdef"), "nx_012••••cdef");
        assert_eq!(mask_key("short"), "••••");
    }

    #[test]
    fn an_organization_link_uses_its_slug_else_its_provider_org_id_else_its_id() {
        let segment = |tenant: Value| org_segment(&tenant);
        assert_eq!(
            segment(json!({ "id": "ten_1", "slug": "acme", "clerkOrgId": "org_2abc" })).as_deref(),
            Some("acme")
        );
        assert_eq!(
            segment(json!({ "id": "ten_1", "slug": "", "clerkOrgId": "org_2abc" })).as_deref(),
            Some("org_2abc")
        );
        assert_eq!(segment(json!({ "id": "ten_1" })).as_deref(), Some("ten_1"));
        assert_eq!(segment(json!({})), None);
    }

    #[test]
    fn a_member_sees_only_what_is_left_for_them_and_an_owner_the_budget() {
        let style = Style::new(false);
        let by_product = json!([{ "product": "core", "costUsd": 0.25, "calls": 2 }]);
        let member = json!({
            "remainingUsd": 1.25, "period": "2026-10-17", "resetsAt": "2026-10-18T00:00:00.000Z",
            "scope": "own", "byProduct": by_product, "recent": [],
        });
        let shown = spend_lines(&style, &member, &[], None).join("\n");
        assert!(shown.contains("Spend · $1.25 left for you"), "{shown}");
        assert!(
            shown.contains("Resets") && shown.contains("2026-10-18 (UTC)"),
            "{shown}"
        );
        for hidden in ["Budget", "Spent", "Reserved", "Remaining", "$50"] {
            assert!(!shown.contains(hidden), "{hidden} in: {shown}");
        }
        assert!(shown.contains("Nexus Gateway"), "their own calls stay: {shown}");
        assert_eq!(left_text(&member), "$1.25 left for you");
        // What Nexus paid for their own calls shows; the organization's pool is not in their answer.
        let paid = json!({ "remainingUsd": 1.25, "scope": "own", "subsidizedUsd": 0.3, "byProduct": [], "recent": [] });
        let shown = spend_lines(&style, &paid, &[], None).join("\n");
        assert!(
            shown.contains("Paid by the free start") && shown.contains("$0.30"),
            "{shown}"
        );
        assert!(!shown.contains("Free start ·"), "{shown}");

        let owner = json!({
            "budgetUsd": 50.0, "spentUsd": 0.25, "reservedUsd": 0.0, "remainingUsd": 49.75,
            "scope": "org", "byProduct": by_product, "recent": [],
        });
        let shown = spend_lines(&style, &owner, &[], None).join("\n");
        assert!(shown.contains("Spend · $49.75 of $50.00 left"), "{shown}");
        for figure in [
            "Budget     $50.00",
            "Spent      $0.25",
            "Reserved   $0.00",
            "Remaining  $49.75",
        ] {
            assert!(shown.contains(figure), "{figure} not in: {shown}");
        }
        assert!(!shown.contains("left for you"), "{shown}");
        assert_eq!(left_text(&owner), "$49.75 of $50.00 left");
    }

    #[test]
    fn an_owner_sees_the_spend_of_the_budget_period_not_the_lifetime_spend() {
        let style = Style::new(false);
        let owner = json!({
            "budgetUsd": 50.0, "spentUsd": 65.0, "budgetPeriod": "month", "periodSpentUsd": 5.0,
            "reservedUsd": 0.0, "remainingUsd": 45.0, "scope": "org", "byProduct": [], "recent": [],
        });
        let shown = spend_lines(&style, &owner, &[], None).join("\n");
        assert!(shown.contains("Spent this month  $5.00"), "{shown}");
        assert!(shown.contains("10.00%"), "{shown}");
        assert!(shown.contains("Remaining         $45.00"), "{shown}");
        assert!(!shown.contains("$65") && !shown.contains("130"), "{shown}");
        assert_eq!(budget_used(&owner), 0.1);

        let week = json!({ "budgetUsd": 50.0, "spentUsd": 65.0, "budgetPeriod": "week", "periodSpentUsd": 12.0,
            "remainingUsd": 38.0, "byProduct": [], "recent": [] });
        let shown = spend_lines(&style, &week, &[], None).join("\n");
        assert!(shown.contains("Spent this week  $12.00"), "{shown}");
        let day = json!({ "budgetUsd": 50.0, "budgetPeriod": "day", "periodSpentUsd": 1.0,
            "remainingUsd": 49.0, "byProduct": [], "recent": [] });
        assert!(
            spend_lines(&style, &day, &[], None)
                .join("\n")
                .contains("Spent today  $1.00")
        );

        let typescript_gateway = json!({ "budgetUsd": 50.0, "spentUsd": 20.0, "remainingUsd": 30.0,
            "byProduct": [], "recent": [] });
        let shown = spend_lines(&style, &typescript_gateway, &[], None).join("\n");
        assert!(
            shown.contains("Spent      $20.00") && shown.contains("40.00%"),
            "{shown}"
        );
        assert_eq!(budget_used(&typescript_gateway), 0.4);
        assert_eq!(budget_used(&json!({ "budgetUsd": 0.0, "periodSpentUsd": 1.0 })), 0.0);
    }

    /// A settled call's mark is the provider that served it, as recorded, never the maker its model id names. A host
    /// the CLI has no mark for is neutral.
    #[test]
    fn a_spend_mark_is_the_recorded_provider() {
        let style = Style::new(true);
        let by_product = json!([{ "product": "core", "costUsd": 0.25, "calls": 1 }]);
        let spend = |model: &str, provider: &str| {
            let recent = json!([{
                "model": model, "provider": provider, "status": "settled", "product": "core",
                "promptTokens": 1, "completionTokens": 1, "costUsd": 0.25, "at": "2026-10-17T00:00:00.000Z",
            }]);
            let body = json!({ "remainingUsd": 1.0, "scope": "own", "byProduct": by_product, "recent": recent });
            spend_lines(&style, &body, &[], None).join("\n")
        };
        let anthropic = style.provider("anthropic", style.g.dot);
        let neutral = style.provider("baseten", style.g.dot);
        assert_ne!(anthropic, neutral);
        let hosted = spend("anthropic/claude-opus-5.5", "baseten");
        assert!(hosted.contains(&neutral) && !hosted.contains(&anthropic), "{hosted}");
        let direct = spend("deepseek/deepseek-v4.1-flash-1017", "anthropic");
        assert!(direct.contains(&anthropic), "{direct}");
    }
}
