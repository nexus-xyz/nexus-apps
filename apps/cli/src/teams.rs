//! Teams, analytics and team budgets (`teams.ts`). Owners and admins manage teams; members
//! see them and pick their own default team. Human output never says "tenant": the org is the organization.

use serde_json::{Value, json};

use crate::catalog::{TEAM_COMMANDS, command_rows};
use crate::cli::{Cmd, js_opt};
use crate::client::{enc, search};
use crate::error::{CliError, Error, Result, usage};
use crate::exports;
use crate::json::{a, js_string, n, n_opt, obj, s, t};
use crate::members::{file_emails, invitation_lines, send_invitations};
use crate::style::{
    Row, Style, ago, capitalize, col, iso, js_round, kv, list as list_rows, locale, meter, now_ms, pad_end, pad_start,
    pairs, parse_ms, right, rows_of, usd,
};

pub fn team_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus team <command> [options]", style.bold("Usage")),
        String::new(),
    ];
    lines.extend(
        TEAM_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 52)))),
    );
    lines.push(String::new());
    lines.push(
        style.dim(
            "Name a team for one call with --team <slug> (product runs, nexus claude, nexus codex) or NEXUS_TEAM.",
        ),
    );
    lines.push(style.dim("Levels are your own labels (e.g. --level department); Gateway gives them no meaning."));
    lines.push(style.dim("`lead` is stored for the planned team-lead role and grants nothing extra yet."));
    lines
}

fn need<'a>(value: Option<&'a String>, usage_text: &str) -> Result<&'a str> {
    match value.filter(|value| !value.is_empty()) {
        Some(value) => Ok(value),
        None => usage(
            format!("Usage: nexus team {usage_text}"),
            Some("Run `nexus team --help` for every team command."),
        ),
    }
}

fn dash(style: &Style, value: Option<&str>) -> String {
    value.map(String::from).unwrap_or_else(|| style.dim("–"))
}

pub fn team_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    let slug = args.get(1);
    let third = args.get(2).filter(|third| !third.is_empty());
    if action == "help" || (cmd.has("help") && slug.is_none_or(|s| s.is_empty())) {
        if cmd.json {
            return Ok(cmd.print(&json!({ "commands": command_rows("team", TEAM_COMMANDS) })));
        }
        cmd.lines(team_help(style));
        return Ok(0);
    }

    match action {
        "list" => {
            let response = cmd.core.get("/v1/teams")?;
            let mine = response.get("default").cloned();
            if cmd.json {
                return Ok(cmd.print(&obj(vec![("teams", response.get("teams").cloned()), ("default", mine)])));
            }
            let teams = a(&response, "teams");
            if teams.is_empty() {
                cmd.out(format!(
                    "No teams yet. An owner or admin creates one: {}",
                    style.cyan("nexus team create web --name \"Web\"")
                ));
                return Ok(0);
            }
            let rows: Vec<Row> = teams
                .iter()
                .map(|team| {
                    let slug = js_string(team.get("slug"));
                    let pending = n(team, "pending");
                    let cells = vec![
                        slug,
                        js_string(team.get("name")),
                        dash(style, s(team, "level")),
                        dash(style, s(team, "parent")),
                        format!(
                            "{}{}",
                            js_string(team.get("members")),
                            if pending != 0.0 {
                                style.dim(&format!(" +{} pending", js_string(team.get("pending"))))
                            } else {
                                String::new()
                            }
                        ),
                        match s(team, "myRole").filter(|r| !r.is_empty()) {
                            Some(role) => format!(
                                "{role}{}",
                                if t(team, "isDefault") {
                                    format!(" {}", style.dim("· default"))
                                } else {
                                    String::new()
                                }
                            ),
                            None => style.dim("–"),
                        },
                    ];
                    Row {
                        cells,
                        selected: t(team, "isDefault"),
                        ..Row::default()
                    }
                })
                .collect();
            cmd.lines(list_rows(
                style,
                &[
                    col("Team"),
                    col("Name"),
                    col("Level"),
                    col("Parent"),
                    right("Members"),
                    col("You"),
                ],
                rows,
                None,
            ));
            cmd.out("");
            cmd.out(
                style.dim(&match response
                    .get("default")
                    .and_then(Value::as_str)
                    .filter(|d| !d.is_empty())
                {
                    Some(mine) => format!("Your spend goes to {mine} unless you name another team (--team)."),
                    None => {
                        "You have no default team: your spend is unassigned unless you name one (--team).".to_string()
                    }
                }),
            );
            Ok(0)
        }
        "tree" => {
            let depth = match cmd.flag("depth") {
                None => None,
                Some(raw) => match js_integer(raw) {
                    Some(depth) if depth >= 1.0 => Some(depth),
                    _ => return usage("--depth must be a whole number from 1.", None),
                },
            };
            let path = match depth {
                Some(depth) => format!("/v1/teams?tree=1&depth={}", crate::style::js_number(depth)),
                None => "/v1/teams?tree=1".to_string(),
            };
            let response = cmd.core.get(&path)?;
            if cmd.json {
                return Ok(cmd.print(&obj(vec![
                    ("tree", response.get("tree").cloned()),
                    ("default", response.get("default").cloned()),
                ])));
            }
            let tree = a(&response, "tree");
            if tree.is_empty() {
                cmd.out(format!(
                    "No teams yet. An owner or admin creates one: {}",
                    style.cyan("nexus team create engineering --level department")
                ));
                return Ok(0);
            }
            cmd.lines(tree_lines(style, tree));
            Ok(0)
        }
        "move" => {
            let target = need(slug, "move <slug> --parent <parent> | --root")?;
            let root = cmd.has("root");
            let parent = cmd.args.flag_set("parent");
            if !root && parent.is_none() {
                return usage(
                    "Say where to: --parent <slug>, or --root for the top level.",
                    Some(&format!("e.g. `nexus team move {target} --parent engineering`")),
                );
            }
            let team = cmd.core.patch(
                &format!("/v1/teams/{}", enc(target)),
                json!({ "parent": if root { Value::Null } else { json!(parent) } }),
            )?;
            if cmd.json {
                return Ok(cmd.print(&team));
            }
            cmd.out(format!(
                "{} Moved {} {}. {}",
                style.ok,
                style.bold(&js_string(team.get("slug"))),
                match s(&team, "parent").filter(|p| !p.is_empty()) {
                    Some(parent) => format!("under {}", style.bold(parent)),
                    None => "to the top level".into(),
                },
                style.dim("Its history moves with it; rollups follow the new tree.")
            ));
            Ok(0)
        }
        "pending" => {
            if let Some(cancel) = cmd.args.flag_set("cancel") {
                let query = if cancel.contains('@') {
                    search(&[
                        ("email", Some(cancel.to_string())),
                        ("team", cmd.flag("team").map(String::from)),
                    ])
                } else {
                    search(&[("id", Some(cancel.to_string()))])
                };
                let path = format!(
                    "/v1/teams/pending{}",
                    if query.is_empty() { "?".to_string() } else { query }
                );
                let response = cmd.core.delete(&path)?;
                let cancelled = a(&response, "cancelled");
                if cmd.json {
                    return Ok(cmd.print(&obj(vec![("cancelled", response.get("cancelled").cloned())])));
                }
                cmd.out(format!(
                    "{} Cancelled {} pending membership{}: {}",
                    style.ok,
                    cancelled.len(),
                    if cancelled.len() == 1 { "" } else { "s" },
                    cancelled
                        .iter()
                        .map(|entry| format!("{} in {}", js_string(entry.get("email")), js_string(entry.get("team"))))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                return Ok(0);
            }
            let path = match cmd.args.flag_set("team") {
                Some(team) => format!("/v1/teams/pending?team={}", enc(team)),
                None => "/v1/teams/pending".to_string(),
            };
            let response = cmd.core.get(&path)?;
            if cmd.json {
                return Ok(cmd.print(&obj(vec![("pending", response.get("pending").cloned())])));
            }
            let pending = a(&response, "pending");
            if pending.is_empty() {
                cmd.out(style.dim(
                    "No pending team members. Someone added by an email not in the organization yet shows here until they join.",
                ));
                return Ok(0);
            }
            let rows: Vec<Vec<String>> = pending
                .iter()
                .map(|entry| {
                    vec![
                        js_string(entry.get("email")),
                        js_string(entry.get("team")),
                        js_string(entry.get("role")),
                        if t(entry, "home") {
                            "yes".into()
                        } else {
                            style.dim("–")
                        },
                        ago(s(entry, "createdAt").unwrap_or("")),
                    ]
                })
                .collect();
            cmd.lines(list_rows(
                style,
                &[col("Email"), col("Team"), col("Role"), col("Home"), col("Added")],
                rows_of(rows),
                None,
            ));
            cmd.out("");
            cmd.out(style.dim(
                "They join these teams when they join the organization with that email. Cancel one: nexus team pending --cancel <email>",
            ));
            Ok(0)
        }
        "import" => import(slug, cmd),
        "show" => {
            let team = cmd
                .core
                .get(&format!("/v1/teams/{}", enc(need(slug, "show <slug>")?)))?;
            if cmd.json {
                return Ok(cmd.print(&team));
            }
            cmd.lines(team_lines(style, &team));
            Ok(0)
        }
        "create" => {
            let slug = need(
                slug,
                "create <slug> [--parent P] [--level L] [--name N] [--description D]",
            )?;
            let team = cmd.core.post(
                "/v1/teams",
                obj(vec![
                    ("slug", Some(json!(slug))),
                    ("name", cmd.flag("name").map(|v| json!(v))),
                    ("description", cmd.flag("description").map(|v| json!(v))),
                    ("parent", cmd.args.flag_set("parent").map(|v| json!(v))),
                    ("level", cmd.args.flag_set("level").map(|v| json!(v))),
                ]),
            )?;
            if cmd.json {
                return Ok(cmd.print(&team));
            }
            let slug = js_string(team.get("slug"));
            cmd.out(format!(
                "{} Created team {} {}",
                style.ok,
                style.bold(&slug),
                style.dim(&format!(
                    "· {}{}{}",
                    js_string(team.get("name")),
                    s(&team, "level")
                        .filter(|l| !l.is_empty())
                        .map(|l| format!(" · {l}"))
                        .unwrap_or_default(),
                    s(&team, "parent")
                        .filter(|p| !p.is_empty())
                        .map(|p| format!(" · under {p}"))
                        .unwrap_or_default()
                ))
            ));
            cmd.out(format!(
                "  {} Add people: {}",
                style.arrow,
                style.cyan(&format!("nexus team add {slug} <email>"))
            ));
            Ok(0)
        }
        "rename" => {
            let from = need(slug, "rename <slug> <new-slug> [--name N] [--description D]")?;
            let changes = obj(vec![
                ("slug", third.map(|v| json!(v))),
                ("name", cmd.args.flag_set("name").map(|v| json!(v))),
                ("description", cmd.flag("description").map(|v| json!(v))),
                (
                    "level",
                    cmd.flag("level")
                        .map(|v| if v.is_empty() { Value::Null } else { json!(v) }),
                ),
            ]);
            if changes.as_object().is_none_or(|map| map.is_empty()) {
                return usage(
                    "Nothing to change.",
                    Some(
                        "Give a new slug, --name, --level or --description: `nexus team rename web web-platform --name \"Web platform\"`.",
                    ),
                );
            }
            let team = cmd.core.patch(&format!("/v1/teams/{}", enc(from)), changes)?;
            if cmd.json {
                return Ok(cmd.print(&team));
            }
            cmd.out(format!(
                "{} Team {} {} updated{}",
                style.ok,
                style.bold(&js_string(team.get("slug"))),
                style.dim(&format!("· {}", js_string(team.get("name")))),
                if third.is_some() {
                    format!(
                        " {}",
                        style.dim(&format!("(was {from}; past spend keeps the old slug)"))
                    )
                } else {
                    String::new()
                }
            ));
            Ok(0)
        }
        "delete" => {
            let target = need(slug, "delete <slug> --yes [--cascade]")?;
            let cascade = cmd.has("cascade");
            if !cmd.has("yes") {
                let team = cmd.core.get(&format!("/v1/teams/{}", enc(target)))?;
                let children = a(&team, "children").len();
                let slug = js_string(team.get("slug"));
                let below = if children > 0 {
                    format!(
                        " and every team under it ({children} child team{}{})",
                        if children == 1 { "" } else { "s" },
                        if cascade { "" } else { "; needs --cascade" }
                    )
                } else {
                    String::new()
                };
                return usage(
                    format!(
                        "This deletes team {slug}{below} with their memberships; spend already attributed to them keeps the team."
                    ),
                    Some(&format!(
                        "Run `nexus team delete {slug} --yes{}` to go ahead.",
                        if children > 0 { " --cascade" } else { "" }
                    )),
                );
            }
            let path = format!("/v1/teams/{}{}", enc(target), if cascade { "?cascade=1" } else { "" });
            let deleted = cmd.core.delete(&path)?;
            if cmd.json {
                return Ok(cmd.print(&deleted));
            }
            let count = a(&deleted, "teams").len();
            cmd.out(format!(
                "{} Deleted team {}{}. Past spend keeps the team in analytics.",
                style.ok,
                style.bold(&js_string(deleted.get("slug"))),
                if count > 1 {
                    format!(" and {} team{} under it", count - 1, if count == 2 { "" } else { "s" })
                } else {
                    String::new()
                }
            ));
            Ok(0)
        }
        "add" => {
            let target = need(slug, "add <slug> <email|userId> [--lead] [--home]")?;
            let person = need(args.get(2), "add <slug> <email|userId> [--lead] [--home]")?;
            let added = cmd.core.post(
                &format!("/v1/teams/{}/members", enc(target)),
                obj(vec![
                    (
                        if person.contains('@') { "email" } else { "userId" },
                        Some(json!(person)),
                    ),
                    ("role", Some(json!(if cmd.has("lead") { "lead" } else { "member" }))),
                    ("home", cmd.has("home").then(|| json!(true))),
                ]),
            )?;
            if cmd.json {
                return Ok(cmd.print(&added));
            }
            let team = js_string(added.get("team"));
            if t(&added, "pending") {
                let entry = added.get("entry").cloned().unwrap_or(Value::Null);
                cmd.out(format!(
                    "{} Added {} to {} as {}: pending until they join the organization{}.",
                    style.ok,
                    style.bold(&js_string(entry.get("email"))),
                    style.bold(&team),
                    js_string(entry.get("role")),
                    if t(&entry, "home") {
                        " (then their default team)"
                    } else {
                        ""
                    }
                ));
                cmd.out(format!(
                    "  {} {}",
                    style.arrow,
                    style.dim("They join when they sign in with that email. See every pending one: nexus team pending")
                ));
                return Ok(0);
            }
            let member = added.get("member").cloned().unwrap_or(Value::Null);
            cmd.out(format!(
                "{} Added {} to {} as {}{}",
                style.ok,
                style.bold(
                    &s(&member, "email")
                        .map(String::from)
                        .unwrap_or_else(|| js_string(member.get("userId")))
                ),
                style.bold(&team),
                js_string(member.get("role")),
                if t(&member, "isDefault") {
                    format!(" {}", style.dim("(now their default team)"))
                } else {
                    String::new()
                }
            ));
            Ok(0)
        }
        "remove" => {
            let target = need(slug, "remove <slug> <email|userId>")?;
            let person = need(args.get(2), "remove <slug> <email|userId>")?;
            let team = cmd.core.get(&format!("/v1/teams/{}", enc(target)))?;
            let team_slug = js_string(team.get("slug"));
            let member = a(&team, "memberList").iter().find(|candidate| {
                s(candidate, "userId") == Some(person)
                    || s(candidate, "email").is_some_and(|email| email.to_lowercase() == person.to_lowercase())
            });
            let Some(member) = member else {
                return Err(CliError::hint(
                    format!("{person} is not in team {team_slug}."),
                    format!("See its members: `nexus team show {team_slug}`."),
                )
                .into());
            };
            let user_id = js_string(member.get("userId"));
            let removed = cmd
                .core
                .delete(&format!("/v1/teams/{}/members/{}", enc(&team_slug), enc(&user_id)))?;
            if cmd.json {
                let mut value = removed.clone();
                if let Value::Object(map) = &mut value {
                    map.insert("team".into(), json!(team_slug));
                    map.insert("userId".into(), json!(user_id));
                }
                return Ok(cmd.print(&value));
            }
            cmd.out(format!(
                "{} Removed {} from {}.",
                style.ok,
                style.bold(&s(member, "email").map(String::from).unwrap_or(user_id)),
                style.bold(&team_slug)
            ));
            Ok(0)
        }
        "use" => {
            let target = need(slug, "use <slug>")?;
            let result = cmd.core.put(
                "/v1/people/me/default-team",
                json!({ "team": if target == "none" { Value::Null } else { json!(target) } }),
            )?;
            if cmd.json {
                return Ok(cmd.print(&result));
            }
            match result.get("team").filter(|team| crate::json::truthy(team)) {
                Some(team) => cmd.out(format!(
                    "{} Your default team is now {}: your spend goes there unless you name another (--team).",
                    style.ok,
                    style.bold(&js_string(team.get("slug")))
                )),
                None => cmd.out(format!(
                    "{} You have no default team now: your spend is unassigned unless you name one (--team).",
                    style.ok
                )),
            }
            Ok(0)
        }
        other => usage(
            format!("Unknown command `nexus team {other}`."),
            Some("Run `nexus team --help` for every team command."),
        ),
    }
}

/// `Number(text)` when it is a whole number (`Number.isInteger`), else `None`.
pub fn js_integer(text: &str) -> Option<f64> {
    let value = js_number_of(text)?;
    (value.fract() == 0.0 && value.is_finite()).then_some(value)
}

/// `Number(text)`: whitespace trimmed, empty is 0, otherwise a decimal (or hex) number or `None` (NaN).
pub fn js_number_of(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some(0.0);
    }
    if let Some(hex) = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok().map(|v| v as f64);
    }
    if trimmed.chars().any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E') && !trimmed.ends_with("Infinity") {
        return None;
    }
    match trimmed {
        "Infinity" | "+Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        _ => trimmed.parse::<f64>().ok(),
    }
}

fn import(slug: Option<&String>, cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let file = need(slug, "import <file.yaml|json> [--dry-run] [--plan P] [--prune [--yes]]")?;
    let Ok(text) = cmd.read_file(file) else {
        return usage(
            format!("Cannot read {file}."),
            Some("Give the path to a YAML or JSON file with a `units` list."),
        );
    };
    let units = parse_units(&text, file)?;
    let dry_run = cmd.has("dry-run");
    let prune = cmd.has("prune");
    let plan = cmd.args.flag_set("plan").map(String::from);
    // Pruning deletes teams and memberships: applying it needs a reviewed plan or an explicit --yes.
    if prune && !dry_run && plan.is_none() && !cmd.has("yes") {
        return usage(
            "--prune deletes the teams and memberships the file leaves out.",
            Some(&format!(
                "Review it first: `nexus team import {file} --prune --dry-run`, then apply that exact plan with --plan <plan>, or add --yes."
            )),
        );
    }
    let body = |dry: bool, plan: Option<&String>| {
        obj(vec![
            ("units", Some(units.clone())),
            ("dryRun", Some(json!(dry))),
            ("prune", Some(json!(prune))),
            ("plan", plan.map(|p| json!(p))),
        ])
    };
    if !cmd.has("invite") {
        let diff = cmd.core.post("/v1/teams/import", body(dry_run, plan.as_ref()))?;
        if cmd.json {
            return Ok(cmd.print(&diff));
        }
        cmd.lines(import_lines(style, &diff, file));
        return Ok(0);
    }
    // --invite: after the import, invite the file's people who are still not in the organization (the auth provider emails them).
    let named = file_emails(&units);
    let still_pending = |diff: &Value| -> Result<Vec<String>> {
        let response = cmd.core.get("/v1/teams/pending?by=person")?;
        let mut pending_after: Vec<String> = a(&response, "people")
            .iter()
            .map(|p| js_string(p.get("email")))
            .collect();
        if t(diff, "dryRun") {
            pending_after.extend(
                a(diff, "pending")
                    .iter()
                    .map(|entry| js_string(entry.get("email")).to_lowercase()),
            );
        }
        let mut emails: Vec<String> = named
            .iter()
            .filter(|email| pending_after.contains(email))
            .cloned()
            .collect();
        emails.sort();
        Ok(emails)
    };
    let reviewed = plan.is_some() || cmd.has("yes");
    if !dry_run && !reviewed {
        let preview = cmd.core.post("/v1/teams/import", body(true, None))?;
        let emails = still_pending(&preview)?;
        if !emails.is_empty() {
            let check = cmd
                .core
                .post("/v1/orgs/invitations", json!({ "emails": emails, "dryRun": true }))?;
            if n(&check, "toSend") > 1.0 {
                return usage(
                    format!(
                        "With --invite this import would send {} invitation emails.",
                        js_string(check.get("toSend"))
                    ),
                    Some(&format!(
                        "Review it: `nexus team import {file}{} --invite --dry-run`, then apply with --plan <plan> or --yes. Nothing was imported.",
                        if prune { " --prune" } else { "" }
                    )),
                );
            }
        }
    }
    let diff = cmd.core.post("/v1/teams/import", body(dry_run, plan.as_ref()))?;
    let emails = still_pending(&diff)?;
    let mut invitations: Option<Value> = None;
    let mut failure: Option<CliError> = None;
    if !emails.is_empty() {
        let retry = format!("nexus members invite {}", emails.join(" "));
        match send_invitations(cmd, json!({ "emails": emails }), dry_run, true, &retry) {
            Ok(result) => invitations = Some(result),
            Err(Error::Cli(error)) => failure = Some(error),
            Err(error) => return Err(error),
        }
    }
    if cmd.json {
        let mut value = diff.clone();
        if let Value::Object(map) = &mut value {
            map.insert(
                "invitations".into(),
                match (&invitations, &failure) {
                    (Some(invitations), _) => invitations.clone(),
                    (None, Some(failure)) => json!({ "error": failure.message }),
                    (None, None) => Value::Null,
                },
            );
        }
        cmd.print(&value);
    } else {
        cmd.lines(import_lines(style, &diff, file));
        if let Some(invitations) = &invitations {
            cmd.out("");
            cmd.out(style.bold(if dry_run {
                "Invitations (--invite)"
            } else {
                "Invitations"
            }));
            cmd.lines(invitation_lines(style, invitations));
        } else if failure.is_none() {
            cmd.out("");
            cmd.out(style.dim("--invite: everyone the file names is already in the organization; no invitations."));
        }
    }
    if let Some(mut failure) = failure {
        if !dry_run {
            failure.message = format!(
                "The import was applied, but no invitations were sent: {}",
                failure.message
            );
        }
        return Err(failure.into());
    }
    Ok(
        if invitations
            .as_ref()
            .is_some_and(|i| n(i.get("counts").unwrap_or(&Value::Null), "failed") != 0.0)
        {
            1
        } else {
            0
        },
    )
}

fn team_lines(style: &Style, team: &Value) -> Vec<String> {
    let slug = js_string(team.get("slug"));
    let path = a(team, "path");
    let mut lines = Vec::new();
    if !path.is_empty() {
        let mut steps: Vec<String> = path.iter().map(|step| js_string(step.get("slug"))).collect();
        steps.push(slug.clone());
        lines.push(style.dim(&steps.join(" › ")));
    }
    let level = s(team, "level")
        .filter(|l| !l.is_empty())
        .map(|l| format!(" · {l}"))
        .unwrap_or_default();
    let mine = match s(team, "myRole").filter(|r| !r.is_empty()) {
        Some(role) => format!(
            " {}",
            style.dim(&format!(
                "· you: {role}{}",
                if t(team, "isDefault") { ", default" } else { "" }
            ))
        ),
        None => String::new(),
    };
    lines.push(format!(
        "{} {}{mine}",
        style.bold(&js_string(team.get("name"))),
        style.dim(&format!("· {slug}{level}"))
    ));
    if let Some(description) = s(team, "description").filter(|d| !d.is_empty()) {
        lines.push(format!("  {description}"));
    }
    lines.push(String::new());
    let children = a(team, "children");
    if !children.is_empty() {
        let list = children
            .iter()
            .map(|child| {
                format!(
                    "{}{}",
                    js_string(child.get("slug")),
                    s(child, "level")
                        .filter(|l| !l.is_empty())
                        .map(|l| style.dim(&format!(" ({l})")))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("{}  {list}", style.bold("Child teams")));
        lines.push(String::new());
    }
    let pending = a(team, "pendingList");
    if !pending.is_empty() {
        let list = pending
            .iter()
            .map(|entry| {
                format!(
                    "{}{}",
                    js_string(entry.get("email")),
                    if s(entry, "role") == Some("lead") {
                        style.dim(" (lead)")
                    } else {
                        String::new()
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "{}  {list} {}",
            style.bold("Pending"),
            style.dim("· join when they join the organization")
        ));
        lines.push(String::new());
    }
    let members = a(team, "memberList");
    if members.is_empty() {
        lines.push(style.dim(&format!("No members yet: nexus team add {slug} <email>")));
        return lines;
    }
    let rows: Vec<Vec<String>> = members
        .iter()
        .map(|member| {
            vec![
                s(member, "email")
                    .or_else(|| s(member, "name"))
                    .map(String::from)
                    .unwrap_or_else(|| js_string(member.get("userId"))),
                js_string(member.get("role")),
                if t(member, "isDefault") {
                    "yes".into()
                } else {
                    style.dim("–")
                },
                ago(s(member, "joinedAt").unwrap_or("")),
            ]
        })
        .collect();
    lines.extend(list_rows(
        style,
        &[col("Member"), col("Role"), col("Default"), col("Joined")],
        rows_of(rows),
        None,
    ));
    lines
}

/// The tree, one team per line: slug, name, level, leads and headcount (pending included).
pub fn tree_lines(style: &Style, tree: &[Value]) -> Vec<String> {
    fn walk(style: &Style, nodes: &[Value], prefix: &str, top: bool, lines: &mut Vec<String>) {
        for (i, node) in nodes.iter().enumerate() {
            let last = i == nodes.len() - 1;
            let branch = if top {
                ""
            } else if last {
                "└─ "
            } else {
                "├─ "
            };
            let leads: Vec<String> = a(node, "leads")
                .iter()
                .map(|lead| {
                    format!(
                        "{}{}",
                        s(lead, "email")
                            .or_else(|| s(lead, "name"))
                            .map(String::from)
                            .unwrap_or_else(|| js_string(lead.get("userId"))),
                        if t(lead, "pending") { " (pending)" } else { "" }
                    )
                })
                .collect();
            let slug = js_string(node.get("slug"));
            let name = js_string(node.get("name"));
            let headcount = n(node, "headcount");
            let mut details = Vec::new();
            if name != slug && !name.is_empty() {
                details.push(name);
            }
            if !leads.is_empty() {
                details.push(format!("lead {}", leads.join(", ")));
            }
            details.push(format!(
                "{} {}",
                js_string(node.get("headcount")),
                if headcount == 1.0 { "person" } else { "people" }
            ));
            let mine = match s(node, "myRole").filter(|r| !r.is_empty()) {
                Some(role) => style.cyan(&format!(
                    " ● {role}{}",
                    if t(node, "isDefault") { ", default" } else { "" }
                )),
                None => String::new(),
            };
            lines.push(format!(
                "{}{}{} {}{mine}",
                style.dim(&format!("{prefix}{branch}")),
                style.bold(&slug),
                s(node, "level")
                    .filter(|l| !l.is_empty())
                    .map(|l| format!(" {}", style.dim(&format!("[{l}]"))))
                    .unwrap_or_default(),
                style.dim(&details.join(" · "))
            ));
            let next = if top {
                String::new()
            } else {
                format!("{prefix}{}", if last { "   " } else { "│  " })
            };
            walk(style, a(node, "children"), &next, false, lines);
        }
    }
    let mut lines = Vec::new();
    walk(style, tree, "", true, &mut lines);
    lines
}

/// An import file: YAML or JSON, either `{ units: [...] }` or the list itself.
pub fn parse_units(text: &str, file: &str) -> Result<Value> {
    let is_json = file.to_lowercase().ends_with(".json");
    let data: Value = if is_json {
        serde_json::from_str(text)
            .map_err(|error| Error::Cli(CliError::usage(format!("{file} is not valid JSON: {error}"), None)))?
    } else {
        yaml_serde::from_str(text).map_err(|error| {
            let message = error.to_string();
            Error::Cli(CliError::usage(
                format!("{file} is not valid YAML: {}", message.lines().next().unwrap_or("")),
                None,
            ))
        })?
    };
    let units = if data.is_array() {
        Some(data.clone())
    } else {
        data.get("units").cloned()
    };
    match units {
        Some(Value::Array(list)) if !list.is_empty() => Ok(Value::Array(list)),
        _ => usage(
            format!("{file} has no teams."),
            Some("It needs a `units:` list, each with at least a `slug` (and `parent`, `level`, `leads`, `members`)."),
        ),
    }
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        return "0".into();
    }
    format!(
        "{}  {}{}",
        items.len(),
        items.iter().take(6).cloned().collect::<Vec<_>>().join(", "),
        if items.len() > 6 { ", …" } else { "" }
    )
}

fn import_lines(style: &Style, diff: &Value, file: &str) -> Vec<String> {
    let count = |n: f64, word: &str| {
        format!(
            "{} {word}{}",
            crate::style::js_number(n),
            if n == 1.0 { "" } else { "s" }
        )
    };
    let dry_run = t(diff, "dryRun");
    let head = if dry_run {
        format!(
            "{} Dry run of {file}: nothing changed. This is what an import would do:",
            style.warn
        )
    } else {
        format!("{} Imported {file}.", style.ok)
    };
    let strs = |key: &str| a(diff, key).iter().map(|v| js_string(Some(v))).collect::<Vec<_>>();
    let mapped = |key: &str, f: &dyn Fn(&Value) -> String| a(diff, key).iter().map(f).collect::<Vec<_>>();
    let len = |key: &str| a(diff, key).len().to_string();
    let mut rows = vec![
        kv("Teams created", list(&strs("created"))),
        kv(
            "Moved",
            list(&mapped("moved", &|m| {
                format!("{} → {}", js_string(m.get("slug")), s(m, "to").unwrap_or("top level"))
            })),
        ),
        kv(
            "Renamed",
            list(&mapped("renamed", &|r| {
                format!("{}: {}", js_string(r.get("slug")), js_string(r.get("to")))
            })),
        ),
        kv(
            "Levels changed",
            list(&mapped("relevelled", &|c| {
                format!("{}: {}", js_string(c.get("slug")), s(c, "to").unwrap_or("none"))
            })),
        ),
        kv("Members added", len("membershipsAdded")),
        kv("Roles changed", len("rolesChanged")),
        kv("Pending (not in the org yet)", len("pending")),
        kv("Default teams set", len("defaults")),
    ];
    if t(diff, "prune") {
        rows.push(kv("Memberships removed", len("membershipsRemoved")));
        rows.push(kv("Pending removed", len("pendingRemoved")));
        rows.push(kv("Teams deleted", list(&strs("deleted"))));
    }
    let mut lines = vec![head, String::new()];
    lines.extend(pairs(style, &rows));
    lines.push(String::new());
    let totals = diff.get("totals").cloned().unwrap_or(Value::Null);
    lines.push(style.dim(&format!(
        "{}: {}, {}, {} pending. Spend is never deleted.",
        if dry_run { "Would leave" } else { "Now" },
        count(n(&totals, "teams"), "team"),
        count(n(&totals, "memberships"), "membership"),
        js_string(totals.get("pending"))
    )));
    let warnings = a(diff, "warnings");
    if !warnings.is_empty() {
        lines.push(String::new());
        lines.push(format!("{} {}:", style.warn, count(warnings.len() as f64, "warning")));
        lines.extend(
            warnings
                .iter()
                .take(20)
                .map(|w| format!("  {}", style.dim(&js_string(Some(w))))),
        );
        if warnings.len() > 20 {
            lines.push(style.dim(&format!("  … and {} more (--json shows all)", warnings.len() - 20)));
        }
    }
    if dry_run {
        let plan = js_string(diff.get("plan"));
        lines.push(String::new());
        lines.push(format!("  {} {}", style.dim("Plan"), style.bold(&plan)));
        lines.push(format!(
            "  {} Apply exactly this: {}",
            style.arrow,
            style.cyan(&format!(
                "nexus team import {file}{} --plan {plan}",
                if t(diff, "prune") { " --prune" } else { "" }
            ))
        ));
        lines.push(style.dim("    (Gateway refuses it if the organization changed since this dry run.)"));
    }
    lines
}

// ---------------------------------------------------------------- analytics

const GROUPS: &[(&str, &str)] = &[
    ("team", "Team"),
    ("person", "Person"),
    ("model", "Model"),
    ("provider", "Provider"),
    ("key", "Key"),
    ("product", "Product"),
    ("day", "Day"),
];

/// `--from`, or the first of the last `--days` days up to `--to` (default today). With `file`, an unreadable `--to`
/// gives no `from`: the file's request sends `--to` as typed, and Gateway's 400 names it.
fn from_days(cmd: &Cmd, file: bool) -> Result<Option<String>> {
    let from = cmd.args.flag_set("from").map(String::from);
    if from.is_none()
        && let Some(days_text) = cmd.args.flag_set("days")
    {
        let days = match js_integer(days_text) {
            Some(days) if (1.0..=366.0).contains(&days) => days,
            _ => return usage("--days must be a whole number from 1 to 366.", None),
        };
        let to = match cmd.args.flag_set("to") {
            Some(to) => parse_ms(&format!("{to}T00:00:00Z")),
            None => Some(now_ms()),
        };
        // An invalid --to makes an invalid date in TypeScript, whose toISOString throws.
        let Some(to) = to else {
            if file {
                return Ok(None);
            }
            return Err(Error::Transport(crate::error::Transport::Other(
                "Invalid time value".into(),
            )));
        };
        return Ok(Some(iso(to - (days - 1.0) * 86_400_000.0).chars().take(10).collect()));
    }
    Ok(from)
}

/// `nexus analytics` and `nexus spend --by`: usage over a date range, grouped, with totals and a daily line.
pub fn analytics_command(cmd: &Cmd, group_by: Option<&str>) -> Result<i32> {
    // The usage file, one row per day, team, person, model, provider and product: no grouping applies.
    if let Some(format) = exports::format(cmd)? {
        exports::check_out(cmd)?;
        if ["by", "depth", "under"].iter().any(|name| exports::given(cmd, name)) {
            return usage(
                "--by, --depth and --under do not apply to the file: it has one row per day, team, person, model, provider and product.",
                None,
            );
        }
        let from = from_days(cmd, true)?;
        let query = exports::filters(cmd, exports::USAGE_FILTERS, from);
        return exports::write_file(cmd, "/v1/analytics/usage/export", format, &query);
    }
    let by = group_by.or_else(|| cmd.args.flag_set("by")).unwrap_or("team");
    if !GROUPS.iter().any(|(name, _)| *name == by) {
        return usage(
            format!(
                "--by must be one of: {}.",
                GROUPS.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ")
            ),
            None,
        );
    }
    let from = from_days(cmd, false)?;
    if (cmd.args.flag_set("depth").is_some() || cmd.args.flag_set("under").is_some()) && by != "team" {
        return usage("--depth and --under roll teams up: use them with --by team.", None);
    }
    let query = search(&[
        ("groupBy", Some(by.to_string())),
        ("from", from),
        ("to", cmd.flag("to").map(String::from)),
        ("team", cmd.flag("team").map(String::from)),
        ("depth", cmd.flag("depth").map(String::from)),
        ("under", cmd.flag("under").map(String::from)),
    ]);
    let report = cmd.core.get(&format!("/v1/analytics/usage{query}"))?;
    if cmd.json {
        return Ok(cmd.print(&report));
    }
    cmd.lines(usage_lines(cmd.style, &report));
    Ok(0)
}

pub fn usage_lines(style: &Style, report: &Value) -> Vec<String> {
    let totals = report.get("totals").cloned().unwrap_or(Value::Null);
    let scope = if s(report, "scope") == Some("org") {
        "the whole organization"
    } else {
        "your usage and your teams'"
    };
    let group_by = js_string(report.get("groupBy"));
    let under = report.get("under").filter(|u| crate::json::truthy(u));
    let range = match under {
        Some(under) => {
            let mut steps: Vec<String> = a(under, "path")
                .iter()
                .map(|step| js_string(step.get("slug")))
                .collect();
            steps.push(js_string(under.get("slug")));
            format!(" under {}", steps.join(" › "))
        }
        None if t(report, "depth") => format!(" at depth {}", js_string(report.get("depth"))),
        None => String::new(),
    };
    let team = report
        .get("team")
        .filter(|team| crate::json::truthy(team))
        .map(|team| format!(" {} team {}", style.bullet, js_string(team.get("slug"))))
        .unwrap_or_default();
    let series: Vec<f64> = a(report, "series").iter().map(|day| n(day, "spentUsd")).collect();
    let mut lines = vec![
        format!(
            "{} {} by {group_by}{range} {} {} → {}{team} {}",
            style.bold("Usage"),
            style.bullet,
            style.bullet,
            js_string(report.get("from")),
            js_string(report.get("to")),
            style.dim(&format!("({scope})"))
        ),
        String::new(),
    ];
    lines.extend(pairs(
        style,
        &[
            kv("Spent", style.bold(&usd(n(&totals, "spentUsd")))),
            kv("Calls", locale(n(&totals, "calls"))),
            kv(
                "Tokens",
                format!(
                    "{} in {} {} out",
                    locale(n(&totals, "inputTokens")),
                    style.bullet,
                    locale(n(&totals, "outputTokens"))
                ),
            ),
            kv("Daily", sparkline(&series)),
        ],
    ));
    lines.push(String::new());
    let groups = a(report, "groups");
    if groups.is_empty() {
        lines.push(
            style.dim("No usage in this range. Every metered call shows here, attributed to its team and person."),
        );
        return lines;
    }
    let rows: Vec<Vec<String>> = groups
        .iter()
        .map(|group| {
            let label = js_string(group.get("label"));
            let slug = s(group, "slug").filter(|slug| !slug.is_empty());
            let name = if t(group, "deleted") {
                format!("{label} {}", style.dim("(deleted)"))
            } else if t(group, "direct") {
                format!("{} {}", js_opt(s(group, "slug")), style.dim("(direct)"))
            } else if let Some(slug) = slug.filter(|slug| *slug != label) {
                format!(
                    "{slug}{} {}",
                    if t(group, "hasChildren") { " ›" } else { "" },
                    style.dim(&format!("· {label}"))
                )
            } else {
                label
            };
            let share = n(group, "share");
            vec![
                name,
                locale(n(group, "calls")),
                locale(n(group, "inputTokens")),
                locale(n(group, "outputTokens")),
                usd(n(group, "spentUsd")),
                format!(
                    "{} {}",
                    meter(style, share, 10, None),
                    style.dim(&pad_start(
                        &format!("{}%", crate::style::js_number(js_round(share * 100.0))),
                        4
                    ))
                ),
            ]
        })
        .collect();
    let header = GROUPS
        .iter()
        .find(|(name, _)| *name == group_by)
        .map(|(_, header)| *header)
        .unwrap_or("undefined");
    lines.extend(list_rows(
        style,
        &[
            col(header),
            right("Calls"),
            right("Tokens in"),
            right("Out"),
            right("Spent"),
            col("Share"),
        ],
        rows_of(rows),
        Some(vec![
            "Total".into(),
            locale(n(&totals, "calls")),
            locale(n(&totals, "inputTokens")),
            locale(n(&totals, "outputTokens")),
            usd(n(&totals, "spentUsd")),
            String::new(),
        ]),
    ));
    if group_by == "team" && groups.iter().any(|group| s(group, "key") == Some("unassigned")) {
        lines.push(String::new());
        lines.push(
            style.dim("Unassigned: calls by people with no default team that named none (nexus team use <slug>)."),
        );
    }
    if let Some(slug) = groups
        .iter()
        .find(|group| t(group, "hasChildren"))
        .and_then(|group| s(group, "slug"))
        .filter(|slug| !slug.is_empty())
    {
        lines.push(String::new());
        lines.push(style.dim(&format!(
            "Each team includes the teams under it. Drill in: nexus analytics --by team --under {slug}"
        )));
    }
    lines
}

/// A day per character, scaled to the busiest day; a dot for a day with nothing.
pub fn sparkline(values: &[f64]) -> String {
    const TICKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = values.iter().copied().fold(0.0, f64::max);
    if max == 0.0 {
        return "·".repeat(values.len().min(60));
    }
    let start = values.len().saturating_sub(60);
    values[start..]
        .iter()
        .map(|&value| {
            if value <= 0.0 {
                '·'
            } else {
                TICKS[((value / max) * 7.0).floor().min(7.0) as usize]
            }
        })
        .collect()
}

// ---------------------------------------------------------------- team budgets

/// `team/<slug>` or `person/<email|userId|me>`.
fn target(value: Option<&String>, command: &str) -> Result<(bool, String)> {
    let text = value.map(String::as_str).unwrap_or("");
    for (kind, team) in [("team/", true), ("person/", false)] {
        if let Some(id) = text.strip_prefix(kind).filter(|id| !id.is_empty()) {
            return Ok((team, id.to_string()));
        }
    }
    usage(
        format!("Usage: nexus {command}"),
        Some("Name what it applies to as org, team/<slug> or person/<email|me>."),
    )
}

/// Person budgets are built: `nexus budgets`.
fn person_moved(who: &str, amount: Option<&String>) -> Result<i32> {
    let amount_text = match amount {
        Some(amount) => match amount.rfind('/') {
            Some(at) if amount[at + 1..].chars().all(|c| c.is_alphanumeric() || c == '_') && at + 1 < amount.len() => {
                amount[..at].to_string()
            }
            _ => amount.clone(),
        },
        None => "<amount>".into(),
    };
    usage(
        "Person budgets are monthly spend limits now: use `nexus budgets`.",
        Some(&format!(
            "e.g. nexus budgets set {who} {amount_text} (or nexus budgets show {who})."
        )),
    )
}

/// `a day`, `a week` or `a month`.
fn per(period: &str) -> &'static str {
    match period {
        "day" => "a day",
        "week" => "a week",
        _ => "a month",
    }
}

/// A cap as the answer of `PUT` shows it: `$500.00 a month`.
fn cap_text(cap: &Value) -> String {
    format!(
        "{} {}",
        usd(n(cap, "limitUsd")),
        per(s(cap, "period").unwrap_or("month"))
    )
}

fn team_slug(setting: &Value) -> String {
    js_string(setting.get("team").and_then(|team| team.get("slug")))
}

/// What a reader sees of a team's cap: an owner's or admin's the cap itself, a member's what is left for them.
fn show_lines(style: &Style, setting: &Value, id: &str) -> Vec<String> {
    let head = style.bold(&format!("Team {}", team_slug(setting)));
    let own_cap = s(setting, "scope") == Some("cap");
    let Some(budget) = setting.get("budget").filter(|budget| !budget.is_null()) else {
        return if own_cap {
            vec![
                format!("{head} {} no cap", style.bullet),
                style.dim(&format!("Set one: nexus budget set team/{id} 500/month.")),
            ]
        } else {
            vec![format!("{head} {} no cap applies to you", style.bullet)]
        };
    };
    let resets = kv(
        "Resets",
        format!(
            "{} (UTC)",
            js_string(budget.get("resetsAt")).chars().take(10).collect::<String>()
        ),
    );
    if !own_cap {
        let mut lines = vec![format!(
            "{head} {} {} left for you",
            style.bullet,
            usd(n(budget, "remainingUsd"))
        )];
        lines.extend(pairs(style, &[resets]));
        return lines;
    }
    let mut lines = vec![format!("{head} {} {}, hard stop", style.bullet, cap_text(budget))];
    lines.extend(pairs(
        style,
        &[
            kv("Spent", usd(n(budget, "spentUsd"))),
            kv("Reserved", usd(n(budget, "reservedUsd"))),
            kv("Left", usd(n(budget, "remainingUsd"))),
            resets,
        ],
    ));
    lines
}

/// `500/month` or `$500/month`: the amount and its period.
fn cap_of(amount: Option<&String>) -> Result<(f64, String)> {
    let parsed = amount.and_then(|amount| {
        let text = amount.strip_prefix('$').unwrap_or(amount);
        let (number, period) = text.split_once('/')?;
        let valid_number = !number.is_empty()
            && number
                .split_once('.')
                .map_or(number.chars().all(|c| c.is_ascii_digit()), |(int, frac)| {
                    !int.is_empty()
                        && !frac.is_empty()
                        && int.chars().all(|c| c.is_ascii_digit())
                        && frac.chars().all(|c| c.is_ascii_digit())
                });
        (valid_number && ["day", "week", "month"].contains(&period))
            .then(|| (number.parse::<f64>().unwrap_or(0.0), period.to_string()))
    });
    match parsed {
        Some(cap) => Ok(cap),
        None => usage(
            "Give the budget as <amount>/<period>, e.g. 500/month.",
            Some("Periods: day, week, month."),
        ),
    }
}

/// A change only owners and admins may make: a member's 403 names who can.
fn managed(change: Result<Value>) -> Result<Value> {
    match change {
        Err(Error::Core(error)) if error.status == 403 && error.code.as_deref() == Some("role_required") => {
            Err(CliError::hint(
                capitalize(&error.message),
                "Ask an owner or admin of your organization.",
            )
            .into())
        }
        other => other,
    }
}

/// The organization's budget as the answer of `POST /v1/orgs/budget` shows it, with its period when it has one.
fn org_cap_text(budget: &Value, period_key: &str) -> String {
    match s(budget, period_key) {
        Some(period) => format!("{} {}", usd(n(budget, "budgetUsd")), per(period)),
        None => usd(n(budget, "budgetUsd")),
    }
}

/// What `GET /v1/spend` says of the organization's budget, and nothing else of it.
const ORG_BUDGET_FIELDS: [&str; 7] = [
    "budgetUsd",
    "budgetPeriod",
    "periodSpentUsd",
    "reservedUsd",
    "remainingUsd",
    "period",
    "resetsAt",
];

/// An owner or admin sees the budget this period; a member only what is left for them.
fn org_show_lines(style: &Style, spend: &Value) -> Vec<String> {
    let head = style.bold("Organization");
    let resets: Vec<(String, String)> = s(spend, "resetsAt")
        .map(|at| kv("Resets", format!("{} (UTC)", at.chars().take(10).collect::<String>())))
        .into_iter()
        .collect();
    if n_opt(spend, "budgetUsd").is_none() {
        let mut lines = vec![format!(
            "{head} {} {} left for you",
            style.bullet,
            usd(n(spend, "remainingUsd"))
        )];
        lines.extend(pairs(style, &resets));
        return lines;
    }
    let spent = n_opt(spend, "periodSpentUsd").unwrap_or_else(|| n(spend, "spentUsd"));
    let mut lines = vec![format!(
        "{head} {} {}, hard stop",
        style.bullet,
        org_cap_text(spend, "budgetPeriod")
    )];
    let mut rows = vec![
        kv("Spent", usd(spent)),
        kv("Reserved", usd(n(spend, "reservedUsd"))),
        kv("Left", usd(n(spend, "remainingUsd"))),
    ];
    rows.extend(resets);
    lines.extend(pairs(style, &rows));
    lines
}

/// `nexus budget show org` and `nexus budget set org <amount>/<period> [--dry-run]`.
fn org_budget(show: bool, amount: Option<&String>, cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    if show {
        let spend = cmd.core.get("/v1/spend")?;
        if cmd.json {
            let fields = ORG_BUDGET_FIELDS
                .iter()
                .map(|key| (*key, spend.get(*key).cloned()))
                .collect();
            return Ok(cmd.print(&obj(fields)));
        }
        cmd.lines(org_show_lines(style, &spend));
        return Ok(0);
    }
    if cmd.flag("mode").is_some() || cmd.flag("warn-at").is_some() {
        return usage(
            "The organization's budget takes no --mode or --warn-at: it is a hard stop.",
            Some("Give only the amount: nexus budget set org 500/month."),
        );
    }
    let (amount_usd, period) = cap_of(amount)?;
    let dry_run = cmd.has("dry-run");
    let body = obj(vec![
        ("budgetUsd", Some(json!(amount_usd))),
        ("period", Some(json!(period))),
        ("dryRun", dry_run.then(|| json!(true))),
    ]);
    let answer = managed(cmd.core.post("/v1/orgs/budget", body))?;
    // An older Gateway ignores `dryRun` and sets the budget.
    if dry_run && !t(&answer, "dryRun") {
        return Err(CliError::hint(
            "This Gateway does not take --dry-run for the organization's budget, so it may have set it.",
            "Check it with `nexus budget show org`.",
        )
        .into());
    }
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    if dry_run {
        let side = |key: &str| org_cap_text(answer.get(key).unwrap_or(&Value::Null), "period");
        cmd.out(format!(
            "{} the organization would be capped at {} (now: {}). Nothing changed.",
            style.dim("Dry run:"),
            side("after"),
            side("before")
        ));
    } else {
        cmd.out(format!(
            "{} The organization is capped at {}.",
            style.ok,
            org_cap_text(&answer, "period")
        ));
    }
    Ok(0)
}

pub fn budget_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str);
    let who = args.get(1);
    let amount = args.get(2);
    if matches!(action, Some("show" | "set")) && who.map(String::as_str) == Some("org") {
        return org_budget(action == Some("show"), amount, cmd);
    }
    if action == Some("show") {
        let (team, id) = target(who, "budget show org|team/<slug>")?;
        if !team {
            return person_moved(&id, None);
        }
        let setting = cmd.core.get(&format!("/v1/teams/{}/budget", enc(&id)))?;
        if cmd.json {
            return Ok(cmd.print(&setting));
        }
        cmd.lines(show_lines(style, &setting, &id));
        return Ok(0);
    }
    if action != Some("set") {
        return usage(
            "Usage: nexus budget set org|team/<slug> <amount>/<day|week|month> [--mode hard_stop] [--warn-at 80] [--dry-run]",
            Some("Or: nexus budget show org|team/<slug>."),
        );
    }
    let (team, id) = target(who, "budget set org|team/<slug> <amount>/<day|week|month>")?;
    if !team {
        return person_moved(&id, amount);
    }
    let (amount_usd, period) = cap_of(amount)?;
    // The Gateway refuses `warn`, `approval` and `warnAtPercent` until they are built: refuse them here, before sending.
    if let Some(mode) = cmd.flag("mode").filter(|mode| *mode != "hard_stop") {
        return usage(
            format!("--mode {mode} is not available: a team's cap is a hard stop."),
            Some("Drop --mode, or give --mode hard_stop."),
        );
    }
    if cmd.flag("warn-at").is_some() {
        return usage(
            "--warn-at belongs to mode warn, which is not available yet.",
            Some("Drop --warn-at."),
        );
    }
    let dry_run = cmd.has("dry-run");
    let budget = obj(vec![
        ("amountUsd", Some(json!(amount_usd))),
        ("period", Some(json!(period))),
        ("mode", Some(json!("hard_stop"))),
        ("dryRun", dry_run.then(|| json!(true))),
    ]);
    let change = managed(cmd.core.put(&format!("/v1/teams/{}/budget", enc(&id)), budget))?;
    if cmd.json {
        return Ok(cmd.print(&change));
    }
    let before = change
        .get("before")
        .filter(|before| !before.is_null())
        .map_or_else(|| "no cap".to_string(), cap_text);
    let after = cap_text(change.get("after").unwrap_or(&Value::Null));
    let slug = team_slug(&change);
    if dry_run {
        cmd.out(format!(
            "{} team {slug} would be capped at {after} (now: {before}). Nothing changed.",
            style.dim("Dry run:")
        ));
    } else {
        cmd.out(format!(
            "{} Team {slug} is capped at {after} (was: {before}).",
            style.ok
        ));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparklines_and_numbers() {
        assert_eq!(sparkline(&[0.0, 0.0]), "··");
        assert_eq!(sparkline(&[0.0, 1.0, 0.5]), "·█▄");
        assert_eq!(js_integer("3"), Some(3.0));
        assert_eq!(js_integer("3.5"), None);
        assert_eq!(js_integer("x"), None);
    }

    #[test]
    fn a_team_cap_shows_an_owner_the_cap_and_a_member_only_what_is_left() {
        let style = Style::new(false);
        let owner = json!({
            "team": { "slug": "web", "name": "Web" }, "scope": "cap", "canManage": true,
            "budget": { "limitUsd": 500.0, "period": "month", "mode": "hard_stop", "spentUsd": 120.0,
                "reservedUsd": 3.0, "remainingUsd": 377.0, "resetsAt": "2026-11-01T00:00:00.000Z" },
        });
        let shown = show_lines(&style, &owner, "web").join("\n");
        assert!(
            shown.contains("$500.00 a month, hard stop") && shown.contains("$377.00"),
            "{shown}"
        );
        let member = json!({
            "team": { "slug": "web", "name": "Web" }, "scope": "own", "canManage": false,
            "budget": { "remainingUsd": 12.5, "period": "day", "resetsAt": "2026-10-23T00:00:00.000Z" },
        });
        let shown = show_lines(&style, &member, "web").join("\n");
        assert!(
            shown.contains("$12.50 left for you") && shown.contains("2026-10-23"),
            "{shown}"
        );
        assert!(!shown.contains("a day") && !shown.contains("hard stop"), "{shown}");
        let none = json!({ "team": { "slug": "web", "name": "Web" }, "scope": "own", "budget": null });
        assert_eq!(
            show_lines(&style, &none, "web"),
            vec!["Team web · no cap applies to you"]
        );
        let open = json!({ "team": { "slug": "web", "name": "Web" }, "scope": "cap", "budget": null });
        assert_eq!(
            show_lines(&style, &open, "web")[1],
            "Set one: nexus budget set team/web 500/month."
        );
    }

    #[test]
    fn the_org_budget_shows_an_owner_the_period_and_a_member_only_what_is_left() {
        let style = Style::new(false);
        let owner = json!({
            "budgetUsd": 50.0, "spentUsd": 900.0, "reservedUsd": 1.5, "budgetPeriod": "week",
            "periodSpentUsd": 12.0, "remainingUsd": 36.5, "resetsAt": "2026-11-02T00:00:00.000Z",
        });
        assert_eq!(
            org_show_lines(&style, &owner),
            vec![
                "Organization · $50.00 a week, hard stop",
                "  Spent     $12.00",
                "  Reserved  $1.50",
                "  Left      $36.50",
                "  Resets    2026-11-02 (UTC)",
            ]
        );
        let member = json!({ "remainingUsd": 4.0, "period": "2026-W44", "resetsAt": "2026-11-02T00:00:00.000Z" });
        assert_eq!(
            org_show_lines(&style, &member),
            vec!["Organization · $4.00 left for you", "  Resets  2026-11-02 (UTC)"]
        );
        assert_eq!(
            cap_of(Some(&"$50/week".to_string())).ok(),
            Some((50.0, "week".to_string()))
        );
        assert!(cap_of(Some(&"50".to_string())).is_err());
    }

    #[test]
    fn reads_import_files() {
        let units = parse_units("units:\n  - slug: web\n    leads: [ada@acme.example]\n", "teams.yaml").expect("yaml");
        assert_eq!(units, json!([{ "slug": "web", "leads": ["ada@acme.example"] }]));
        assert!(parse_units("[]", "teams.json").is_err());
        assert!(parse_units("{", "teams.json").is_err());
    }
}
