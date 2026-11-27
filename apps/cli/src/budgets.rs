//! Person budgets (`budgets.ts`): monthly spend limits per person, per calendar month in UTC. Amounts are US
//! dollars a month: `5`, `$5`, `12.50`, `5/month`.

use reqwest::Method;
use serde_json::{Map, Value, json};

use crate::catalog::{BUDGET_COMMANDS, command_rows};
use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{CliError, Result, usage};
use crate::json::{a, js_string, n, n_opt, s, t};
use crate::style::{Row, Style, col, flex, kv, level_tone, list, meter, pad_end, pairs, right, rows_of, usd};

pub fn budgets_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus budgets <command> [options]", style.bold("Usage")),
        String::new(),
    ];
    lines.extend(
        BUDGET_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 40)))),
    );
    lines.push(String::new());
    lines.push(
        style.dim(
            "Limits are per calendar month (UTC) and reset on the 1st. A call that would go over is refused (402).",
        ),
    );
    lines.push(
        style.dim("Amounts in US dollars: 5, $5, 12.50 or 5/month. No default set: people have no limit of their own."),
    );
    lines
}

/// `5`, `$5`, `12.50`, `5/month` → dollars; `clear` → `None`.
fn amount(value: Option<&String>, clear: &str, usage_text: &str) -> Result<Option<f64>> {
    let text = value.map(String::as_str).unwrap_or("");
    if text == clear {
        return Ok(None);
    }
    let body = text.strip_prefix('$').unwrap_or(text);
    let body = body.strip_suffix("/month").unwrap_or(body);
    let valid = match body.split_once('.') {
        Some((int, frac)) => {
            !int.is_empty()
                && int.chars().all(|c| c.is_ascii_digit())
                && (1..=2).contains(&frac.len())
                && frac.chars().all(|c| c.is_ascii_digit())
        }
        None => !body.is_empty() && body.chars().all(|c| c.is_ascii_digit()),
    };
    if !valid {
        return usage(
            format!("Give a monthly amount in US dollars (e.g. 5 or 12.50), or {clear}."),
            Some(&format!("Usage: {usage_text}")),
        );
    }
    Ok(Some(body.parse::<f64>().unwrap_or(0.0)))
}

/// An email, a user id or `me`, as the user id Gateway knows.
fn person_id(cmd: &Cmd, who: &str) -> Result<String> {
    if who == "me" || !who.contains('@') {
        return Ok(who.to_string());
    }
    let response = cmd.core.get("/v1/orgs/members")?;
    let found = a(&response, "members")
        .iter()
        .find(|member| s(member, "email").is_some_and(|email| email.to_lowercase() == who.to_lowercase()));
    match found {
        Some(member) => Ok(js_string(member.get("userId"))),
        None => Err(CliError::hint(
            format!("No member with email {who} in this organization."),
            "Run `nexus members` to see who is in it.",
        )
        .into()),
    }
}

fn person_name(person: &Value) -> String {
    s(person, "email")
        .or_else(|| s(person, "name"))
        .map(String::from)
        .unwrap_or_else(|| js_string(person.get("userId")))
}

/// The role, marked for an admin an owner allowed to set person limits.
fn role_text(style: &Style, person: &Value) -> String {
    let role = js_string(person.get("role"));
    if t(person, "limitManager") {
        format!("{role}{}", style.dim(" (sets limits)"))
    } else {
        role
    }
}

fn limit_text(style: &Style, person: &Value) -> String {
    match n_opt(person, "limitUsd") {
        None => style.dim("no limit"),
        Some(limit) => format!(
            "{}{}",
            usd(limit),
            if s(person, "source") == Some("default") {
                style.dim(" (default)")
            } else {
                String::new()
            }
        ),
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// How much of their limit a person used this month, as a meter; `–` without a limit.
fn used_meter(style: &Style, person: &Value) -> String {
    let Some(limit) = n_opt(person, "limitUsd") else {
        return style.dim("–");
    };
    let share = if limit != 0.0 {
        n(person, "spentUsd") / limit
    } else {
        1.0
    };
    meter(style, share, 10, Some(level_tone(share)))
}

pub fn month_name(period: &str) -> String {
    let mut parts = period.split('-').map(|part| part.parse::<i64>().ok());
    let year = parts.next().flatten().unwrap_or(1970);
    let month = parts.next().flatten().unwrap_or(1);
    let index = (month - 1).rem_euclid(12) as usize;
    let year = year + (month - 1).div_euclid(12);
    format!("{} {year}", MONTHS[index])
}

fn resets(budgets: &Value) -> String {
    s(budgets, "resetsAt").unwrap_or("").chars().take(10).collect()
}

fn header_lines(style: &Style, budgets: &Value) -> Vec<String> {
    let fallback = budgets.get("personDefault").and_then(|d| n_opt(d, "monthlyUsd"));
    let mut lines = vec![format!(
        "{} {} {} (UTC) {} resets {}",
        style.bold("Spend limits"),
        style.bullet,
        month_name(s(budgets, "period").unwrap_or("")),
        style.bullet,
        resets(budgets)
    )];
    lines.extend(pairs(
        style,
        &[kv(
            "Default per person",
            match fallback {
                None => "none (no person limit unless one is set for them)".to_string(),
                Some(value) => format!("{} a month", usd(value)),
            },
        )],
    ));
    lines
}

fn person_lines(style: &Style, person: &Value, budgets: Option<&Value>) -> Vec<String> {
    let mut lines = vec![format!(
        "{} {} {}",
        style.bold(&person_name(person)),
        style.bullet,
        js_string(person.get("role"))
    )];
    let mut entries = vec![
        kv(
            "Limit",
            match n_opt(person, "limitUsd") {
                None => "none".to_string(),
                Some(limit) => format!(
                    "{} a month ({})",
                    usd(limit),
                    if s(person, "source") == Some("override") {
                        "set for them"
                    } else {
                        "the organization's default"
                    }
                ),
            },
        ),
        kv("Spent this month", usd(n(person, "spentUsd"))),
    ];
    if n(person, "reservedUsd") != 0.0 {
        entries.push(kv("Reserved", usd(n(person, "reservedUsd"))));
    }
    entries.push(kv(
        "Left",
        n_opt(person, "remainingUsd").map(usd).unwrap_or_else(|| "–".into()),
    ));
    entries.push(kv(
        "At limit",
        if t(person, "atLimit") {
            style.yellow("yes: their next call is refused")
        } else {
            "no".into()
        },
    ));
    entries.push(kv("Overspends prevented", js_string(person.get("overspendsPrevented"))));
    if let Some(budgets) = budgets {
        entries.push(kv("Resets", format!("{} (UTC)", resets(budgets))));
    }
    lines.extend(pairs(style, &entries));
    lines
}

pub fn budgets_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    let who = args.get(1).filter(|w| !w.is_empty());
    let value = args.get(2);
    if action == "help" || cmd.has("help") {
        if cmd.json {
            return Ok(cmd.print(&json!({ "commands": command_rows("budgets", BUDGET_COMMANDS) })));
        }
        cmd.lines(budgets_help(style));
        return Ok(0);
    }
    match action {
        "list" => {
            let budgets = cmd.core.get("/v1/budgets")?;
            if cmd.json {
                return Ok(cmd.print(&budgets));
            }
            let rows: Vec<Row> = a(&budgets, "people")
                .iter()
                .map(|person| {
                    let cells = vec![
                        person_name(person),
                        role_text(style, person),
                        limit_text(style, person),
                        usd(n(person, "spentUsd")),
                        n_opt(person, "remainingUsd").map(usd).unwrap_or_else(|| style.dim("–")),
                        used_meter(style, person),
                        if t(person, "atLimit") {
                            style.yellow("yes")
                        } else {
                            style.dim("no")
                        },
                        js_string(person.get("overspendsPrevented")),
                    ];
                    Row {
                        cells,
                        mark: t(person, "atLimit").then(|| style.warn.clone()),
                        ..Row::default()
                    }
                })
                .collect();
            cmd.lines(header_lines(style, &budgets));
            cmd.out("");
            cmd.lines(list(
                style,
                &[
                    flex("Person"),
                    col("Role"),
                    right("Limit"),
                    right("Spent"),
                    right("Left"),
                    col("Used"),
                    col("At limit"),
                    right("Prevented"),
                ],
                rows,
                None,
            ));
            if t(&budgets, "canManage") {
                cmd.out("");
                cmd.out(style.dim(
                    "Set the default: nexus budgets default 50. One person: nexus budgets set <email> 100 (default removes it).",
                ));
            }
            Ok(0)
        }
        "show" => {
            let Some(who) = who else {
                return usage("Usage: nexus budgets show <email|userId|me>", None);
            };
            let id = person_id(cmd, who)?;
            let person = cmd.core.get(&format!("/v1/people/{}/budget", enc(&id)))?;
            let budgets = cmd.core.get("/v1/budgets").ok();
            if cmd.json {
                return Ok(cmd.print(&person));
            }
            cmd.lines(person_lines(style, &person, budgets.as_ref()));
            Ok(0)
        }
        "default" => {
            let usage_text = "nexus budgets default <amount|none>";
            if who.is_none() {
                return usage(
                    format!("Usage: {usage_text}"),
                    Some("e.g. nexus budgets default 50, or none to clear it."),
                );
            }
            let monthly = amount(who, "none", usage_text)?;
            let budgets = cmd
                .core
                .put("/v1/budgets/person-default", json!({ "monthlyUsd": monthly }))?;
            if cmd.json {
                return Ok(cmd.print(&budgets));
            }
            cmd.out(match monthly {
                None => format!(
                    "{} Cleared the default limit per person. People without a limit of their own have none.",
                    style.ok
                ),
                Some(value) => format!(
                    "{} Default limit per person: {} a month. It applies to everyone without a limit of their own.",
                    style.ok,
                    usd(value)
                ),
            });
            let at_limit = a(&budgets, "people").iter().filter(|p| t(p, "atLimit")).count();
            if at_limit > 0 {
                cmd.out(style.dim(&format!(
                    "{at_limit} {} at their limit this month: nexus budgets",
                    if at_limit == 1 { "person is" } else { "people are" }
                )));
            }
            Ok(0)
        }
        "set" => {
            let usage_text = "nexus budgets set <email|userId> <amount|default>";
            let (Some(who), Some(_)) = (who, value) else {
                return usage(
                    format!("Usage: {usage_text}"),
                    Some("e.g. nexus budgets set ada@acme.example 100, or default to remove it."),
                );
            };
            let monthly = amount(value, "default", usage_text)?;
            let id = person_id(cmd, who)?;
            let person = cmd.core.put(
                &format!("/v1/people/{}/budget", enc(&id)),
                json!({ "monthlyUsd": monthly }),
            )?;
            if cmd.json {
                return Ok(cmd.print(&person));
            }
            let name = s(&person, "email")
                .map(String::from)
                .unwrap_or_else(|| js_string(person.get("userId")));
            cmd.out(match monthly {
                None => format!(
                    "{} Removed {name}'s own limit: {}.",
                    style.ok,
                    match n_opt(&person, "limitUsd") {
                        None => "they have no limit now".to_string(),
                        Some(limit) => format!("the default, {} a month, applies", usd(limit)),
                    }
                ),
                Some(value) => format!(
                    "{} {name}: {} a month. Spent this month: {}{}.",
                    style.ok,
                    usd(value),
                    usd(n(&person, "spentUsd")),
                    if t(&person, "atLimit") {
                        style.yellow(" (at the limit: their next call is refused)")
                    } else {
                        String::new()
                    }
                ),
            });
            Ok(0)
        }
        "managers" => managers_command(&args[1..], cmd),
        other => usage(
            format!("Unknown command `nexus budgets {other}`."),
            Some("Run `nexus budgets --help`."),
        ),
    }
}

/// Who may set person limits: every owner, then every admin an owner allowed.
fn managers_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    let target = args
        .get(1)
        .map(|target| target.trim())
        .filter(|target| !target.is_empty());
    match action {
        "list" => {
            let budgets = cmd.core.get("/v1/budgets")?;
            if s(&budgets, "scope") != Some("org") {
                return Err(CliError::hint(
                    "Only owners and admins can see who may set person spend limits.",
                    "Ask an owner or admin of your organization.",
                )
                .into());
            }
            let people = a(&budgets, "people");
            let owners = people.iter().filter(|person| s(person, "role") == Some("owner"));
            let allowed = people
                .iter()
                .filter(|person| s(person, "role") == Some("admin") && t(person, "limitManager"));
            let managers: Vec<&Value> = owners.chain(allowed).collect();
            if cmd.json {
                let summaries: Vec<Value> = managers
                    .iter()
                    .map(|person| {
                        let mut summary = Map::new();
                        for key in ["userId", "role", "email", "name"] {
                            if let Some(value) = person.get(key) {
                                summary.insert(key.into(), value.clone());
                            }
                        }
                        Value::Object(summary)
                    })
                    .collect();
                return Ok(cmd.print(&json!({ "managers": summaries })));
            }
            cmd.out(style.bold("Who may set person spend limits"));
            cmd.out("");
            let rows: Vec<Vec<String>> = managers
                .iter()
                .map(|person| vec![person_name(person), js_string(person.get("role"))])
                .collect();
            cmd.lines(list(style, &[flex("Person"), col("Role")], rows_of(rows), None));
            if t(&budgets, "canGrant") {
                cmd.out("");
                cmd.out(style.dim(
                    "Allow an admin: nexus budgets managers add <email|userId>. Stop one: nexus budgets managers remove <email|userId>.",
                ));
            }
            Ok(0)
        }
        "add" | "remove" => {
            let allow = action == "add";
            let usage_text = format!("nexus budgets managers {action} <email|userId>");
            let Some(target) = target else {
                return usage(
                    format!("Usage: {usage_text}"),
                    Some(if allow {
                        "e.g. nexus budgets managers add ada@acme.example. Only an admin can be allowed; owners always may."
                    } else {
                        "e.g. nexus budgets managers remove ada@acme.example."
                    }),
                );
            };
            let path = format!("/v1/budgets/limit-managers/{}", enc(&person_id(cmd, target)?));
            let method = if allow { Method::PUT } else { Method::DELETE };
            let changed = cmd.core.call(method, &path, None)?;
            if cmd.json {
                return Ok(cmd.print(&changed));
            }
            cmd.out(format!(
                "{} {target} is {} allowed to set person spend limits.",
                style.ok,
                if allow { "now" } else { "no longer" }
            ));
            Ok(0)
        }
        other => usage(
            format!("Unknown command `nexus budgets managers {other}`."),
            Some("Run `nexus budgets --help`."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_amounts_and_months() {
        let usage_text = "u";
        assert_eq!(
            amount(Some(&"$12.50".into()), "none", usage_text).ok(),
            Some(Some(12.5))
        );
        assert_eq!(
            amount(Some(&"5/month".into()), "none", usage_text).ok(),
            Some(Some(5.0))
        );
        assert_eq!(amount(Some(&"none".into()), "none", usage_text).ok(), Some(None));
        assert!(amount(Some(&"1.234".into()), "none", usage_text).is_err());
        assert_eq!(month_name("2026-10"), "October 2026");
    }
}
