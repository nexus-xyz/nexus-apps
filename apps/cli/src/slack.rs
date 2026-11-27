//! `nexus slack`, as `slack.ts` does it: bind a Slack workspace to the organization through Slack's install,
//! and bind a channel of it to a team, so the team's policy and caps apply there.

use serde_json::{Value, json};

use crate::catalog::{SLACK_CHANNEL_COMMANDS, SLACK_COMMANDS};
use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{CliError, Error, Result, usage};
use crate::json::{a, js_string, s};
use crate::style::{Row, Style, capitalize, col, flex, list};

const STATUS_HINT: &str = "`nexus slack status` lists the workspaces and their ids.";
const NOT_CONFIGURED_HINT: &str = "Ask whoever runs this Gateway to set up the Nexus Slack app.";
/// Who may change the organization's workspaces, and who may bind a channel.
const MANAGERS: &str = "Ask an owner or admin of your organization.";
const CHANNEL_MANAGERS: &str = "Ask an owner, an admin or the team's lead.";
const BIND_USAGE: &str = "Usage: nexus slack channel bind <workspace id> <channel id> --team <slug>";

/// The command names of `nexus slack help --json`.
pub fn slack_command_names() -> Vec<&'static str> {
    SLACK_COMMANDS
        .iter()
        .map(|(name, _)| name.split(' ').next().unwrap_or(name))
        .chain(["channel"])
        .collect()
}

pub fn slack_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus slack <command> [options]", style.bold("Usage")),
        String::new(),
    ];
    for (name, text) in SLACK_COMMANDS {
        lines.push(format!("  {}", style.cyan(name)));
        lines.push(format!("      {text}"));
    }
    for (name, text) in SLACK_CHANNEL_COMMANDS {
        lines.push(format!("  {}", style.cyan(&format!("channel {name}"))));
        lines.push(format!("      {text}"));
    }
    lines.push(String::new());
    lines.push(
        style.dim("Only the person who started an install completes it (in Nexus UI, signed in) and confirms it, within the hour."),
    );
    lines.push(style.dim("A workspace binds to one organization at a time."));
    lines.push(style.dim(
        "To read Slack as yourself, connect your own account: nexus connect slack. nexus slack binds the @nexus bot to the organization.",
    ));
    lines.push(style.dim(
        "A workspace id is the binding's id (chw_…). A channel id is Slack's (C…): open the channel's details in Slack.",
    ));
    lines.push(style.dim("A channel binds to one team. Unbind it before you bind it to another."));
    lines
}

pub fn slack_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let action = args.first().map(String::as_str).unwrap_or("status");
    let rest = args.get(1..).unwrap_or(&[]);
    match action {
        "connect" if cmd.flag("confirm").is_some() || cmd.has("confirm") => confirm(cmd),
        "connect" => connect(cmd),
        "status" => status(cmd),
        "disconnect" => disconnect(rest, cmd),
        "channel" => channel_command(rest, cmd),
        other => usage(
            format!("Unknown command `nexus slack {other}`."),
            Some("Run `nexus slack --help`."),
        ),
    }
}

/// Gateway's words with a capital first letter, unless the first word is a field name.
fn sentence(message: &str) -> String {
    let first = message.split(' ').next().unwrap_or("");
    if !first.is_empty() && first.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'\'') {
        capitalize(message)
    } else {
        message.to_string()
    }
}

/// A refusal prints Gateway's own words, and Slack failing (502) too. A role refusal also says who may (`who`), and a
/// Gateway without the Slack app (503 `not_configured`) who sets it up.
fn asked_by(result: Result<Value>, who: &str) -> Result<Value> {
    match result {
        Err(Error::Core(error))
            if matches!(error.status, 400 | 403 | 404 | 409 | 410 | 502 | 503)
                && error.code.as_deref().is_some_and(|code| !code.is_empty()) =>
        {
            let message = sentence(&error.message);
            match (error.status, error.code.as_deref()) {
                (403, Some("role_required")) => Err(CliError::hint(message, who).into()),
                (503, Some("not_configured")) => Err(CliError::hint(message, NOT_CONFIGURED_HINT).into()),
                _ => Err(CliError::new(message).into()),
            }
        }
        other => other,
    }
}

/// [`asked_by`] for the organization's workspaces: owners and admins.
fn asked(result: Result<Value>) -> Result<Value> {
    asked_by(result, MANAGERS)
}

/// `Acme (T0ACME)`, or the id alone.
fn named(workspace: &Value) -> String {
    let id = js_string(workspace.get("teamId"));
    match s(workspace, "teamName") {
        Some(name) => format!("{name} ({id})"),
        None => id,
    }
}

fn connect(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    if cmd.has("dry-run") {
        if cmd.json {
            return Ok(cmd.print(&json!({ "dryRun": true })));
        }
        cmd.out(format!(
            "{} Would start a Slack install and print Slack's link. Run without --dry-run to start it.",
            style.warn
        ));
        return Ok(0);
    }
    let started = asked(cmd.core.post("/v1/slack/installs", json!({})))?;
    if cmd.json {
        return Ok(cmd.print(&started));
    }
    let id = js_string(started.get("installId"));
    cmd.out(format!(
        "{} Open this link in the browser where you are signed in to Nexus UI, and approve @nexus in your Slack workspace:",
        style.ok
    ));
    cmd.out(format!("  {}", js_string(started.get("authorizeUrl"))));
    cmd.out("");
    cmd.out(
        "Nexus UI completes the install in that browser. Only you can complete it: someone else who approves this link gets nothing.",
    );
    cmd.out(format!(
        "Then bind the workspace: {}",
        style.cyan(&format!("nexus slack connect --confirm {id}"))
    ));
    cmd.out(style.dim(&format!(
        "The link works once, until {}.",
        js_string(started.get("expiresAt"))
    )));
    Ok(0)
}

fn confirm(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let Some(id) = cmd.args.flag_set("confirm") else {
        return usage(
            "Usage: nexus slack connect --confirm <install-id> [--dry-run]",
            Some("`nexus slack connect` prints the install id."),
        );
    };
    let install = asked(cmd.core.get(&format!("/v1/slack/installs/{}", enc(id))))?;
    let status = s(&install, "status").unwrap_or("");
    let Some(workspace) = install.get("workspace").filter(|workspace| workspace.is_object()) else {
        return Err(CliError::new(format!("Gateway answered no workspace for {id}.")).into());
    };
    if status == "revoked" {
        return Err(CliError::hint(
            format!("The install {id} was unbound or replaced."),
            "Start again: nexus slack connect",
        )
        .into());
    }
    if status == "active" {
        if cmd.json {
            return Ok(cmd.print(&install));
        }
        cmd.out(format!(
            "{} {} is already bound to this organization.",
            style.ok,
            named(workspace)
        ));
        return Ok(0);
    }
    if cmd.has("dry-run") {
        if cmd.json {
            let mut shown = install.clone();
            shown["dryRun"] = json!(true);
            return Ok(cmd.print(&shown));
        }
        cmd.out(format!(
            "{} Would bind Slack workspace {} to this organization. Run without --dry-run to bind it.",
            style.warn,
            style.bold(&named(workspace))
        ));
        return Ok(0);
    }
    let bound = asked(
        cmd.core
            .post(&format!("/v1/slack/installs/{}/confirm", enc(id)), json!({})),
    )?;
    if cmd.json {
        return Ok(cmd.print(&bound));
    }
    cmd.out(format!(
        "{} Bound Slack workspace {} to this organization.",
        style.ok,
        style.bold(&named(workspace))
    ));
    Ok(0)
}

fn status(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let answer = asked(cmd.core.get("/v1/slack/workspaces"))?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    let workspaces = a(&answer, "workspaces");
    if workspaces.is_empty() {
        cmd.out(format!(
            "No Slack workspace yet. Connect one: {}",
            style.cyan("nexus slack connect")
        ));
        return Ok(0);
    }
    let rows = workspaces
        .iter()
        .map(|workspace| {
            let state = js_string(workspace.get("state"));
            let since = s(workspace, "confirmedAt")
                .or_else(|| s(workspace, "createdAt"))
                .unwrap_or("")
                .chars()
                .take(10)
                .collect();
            Row {
                cells: vec![
                    js_string(workspace.get("id")),
                    s(workspace, "teamName").unwrap_or("–").to_string(),
                    js_string(workspace.get("teamId")),
                    state.clone(),
                    since,
                ],
                muted: state != "active",
                ..Row::default()
            }
        })
        .collect();
    cmd.lines(list(
        style,
        &[
            col("Id"),
            flex("Workspace"),
            col("Slack id"),
            col("State"),
            col("Since"),
        ],
        rows,
        None,
    ));
    if workspaces
        .iter()
        .any(|workspace| s(workspace, "state") == Some("pending"))
    {
        cmd.out("");
        cmd.out(style.dim("Bind a pending workspace: nexus slack connect --confirm <id>"));
    }
    Ok(0)
}

fn disconnect(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let Some(id) = args.first().filter(|id| !id.is_empty()) else {
        return usage("Usage: nexus slack disconnect <id> [--dry-run]", Some(STATUS_HINT));
    };
    if cmd.has("dry-run") {
        let answer = asked(cmd.core.get("/v1/slack/workspaces"))?;
        let Some(workspace) = a(&answer, "workspaces")
            .iter()
            .find(|workspace| s(workspace, "id") == Some(id.as_str()))
        else {
            return Err(CliError::hint(format!("No Slack workspace {id} in this organization."), STATUS_HINT).into());
        };
        if cmd.json {
            let mut shown = workspace.clone();
            shown["dryRun"] = json!(true);
            return Ok(cmd.print(&shown));
        }
        let what = if s(workspace, "state") == Some("active") {
            "uninstall the app from it and delete its bot token, unless another binding uses it"
        } else {
            "decline its pending install: uninstall the app and delete its bot token, unless another binding uses it"
        };
        cmd.out(format!(
            "{} Would disconnect {}: Gateway would {what}. Run without --dry-run to disconnect it.",
            style.warn,
            style.bold(&named(workspace))
        ));
        return Ok(0);
    }
    let removed = asked(cmd.core.delete(&format!("/v1/slack/workspaces/{}", enc(id))))?;
    if cmd.json {
        return Ok(cmd.print(&removed));
    }
    let uninstalled = removed.get("uninstalled") == Some(&json!(true));
    cmd.out(format!(
        "{} Disconnected {id}{}.",
        style.ok,
        if uninstalled {
            ": the app is uninstalled from the workspace"
        } else {
            ""
        }
    ));
    if removed.get("credentialRevoked") == Some(&json!(false)) {
        cmd.out(style.dim("The bot token was not deleted: another binding of this organization still uses it, or the vault no longer held it."));
    }
    Ok(0)
}

/// `nexus slack channel`: owners and admins bind any channel; a team lead, to a team they lead.
fn channel_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let action = args.first().map(String::as_str).unwrap_or("list");
    let rest = args.get(1..).unwrap_or(&[]);
    match action {
        "list" => list_channels(rest, cmd),
        "bind" => bind_channel(rest, cmd),
        "unbind" => unbind_channel(rest, cmd),
        other => usage(
            format!("Unknown command `nexus slack channel {other}`."),
            Some("Run `nexus slack --help`."),
        ),
    }
}

fn channels_path(workspace: &str, channel: Option<&str>) -> String {
    let base = format!("/v1/slack/workspaces/{}/channels", enc(workspace));
    match channel {
        Some(channel) => format!("{base}/{}", enc(channel)),
        None => base,
    }
}

fn positional(args: &[String], at: usize) -> Option<&str> {
    args.get(at).map(String::as_str).filter(|value| !value.is_empty())
}

fn list_channels(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let Some(workspace) = positional(args, 0) else {
        return usage("Usage: nexus slack channel list <workspace id>", None);
    };
    let answer = asked_by(cmd.core.get(&channels_path(workspace, None)), CHANNEL_MANAGERS)?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    let channels = a(&answer, "channels");
    if channels.is_empty() {
        cmd.out(format!(
            "No channel is bound to a team yet. Bind one: {}",
            style.cyan(&format!(
                "nexus slack channel bind {workspace} <channel id> --team <slug>"
            ))
        ));
        return Ok(0);
    }
    let rows = channels
        .iter()
        .map(|channel| Row {
            cells: vec![
                js_string(channel.get("channelId")),
                s(channel, "teamSlug")
                    .map(String::from)
                    .unwrap_or_else(|| js_string(channel.get("teamId"))),
                js_string(channel.get("createdBy")),
                s(channel, "createdAt")
                    .map(|at| at.chars().take(10).collect())
                    .unwrap_or_default(),
            ],
            ..Row::default()
        })
        .collect();
    cmd.lines(list(
        style,
        &[col("Channel"), flex("Team"), col("Bound by"), col("Bound")],
        rows,
        None,
    ));
    Ok(0)
}

fn bind_channel(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let (Some(workspace), Some(channel), Some(team)) =
        (positional(args, 0), positional(args, 1), cmd.args.flag_set("team"))
    else {
        return usage(BIND_USAGE, Some("Run `nexus team list` to see the team slugs."));
    };
    let body = json!({ "channelId": channel, "team": team });
    let bound = asked_by(cmd.core.post(&channels_path(workspace, None), body), CHANNEL_MANAGERS)?;
    if cmd.json {
        return Ok(cmd.print(&bound));
    }
    let team = s(&bound, "teamSlug")
        .map(String::from)
        .unwrap_or_else(|| js_string(bound.get("teamId")));
    cmd.out(format!(
        "{} Channel {} is bound to team {}: its calls go to that team, under its policy and caps.",
        style.ok,
        style.bold(&js_string(bound.get("channelId"))),
        style.bold(&team)
    ));
    Ok(0)
}

fn unbind_channel(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let (Some(workspace), Some(channel)) = (positional(args, 0), positional(args, 1)) else {
        return usage("Usage: nexus slack channel unbind <workspace id> <channel id>", None);
    };
    let unbound = asked_by(
        cmd.core.delete(&channels_path(workspace, Some(channel))),
        CHANNEL_MANAGERS,
    )?;
    if cmd.json {
        return Ok(cmd.print(&unbound));
    }
    cmd.out(format!(
        "{} Channel {} is no longer bound to a team.",
        style.ok,
        style.bold(&js_string(unbound.get("channelId")))
    ));
    Ok(0)
}
