//! `nexus audit` (`audit.ts`): the organization's audit log, newest first. Owners and admins see every change;
//! a team lead the changes to their teams; everyone else their own. `--csv` writes every matching event as CSV.

use serde_json::Value;

use crate::catalog::AUDIT_OPTIONS;
use crate::cli::Cmd;
use crate::client::search;
use crate::error::{Result, usage};
use crate::json::{js_string, s};
use crate::style::{Style, col, flex, list, pad_end, rows_of};
use crate::teams::js_integer;

pub fn audit_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus audit [options]", style.bold("Usage")),
        String::new(),
        "Who changed what in your organization, from where (API, CLI, Nexus UI, a webhook), newest first.".to_string(),
        String::new(),
    ];
    lines.extend(
        AUDIT_OPTIONS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 18)))),
    );
    lines.push(String::new());
    lines.push(
        style.dim(
            "Owners and admins see everything; a team lead their teams' changes; everyone else their own actions.",
        ),
    );
    lines
}

fn scope_text(scope: &str) -> String {
    match scope {
        "all" => "every change in the organization",
        "team" => "changes to the teams you lead and the teams under them, and your own",
        "own" => "your own changes",
        _ => "undefined",
    }
    .to_string()
}

pub fn audit_command(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let since = match cmd.flag("since") {
        Some(since) => Some(since.to_string()),
        None if cmd.has("since") => None,
        None => Some("7d".to_string()),
    };
    let mut query = vec![
        ("since", since.clone()),
        ("until", cmd.flag("until").map(String::from)),
        ("action", cmd.flag("action").map(String::from)),
        ("actor", cmd.flag("actor").map(String::from)),
        ("team", cmd.flag("team").map(String::from)),
        ("target", cmd.flag("target").map(String::from)),
    ];
    if cmd.has("csv") {
        let csv = cmd.core.get_text(&format!("/v1/audit/export.csv{}", search(&query)))?;
        let trimmed = csv
            .strip_suffix('\n')
            .map(|t| t.strip_suffix('\r').unwrap_or(t))
            .unwrap_or(&csv);
        cmd.out(trimmed);
        return Ok(0);
    }
    if let Some(raw) = cmd.flag("limit") {
        match js_integer(raw) {
            Some(limit) if (1.0..=500.0).contains(&limit) => {
                query.push(("limit", Some(crate::style::js_number(limit))))
            }
            _ => return usage("--limit must be a whole number from 1 to 500.", None),
        }
    }
    if let Some(cursor) = cmd.args.flag_set("cursor") {
        query.push(("cursor", Some(cursor.to_string())));
    }
    let page = cmd.core.get(&format!("/v1/audit{}", search(&query)))?;
    if cmd.json {
        return Ok(cmd.print(&page));
    }
    let scope = scope_text(s(&page, "scope").unwrap_or(""));
    let since_shown = since.filter(|since| !since.is_empty());
    let events = crate::json::a(&page, "events");
    if events.is_empty() {
        cmd.out(style.dim(&format!(
            "No changes{} in {scope}.",
            since_shown.as_ref().map(|since| format!(" since {since}")).unwrap_or_default()
        )));
        return Ok(0);
    }
    cmd.out(format!(
        "{} {} {scope}{}",
        style.bold("Audit log"),
        style.bullet,
        since_shown
            .as_ref()
            .map(|since| format!(" {}", style.dim(&format!("(since {since})"))))
            .unwrap_or_default()
    ));
    cmd.out("");
    let rows: Vec<Vec<String>> = events
        .iter()
        .map(|event| {
            vec![
                style.dim(
                    &s(event, "at")
                        .unwrap_or("")
                        .chars()
                        .take(19)
                        .collect::<String>()
                        .replacen('T', " ", 1),
                ),
                actor_text(event),
                js_string(event.get("action")),
                target_text(event),
                js_string(event.get("surface")),
            ]
        })
        .collect();
    cmd.lines(list(
        style,
        &[col("Time"), col("Actor"), col("Action"), flex("Target"), col("Via")],
        rows_of(rows),
        None,
    ));
    if let Some(next) = s(&page, "nextCursor").filter(|next| !next.is_empty()) {
        cmd.out("");
        cmd.out(style.dim(&format!(
            "More: nexus audit --cursor {next}{}",
            since_shown.as_ref().map(|since| format!(" --since {since}")).unwrap_or_default()
        )));
    }
    Ok(0)
}

fn actor_text(event: &Value) -> String {
    let actor = event.get("actor").cloned().unwrap_or(Value::Null);
    if let Some(email) = s(&actor, "email").filter(|e| !e.is_empty()) {
        return email.to_string();
    }
    if let Some(user) = s(&actor, "userId").filter(|u| !u.is_empty()) {
        return user.to_string();
    }
    match s(&actor, "keyId") {
        Some("github") => "GitHub".into(),
        Some("clerk") => "Clerk".into(),
        _ => js_string(actor.get("kind")),
    }
}

fn target_text(event: &Value) -> String {
    let field = |key: &str| {
        event
            .get(key)
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or(Value::Object(Default::default()))
    };
    let (after, before, metadata) = (field("after"), field("before"), field("metadata"));
    let who = [&after, &before, &metadata]
        .iter()
        .find_map(|value| value.get("email").filter(|v| !v.is_null()))
        .map(|value| js_string(Some(value)));
    let target = event.get("target").cloned().unwrap_or(Value::Null);
    let kind = js_string(target.get("type"));
    if kind == "member" {
        return format!("member {}", who.unwrap_or_else(|| js_string(target.get("id"))));
    }
    let name = s(&target, "slug")
        .map(String::from)
        .unwrap_or_else(|| js_string(target.get("id")));
    let suffix = match (&who, kind.as_str()) {
        (Some(who), "team") if !who.is_empty() => format!(" ({who})"),
        _ => String::new(),
    };
    format!("{kind} {name}{suffix}")
}
