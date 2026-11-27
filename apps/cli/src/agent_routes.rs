//! `nexus agents` (`agent-routes.ts`): each coding agent's route, who pays, and what is left to set up on this machine.
//! `nexus agents setup <agent>` prints the agent's recipe (`recipes.rs`): its limits first, then the steps, the check and
//! what Nexus changes for a run, in the words of the generated docs page. It reads only the recipes this CLI carries.
//! `nexus agents share` (`agent_share.rs`) needs a sign-in, so `cli.rs` runs it with the API commands.

use serde_json::{Value, json};

use nexus_client::agent_entries::Agent;
use nexus_client::agent_setup::{
    SetupOptions, agent_entry, offline_agent_access, payer_label, route_label, route_of, setup_ready, setup_steps,
};
use nexus_client::agents::Route;
use nexus_client::local_agents::{LocalAgent, Login, local_with};

use crate::Context;
use crate::catalog::{AGENT_SHARE_COMMANDS, agent_commands};
use crate::cli::cli_client;
use crate::config::{Config, DEFAULT_URL};
use crate::error::{CliError, Error, Result, Transport};
use crate::json::{pretty, s};
use crate::recipes;
use crate::style::{Style, capitalize, pad_end, quote};

/// `items` as a person writes a list: `a, b and c`, with `last` (`and`, `or`) before the last one.
pub(crate) fn prose_list(items: &[String], last: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., final_item] => format!("{} {last} {final_item}", rest.join(", ")),
    }
}

/// The agents this CLI launches, in the catalog's order.
pub(crate) fn launchers() -> impl Iterator<Item = Agent> {
    Agent::ALL
        .into_iter()
        .filter(|agent| agent.entry().session.has_cli_launcher())
}

/// Each launcher's command (`nexus claude, nexus codex and nexus agy`).
fn launcher_commands() -> String {
    let commands: Vec<String> = launchers().map(|agent| format!("nexus {}", agent.command())).collect();
    prose_list(&commands, "and")
}

pub fn agents_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{} {}",
            style.bold("nexus agents"),
            style.dim(&format!("· how {} reach their models", launcher_commands()))
        ),
        String::new(),
    ];
    lines.extend(
        agent_commands()
            .iter()
            .map(|(usage, description)| format!("  {}{description}", pad_end(&format!("nexus agents {usage}"), 58))),
    );
    let plans: Vec<String> = launchers()
        .map(|agent| agent.entry().identity.plan.to_string())
        .collect();
    lines.extend([String::new(), style.bold("Sharing a session")]);
    for (usage, description) in AGENT_SHARE_COMMANDS {
        lines.push(format!("  nexus agents {usage}"));
        lines.push(format!("      {description}"));
    }
    lines.extend([
        String::new(),
        format!(
            "  {}  gateway: through Nexus Gateway, metered and checked, paid with your organization's own key (BYOK), never Nexus credit",
            style.dim("Routes")
        ),
        format!(
            "          subscription: on your own {} plan, signed in on this machine; calls go straight to the vendor",
            prose_list(&plans, "or")
        ),
        format!(
            "  {} nexus claude --api, nexus claude --subscription (or NEXUS_CLAUDE_AUTH=gateway|subscription)",
            style.dim("One run")
        ),
        format!(
            "  {}   nexus agents --csv or --jsonl [--from D] [--to D] [--team T] [--person P] [--agent A] [--via V] [--status S] [--out F]: the organization's agent sessions (signed in)",
            style.dim("Files")
        ),
    ]);
    lines
}

/// `nexus agents help --json`: the command names, once each.
pub fn agent_command_names() -> Vec<String> {
    agent_commands()
        .iter()
        .map(|(usage, _)| usage.as_str())
        .chain(AGENT_SHARE_COMMANDS.iter().take(1).map(|(usage, _)| *usage))
        .map(|usage| usage.split(' ').next().unwrap_or("").to_string())
        .collect()
}

struct Agents<'a> {
    ctx: &'a Context,
    config: &'a Config,
    style: &'a Style,
    json: bool,
}

pub fn agents_command(rest: &[String], ctx: &Context, config: &Config, style: &Style, json: bool) -> Result<i32> {
    let run = Agents {
        ctx,
        config,
        style,
        json,
    };
    match rest.first().map(String::as_str).unwrap_or("list") {
        "list" => run.list(),
        "use" => run.save(&rest[1..]),
        "setup" => run.setup(rest.get(1).map(String::as_str)),
        action => Err(CliError::hint(format!("Unknown agents command: {action}."), "Run `nexus agents help`.").into()),
    }
}

/// Each agent: Gateway's entry, what this machine has, and the steps left for its route (`report` in the TypeScript).
fn report(
    entry: &Value,
    local: &LocalAgent,
    signed_in: bool,
    agent_subscriptions: Option<bool>,
    without_account: bool,
) -> Value {
    let route = s(entry, "route").and_then(Route::parse).unwrap_or(Route::Gateway);
    let mut steps = setup_steps(
        entry,
        route,
        SetupOptions {
            local: Some(local),
            signed_in: Some(signed_in),
            agent_subscriptions,
            cli_install: None,
        },
    );
    // Signed out with NEXUS_BYO_PROVIDERS, the agent runs on its own sign-in: a Nexus sign-in is not a step.
    if without_account && !signed_in && route == Route::Subscription {
        steps.retain(|step| step.id != "nexus_login");
    }
    let ready = setup_ready(&steps);
    let mut out = entry.as_object().cloned().unwrap_or_default();
    out.insert("local".into(), json!(local));
    out.insert("steps".into(), json!(steps));
    out.insert("ready".into(), json!(ready));
    Value::Object(out)
}

impl Agents<'_> {
    fn print(&self, value: &Value) -> Result<i32> {
        self.ctx.io.out(&pretty(value));
        Ok(0)
    }

    fn out(&self, line: impl AsRef<str>) {
        self.ctx.io.out(line.as_ref());
    }

    fn local(&self, agent: &str) -> LocalAgent {
        // Only the agents this CLI launches are looked for: a name from Gateway never runs a program here. A
        // NEXUS_<AGENT>_BIN that names no file shows as not installed; `nexus claude` names the file.
        let Some(known) = Agent::parse(agent).filter(|agent| agent.entry().session.has_cli_launcher()) else {
            return LocalAgent {
                agent: agent.into(),
                installed: false,
                path: None,
                version: None,
                login: None,
                plan: None,
            };
        };
        let bin = nexus_client::agents::find_agent(known, &self.ctx.env, cfg!(windows), &self.ctx.cwd)
            .ok()
            .flatten();
        local_with(known, bin.as_deref(), &self.ctx.env, &self.ctx.cwd)
    }

    /// Gateway's answer, or what this machine can say without it, and why.
    fn access(&self) -> (Value, bool, Option<String>) {
        let Some(key) = self.config.api_key.as_deref() else {
            return (
                offline_agent_access(),
                false,
                Some(if crate::byo::enabled(self.ctx) {
                    "Signed out: each agent runs on your own plan or key, and Nexus records nothing. Sign in (nexus auth login) for Gateway, your routes and who pays.".into()
                } else {
                    "Signed out: this machine only. Sign in (nexus auth login) to see your routes and who pays.".into()
                }),
            );
        };
        match cli_client(&self.config.url, Some(key)).get("/v1/agents/access") {
            Ok(answer) => (answer, true, None),
            Err(Error::Core(error)) if error.status == 401 => (
                offline_agent_access(),
                false,
                Some("Gateway does not accept your saved sign-in: run nexus auth login.".into()),
            ),
            Err(Error::Core(error)) if error.status == 404 => (
                offline_agent_access(),
                true,
                Some("This Gateway does not keep agent routes yet: who pays is not shown, and nexus agents use is not available.".into()),
            ),
            Err(error) => {
                // What the TypeScript CLI's fetch says.
                let message = match error {
                    Error::Core(error) => error.message,
                    Error::Cli(error) => error.message,
                    Error::Transport(Transport::Unreachable(_)) => "fetch failed".into(),
                    Error::Transport(Transport::Timeout) => "The operation was aborted due to timeout".into(),
                    Error::Transport(Transport::Other(message)) => message,
                };
                (
                    offline_agent_access(),
                    true,
                    Some(format!("Could not reach Gateway ({message}): this machine only.")),
                )
            }
        }
    }

    fn list(&self) -> Result<i32> {
        let (answer, signed_in, note) = self.access();
        let subscriptions = answer.get("agentSubscriptions").and_then(Value::as_bool);
        let agents: Vec<Value> = answer
            .get("agents")
            .and_then(Value::as_array)
            .map(|agents| agents.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(|entry| {
                s(entry, "agent")
                    .and_then(Agent::parse)
                    .is_none_or(|agent| agent.entry().session.has_cli_launcher())
            })
            .map(|entry| {
                report(
                    entry,
                    &self.local(s(entry, "agent").unwrap_or_default()),
                    signed_in,
                    subscriptions,
                    crate::byo::enabled(self.ctx),
                )
            })
            .collect();
        if self.json {
            let mut out = json!({ "signedIn": signed_in });
            if let Some(allowed) = answer.get("agentSubscriptions") {
                out["agentSubscriptions"] = allowed.clone();
            }
            if let Some(note) = &note {
                out["note"] = json!(note);
            }
            out["agents"] = json!(agents);
            return self.print(&out);
        }
        let style = self.style;
        self.out(format!(
            "{} {}",
            style.bold("Coding agents"),
            style.dim(&format!("· {}", launcher_commands()))
        ));
        if let Some(note) = &note {
            self.out(style.dim(note));
        }
        for agent in &agents {
            self.out("");
            for line in agent_lines(style, agent) {
                self.out(line);
            }
        }
        self.out("");
        self.out(style.dim(&format!(
            "Change a route: nexus agents use <{}> <gateway|subscription|default> · one run: nexus claude --api|--subscription",
            Agent::commands("|")
        )));
        Ok(0)
    }

    fn save(&self, args: &[String]) -> Result<i32> {
        // The id or the command (`antigravity` or `agy`); Gateway keeps the id.
        let agent = args
            .first()
            .and_then(|agent| Agent::parse_named(agent))
            .filter(|agent| agent.entry().session.has_cli_launcher());
        let route = args.get(1).map(String::as_str).filter(|route| !route.is_empty());
        let (Some(agent), Some(route)) = (agent, route) else {
            return Err(CliError::usage(
                format!(
                    "Name an agent and a route: nexus agents use <{}> <gateway|subscription|default>.",
                    Agent::commands("|")
                ),
                Some("For example `nexus agents use claude subscription`."),
            )
            .into());
        };
        if route != "default" && Route::parse(route).is_none() {
            return Err(CliError::usage(
                format!("{} is not a route.", quote(route)),
                Some("Use gateway, subscription or default."),
            )
            .into());
        }
        let Some(key) = self.config.api_key.as_deref() else {
            if crate::byo::enabled(self.ctx) {
                return Err(CliError::hint(
                    "Saving a route needs a Nexus account.",
                    format!(
                        "Sign in with `nexus auth login`, or create an account at {}. Without one, `nexus {}` runs on your own plan or key: there is no route to choose.",
                        crate::byo::signup_url(self.ctx, self.config),
                        agent.command()
                    ),
                )
                .with_code("account_required")
                .into());
            }
            return Err(CliError::hint(
                "You are not signed in to Nexus.",
                "Run `nexus auth login`: your saved route is kept in Nexus, for every app you sign in to.",
            )
            .into());
        };
        let body = json!({ agent.as_str(): if route == "default" { Value::Null } else { json!(route) } });
        let answer = match cli_client(&self.config.url, Some(key)).put("/v1/agents/access", body) {
            Err(Error::Core(error)) if error.status == 404 => {
                return Err(CliError::hint(
                    "This Gateway does not keep agent routes yet.",
                    format!(
                        "Set the route for this shell instead: `export NEXUS_{}_AUTH={}`.",
                        agent.as_str().to_uppercase(),
                        if route == "default" { "subscription" } else { route }
                    ),
                )
                .into());
            }
            answer => answer?,
        };
        let entry = agent_entry(Some(&answer), agent);
        let report = report(
            &entry,
            &self.local(agent.as_str()),
            true,
            answer.get("agentSubscriptions").and_then(Value::as_bool),
            false,
        );
        if self.json {
            let mut out = answer.as_object().cloned().unwrap_or_default();
            out.insert("agent".into(), report);
            return self.print(&Value::Object(out));
        }
        let style = self.style;
        let chosen = if route == "default" {
            s(&entry, "route").unwrap_or("subscription")
        } else {
            route
        };
        self.out(format!(
            "{} nexus {} now runs {} when you name no route, in every Nexus app you sign in to.",
            style.ok,
            agent.command(),
            if chosen == "gateway" {
                "through Gateway".to_string()
            } else {
                format!(
                    "on your {} plan",
                    route_of(&entry, Route::Subscription)
                        .and_then(|view| s(view, "plan"))
                        .unwrap_or("undefined")
                )
            }
        ));
        self.out("");
        for line in agent_lines(style, &report) {
            self.out(line);
        }
        Ok(0)
    }

    /// `nexus agents setup <agent>`: the agent's recipe as its docs page says it, limits first. Writes nothing.
    fn setup(&self, agent: Option<&str>) -> Result<i32> {
        let all = recipes::recipes().map_err(|error| {
            CliError::hint(
                format!("The recipes this CLI carries are broken: {error}"),
                "Run `nexus update`; the recipes are checked when the CLI is built.",
            )
        })?;
        let agents: Vec<&str> = all.iter().map(|recipe| recipe.agent.as_str()).collect();
        let Some(agent) = agent else {
            return Err(CliError::usage(
                format!("Name an agent: nexus agents setup <{}>.", agents.join("|")),
                Some(&format!(
                    "For example `nexus agents setup {}`.",
                    agents.first().unwrap_or(&"claude")
                )),
            )
            .into());
        };
        let Some(recipe) = all.iter().find(|recipe| recipe.agent.as_str() == agent) else {
            return Err(CliError::usage(
                format!("{} has no recipe.", serde_json::to_string(agent).unwrap_or_default()),
                Some(&format!(
                    "Recipes: {}. For example `nexus agents setup {}`.",
                    agents.join(", "),
                    agents.first().unwrap_or(&"claude")
                )),
            )
            .into());
        };
        let core_url = if self.config.url.is_empty() {
            DEFAULT_URL
        } else {
            &self.config.url
        };
        let (steps, verify) = recipes::setup(recipe, core_url);
        let limits = recipes::limits(recipe);
        let status = recipes::status(recipe);
        let changes = recipes::changes(recipe);
        if self.json {
            return self.print(&json!({
                "recipe": recipe.id,
                "agent": recipe.agent,
                "title": recipe.title,
                "claim": recipes::claim(recipe),
                "status": status,
                "limits": limits,
                "steps": steps,
                "verify": verify,
                "changes": changes,
            }));
        }
        let style = self.style;
        // The page's sections, in its order and words: tests/recipes.rs holds the two together.
        self.out(style.bold(&recipe.title));
        self.out(format!("{} {}", style.dim("Status:"), recipes::claim(recipe)));
        self.out(format!("  {status}"));
        self.out("");
        self.out(style.bold("Read this first"));
        for limit in &limits {
            self.out(format!("  {} {limit}", style.bullet));
        }
        self.out("");
        self.out(style.bold("Set up and run"));
        for (i, step) in steps.iter().enumerate() {
            let admin = if step.who == "admin" {
                format!("{} ", style.bold("Owner or admin."))
            } else {
                String::new()
            };
            self.out(format!("  {}. {admin}{}", i + 1, step.text));
            if let Some(command) = &step.command {
                self.out(format!("     {}", style.accent(command)));
            }
            if let Some(note) = &step.note {
                self.out(format!("     {}", style.dim(note)));
            }
        }
        self.out("");
        self.out(style.bold("Check it worked"));
        self.out(format!("  {}", verify.text));
        self.out(format!("     {}", style.accent(&verify.command)));
        self.out("");
        self.out(style.bold("What Nexus changes for the run"));
        self.out(format!("  {changes}"));
        Ok(0)
    }
}

fn from_text(from: Option<&str>) -> &'static str {
    match from {
        Some("preference") => "your saved route",
        Some("default") => "the default",
        Some("organization") => "your organization turned subscription mode off",
        _ => "undefined",
    }
}

fn local_line(style: &Style, local: &Value, entry: &Value) -> String {
    if local.get("installed") != Some(&Value::Bool(true)) {
        return format!("{} not installed", style.fail);
    }
    let version = s(local, "version").map(|v| format!(" {v}")).unwrap_or_default();
    let plan = route_of(entry, Route::Subscription)
        .and_then(|view| s(view, "plan"))
        .unwrap_or("undefined");
    let login = match s(local, "login") {
        Some("plan") => format!(
            "{} signed in to your {plan} plan{}",
            style.ok,
            s(local, "plan").map(|p| format!(" ({p})")).unwrap_or_default()
        ),
        Some("api_key") => format!("{} signed in with an API key, not your {plan} plan", style.warn),
        Some("signed_out") => format!("{} not signed in to your {plan} plan", style.dim("○")),
        // Unset on an installed agent: its entry has no sign-in check (`local_with`), so nothing ran.
        None => style.dim("sign-in not checked"),
        _ => style.dim("sign-in unknown"),
    };
    format!("{} installed{version} {} {login}", style.ok, style.bullet)
}

/// One agent's lines in `nexus agents` (`agentLines` in the TypeScript).
pub fn agent_lines(style: &Style, agent: &Value) -> Vec<String> {
    let local = agent.get("local").cloned().unwrap_or_default();
    let mut lines = vec![format!(
        "{} {}",
        style.bold(&pad_end(s(agent, "name").unwrap_or_default(), 12)),
        local_line(style, &local, agent)
    )];
    // As in `report`: a Gateway entry without a route is on Gateway.
    let current = s(agent, "route").and_then(Route::parse).unwrap_or(Route::Gateway);
    let reports_usage = s(agent, "agent")
        .and_then(Agent::parse)
        .is_none_or(|agent| agent.entry().billing.accepts_subscription_usage);
    for route in Route::ALL {
        let view = route_of(agent, route);
        let chosen = current == route;
        let mark = if chosen { style.accent("●") } else { style.dim("○") };
        let label = capitalize(&route_label(agent, route));
        let refused = view.and_then(|view| view.get("available")) == Some(&Value::Bool(false));
        let payer = match (route, refused) {
            (Route::Subscription, true) => "turned off by your organization".to_string(),
            (Route::Subscription, false) => format!(
                "calls straight to {}{}",
                view.and_then(|view| s(view, "vendor")).unwrap_or("undefined"),
                if reports_usage { ", usage reported" } else { "" },
            ),
            // No provider: the agent has no Gateway route at all, and the view says why.
            (Route::Gateway, true) if view.and_then(|view| s(view, "provider")).is_none() => view
                .and_then(|view| s(view, "reason"))
                .unwrap_or("not available")
                .to_string(),
            (Route::Gateway, true) => format!("{}: Gateway refuses its calls", payer_label(view)),
            (Route::Gateway, false) => format!("paid with {}", payer_label(view)),
        };
        let why = if chosen {
            style.dim(&format!(" ({})", from_text(s(agent, "from"))))
        } else {
            String::new()
        };
        lines.push(format!("  {mark} {} {payer}{why}", pad_end(&label, 20)));
    }
    let steps: Vec<Value> = agent
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let left: Vec<&Value> = steps
        .iter()
        .filter(|step| step.get("done") != Some(&Value::Bool(true)))
        .collect();
    if agent.get("ready") == Some(&Value::Bool(true)) {
        lines.push(format!(
            "  {} Ready: {}",
            style.ok,
            style.accent(&format!("nexus {}", command_of(agent)))
        ));
    } else if !left.is_empty() {
        lines.push(format!("  {}", style.dim("To do")));
        for (i, step) in left.iter().enumerate() {
            let unsure = if step.get("done").is_none() {
                style.dim(" (if not done yet)")
            } else {
                String::new()
            };
            let command = s(step, "command")
                .filter(|command| !command.is_empty())
                .map(|command| format!(": {}", style.accent(command)))
                .unwrap_or_default();
            lines.push(format!(
                "    {}. {}{unsure}{command}",
                i + 1,
                s(step, "text").unwrap_or_default()
            ));
        }
        // The other route may already work here: say so, with the one command that switches.
        // The other route may already work here: say so, with the one command that switches (it needs a Nexus sign-in).
        if let Some(other) = Route::ALL.into_iter().find(|route| *route != current) {
            let local: Option<LocalAgent> = local_agent_of(&local);
            let ready = setup_ready(&setup_steps(
                agent,
                other,
                SetupOptions {
                    local: local.as_ref(),
                    signed_in: signed_in(agent),
                    ..SetupOptions::default()
                },
            ));
            if ready == Some(true) {
                lines.push(format!(
                    "    {} {} is ready on this machine: {}",
                    style.dim("Or now:"),
                    route_label(agent, other),
                    style.accent(&format!("nexus agents use {} {}", command_of(agent), other.as_str()))
                ));
            }
        }
    }
    lines
}

/// The command that starts a report's agent (`agy` for `antigravity`); an agent this CLI does not know keeps its id.
fn command_of(agent: &Value) -> String {
    let id = s(agent, "agent").unwrap_or_default();
    Agent::parse(id).map_or_else(|| id.to_string(), |known| known.command().to_string())
}

/// Whether the report found the CLI signed in to Nexus (its `nexus_login` step).
fn signed_in(agent: &Value) -> Option<bool> {
    agent
        .get("steps")
        .and_then(Value::as_array)?
        .iter()
        .find(|step| step["id"] == "nexus_login")?
        .get("done")
        .and_then(Value::as_bool)
}

/// The `local` a report carries, back as what the setup steps read.
fn local_agent_of(local: &Value) -> Option<LocalAgent> {
    let login = match s(local, "login") {
        Some("plan") => Some(Login::Plan),
        Some("api_key") => Some(Login::ApiKey),
        Some("signed_out") => Some(Login::SignedOut),
        Some(_) => Some(Login::Unknown),
        None => None,
    };
    Some(LocalAgent {
        agent: s(local, "agent")?.to_string(),
        installed: local.get("installed") == Some(&Value::Bool(true)),
        path: s(local, "path").map(String::from),
        version: s(local, "version").map(String::from),
        login,
        plan: s(local, "plan").map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_client::agent_setup::agent_catalog;

    fn hint(entry: &Value, signed_in: bool) -> Option<String> {
        let local = LocalAgent {
            agent: "claude".into(),
            installed: true,
            path: None,
            version: None,
            login: Some(Login::Plan),
            plan: None,
        };
        agent_lines(&Style::new(false), &report(entry, &local, signed_in, Some(true), false))
            .into_iter()
            .find(|line| line.contains("Or now:"))
    }

    /// The help names every launcher and every launcher's plan, from the catalog.
    #[test]
    fn help_names_each_launcher_and_its_plan() {
        let help = agents_help(&Style::new(false)).join("\n");
        assert!(
            help.contains(
                "how nexus claude, nexus codex, nexus agy, nexus cursor-agent and nexus grok reach their models"
            ),
            "{help}"
        );
        assert!(
            help.contains("on your own Claude, ChatGPT, Google AI, Cursor or Grok plan"),
            "{help}"
        );
        assert_eq!(prose_list(&["a".into()], "or"), "a");
        assert_eq!(prose_list(&[], "or"), "");
    }

    #[test]
    fn offers_the_other_route_only_when_its_command_can_run() {
        let mut refused = agent_entry(None, Agent::Claude);
        refused["route"] = json!("gateway");
        refused["routes"][0]["available"] = json!(false);
        assert!(hint(&refused, true).is_some_and(|line| line.contains("nexus agents use claude subscription")));
        // Signed out, `nexus agents use` would only say to sign in first.
        assert_eq!(hint(&agent_entry(None, Agent::Claude), false), None);
    }

    #[test]
    fn installed_agent_without_a_sign_in_check_says_not_checked() {
        let entry = agent_entry(None, Agent::Claude);
        let unchecked = json!({ "agent": "claude", "installed": true, "path": "/bin/agy" });
        let line = local_line(&Style::new(false), &unchecked, &entry);
        assert!(line.ends_with("sign-in not checked"), "{line}");
        let unknown = json!({ "agent": "claude", "installed": true, "login": "unknown" });
        assert!(local_line(&Style::new(false), &unknown, &entry).ends_with("sign-in unknown"));
    }

    #[test]
    fn an_entry_without_a_route_is_on_gateway() {
        let mut entry = agent_entry(None, Agent::Claude);
        entry.as_object_mut().map(|entry| entry.shift_remove("route"));
        let local = LocalAgent {
            agent: "claude".into(),
            installed: false,
            path: None,
            version: None,
            login: None,
            plan: None,
        };
        let lines = agent_lines(&Style::new(false), &report(&entry, &local, true, None, false));
        assert!(lines.iter().any(|line| line.starts_with("  ● Gateway")), "{lines:?}");
    }

    /// The offline catalog is built by hand (`agent_setup::catalog_entry`), so it can drift from the facts it copies.
    /// Each agent's name, install command and subscription route say what its entry says, as Gateway's answer does.
    #[test]
    fn signed_out_catalog_matches_each_agents_facts() {
        let catalog = agent_catalog();
        assert_eq!(catalog.len(), Agent::ALL.len());
        for agent in Agent::ALL {
            let fact = agent.entry();
            let entry = catalog
                .iter()
                .find(|entry| entry["agent"] == fact.identity.id)
                .unwrap_or_else(|| panic!("{agent} is in the offline catalog"));
            assert_eq!(
                [&entry["name"], &entry["install"]],
                [fact.identity.name, fact.install.command],
                "{agent}"
            );
            let plan = &entry["routes"][1];
            assert_eq!(
                [&plan["plan"], &plan["vendor"], &plan["login"], &plan["loginStatus"]],
                [
                    fact.identity.plan,
                    fact.identity.vendor,
                    fact.sign_in.login,
                    fact.sign_in.login_status
                ],
                "{agent}"
            );
        }
    }
}
